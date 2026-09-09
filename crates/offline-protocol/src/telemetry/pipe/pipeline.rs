//! The in-memory half of the pipe: the ring buffer, the session id, the
//! rollup and the session tracker, and the lifecycle rules that draw
//! session boundaries.
//!
//! Everything here runs on the emit path under the pipe's own mutex and
//! never performs I/O. The uploader takes that mutex only to drain the ring,
//! which is one `mem::take`.

use std::collections::VecDeque;

use uuid::Uuid;

use crate::events::Event;
use crate::telemetry::pipe::classify::{
    classify, classify_protocol, ClassifyCtx, Disposition, MessageKind,
};
use crate::telemetry::pipe::host::AppState;
use crate::telemetry::pipe::rollup::RollupAggregator;
use crate::telemetry::pipe::session_pairer::SessionPairer;
use crate::telemetry::pipe::wire::{BufferedEvent, WireEvent, WireEventData};
use crate::telemetry::TelemetryRecord;

/// What a lifecycle transition asked the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleAction {
    /// Nothing: the state was not an edge.
    None,
    /// The app went to the background: a summary was recorded; flush now.
    BackgroundFlush,
    /// The app came back: the session was rotated.
    Rotated,
}

#[derive(Debug)]
pub(crate) struct Pipeline {
    buffer: VecDeque<BufferedEvent>,
    capacity: usize,
    dropped: u64,
    session_id: Uuid,
    rollup: RollupAggregator,
    pairer: SessionPairer,
    routing_diagnostic: bool,
    was_backgrounded: bool,
    /// Whether this session has already had a summary reported for it. The
    /// first boundary always reports, because the session's duration is the
    /// point of the row; a second boundary with nothing observed since does
    /// not, because that row would be billed for repeating what the first
    /// one said.
    summary_reported: bool,
    /// Bytes buffered since the last flush, by the cheap per-event estimate.
    buffered_bytes: usize,
    max_batch_bytes: usize,
}

