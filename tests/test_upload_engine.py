"""Choosing the JetStream (Rust) upload engine instead of gcloud rsync."""

import asyncio
import json
import os
import shutil
import socket
import stat
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import pytest

from jetstream import services
from jetstream.models import UploadRequest
from jetstream.services import EngineRunState, UploadService, list_upload_tools

REPO = Path(__file__).resolve().parent.parent
ENGINE_BUILD = REPO / "desktop" / "transfer" / "target"


def write_fake_engine(tmp_path, events, exit_code=0):
    """A stand-in engine that records its arguments and prints the given JSON events."""
    script = tmp_path / "fake-engine"
    args_file = tmp_path / "args.json"
    lines = "\n".join(json.dumps(e) for e in events)
    script.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys\n"
        f"json.dump(sys.argv[1:], open({str(args_file)!r}, 'w'))\n"
        f"print({lines!r})\n"
        f"sys.exit({exit_code})\n"
    )
    script.chmod(script.stat().st_mode | stat.S_IEXEC)
    return script, args_file


PLAN = {"event": "plan", "local_files": 10, "local_bytes": 1000, "excluded": 0, "unchanged": 6,
        "to_check": 4, "to_check_bytes": 400, "listed_objects": None}
DONE = {"event": "done", "local_files": 10, "local_bytes": 1000, "excluded": 1, "unchanged": 6,
        "already_in_cloud": 1, "uploaded": 2, "uploaded_bytes": 300, "resumed": 1, "would_upload": 0,
        "would_upload_bytes": 0, "skipped_exists": 0, "failed": 1, "unreadable": 0, "listed_objects": None,
        "cancelled": False, "elapsed_seconds": 1.5}


@pytest.mark.skipif(sys.platform == "win32", reason="POSIX script stands in for the engine")
def test_engine_command_line_progress_and_summary(tmp_path, monkeypatch):
    events = [
        {"event": "start", "dry_run": False},
        PLAN,
        {"event": "progress", "files_done": 2, "files_total": 4, "bytes_done": 150, "bytes_total": 400},
        {"event": "file_failed", "path": "bad.jpg", "error": "403: no access"},
        DONE,
    ]
    engine, args_file = write_fake_engine(tmp_path, events, exit_code=2)
    monkeypatch.setenv("JETSTREAM_TRANSFER_BIN", str(engine))
    progress = []

    success, output = asyncio.run(UploadService().upload_to_gcs(
        job_id="job-1",
        source_path=str(tmp_path),
        destination_bucket="bucket",
        destination_path="bucket/surveys/2026/",
        dry_run=False,
        recursive=False,
        upload_tool="jetstream",
        exclude_patterns=["*.tmp"],
        exclude_folders=["_archive"],
        no_clobber=True,
        log_path=str(tmp_path / "logs" / "job.log"),
        progress_callback=lambda **kw: progress.append(kw),
    ))

    args = json.loads(args_file.read_text())
    assert args[:3] == ["sync", str(tmp_path), "gs://bucket/surveys/2026"]
    assert "--no-recursive" in args and "--no-clobber" in args and "--dry-run" not in args
    excludes = [args[i + 1] for i, a in enumerate(args) if a == "--exclude"]
    assert excludes == [r".*\.tmp$", ".*/_archive/.*", "^_archive/.*"]

    # Unchanged bytes/files count as already done, so the bar reflects the whole folder.
    assert progress[0] == {"bytes_uploaded": 600, "total_bytes": 1000, "files_uploaded": 6}
    assert progress[-1] == {"bytes_uploaded": 750, "total_bytes": 1000, "files_uploaded": 8}

    assert success is False
    assert "Uploaded 2 file(s)" in output and "Resumed: 1" in output
    assert "bad.jpg: 403: no access" in output
    log = (tmp_path / "logs" / "job.log").read_text()
    assert "JetStream engine" in log and '"event": "file_failed"' in log


