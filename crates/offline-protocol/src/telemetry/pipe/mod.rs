//! The telemetry pipe: the client half of the hosted telemetry product,
//! living in the core so every binding ships it from one implementation.
//!
//! # Shape
//!
//! ```text
//!   emit path (any engine thread, under the engine's locks)
//!     TelemetrySink::emit ─► classify ─► rollup / pairer / ring buffer
//!                                              │  one mutex, no I/O
//!   uploader thread (the one background thread in the workspace)
//!     drain ring ─► cut batches ─► durable queue ─► HTTPS POST ─► stats
//! ```
//!
//! The emit side is a bounded amount of work with no I/O: a gate on an
//! atomic, a match on the record, and a push into a ring buffer under the
//! pipe's own mutex. The uploader takes that mutex only to take the ring, so
//! the one lock an engine thread can be holding is never held across storage
//! or network I/O.
//!
//! The store and uploader mutexes *are* held across a drain, and deliberately:
//! they guard the worker's own state, the alternative is a queue that can be
//! reordered mid-send, and no engine thread reaches them. That last clause is
//! load-bearing rather than incidental, so every handle method an engine
//! thread can call reads an atomic instead ([`TelemetryPipe::stats`] takes
//! only the pipeline mutex, [`TelemetryPipe::is_durable`] takes none). A
//! method added here that locks the store would put a caller behind a
//! fifteen-second request.
//!
//! Nothing here holds a reference to the engine, so a stopped pipe whose
//! final request is still in flight can be dropped without keeping the
//! engine alive. Such a pipe does keep its durable queue until its worker
//! exits, and a pipe started meanwhile over the same storage holds its
//! batches in memory until then: one writer per queue, for the reason
//! `store.rs` gives.
//!
//! # What leaves the device
//!
//! Only what `WireEventData` in `wire.rs` can express: typed projections with no
//! identifier field, the rollup and session summary the pipe computes, and
//! the engine's own fixed `reason` tokens. `docs/telemetry.md` is the
//! adopter-facing inventory; the golden fixture under `tests/` is the pin.
//!
//! # Session boundaries
//!
//! A session id is minted when telemetry is enabled and rotated on the first
//! foreground after a background; the background edge emits the session
//! summary and asks for a flush. Every buffered event is stamped with the
//! session current at push time, and batches are cut on that stamp before
//! they are cut on size, so a boundary event always carries the session it
//! reports.

pub(crate) mod classify;
pub mod host;
pub(crate) mod pipeline;
pub(crate) mod rollup;
pub(crate) mod scheduler;
pub(crate) mod session_pairer;
pub(crate) mod store;
#[cfg(feature = "test-utils")]
pub mod testing;
pub(crate) mod uploader;
pub(crate) mod wire;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use offline_protocol_telemetry_wire::{
    Batch, BatchId, DeviceId, RawEvent, SessionId, CURRENT_WIRE_VERSION,
};
use offline_protocol_transport::{TransportStatus, TransportType};
use uuid::Uuid;

pub use host::{AppState, TelemetryHost, TelemetryOs};

use crate::events::Event;
use crate::protocol::state_crypto::StateRecordCipher;
use crate::protocol_state_storage::ProtocolStateStorage;
use crate::telemetry::config::TelemetryConfig;
use crate::telemetry::pipe::pipeline::{LifecycleAction, Pipeline};
use crate::telemetry::pipe::scheduler::{wake, Scheduler, WakeSignal};
use crate::telemetry::pipe::store::{Attach, Backend, BatchStore, PendingBatch};
use crate::telemetry::pipe::uploader::{
    battery_defers, user_agent, HttpClient, Uploader, UreqClient, MAX_BATCHES_PER_WAKE, SDK_VERSION,
};
use crate::telemetry::pipe::wire::BufferedEvent;
use crate::telemetry::sink::TelemetrySink;
use crate::telemetry::TelemetryRecord;
use crate::Error;

/// The `tracing` target every pipe log line carries. The uniffi crate raises
/// this target to `debug` while a pipe runs with `debug: true`.
pub const PIPE_LOG_TARGET: &str = "offline_protocol::telemetry::pipe";

