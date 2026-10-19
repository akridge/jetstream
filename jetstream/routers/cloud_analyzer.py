"""Cloud Storage Analyzer - On-demand GCS bucket analysis."""

from fastapi import APIRouter, HTTPException, BackgroundTasks, Depends
from pydantic import BaseModel, Field
from typing import Optional, List, Dict, Literal
from datetime import datetime, timezone
import asyncio
import subprocess
import threading
import uuid
import shutil
from sqlalchemy import or_
from sqlalchemy.orm import Session, defer

from ..database import UploadJob, get_db
from ..services import redact_sensitive_text, cap_output, OutputCapture, ConsoleEcho

# Lazy import for Google Cloud Storage to avoid startup crashes
# Will be imported only when actually needed
def get_storage_client():
    """Lazy import and initialize Google Cloud Storage client."""
    try:
        from google.cloud import storage
        return storage.Client()
    except ImportError:
        raise HTTPException(
            status_code=503,
            detail="Google Cloud Storage library not installed. Install with: pip install google-cloud-storage"
        )
    except Exception as e:
        detail = redact_sensitive_text(str(e))
        raise HTTPException(
            status_code=401,
            detail=(
                "Authentication failed. Please run 'gcloud auth application-default login' "
                "or set GOOGLE_APPLICATION_CREDENTIALS environment variable. "
                f"Error: {detail}"
            )
        )

router = APIRouter()

# Store for tracking cloud transfer jobs (in-memory for simplicity)
cloud_transfer_jobs = {}

class CloudAnalysisRequest(BaseModel):
    """Request model for cloud storage analysis."""
    bucket_name: str = Field(..., description="GCS bucket name")
    prefix: str = Field("", description="Folder prefix to analyze (optional)")
    max_depth: int = Field(3, description="Maximum folder depth to analyze")

class CloudTransferRequest(BaseModel):
    """Request model for cloud-to-cloud transfer."""
    source_path: str = Field(..., description="Source GCS path (gs://bucket/path or bucket/path)")
    dest_path: str = Field(..., description="Destination GCS path (gs://bucket/path or bucket/path)")
    recursive: bool = Field(True, description="Include subfolders")
    dry_run: bool = Field(True, description="Preview without actually transferring")
    no_clobber: bool = Field(False, description="Skip objects that already exist at destination")
    exclude_patterns: Optional[List[str]] = Field(None, description="Patterns to exclude")

class CloudTransferResponse(BaseModel):
    """Response model for cloud transfer."""
    job_id: str
    status: str
    message: str
    command: str

class CloudMoveRequest(BaseModel):
    """Request model for moving a GCS object or folder."""
    source_path: str = Field(..., description="Source GCS URI beginning with gs://")
    dest_path: str = Field(..., description="Destination GCS URI beginning with gs://")
    source_kind: Literal["object", "folder", "folder_contents"] = Field(
        ..., description="Move one object, a folder into the destination, or a folder's contents"
    )
    dry_run: bool = Field(False, description="Preview the move without copying or deleting anything")

class CloudMoveResponse(BaseModel):
    """Response model for a cloud move."""
    job_id: str
    status: str
    message: str
    operation: str
    
class FolderStats(BaseModel):
    """Folder statistics model."""
    path: str
    file_count: int
    folder_count: int
    total_size_bytes: int
    total_size_gb: float
    earliest_created: Optional[str]
    latest_updated: Optional[str]

