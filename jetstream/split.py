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
from .services import FileFilter, FolderAnalyzer, generate_friendly_job_name


def create_split_jobs(db, request: UploadRequest, extra_filters: Optional[Dict] = None, name_prefix: str = "") -> List[UploadJob]:
    """Create the jobs for a split upload, queued (or scheduled). Raises ValueError when the
    folder has nothing to upload."""
    file_filter = FileFilter(
        include_patterns=None,
        exclude_patterns=request.exclude_patterns,
        exclude_folders=request.exclude_folders,
    )
    analyzer = FolderAnalyzer(file_filter)
    source = request.source_path
    base_dest = (request.destination_path or "").strip("/")

    parts = []  # (source path, destination path, recursive, stats, part name)
    root = analyzer.analyze(source, recursive=False)
    if root.get("total_files"):
        parts.append((source, base_dest, False, root, ""))
    for sub in sorted(analyzer.analyze_subfolders(source), key=lambda s: s["name"].lower()):
        if not sub.get("total_files"):
            continue
        dest = f"{base_dest}/{sub['name']}" if base_dest else sub["name"]
        parts.append((sub["path"], dest, True, sub, sub["name"]))
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
    for path, dest, recursive, stats, part in parts:
        friendly_name = name_prefix + generate_friendly_job_name(path)
        job = UploadJob(
            job_id=str(uuid.uuid4()),
            friendly_name=friendly_name,
            status="scheduled" if scheduled else "queued",
            source_path=path,
            destination_bucket=request.destination_bucket,
            destination_path=dest,
            total_files=stats.get("total_files", 0),
            total_size_bytes=stats.get("total_size_bytes", 0),
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
