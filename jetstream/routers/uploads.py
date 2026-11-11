"""Upload endpoints for creating and managing upload jobs."""

from fastapi import APIRouter, BackgroundTasks, Depends, HTTPException
from fastapi.responses import FileResponse, PlainTextResponse
from sqlalchemy.orm import Session, defer
from typing import List
import uuid
from datetime import datetime, timezone, timedelta
import os

from ..database import get_db, UploadJob
from ..models import (
    UploadRequest, UploadResponse, JobStatusResponse, JobStatus
)
from ..events import notify_job
from ..services import (
    FileFilter, FolderAnalyzer, upload_service, queue_manager, generate_friendly_job_name,
    make_progress_callback, claim_job_for_run, job_activity, count_job_files
)

router = APIRouter()


def start_queued_jobs() -> int:
    """Start queued jobs while there are free slots (the queue limit is "folders at once")."""
    import asyncio

    free = max(0, queue_manager.max_concurrent - len(queue_manager.running_jobs))
    started = 0
    for job_id in list(queue_manager.queue)[:free]:
        asyncio.create_task(process_upload_job(job_id, ""))
        started += 1
    return started


async def process_upload_job(job_id: str, db_path: str):
    """Background task to process upload job."""
    from .. import database

    # Reuse the app's session factory; creating an engine per job leaked
    # connection pools and skipped the SQLite connection settings.
    db = database.SessionLocal()
    job = None

    try:
        job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
        if not job:
            return

        # Fix #11: job may have been cancelled before background task ran
        if job.status == "cancelled":
            return

        # Check if can start (queue management)
        if not queue_manager.can_start_job():
            job.status = "queued"
            queue_manager.add_to_queue(job_id)
            db.commit()
            notify_job(job_id)
            return
        
        # Start job (atomic claim so the scheduler can't start it too)
        if not claim_job_for_run(db, job_id):
            return
        queue_manager.start_job(job_id)
        db.refresh(job)

        # Count the files first (off the event loop) so progress has a total.
        if not await count_job_files(job):
            # Cancelled while counting: free the slot for the next folder.
            queue_manager.complete_job(job_id)
            start_queued_jobs()
            return
        db.refresh(job)
        
        # Perform upload
        success, output = await upload_service.upload_to_gcs(
            job_id=job_id,
            source_path=job.source_path,
            destination_bucket=job.destination_bucket,
            destination_path=job.destination_path or "",
            dry_run=job.dry_run,
            recursive=job.recursive,
            threads=job.threads,
            log_path=job.log_path,
            upload_tool=getattr(job, 'upload_tool', None) or "gcloud",
            exclude_patterns=job.filters.get('exclude_patterns') if job.filters else None,
            exclude_folders=job.filters.get('exclude_folders') if job.filters else None,
            no_clobber=getattr(job, 'no_clobber', False) or False,
            progress_callback=make_progress_callback(job_id),
        )

        # Pick up progress written by the callback's own sessions
        db.refresh(job)

        # Update job status (a job cancelled while running stays cancelled: no auto-retry)
        if job.status == "cancelled":
            pass
        elif success:
            job.status = "completed"
            job.files_uploaded = job.total_files
            job.bytes_uploaded = job.total_size_bytes
        else:
            # Auto-retry logic (Issue #14)
            retry_count = getattr(job, 'retry_count', 0) or 0
            max_retries = getattr(job, 'max_auto_retries', 3) or 3
            if getattr(job, 'auto_retry', False) and retry_count < max_retries:
                delay = getattr(job, 'auto_retry_delay_minutes', 30) or 30
                job.next_retry_at = datetime.now(timezone.utc) + timedelta(minutes=delay)
                job.retry_count = retry_count + 1
                job.status = "scheduled"
                job.scheduled_for = job.next_retry_at
                job.error_message = (
                    f"Auto-retry {retry_count + 1}/{max_retries} "
                    f"scheduled for {job.next_retry_at.strftime('%H:%M UTC')}"
                )
            else:
                job.status = "failed"
                job.error_message = "Upload failed - check logs"
        
        # Save upload output to database
        job.upload_output = output
        job.completed_at = datetime.now(timezone.utc)
        db.commit()
        notify_job(job_id)
        
        # Complete job in queue
        queue_manager.complete_job(job_id)
        
        # Check if there's a next job to process
        next_job_id = queue_manager.get_next_job()
        if next_job_id:
            # Trigger next job
            import asyncio
            asyncio.create_task(process_upload_job(next_job_id, db_path))
        
    except Exception as e:
        print(f"Error processing job {job_id}: {str(e)}")
        if job:
            job.status = "failed"
            job.error_message = str(e)
            job.completed_at = datetime.now(timezone.utc)
            db.commit()
            notify_job(job_id)
        queue_manager.complete_job(job_id)
        # Ensure the next waiting job is not left stuck when this one fails
        next_job_id = queue_manager.get_next_job()
        if next_job_id:
            import asyncio
            asyncio.create_task(process_upload_job(next_job_id, db_path))
    finally:
        db.close()

