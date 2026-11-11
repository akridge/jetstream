"""Upload a folder as one job per subfolder, grouped under the folder.

Long uploads of a big folder get checkpoints: each subfolder finishes on its own, a failure
retries just that subfolder, and progress reads "5 of 12 folders". Files directly in the
folder get their own job (they used to be skipped when splitting).
"""

import os
import uuid
from datetime import datetime, timezone
from typing import Dict, List, Optional

from .database import UploadJob
from .events import notify_job
from .models import UploadRequest
from .services import FileFilter, generate_friendly_job_name


def _has_entries(path: str) -> bool:
    """Whether a folder has anything in it (reads one entry, not the whole folder)."""
    try:
        with os.scandir(path) as entries:
            return next(entries, None) is not None
    except OSError:
        return True  # let the job report the problem


def create_split_jobs(db, request: UploadRequest, extra_filters: Optional[Dict] = None, name_prefix: str = "") -> List[UploadJob]:
    """Create the jobs for a split upload, queued (or scheduled): one per subfolder, plus one
    for files directly in the folder. Raises ValueError when the folder has nothing in it."""
    file_filter = FileFilter(
        include_patterns=None,
        exclude_patterns=request.exclude_patterns,
        exclude_folders=request.exclude_folders,
    )
    source = request.source_path
    base_dest = (request.destination_path or "").strip("/")

    # Only the top level is listed here, so the request answers at once even for huge
    # folders; each job counts its own files when it runs.
    parts = []  # (source path, destination path, recursive, part name)
    has_root_files = False
    subfolders = []
    try:
        with os.scandir(source) as entries:
            for entry in entries:
                try:
                    if entry.is_dir(follow_symlinks=False):
                        if not file_filter.should_exclude_folder(entry.name) and _has_entries(entry.path):
                            subfolders.append((entry.name, entry.path))
                    elif not has_root_files and file_filter.should_include_file(entry.path):
                        has_root_files = True
                except OSError:
                    continue
    except OSError as exc:
        raise ValueError(f"Can't read {source}: {exc.strerror or exc}") from exc
    if has_root_files:
        parts.append((source, base_dest, False, ""))
    for name, path in sorted(subfolders, key=lambda s: s[0].lower()):
        dest = f"{base_dest}/{name}" if base_dest else name
        parts.append((path, dest, True, name))
    if not parts:
        raise ValueError(f"Nothing to upload in {source}")

    group = {
        "id": uuid.uuid4().hex,
        "name": os.path.basename(os.path.normpath(source)) or source,
        "source_path": source,
        "destination": f"{request.destination_bucket}/{base_dest}".rstrip("/"),
        "parts": len(parts),
    }
    scheduled = request.scheduled_for and request.scheduled_for > datetime.now(timezone.utc)
    log_dir = os.path.join(os.getcwd(), "logs")
    jobs = []
    for path, dest, recursive, part in parts:
        friendly_name = name_prefix + generate_friendly_job_name(path)
        job = UploadJob(
            job_id=str(uuid.uuid4()),
            friendly_name=friendly_name,
            status="scheduled" if scheduled else "queued",
            source_path=path,
            destination_bucket=request.destination_bucket,
            destination_path=dest,
            total_files=0,
            total_size_bytes=0,
            dry_run=request.dry_run,
            recursive=recursive,
            threads=request.threads,
            split_by_folder=False,
            upload_tool=request.upload_tool,
            scheduled_for=request.scheduled_for if scheduled else None,
            log_path=os.path.join(log_dir, f"{friendly_name}.log"),
            filters={
                "exclude_patterns": request.exclude_patterns,
                "exclude_folders": request.exclude_folders,
                **(extra_filters or {}),
                "group": group,
                "group_part": part,
            },
            no_clobber=request.no_clobber,
            auto_retry=request.auto_retry,
            auto_retry_delay_minutes=request.auto_retry_delay_minutes,
            max_auto_retries=request.max_auto_retries,
        )
        db.add(job)
        jobs.append(job)
    db.commit()
    for job in jobs:
        notify_job(job.job_id)
    return jobs


def group_jobs(db, group_id: str) -> List[UploadJob]:
    """Every job of a split upload."""
    from sqlalchemy import func

    return db.query(UploadJob).filter(func.json_extract(UploadJob.filters, "$.group.id") == group_id).all()
