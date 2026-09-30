"""Frames submitted to a gateway and not yet answered.

A port of ``GatewayVerdictTracker.swift`` and ``GatewayVerdictTracker.kt``.
"""

from __future__ import annotations

import threading


class GatewayVerdictTracker:
    """Tracks submitted frames until the gateway's verdict settles them.

    The gateway answers every ``SendMessage`` with exactly one
    ``MessageSent`` or ``DeliveryError``, correlated by ``message_id``. This
    holds the ids between the two, so the manager can bound how many are
    outstanding, notice the ones a gateway never answered, and settle every
    one of them when a connection dies.

    Three rules it exists to enforce, each of which was a real defect on
    some implementation of this contract before it was written down:

    1. **An id already in flight is not sent again.** The core re-queues an
       unconfirmed frame under the same id after its own acknowledgement
       timeout, and a verdict can honestly take longer than that on a slow
       backbone. Sending it twice forwards the frame twice and, when the
       second copy times out, fails an id the gateway already confirmed.
    2. **Every id is settled exactly once.** :meth:`settle` returns whether
       this call was the one that removed it, so a duplicate verdict cannot
       report a second outcome for a frame the core has already moved on
       from.
    3. **Nothing is left waiting on a dead connection.** :meth:`drain_all`
       hands back every outstanding id so the manager can fail them; a frame
       nobody answers for costs the core its full 120-second expiry.

    Thread-safe: the manager confines its own calls to the event loop, but
    the core's wake callback arrives on a Rust thread and the mobile
    trackers are shared between queues, so the lock is kept for parity.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._in_flight: dict[str, float] = {}

    @property
    def count(self) -> int:
        """Frames outstanding right now."""
        with self._lock:
            return len(self._in_flight)

    def begin(self, message_id: str, now: float) -> bool:
        """Records ``message_id`` as submitted at ``now`` (seconds).

        Returns ``False`` when it was already in flight, in which case the
        caller must **not** send it again. Popping it from the core's outbox
        was enough: the attempt already outstanding is what settles that
        id, and the core's pending entry is refreshed by the pop.
        """
        with self._lock:
            if message_id in self._in_flight:
                return False
            self._in_flight[message_id] = now
            return True

    def settle(self, message_id: str) -> bool:
        """Settles ``message_id``. Returns ``False`` if it was not
        outstanding, which is how a duplicate or unsolicited verdict is
        ignored."""
        with self._lock:
            return self._in_flight.pop(message_id, None) is not None

    def expired(self, now: float, timeout: float) -> list[str]:
        """Removes and returns every id submitted more than ``timeout``
        seconds ago.

        A gateway that answers nothing is a contract violation, but it is
        also indistinguishable from one whose socket is wedged, and the core
        cannot retry a frame nobody has failed. Removing them here is what
        turns silence back into a retry.
        """
        with self._lock:
            stale = [
                message_id
                for message_id, began in self._in_flight.items()
                if now - began > timeout
            ]
            for message_id in stale:
                del self._in_flight[message_id]
            return stale

    def drain_all(self) -> list[str]:
        """Removes and returns everything outstanding, for a connection
        that is going away."""
        with self._lock:
            ids = list(self._in_flight)
            self._in_flight.clear()
            return ids
