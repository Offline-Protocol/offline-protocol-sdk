//! Per-session accumulator and MLS pairing.
//!
//! Two things, both landing inside the single `mesh_session_summary` event:
//!
//! - **Pairs** `mls.session_missing` with `mls.session_ready` on the scrubbed
//!   peer id to derive `mls_session_ready_latency_p50_ms`: the time from the
//!   engine wanting a session with a peer and not having one, to that
//!   session being usable.
//!
//!   Not on the MLS `session_id`, and not from `mls.initialized`. That id is
//!   `hash(peer=<id>|group=<id>)` and the group is minted *during* the
//!   handshake, so the id an end carries is never the id the start carried;
//!   `mls.initialized` is emitted once per process for no peer at all. Keyed
//!   either of those ways the pair never forms and the column is always
//!   absent, which is what the frozen TypeScript client shipped.
//! - **Counts** routing decisions into `routing_switches`,
//!   `routing_escalations` and `escalation_reasons`, and `mls.encryption_used`
//!   into `mls_encryption_used_count`.
//!
//! The peer id is an in-memory pairing key only; it is never emitted.
//! Memory is bounded on both axes: the unpaired map by a pairing timeout and
//! a cap, the paired sample by a sliding window. Both reset at a session
//! boundary.
//!
//! Every field is a delta, `session_duration_s` included. The ingest sums
//! summary rows, and one session can legitimately produce more than one
//! (background, an explicit end, a rotation with unreported records), so a
//! repeated total would double every counter on the dashboard, and a
//! duration reported from the session start would count the same seconds
//! once per summary. `take_summary` drains what it reports and advances the
//! anchor the next duration is measured from.

use std::collections::HashMap;

use crate::telemetry::pipe::classify::SessionInput;
use crate::telemetry::pipe::wire::SessionSummary;
use crate::telemetry::routing::{RoutingPhase, RoutingReasonCode};

/// Upper bound on simultaneously pending unpaired peers.
pub(crate) const MAX_PENDING: usize = 256;
/// A pending want older than this is evicted unpaired.
///
/// Ten minutes, not the thirty seconds a local subsystem init would need. A
/// handshake here waits on the *counterparty*: a key package has to reach a
/// peer that may be asleep, out of range, or reachable only over a relay, so
/// a minute-scale establishment is ordinary rather than pathological, and a
/// window shorter than the thing being measured reports only the fast half
/// of the distribution. The cost of the longer window is that a want whose
/// handshake was abandoned can pair with a much later one for the same peer
/// and overstate that sample; the median absorbs it, and the alternative
/// discards the samples that matter most.
pub(crate) const PAIR_TIMEOUT_MS: i64 = 10 * 60_000;
/// Upper bound on retained paired latencies (a sliding window).
pub(crate) const MAX_LATENCIES: usize = 1024;

/// The index into `escalation_reasons` an escalation reason counts under,
/// or `None` for a reason that is not an escalation trigger.
///
/// `fallback_success` and `escalation_applied` are the apply confirmations of
/// an escalation already counted at its trigger, so counting them would double
/// every escalation that worked. `current_unavailable` is a switch reason
/// with no escalation producer today; its bucket is emitted as a real zero
/// because both planes have a column for it.
fn escalation_bucket(code: RoutingReasonCode) -> Option<usize> {
    match code {
        RoutingReasonCode::PoorSignal => Some(0),
        RoutingReasonCode::LowSuccessRate => Some(1),
        RoutingReasonCode::RetryThreshold => Some(2),
        RoutingReasonCode::Congestion => Some(3),
        RoutingReasonCode::LowTtl => Some(4),
        RoutingReasonCode::CurrentUnavailable => Some(5),
        _ => None,
    }
}

#[derive(Debug)]
pub(crate) struct SessionPairer {
    /// Scrubbed peer id to the instant a session with it was first wanted,
    /// awaiting its `session_ready`.
    pending: HashMap<String, i64>,
    /// Paired want-to-ready latencies, most recent last.
    latencies: Vec<i64>,
    /// The instant the last summary reported through. Every duration is
    /// measured from here, not from the session start, so summing the rows
    /// of a session yields the session's length exactly once.
    since_ms: i64,
    switches: u32,
    escalations: [u32; 6],
    encryption_used: u32,
}

impl SessionPairer {
    pub(crate) fn new(now_ms: i64) -> Self {
        Self {
            pending: HashMap::new(),
            latencies: Vec::new(),
            since_ms: now_ms,
            switches: 0,
            escalations: [0; 6],
            encryption_used: 0,
        }
    }