impl Pipeline {
    pub(crate) fn new(
        capacity: usize,
        max_batch_bytes: usize,
        routing_diagnostic: bool,
        now_ms: i64,
    ) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity.min(256)),
            capacity: capacity.max(1),
            dropped: 0,
            session_id: Uuid::new_v4(),
            rollup: RollupAggregator::new(),
            pairer: SessionPairer::new(now_ms),
            routing_diagnostic,
            was_backgrounded: false,
            summary_reported: false,
            buffered_bytes: 0,
            max_batch_bytes,
        }
    }

    /// Routes one record. Returns whether the size trigger fired.
    pub(crate) fn handle(&mut self, record: &TelemetryRecord, now_ms: i64) -> bool {
        let ctx = ClassifyCtx {
            routing_diagnostic: self.routing_diagnostic,
            now_ms,
        };
        self.apply(classify(record, &ctx), now_ms)
    }

    /// Routes one protocol event by reference. Returns whether the size
    /// trigger fired.
    pub(crate) fn handle_protocol_event(&mut self, event: &Event, now_ms: i64) -> bool {
        self.apply(classify_protocol(event, now_ms), now_ms)
    }

    fn apply(&mut self, disposition: Disposition, now_ms: i64) -> bool {
        match disposition {
            Disposition::Drop => false,
            Disposition::CountSent => {
                self.rollup.count_message(MessageKind::Sent);
                false
            }
            Disposition::Rollup(sample) => match self.rollup.add(sample) {
                Some(event) => self.push(event),
                None => false,
            },
            Disposition::Session(input) => {
                self.pairer.observe(input, now_ms);
                false
            }
            Disposition::ForwardAndObserve(event, input) => {
                self.pairer.observe(input, now_ms);
                self.push(event)
            }
            Disposition::Forward(event) => {
                // Forwarded events that are also tapped: the per-window send
                // counts, and the session's routing counters. Forwarding and
                // counting feed different columns, so a record goes both ways.
                match &event.data {
                    WireEventData::MessageDelivered { .. } => {
                        self.rollup.count_message(MessageKind::Delivered);
                    }
                    WireEventData::MessageFailed { .. } => {
                        self.rollup.count_message(MessageKind::Failed);
                    }
                    WireEventData::RoutingDecision {
                        phase, reason_code, ..
                    } => {
                        self.pairer.observe_routing(*phase, *reason_code);
                    }
                    _ => {}
                }
                self.push(event)
            }
        }
    }

    /// The single choke point for every buffer write, so byte accounting
    /// cannot miss a source and the session id is stamped in one place.
    fn push(&mut self, event: WireEvent) -> bool {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
            self.dropped += 1;
        }
        self.buffered_bytes += event.data.approx_bytes();
        self.buffer.push_back(BufferedEvent {
            session: self.session_id,
            event,
        });
        if self.buffered_bytes >= self.max_batch_bytes {
            // Reset now so a burst does not re-trigger on every push.
            self.buffered_bytes = 0;
            true
        } else {
            false
        }
    }

    /// Pushes a summary for the current session, closing the open rollup
    /// window first so its rollup lands before the summary.
    ///
    /// The first boundary of a session always reports: the duration is the
    /// row's reason for existing, and every session that reached a boundary
    /// has one. A later boundary in the same session reports only if
    /// something has been observed since the last summary. Backgrounding and
    /// then calling `endTelemetrySession` is the sequence `docs/telemetry.md`
    /// calls safe, and without this it bills a second row carrying nothing.
    /// The rollup is flushed either way: it is a different event with its own
    /// contents, and whether a metrics window closes must not depend on what
    /// the session lane happened to observe.
    pub(crate) fn emit_session_summary(&mut self, now_ms: i64) {
        if let Some(event) = self.rollup.flush() {
            self.push(event);
        }
        if self.summary_reported && !self.pairer.has_unreported() {
            return;
        }
        self.record_summary(now_ms);
    }

    /// Pushes the summary alone, leaving the rollup's open window open.
    ///
    /// The rescue in [`Self::rotate_session`] must not drag the rollup along:
    /// were it to, whether a metrics window closed at a foreground boundary
    /// would depend on whether the session lane happened to observe a record
    /// in the background, two unrelated lanes deciding each other's
    /// boundaries. The rollup closes on background, and otherwise only by a
    /// later-bucket frame.
    fn record_summary(&mut self, now_ms: i64) {
        self.summary_reported = true;
        let summary = self.pairer.take_summary(now_ms);
        self.push(WireEvent {
            ts_ms: now_ms,
            data: WireEventData::SessionSummary(summary),
        });
    }

    /// Resets per-session state and mints a fresh session id, reporting
    /// anything observed since the last summary first.
    ///
    /// The window this rescues is real: the summary goes out when the app
    /// backgrounds, the reset happens when it returns, and the engine keeps
    /// working in between. The rescued summary is stamped with the session it
    /// reports, because [`Self::push`] stamps at push time and the id changes
    /// only after.
    pub(crate) fn rotate_session(&mut self, now_ms: i64) {
        if self.pairer.has_unreported() {
            self.record_summary(now_ms);
        }
        self.pairer.reset(now_ms);
        self.session_id = Uuid::new_v4();
        self.summary_reported = false;
    }

    /// Records an application state without drawing a boundary from it.
    ///
    /// Two callers, one reason: the state is a fact about the app that the
    /// next edge is measured against, and neither caller has a session to
    /// report.
    ///
    /// At enable time there is nothing to close. The pipe was built moments
    /// ago and the pairer has observed nothing, so running the `Background`
    /// arm of [`Self::app_state`] there would push an all-zero summary with a
    /// zero duration and wake the uploader for it: one billed event on every
    /// background launch (an iOS BLE restoration, an Android headless mesh
    /// wake). What the seed is actually for is the *next* edge, so that a
    /// pipe enabled in the background rotates on the first foreground instead
    /// of ignoring it.
    ///
    /// With collection switched off the same holds for a different reason:
    /// the edge is real but reporting it is collection, so the transition is
    /// tracked and nothing is emitted. `Inactive` is not an edge on either
    /// path and leaves the state alone.
    pub(crate) fn seed_app_state(&mut self, state: AppState) {
        match state {
            AppState::Background => self.was_backgrounded = true,
            AppState::Active => self.was_backgrounded = false,
            AppState::Inactive => {}
        }
    }

    /// Applies one application-state observation.
    ///
    /// Edges are debounced: one background-then-foreground cycle produces
    /// exactly one summary and one rotation, and `Inactive` is never an edge.
    pub(crate) fn app_state(&mut self, state: AppState, now_ms: i64) -> LifecycleAction {
        match state {
            AppState::Background => {
                if self.was_backgrounded {
                    return LifecycleAction::None;
                }
                self.was_backgrounded = true;
                self.emit_session_summary(now_ms);
                LifecycleAction::BackgroundFlush
            }
            AppState::Active => {
                if !self.was_backgrounded {
                    return LifecycleAction::None;
                }
                self.was_backgrounded = false;
                self.rotate_session(now_ms);
                LifecycleAction::Rotated
            }
            AppState::Inactive => LifecycleAction::None,
        }
    }

    /// Removes and returns everything buffered, in FIFO order.
    pub(crate) fn drain(&mut self) -> VecDeque<BufferedEvent> {
        self.buffered_bytes = 0;
        std::mem::take(&mut self.buffer)
    }

    pub(crate) fn buffered(&self) -> usize {
        self.buffer.len()
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(crate) fn session_id(&self) -> Uuid {
        self.session_id
    }

    #[cfg(test)]
    pub(crate) fn buffer(&self) -> &VecDeque<BufferedEvent> {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::pipe::classify::SessionInput;
    use crate::telemetry::pipe::wire::SessionSummary;

    fn summaries(p: &Pipeline) -> Vec<(Uuid, SessionSummary)> {
        p.buffer()
            .iter()
            .filter_map(|b| match &b.event.data {
                WireEventData::SessionSummary(s) => Some((b.session, s.clone())),
                _ => None,
            })
            .collect()
    }

    fn pipeline() -> Pipeline {
        Pipeline::new(16, 1 << 20, false, 0)
    }

    #[test]
    fn summary_counters_are_deltas_not_running_totals() {
        let mut p = pipeline();
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        p.emit_session_summary(1_000);
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        p.emit_session_summary(2_000);
        let counts: Vec<u32> = summaries(&p)
            .iter()
            .map(|(_, s)| s.mls_encryption_used_count)
            .collect();
        assert_eq!(counts, vec![2, 1]);
    }

    #[test]
    fn p50_is_omitted_when_the_window_is_empty() {
        let mut p = pipeline();
        p.emit_session_summary(1_000);
        let (_, s) = &summaries(&p)[0];
        assert!(s.mls_session_ready_latency_p50_ms.is_none());
        assert_eq!(
            s.mls_session_ready_latency_p50_ms.map(|_| ()),
            None,
            "absent, not zero"
        );
    }

    #[test]
    fn p50_and_neighbor_p50_are_integers_on_the_wire() {
        let mut p = pipeline();
        p.pairer.observe(
            SessionInput::MlsSessionWanted {
                peer: "a".into(),
                ts_ms: 0,
            },
            0,
        );
        p.pairer.observe(
            SessionInput::MlsSessionReady {
                peer: "a".into(),
                ts_ms: 101,
            },
            101,
        );
        p.pairer.observe(
            SessionInput::MlsSessionWanted {
                peer: "b".into(),
                ts_ms: 0,
            },
            0,
        );
        p.pairer.observe(
            SessionInput::MlsSessionReady {
                peer: "b".into(),
                ts_ms: 102,
            },
            102,
        );
        p.emit_session_summary(1_000);
        let json = p.buffer()[0].event.data.to_json();
        assert!(json["mls_session_ready_latency_p50_ms"].is_i64());
        assert_eq!(json["mls_session_ready_latency_p50_ms"], 102);
    }

    #[test]
    fn batches_are_cut_on_session_id_stamped_at_push_time() {
        let mut p = pipeline();
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        let first = p.session_id();
        assert_eq!(
            p.app_state(AppState::Background, 10),
            LifecycleAction::BackgroundFlush
        );
        assert_eq!(p.app_state(AppState::Active, 20), LifecycleAction::Rotated);
        let second = p.session_id();
        assert_ne!(first, second);
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        p.emit_session_summary(30);
        let stamped: Vec<Uuid> = p.buffer().iter().map(|b| b.session).collect();
        assert_eq!(stamped, vec![first, second]);
    }

    #[test]
    fn rescue_summary_goes_out_under_the_old_session_id() {
        let mut p = pipeline();
        assert_eq!(
            p.app_state(AppState::Background, 10),
            LifecycleAction::BackgroundFlush
        );
        let old = p.session_id();
        // Observed in the background, after the boundary summary.
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        assert_eq!(p.app_state(AppState::Active, 20), LifecycleAction::Rotated);
        let rescued = summaries(&p);
        assert_eq!(rescued.len(), 2);
        assert_eq!(rescued[1].0, old);
        assert_eq!(rescued[1].1.mls_encryption_used_count, 1);
        assert_ne!(p.session_id(), old);
    }

    #[test]
    fn lifecycle_edges_are_debounced_and_inactive_is_ignored() {
        let mut p = pipeline();
        assert_eq!(p.app_state(AppState::Active, 0), LifecycleAction::None);
        assert_eq!(p.app_state(AppState::Inactive, 0), LifecycleAction::None);
        assert_eq!(
            p.app_state(AppState::Background, 0),
            LifecycleAction::BackgroundFlush
        );
        assert_eq!(p.app_state(AppState::Background, 0), LifecycleAction::None);
        assert_eq!(p.app_state(AppState::Inactive, 0), LifecycleAction::None);
        assert_eq!(p.app_state(AppState::Active, 0), LifecycleAction::Rotated);
        assert_eq!(p.app_state(AppState::Active, 0), LifecycleAction::None);
        assert_eq!(summaries(&p).len(), 1);
    }

    /// Seeding records the state and reports nothing.
    ///
    /// The failure this catches: a pipe enabled by a background launch
    /// pushing an all-zero summary with a zero duration, which the ingest
    /// stores and bills as an accepted event. What the seed is for is the
    /// next edge, and both directions of it are pinned here.
    #[test]
    fn seeding_records_the_state_without_reporting_a_session() {
        let mut p = pipeline();
        p.seed_app_state(AppState::Background);
        assert_eq!(p.buffered(), 0, "a seed emits nothing");
        // The seed armed the boundary: the first foreground rotates.
        let first = p.session_id();
        assert_eq!(p.app_state(AppState::Active, 10), LifecycleAction::Rotated);
        assert_ne!(p.session_id(), first);

        // Seeded active, a foreground is not an edge and a background is.
        let mut p = pipeline();
        p.seed_app_state(AppState::Active);
        assert_eq!(p.app_state(AppState::Active, 10), LifecycleAction::None);
        assert_eq!(
            p.app_state(AppState::Background, 20),
            LifecycleAction::BackgroundFlush
        );

        // `Inactive` is not a state to seed from.
        let mut p = pipeline();
        p.seed_app_state(AppState::Background);
        p.seed_app_state(AppState::Inactive);
        assert_eq!(p.app_state(AppState::Active, 10), LifecycleAction::Rotated);
    }

    #[test]
    fn the_ring_drops_oldest_and_counts_it() {
        let mut p = Pipeline::new(2, 1 << 20, false, 0);
        for i in 0..3 {
            // Something observed between the boundaries, so each one has a
            // row to report: a repeat boundary with nothing new is
            // deliberately not one. See `emit_session_summary`.
            p.pairer.observe(SessionInput::EncryptionUsed, i);
            p.emit_session_summary(i);
        }
        assert_eq!(p.buffered(), 2);
        assert_eq!(p.dropped(), 1);
        assert_eq!(p.buffer()[0].event.ts_ms, 1);
    }

    /// Backgrounding and then calling `endTelemetrySession` is the sequence
    /// `docs/telemetry.md` calls safe. Each summary is a row the ingest
    /// stores and bills, so the second one must not repeat the first.
    #[test]
    fn a_repeat_boundary_with_nothing_observed_does_not_bill_a_second_row() {
        let mut p = pipeline();
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        p.emit_session_summary(1_000);
        p.emit_session_summary(2_000);
        assert_eq!(summaries(&p).len(), 1);

        // Anything observed since makes the next boundary a row again, and
        // its duration covers the whole span the skipped one would have.
        p.pairer.observe(SessionInput::EncryptionUsed, 0);
        p.emit_session_summary(4_000);
        let rows = summaries(&p);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].1.session_duration_s, 3);
    }

    /// A session that closes without ever reporting is a session the ingest
    /// never sees, so the first boundary reports whatever it has.
    #[test]
    fn the_first_boundary_of_a_session_always_reports() {
        let mut p = pipeline();
        p.emit_session_summary(5_000);
        let rows = summaries(&p);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1.session_duration_s, 5);
    }

    #[test]
    fn the_size_trigger_fires_once_per_threshold() {
        // A summary estimates at 308 bytes; three relayed events at 88.
        let mut p = Pipeline::new(64, 400, false, 0);
        p.emit_session_summary(0);
        let mut fired = 0;
        for _ in 0..3 {
            if p.push(WireEvent {
                ts_ms: 0,
                data: WireEventData::MessageRelayed {
                    hop_count: 1,
                    remaining_ttl: 2,
                },
            }) {
                fired += 1;
            }
        }
        assert_eq!(fired, 1);
    }
}
