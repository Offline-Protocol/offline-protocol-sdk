//! Cross-cutting telemetry sink trait.
//!
//! [`TelemetrySink`] is the single observer interface for all telemetry the
//! SDK emits. Apps implement it (or install [`NoopTelemetrySink`]) and the SDK
//! pushes every [`TelemetryRecord`] through it.
//!
//! Emit wiring lives in follow-up work; this module ships the types only.

use super::record::TelemetryRecord;

/// Sink for structured telemetry records.
///
/// Implementations must be cheap when called from hot paths — the SDK does not
/// buffer or rate-limit before dispatch beyond its own sampling layer, so
/// expensive work (I/O, serialization to external formats) should be
/// deferred to a background task owned by the implementor.
///
/// # Reentrancy and locking contract
///
/// The SDK may invoke [`TelemetrySink::emit`] while holding internal locks
/// (most notably the shared-state mutex on the `Protocol` fan-out path).
/// Implementations MUST NOT:
///
/// - block on slow I/O (network, disk, logging sinks that fsync), or
/// - reenter SDK methods on the same `OfflineProtocol` instance.
///
/// A blocking `emit` stalls every protocol operation that needs the lock.
/// Sinks that need to do real work should push the record onto a bounded
/// channel and return immediately, leaving the heavy lifting to a
/// background task owned by the implementor.
///
/// Implementations *should* also not panic, but the SDK defends against
/// this: every internal dispatch to `emit` runs under `catch_unwind`, so a
/// panicking sink is logged at `error!`, the offending record is dropped,
/// and the protocol continues. A panicking sink therefore loses telemetry
/// and degrades observability but does not corrupt SDK state. Panic
/// isolation only applies under `panic = unwind` — release profiles that
/// set `panic = abort` (including the `minisize` profile used for mobile)
/// abort the process on any panic, including one from a sink.
///
/// # Record passing
///
/// The record is passed by reference so implementations that only read fields
/// do not incur a clone. Implementations that need ownership (for example to
/// forward the record to another thread) should call [`Clone::clone`]
/// explicitly.
pub trait TelemetrySink: Send + Sync {
    /// Dispatches a single telemetry record to the sink.
    fn emit(&self, record: &TelemetryRecord);
}

/// Default sink that discards every record.
///
/// Used by the SDK when no telemetry sink has been installed. Proves the
/// zero-cost baseline: the compiler inlines the empty body at every call site
/// that can statically resolve the concrete type, and the indirect dispatch
/// cost through `Arc<dyn TelemetrySink>` is a single virtual call that returns
/// immediately.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopTelemetrySink;

impl TelemetrySink for NoopTelemetrySink {
    fn emit(&self, _record: &TelemetryRecord) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mls_observability::{MlsLifecycleEvent, MlsOperationContext};
    use std::sync::Arc;

    #[test]
    fn noop_sink_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoopTelemetrySink>();
    }

    #[test]
    fn trait_is_object_safe() {
        let _: Arc<dyn TelemetrySink> = Arc::new(NoopTelemetrySink);
    }

    #[test]
    fn noop_sink_accepts_records_without_panic() {
        let sink = NoopTelemetrySink;
        let record = TelemetryRecord::Mls(MlsLifecycleEvent::Initialized {
            timestamp_ms: 0,
            session_id: "s".into(),
            group_id: None,
            peer_id: None,
            context: MlsOperationContext::Initialize,
            error_category: None,
        });
        sink.emit(&record);
    }
}
