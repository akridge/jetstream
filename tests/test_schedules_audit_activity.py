"""Scheduled folder syncs, importing desktop audit scans, and live per-file activity."""

import re
import uuid
from datetime import datetime, timedelta, timezone

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from jetstream import database, services
from jetstream.config import settings
from jetstream.database import CloudAuditFinding, SyncPreset, UploadJob
from jetstream.routers import cloud_audit, presets, uploads
from jetstream.schedules import next_run, run_due_presets


@pytest.fixture
def db_env(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "DATABASE_URL", f"sqlite:///{(tmp_path / 'test.db').as_posix()}")
    monkeypatch.chdir(tmp_path)
    database.init_db()
    services.queue_manager.queue.clear()
    yield tmp_path
    services.queue_manager.queue.clear()
    database.close_db()
    database.engine = None
    database.SessionLocal = None


def local_to_utc_naive(dt_local):
    return dt_local.astimezone(timezone.utc).replace(tzinfo=None)


# ----- next_run -------------------------------------------------------------------------

def test_next_run_daily_picks_the_next_matching_day_and_time():
    # Wednesday 10:00 local
    wednesday = datetime(2026, 10, 21, 10, 0).astimezone()
    after = local_to_utc_naive(wednesday)

    later_today = next_run({"mode": "daily", "time": "18:30"}, after)
    assert local_to_utc_naive(wednesday.replace(hour=18, minute=30)) == later_today

    tomorrow = next_run({"mode": "daily", "time": "09:00"}, after)
    assert local_to_utc_naive((wednesday + timedelta(days=1)).replace(hour=9, minute=0)) == tomorrow

    weekdays_only = next_run({"mode": "daily", "time": "09:00", "days": [0]}, after)  # Mondays
    assert local_to_utc_naive((wednesday + timedelta(days=5)).replace(hour=9, minute=0)) == weekdays_only


def test_next_run_hourly_and_off():
    after = datetime(2026, 10, 21, 10, 17, 42)
    assert next_run({"mode": "hourly", "every_hours": 6}, after) == datetime(2026, 10, 21, 16, 17)
    assert next_run({"mode": "off"}, after) is None
    assert next_run(None, after) is None
    assert next_run({"mode": "daily", "time": "25:00"}, after) is None


# ----- run_due_presets ------------------------------------------------------------------

def make_preset(source, schedule, due=True, **options):
    db = database.SessionLocal()
    preset = SyncPreset(
        preset_id=uuid.uuid4().hex,
        name=f"p-{uuid.uuid4().hex[:6]}",
        source_path=str(source),
        destination="bucket/backups/site",
        options={"schedule": schedule, **options},
        next_run_at=datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(minutes=1) if due else None,
    )
    db.add(preset)
    db.commit()
    preset_id = preset.preset_id
    db.close()
    return preset_id


def get_preset(preset_id):
    db = database.SessionLocal()
    preset = db.query(SyncPreset).filter(SyncPreset.preset_id == preset_id).one()
    db.expunge(preset)
    db.close()
    return preset


def get_job(job_id):
    db = database.SessionLocal()
    job = db.query(UploadJob).filter(UploadJob.job_id == job_id).one()
    db.expunge(job)
    db.close()
    return job


def test_due_preset_queues_a_job_and_schedules_the_next_run(db_env):
    source = db_env / "site"
    source.mkdir()
    (source / "a.jpg").write_bytes(b"12345")
    (source / "skip.tmp").write_bytes(b"x")
    preset_id = make_preset(source, {"mode": "hourly", "every_hours": 2}, exclude_patterns="*.tmp", no_clobber=True)

    started = run_due_presets()

    assert len(started) == 1
    job = get_job(started[0])
    assert job.status == "queued"
    assert job.friendly_name.startswith("scheduled-")
    assert (job.destination_bucket, job.destination_path) == ("bucket", "backups/site")
    assert job.total_files == 1 and job.no_clobber is True
    assert job.filters["preset_id"] == preset_id and job.filters["trigger"] == "schedule"
    assert started[0] in services.queue_manager.queue

    preset = get_preset(preset_id)
    assert preset.last_run_job_id == started[0]
    assert preset.next_run_at > datetime.now(timezone.utc).replace(tzinfo=None) + timedelta(minutes=100)

    # Not due again yet.
    assert run_due_presets() == []


def test_due_preset_is_skipped_while_its_last_sync_is_still_running(db_env):
    source = db_env / "site"
    source.mkdir()
    preset_id = make_preset(source, {"mode": "hourly", "every_hours": 1})
    first = run_due_presets()[0]

    db = database.SessionLocal()
    db.query(SyncPreset).filter(SyncPreset.preset_id == preset_id).update(
        {"next_run_at": datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(seconds=1)}
    )
    db.commit()
    db.close()

    assert run_due_presets() == []
    assert get_preset(preset_id).last_run_job_id == first


