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
    # Files are counted when the job runs, not when it is queued.
    assert job.total_files == 0 and job.no_clobber is True
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
    defaults = {"max_mbps": 0.0, "parallel_jobs": 1, "parallel_parts": True}
    assert transfer_settings.load() == defaults
    assert transfer_settings.save({"max_mbps": 25, "unknown": 1}) == {**defaults, "max_mbps": 25.0}
    assert transfer_settings.save({"parallel_jobs": 9})["parallel_jobs"] == 4
    assert transfer_settings.load() == {**defaults, "max_mbps": 25.0, "parallel_jobs": 4}
    (tmp_path / "transfer-settings.json").write_text("not json")
    assert transfer_settings.load() == defaults


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

    # A dropped connection: shown while it lasts, and mentioned in the summary.
    state.handle('{"event":"offline","error":"network error: connection refused"}', None)
    snap = services.job_activity("job-e")
    assert snap["offline"] is True and snap["offline_seconds"] == 0
    state.handle('{"event":"online","offline_seconds":42}', None)
    assert services.job_activity("job-e")["offline"] is False
    state.handle('{"event":"done","uploaded":3,"failed":0,"unchanged":0,"elapsed_seconds":50}', None)
    assert "connection dropped 1 time(s) (42 s in all)" in state.summary_text(0)


def test_pause_endpoint_refuses_jobs_it_cannot_pause(db_env):
    app = FastAPI()
    app.include_router(uploads.router, prefix="/api/uploads")
    with TestClient(app) as client:
        response = client.post("/api/uploads/not-running/pause")
    assert response.status_code == 409 and "JetStream engine" in response.json()["detail"]


# ----- split by subfolder ---------------------------------------------------------------

def _tree(root):
    for rel, size in [("top.txt", 5), ("photos/a.jpg", 10), ("photos/deep/b.jpg", 20), ("video/v.mp4", 30), ("empty/.keep_dir", 0)]:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * size)
    (root / "empty" / ".keep_dir").unlink()  # an empty subfolder


def test_split_upload_creates_a_job_per_subfolder_plus_root_files(db_env, monkeypatch):
    from jetstream.routers import uploads as uploads_router

    started = []
    monkeypatch.setattr(uploads_router, "start_queued_jobs", lambda: started.append(True) or 0)
    source = db_env / "site"
    _tree(source)
    app = FastAPI()
    app.include_router(uploads.router, prefix="/api/uploads")
    with TestClient(app) as client:
        response = client.post("/api/uploads/", json={
            "source_path": str(source), "gcs_destination": "bucket/backups/site", "split_by_folder": True,
            "upload_tool": "jetstream",
        })
        assert response.status_code == 200, response.text
        assert response.json()["job_id"].startswith("group:")
        db = database.SessionLocal()
        jobs = sorted(db.query(UploadJob).all(), key=lambda j: j.destination_path)
        db.close()
        parts = [(j.destination_path, j.recursive, j.total_files, j.filters["group_part"], j.status) for j in jobs]
        # Only the top level is listed (empty subfolders skipped); each job counts its own
        # files when it runs, so the request answers at once for huge folders.
        assert parts == [
            ("backups/site", False, 0, "", "queued"),  # files directly in the folder
            ("backups/site/photos", True, 0, "photos", "queued"),
            ("backups/site/video", True, 0, "video", "queued"),
        ]
        group = jobs[0].filters["group"]
        assert group["source_path"] == str(source) and group["parts"] == 3 and group["destination"] == "bucket/backups/site"
        assert all(j.filters["group"]["id"] == group["id"] for j in jobs)
        assert started, "queued jobs are started right away"

        # Cancel the whole group, then retry what didn't finish.
        services.queue_manager.queue.clear()
        cancelled = client.post(f"/api/uploads/groups/{group['id']}/cancel").json()
        assert cancelled["cancelled"] == 3
        retried = client.post(f"/api/uploads/groups/{group['id']}/retry-failed").json()
        assert retried["retried"] == 3
        assert len(services.queue_manager.queue) == 3
        assert client.post("/api/uploads/groups/nope/cancel").status_code == 404

        empty = db_env / "nothing"
        (empty / "sub").mkdir(parents=True)
        bad = client.post("/api/uploads/", json={"source_path": str(empty), "gcs_destination": "bucket/x", "split_by_folder": True})
        assert bad.status_code == 400 and "Nothing to upload" in bad.json()["detail"]


def test_scheduled_split_sync_queues_the_group_and_blocks_overlap(db_env):
    source = db_env / "site"
    _tree(source)
    preset_id = make_preset(source, {"mode": "hourly", "every_hours": 1}, split_by_folder=True)
    started = run_due_presets()
    assert len(started) == 1
    db = database.SessionLocal()
    jobs = db.query(UploadJob).all()
    db.close()
    assert len(jobs) == 3 and all(j.filters["preset_id"] == preset_id for j in jobs)
    assert all(j.filters["trigger"] == "schedule" for j in jobs)
    assert len(services.queue_manager.queue) == 3

    db = database.SessionLocal()
    db.query(SyncPreset).filter(SyncPreset.preset_id == preset_id).update(
        {"next_run_at": datetime.now(timezone.utc).replace(tzinfo=None) - timedelta(seconds=1)}
    )
    db.commit()
    db.close()
    assert run_due_presets() == [], "a running split sync isn't started twice"


def test_cancelled_jobs_stay_cancelled_and_engine_stop_is_graceful(db_env, monkeypatch):
    import io

    class FakeProcess:
        def __init__(self):
            self.stdin = io.StringIO()
            self.killed = False

        def wait(self, timeout=None):
            return 3  # exited on its own after "stop"

        def kill(self):
            self.killed = True

    service = services.UploadService()
    process = FakeProcess()
    service.active_uploads["job-s"] = process
    service.engine_jobs.add("job-s")
    assert service.cancel_upload("job-s")
    assert process.stdin.getvalue().strip() == '{"cmd": "stop"}'
    import time
    time.sleep(0.1)
    assert not process.killed