/// How long a final flush (disable, end of session, drop) may block.
pub const FINAL_FLUSH_BUDGET: Duration = Duration::from_secs(3);

/// How soon a worker waiting for another pipe's queue asks for it again. The
/// other pipe lets go when its worker exits, after the drain it was in and
/// its final flush.
const ADOPTION_RETRY: Duration = Duration::from_millis(500);

/// Rough per-batch envelope overhead, for cutting batches on size.
const ENVELOPE_OVERHEAD_BYTES: usize = 256;

/// A snapshot of the pipe's counters, for diagnostics and for reconciling
/// an invoice against the device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelemetryStats {
    /// Events in the ring buffer, not yet cut into a batch.
    pub buffered: u64,
    /// Events in batches the ingest answered 2xx.
    pub sent_events: u64,
    /// Events the ingest reported accepted (the `accepted` count of each
    /// 202 body, summed; a replay counts nothing new).
    pub accepted_events: u64,
    /// Events lost: ring overflow, queue caps, TTL expiry, permanent 4xx,
    /// and everything queued or collected once the developer portal switched
    /// telemetry off for the application.
    ///
    /// Events, never records. This number is what reconciles an invoice
    /// against the device, so it counts in the unit the ingest meters. A
    /// queued batch that will not open or parse on load is therefore absent
    /// from it: how many events that batch held is precisely what was lost
    /// with it, and a count of records added here would be two units summed
    /// into one number. Those are reported by a warning on the pipe's log
    /// target instead.
    pub dropped: u64,
    /// The current session id.
    pub session_id: String,
    /// The most recent send failure or configuration problem.
    pub last_error: Option<String>,
    /// When a batch was last accepted, epoch milliseconds.
    pub last_flush_at_ms: Option<i64>,
}

/// Wall-clock source, injectable for tests.
pub(crate) type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_clock() -> Clock {
    Arc::new(|| chrono::Utc::now().timestamp_millis())
}

struct Envelope {
    app_version: String,
    os: TelemetryOs,
    os_major: u16,
}

