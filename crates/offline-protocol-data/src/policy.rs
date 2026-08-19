//! Size caps and the compaction trigger.
//!
//! Pure arithmetic over byte counts, deliberately free of engine and storage
//! types so the thresholds can be tested and reasoned about on their own.
//! Every constant here is derived from measurements recorded in the data
//! layer design record, not chosen by feel.

/// Hard cap on the compacted encoding of one document, 1 MiB.
///
/// The physical ceiling is the sealed protocol-state record (4 MiB). The
/// working budget is a quarter of it, and the gap is not slack: it absorbs
/// the delta log accumulating between compactions, seal and framing
/// overhead, and the growth between the write that breaches the cap and the
/// next compaction pass. A document that only checked itself against the
/// physical ceiling would become unpersistable while the user was still
/// typing into it.
pub const MAX_DOC_BYTES: usize = 1024 * 1024;

/// Size at which a document starts warning, 768 KiB (75% of the cap).
///
/// The warning exists so an application hears about growth while it can
/// still act (archive, split, prune). A cap with no approach signal turns
/// into a cliff the app meets for the first time in production.
pub const DOC_SIZE_WARN_BYTES: usize = 768 * 1024;

/// Floor under the compaction trigger, 64 KiB.
///
/// Without it a small document compacts on nearly every commit: measured
/// history growth is ~6.7 bytes per operation while a shallow snapshot of a
/// small document is under a kilobyte, so the ratio rule alone would fire
/// constantly and rewrite a record per keystroke.
pub const COMPACT_MIN_DELTA_LOG_BYTES: usize = 64 * 1024;

/// Compact once the delta log is this many times the compacted document.
///
/// Bounds worst-case storage at roughly five times the document's state
/// size, which is the trade being made: replay work and disk against how
/// often a full snapshot is rewritten.
pub const COMPACT_DELTA_LOG_RATIO: usize = 4;

/// Compact after this many commits regardless of byte ratios, 1024.
///
/// Caps replay work after a crash. The ratio rule alone can idle for a very
/// long time on a document whose commits are tiny.
pub const COMPACT_MAX_COMMITS: u32 = 1024;

/// Whether a document should be rewritten as a fresh compacted snapshot.
///
/// `compacted_doc_bytes` is the size of the last snapshot written for the
/// document (zero if it has never been compacted).
pub fn should_compact(
    delta_log_bytes: usize,
    compacted_doc_bytes: usize,
    commits_since_compaction: u32,
) -> bool {
    if commits_since_compaction >= COMPACT_MAX_COMMITS {
        return true;
    }
    let ratio_threshold = compacted_doc_bytes.saturating_mul(COMPACT_DELTA_LOG_RATIO);
    delta_log_bytes > ratio_threshold.max(COMPACT_MIN_DELTA_LOG_BYTES)
}

/// Where a compacted document sits against the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeVerdict {
    /// Comfortably under the warning threshold.
    Ok,
    /// At or past [`DOC_SIZE_WARN_BYTES`], still under the cap.
    Warn,
    /// Past [`MAX_DOC_BYTES`]. The commit that produced this must fail.
    TooLarge,
}

/// Classify a compacted document size.
pub fn size_verdict(compacted_bytes: usize) -> SizeVerdict {
    if compacted_bytes > MAX_DOC_BYTES {
        SizeVerdict::TooLarge
    } else if compacted_bytes >= DOC_SIZE_WARN_BYTES {
        SizeVerdict::Warn
    } else {
        SizeVerdict::Ok
    }
}

/// Whether the document is close enough to the warning threshold that its
/// true compacted size has to be measured before the next commit is accepted.
///
/// Measuring means encoding a shallow snapshot, which is far too expensive to
/// do on every commit. Estimating from the last real measurement plus the
/// bytes written since it is always an over-estimate of the compacted size
/// (history compacts, it does not expand), so a document can never cross the
/// cap without this answering true first. That is the property that keeps the
/// check from lagging the truth by more than a single flush.
pub fn needs_size_check(last_measured_bytes: usize, delta_bytes_since: usize) -> bool {
    last_measured_bytes.saturating_add(delta_bytes_since) >= DOC_SIZE_WARN_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_documents_do_not_compact_on_every_commit() {
        // A 900 byte document with 8 KiB of log: the ratio rule would fire
        // (8192 > 3600) but the floor holds it back.
        assert!(!should_compact(8 * 1024, 900, 3));
    }

    #[test]
    fn the_floor_is_not_a_ceiling_for_large_documents() {
        // 100 KiB of log against a 200 KiB document stays under 4x.
        assert!(!should_compact(100 * 1024, 200 * 1024, 10));
        // 900 KiB of log against the same document is past it.
        assert!(should_compact(900 * 1024, 200 * 1024, 10));
    }

    #[test]
    fn a_never_compacted_document_uses_the_floor() {
        assert!(!should_compact(COMPACT_MIN_DELTA_LOG_BYTES, 0, 1));
        assert!(should_compact(COMPACT_MIN_DELTA_LOG_BYTES + 1, 0, 1));
    }

    #[test]
    fn commit_count_forces_compaction_regardless_of_size() {
        assert!(should_compact(1, 10 * 1024 * 1024, COMPACT_MAX_COMMITS));
    }

    #[test]
    fn size_verdict_boundaries() {
        assert_eq!(size_verdict(0), SizeVerdict::Ok);
        assert_eq!(size_verdict(DOC_SIZE_WARN_BYTES - 1), SizeVerdict::Ok);
        assert_eq!(size_verdict(DOC_SIZE_WARN_BYTES), SizeVerdict::Warn);
        assert_eq!(size_verdict(MAX_DOC_BYTES), SizeVerdict::Warn);
        assert_eq!(size_verdict(MAX_DOC_BYTES + 1), SizeVerdict::TooLarge);
    }

    #[test]
    fn the_cap_leaves_room_under_the_record_ceiling() {
        // The sealed protocol-state record is 4 MiB. The working cap must
        // stay well under it so the delta log, seal and framing all fit.
        assert!(MAX_DOC_BYTES * 4 <= 4 * 1024 * 1024);
        assert!(DOC_SIZE_WARN_BYTES < MAX_DOC_BYTES);
    }

    #[test]
    fn size_checks_engage_before_the_cap_can_be_crossed() {
        // Just under the warning line with nothing written since: no check.
        assert!(!needs_size_check(DOC_SIZE_WARN_BYTES - 1, 0));
        // The same document after a single byte of delta: check.
        assert!(needs_size_check(DOC_SIZE_WARN_BYTES - 1, 1));
        // A small document with a large uncompacted log: check.
        assert!(needs_size_check(1024, DOC_SIZE_WARN_BYTES));
    }
}