@router.post("/analyze-bucket")
async def analyze_bucket(request: CloudAnalysisRequest):
    """
    Analyze a GCS bucket and return statistics.
    This can be a long-running operation for large buckets.
    """
    try:
        def analyze_sync():
            """Synchronous analysis function to run in thread."""
            # Use lazy-loaded storage client
            client = get_storage_client()
            
            bucket = client.bucket(request.bucket_name)
            
            # Check if bucket exists
            if not bucket.exists():
                raise HTTPException(status_code=404, detail=f"Bucket '{request.bucket_name}' not found")
            
            folder_stats = {}
            
            # List all blobs with the given prefix
            blobs = bucket.list_blobs(prefix=request.prefix)
            
            total_blobs = 0
            for blob in blobs:
                total_blobs += 1
                
                # Extract folder path
                if blob.name.endswith('/'):
                    # It's a folder marker
                    folder_path = blob.name
                    if folder_path not in folder_stats:
                        folder_stats[folder_path] = {
                            'files': [],
                            'subfolders': set(),
                            'total_size': 0
                        }
                else:
                    # It's a file
                    parts = blob.name.split('/')
                    
                    # Track file in its immediate folder
                    if len(parts) > 1:
                        folder_path = '/'.join(parts[:-1]) + '/'
                    else:
                        folder_path = '/'
                    
                    if folder_path not in folder_stats:
                        folder_stats[folder_path] = {
                            'files': [],
                            'subfolders': set(),
                            'total_size': 0
                        }
                    
                    folder_stats[folder_path]['files'].append({
                        'name': blob.name,
                        'size': blob.size or 0,
                        'created': blob.time_created.isoformat() if blob.time_created else None,
                        'updated': blob.updated.isoformat() if blob.updated else None
                    })
                    folder_stats[folder_path]['total_size'] += blob.size or 0
                
                # Stop after reasonable amount for quick preview
                if total_blobs > 10000:
                    break
            
            # Calculate statistics
            results = []
            for path, stats in folder_stats.items():
                files = stats['files']
                results.append({
                    'path': path,
                    'file_count': len(files),
                    'folder_count': len(stats['subfolders']),
                    'total_size_bytes': stats['total_size'],
                    'total_size_gb': round(stats['total_size'] / (1024**3), 4),
                    'earliest_created': min((f['created'] for f in files if f['created']), default=None),
                    'latest_updated': max((f['updated'] for f in files if f['updated']), default=None)
                })
            
            # Sort by size descending
            results.sort(key=lambda x: x['total_size_bytes'], reverse=True)
            
            return {
                'bucket_name': request.bucket_name,
                'prefix': request.prefix,
                'total_folders_analyzed': len(results),
                'total_blobs_scanned': total_blobs,
                'folders': results[:50],  # Return top 50 folders
                'analysis_time': datetime.now(timezone.utc).isoformat()
            }
        
        # Run analysis in thread pool to avoid blocking
        result = await asyncio.to_thread(analyze_sync)
        return result
        
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Analysis failed: {str(e)}")

@router.get("/bucket-summary/{bucket_name}")
async def get_bucket_summary(bucket_name: str):
    """Get quick summary of a GCS bucket."""
    try:
        def get_summary():
            # Use lazy-loaded storage client
            client = get_storage_client()
            
            bucket = client.bucket(bucket_name)
            
            if not bucket.exists():
                raise HTTPException(status_code=404, detail=f"Bucket '{bucket_name}' not found")
            
            # Get bucket metadata
            bucket.reload()
            
            # Count objects (limited sample for speed)
            blobs = list(bucket.list_blobs(max_results=1000))
            sample_size = len(blobs)
            total_size = sum(blob.size for blob in blobs if blob.size)
            
            return {
                'bucket_name': bucket_name,
                'location': bucket.location,
                'storage_class': bucket.storage_class,
                'created': bucket.time_created.isoformat() if bucket.time_created else None,
                'sample_object_count': sample_size,
                'sample_total_size_gb': round(total_size / (1024**3), 2),
                'note': 'Limited to first 1000 objects for quick preview'
            }
        
        result = await asyncio.to_thread(get_summary)
        return result
        
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Failed to get bucket summary: {str(e)}")


def normalize_gcs_path(path: str) -> str:
    """Normalize a GCS path to gs:// format."""
    path = path.strip().rstrip('/')
    if path.startswith('gs://'):
        return path
    return f"gs://{path}"


