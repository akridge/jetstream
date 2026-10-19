"""Scheduled syncs for saved folders (presets).

A preset's ``options["schedule"]`` looks like::

    {"mode": "daily", "time": "18:30", "days": [0, 1, 2, 3, 4], "preview": False}
    {"mode": "hourly", "every_hours": 6}
    {"mode": "off"}

Times are local to this computer; ``days`` uses Monday = 0 (empty = every day).
The upload scheduler calls :func:`run_due_presets` every few seconds.
"""

import logging
import os
import re
import uuid
from datetime import datetime, timedelta, timezone
from typing import List, Optional

from . import database
from .database import SyncPreset, UploadJob
from .events import notify_job

logger = logging.getLogger(__name__)

ACTIVE_STATUSES = ("pending", "queued", "running", "scheduled")


def _to_local(utc_naive: datetime) -> datetime:
    return utc_naive.replace(tzinfo=timezone.utc).astimezone()


def _to_utc_naive(local_aware: datetime) -> datetime:
    return local_aware.astimezone(timezone.utc).replace(tzinfo=None)


def next_run(schedule: Optional[dict], after: datetime) -> Optional[datetime]:
    """Next run strictly after ``after`` (naive UTC), as naive UTC; None if not scheduled."""
    if not schedule or schedule.get("mode") in (None, "", "off"):
        return None
    mode = schedule.get("mode")
    if mode == "hourly":
        hours = max(1, min(24, int(schedule.get("every_hours") or 1)))
        return (after + timedelta(hours=hours)).replace(second=0, microsecond=0)
    if mode == "daily":
        match = re.fullmatch(r"(\d{1,2}):(\d{2})", str(schedule.get("time") or ""))
        if not match:
            return None
        hour, minute = int(match.group(1)), int(match.group(2))
        if hour > 23 or minute > 59:
            return None
        days = {int(d) for d in (schedule.get("days") or []) if 0 <= int(d) <= 6}
        local_after = _to_local(after)
        for offset in range(0, 8):
            day = local_after + timedelta(days=offset)
            candidate = day.replace(hour=hour, minute=minute, second=0, microsecond=0)
            if candidate > local_after and (not days or candidate.weekday() in days):
                return _to_utc_naive(candidate)
    return None


def _lines(text: Optional[str]) -> Optional[List[str]]:
    items = [s.strip() for s in (text or "").splitlines() if s.strip()]
    return items or None


def _commas(text: Optional[str]) -> Optional[List[str]]:
    items = [s.strip() for s in (text or "").split(",") if s.strip()]
    return items or None


def create_preset_job(db, preset: SyncPreset, dry_run: bool) -> UploadJob:
    """Queue an upload for a saved folder. Problems become a failed job the user can see."""
    from .models import UploadRequest
    from .services import FileFilter, FolderAnalyzer, find_transfer_engine, generate_friendly_job_name, queue_manager

    options = preset.options or {}
    tool = options.get("upload_tool") or "gcloud"
    if tool == "jetstream" and not find_transfer_engine():
        tool = "gcloud"
    job_id = uuid.uuid4().hex
    friendly_name = generate_friendly_job_name(preset.source_path).replace("upload-", "scheduled-", 1)
    log_path = os.path.join(os.getcwd(), "logs", f"{friendly_name}.log")
    filters = {"preset_id": preset.preset_id, "trigger": "schedule"}

    try:
        request = UploadRequest(
            source_path=preset.source_path,
            gcs_destination=preset.destination,
            dry_run=dry_run,
            recursive=options.get("recursive", True) is not False,
            upload_tool=tool,
            no_clobber=bool(options.get("no_clobber")),
            exclude_patterns=_lines(options.get("exclude_patterns")),
            exclude_folders=_commas(options.get("exclude_folders")),
        )
    except Exception as exc:  # e.g. the folder is gone or the drive isn't connected
        errors = exc.errors() if hasattr(exc, "errors") else None
        message = (errors[0].get("msg", "") if errors else str(exc)).replace("Value error, ", "")
        job = UploadJob(
            job_id=job_id,
            friendly_name=friendly_name,
            status="failed",
            source_path=preset.source_path,
            destination_bucket=(preset.destination or "?").split("/")[0],
            destination_path="/".join((preset.destination or "").split("/")[1:]),
            dry_run=dry_run,
            upload_tool=tool,
            filters=filters,
            error_message=f"Scheduled sync couldn't start: {message}",
            completed_at=datetime.now(timezone.utc),
        )
        db.add(job)
        db.commit()
        notify_job(job_id)
        return job

    stats = FolderAnalyzer(
        FileFilter(include_patterns=None, exclude_patterns=request.exclude_patterns, exclude_folders=request.exclude_folders)
    ).analyze(request.source_path, recursive=request.recursive)
    job = UploadJob(
        job_id=job_id,
        friendly_name=friendly_name,
        status="queued",
        source_path=request.source_path,
        destination_bucket=request.destination_bucket,
        destination_path=request.destination_path,
        total_files=stats["total_files"],
        total_size_bytes=stats["total_size_bytes"],
        dry_run=dry_run,
        recursive=request.recursive,
        threads=4,
        split_by_folder=False,
        upload_tool=tool,
        log_path=log_path,
        filters={**filters, "exclude_patterns": request.exclude_patterns, "exclude_folders": request.exclude_folders},
        no_clobber=request.no_clobber,
    )
    db.add(job)
    db.commit()
    notify_job(job_id)
    queue_manager.add_to_queue(job_id)
    return job


def _has_active_job(db, preset: SyncPreset) -> bool:
    destination = (preset.destination or "").strip().replace("gs://", "").strip("/")
    bucket, _, path = destination.partition("/")
    jobs = db.query(UploadJob).filter(
        UploadJob.source_path == preset.source_path,
        UploadJob.destination_bucket == bucket,
        UploadJob.status.in_(ACTIVE_STATUSES),
    ).all()
    return any((j.destination_path or "").strip("/") == path.strip("/") for j in jobs)


def run_due_presets(now: Optional[datetime] = None) -> List[str]:
    """Start every scheduled folder sync that is due. Returns the new job ids."""
    if database.SessionLocal is None:
        return []
    now = now or datetime.now(timezone.utc).replace(tzinfo=None)
    started: List[str] = []
    db = database.SessionLocal()
    try:
        due = db.query(SyncPreset).filter(SyncPreset.next_run_at.isnot(None), SyncPreset.next_run_at <= now).all()
        for preset in due:
            schedule = (preset.options or {}).get("schedule") or {}
            try:
                if _has_active_job(db, preset):
                    logger.info("Skipping scheduled sync of %s: previous sync still running", preset.name)
                else:
                    job = create_preset_job(db, preset, dry_run=bool(schedule.get("preview")))
                    preset.last_run_job_id = job.job_id
                    started.append(job.job_id)
                    logger.info("Started scheduled sync of %s (job %s)", preset.name, job.job_id)
                preset.last_run_at = now
            except Exception:
                logger.exception("Scheduled sync of %s failed to start", preset.name)
            preset.next_run_at = next_run(schedule, now)
            db.commit()
    finally:
        db.close()
    return started