/// Everything the emit side, the uploader thread and the handle share.
pub(crate) struct PipeShared {
    /// The hot-path gate, read at every emit site before anything else.
    enabled: Arc<AtomicBool>,
    debug: bool,
    /// Run cycles on the calling thread instead of on a worker. The tests and
    /// the benches; never a shipped binary, since `PipeParts::production`
    /// always asks for a thread and a spawn that fails is an error rather
    /// than a fallback.
    inline: bool,
    pipeline: Mutex<Pipeline>,
    store: Mutex<BatchStore>,
    uploader: Mutex<Uploader>,
    /// A backend handed over by the engine, adopted by the worker so the
    /// engine never waits behind an in-flight request.
    pending_attach: Mutex<Option<Backend>>,
    /// Set while that backend waits for another pipe in this process to let
    /// go of the queue there; the worker asks again on a short timer.
    adoption_blocked: AtomicBool,
    device_id: Mutex<Option<String>>,
    include_device_id: bool,
    /// Mirrors `BatchStore::is_durable`, so a caller can ask without taking
    /// the store mutex the uploader holds across a request.
    durable: AtomicBool,
    envelope: Envelope,
    max_batch_bytes: usize,
    flush_interval: Duration,
    clock: Clock,
    signal: WakeSignal,
    /// Packed battery: bit 16 set when known, bit 8 when charging, low byte
    /// the level.
    battery: AtomicU32,
    sent_events: AtomicU64,
    accepted_events: AtomicU64,
    uploader_dropped: AtomicU64,
    store_dropped: AtomicU64,
    /// `-1` until a batch has been accepted.
    last_flush_at_ms: AtomicI64,
    last_error: Mutex<Option<String>>,
    /// Set once the ingest has answered `telemetry_disabled`: the
    /// application's toggle is off in the developer portal. From then on the
    /// ring is emptied into nothing at every cycle rather than cut into
    /// batches, so nothing collected while the toggle is off is persisted or
    /// sent, and the gap the toggle promises holds at the device. Read on the
    /// cycle, never on the emit path, which keeps its one atomic load.
    /// Cleared only by a fresh pipe, which is what the next `enable_telemetry`
    /// builds.
    server_disabled: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl PipeShared {
    fn note_battery(&self, level: Option<u8>, is_charging: bool) {
        let packed = 1 << 16 | u32::from(is_charging) << 8 | u32::from(level.unwrap_or(0));
        let packed = if level.is_some() {
            packed
        } else {
            packed & !0xff | 0x8000
        };
        self.battery.store(packed, Ordering::Relaxed);
    }

    fn battery(&self) -> (Option<u8>, bool) {
        let packed = self.battery.load(Ordering::Relaxed);
        if packed & (1 << 16) == 0 {
            return (None, false);
        }
        let level = if packed & 0x8000 != 0 {
            None
        } else {
            Some((packed & 0xff) as u8)
        };
        (level, packed & (1 << 8) != 0)
    }

    fn set_last_error(&self, error: Option<String>) {
        *lock(&self.last_error) = error;
    }

    /// One drain-and-send cycle. Returns when the backoff lifts, if a
    /// backoff is in force, so the worker can wake for it.
    fn cycle(&self, reasons: u8, deadline: Option<Instant>) -> Option<i64> {
        let now = (self.clock)();
        self.adopt_pending_backend();

        let drained = lock(&self.pipeline).drain();
        if self.server_disabled.load(Ordering::Relaxed) {
            // Counted as dropped, in events, the unit the rest of `dropped`
            // uses: the toggle is off, and these are the events it says are
            // not kept.
            self.uploader_dropped
                .fetch_add(drained.len() as u64, Ordering::Relaxed);
        } else if !drained.is_empty() {
            let device_id = if self.include_device_id {
                lock(&self.device_id).clone()
            } else {
                None
            };
            let batches = build_batches(
                drained,
                &self.envelope,
                device_id,
                self.max_batch_bytes,
                now,
                self.debug,
            );
            let mut store = lock(&self.store);
            for batch in batches {
                store.push(batch);
            }
            store.flush_index();
            self.durable.store(store.is_durable(), Ordering::Relaxed);
            self.store_dropped
                .store(store.dropped_events(), Ordering::Relaxed);
        }

        let is_final = reasons & wake::FINAL != 0;
        let (level, charging) = self.battery();
        if !is_final && battery_defers(level, charging) {
            tracing::debug!(
                target: PIPE_LOG_TARGET,
                battery_level = ?level,
                "telemetry flush deferred on battery"
            );
            return lock(&self.uploader).next_retry_at_ms();
        }

        let mut store = lock(&self.store);
        let mut uploader = lock(&self.uploader);
        let report = uploader.drain(&mut store, now, MAX_BATCHES_PER_WAKE, deadline);
        self.durable.store(store.is_durable(), Ordering::Relaxed);
        self.store_dropped
            .store(store.dropped_events(), Ordering::Relaxed);
        drop(store);
        self.sent_events
            .fetch_add(report.sent_events, Ordering::Relaxed);
        self.accepted_events
            .fetch_add(report.accepted_events, Ordering::Relaxed);
        self.uploader_dropped
            .fetch_add(report.dropped_events, Ordering::Relaxed);
        if report.batches_sent > 0 {
            self.last_flush_at_ms.store(now, Ordering::Relaxed);
        }
        if report.disabled && !self.server_disabled.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                target: PIPE_LOG_TARGET,
                dropped_events = report.dropped_events,
                "hosted telemetry is switched off for this application in the developer \
                 portal; the queue was dropped and nothing more is collected until \
                 enable_telemetry"
            );
        }
        // Mirrored, not accumulated: the uploader clears its own error when a
        // batch is accepted, and a stats reader asking "is telemetry working"
        // after a recovered outage must not still be shown the outage. A
        // cycle that sent nothing leaves the uploader's value alone, so this
        // does not erase an error just because the queue was empty.
        self.set_last_error(uploader.last_error().map(str::to_string));
        uploader.next_retry_at_ms()
    }

    /// Offers the store the backend the engine handed over, if any. One that
    /// is refused is kept for the next attempt unless a newer one arrived in
    /// the meantime.
    fn adopt_pending_backend(&self) {
        // Taken and released before the store mutex, never held with it: the
        // engine's `attach_storage` takes this one, and the store mutex is
        // held across a request.
        let Some(backend) = lock(&self.pending_attach).take() else {
            return;
        };
        let mut store = lock(&self.store);
        let refused = match store.attach(backend) {
            Attach::Adopted => None,
            Attach::Busy(backend) => Some((backend, true)),
            Attach::Failed(backend) => Some((backend, false)),
        };
        store.flush_index();
        self.durable.store(store.is_durable(), Ordering::Relaxed);
        self.store_dropped
            .store(store.dropped_events(), Ordering::Relaxed);
        drop(store);
        self.adoption_blocked.store(
            refused.as_ref().is_some_and(|(_, busy)| *busy),
            Ordering::Relaxed,
        );
        if let Some((backend, _)) = refused {
            lock(&self.pending_attach).get_or_insert(backend);
        }
    }

    /// Lets go of the durable queue so another pipe over the same storage can
    /// adopt it. Called once no further cycle will run.
    fn release_queue(&self) {
        lock(&self.store).release();
        self.durable.store(false, Ordering::Relaxed);
    }
}

