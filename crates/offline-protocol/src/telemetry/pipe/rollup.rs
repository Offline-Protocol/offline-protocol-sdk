//! The 60 s tumbling-window aggregator.
//!
//! The engine emits a metrics frame every few seconds. Forwarding each one
//! would be wasteful, so they are folded into one `mesh_metrics_rollup` per
//! wall-clock minute: mean, maximum and median of the gauge fields, a
//! transport dwell accumulator, and the per-window message counts.
//!
//! Dwell is attributed forward-looking on real deltas: the interval between
//! two frames was held on the transport the earlier frame observed, so the
//! elapsed gap is credited there; the last frame's tail to the bucket
//! boundary is credited when the window closes naturally, and never on a
//! partial flush, where nothing shows the transport persisted.
//!
//! A window with fewer than two frames and no sends is discarded: a
//! single-frame window carries single-sample gauges and no transition
//! evidence, and an idle, sparse-frame bucket would otherwise emit a
//! degenerate full-minute rollup.

use crate::telemetry::pipe::classify::{FrameSample, MessageKind};
use crate::telemetry::pipe::session_pairer::rounded_median;
use crate::telemetry::pipe::wire::{DwellKey, RollupData, WireEvent, WireEventData};

/// The window length.
pub(crate) const WINDOW_MS: i64 = 60_000;

#[derive(Debug)]
struct OpenWindow {
    bucket_start_ms: i64,
    last_frame_ts: i64,
    frame_count: u32,
    neighbor_counts: Vec<i64>,
    ack_pendings: Vec<i64>,
    retry_totals: Vec<i64>,
    retry_criticals: Vec<i64>,
    transport_ms: [i64; 4],
    relay_ms: i64,
    last_transport: DwellKey,
    last_is_relay: bool,
    sends_attempted: i64,
    sends_succeeded: i64,
    sends_failed: i64,
}

#[derive(Debug, Default)]
pub(crate) struct RollupAggregator {
    open: Option<OpenWindow>,
    /// Sends that arrived before any frame anchored a window. Folded into the
    /// first window seeded; dropped at a session boundary so they cannot leak
    /// into the next session.
    pending_attempted: i64,
    pending_succeeded: i64,
    pending_failed: i64,
}

