"""Regression tests for job-history growth slowing the app down."""

import asyncio
import os
import stat
import sys
import uuid

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient
from sqlalchemy import text

from jetstream import database
from jetstream.config import settings, default_database_url
from jetstream.database import UploadJob
from jetstream.routers import cloud_analyzer, uploads, queue, stats
from jetstream.services import (
    OUTPUT_MAX_CHARS,
    OUTPUT_TAIL_LINES,
    OutputCapture,
    UploadService,
    cap_output,
    claim_job_for_run,
)

BIG_OUTPUT = "Copying file://local/x.jpg to gs://bucket/x.jpg\n" * 20000  # ~1 MB


@pytest.fixture
def db_env(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "DATABASE_URL", f"sqlite:///{(tmp_path / 'test.db').as_posix()}")
    database.init_db()
    yield tmp_path
    database.close_db()
    database.engine = None
    database.SessionLocal = None


@pytest.fixture
def client(db_env):
    app = FastAPI()
    app.include_router(uploads.router, prefix="/api/uploads")
    app.include_router(queue.router, prefix="/api/queue")
    app.include_router(stats.router, prefix="/api/stats")
    app.include_router(cloud_analyzer.router, prefix="/api/cloud")
    with TestClient(app) as test_client:
        yield test_client


def add_job(db, status="completed", output=None, filters=None, **kwargs):
    job = UploadJob(
        job_id=kwargs.pop("job_id", None) or uuid.uuid4().hex,
        status=status,
        source_path="/data/src",
        destination_bucket="bucket",
        upload_output=output,
        filters=filters,
        **kwargs,
    )
    db.add(job)
    db.commit()
    return job


def test_sqlite_uses_wal_and_indexes(db_env):
    with database.engine.connect() as conn:
        assert conn.execute(text("PRAGMA journal_mode")).scalar() == "wal"
        indexes = {row[1] for row in conn.execute(text("PRAGMA index_list('upload_jobs')"))}
    assert {"ix_upload_jobs_status", "ix_upload_jobs_created_at", "ix_upload_jobs_cleared"} <= indexes


def test_list_endpoints_omit_output_but_detail_keeps_it(client):
    db = database.SessionLocal()
    job = add_job(db, output="tail of output")
    job_id = job.job_id
    db.close()

    listed = client.get("/api/uploads/?limit=10").json()
    assert listed[0]["job_id"] == job_id
    assert listed[0]["upload_output"] is None
    assert client.get("/api/stats/recent").json()["recent_jobs"][0]["upload_output"] is None
    assert client.get("/api/queue/history").json()["history"][0]["upload_output"] is None

    assert client.get(f"/api/uploads/{job_id}").json()["upload_output"] == "tail of output"


def test_log_endpoint_serves_full_log_file(client, db_env):
    log_path = db_env / "job.log"
    log_path.write_text(BIG_OUTPUT, encoding="utf-8")
    db = database.SessionLocal()
    job = add_job(db, output="tail", log_path=str(log_path), friendly_name="my-job")
    job_id = job.job_id
    db.close()

    response = client.get(f"/api/uploads/{job_id}/log")
    assert response.status_code == 200
    assert len(response.text) == len(BIG_OUTPUT)
    assert "my-job.log" in response.headers["content-disposition"]


def test_startup_trims_oversized_stored_output(db_env):
    db = database.SessionLocal()
    job_id = add_job(db, output=BIG_OUTPUT).job_id
    small_id = add_job(db, output="small").job_id
    db.close()

    database.close_db()
    database.init_db()

    db = database.SessionLocal()
    trimmed = db.query(UploadJob).filter(UploadJob.job_id == job_id).one().upload_output
    small = db.query(UploadJob).filter(UploadJob.job_id == small_id).one().upload_output
    db.close()
    assert len(trimmed) < OUTPUT_MAX_CHARS + 200
    assert trimmed.startswith("[... earlier output truncated")
    assert trimmed.endswith(BIG_OUTPUT[-100:])
    assert small == "small"


def test_list_transfers_is_limited_and_filtered_in_sql(client, monkeypatch):
    monkeypatch.setattr(cloud_analyzer, "MAX_LISTED_TRANSFERS", 5)
    db = database.SessionLocal()
    for _ in range(8):
        add_job(db, output=BIG_OUTPUT, filters={"job_type": "cloud_sync", "transfer_direction": "cloud_to_cloud"})
    add_job(db, output=BIG_OUTPUT, filters={"job_type": "cloud_move"})
    for _ in range(3):
        add_job(db, output=BIG_OUTPUT, filters={"job_type": "upload"})
    db.close()

    transfers = client.get("/api/cloud/transfers").json()
    assert len(transfers) == 5
    assert all(t["output"] == "" for t in transfers)
    assert all(t["operation"] in ("sync", "move") for t in transfers)