@router.post("/", response_model=UploadResponse)
async def create_upload(
    request: UploadRequest,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db)
):
    """
    Create a new upload job.

    Answers at once: the folder's files are counted when the job runs, never here, so a big
    folder doesn't hold up the app. If this folder is already uploading to this destination,
    nothing new starts and the running job is returned (already_running), so a double click
    can't start the same upload twice.

    If split_by_folder is True, creates separate jobs for each immediate subfolder.
    """
    import asyncio

    from ..schedules import RUNNING_STATUSES, find_active_job

    destination = f"gs://{request.destination_bucket}/{request.destination_path or ''}"
    if not request.scheduled_for:
        existing = find_active_job(db, request.source_path, request.destination_bucket,
                                   request.destination_path or "", statuses=RUNNING_STATUSES)
        if existing is not None:
            group = (existing.filters or {}).get("group")
            return UploadResponse(
                job_id=f"group:{group['id']}" if group else existing.job_id,
                status=JobStatus(existing.status),
                message="This folder is already uploading to this destination",
                source_path=request.source_path,
                destination=destination,
                already_running=True,
                dry_run=bool(existing.dry_run),
            )

    # One job per subfolder (plus one for files directly in the folder), grouped.
    if request.split_by_folder:
        from ..split import create_split_jobs

        try:
            # Lists only the top level; still off the event loop for slow network drives.
            jobs = await asyncio.to_thread(create_split_jobs, db, request)
        except Exception as e:
            raise HTTPException(status_code=400, detail=str(e))
        scheduled = jobs[0].status == "scheduled"
        if not scheduled:
            for job in jobs:
                queue_manager.add_to_queue(job.job_id)
            start_queued_jobs()
        group = jobs[0].filters["group"]
        when = f", scheduled for {request.scheduled_for.strftime('%Y-%m-%d %H:%M:%S')} UTC" if scheduled else ""
        return UploadResponse(
            job_id=f"group:{group['id']}",
            status=JobStatus.SCHEDULED if scheduled else JobStatus.QUEUED,
            message=f"Created {len(jobs)} upload jobs (one per subfolder){when}",
            source_path=request.source_path,
            destination=destination,
            dry_run=request.dry_run,
        )

    else:
        # Create single job
        job_id = str(uuid.uuid4())
        friendly_name = generate_friendly_job_name(request.source_path)
        
        # Create log path using friendly name
        log_dir = os.path.join(os.getcwd(), "logs")
        log_path = os.path.join(log_dir, f"{friendly_name}.log")
        
        # Create job in database
        job = UploadJob(
            job_id=job_id,
            friendly_name=friendly_name,
            status="pending",
            source_path=request.source_path,
            destination_bucket=request.destination_bucket,
            destination_path=request.destination_path or "",
            dry_run=request.dry_run,
            recursive=request.recursive,
            threads=request.threads,
            split_by_folder=request.split_by_folder,
            upload_tool=request.upload_tool,
            scheduled_for=request.scheduled_for,
            log_path=log_path,
            filters={
                'exclude_patterns': request.exclude_patterns,
                'exclude_folders': request.exclude_folders
            },
            no_clobber=request.no_clobber,
            auto_retry=request.auto_retry,
            auto_retry_delay_minutes=request.auto_retry_delay_minutes,
            max_auto_retries=request.max_auto_retries,
        )
        db.add(job)
        db.commit()
        db.refresh(job)
        notify_job(job_id)
        
        # Check if job is scheduled for later
        if request.scheduled_for and request.scheduled_for > datetime.now(timezone.utc):
            # Set status to scheduled - will be picked up by scheduler
            job.status = "scheduled"
            db.commit()
            
            return UploadResponse(
                job_id=job_id,
                status=JobStatus.SCHEDULED,
                message=f"Upload scheduled for {request.scheduled_for.strftime('%Y-%m-%d %H:%M:%S')} UTC",
                source_path=request.source_path,
                destination=f"gs://{request.destination_bucket}/{request.destination_path or ''}"
            )
        
        # Add to background tasks for immediate processing
        background_tasks.add_task(process_upload_job, job_id, "")
        
        return UploadResponse(
            job_id=job_id,
            status=JobStatus.PENDING,
            message="Upload job created successfully",
            source_path=request.source_path,
            destination=f"gs://{request.destination_bucket}/{request.destination_path or ''}"
        )