def test_missing_folder_becomes_a_visible_failed_job(db_env):
    preset_id = make_preset(db_env / "unplugged-drive", {"mode": "daily", "time": "18:00"})
    started = run_due_presets()
    job = get_job(started[0])
    assert job.status == "failed"
    assert "couldn't start" in job.error_message and "does not exist" in job.error_message
    assert get_preset(preset_id).next_run_at is not None


def test_saving_a_preset_with_a_schedule_sets_next_run(db_env):
    app = FastAPI()
    app.include_router(presets.router, prefix="/api/presets")
    with TestClient(app) as client:
        saved = client.post("/api/presets/", json={
            "name": "Nightly", "source_path": str(db_env), "destination": "bucket/x",
            "options": {"schedule": {"mode": "daily", "time": "23:30"}},
        }).json()
        assert saved["next_run_at"].endswith("Z")
        off = client.post("/api/presets/", json={
            "name": "Nightly", "source_path": str(db_env), "destination": "bucket/x", "options": {},
        }).json()
        assert off["next_run_at"] is None


# ----- audit import ---------------------------------------------------------------------

def test_default_junk_patterns_match_real_names():
    names = ["a/b.jpg.gstmp", "Thumbs.db", "x/.DS_Store", "nas/@eaDir/t.jpg", "docs/~$r.docx", "c/d.tmp"]
    for name in names:
        assert any(re.search(p, name, re.I) for p in settings.GCS_JUNK_REGEX_PATTERNS), name
    for name in ["a/photo.jpg", "data/temperature.csv", "notThumbs.dbx"]:
        assert not any(re.search(p, name, re.I) for p in settings.GCS_JUNK_REGEX_PATTERNS), name


def test_imported_audit_run_can_be_quarantined(db_env, monkeypatch):
    app = FastAPI()
    app.include_router(cloud_audit.router, prefix="/api/cloud-audit")
    moved = []

    class Blob:
        def __init__(self, bucket, name):
            self.bucket, self.name, self.size, self.crc32c, self.generation = bucket, name, 10, "crc", 1

        def reload(self):
            pass

        def delete(self, if_generation_match=None):
            moved.append(("delete", self.name))

    class Bucket:
        def __init__(self, name):
            self.name = name

        def blob(self, name):
            return Blob(self, name)

        def copy_blob(self, blob, dest, new_name):
            moved.append(("copy", blob.name, dest.name, new_name))
            return Blob(dest, new_name)

    class Client:
        def bucket(self, name):
            return Bucket(name)

    monkeypatch.setattr(cloud_audit, "get_storage_client", lambda: Client())
    with TestClient(app) as client:
        defaults = client.get("/api/cloud-audit/defaults").json()
        assert r"\.gstmp$" in defaults["patterns"]

        run = client.post("/api/cloud-audit/runs/import", json={
            "bucket_name": "src-bucket", "prefix": "data/", "patterns": defaults["patterns"],
            "scanned_objects": 100, "scanned_bytes": 1000, "junk_objects": 2, "junk_bytes": 20,
            "findings": [
                {"object_name": "data/Thumbs.db", "size": 10, "matched_pattern": r"(^|/)Thumbs\.db$", "updated": "2026-01-02T03:04:05Z"},
                {"object_name": "_critical/x.tmp", "size": 10, "matched_pattern": r"\.(tmp|temp|crdownload|part)$"},
            ],
        }).json()
        assert run["status"] == "completed" and run["junk_objects"] == 2

        result = client.post(f"/api/cloud-audit/runs/{run['run_id']}/quarantine", json={
            "confirm_text": "MOVE_TO_QUARANTINE", "quarantine_bucket": "q-bucket", "dry_run": False,
        }).json()
    assert (result["quarantined"], result["skipped"], result["errors"]) == (1, 1, 0)
    assert ("copy", "data/Thumbs.db", "q-bucket", "quarantine/src-bucket/data/Thumbs.db") in moved
    db = database.SessionLocal()
    statuses = {f.object_name: f.action_status for f in db.query(CloudAuditFinding).all()}
    db.close()
    assert statuses == {"data/Thumbs.db": "quarantined", "_critical/x.tmp": "skipped"}


# ----- per-file activity ----------------------------------------------------------------