def split_gcs_path(path: str) -> tuple[str, str]:
    """Split a normalized GCS path into bucket and object prefix."""
    normalized = normalize_gcs_path(path)[5:]
    bucket, _, prefix = normalized.partition('/')
    return bucket, prefix


def parse_move_path(path: str, source_kind: str) -> tuple[str, str, str]:
    """Validate a move URI while preserving prefix trailing-slash semantics."""
    value = path.strip()
    if not value.startswith("gs://"):
        raise ValueError("Move paths must begin with gs://")

    bucket, separator, object_name = value[5:].partition("/")
    if not bucket or not separator or not object_name:
        raise ValueError("Move paths must include a bucket and object or prefix")

    if source_kind == "prefix":
        if not value.endswith("/"):
            raise ValueError("Folder paths must end with /")
        object_name = object_name.rstrip("/") + "/"
    elif value.endswith("/"):
        raise ValueError("A single-file path cannot end with /. Choose a folder option to move a folder.")

    return bucket, object_name, f"gs://{bucket}/{object_name}"


def destination_name_for_move(source_name: str, source_prefix: str, destination_prefix: str) -> str:
    """Map a source object beneath a contents-only destination prefix."""
    if not source_name.startswith(source_prefix):
        raise ValueError(f"Object '{source_name}' is outside source prefix '{source_prefix}'")
    relative_name = source_name[len(source_prefix):]
    if not relative_name:
        raise ValueError("Prefix marker objects cannot be moved as content")
    return destination_prefix + relative_name


def make_cloud_sync_name(job_id: str) -> str:
    timestamp = datetime.now(timezone.utc).strftime('%Y-%m-%d-%H%M%S')
    return f"cloud-sync-{timestamp}-{job_id}"


def update_cloud_operation_job(
    job_id: str,
    status: Optional[str] = None,
    output: str = None,
    error: str = None,
    phase: str = None,
    total_files: int = None,
    total_size_bytes: float = None,
    files_uploaded: int = None,
    bytes_uploaded: float = None,
):
    """Persist cloud operation progress into the shared jobs table."""
    from .. import database

    if database.SessionLocal is None:
        return

    db = database.SessionLocal()
    try:
        job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
        if not job:
            return
        if status is not None:
            job.status = status
        if output is not None:
            job.upload_output = cap_output(output)
        if error is not None:
            job.error_message = error
        if phase is not None:
            job.filters = {**(job.filters or {}), "phase": phase}
        if total_files is not None:
            job.total_files = total_files
        if total_size_bytes is not None:
            job.total_size_bytes = total_size_bytes
        if files_uploaded is not None:
            job.files_uploaded = files_uploaded
        if bytes_uploaded is not None:
            job.bytes_uploaded = bytes_uploaded
        if status in ("completed", "failed", "cancelled"):
            job.completed_at = datetime.now(timezone.utc)
        db.commit()
    finally:
        db.close()


def is_cloud_operation_job(job: UploadJob) -> bool:
    filters = job.filters or {}
    return filters.get("job_type") in {"cloud_sync", "cloud_move"} or filters.get("transfer_direction") == "cloud_to_cloud"


MAX_LISTED_TRANSFERS = 200


def cloud_operation_filter():
    """SQL equivalent of is_cloud_operation_job, so list queries don't scan every job in Python."""
    return or_(
        UploadJob.filters["job_type"].as_string().in_(["cloud_sync", "cloud_move"]),
        UploadJob.filters["transfer_direction"].as_string() == "cloud_to_cloud",
    )