    /// Folds one session-lane record into the running state.
    pub(crate) fn observe(&mut self, input: SessionInput, now_ms: i64) {
        match input {
            SessionInput::MlsSessionWanted { peer, ts_ms } => {
                self.evict_stale(now_ms);
                if !self.pending.contains_key(&peer) && self.pending.len() >= MAX_PENDING {
                    self.drop_oldest_pending();
                }
                // First want wins. The engine reports a miss on every send
                // attempt while the session is still being established, so
                // overwriting would measure from the last attempt before it
                // succeeded and report a latency near zero for a handshake
                // that in fact took minutes.
                self.pending.entry(peer).or_insert(ts_ms);
            }
            SessionInput::MlsSessionReady { peer, ts_ms } => {
                // The timeout holds on this path too: a late `ready` with no
                // intervening want to trigger a sweep would otherwise pair
                // with a long-dead want and inject an oversized latency.
                self.evict_stale(now_ms);
                let Some(init_ts) = self.pending.remove(&peer) else {
                    return;
                };
                if self.latencies.len() >= MAX_LATENCIES {
                    self.latencies.remove(0);
                }
                self.latencies.push((ts_ms - init_ts).max(0));
            }
            SessionInput::EncryptionUsed => self.encryption_used += 1,
            SessionInput::NeighborChurn => {}
        }
    }

    /// Folds one routing decision. Every `switched` counts, whatever its
    /// reason, which makes `routing_switches` a superset of the applied
    /// escalations rather than a disjoint population: a successful fallback
    /// emits a switch too. The two counters are read side by side.
    pub(crate) fn observe_routing(
        &mut self,
        phase: RoutingPhase,
        reason: Option<RoutingReasonCode>,
    ) {
        match phase {
            RoutingPhase::Switched => self.switches += 1,
            RoutingPhase::Escalated => {
                if let Some(bucket) = reason.and_then(escalation_bucket) {
                    self.escalations[bucket] += 1;
                }
            }
            _ => {}
        }
    }

    /// Builds the summary and drains what it reports.
    pub(crate) fn take_summary(&mut self, now_ms: i64) -> SessionSummary {
        let p50 = if self.latencies.is_empty() {
            None
        } else {
            let mut sorted = self.latencies.clone();
            sorted.sort_unstable();
            Some(rounded_median(&sorted))
        };
        let summary = SessionSummary {
            session_duration_s: ((now_ms - self.since_ms) as f64 / 1000.0).round().max(0.0) as i64,
            routing_switches: self.switches,
            routing_escalations: self.escalations.iter().sum(),
            escalation_reasons: self.escalations,
            mls_session_ready_latency_p50_ms: p50,
            mls_encryption_used_count: self.encryption_used,
        };
        self.switches = 0;
        self.escalations = [0; 6];
        self.encryption_used = 0;
        self.latencies.clear();
        // The anchor moves only when a summary actually reports the span, so
        // a boundary that emits nothing leaves those seconds to be carried by
        // the next summary rather than dropping them.
        self.since_ms = now_ms;
        summary
    }

    /// Whether anything observed since the last summary is still unreported.
    /// The duration is not a reason to emit: it is derived, not observed.
    pub(crate) fn has_unreported(&self) -> bool {
        self.switches > 0
            || self.escalations.iter().any(|n| *n > 0)
            || self.encryption_used > 0
            || !self.latencies.is_empty()
    }

    /// Clears all per-session state and restamps the session start.
    pub(crate) fn reset(&mut self, now_ms: i64) {
        self.pending.clear();
        self.latencies.clear();
        self.switches = 0;
        self.escalations = [0; 6];
        self.encryption_used = 0;
        self.since_ms = now_ms;
    }

    fn evict_stale(&mut self, now_ms: i64) {
        let cutoff = now_ms - PAIR_TIMEOUT_MS;
        self.pending.retain(|_, init_ts| *init_ts >= cutoff);
    }

    fn drop_oldest_pending(&mut self) {
        let oldest = self
            .pending
            .iter()
            .min_by_key(|(_, ts)| **ts)
            .map(|(sid, _)| sid.clone());
        if let Some(sid) = oldest {
            self.pending.remove(&sid);
        }
    }
}

