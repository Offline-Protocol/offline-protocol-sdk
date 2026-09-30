"""Tests for the verdict tracker: the three rules it exists to enforce."""

from __future__ import annotations

from offline_protocol_sdk.gateway_verdict_tracker import GatewayVerdictTracker


class TestAnIdInFlightIsNotSentAgain:
    def test_begin_records_and_counts(self):
        tracker = GatewayVerdictTracker()
        assert tracker.begin("a", now=1.0)
        assert tracker.begin("b", now=1.0)
        assert tracker.count == 2

    def test_a_second_begin_for_the_same_id_is_refused(self):
        # The core re-queues an unconfirmed frame under the same id; the
        # attempt already outstanding is what settles it.
        tracker = GatewayVerdictTracker()
        assert tracker.begin("a", now=1.0)
        assert not tracker.begin("a", now=2.0)
        assert tracker.count == 1

    def test_a_refused_begin_does_not_refresh_the_clock(self):
        tracker = GatewayVerdictTracker()
        tracker.begin("a", now=1.0)
        tracker.begin("a", now=100.0)
        # Still stale by the first clock.
        assert tracker.expired(now=62.0, timeout=60.0) == ["a"]


class TestEveryIdIsSettledExactlyOnce:
    def test_settle_returns_true_once(self):
        tracker = GatewayVerdictTracker()
        tracker.begin("a", now=1.0)
        assert tracker.settle("a")
        assert not tracker.settle("a")
        assert tracker.count == 0

    def test_an_unsolicited_verdict_settles_nothing(self):
        tracker = GatewayVerdictTracker()
        assert not tracker.settle("never-sent")

    def test_settled_ids_can_be_sent_again(self):
        tracker = GatewayVerdictTracker()
        tracker.begin("a", now=1.0)
        tracker.settle("a")
        assert tracker.begin("a", now=2.0)


class TestNothingIsLeftWaitingOnADeadConnection:
    def test_drain_all_returns_everything_and_empties(self):
        tracker = GatewayVerdictTracker()
        tracker.begin("a", now=1.0)
        tracker.begin("b", now=1.0)
        assert sorted(tracker.drain_all()) == ["a", "b"]
        assert tracker.count == 0
        assert tracker.drain_all() == []

    def test_expired_removes_only_the_stale_ids(self):
        tracker = GatewayVerdictTracker()
        tracker.begin("old", now=0.0)
        tracker.begin("fresh", now=50.0)
        assert tracker.expired(now=61.0, timeout=60.0) == ["old"]
        assert tracker.count == 1
        assert not tracker.settle("old")
        assert tracker.settle("fresh")

    def test_exactly_at_the_timeout_is_not_expired(self):
        # Strictly older than the timeout, as on the other two platforms.
        tracker = GatewayVerdictTracker()
        tracker.begin("a", now=0.0)
        assert tracker.expired(now=60.0, timeout=60.0) == []
        assert tracker.expired(now=60.001, timeout=60.0) == ["a"]
