"""Event routing and the per-application hold.

Implements the chapter's routing classes (stamped, then the caller of the
call an event was emitted inside, then correlated, then broadcast) and its
replay rule. The event object the engine serialised is pushed unchanged
(C3); this module reads only ``type``, ``app_id``, the correlation
identifiers and ``space_id``.
"""

from __future__ import annotations

import logging
from collections import OrderedDict, deque
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

#: How many issued identifiers are remembered. An identifier is forgotten on
#: its terminal event; one whose terminal event never comes (a query, a
#: message the engine gave up on silently) is evicted oldest-first past
#: this, after which its events are broadcast, which is what the chapter
#: says happens to an identifier the server does not know.
ISSUED_CAPACITY = 65536

#: How many settled identifiers still route to their owner. The engine emits
#: some events after the one that settles an id, in the same breath: a
#: connection request it gives up on reports ``message_failed`` and then
#: ``connection_request_undeliverable`` for the same id. Forgetting the id
#: outright on the first broadcast the second, recipient and all, to every
#: client. Settled ids sit in their own table so they never crowd a live
#: one (a message parked for days) out of the issued table.
SETTLED_CAPACITY = 1024

#: The event that settles an identifier, after which it is no longer live.
#: Neither ``message_undeliverable`` nor ``connection_request_undeliverable``
#: is one: on a relay verdict the engine keeps the message (or request) in
#: its outbox, may repeat the event on every reachability probe, and settles
#: it later with ``message_delivered`` or ``message_failed``. Treating either
#: as the last word broadcast that later receipt to every client.
TERMINAL_TAGS: dict[str, str] = {
    "message_delivered": "message_id",
    "message_failed": "message_id",
    "media_sent": "file_id",
    "media_send_failed": "file_id",
    "service_response_received": "request_id",
}


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
        self._issued: OrderedDict[str, str] = OrderedDict()
        self._settled: OrderedDict[str, str] = OrderedDict()
        self._held: dict[str, deque[dict[str, Any]]] = {}
        #: Events the run loop emitted while a call was in flight, naming an
        #: identifier nobody owned yet. Routed once the call's result is
        #: recorded (see `flush_parked`).
        self._parked: list[dict[str, Any]] = []
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

    def issued_count(self) -> int:
        return len(self._issued)

    def knows(self, identifier: str) -> bool:
        return identifier in self._issued

    def note_ids(self, app_id: str, ids: Iterable[Any]) -> None:
        """Records identifiers the server handed to ``app_id`` as results."""
        for value in ids:
            if isinstance(value, str) and value:
                self._remember(value, app_id)

    def _remember(self, identifier: str, app_id: str) -> None:
        self._issued[identifier] = app_id
        self._issued.move_to_end(identifier)
        while len(self._issued) > ISSUED_CAPACITY:
            self._issued.popitem(last=False)

    def _record_from_event(self, app_id: str, event: dict[str, Any]) -> None:
        for key in CORRELATION_KEYS:
            value = event.get(key)
            if isinstance(value, str) and value and value not in self._issued:
                self._remember(value, app_id)

    def _correlated_app(self, event: dict[str, Any]) -> str | None:
        for key in CORRELATION_KEYS:
            value = event.get(key)
            if isinstance(value, str) and value in self._issued:
                return self._issued[value]
        for key in CORRELATION_KEYS:
            value = event.get(key)
            if isinstance(value, str) and value in self._settled:
                return self._settled[value]
        service_id = event.get("service_id")
        if isinstance(service_id, str):
            return self._ownership.owner(service_id)
        return None

    def _forget_terminal(self, tag: Any, event: dict[str, Any]) -> None:
        key = TERMINAL_TAGS.get(tag) if isinstance(tag, str) else None
        if key is None:
            return
        value = event.get(key)
        if not isinstance(value, str):
            return
        owner = self._issued.pop(value, None)
        if owner is not None:
            self._settled[value] = owner
            self._settled.move_to_end(value)
            while len(self._settled) > SETTLED_CAPACITY:
                self._settled.popitem(last=False)

    # -- routing --------------------------------------------------------------

    def route(self, event: dict[str, Any], *, in_call: bool = False) -> None:
        """Pushes ``event`` to the sessions the rules select.

        ``in_call`` says the event was emitted on the thread executing a
        client's call. Only then does the caller rule apply: an event the
        run loop emits while a call is in flight on the executor is not the
        caller's, and would be misattributed to it otherwise.
        """
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
        elif in_call and self.current_caller is not None and self.current_caller.app_id is not None:
            caller = self.current_caller
            self._record_from_event(caller.app_id, event)
            targets = self.sessions_for(caller.app_id)
        else:
            owner = self._correlated_app(event)
            if owner is None and self.current_caller is not None and self._names_an_identifier(event):
                # The run loop emitted this while a call was in flight, and
                # nobody owns the identifier it names yet. Between the
                # executor's completion and the wakeup that records the
                # call's result, one or two loop iterations run; a `process()`
                # tick landing there can emit `message_sent` for the very id
                # the call is about to return, content and all. Broadcasting
                # it would hand one application's message to every other, so
                # it waits for the result to be recorded.
                self._parked.append(event)
                return
            targets = self.sessions_for(owner) if owner is not None else self.all_sessions()
        for session in targets:
            self._deliver(session, tag, event)
        self._forget_terminal(tag, event)

    def flush_parked(self) -> None:
        """Routes what was parked during a call, now that its identifiers are
        recorded. Called with no caller set, after the call's result was
        noted, or after its failure, when the events route by the ordinary
        rules (an unknown identifier broadcasts) but never with the caller's
        own identifiers still unknown."""
        parked, self._parked = self._parked, []
        for event in parked:
            self.route(event)

    def parked_count(self) -> int:
        return len(self._parked)

    @staticmethod
    def _names_an_identifier(event: dict[str, Any]) -> bool:
        return any(
            isinstance(event.get(key), str) and event.get(key) for key in CORRELATION_KEYS
        )

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
