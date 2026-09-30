"""One connection's state: the application id it declared, its event
filter, and the outbound queue everything it receives goes through.

Every response and every notification a connection receives is pushed onto
one queue and written by one task, in push order. That is what makes the
chapter's ordering rules cheap to keep: the ``hello`` result is pushed, then
the held events, then whatever the router delivers later, and nothing can
overtake anything.
"""

from __future__ import annotations

import asyncio
import itertools
import json
from typing import Any

_ids = itertools.count(1)


class _Close:
    """A close request queued behind everything pushed before it."""

    def __init__(self, code: int, reason: str) -> None:
        self.code = code
        self.reason = reason


class Session:
    """Per-connection state. ``app_id`` is ``None`` until ``hello``."""

    def __init__(self, carrier: str) -> None:
        self.id: int = next(_ids)
        self.carrier = carrier
        self.app_id: str | None = None
        self.client: str | None = None
        #: ``None`` delivers every event the routing rules select; a set
        #: narrows delivery to those tags.
        self.subscriptions: set[str] | None = None
        self._queue: asyncio.Queue[str | _Close] = asyncio.Queue()
        self.closed = False

    # -- the filter ---------------------------------------------------------

    def wants(self, tag: str | None) -> bool:
        if self.subscriptions is None:
            return True
        return tag is not None and tag in self.subscriptions

    def subscribe(self, types: Any) -> None:
        if types == "all":
            self.subscriptions = None
            return
        if self.subscriptions is None:
            self.subscriptions = set()
        self.subscriptions.update(types)

    def unsubscribe(self, types: Any) -> None:
        if types == "all":
            self.subscriptions = set()
            return
        if self.subscriptions is not None:
            self.subscriptions.difference_update(types)

    # -- the outbound queue -------------------------------------------------

    def push(self, payload: dict[str, Any]) -> None:
        if not self.closed:
            self._queue.put_nowait(json.dumps(payload, separators=(",", ":")))

    def push_event(self, event: dict[str, Any]) -> None:
        self.push({"jsonrpc": "2.0", "method": "event", "params": event})

    def push_close(self, code: int, reason: str) -> None:
        self._queue.put_nowait(_Close(code, reason))

    async def sender(self, websocket: Any) -> None:
        """Writes the queue to the socket until a close is queued or the
        socket goes away. Runs as one task per connection."""
        while True:
            item = await self._queue.get()
            if isinstance(item, _Close):
                self.closed = True
                await websocket.close(item.code, item.reason)
                return
            await websocket.send(item)