def cloud_operation_job_to_transfer(job: UploadJob, include_output: bool = True) -> dict:
    filters = job.filters or {}
    summary = job.to_summary_dict()
    destination = f"gs://{job.destination_bucket}"
    if job.destination_path:
        destination += f"/{job.destination_path}"

    return {
        "job_id": job.job_id,
        "status": job.status,
        "source_path": job.source_path,
        "dest_path": destination,
        "dry_run": job.dry_run,
        "recursive": job.recursive,
        "no_clobber": job.no_clobber or False,
        "command": filters.get("display_command"),
        "output": (job.upload_output or "") if include_output else "",
        "created_at": summary["created_at"],
        "completed_at": summary["completed_at"],
        "error": job.error_message,
        "operation": filters.get("operation", "move" if filters.get("job_type") == "cloud_move" else "sync"),
        "source_kind": filters.get("source_kind"),
        "phase": filters.get("phase"),
        "total_files": job.total_files or 0,
        "total_size_bytes": job.total_size_bytes or 0,
        "files_uploaded": job.files_uploaded or 0,
        "bytes_uploaded": job.bytes_uploaded or 0,
    }


@router.get("/bucket-metadata")
async def get_bucket_metadata(path: str):
    """Return advisory bucket metadata for Cloud Ops cost warnings."""
    try:
        value = path.strip()
        if not value.startswith("gs://"):
            raise HTTPException(status_code=400, detail="Path must begin with gs://")
        bucket_name = value[5:].split("/", 1)[0]
        if not bucket_name:
            raise HTTPException(status_code=400, detail="Path must include a bucket")

        def load_metadata():
            bucket = get_storage_client().bucket(bucket_name)
            bucket.reload()
            properties = getattr(bucket, "_properties", {}) or {}
            custom_placement = properties.get("customPlacementConfig") or properties.get("custom_placement_config") or {}
            data_locations = custom_placement.get("dataLocations") or custom_placement.get("data_locations") or []
            return {
                "bucket_name": bucket_name,
                "location": bucket.location,
                "data_locations": data_locations,
                "storage_class": bucket.storage_class,
            }

        return await asyncio.to_thread(load_metadata)
    except HTTPException:
        raise
    except Exception:
        raise HTTPException(
            status_code=502,
            detail="Bucket metadata unavailable. Check bucket access and existence.",
        )


def resolve_gcloud_executable() -> Optional[str]:
    """Resolve gcloud executable path from PATH on all platforms."""
    return shutil.which("gcloud") or shutil.which("gcloud.cmd")


def format_display_command(cmd: list) -> str:
    """Build a clean display command while preserving quoted paths/options."""
    display_cmd = cmd.copy()
    display_cmd[0] = "gcloud"

    parts = []
    for index, arg in enumerate(display_cmd):
        value = str(arg)
        if index >= len(display_cmd) - 2 or " " in value or "\\" in value:
            parts.append(f'"{value}"')
        else:
            parts.append(value)
    return " ".join(parts)