/// Median of a sorted sample, rounded to an integer the way the earlier
/// client rounded it: the mean of the two middle samples on an even window,
/// half rounded up.
pub(crate) fn rounded_median(sorted: &[i64]) -> i64 {
    let n = sorted.len();
    if n == 0 {
        return 0;
    }
    let mid = n / 2;
    if n % 2 == 1 {
        sorted[mid]
    } else {
        ((sorted[mid - 1] + sorted[mid]) as f64 / 2.0).round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(peer: &str, ts: i64) -> SessionInput {
        SessionInput::MlsSessionWanted {
            peer: peer.into(),
            ts_ms: ts,
        }
    }

    fn ready(peer: &str, ts: i64) -> SessionInput {
        SessionInput::MlsSessionReady {
            peer: peer.into(),
            ts_ms: ts,
        }
    }

    #[test]
    fn pairs_want_with_ready_and_reports_the_rounded_median() {
        let mut p = SessionPairer::new(0);
        p.observe(wanted("a", 1000), 1000);
        p.observe(ready("a", 1100), 1100);
        p.observe(wanted("b", 2000), 2000);
        p.observe(ready("b", 2201), 2201);
        let s = p.take_summary(3000);
        // (100 + 201) / 2 = 150.5, rounded half up.
        assert_eq!(s.mls_session_ready_latency_p50_ms, Some(151));
        assert_eq!(s.session_duration_s, 3);
    }

    /// The engine reports a miss on every send attempt while a handshake is
    /// still in flight. Measuring from the last one would report a handshake
    /// that took a minute as having taken a millisecond.
    #[test]
    fn repeated_wants_for_one_peer_measure_from_the_first() {
        let mut p = SessionPairer::new(0);
        p.observe(wanted("a", 1_000), 1_000);
        p.observe(wanted("a", 30_000), 30_000);
        p.observe(wanted("a", 60_000), 60_000);
        p.observe(ready("a", 61_000), 61_000);
        assert_eq!(
            p.take_summary(61_000).mls_session_ready_latency_p50_ms,
            Some(60_000)
        );
    }

    #[test]
    fn a_stale_want_never_pairs() {
        let mut p = SessionPairer::new(0);
        p.observe(wanted("a", 0), 0);
        p.observe(ready("a", PAIR_TIMEOUT_MS + 1), PAIR_TIMEOUT_MS + 1);
        assert!(p.take_summary(0).mls_session_ready_latency_p50_ms.is_none());
    }

    /// A mesh handshake waits on the counterparty, so the window has to
    /// outlast a peer that is asleep or reachable only over a relay.
    #[test]
    fn a_handshake_lasting_minutes_still_pairs() {
        let mut p = SessionPairer::new(0);
        let five_minutes = 5 * 60_000;
        p.observe(wanted("a", 0), 0);
        p.observe(ready("a", five_minutes), five_minutes);
        assert_eq!(
            p.take_summary(five_minutes)
                .mls_session_ready_latency_p50_ms,
            Some(five_minutes)
        );
    }

    #[test]
    fn a_ready_without_a_want_is_ignored() {
        let mut p = SessionPairer::new(0);
        p.observe(ready("ghost", 10), 10);
        assert!(!p.has_unreported());
    }

    #[test]
    fn pending_is_bounded_by_dropping_the_oldest() {
        let mut p = SessionPairer::new(0);
        for i in 0..MAX_PENDING {
            p.observe(wanted(&format!("s{i}"), i as i64), 0);
        }
        p.observe(wanted("late", 1000), 0);
        assert_eq!(p.pending.len(), MAX_PENDING);
        assert!(!p.pending.contains_key("s0"));
    }

    #[test]
    fn latencies_are_a_sliding_window() {
        let mut p = SessionPairer::new(0);
        for i in 0..(MAX_LATENCIES + 10) {
            let peer = format!("s{i}");
            p.observe(wanted(&peer, 0), 0);
            p.observe(ready(&peer, i as i64), 0);
        }
        assert_eq!(p.latencies.len(), MAX_LATENCIES);
        assert_eq!(p.latencies[0], 10);
    }

    /// Summing a session's rows must yield the session's length, once.
    #[test]
    fn the_duration_is_a_delta_so_summing_the_rows_yields_the_session() {
        let mut p = SessionPairer::new(0);
        assert_eq!(p.take_summary(205_000).session_duration_s, 205);
        assert_eq!(p.take_summary(260_000).session_duration_s, 55);
        assert_eq!(p.take_summary(300_000).session_duration_s, 40);
    }

    #[test]
    fn escalations_count_only_trigger_reasons_and_switches_count_everything() {
        let mut p = SessionPairer::new(0);
        p.observe_routing(RoutingPhase::Escalated, Some(RoutingReasonCode::PoorSignal));
        p.observe_routing(
            RoutingPhase::Escalated,
            Some(RoutingReasonCode::FallbackSuccess),
        );
        p.observe_routing(RoutingPhase::Escalated, None);
        p.observe_routing(
            RoutingPhase::Switched,
            Some(RoutingReasonCode::PrimarySuccess),
        );
        p.observe_routing(
            RoutingPhase::Switched,
            Some(RoutingReasonCode::EscalationApplied),
        );
        p.observe_routing(
            RoutingPhase::Selected,
            Some(RoutingReasonCode::InitialSelection),
        );
        let s = p.take_summary(0);
        assert_eq!(s.routing_escalations, 1);
        assert_eq!(s.escalation_reasons, [1, 0, 0, 0, 0, 0]);
        assert_eq!(s.routing_switches, 2);
    }

    #[test]
    fn take_summary_drains_so_a_second_summary_reports_only_the_delta() {
        let mut p = SessionPairer::new(0);
        p.observe(SessionInput::EncryptionUsed, 0);
        p.observe(SessionInput::EncryptionUsed, 0);
        assert_eq!(p.take_summary(0).mls_encryption_used_count, 2);
        assert!(!p.has_unreported());
        p.observe(SessionInput::EncryptionUsed, 0);
        assert_eq!(p.take_summary(0).mls_encryption_used_count, 1);
    }

    #[test]
    fn the_median_rounds_half_up_like_the_earlier_client() {
        assert_eq!(rounded_median(&[]), 0);
        assert_eq!(rounded_median(&[5]), 5);
        assert_eq!(rounded_median(&[1, 2]), 2);
        assert_eq!(rounded_median(&[1, 2, 3, 4]), 3);
    }
}