/// Releases the pipe's durable queue when dropped. See [`run_worker`].
struct ReleaseQueue<'a>(&'a PipeShared);

impl Drop for ReleaseQueue<'_> {
    fn drop(&mut self) {
        self.0.release_queue();
    }
}

/// The worker loop: wait for a reason or the interval, run one cycle,
/// publish the flush generation, stop after the final flush, and let go of
/// the durable queue on the way out.
fn run_worker(shared: Arc<PipeShared>) {
    // Declared first so it drops last, after the final flush, and on every
    // way out of this function including an unwinding panic. A queue left
    // claimed would keep every later pipe over the same storage in memory for
    // the rest of the process.
    let _release = ReleaseQueue(&shared);
    let mut next_interval = Instant::now() + shared.flush_interval;
    loop {
        let wake_at = if shared.adoption_blocked.load(Ordering::Relaxed) {
            next_interval.min(Instant::now() + ADOPTION_RETRY)
        } else {
            next_interval
        };
        let (reasons, generation) = shared.signal.wait(wake_at);
        if reasons == wake::INTERVAL && Instant::now() < next_interval {
            // Only the adoption retry came due. Nothing was asked for and the
            // interval has not elapsed, so no batch is cut and no socket opens.
            shared.adopt_pending_backend();
            continue;
        }
        let stopping = reasons & wake::STOP != 0;
        let deadline = stopping.then(|| Instant::now() + FINAL_FLUSH_BUDGET);
        let retry_at = shared.cycle(reasons, deadline);
        shared.signal.complete(generation);
        if stopping {
            break;
        }
        let now_ms = (shared.clock)();
        let interval = Instant::now() + shared.flush_interval;
        next_interval = match retry_at {
            Some(at) if at > now_ms => {
                let wait = Duration::from_millis((at - now_ms).min(i64::MAX / 2) as u64);
                interval.min(Instant::now() + wait)
            }
            _ => interval,
        };
    }
}