@router.post("/transfer", response_model=CloudTransferResponse)
async def start_cloud_transfer(
    request: CloudTransferRequest,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db)
):
    """
    Start a cloud-to-cloud transfer using gcloud storage rsync.
    """
    job_id = str(uuid.uuid4())[:8]
    
    source = normalize_gcs_path(request.source_path)
    dest = normalize_gcs_path(request.dest_path)

    gcloud_exe = resolve_gcloud_executable()
    if not gcloud_exe:
        raise HTTPException(
            status_code=503,
            detail=(
                "Cloud Sync requires the Google Cloud CLI ('gcloud') but it was not found on PATH. "
                "Install/repair Google Cloud SDK and verify with 'gcloud --version'. "
                "If JetStream was already running, restart it after PATH updates."
            )
        )
    
    # Build command
    cmd = [gcloud_exe, "storage", "rsync"]
    
    if request.recursive:
        cmd.append("--recursive")
    
    if request.dry_run:
        cmd.append("--dry-run")
    
    cmd.append("--checksums-only")

    if request.no_clobber:
        cmd.append("--no-clobber")

    # Combine exclude patterns into a single regex (gcloud accepts one --exclude flag)
    if request.exclude_patterns:
        patterns = [p.strip() for p in request.exclude_patterns if p.strip()]
        if patterns:
            combined = "|".join(f"({p})" for p in patterns)
            cmd.append(f"--exclude={combined}")

    cmd.extend([source, dest])
    
    # Keep the absolute executable path for process launch reliability, but
    # show a clean command in UI to match other pages.
    command_str = format_display_command(cmd)
    dest_bucket, dest_prefix = split_gcs_path(dest)

    job = UploadJob(
        job_id=job_id,
        friendly_name=make_cloud_sync_name(job_id),
        status="running",
        source_path=source,
        destination_bucket=dest_bucket,
        destination_path=dest_prefix,
        total_files=0,
        total_size_bytes=0,
        files_uploaded=0,
        bytes_uploaded=0,
        dry_run=request.dry_run,
        recursive=request.recursive,
        threads=1,
        split_by_folder=False,
        upload_tool="gcloud",
        started_at=datetime.now(timezone.utc),
        filters={
            "job_type": "cloud_sync",
            "transfer_direction": "cloud_to_cloud",
            "display_command": command_str,
            "exclude_patterns": request.exclude_patterns or []
        },
        no_clobber=request.no_clobber,
    )
    db.add(job)
    db.commit()
    
    # Initialize job status
    cloud_transfer_jobs[job_id] = {
        "status": "running",
        "source_path": source,
        "dest_path": dest,
        "dry_run": request.dry_run,
        "command": command_str,
        "output": "",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "completed_at": None,
        "error": None
    }
    
    # Run transfer in background
    background_tasks.add_task(run_cloud_transfer, job_id, cmd)
    
    return CloudTransferResponse(
        job_id=job_id,
        status="running",
        message="Cloud sync started" + (" (dry run)" if request.dry_run else ""),
        command=command_str
    )


@router.post("/move", response_model=CloudMoveResponse)
async def start_cloud_move(
    request: CloudMoveRequest,
    background_tasks: BackgroundTasks,
    db: Session = Depends(get_db),
):
    """Start a generation-safe GCS object or folder move."""
    # The worker only understands exact objects or contents-only prefixes; a whole-folder
    # move is expressed by appending the source folder name to the destination.
    worker_kind = "object" if request.source_kind == "object" else "prefix"
    try:
        source_bucket, source_name, source = parse_move_path(request.source_path, worker_kind)
        dest_bucket, dest_name, dest = parse_move_path(request.dest_path, worker_kind)
        if request.source_kind == "folder":
            folder_name = source_name.rstrip("/").split("/")[-1]
            dest_name = f"{dest_name}{folder_name}/"
            dest = f"gs://{dest_bucket}/{dest_name}"
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc))

    if source_bucket == dest_bucket and source_name == dest_name:
        raise HTTPException(status_code=400, detail="Source and destination must be different")
    if worker_kind == "prefix" and source_bucket == dest_bucket and dest_name.startswith(source_name):
        raise HTTPException(status_code=400, detail="Destination folder cannot be inside the source folder")

    job_id = str(uuid.uuid4())[:8]
    job = UploadJob(
        job_id=job_id,
        friendly_name=f"cloud-move-{datetime.now(timezone.utc).strftime('%Y-%m-%d-%H%M%S')}-{job_id}",
        status="running",
        source_path=source,
        destination_bucket=dest_bucket,
        destination_path=dest_name,
        total_files=0,
        total_size_bytes=0,
        files_uploaded=0,
        bytes_uploaded=0,
        dry_run=request.dry_run,
        recursive=worker_kind == "prefix",
        threads=1,
        split_by_folder=False,
        upload_tool="gcs-sdk",
        started_at=datetime.now(timezone.utc),
        filters={
            "job_type": "cloud_move",
            "transfer_direction": "cloud_to_cloud",
            "operation": "move",
            "source_kind": request.source_kind,
            "phase": "preflight",
        },
        no_clobber=True,
    )
    db.add(job)
    db.commit()

    cloud_transfer_jobs[job_id] = {
        "status": "running",
        "source_path": source,
        "dest_path": dest,
        "operation": "move",
        "source_kind": request.source_kind,
        "dry_run": request.dry_run,
        "phase": "preflight",
        "output": "",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "completed_at": None,
        "error": None,
    }
    background_tasks.add_task(run_cloud_move, job_id, source, dest, worker_kind, request.dry_run)

    return CloudMoveResponse(
        job_id=job_id,
        status="running",
        message="Move preview started" if request.dry_run else "Cloud move started",
        operation="move",
    )


