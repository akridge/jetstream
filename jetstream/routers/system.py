"""Live job updates (Server-Sent Events) and Google Cloud sign-in status."""

import asyncio
import json
import logging
import subprocess
import sys
from typing import Dict, Iterable, List

from fastapi import APIRouter, HTTPException, Request
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import StreamingResponse
from sqlalchemy.orm import defer

from .. import database
from ..database import UploadJob
from ..events import broker
from ..services import check_gcloud_auth, clear_auth_cache, find_gcloud

logger = logging.getLogger(__name__)

router = APIRouter()

TERMINAL_STATUSES = {"completed", "failed", "cancelled"}
_BATCH_SECONDS = 0.5
_HEARTBEAT_SECONDS = 15.0
_LOOPBACK_HOSTS = {"127.0.0.1", "::1", "localhost"}

_login_process = {"proc": None}


def format_sse(event: str, data: dict) -> str:
    return f"event: {event}\ndata: {json.dumps(data, default=str)}\n\n"


def build_job_events(db, job_ids: Iterable[str], last_status: Dict[str, str]) -> List[str]:
    """SSE messages for changed jobs.

    Emits ``job`` with the list-view summary for every changed job, ``job_finished``
    when a job reaches a terminal status it wasn't in before (drives notifications),
    and ``job_removed`` for deleted jobs. ``last_status`` is per-connection state.
    """
    ids = list(dict.fromkeys(job_ids))
    rows = (
        db.query(UploadJob)
        .options(defer(UploadJob.upload_output))
        .filter(UploadJob.job_id.in_(ids))
        .all()
    )
    messages = []
    found = set()
    for row in rows:
        found.add(row.job_id)
        summary = row.to_summary_dict()
        messages.append(format_sse("job", summary))
        previous = last_status.get(row.job_id)
        last_status[row.job_id] = row.status
        if row.status in TERMINAL_STATUSES and previous != row.status:
            messages.append(format_sse("job_finished", {
                "job_id": row.job_id,
                "name": row.friendly_name or row.job_id,
                "status": row.status,
                "job_type": summary.get("job_type"),
                "destination": f"gs://{row.destination_bucket}/{row.destination_path or ''}",
                "completed_at": summary.get("completed_at"),
                "error_message": row.error_message,
                "dry_run": bool(row.dry_run),
            }))
    for job_id in ids:
        if job_id not in found:
            last_status.pop(job_id, None)
            messages.append(format_sse("job_removed", {"job_id": job_id}))
    return messages


def _load_job_events(job_ids, last_status) -> List[str]:
    if database.SessionLocal is None:
        return []
    db = database.SessionLocal()
    try:
        return build_job_events(db, job_ids, last_status)
    finally:
        db.close()


async def _event_stream(request: Request, queue: asyncio.Queue):
    last_status: Dict[str, str] = {}
    try:
        yield "retry: 3000\n\n"
        yield format_sse("hello", {"subscribers": broker.subscriber_count})
        while True:
            if await request.is_disconnected():
                break
            try:
                first = await asyncio.wait_for(queue.get(), timeout=_HEARTBEAT_SECONDS)
            except asyncio.TimeoutError:
                yield ": keep-alive\n\n"
                continue
            # Coalesce bursts (progress writes, batch creates) into one DB read.
            await asyncio.sleep(_BATCH_SECONDS)
            job_ids = [first]
            while not queue.empty():
                job_ids.append(queue.get_nowait())
            for message in await run_in_threadpool(_load_job_events, job_ids, last_status):
                yield message
    finally:
        broker.unsubscribe(queue)


@router.get("/events")
async def job_events(request: Request):
    """Stream job changes so pages update without polling."""
    queue = broker.subscribe()
    return StreamingResponse(
        _event_stream(request, queue),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )


@router.get("/auth-status")
async def auth_status(refresh: bool = False):
    """Google Cloud sign-in state for the header badge."""
    result = await run_in_threadpool(check_gcloud_auth, refresh)
    proc = _login_process["proc"]
    result["login_in_progress"] = proc is not None and proc.poll() is None
    return result


@router.post("/auth-login")
async def auth_login(request: Request):
    """Start `gcloud auth login --update-adc` on this computer.

    gcloud opens the user's browser itself. Only allowed from this computer, since
    the server may be reachable from the network.
    """
    client_host = request.client.host if request.client else ""
    if client_host not in _LOOPBACK_HOSTS:
        raise HTTPException(status_code=403, detail="Sign-in can only be started from this computer")

    gcloud = find_gcloud()
    if not gcloud:
        raise HTTPException(
            status_code=400,
            detail="Google Cloud SDK (gcloud) is not installed. Install it from https://cloud.google.com/sdk/docs/install",
        )

    proc = _login_process["proc"]
    if proc is not None and proc.poll() is None:
        return {"status": "already_running"}

    cmd = [gcloud, "auth", "login", "--update-adc"]
    kwargs = {"stdin": subprocess.DEVNULL}
    if sys.platform == "win32":
        # Own console window so the user can see prompts and errors.
        kwargs["creationflags"] = subprocess.CREATE_NEW_CONSOLE
    else:
        kwargs["start_new_session"] = True
        kwargs["stdout"] = subprocess.DEVNULL
        kwargs["stderr"] = subprocess.DEVNULL
    try:
        _login_process["proc"] = subprocess.Popen(cmd, **kwargs)
    except OSError as e:
        raise HTTPException(status_code=500, detail=f"Could not start gcloud: {e}")
    clear_auth_cache()
    return {"status": "started"}