/// Cuts drained events into batches that agree on their session and fit
/// `max_batch_bytes`. Session first, size second: `session_id` lives on the
/// envelope, so one batch can only ever claim one session.
fn build_batches(
    events: VecDeque<BufferedEvent>,
    envelope: &Envelope,
    device_id: Option<String>,
    max_batch_bytes: usize,
    now_ms: i64,
    debug: bool,
) -> Vec<PendingBatch> {
    let mut batches = Vec::new();
    let mut chunk: Vec<RawEvent> = Vec::new();
    let mut chunk_session = Uuid::nil();
    let mut chunk_bytes = ENVELOPE_OVERHEAD_BYTES;
    let finish = |chunk: &mut Vec<RawEvent>, session: Uuid, batches: &mut Vec<PendingBatch>| {
        if chunk.is_empty() {
            return;
        }
        let batch = Batch {
            batch_id: BatchId(Uuid::new_v4()),
            session_id: SessionId(session),
            device_id: device_id.clone().map(DeviceId),
            wire_version: CURRENT_WIRE_VERSION,
            app_version: envelope.app_version.clone(),
            os: envelope.os.wire(),
            os_major: envelope.os_major,
            sdk_version: SDK_VERSION.to_string(),
            events: std::mem::take(chunk),
        };
        if debug {
            log_batch(&batch);
        }
        let Ok(body) = serde_json::to_string(&batch) else {
            return;
        };
        batches.push(PendingBatch {
            seq: None,
            batch_id: batch.batch_id.0,
            created_ms: now_ms,
            event_count: batch.events.len().min(u32::MAX as usize) as u32,
            body,
        });
    };
    for item in events {
        let raw = item.event.to_raw();
        let bytes = serde_json::to_string(&raw).map_or(0, |s| s.len());
        if !chunk.is_empty()
            && (item.session != chunk_session || chunk_bytes + bytes > max_batch_bytes)
        {
            finish(&mut chunk, chunk_session, &mut batches);
            chunk_bytes = ENVELOPE_OVERHEAD_BYTES;
        }
        if chunk.is_empty() {
            chunk_session = item.session;
        }
        chunk.push(raw);
        chunk_bytes += bytes;
    }
    finish(&mut chunk, chunk_session, &mut batches);
    batches
}

/// The debug tap: one `debug` line per wire event, under [`PIPE_LOG_TARGET`].
pub(crate) fn log_batch(batch: &Batch) {
    for event in &batch.events {
        tracing::debug!(
            target: PIPE_LOG_TARGET,
            batch_id = %batch.batch_id.0,
            session_id = %batch.session_id.0,
            event_type = %event.event_type,
            ts_ms = event.ts_ms,
            data = %event.data,
            "telemetry wire event"
        );
    }
}

/// The sink the engine dispatches into while a pipe is enabled.
pub(crate) struct PipeSink {
    shared: Arc<PipeShared>,
}

impl TelemetrySink for PipeSink {
    fn emit(&self, record: &TelemetryRecord) {
        if !self.shared.enabled.load(Ordering::Relaxed) {
            return;
        }
        if let TelemetryRecord::Device(snapshot) = record {
            self.shared
                .note_battery(snapshot.battery_level, snapshot.is_charging);
        }
        let now = (self.shared.clock)();
        let trigger = lock(&self.shared.pipeline).handle(record, now);
        if trigger {
            self.shared.signal.wake(wake::SIZE);
        }
    }

    fn try_emit_protocol_event(&self, event: &Event) -> bool {
        if !self.shared.enabled.load(Ordering::Relaxed) {
            return true;
        }
        // Dropped by name before any clock read or lock: this is the path
        // the sixty-odd uncollected events take.
        if !classify::PROTOCOL_ALLOWLIST.contains(&event.telemetry_name()) {
            return true;
        }
        let now = (self.shared.clock)();
        let trigger = lock(&self.shared.pipeline).handle_protocol_event(event, now);
        if trigger {
            self.shared.signal.wake(wake::SIZE);
        }
        true
    }
}

/// A running telemetry pipe. Created by `OfflineProtocol::enable_telemetry`
/// and stopped by `disable_telemetry` or by being dropped.
pub struct TelemetryPipe {
    shared: Arc<PipeShared>,
    scheduler: Mutex<Option<Scheduler>>,
    stopped: AtomicBool,
}

impl std::fmt::Debug for TelemetryPipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelemetryPipe")
            .field("enabled", &self.shared.enabled.load(Ordering::Relaxed))
            .field("stopped", &self.stopped.load(Ordering::Relaxed))
            .finish()
    }
}

/// What a pipe is built from, beyond its configuration.
pub(crate) struct PipeParts {
    pub(crate) host: TelemetryHost,
    pub(crate) backend: Backend,
    pub(crate) device_id: Option<String>,
    pub(crate) client: Box<dyn HttpClient>,
    pub(crate) clock: Clock,
    pub(crate) jitter: Box<dyn FnMut() -> u64 + Send>,
    /// `false` runs cycles inline on the caller (tests).
    pub(crate) spawn_thread: bool,
}