def test_job_activity_tracks_in_flight_and_recent_files():
    activity = services.start_job_activity("job-x", "jetstream")
    state = services.EngineRunState()
    state.activity = activity
    state.handle('{"event":"file_started","path":"video/big.mp4","bytes":900}', None)
    state.handle('{"event":"file_uploaded","path":"a.jpg","bytes":5}', None)
    snap = services.job_activity("job-x")
    assert [f["path"] for f in snap["in_flight"]] == ["video/big.mp4"]
    assert snap["recent"][0]["path"] == "a.jpg" and snap["files_done"] == 1

    state.handle('{"event":"file_uploaded","path":"video/big.mp4","bytes":900}', None)
    state.handle('{"event":"file_failed","path":"bad.jpg","error":"403"}', None)
    snap = services.job_activity("job-x")
    assert snap["in_flight"] == []
    assert [f["path"] for f in snap["recent"]] == ["bad.jpg", "video/big.mp4", "a.jpg"]
    assert snap["recent"][0]["error"] == "403" and snap["failed"] == 1


def test_gcloud_copy_lines_become_relative_paths():
    line = "Copying file://C:\\Data\\Survey\\photos\\IMG_1.jpg to gs://bucket/photos/IMG_1.jpg\n"
    match = services._COPY_LINE_RE.search(line)
    assert services._relative_to(match.group(1), "C:\\Data\\Survey") == "photos/IMG_1.jpg"
    gsutil = "Copying file:///home/me/site/a b.jpg [Content-Type=image/jpeg]...\n"
    assert services._relative_to(services._COPY_LINE_RE.search(gsutil).group(1), "/home/me/site/") == "a b.jpg"


def test_activity_endpoint(db_env):
    app = FastAPI()
    app.include_router(uploads.router, prefix="/api/uploads")
    services.start_job_activity("job-y", "gcloud").finished("x.jpg")
    with TestClient(app) as client:
        assert client.get("/api/uploads/job-y/activity").json()["recent"][0]["path"] == "x.jpg"
        assert client.get("/api/uploads/unknown/activity").json()["recent"] == []


# ----- console encoding -----------------------------------------------------------------

def test_server_output_survives_a_non_utf8_pipe():
    """The desktop app reads the server through a pipe, which on Windows uses cp1252."""
    import os
    import subprocess
    import sys

    code = "from jetstream.cli import use_utf8_output; use_utf8_output(); print('\\U0001f680 NOAA JetStream'); print('\\u274c', file=sys.stderr)"
    env = dict(os.environ, PYTHONIOENCODING="cp1252")
    result = subprocess.run([sys.executable, "-c", "import sys; " + code], capture_output=True, env=env)
    assert result.returncode == 0, result.stderr.decode(errors="replace")
    assert "🚀 NOAA JetStream" in result.stdout.decode("utf-8")


# ----- speed limit and pause ------------------------------------------------------------

def test_transfer_settings_round_trip(tmp_path, monkeypatch):
    from jetstream import transfer_settings

    monkeypatch.setenv("JETSTREAM_DATA_DIR", str(tmp_path))
    assert transfer_settings.load() == {"max_mbps": 0.0}
    assert transfer_settings.save({"max_mbps": 25, "unknown": 1}) == {"max_mbps": 25.0}
    assert transfer_settings.load() == {"max_mbps": 25.0}
    (tmp_path / "transfer-settings.json").write_text("not json")
    assert transfer_settings.load() == {"max_mbps": 0.0}


def test_engine_jobs_can_be_paused_resumed_and_limited(tmp_path, monkeypatch):
    import io

    monkeypatch.setenv("JETSTREAM_DATA_DIR", str(tmp_path))

    class FakeProcess:
        def __init__(self):
            self.stdin = io.StringIO()

    service = services.UploadService()
    process = FakeProcess()
    service.active_uploads["job-e"] = process
    service.engine_jobs.add("job-e")
    service.active_uploads["job-g"] = FakeProcess()  # a gcloud job: no control channel
    activity = services.start_job_activity("job-e", "jetstream")

    assert service.pause_upload("job-e") and activity.paused
    assert not service.pause_upload("job-g")
    assert service.resume_upload("job-e") and not activity.paused
    assert service.apply_speed_limit(40) == 1
    sent = [line for line in process.stdin.getvalue().splitlines()]
    assert sent == ['{"cmd": "pause"}', '{"cmd": "resume"}', '{"cmd": "limit", "mbps": 40}']

    # The engine's progress events report whether it's paused.
    state = services.EngineRunState()
    state.activity = activity
    state.handle('{"event":"progress","files_done":1,"files_total":2,"bytes_done":5,"bytes_total":9,"paused":true}', None)
    assert services.job_activity("job-e")["paused"] is True


def test_pause_endpoint_refuses_jobs_it_cannot_pause(db_env):
    app = FastAPI()
    app.include_router(uploads.router, prefix="/api/uploads")
    with TestClient(app) as client:
        response = client.post("/api/uploads/not-running/pause")
    assert response.status_code == 409 and "JetStream engine" in response.json()["detail"]
