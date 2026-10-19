"""In-process job change notifications for the live-updates stream (/api/system/events)."""

import asyncio
import logging
from typing import Optional, Set

logger = logging.getLogger(__name__)

# Per-subscriber backlog; when a slow client falls this far behind, new ids are dropped
# and the client catches up on its next poll.
_QUEUE_MAX = 1000


class EventBroker:
    """Fan job ids out to SSE subscribers.

    Publishers only say *which* job changed; each stream loads the current row itself,
    so callers never have to build payloads. ``notify_job`` is safe to call from any
    thread (upload progress is parsed on subprocess reader threads).
    """

    def __init__(self):
        self._loop: Optional[asyncio.AbstractEventLoop] = None
        self._subscribers: Set[asyncio.Queue] = set()

    def bind_loop(self, loop: Optional[asyncio.AbstractEventLoop] = None) -> None:
        self._loop = loop or asyncio.get_running_loop()

    def subscribe(self) -> asyncio.Queue:
        if self._loop is None:
            self.bind_loop()
        queue: asyncio.Queue = asyncio.Queue(maxsize=_QUEUE_MAX)
        self._subscribers.add(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue) -> None:
        self._subscribers.discard(queue)

    @property
    def subscriber_count(self) -> int:
        return len(self._subscribers)

    def notify_job(self, job_id: Optional[str]) -> None:
        if not job_id or not self._subscribers:
            return
        loop = self._loop
        if loop is None or loop.is_closed():
            return
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is loop:
            self._publish(job_id)
        else:
            try:
                loop.call_soon_threadsafe(self._publish, job_id)
            except RuntimeError:
                # Loop shut down between the check and the call.
                pass

    def _publish(self, job_id: str) -> None:
        for queue in list(self._subscribers):
            try:
                queue.put_nowait(job_id)
            except asyncio.QueueFull:
                pass


broker = EventBroker()


def notify_job(job_id: Optional[str]) -> None:
    """Tell live-update subscribers that a job changed. Never raises."""
    try:
        broker.notify_job(job_id)
    except Exception:
        logger.debug("notify_job failed", exc_info=True)
