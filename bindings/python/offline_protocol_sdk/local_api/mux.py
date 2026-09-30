"""Event routing and the per-application hold.

Implements the chapter's routing classes (stamped, then the caller of the
call an event was emitted inside, then correlated, then broadcast) and its
replay rule. The event object the engine serialised is pushed unchanged
(C3); this module reads only ``type``, ``app_id``, the correlation
identifiers and ``space_id``.
"""

from __future__ import annotations

import logging
from collections import deque
from typing import Any, Callable, Iterable

from .authz import Policy, ServiceOwnership
from .session import Session

logger = logging.getLogger(__name__)

#: The stamped inbound tags held for an application with no connected client
#: (chapter, "Replay"). The engine has already acknowledged and dedup-marked
#: the message by the time these fire, so nothing will restate them.
HELD_TAGS: frozenset[str] = frozenset({"message_received", "file_received", "media_resend_required"})

#: The hold's capacity per application id: the same real capacity the
#: mobile bridges keep for inbound tags before a subscriber exists (C10).
HOLD_CAPACITY = 256

#: Identifier fields the server correlates by, in the order they are read.
CORRELATION_KEYS: tuple[str, ...] = ("message_id", "file_id", "query_id", "request_id")


class EventRouter:
    """Selects the sessions an event reaches and pushes it to them."""

    def __init__(
        self,
        policy: Policy,
        ownership: ServiceOwnership,
        *,
        on_drop: Callable[[str, dict[str, Any]], None] | None = None,
    ) -> None:
        self._policy = policy
        self._ownership = ownership
        self._sessions: dict[str, list[Session]] = {}
        self._issued: dict[str, str] = {}
        self._held: dict[str, deque[dict[str, Any]]] = {}
        self._on_drop = on_drop
        #: The session whose call the server is executing right now, if any.
        self.current_caller: Session | None = None
        self.dropped = 0

    # -- sessions -------------------------------------------------------------

    def attach(self, session: Session) -> list[dict[str, Any]]:
        """Registers a session under its application id and returns the
        events held for that id, oldest first, emptying the hold."""
        assert session.app_id is not None
        self._sessions.setdefault(session.app_id, []).append(session)
        held = self._held.pop(session.app_id, None)
        return list(held) if held else []

    def detach(self, session: Session) -> None:
        if session.app_id is None:
            return
        sessions = self._sessions.get(session.app_id)
        if sessions and session in sessions:
            sessions.remove(session)
            if not sessions:
                del self._sessions[session.app_id]

    def sessions_for(self, app_id: str) -> list[Session]:
        return list(self._sessions.get(app_id, ()))

    def all_sessions(self) -> list[Session]:
        return [s for sessions in self._sessions.values() for s in sessions]

    def held_count(self, app_id: str) -> int:
        return len(self._held.get(app_id, ()))

    # -- identifiers ----------------------------------------------------------

    def note_ids(self, app_id: str, ids: Iterable[Any]) -> None:
        """Records identifiers the server handed to ``app_id`` as results."""
        for value in ids:
            if isinstance(value, str) and value:
                self._issued[value] = app_id

    def _record_from_event(self, app_id: str, event: dict[str, Any]) -> None:
        for key in CORRELATION_KEYS:
            value = event.get(key)
            if isinstance(value, str) and value:
                self._issued.setdefault(value, app_id)

    def _correlated_app(self, event: dict[str, Any]) -> str | None:
        for key in CORRELATION_KEYS:
            value = event.get(key)
            if isinstance(value, str) and value in self._issued:
                return self._issued[value]
        service_id = event.get("service_id")
        if isinstance(service_id, str):
            return self._ownership.owner(service_id)
        return None

    # -- routing --------------------------------------------------------------

    def route(self, event: dict[str, Any]) -> None:
        """Pushes ``event`` to the sessions the rules select."""
        tag = event.get("type")
        app_id = event.get("app_id")
        if isinstance(app_id, str):
            targets = self.sessions_for(app_id)
            if not targets and tag in HELD_TAGS:
                self._hold(app_id, event)
                return
        elif tag in HELD_TAGS:
            # A stamped tag whose id is absent (a `file_received` assembled
            # with no metadata entry, a `media_resend_required` without one)
            # has nothing to route or hold by, so it is broadcast, and said.
            logger.info("%s carries no app_id: broadcast, not held", tag)
            targets = self.all_sessions()
        elif self.current_caller is not None and self.current_caller.app_id is not None:
            caller = self.current_caller
            self._record_from_event(caller.app_id, event)
            targets = self.sessions_for(caller.app_id)
        else:
            owner = self._correlated_app(event)
            targets = self.sessions_for(owner) if owner is not None else self.all_sessions()
        for session in targets:
            self._deliver(session, tag, event)

    def _deliver(self, session: Session, tag: Any, event: dict[str, Any]) -> None:
        if not session.wants(tag):
            return
        if isinstance(tag, str) and tag.startswith("data_"):
            # The reference server filters document events by the space
            # allow-list, which the chapter allows a server to add.
            space_id = event.get("space_id")
            if isinstance(space_id, str) and session.app_id is not None:
                if not self._policy.allows_space(session.app_id, space_id):
                    return
        session.push_event(event)

    def _hold(self, app_id: str, event: dict[str, Any]) -> None:
        held = self._held.get(app_id)
        if held is None:
            held = self._held[app_id] = deque()
        if len(held) >= HOLD_CAPACITY:
            dropped = held.popleft()
            self.dropped += 1
            logger.warning(
                "hold for %r is full (%d): dropping the oldest %s",
                app_id,
                HOLD_CAPACITY,
                dropped.get("type"),
            )
            if self._on_drop is not None:
                self._on_drop(app_id, dropped)
        held.append(event)