def test_engine_missing_fails_with_clear_message(monkeypatch, tmp_path):
    monkeypatch.setattr(services, "find_transfer_engine", lambda: None)
    success, output = asyncio.run(UploadService().upload_to_gcs(
        job_id="job-2", source_path=str(tmp_path), destination_bucket="bucket", upload_tool="jetstream",
    ))
    assert success is False
    assert "not installed" in output


def test_dry_run_summary_reads_as_preview():
    state = EngineRunState()
    state.handle(json.dumps({"event": "start", "dry_run": True}), None)
    state.handle(json.dumps({**DONE, "uploaded": 0, "would_upload": 3, "would_upload_bytes": 3 * 1024 * 1024}), None)
    assert state.summary_text(0).startswith("Preview: 3 file(s), 3.0 MB would be uploaded.")


def test_upload_tools_report_availability(monkeypatch):
    monkeypatch.setattr(services, "find_gcloud", lambda: "/usr/bin/gcloud")
    monkeypatch.setattr(services, "find_transfer_engine", lambda: None)
    tools = {t["id"]: t for t in list_upload_tools()}
    assert list(tools) == ["gcloud", "jetstream", "gsutil"]
    assert tools["gcloud"]["available"] is True
    assert tools["jetstream"]["available"] is False
    assert "desktop app" in tools["jetstream"]["detail"]


def test_upload_request_accepts_only_known_tools(tmp_path):
    base = {"source_path": str(tmp_path), "gcs_destination": "bucket/path"}
    assert UploadRequest(**base, upload_tool="jetstream").upload_tool == "jetstream"
    with pytest.raises(ValueError):
        UploadRequest(**base, upload_tool="curl")


def _real_engine():
    for build in ("release", "debug"):
        candidate = ENGINE_BUILD / build / "jetstream-transfer"
        if candidate.is_file():
            return candidate
    return None


@pytest.mark.skipif(
    not (_real_engine() and os.environ.get("FAKE_GCS_SERVER")),
    reason="needs a built engine (cargo build in desktop/transfer) and FAKE_GCS_SERVER",
)
def test_real_engine_uploads_through_jetstream(tmp_path, monkeypatch):
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    server = subprocess.Popen(
        [os.environ["FAKE_GCS_SERVER"], "-scheme", "http", "-host", "127.0.0.1", "-port", str(port),
         "-public-host", f"127.0.0.1:{port}", "-backend", "memory", "-log-level", "error"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    base = f"http://127.0.0.1:{port}"
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        for _ in range(100):
            try:
                opener.open(base + "/storage/v1/b")
                break
            except OSError:
                time.sleep(0.1)
        opener.open(urllib.request.Request(
            base + "/storage/v1/b", data=b'{"name":"bucket"}', headers={"Content-Type": "application/json"}))

        src = tmp_path / "src"
        (src / "sub").mkdir(parents=True)
        (src / "a.jpg").write_bytes(b"alpha")
        (src / "sub" / "big.bin").write_bytes(os.urandom(9 * 1024 * 1024))
        (src / "skip.tmp").write_bytes(b"x")
        monkeypatch.setenv("JETSTREAM_TRANSFER_BIN", str(_real_engine()))
        monkeypatch.setenv("STORAGE_EMULATOR_HOST", base)
        monkeypatch.setenv("JETSTREAM_DATA_DIR", str(tmp_path / "data"))
        for var in ("HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"):
            monkeypatch.delenv(var, raising=False)

        def run():
            return asyncio.run(UploadService().upload_to_gcs(
                job_id="real", source_path=str(src), destination_bucket="bucket", destination_path="dest",
                upload_tool="jetstream", exclude_patterns=["*.tmp"], progress_callback=lambda **kw: None,
            ))

        success, output = run()
        assert success, output
        assert "Uploaded 2 file(s)" in output
        listing = json.load(opener.open(base + "/storage/v1/b/bucket/o?prefix=dest/"))
        assert sorted(o["name"] for o in listing["items"]) == ["dest/a.jpg", "dest/sub/big.bin"]

        success, output = run()
        assert success and "Uploaded 0 file(s)" in output and "Unchanged: 2" in output
    finally:
        server.terminate()
        server.wait()