impl RollupAggregator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Folds one frame. Returns the rollup of the window it closed, if any.
    pub(crate) fn add(&mut self, sample: FrameSample) -> Option<WireEvent> {
        let ts = sample.ts_ms;
        let bucket_start_ms = ts.div_euclid(WINDOW_MS) * WINDOW_MS;
        let mut closed = None;

        if let Some(open) = &self.open {
            if bucket_start_ms < open.bucket_start_ms {
                // Out of order for an already-passed bucket: folding it into
                // the open window would attribute its gauges to the wrong
                // bucket, so it is dropped.
                return None;
            }
            if bucket_start_ms > open.bucket_start_ms {
                let window = self.open.take().expect("checked above");
                closed = Self::close_window(window, true);
            }
        }

        if self.open.is_none() {
            self.open = Some(self.seed(bucket_start_ms, ts));
        }
        let w = self.open.as_mut().expect("seeded above");

        let transport = sample
            .current_transport
            .map_or(DwellKey::None, |t| t.dwell());
        if w.frame_count > 0 {
            let dt = (ts - w.last_frame_ts).max(0);
            w.transport_ms[w.last_transport.index()] += dt;
            if w.last_is_relay {
                w.relay_ms += dt;
            }
        }
        // Never backwards. A wall-clock correction inside the open bucket
        // arrives as a frame earlier than the last one; `dt` clamps to zero
        // for that frame, but moving the mark back would re-credit the
        // skipped span to the next frame and let a window's dwell exceed the
        // window itself. An earlier *bucket* is dropped above; this is the
        // same step landing inside the current one.
        w.last_frame_ts = w.last_frame_ts.max(ts);
        w.last_transport = transport;
        w.last_is_relay = sample.is_local_relay;
        w.frame_count += 1;

        w.neighbor_counts.push(sample.neighbor_count as i64);
        w.ack_pendings.push(sample.ack_pending as i64);
        w.retry_totals.push(sample.retry_total as i64);
        w.retry_criticals.push(sample.retry_critical as i64);
        closed
    }

    /// Counts one message event into the open window, or holds it until a
    /// window opens.
    pub(crate) fn count_message(&mut self, kind: MessageKind) {
        let (attempted, succeeded, failed) = match &mut self.open {
            Some(w) => (
                &mut w.sends_attempted,
                &mut w.sends_succeeded,
                &mut w.sends_failed,
            ),
            None => (
                &mut self.pending_attempted,
                &mut self.pending_succeeded,
                &mut self.pending_failed,
            ),
        };
        match kind {
            MessageKind::Sent => *attempted += 1,
            MessageKind::Delivered => *succeeded += 1,
            MessageKind::Failed => *failed += 1,
        }
    }

    /// Emits the open window at a session boundary, if it is thick enough,
    /// and drops any unanchored counts so they cannot leak across sessions.
    pub(crate) fn flush(&mut self) -> Option<WireEvent> {
        let Some(window) = self.open.take() else {
            self.pending_attempted = 0;
            self.pending_succeeded = 0;
            self.pending_failed = 0;
            return None;
        };
        Self::close_window(window, false)
    }

    fn seed(&mut self, bucket_start_ms: i64, ts: i64) -> OpenWindow {
        let w = OpenWindow {
            bucket_start_ms,
            last_frame_ts: ts,
            frame_count: 0,
            neighbor_counts: Vec::new(),
            ack_pendings: Vec::new(),
            retry_totals: Vec::new(),
            retry_criticals: Vec::new(),
            transport_ms: [0; 4],
            relay_ms: 0,
            last_transport: DwellKey::None,
            last_is_relay: false,
            sends_attempted: self.pending_attempted,
            sends_succeeded: self.pending_succeeded,
            sends_failed: self.pending_failed,
        };
        self.pending_attempted = 0;
        self.pending_succeeded = 0;
        self.pending_failed = 0;
        w
    }

    fn close_window(mut w: OpenWindow, natural: bool) -> Option<WireEvent> {
        if natural {
            let bucket_end_ms = w.bucket_start_ms + WINDOW_MS;
            let tail = (bucket_end_ms - w.last_frame_ts).max(0);
            w.transport_ms[w.last_transport.index()] += tail;
            if w.last_is_relay {
                w.relay_ms += tail;
            }
        }
        let sends = w.sends_attempted + w.sends_succeeded + w.sends_failed;
        if w.frame_count < 2 && sends == 0 {
            return None;
        }
        let duration_s = if natural {
            WINDOW_MS / 1000
        } else {
            ((w.last_frame_ts - w.bucket_start_ms) as f64 / 1000.0).round() as i64
        };
        Some(Self::emit_window(w, duration_s))
    }

    fn emit_window(w: OpenWindow, bucket_duration_s: i64) -> WireEvent {
        let mut sorted = w.neighbor_counts.clone();
        sorted.sort_unstable();
        let data = RollupData {
            bucket_start_ms: w.bucket_start_ms,
            bucket_duration_s,
            neighbor_count_avg: mean(&w.neighbor_counts),
            neighbor_count_max: max(&w.neighbor_counts),
            neighbor_count_p50: rounded_median(&sorted),
            ack_pending_avg: mean(&w.ack_pendings),
            ack_pending_max: max(&w.ack_pendings),
            retry_queue_total_avg: mean(&w.retry_totals),
            retry_queue_total_max: max(&w.retry_totals),
            retry_queue_critical_count_max: max(&w.retry_criticals),
            transport_time_ms: w.transport_ms,
            is_local_relay_duration_s: (w.relay_ms as f64 / 1000.0).round() as i64,
            current_transport_at_bucket_end: w.last_transport,
            sends_attempted_sum: w.sends_attempted,
            sends_succeeded_sum: w.sends_succeeded,
            sends_failed_sum: w.sends_failed,
        };
        WireEvent {
            ts_ms: w.bucket_start_ms,
            data: WireEventData::MetricsRollup(Box::new(data)),
        }
    }
}