impl PipeParts {
    /// The production parts: the ureq client, the system clock, OS entropy
    /// for the jitter, and a thread.
    pub(crate) fn production(
        host: TelemetryHost,
        backend: Backend,
        device_id: Option<String>,
    ) -> Self {
        #[cfg(feature = "test-utils")]
        let client = testing::override_client().unwrap_or_else(|| Box::new(UreqClient::new()));
        #[cfg(not(feature = "test-utils"))]
        let client: Box<dyn HttpClient> = Box::new(UreqClient::new());
        Self {
            host,
            backend,
            device_id,
            client,
            clock: system_clock(),
            jitter: Box::new(|| {
                use rand_core::RngCore;
                u64::from(rand_core::OsRng.next_u32())
            }),
            spawn_thread: true,
        }
    }
}

impl TelemetryPipe {
    /// Builds and starts a pipe. The configuration must already be valid.
    pub(crate) fn start(config: &TelemetryConfig, parts: PipeParts) -> Result<Arc<Self>, Error> {
        let now = (parts.clock)();
        let pipeline = Pipeline::new(
            config.max_buffered_records(),
            config.max_batch_bytes(),
            config.routing_diagnostic(),
            now,
        );
        let uploader = Uploader::new(
            parts.client,
            config.api_key().unwrap_or_default(),
            config.app_id().unwrap_or_default(),
            user_agent(parts.host.os.user_agent_token(), parts.host.os_major),
            parts.jitter,
        );
        let mut store = BatchStore::new(
            Backend::Memory,
            config.app_id().unwrap_or_default().to_string(),
        );
        let (pending_attach, adoption_blocked) = match store.attach(parts.backend) {
            Attach::Adopted => (None, false),
            Attach::Busy(backend) => (Some(backend), true),
            Attach::Failed(backend) => (Some(backend), false),
        };
        let durable = store.is_durable();
        let shared = Arc::new(PipeShared {
            enabled: Arc::new(AtomicBool::new(true)),
            debug: config.debug(),
            inline: !parts.spawn_thread,
            pipeline: Mutex::new(pipeline),
            store: Mutex::new(store),
            uploader: Mutex::new(uploader),
            pending_attach: Mutex::new(pending_attach),
            adoption_blocked: AtomicBool::new(adoption_blocked),
            device_id: Mutex::new(parts.device_id),
            include_device_id: config.include_device_id(),
            durable: AtomicBool::new(durable),
            envelope: Envelope {
                app_version: config.app_version().to_string(),
                os: parts.host.os,
                os_major: parts.host.os_major,
            },
            max_batch_bytes: config.max_batch_bytes(),
            flush_interval: config.flush_interval(),
            clock: parts.clock,
            signal: WakeSignal::default(),
            battery: AtomicU32::new(0),
            sent_events: AtomicU64::new(0),
            accepted_events: AtomicU64::new(0),
            uploader_dropped: AtomicU64::new(0),
            store_dropped: AtomicU64::new(0),
            last_flush_at_ms: AtomicI64::new(-1),
            last_error: Mutex::new(None),
            server_disabled: AtomicBool::new(false),
        });
        // The host's state at enable time, recorded rather than reported:
        // there is no session to close yet, and the seed exists so that a
        // pipe enabled in the background rotates on the first foreground.
        // See `Pipeline::seed_app_state`. Done before the worker exists, so
        // it can never observe an unseeded pipeline.
        lock(&shared.pipeline).seed_app_state(parts.host.app_state);
        let scheduler = if parts.spawn_thread {
            let worker = shared.clone();
            Some(
                Scheduler::spawn(move || run_worker(worker))
                    .map_err(|e| Error::Other(format!("telemetry thread could not start: {e}")))?,
            )
        } else {
            None
        };
        Ok(Arc::new(Self {
            shared,
            scheduler: Mutex::new(scheduler),
            stopped: AtomicBool::new(false),
        }))
    }

    /// The sink to install on the engine.
    pub(crate) fn sink(&self) -> Arc<dyn TelemetrySink> {
        Arc::new(PipeSink {
            shared: self.shared.clone(),
        })
    }