async def run_cloud_move(job_id: str, source: str, destination: str, source_kind: str, dry_run: bool = False):
    """Run a manifest-first copy, verify, and source-delete move."""
    try:
        await asyncio.to_thread(_run_cloud_move_sync, job_id, source, destination, source_kind, dry_run)
    except Exception as exc:
        error = redact_sensitive_text(str(exc))
        job_data = cloud_transfer_jobs.setdefault(job_id, {})
        job_data.update({
            "status": "failed",
            "error": error,
            "completed_at": datetime.now(timezone.utc).isoformat(),
        })
        update_cloud_operation_job(job_id, status="failed", error=error)


def _run_cloud_move_sync(job_id: str, source: str, destination: str, source_kind: str, dry_run: bool = False):
    client = get_storage_client()
    source_bucket_name, source_name, _ = parse_move_path(source, source_kind)
    dest_bucket_name, dest_name, _ = parse_move_path(destination, source_kind)
    source_bucket = client.bucket(source_bucket_name)
    dest_bucket = client.bucket(dest_bucket_name)

    if source_kind == "object":
        source_blobs = [source_bucket.blob(source_name)]
        source_blobs[0].reload()
    else:
        source_blobs = [blob for blob in source_bucket.list_blobs(prefix=source_name) if blob.name != source_name]
    if not source_blobs:
        raise RuntimeError("No source objects found")

    manifest = []
    for blob in source_blobs:
        if blob.generation is None or blob.size is None or blob.crc32c is None:
            blob.reload()
        target_name = dest_name if source_kind == "object" else destination_name_for_move(blob.name, source_name, dest_name)
        if source_bucket_name == dest_bucket_name and blob.name == target_name:
            raise RuntimeError(f"Source and destination resolve to the same object: {blob.name}")
        manifest.append({
            "blob": blob,
            "destination_name": target_name,
            "generation": blob.generation,
            "size": int(blob.size or 0),
            "crc32c": blob.crc32c,
        })

    total_size = sum(item["size"] for item in manifest)
    _update_move_progress(job_id, "preflight", total_files=len(manifest), total_size_bytes=total_size)
    conflicts = [item["destination_name"] for item in manifest if dest_bucket.blob(item["destination_name"]).exists()]
    if conflicts:
        preview = ", ".join(conflicts[:3])
        suffix = "" if len(conflicts) <= 3 else f" and {len(conflicts) - 3} more"
        raise RuntimeError(f"Destination already contains {len(conflicts)} object(s): {preview}{suffix}")

    if dry_run:
        sample = "\n".join(
            f"{item['blob'].name} -> {item['destination_name']}" for item in manifest[:20]
        )
        remaining = "" if len(manifest) <= 20 else f"\n... and {len(manifest) - 20} more"
        output = (
            f"DRY RUN: {len(manifest)} object(s), {total_size} bytes would move.\n"
            f"No destination conflicts were found. Nothing was copied or deleted.\n\n"
            f"{sample}{remaining}"
        )
        cloud_transfer_jobs[job_id].update({
            "status": "completed",
            "phase": "completed",
            "output": output,
            "completed_at": datetime.now(timezone.utc).isoformat(),
        })
        update_cloud_operation_job(job_id, status="completed", output=output, phase="completed")
        return

    created = []
    try:
        _update_move_progress(job_id, "copying")
        for item in manifest:
            copied = source_bucket.copy_blob(
                item["blob"],
                dest_bucket,
                new_name=item["destination_name"],
                if_generation_match=0,
                if_source_generation_match=item["generation"],
            )
            created.append(copied)
            copied.reload()

        _update_move_progress(job_id, "verifying")
        verified_bytes = 0
        for index, (item, copied) in enumerate(zip(manifest, created), start=1):
            if int(copied.size or 0) != item["size"] or copied.crc32c != item["crc32c"]:
                raise RuntimeError(f"Copy verification failed for {item['destination_name']}")
            verified_bytes += item["size"]
            _update_move_progress(
                job_id,
                "verifying",
                files_uploaded=index,
                bytes_uploaded=verified_bytes,
            )
    except Exception as exc:
        rollback_errors = []
        for copied in created:
            try:
                if copied.generation is None:
                    raise RuntimeError("Destination generation unavailable")
                copied.delete(if_generation_match=copied.generation)
            except Exception as rollback_exc:
                rollback_errors.append(f"{copied.name}: {rollback_exc}")
        if rollback_errors:
            raise RuntimeError(f"{exc}; rollback failed for {'; '.join(rollback_errors)}") from exc
        raise

    _update_move_progress(job_id, "deleting")
    for item in manifest:
        item["blob"].delete(if_generation_match=item["generation"])

    output = f"Moved {len(manifest)} object(s), {total_size} bytes"
    cloud_transfer_jobs[job_id].update({
        "status": "completed",
        "phase": "completed",
        "output": output,
        "completed_at": datetime.now(timezone.utc).isoformat(),
    })
    update_cloud_operation_job(job_id, status="completed", output=output, phase="completed")