fn mean(samples: &[i64]) -> f64 {
    if samples.is_empty() {
        0.0
    } else {
        samples.iter().sum::<i64>() as f64 / samples.len() as f64
    }
}

fn max(samples: &[i64]) -> i64 {
    samples.iter().copied().max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::pipe::wire::TransportKey;

    fn frame(ts: i64, neighbors: u64, transport: Option<TransportKey>, relay: bool) -> FrameSample {
        FrameSample {
            ts_ms: ts,
            neighbor_count: neighbors,
            ack_pending: 1,
            retry_total: 2,
            retry_critical: 0,
            current_transport: transport,
            is_local_relay: relay,
        }
    }

    fn rollup(event: WireEvent) -> RollupData {
        match event.data {
            WireEventData::MetricsRollup(r) => *r,
            other => panic!("expected a rollup, got {other:?}"),
        }
    }

    #[test]
    fn a_later_bucket_closes_the_window_with_the_tail_credited() {
        let mut agg = RollupAggregator::new();
        let base = 1_747_000_020_000; // minute-aligned
        assert!(agg
            .add(frame(base, 4, Some(TransportKey::Ble), false))
            .is_none());
        assert!(agg
            .add(frame(base + 5_000, 6, Some(TransportKey::WifiDirect), true))
            .is_none());
        let closed = agg
            .add(frame(base + WINDOW_MS, 1, Some(TransportKey::Ble), false))
            .expect("window closed");
        let r = rollup(closed);
        assert_eq!(r.bucket_start_ms, base);
        assert_eq!(r.bucket_duration_s, 60);
        // 5 s on BLE (first interval), then the 55 s tail on Wi-Fi Direct.
        assert_eq!(r.transport_time_ms, [5_000, 55_000, 0, 0]);
        assert_eq!(r.is_local_relay_duration_s, 55);
        assert_eq!(r.current_transport_at_bucket_end, DwellKey::WifiDirect);
        assert_eq!(r.neighbor_count_avg, 5.0);
        assert_eq!(r.neighbor_count_p50, 5);
        assert_eq!(r.neighbor_count_max, 6);
    }

    #[test]
    fn a_thin_window_without_sends_is_discarded_and_with_sends_is_kept() {
        let mut agg = RollupAggregator::new();
        agg.add(frame(0, 1, None, false));
        assert!(agg.flush().is_none());
        agg.count_message(MessageKind::Sent);
        agg.add(frame(1_000, 1, None, false));
        let r = rollup(agg.flush().expect("sends make it worth reporting"));
        assert_eq!(r.sends_attempted_sum, 1);
        assert_eq!(r.bucket_duration_s, 1);
    }

    #[test]
    fn an_out_of_order_frame_is_dropped() {
        let mut agg = RollupAggregator::new();
        agg.add(frame(WINDOW_MS, 1, None, false));
        agg.add(frame(0, 99, None, false));
        agg.add(frame(WINDOW_MS + 1_000, 1, None, false));
        let r = rollup(agg.flush().expect("two frames"));
        assert_eq!(r.neighbor_count_max, 1);
    }

    #[test]
    fn a_flush_with_no_window_drops_held_counts() {
        let mut agg = RollupAggregator::new();
        agg.count_message(MessageKind::Delivered);
        assert!(agg.flush().is_none());
        agg.add(frame(0, 1, None, false));
        agg.add(frame(1_000, 1, None, false));
        let r = rollup(agg.flush().expect("two frames"));
        assert_eq!(r.sends_succeeded_sum, 0);
    }

    #[test]
    fn reticulum_and_nostr_dwell_land_in_none() {
        let mut agg = RollupAggregator::new();
        agg.add(frame(0, 1, Some(TransportKey::Nostr), false));
        agg.add(frame(10_000, 1, Some(TransportKey::Reticulum), false));
        let r = rollup(agg.flush().expect("two frames"));
        assert_eq!(r.transport_time_ms[DwellKey::None.index()], 10_000);
    }
}