    /// The hot-path gate, shared with the engine's telemetry context.
    pub(crate) fn enabled_flag(&self) -> Arc<AtomicBool> {
        self.shared.enabled.clone()
    }

    /// Stops or resumes collection. Off, an emit costs one atomic load and
    /// nothing is buffered; what is already queued still drains.
    ///
    /// Resuming restamps the summary anchor. The duration is the one summary
    /// field derived from a clock rather than accumulated from records, so
    /// unlike every counter it keeps growing while collection is off, and the
    /// first row afterwards would otherwise bill the entire opt-out. Only the
    /// off-to-on edge does this, so repeated `set_enabled(true)` calls on a
    /// live pipe do not keep resetting the span.
    pub fn set_enabled(&self, enabled: bool) {
        let was = self.shared.enabled.swap(enabled, Ordering::Relaxed);
        if enabled && !was {
            let now = (self.shared.clock)();
            lock(&self.shared.pipeline).resume_collection(now);
        }
    }

    /// Whether collection is on.
    pub fn is_enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    /// Asks the uploader to flush now. Returns immediately.
    pub fn flush(&self) {
        if self.shared.inline {
            self.shared.cycle(wake::EXPLICIT, None);
            return;
        }
        self.shared.signal.wake(wake::EXPLICIT);
    }

    /// Flushes and waits up to `budget` for the uploader to finish the
    /// cycle. Returns whether it finished in time.
    pub fn flush_blocking(&self, budget: Duration) -> bool {
        if self.shared.inline {
            self.shared
                .cycle(wake::EXPLICIT, Some(Instant::now() + budget));
            return true;
        }
        if self.stopped.load(Ordering::Relaxed) {
            return false;
        }
        let generation = self.shared.signal.request_flush(wake::EXPLICIT);
        self.shared
            .signal
            .wait_completed(generation, Instant::now() + budget)
    }

    /// A snapshot of the counters.
    pub fn stats(&self) -> TelemetryStats {
        let (buffered, ring_dropped, session_id) = {
            let pipeline = lock(&self.shared.pipeline);
            (
                pipeline.buffered() as u64,
                pipeline.dropped(),
                pipeline.session_id().to_string(),
            )
        };
        let last_flush = self.shared.last_flush_at_ms.load(Ordering::Relaxed);
        TelemetryStats {
            buffered,
            sent_events: self.shared.sent_events.load(Ordering::Relaxed),
            accepted_events: self.shared.accepted_events.load(Ordering::Relaxed),
            dropped: ring_dropped
                + self.shared.store_dropped.load(Ordering::Relaxed)
                + self.shared.uploader_dropped.load(Ordering::Relaxed),
            session_id,
            last_error: lock(&self.shared.last_error).clone(),
            last_flush_at_ms: (last_flush >= 0).then_some(last_flush),
        }
    }

    /// Closes the current session: summary, a blocking flush within
    /// [`FINAL_FLUSH_BUDGET`], then a fresh session id.
    ///
    /// A no-op while collection is off, for the reason given on
    /// [`Self::notify_app_state`]. [`Self::flush`] is the call that pushes a
    /// queued backlog without collecting anything new.
    pub fn end_session(&self) {
        if !self.is_enabled() {
            return;
        }
        let now = (self.shared.clock)();
        lock(&self.shared.pipeline).emit_session_summary(now);
        self.flush_blocking(FINAL_FLUSH_BUDGET);
        let now = (self.shared.clock)();
        lock(&self.shared.pipeline).rotate_session(now);
    }

    /// Reports an application lifecycle transition.
    ///
    /// While collection is off the transition is recorded but nothing is
    /// emitted. A session summary is collection: it is a row the ingest
    /// stores and bills, carrying the session duration and every counter the
    /// pairer accumulated, so emitting one for a user who switched telemetry
    /// off would break the promise `docs/telemetry.md` makes for that switch.
    /// The edge is still tracked, so the next foreground after collection
    /// resumes rotates the session rather than being read as a non-edge.
    pub fn notify_app_state(&self, state: AppState) {
        if !self.is_enabled() {
            lock(&self.shared.pipeline).seed_app_state(state);
            return;
        }
        let now = (self.shared.clock)();
        let action = lock(&self.shared.pipeline).app_state(state, now);
        if action == LifecycleAction::BackgroundFlush {
            if self.shared.inline {
                self.shared.cycle(wake::BACKGROUND, None);
            } else {
                self.shared.signal.wake(wake::BACKGROUND);
            }
        }
    }

