//! The per-reason ceiling on forwarded `protocol.message.failed` rows.
//!
//! A failure reaches the wire twice: its count goes into the minute rollup's
//! `sends_failed_sum`, and the row itself carries its `reason` and
//! `retry_count`. The count needs no ceiling. The row does, because no row can
//! carry a message id (no identifier has a field to land in), so the ingest
//! cannot tell one failure reported many times from many failures, and it
//! meters every row. A device reporting the same failure in a loop is billed
//! for the loop. That happened: an outbox eviction that left its message
//! queued for retry reported a terminal failure on every retry, about fifteen
//! a second per device, for days, and every one was an accepted event.
//!
//! The ceiling is a token bucket per reason. A bucket starts full at
//! [`BURST`], so an outbox failing in one sweep still reports every row, and
//! earns one row back every [`REFILL_MS`]. A row it refuses is still counted
//! in the rollup, and is added to the pipe's `dropped`, so the stats keep
//! reconciling: sent plus dropped is everything the pipe was given.
//!
//! Reasons are the engine's own fixed literals, about a dozen of them, so the
//! table is small and scanned rather than hashed, and a reason already seen
//! costs no allocation. A table that somehow fills puts every further reason
//! in one shared bucket rather than growing.

/// Rows one reason may send at once: a full outbox expiring, or evicted, in a
/// single sweep.
pub(crate) const BURST: u32 = crate::constants::MAX_OUTBOX_ENTRIES as u32;

/// Once a reason's burst is spent, it earns one row back per this many
/// milliseconds.
pub(crate) const REFILL_MS: i64 = 10_000;

/// Distinct reasons given a bucket of their own.
const MAX_REASONS: usize = 32;

#[derive(Debug)]
struct Bucket {
    tokens: u32,
    /// Where earning is measured from: the last row credited, or the moment
    /// the bucket was last seen full, since a full bucket earns nothing.
    earned_from_ms: i64,
}

impl Bucket {
    fn full(now_ms: i64) -> Self {
        Self {
            tokens: BURST,
            earned_from_ms: now_ms,
        }
    }

    fn take(&mut self, now_ms: i64) -> bool {
        let elapsed = now_ms.saturating_sub(self.earned_from_ms);
        if elapsed < 0 {
            // The clock went back. Re-anchor rather than wait out the gap.
            self.earned_from_ms = now_ms;
        } else if elapsed >= REFILL_MS {
            let earned = elapsed / REFILL_MS;
            self.tokens = u32::try_from(earned)
                .map_or(BURST, |earned| self.tokens.saturating_add(earned))
                .min(BURST);
            self.earned_from_ms = self.earned_from_ms.saturating_add(earned * REFILL_MS);
        }
        if self.tokens == BURST {
            self.earned_from_ms = now_ms;
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

#[derive(Debug, Default)]
pub(crate) struct FailureRowLimit {
    buckets: Vec<(Box<str>, Bucket)>,
    overflow: Option<Bucket>,
}

impl FailureRowLimit {
    /// Whether a `protocol.message.failed` row with this reason may be
    /// forwarded now. Spends the row when it may.
    pub(crate) fn allow(&mut self, reason: &str, now_ms: i64) -> bool {
        if let Some((_, bucket)) = self
            .buckets
            .iter_mut()
            .find(|(known, _)| &**known == reason)
        {
            return bucket.take(now_ms);
        }
        if self.buckets.len() < MAX_REASONS {
            let mut bucket = Bucket::full(now_ms);
            let allowed = bucket.take(now_ms);
            self.buckets.push((reason.into(), bucket));
            return allowed;
        }
        self.overflow
            .get_or_insert_with(|| Bucket::full(now_ms))
            .take(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVICTED: &str = "Outbox capacity exceeded";

    fn spend_burst(limit: &mut FailureRowLimit, reason: &str, now_ms: i64) {
        for _ in 0..BURST {
            assert!(limit.allow(reason, now_ms));
        }
    }

    #[test]
    fn a_whole_outbox_failing_at_once_is_reported_in_full() {
        let mut limit = FailureRowLimit::default();
        spend_burst(&mut limit, EVICTED, 0);
        assert!(!limit.allow(EVICTED, 0), "the row past the burst is held");
    }

    #[test]
    fn a_repeating_failure_is_held_to_one_row_per_refill() {
        let mut limit = FailureRowLimit::default();
        spend_burst(&mut limit, EVICTED, 0);
        assert!(!limit.allow(EVICTED, REFILL_MS - 1));
        assert!(limit.allow(EVICTED, REFILL_MS));
        assert!(!limit.allow(EVICTED, REFILL_MS));
        // A loop at twenty a second for a day sends one row per refill.
        const DAY_MS: i64 = 86_400_000;
        let mut sent = 0u32;
        let mut now = REFILL_MS;
        while now < REFILL_MS + DAY_MS {
            now += 50;
            sent += u32::from(limit.allow(EVICTED, now));
        }
        assert_eq!(i64::from(sent), DAY_MS / REFILL_MS);
    }

    #[test]
    fn each_reason_has_its_own_bucket() {
        let mut limit = FailureRowLimit::default();
        spend_burst(&mut limit, EVICTED, 0);
        assert!(!limit.allow(EVICTED, 0));
        assert!(
            limit.allow("Max retries exceeded", 0),
            "one reason looping must not silence the others"
        );
    }

    #[test]
    fn a_quiet_reason_earns_back_its_burst_and_no_more() {
        let mut limit = FailureRowLimit::default();
        spend_burst(&mut limit, EVICTED, 0);
        let later = 1_000 * REFILL_MS;
        spend_burst(&mut limit, EVICTED, later);
        assert!(!limit.allow(EVICTED, later));
    }

    #[test]
    fn a_full_bucket_banks_no_progress_while_it_waits() {
        let mut limit = FailureRowLimit::default();
        assert!(limit.allow(EVICTED, 0));
        // Back to full half-way through a refill period, then drained. The
        // next row is a whole period after the drain, not the end of the
        // period that began while the bucket was already full.
        spend_burst(&mut limit, EVICTED, 5 * REFILL_MS + REFILL_MS / 2);
        assert!(!limit.allow(EVICTED, 6 * REFILL_MS));
        assert!(limit.allow(EVICTED, 6 * REFILL_MS + REFILL_MS / 2));
    }

    #[test]
    fn a_clock_that_goes_back_does_not_stall_the_refill() {
        let mut limit = FailureRowLimit::default();
        let start = 1_000_000_000;
        spend_burst(&mut limit, EVICTED, start);
        assert!(!limit.allow(EVICTED, 0), "the clock went back");
        assert!(
            limit.allow(EVICTED, REFILL_MS),
            "earning resumes from the new reading, not the old one"
        );
    }

    #[test]
    fn reasons_past_the_table_share_one_bucket() {
        let mut limit = FailureRowLimit::default();
        for n in 0..MAX_REASONS {
            assert!(limit.allow(&format!("reason {n}"), 0));
        }
        spend_burst(&mut limit, "one more", 0);
        assert!(!limit.allow("and another", 0));
        assert_eq!(limit.buckets.len(), MAX_REASONS, "the table did not grow");
    }
}