@router.get("/{job_id}", response_model=JobStatusResponse)
async def get_upload_status(job_id: str, db: Session = Depends(get_db)):
    """Get status of a specific upload job."""
    job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    
    return JobStatusResponse(**job.to_dict())

@router.get("/{job_id}/activity")
async def get_upload_activity(job_id: str):
    """Files uploading now and recently finished, while a job runs (empty if unknown)."""
    return job_activity(job_id) or {"tool": None, "files_done": 0, "failed": 0, "in_flight": [], "recent": []}

@router.post("/{job_id}/pause")
async def pause_upload(job_id: str):
    """Pause a running upload. Works for the JetStream engine; gcloud uploads can only be cancelled."""
    if not upload_service.pause_upload(job_id):
        raise HTTPException(status_code=409, detail="Only running uploads using the JetStream engine can be paused.")
    return {"job_id": job_id, "paused": True}

@router.post("/{job_id}/resume")
async def resume_upload(job_id: str):
    if not upload_service.resume_upload(job_id):
        raise HTTPException(status_code=409, detail="This upload isn't running with the JetStream engine.")
    return {"job_id": job_id, "paused": False}

@router.get("/{job_id}/log")
async def download_upload_log(job_id: str, db: Session = Depends(get_db)):
    """Download the full job log file, falling back to the stored output tail."""
    job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    filename = f"{job.friendly_name or job_id}.log"
    if job.log_path and os.path.isfile(job.log_path):
        return FileResponse(job.log_path, media_type="text/plain", filename=filename)
    if job.upload_output:
        return PlainTextResponse(
            job.upload_output,
            headers={"Content-Disposition": f'attachment; filename="{filename}"'},
        )
    raise HTTPException(status_code=404, detail="No output available for this job")

@router.get("/", response_model=List[JobStatusResponse])
async def list_uploads(
    status: str = None,
    limit: int = 50,
    include_cleared: bool = False,
    db: Session = Depends(get_db)
):
    """List all upload jobs, optionally filtered by status."""
    query = db.query(UploadJob)
    
    if status:
        query = query.filter(UploadJob.status == status)
    
    # Exclude cleared jobs by default
    if not include_cleared:
        query = query.filter((UploadJob.cleared == False) | (UploadJob.cleared == None))
    
    # Command output is only needed in the job detail view; skip loading it here.
    jobs = (
        query.options(defer(UploadJob.upload_output))
        .order_by(UploadJob.created_at.desc())
        .limit(limit)
        .all()
    )
    
    return [JobStatusResponse(**job.to_summary_dict()) for job in jobs]

@router.post("/clear-completed")
async def clear_completed_jobs(db: Session = Depends(get_db)):
    """Clear all completed jobs from the recent list (mark as hidden)."""
    result = db.query(UploadJob).filter(
        UploadJob.status.in_(["completed", "failed", "cancelled"]),
        (UploadJob.cleared == False) | (UploadJob.cleared == None)
    ).update({"cleared": True}, synchronize_session=False)
    db.commit()
    return {"message": f"Cleared {result} job(s) from recent list"}

@router.delete("/{job_id}")
async def delete_upload(job_id: str, db: Session = Depends(get_db)):
    """Delete an upload job from the database."""
    job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    if (job.filters or {}).get("transfer_direction") == "cloud_to_cloud":
        raise HTTPException(status_code=400, detail="Cloud operations must be managed from Cloud Ops")
    
    # If job is running or queued, cancel it first
    if job.status in ["running", "queued", "pending"]:
        if job.status == "running":
            upload_service.cancel_upload(job_id)
        queue_manager.complete_job(job_id)
    
    # Delete the job from database
    db.delete(job)
    db.commit()
    notify_job(job_id)
    
    return {"message": "Job deleted successfully", "job_id": job_id}