def _update_move_progress(job_id: str, phase: str, **progress):
    cloud_transfer_jobs.setdefault(job_id, {}).update({"phase": phase, **progress})
    update_cloud_operation_job(job_id, phase=phase, **progress)


async def run_cloud_transfer(job_id: str, cmd: list):
    """Execute the cloud transfer command."""
    try:
        def run_sync():
            print(f"Cloud Transfer: {redact_sensitive_text(' '.join(cmd))}")

            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True
            )

            # Keep only a bounded tail; sync output is one line per object.
            capture = OutputCapture()
            echo = ConsoleEcho()

            def _read_stdout():
                for line in process.stdout:
                    capture.add(line, "stdout")

            def _read_stderr():
                for line in process.stderr:
                    capture.add(line, "stderr")
                    echo(line)

            t1 = threading.Thread(target=_read_stdout, daemon=True)
            t2 = threading.Thread(target=_read_stderr, daemon=True)
            t1.start()
            t2.start()
            t1.join()
            t2.join()
            process.wait()

            return process.returncode, capture.stdout_text(), capture.stderr_text()

        returncode, stdout, stderr = await asyncio.to_thread(run_sync)

        output = ""
        if stdout:
            output += f"STDOUT:\n{stdout}\n\n"
        if stderr:
            output += f"STDERR:\n{stderr}"
        output = cap_output(redact_sensitive_text(output))

        if returncode == 0:
            cloud_transfer_jobs[job_id]["status"] = "completed"
            cloud_transfer_jobs[job_id]["output"] = output
            update_cloud_operation_job(job_id, "completed", output=output)
        else:
            cloud_transfer_jobs[job_id]["status"] = "failed"
            cloud_transfer_jobs[job_id]["output"] = output
            cloud_transfer_jobs[job_id]["error"] = redact_sensitive_text(stderr.strip() or "Unknown error")
            update_cloud_operation_job(
                job_id,
                "failed",
                output=output,
                error=cloud_transfer_jobs[job_id]["error"]
            )

        cloud_transfer_jobs[job_id]["completed_at"] = datetime.now(timezone.utc).isoformat()

    except FileNotFoundError:
        error = (
            "Cloud Sync failed: Google Cloud CLI ('gcloud') executable was not found. "
            "Install/repair Google Cloud SDK, ensure gcloud is on PATH, then restart JetStream."
        )
        cloud_transfer_jobs[job_id]["status"] = "failed"
        cloud_transfer_jobs[job_id]["error"] = error
        cloud_transfer_jobs[job_id]["completed_at"] = datetime.now(timezone.utc).isoformat()
        update_cloud_operation_job(job_id, "failed", error=error)
    except Exception as e:
        error = redact_sensitive_text(str(e))
        cloud_transfer_jobs[job_id]["status"] = "failed"
        cloud_transfer_jobs[job_id]["error"] = error
        cloud_transfer_jobs[job_id]["completed_at"] = datetime.now(timezone.utc).isoformat()
        update_cloud_operation_job(job_id, "failed", error=error)


