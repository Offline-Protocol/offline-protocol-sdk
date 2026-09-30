"""Decides which peers to query for gateway presence (``CheckPresence``)
each tick.

A port of ``PresenceWatchPolicy.swift`` and ``PresenceWatchPolicy.kt``.

Watch sources: recipients the gateway reported unreachable
(``DeliveryError``) plus the core watchlist (peers with undelivered or
session-unproven MLS welcomes) merged at tick time. A peer leaves the set on
an online presence answer, on inbound traffic from the peer, or after the
idle TTL.

Queries rotate round-robin so a large watch set is fully covered across
ticks while staying far under a gateway's per-connection rate budget.

The three defaults are hand-mirrored in Swift and Kotlin (C5, the eighth
set); ``test_presence_watch_policy.py`` pins them as literals and the Rust
guard ``presence_watch_defaults_match_across_both_bridges`` reads this file
beside the other two. A tick interval that drifted apart would give the
platforms different presence latency, which reads in the field as a device
problem rather than as a constant.
"""

from __future__ import annotations

import threading

#: Milliseconds a watched peer stays in the set without a reason to.
DEFAULT_IDLE_TTL_MS = 10 * 60_000
#: Peers asked about per tick.
DEFAULT_MAX_QUERIES_PER_TICK = 10
#: Seconds between ticks.
DEFAULT_TICK_INTERVAL = 20.0


class PresenceWatchPolicy:
    def __init__(
        self,
        idle_ttl_ms: int = DEFAULT_IDLE_TTL_MS,
        max_queries_per_tick: int = DEFAULT_MAX_QUERIES_PER_TICK,
    ) -> None:
        self._idle_ttl_ms = idle_ttl_ms
        self._max_queries_per_tick = max_queries_per_tick
        self._last_relevant_at_ms: dict[str, int] = {}
        self._rotation: list[str] = []
        self._lock = threading.Lock()

    def watch(self, peer_id: str, now_ms: int) -> None:
        """Adds a peer to the watch set (or refreshes its idle clock)."""
        if not peer_id:
            return
        with self._lock:
            self._watch_locked(peer_id, now_ms)

    def unwatch(self, peer_id: str) -> None:
        """Removes a peer (an online answer or inbound traffic proved
        reachability)."""
        with self._lock:
            self._unwatch_locked(peer_id)

    def watched_peers(self) -> set[str]:
        with self._lock:
            return set(self._last_relevant_at_ms)

    def peers_to_query(self, core_watchlist: list[str], now_ms: int) -> list[str]:
        """Merges the core watchlist (authoritatively still-pending peers
        refresh their idle clock), evicts idle entries, and returns up to
        ``max_queries_per_tick`` peers to query this tick, round-robin."""
        with self._lock:
            for peer in core_watchlist:
                if peer:
                    self._watch_locked(peer, now_ms)
            expired = [
                peer
                for peer, last in self._last_relevant_at_ms.items()
                if now_ms - last > self._idle_ttl_ms
            ]
            for peer in expired:
                self._unwatch_locked(peer)

            if not self._rotation:
                return []
            count = min(self._max_queries_per_tick, len(self._rotation))
            result: list[str] = []
            for _ in range(count):
                peer = self._rotation.pop(0)
                self._rotation.append(peer)
                result.append(peer)
            return result

    def clear(self) -> None:
        with self._lock:
            self._last_relevant_at_ms.clear()
            self._rotation.clear()

    def _watch_locked(self, peer_id: str, now_ms: int) -> None:
        if peer_id not in self._last_relevant_at_ms:
            self._rotation.append(peer_id)
        self._last_relevant_at_ms[peer_id] = now_ms

    def _unwatch_locked(self, peer_id: str) -> None:
        if self._last_relevant_at_ms.pop(peer_id, None) is not None:
            self._rotation = [p for p in self._rotation if p != peer_id]
