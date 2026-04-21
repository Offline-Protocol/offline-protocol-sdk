//! Panic-isolated dispatch into [`TelemetrySink::emit`].
//!
//! Every emit site in the SDK reaches the foreign sink through this helper so
//! a panicking sink cannot unwind across an SDK boundary. The trait docstring
//! still asks implementors not to panic, but `catch_unwind` here makes that a
//! soft contract rather than a load-bearing one — a panic is logged at
//! `error!`, the offending record is dropped, and the SDK continues.
//!
//! The crucial property this preserves is mutex-poisoning safety: several
//! emit sites invoke the sink while holding `SharedState`'s mutex. Without
//! this isolation, a panicking sink would unwind through the live
//! `MutexGuard`, poisoning the mutex on drop and silently degrading every
//! subsequent SDK operation that needs the lock to `Error::Other("Shared
//! state mutex poisoned")`.
//!
//! `AssertUnwindSafe` is required because `&Arc<dyn TelemetrySink>` and
//! `&TelemetryRecord` carry no `UnwindSafe` bound. The assertion is sound:
//! both arguments are immutable borrows; the only state that could be left
//! inconsistent on panic lives inside the user-supplied sink, and that is
//! exactly the thing the user is responsible for.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use super::record::TelemetryRecord;
use super::sink::TelemetrySink;

/// Dispatches `record` to `sink`, isolating any panic the sink raises.
///
/// On panic, logs at `error!` with the record name and (when extractable) the
/// panic message, then returns. Returns normally on every input — there is no
/// `Result` because there is nothing meaningful for a caller to do with a
/// dispatch failure beyond what this helper already does.
pub(crate) fn dispatch_record(sink: &Arc<dyn TelemetrySink>, record: &TelemetryRecord) {
    let result = catch_unwind(AssertUnwindSafe(|| sink.emit(record)));
    if let Err(payload) = result {
        let message = panic_message(&*payload);
        tracing::error!(
            telemetry_record = record.name(),
            panic = %message,
            "TelemetrySink panicked; record dropped. Sinks must not panic — see the TelemetrySink docstring.",
        );
    }
}

/// Best-effort extraction of a human-readable message from a panic payload.
///
/// Standard library `panic!()` payloads are either `&'static str` or `String`;
/// `panic_any` can carry arbitrary `Send + 'static` types. Falls back to a
/// generic placeholder when the payload type is not one we can render.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Recording sink that captures the records it receives.
    #[derive(Default)]
    struct RecordingSink {
        seen: Mutex<Vec<&'static str>>,
    }

    impl TelemetrySink for RecordingSink {
        fn emit(&self, record: &TelemetryRecord) {
            self.seen.lock().unwrap().push(record.name());
        }
    }

    fn sample_record() -> TelemetryRecord {
        TelemetryRecord::Protocol(Box::new(Event::neighbor_lost("alice".into())))
    }

    #[test]
    fn dispatch_record_calls_sink_with_record() {
        let concrete = Arc::new(RecordingSink::default());
        let dyn_sink: Arc<dyn TelemetrySink> = concrete.clone();
        dispatch_record(&dyn_sink, &sample_record());
        let names = concrete.seen.lock().unwrap().clone();
        assert_eq!(names, vec!["protocol.neighbor.lost"]);
    }

    /// Sink whose `emit` always panics with a `&'static str` payload — the
    /// most common shape produced by `panic!("literal")`.
    struct StringPanicSink;
    impl TelemetrySink for StringPanicSink {
        fn emit(&self, _record: &TelemetryRecord) {
            panic!("simulated sink panic");
        }
    }

    #[test]
    fn dispatch_record_swallows_static_str_panic() {
        let sink: Arc<dyn TelemetrySink> = Arc::new(StringPanicSink);
        // No catch_unwind here — if the helper failed to isolate, this test
        // would fail with the panic propagating out.
        dispatch_record(&sink, &sample_record());
    }

    /// Sink whose `emit` panics with a `String` payload (as produced by
    /// `panic!("{}", x)`).
    struct FormattedPanicSink;
    impl TelemetrySink for FormattedPanicSink {
        fn emit(&self, _record: &TelemetryRecord) {
            let cause = "boom";
            panic!("formatted: {}", cause);
        }
    }

    #[test]
    fn dispatch_record_swallows_formatted_string_panic() {
        let sink: Arc<dyn TelemetrySink> = Arc::new(FormattedPanicSink);
        dispatch_record(&sink, &sample_record());
    }

    /// Sink whose `emit` panics with a non-string payload via `panic_any`.
    struct NonStringPanicSink;
    impl TelemetrySink for NonStringPanicSink {
        fn emit(&self, _record: &TelemetryRecord) {
            std::panic::panic_any(123_i32);
        }
    }

    #[test]
    fn dispatch_record_swallows_panic_any_payload() {
        let sink: Arc<dyn TelemetrySink> = Arc::new(NonStringPanicSink);
        dispatch_record(&sink, &sample_record());
    }

    /// After a panicking emit, the SDK must keep dispatching — a sink that
    /// panics on the first call but succeeds on the second should still see
    /// the second record.
    #[test]
    fn dispatch_record_continues_dispatching_after_panic() {
        struct PanicOnceThenRecord {
            count: AtomicUsize,
            seen: Mutex<Vec<&'static str>>,
        }
        impl TelemetrySink for PanicOnceThenRecord {
            fn emit(&self, record: &TelemetryRecord) {
                if self.count.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("first call panics");
                }
                self.seen.lock().unwrap().push(record.name());
            }
        }

        let concrete = Arc::new(PanicOnceThenRecord {
            count: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let dyn_sink: Arc<dyn TelemetrySink> = concrete.clone();
        dispatch_record(&dyn_sink, &sample_record());
        dispatch_record(&dyn_sink, &sample_record());
        let names = concrete.seen.lock().unwrap().clone();
        assert_eq!(
            names,
            vec!["protocol.neighbor.lost"],
            "second dispatch must fire even after the first panicked",
        );
    }

    // Compile-only check that the helper's signature accepts the exact
    // shape the protocol engine holds — `&Arc<dyn TelemetrySink>` and
    // `&TelemetryRecord`.
    #[allow(dead_code)]
    fn signature_matches_call_sites(sink: &Arc<dyn TelemetrySink>, record: &TelemetryRecord) {
        dispatch_record(sink, record);
    }
}