@router.get("/transfer/{job_id}")
async def get_transfer_status(job_id: str, db: Session = Depends(get_db)):
    """Get status of a cloud transfer job."""
    if job_id in cloud_transfer_jobs:
        job = cloud_transfer_jobs[job_id].copy()
        job["job_id"] = job_id
        return job

    persisted_job = db.query(UploadJob).filter(UploadJob.job_id == job_id).first()
    if persisted_job and is_cloud_operation_job(persisted_job):
        return cloud_operation_job_to_transfer(persisted_job)

    raise HTTPException(status_code=404, detail="Transfer job not found")


@router.get("/transfers")
async def list_transfers(db: Session = Depends(get_db)):
    """List recent cloud transfer jobs."""
    # Output is only shown in the detail view (/transfer/{job_id}); keep the list light.
    persisted_jobs = (
        db.query(UploadJob)
        .options(defer(UploadJob.upload_output))
        .filter(cloud_operation_filter())
        .filter((UploadJob.cleared == False) | (UploadJob.cleared == None))
        .order_by(UploadJob.created_at.desc())
        .limit(MAX_LISTED_TRANSFERS)
        .all()
    )
    jobs = []
    seen_job_ids = set()

    for persisted_job in persisted_jobs:
        if not is_cloud_operation_job(persisted_job):
            continue
        transfer = cloud_operation_job_to_transfer(persisted_job, include_output=False)
        if persisted_job.job_id in cloud_transfer_jobs:
            transfer.update(cloud_transfer_jobs[persisted_job.job_id])
            transfer["job_id"] = persisted_job.job_id
            transfer["output"] = ""
        jobs.append(transfer)
        seen_job_ids.add(persisted_job.job_id)

    for job_id, job_data in cloud_transfer_jobs.items():
        if job_id in seen_job_ids:
            continue
        job = job_data.copy()
        job["job_id"] = job_id
        job["output"] = ""
        jobs.append(job)

    return jobs


@router.post("/transfers/clear-completed")
async def clear_completed_transfers(db: Session = Depends(get_db)):
    """Clear completed cloud transfer jobs from the recent transfers list."""
    cleared = 0
    jobs = db.query(UploadJob).options(defer(UploadJob.upload_output)).filter(
        UploadJob.status.in_(["completed", "failed", "cancelled"]),
        (UploadJob.cleared == False) | (UploadJob.cleared == None),
        cloud_operation_filter(),
    ).all()

    for job in jobs:
        if not is_cloud_operation_job(job):
            continue
        job.cleared = True
        cleared += 1
        cloud_transfer_jobs.pop(job.job_id, None)

    for job_id, job_data in list(cloud_transfer_jobs.items()):
        if job_data.get("status") in ("completed", "failed", "cancelled"):
            cloud_transfer_jobs.pop(job_id, None)
            cleared += 1

    db.commit()
    return {"message": f"Cleared {cleared} transfer(s) from recent list"}