    /// Reports a transport status change observed by the engine's tick. An
    /// internet-capable transport becoming available is a reason to try
    /// sending what an offline stretch queued.
    pub(crate) fn notify_transport(&self, transport: TransportType, status: TransportStatus) {
        if matches!(transport, TransportType::Internet | TransportType::Nostr)
            && status == TransportStatus::Available
        {
            if self.shared.inline {
                self.shared.cycle(wake::TRANSPORT, None);
            } else {
                self.shared.signal.wake(wake::TRANSPORT);
            }
        }
    }

    /// Hands the pipe a durable backend. Adopted by the uploader on its next
    /// cycle, which persists whatever is pending, so the caller never waits
    /// behind a request. While another pipe in this process still owns the
    /// queue on that storage, adoption waits for it to let go.
    pub(crate) fn attach_storage(
        &self,
        storage: Arc<dyn ProtocolStateStorage>,
        cipher: StateRecordCipher,
    ) {
        *lock(&self.shared.pending_attach) = Some(Backend::Sealed { storage, cipher });
        if self.shared.inline {
            self.shared.cycle(wake::ATTACH, None);
        } else {
            self.shared.signal.wake(wake::ATTACH);
        }
    }

    /// Sets the per-install id stamped on batches when the configuration
    /// opted in. Re-asked by the engine until it resolves.
    pub(crate) fn set_device_id(&self, device_id: Option<String>) {
        *lock(&self.shared.device_id) = device_id;
    }

    /// Stops the uploader after one final flush, waiting up to `budget` for
    /// the thread to exit before detaching it. Idempotent.
    pub fn stop(&self, budget: Duration) -> bool {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return true;
        }
        self.shared.signal.request_stop();
        let mut scheduler = lock(&self.scheduler);
        match scheduler.as_mut() {
            Some(scheduler) => scheduler.join_until(Instant::now() + budget),
            None => {
                self.shared.cycle(wake::STOP, Some(Instant::now() + budget));
                self.shared.release_queue();
                true
            }
        }
    }

    /// Whether `stop` has run.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Whether the batch queue is mirrored to storage.
    ///
    /// Reads an atomic rather than the store mutex, which the uploader holds
    /// across a request: a caller asking a question about configuration must
    /// not be parked behind a fifteen-second socket timeout.
    pub fn is_durable(&self) -> bool {
        self.shared.durable.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn shared(&self) -> &Arc<PipeShared> {
        &self.shared
    }

    #[cfg(test)]
    pub(crate) fn run_cycle(&self, reasons: u8) {
        self.shared.cycle(reasons, None);
    }
}

impl Drop for TelemetryPipe {
    /// Signals the final flush and detaches the worker without waiting for it.
    ///
    /// The flush still happens, bounded by [`FINAL_FLUSH_BUDGET`] on the
    /// worker's own thread; what the dropping thread does not do is block for
    /// it. A drop lands wherever the last handle happens to be released, and
    /// on the mobile bindings that is a teardown path: iOS releases native
    /// modules on the main thread during a bridge reload, so waiting here put
    /// three seconds inside the scene-update watchdog's window. A caller that
    /// needs to observe the flush calls [`Self::stop`] with a budget first,
    /// which is what `disable_telemetry` does.
    ///
    /// An inline pipe is the exception, and only because it has to be: with
    /// no worker there is no other thread to run the flush on, so the
    /// dropping thread is the only one that can. That configuration is the
    /// tests and the benches, never a shipped binary.
    fn drop(&mut self) {
        let budget = if self.shared.inline {
            FINAL_FLUSH_BUDGET
        } else {
            Duration::ZERO
        };
        self.stop(budget);
    }
}