def test_clear_completed_transfers_only_touches_cloud_jobs(client):
    db = database.SessionLocal()
    cloud_id = add_job(db, filters={"job_type": "cloud_sync"}).job_id
    upload_id = add_job(db, filters={"job_type": "upload"}).job_id
    db.close()

    client.post("/api/cloud/transfers/clear-completed")

    db = database.SessionLocal()
    assert db.query(UploadJob).filter(UploadJob.job_id == cloud_id).one().cleared is True
    assert not db.query(UploadJob).filter(UploadJob.job_id == upload_id).one().cleared
    db.close()


def test_output_capture_keeps_bounded_tail_and_streams_full_log(tmp_path):
    log_path = tmp_path / "out.log"
    with open(log_path, "w", encoding="utf-8") as log_file:
        capture = OutputCapture(log_file=log_file)
        for i in range(5000):
            capture.add(f"line {i}\n", "stderr")
    assert len(capture.stderr) == OUTPUT_TAIL_LINES
    assert capture.stderr_text().endswith("line 4999\n")
    assert "earlier line(s) omitted" in capture.summary()
    assert log_path.read_text(encoding="utf-8").count("\n") == 5000


def test_cap_output():
    assert cap_output(None) is None
    assert cap_output("short") == "short"
    capped = cap_output("x" * (OUTPUT_MAX_CHARS * 3))
    assert len(capped) < OUTPUT_MAX_CHARS + 200


@pytest.mark.skipif(sys.platform == "win32", reason="uses a POSIX shell script as a fake gcloud")
def test_upload_streams_output_to_log_and_returns_tail(tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    fake_gcloud = bin_dir / "gcloud"
    fake_gcloud.write_text(
        "#!/bin/sh\n"
        "i=0\n"
        "while [ $i -lt 3000 ]; do echo \"Copying file://src/$i.jpg to gs://b/$i.jpg\" >&2; i=$((i+1)); done\n"
        "echo 'Operation completed over 3000 objects' >&2\n"
    )
    fake_gcloud.chmod(fake_gcloud.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    log_path = tmp_path / "logs" / "job.log"
    success, output = asyncio.run(UploadService().upload_to_gcs(
        job_id="job-1",
        source_path=str(tmp_path),
        destination_bucket="b",
        log_path=str(log_path),
    ))

    assert success is True
    assert len(output) < OUTPUT_MAX_CHARS + 200
    assert "Operation completed over 3000 objects" in output
    log_text = log_path.read_text(encoding="utf-8")
    assert log_text.count("Copying file://") == 3000
    assert "Status:          SUCCESS" in log_text


def test_job_can_only_be_claimed_once(db_env):
    db = database.SessionLocal()
    job_id = add_job(db, status="queued").job_id
    assert claim_job_for_run(db, job_id) is True
    assert claim_job_for_run(db, job_id) is False
    assert db.query(UploadJob).filter(UploadJob.job_id == job_id).one().status == "running"

    cancelled_id = add_job(db, status="cancelled").job_id
    assert claim_job_for_run(db, cancelled_id) is False
    db.close()


def test_default_database_adopts_legacy_cwd_database(tmp_path, monkeypatch):
    data_dir = tmp_path / "data"
    work_dir = tmp_path / "work"
    work_dir.mkdir()
    monkeypatch.setenv("JETSTREAM_DATA_DIR", str(data_dir))
    monkeypatch.chdir(work_dir)
    monkeypatch.setattr(settings, "DATABASE_URL", default_database_url())

    # Build a legacy database in the working directory with one job in it.
    legacy_engine = database.create_db_engine(f"sqlite:///{(work_dir / 'jetstream.db').as_posix()}")
    database.Base.metadata.create_all(bind=legacy_engine)
    with legacy_engine.connect() as conn:
        conn.execute(text(
            "INSERT INTO upload_jobs (job_id, status, source_path, destination_bucket) "
            "VALUES ('legacy-job', 'completed', '/src', 'bucket')"
        ))
        conn.commit()
    with legacy_engine.connect() as conn:
        conn.execute(text("PRAGMA wal_checkpoint(TRUNCATE)"))
    legacy_engine.dispose()

    try:
        database.init_db()
        assert (data_dir / "jetstream.db").exists()
        db = database.SessionLocal()
        assert db.query(UploadJob).filter(UploadJob.job_id == "legacy-job").count() == 1
        db.close()
    finally:
        database.close_db()
        database.engine = None
        database.SessionLocal = None