@router.post("/{job_id}/cancel")
async def cancel_upload(job_id: str, db: Session = Depends(get_db)):
    """Cancel a running, queued, or scheduled upload job."""
    job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    if (job.filters or {}).get("transfer_direction") == "cloud_to_cloud":
        raise HTTPException(status_code=400, detail="Cloud operations cannot be cancelled from Uploads")
    
    if job.status not in ["running", "queued", "pending", "scheduled"]:
        raise HTTPException(status_code=400, detail=f"Cannot cancel job with status: {job.status}")
    
    # Cancel the upload if running
    if job.status == "running":
        upload_service.cancel_upload(job_id)
    
    # Update status
    job.status = "cancelled"
    job.completed_at = datetime.now(timezone.utc)
    db.commit()
    notify_job(job_id)
    
    # Remove from queue
    queue_manager.complete_job(job_id)
    
    return {"message": "Job cancelled successfully", "job_id": job_id}

@router.post("/groups/{group_id}/cancel")
async def cancel_group(group_id: str, db: Session = Depends(get_db)):
    """Cancel every unfinished job of a split upload."""
    from ..split import group_jobs

    jobs = group_jobs(db, group_id)
    if not jobs:
        raise HTTPException(status_code=404, detail="Upload group not found")
    cancelled = 0
    for job in jobs:
        if job.status in ("running", "queued", "pending", "scheduled"):
            if job.status == "running":
                upload_service.cancel_upload(job.job_id)
            job.status = "cancelled"
            job.completed_at = datetime.now(timezone.utc)
            queue_manager.complete_job(job.job_id)
            if job.job_id in queue_manager.queue:
                queue_manager.queue.remove(job.job_id)
            cancelled += 1
    db.commit()
    for job in jobs:
        notify_job(job.job_id)
    return {"group_id": group_id, "cancelled": cancelled}


@router.post("/groups/{group_id}/retry-failed")
async def retry_failed_in_group(group_id: str, db: Session = Depends(get_db)):
    """Run the failed or cancelled subfolders of a split upload again."""
    from ..split import group_jobs

    jobs = group_jobs(db, group_id)
    if not jobs:
        raise HTTPException(status_code=404, detail="Upload group not found")
    retried = []
    for job in jobs:
        if job.status in ("failed", "cancelled"):
            job.status = "queued"
            job.error_message = None
            job.started_at = None
            job.completed_at = None
            job.files_uploaded = 0
            job.bytes_uploaded = 0
            job.upload_output = None
            retried.append(job.job_id)
    db.commit()
    for job_id in retried:
        queue_manager.add_to_queue(job_id)
        notify_job(job_id)
    start_queued_jobs()
    return {"group_id": group_id, "retried": len(retried)}


@router.post("/{job_id}/retry")
async def retry_upload(
    job_id: str,
    background_tasks: BackgroundTasks,
    remove_dry_run: bool = False,
    db: Session = Depends(get_db)
):
    """Retry a failed, cancelled, or completed upload job. Optionally remove dry_run flag."""
    job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    if (job.filters or {}).get("transfer_direction") == "cloud_to_cloud":
        raise HTTPException(status_code=400, detail="Cloud operations cannot be retried as uploads")
    
    if job.status not in ["failed", "cancelled", "completed"]:
        raise HTTPException(status_code=400, detail=f"Cannot retry job with status: {job.status}")
    
    # Reset job
    job.status = "pending"
    job.error_message = None
    job.started_at = None
    job.completed_at = None
    job.files_uploaded = 0
    job.bytes_uploaded = 0
    job.upload_output = None
    
    # Remove dry_run flag if requested (converts dry-run preview to actual upload)
    if remove_dry_run:
        job.dry_run = False
    
    db.commit()
    notify_job(job_id)
    
    # Add to background tasks
    background_tasks.add_task(process_upload_job, job_id, "")
    
    message = "Job queued for actual upload" if remove_dry_run else "Job queued for retry"
    return {"message": message, "job_id": job_id}
