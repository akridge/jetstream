"""Tests for live job events, Google sign-in status endpoints and sync presets."""

import asyncio
import json
import threading
import uuid

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from jetstream import database, services
from jetstream.config import settings
from jetstream.database import UploadJob
from jetstream.events import EventBroker
from jetstream.routers import presets, system


@pytest.fixture
def db_env(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "DATABASE_URL", f"sqlite:///{(tmp_path / 'test.db').as_posix()}")
    database.init_db()
    yield tmp_path
    database.close_db()
    database.engine = None
    database.SessionLocal = None


def make_app():
    app = FastAPI()
    app.include_router(system.router, prefix="/api/system")
    app.include_router(presets.router, prefix="/api/presets")
    return app


def add_job(status="running", **kwargs):
    db = database.SessionLocal()
    job = UploadJob(
        job_id=kwargs.pop("job_id", None) or uuid.uuid4().hex,
        status=status,
        source_path="/data/src",
        destination_bucket="bucket",
        destination_path="dest",
        friendly_name="upload-test",
        **kwargs,
    )
    db.add(job)
    db.commit()
    job_id = job.job_id
    db.close()
    return job_id


def set_status(job_id, status):
    db = database.SessionLocal()
    db.query(UploadJob).filter(UploadJob.job_id == job_id).update({"status": status})
    db.commit()
    db.close()


def parse(messages):
    out = []
    for message in messages:
        lines = dict(line.split(": ", 1) for line in message.strip().splitlines())
        out.append((lines["event"], json.loads(lines["data"])))
    return out


def test_job_events_report_progress_then_one_finish(db_env):
    job_id = add_job(status="running", upload_output="x" * 10000)
    last_status = {}
    db = database.SessionLocal()

    events = parse(system.build_job_events(db, [job_id], last_status))
    assert [e for e, _ in events] == ["job"]
    assert events[0][1]["status"] == "running"
    assert events[0][1]["upload_output"] is None

    set_status(job_id, "completed")
    db.expire_all()
    events = parse(system.build_job_events(db, [job_id, job_id], last_status))
    assert [e for e, _ in events] == ["job", "job_finished"]
    finished = events[1][1]
    assert finished["status"] == "completed"
    assert finished["destination"] == "gs://bucket/dest"

    # A later change to an already-finished job does not notify again.
    db.expire_all()
    events = parse(system.build_job_events(db, [job_id], last_status))
    assert [e for e, _ in events] == ["job"]

    db.query(UploadJob).filter(UploadJob.job_id == job_id).delete()
    db.commit()
    events = parse(system.build_job_events(db, [job_id], last_status))
    assert events == [("job_removed", {"job_id": job_id})]
    db.close()


def test_broker_delivers_notifications_from_other_threads():
    async def scenario():
        broker = EventBroker()
        broker.bind_loop()
        queue = broker.subscribe()
        threads = [threading.Thread(target=broker.notify_job, args=(f"job-{i}",)) for i in range(20)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        broker.notify_job("same-thread")
        received = set()
        for _ in range(21):
            received.add(await asyncio.wait_for(queue.get(), timeout=2))
        broker.unsubscribe(queue)
        broker.notify_job("after-unsubscribe")
        return received, queue.empty()

    received, empty_after = asyncio.run(scenario())
    assert received == {f"job-{i}" for i in range(20)} | {"same-thread"}
    assert empty_after


def test_broker_without_loop_or_subscribers_is_a_no_op():
    EventBroker().notify_job("anything")


def test_events_endpoint_streams_hello(db_env):
    with TestClient(make_app()) as client:
        with client.stream("GET", "/api/system/events") as response:
            assert response.headers["content-type"].startswith("text/event-stream")
            lines = []
            for line in response.iter_lines():
                lines.append(line)
                if line.startswith("data:"):
                    break
    assert "event: hello" in lines


def test_auth_status_shape(monkeypatch):
    monkeypatch.setattr(system, "check_gcloud_auth", lambda refresh=False: {
        "gcloud_installed": True, "gcloud_account": "me@example.gov", "adc_available": True,
        "project": "p", "service_account": None, "error": None, "signed_in": True,
    })
    with TestClient(make_app()) as client:
        body = client.get("/api/system/auth-status").json()
    assert body["signed_in"] is True
    assert body["gcloud_account"] == "me@example.gov"
    assert body["login_in_progress"] is False


def test_check_gcloud_auth_reports_missing_sdk(monkeypatch):
    monkeypatch.setattr(services, "find_gcloud", lambda: None)
    services.clear_auth_cache()
    result = services.check_gcloud_auth(force=True)
    assert result["gcloud_installed"] is False
    assert result["gcloud_account"] is None
    assert "signed_in" in result
    services.clear_auth_cache()


def test_auth_login_rejects_remote_clients():
    with TestClient(make_app(), client=("10.1.2.3", 5000)) as client:
        response = client.post("/api/system/auth-login")
    assert response.status_code == 403


def test_auth_login_starts_gcloud_for_local_client(monkeypatch):
    started = []

    class FakeProc:
        def poll(self):
            return None

    monkeypatch.setattr(system, "find_gcloud", lambda: "/usr/bin/gcloud")
    monkeypatch.setattr(system.subprocess, "Popen", lambda cmd, **kw: started.append(cmd) or FakeProc())
    monkeypatch.setitem(system._login_process, "proc", None)
    with TestClient(make_app(), client=("127.0.0.1", 5000)) as client:
        first = client.post("/api/system/auth-login").json()
        second = client.post("/api/system/auth-login").json()
    assert first == {"status": "started"}
    assert second == {"status": "already_running"}
    assert started == [["/usr/bin/gcloud", "auth", "login", "--update-adc"]]


def test_auth_login_without_sdk(monkeypatch):
    monkeypatch.setattr(system, "find_gcloud", lambda: None)
    monkeypatch.setitem(system._login_process, "proc", None)
    with TestClient(make_app(), client=("127.0.0.1", 5000)) as client:
        response = client.post("/api/system/auth-login")
    assert response.status_code == 400
    assert "not installed" in response.json()["detail"]


def test_preset_crud(db_env):
    with TestClient(make_app()) as client:
        created = client.post("/api/presets/", json={
            "name": "Survey photos",
            "source_path": "D:\\Survey\\2026",
            "destination": "my-bucket/surveys/2026",
            "options": {"no_clobber": True},
        }).json()
        assert created["name"] == "Survey photos"

        # Saving the same name updates instead of duplicating.
        updated = client.post("/api/presets/", json={
            "name": "Survey photos",
            "source_path": "D:\\Survey\\2027",
            "destination": "my-bucket/surveys/2027",
        }).json()
        assert updated["preset_id"] == created["preset_id"]

        listed = client.get("/api/presets/").json()
        assert len(listed) == 1
        assert listed[0]["source_path"] == "D:\\Survey\\2027"
        assert listed[0]["options"] == {}

        assert client.delete(f"/api/presets/{created['preset_id']}").status_code == 200
        assert client.get("/api/presets/").json() == []
        assert client.delete("/api/presets/missing").status_code == 404
