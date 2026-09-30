"""Tests for the presence watch policy: which peers a tick asks about."""

from __future__ import annotations

from offline_protocol_sdk import presence_watch_policy as module
from offline_protocol_sdk.presence_watch_policy import PresenceWatchPolicy


class TestDefaultsArePinnedAsLiterals:
    # Hand-mirrored in Swift and Kotlin (C5, the eighth set). A tick interval
    # that drifted apart would give the platforms different presence latency.
    def test_idle_ttl(self):
        assert module.DEFAULT_IDLE_TTL_MS == 600_000

    def test_max_queries_per_tick(self):
        assert module.DEFAULT_MAX_QUERIES_PER_TICK == 10

    def test_tick_interval(self):
        assert module.DEFAULT_TICK_INTERVAL == 20.0


class TestWatchSet:
    def test_watch_and_unwatch(self):
        watch = PresenceWatchPolicy()
        watch.watch("a", now_ms=0)
        watch.watch("b", now_ms=0)
        assert watch.watched_peers() == {"a", "b"}
        watch.unwatch("a")
        assert watch.watched_peers() == {"b"}

    def test_an_empty_peer_is_ignored(self):
        watch = PresenceWatchPolicy()
        watch.watch("", now_ms=0)
        assert watch.watched_peers() == set()

    def test_unwatching_an_unknown_peer_is_harmless(self):
        watch = PresenceWatchPolicy()
        watch.unwatch("nobody")
        assert watch.peers_to_query([], now_ms=0) == []

    def test_clear_empties_everything(self):
        watch = PresenceWatchPolicy()
        watch.watch("a", now_ms=0)
        watch.clear()
        assert watch.watched_peers() == set()
        assert watch.peers_to_query([], now_ms=0) == []


class TestPeersToQuery:
    def test_the_core_watchlist_is_merged_in(self):
        watch = PresenceWatchPolicy()
        assert watch.peers_to_query(["x", "y", ""], now_ms=0) == ["x", "y"]
        assert watch.watched_peers() == {"x", "y"}

    def test_queries_rotate_round_robin_under_the_cap(self):
        watch = PresenceWatchPolicy(max_queries_per_tick=2)
        for peer in ("a", "b", "c"):
            watch.watch(peer, now_ms=0)
        assert watch.peers_to_query([], now_ms=0) == ["a", "b"]
        assert watch.peers_to_query([], now_ms=0) == ["c", "a"]
        assert watch.peers_to_query([], now_ms=0) == ["b", "c"]

    def test_idle_peers_are_evicted(self):
        watch = PresenceWatchPolicy(idle_ttl_ms=100)
        watch.watch("stale", now_ms=0)
        watch.watch("fresh", now_ms=90)
        assert watch.peers_to_query([], now_ms=150) == ["fresh"]
        assert watch.watched_peers() == {"fresh"}

    def test_exactly_at_the_ttl_is_not_idle(self):
        watch = PresenceWatchPolicy(idle_ttl_ms=100)
        watch.watch("a", now_ms=0)
        assert watch.peers_to_query([], now_ms=100) == ["a"]
        assert watch.peers_to_query([], now_ms=101) == []

    def test_a_core_watchlist_entry_refreshes_the_idle_clock(self):
        watch = PresenceWatchPolicy(idle_ttl_ms=100)
        watch.watch("a", now_ms=0)
        # Still pending at the core, so it stays.
        assert watch.peers_to_query(["a"], now_ms=150) == ["a"]
        assert watch.peers_to_query([], now_ms=200) == ["a"]
        assert watch.peers_to_query([], now_ms=251) == []

    def test_re_watching_keeps_the_rotation_position(self):
        watch = PresenceWatchPolicy(max_queries_per_tick=1)
        watch.watch("a", now_ms=0)
        watch.watch("b", now_ms=0)
        watch.watch("a", now_ms=5)
        assert watch.peers_to_query([], now_ms=5) == ["a"]
        assert watch.peers_to_query([], now_ms=5) == ["b"]

    def test_the_default_cap_is_ten(self):
        watch = PresenceWatchPolicy()
        peers = [f"p{i}" for i in range(25)]
        assert watch.peers_to_query(peers, now_ms=0) == peers[:10]
        assert watch.peers_to_query([], now_ms=0) == peers[10:20]
        assert watch.peers_to_query([], now_ms=0) == peers[20:] + peers[:5]