# ----- starting an upload answers at once and can't start twice -------------------------

def test_starting_an_upload_never_walks_the_folder_and_refuses_duplicates(db_env, monkeypatch):
    from jetstream.routers import uploads as uploads_router

    def no_walk(*args, **kwargs):
        raise AssertionError("the request must not walk the folder")

    monkeypatch.setattr(services.FolderAnalyzer, "analyze", no_walk)
    monkeypatch.setattr(services.FolderAnalyzer, "analyze_subfolders", no_walk)
    monkeypatch.setattr(services, "count_files", no_walk)
    ran = []

    async def fake_process(job_id, db_path):
        ran.append(job_id)

    monkeypatch.setattr(uploads_router, "process_upload_job", fake_process)
    monkeypatch.setattr(uploads_router, "start_queued_jobs", lambda: 0)
    source = db_env / "site"
    _tree(source)
    app = FastAPI()
    app.include_router(uploads.router, prefix="/api/uploads")
    payload = {"source_path": str(source), "gcs_destination": "bucket/backups/site", "upload_tool": "gcloud", "dry_run": True}
    with TestClient(app) as client:
        first = client.post("/api/uploads/", json=payload).json()
        assert not first["already_running"]
        job = get_job(first["job_id"])
        assert job.total_files == 0, "files are counted when the job runs"

        # A second click (preview or sync) while it runs returns the same job.
        second = client.post("/api/uploads/", json={**payload, "dry_run": False}).json()
        assert second["already_running"] and second["job_id"] == first["job_id"] and second["dry_run"] is True
        # Another destination is a different upload.
        other = client.post("/api/uploads/", json={**payload, "gcs_destination": "bucket/elsewhere"}).json()
        assert not other["already_running"] and other["job_id"] != first["job_id"]

        # Split uploads: the second click returns the group.
        split = {**payload, "gcs_destination": "bucket/split", "split_by_folder": True}
        group = client.post("/api/uploads/", json=split).json()
        again = client.post("/api/uploads/", json=split).json()
        assert group["job_id"].startswith("group:") and again["already_running"] and again["job_id"] == group["job_id"]

        # Once it has finished, the same folder starts again.
        db = database.SessionLocal()
        db.query(UploadJob).filter(UploadJob.job_id == first["job_id"]).update({UploadJob.status: "completed"})
        db.commit()
        db.close()
        third = client.post("/api/uploads/", json=payload).json()
        assert not third["already_running"] and third["job_id"] != first["job_id"]


def test_jobs_count_their_files_when_they_run(db_env):
    import asyncio

    source = db_env / "site"
    _tree(source)
    (source / "skip.tmp").write_bytes(b"x")
    db = database.SessionLocal()
    job = UploadJob(job_id="count-1", friendly_name="c", status="running", source_path=str(source),
                    destination_bucket="b", destination_path="", recursive=True, upload_tool="gcloud",
                    filters={"exclude_patterns": ["*.tmp"], "exclude_folders": []})
    db.add(job)
    db.commit()
    assert asyncio.run(services.count_job_files(job))
    db.refresh(job)
    assert (job.total_files, job.total_size_bytes) == (4, 65)  # top, a, b, v; not skip.tmp
    activity = services.job_activity("count-1")
    assert activity["checking"]["counted_files"] == 4 and activity["phase"] is None

    # Cancelled while counting: the upload doesn't start.
    job2 = UploadJob(job_id="count-2", friendly_name="c2", status="running", source_path=str(source),
                     destination_bucket="b", destination_path="", recursive=True, upload_tool="gcloud", filters={})
    db.add(job2)
    db.commit()
    original = services.count_files

    def cancel_midway(*args, **kwargs):
        services.upload_service.cancel_upload("count-2")
        return original(*args, **kwargs)

    services.count_files = cancel_midway
    try:
        assert not asyncio.run(services.count_job_files(job2))
    finally:
        services.count_files = original

    # The engine counts for itself.
    job3 = UploadJob(job_id="count-3", friendly_name="c3", status="running", source_path=str(source),
                     destination_bucket="b", destination_path="", recursive=True, upload_tool="jetstream", filters={})
    db.add(job3)
    db.commit()
    assert asyncio.run(services.count_job_files(job3))
    assert job3.total_files == 0
    db.close()



def test_parallel_parts_setting_reaches_the_engine_and_the_summary(tmp_path, monkeypatch):
    from jetstream import transfer_settings

    monkeypatch.setenv("JETSTREAM_DATA_DIR", str(tmp_path))
    assert transfer_settings.load()["parallel_parts"] is True, "on by default"
    assert transfer_settings.save({"parallel_parts": False})["parallel_parts"] is False
    assert transfer_settings.load()["parallel_parts"] is False

    activity = services.start_job_activity("job-p", "jetstream")
    state = services.EngineRunState()
    state.activity = activity
    state.handle('{"event":"file_started","path":"video/big.mov","bytes":900000000,"parts":27}', None)
    assert services.job_activity("job-p")["in_flight"][0]["parts"] == 27
    state.handle('{"event":"composite_off","reason":"the bucket has a retention policy"}', None)
    state.handle('{"event":"done","uploaded":1,"failed":0,"composite_files":0,"parts_left":2,"elapsed_seconds":5}', None)
    text = state.summary_text(0)
    assert "one stream each: the bucket has a retention policy" in text
    assert "2 temporary part(s)" in text
