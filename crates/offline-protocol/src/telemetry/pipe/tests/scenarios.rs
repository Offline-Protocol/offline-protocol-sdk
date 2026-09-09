//! End-to-end scenarios through an inline pipe: durability, the halt rules,
//! the deferrals, and a soak through an outage.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::events::Event;
use crate::protocol::state_crypto::StateRecordCipher;
use crate::telemetry::device::DeviceCapabilitySnapshot;
use crate::telemetry::pipe::host::AppState;
use crate::telemetry::pipe::scheduler::wake;
use crate::telemetry::pipe::store::tests::MemoryStorage;
use crate::telemetry::pipe::store::{Backend, MAX_PENDING_BATCHES};
use crate::telemetry::pipe::tests::{inline_pipe, test_config, CapturingClient, FakeClock};
use crate::telemetry::pipe::uploader::BACKOFF_MAX_MS;
use crate::telemetry::pipe::{TelemetryPipe, FINAL_FLUSH_BUDGET};
use crate::telemetry::TelemetryRecord;
use offline_protocol_router::RelayRole;

fn failed(n: u32) -> Event {
    Event::MessageFailed {
        message_id: format!("m{n}"),
        reason: "Max retries exceeded".into(),
        retry_count: 1,
    }
}

fn emit_failed(pipe: &TelemetryPipe, n: u32) {
    assert!(pipe.sink().try_emit_protocol_event(&failed(n)));
}

fn sealed(storage: &Arc<MemoryStorage>) -> Backend {
    Backend::Sealed {
        storage: storage.clone(),
        cipher: StateRecordCipher::new(&[3u8; 32]),
    }
}

#[test]
fn a_pipe_enabled_before_storage_persists_what_was_pending_when_storage_attaches() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    client.status.store(0, Ordering::SeqCst); // offline: batches queue up
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    emit_failed(&pipe, 1);
    pipe.flush();
    assert!(!pipe.is_durable());
    assert_eq!(pipe.stats().buffered, 0, "cut into a batch");

    let storage = Arc::new(MemoryStorage::default());
    pipe.attach_storage(storage.clone(), StateRecordCipher::new(&[3u8; 32]));
    assert!(pipe.is_durable());
    let records = storage.records.lock().unwrap();
    assert!(
        records.keys().any(|(t, _)| t == "telemetry_batch"),
        "the pending batch was persisted on attach"
    );
    assert!(records.keys().any(|(t, _)| t == "telemetry_batch_index"));
}

#[test]
fn a_restart_replays_the_persisted_queue_and_the_ingest_deduplicates_on_batch_id() {
    let clock = FakeClock::at(1_000);
    let storage = Arc::new(MemoryStorage::default());
    let offline = CapturingClient::accepting();
    offline.status.store(0, Ordering::SeqCst);
    let first = inline_pipe(&test_config(), &clock, offline, sealed(&storage));
    emit_failed(&first, 1);
    first.flush();
    let queued_id = {
        let store = crate::telemetry::pipe::lock(&first.shared().store);
        store.front().expect("queued").batch_id
    };
    drop(first);

    let online = CapturingClient::accepting();
    let second = inline_pipe(&test_config(), &clock, online.clone(), sealed(&storage));
    second.flush();
    let batches = online.batches();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0]["batch_id"],
        queued_id.to_string(),
        "same id on replay"
    );
    assert_eq!(second.stats().sent_events, 1);
}

#[test]
fn a_401_halts_the_uploader_until_telemetry_is_enabled_again() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    client.status.store(401, Ordering::SeqCst);
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    emit_failed(&pipe, 1);
    pipe.flush();
    assert_eq!(client.bodies.lock().unwrap().len(), 1);
    assert_eq!(
        pipe.stats().last_error.as_deref(),
        Some("ingest responded 401")
    );
    assert_eq!(
        pipe.stats().dropped,
        0,
        "a refused key loses nothing: the batch stays queued for a good one"
    );

    client.status.store(202, Ordering::SeqCst);
    emit_failed(&pipe, 2);
    clock.set(1_000_000);
    pipe.flush();
    assert_eq!(
        client.bodies.lock().unwrap().len(),
        1,
        "halted: nothing more is sent"
    );

    // A new pipe (what `enable_telemetry` builds) sends again.
    let again = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    emit_failed(&again, 3);
    again.flush();
    assert_eq!(client.bodies.lock().unwrap().len(), 2);
}

/// The point of keeping the batch a 401 refused: with a durable queue it is
/// still there when the key is fixed.
///
/// A rejected key is a configuration fault, not bytes the ingest will never
/// take, so the events collected before anyone noticed are exactly the ones
/// worth having. Dropping the head — which is what the uploader used to do —
/// lost them for nothing, since the halt already stopped every later send.
#[test]
fn a_batch_refused_for_a_bad_key_is_still_there_when_the_key_is_fixed() {
    let clock = FakeClock::at(1_000);
    let storage = Arc::new(MemoryStorage::default());
    let client = CapturingClient::accepting();
    client.status.store(401, Ordering::SeqCst);

    let pipe = inline_pipe(&test_config(), &clock, client.clone(), sealed(&storage));
    emit_failed(&pipe, 1);
    pipe.flush();
    assert_eq!(client.bodies.lock().unwrap().len(), 1, "it was attempted");
    assert_eq!(pipe.stats().dropped, 0, "and nothing was lost");
    pipe.stop(FINAL_FLUSH_BUDGET);

    // The developer fixes the key in the portal and calls enableTelemetry
    // again, which is a fresh pipe over the same protocol-state store.
    client.status.store(202, Ordering::SeqCst);
    let again = inline_pipe(&test_config(), &clock, client.clone(), sealed(&storage));
    again.flush();
    let bodies = client.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "the refused batch was resent");
    assert!(
        bodies[1].contains("protocol.message.failed"),
        "and it is the same batch, not an empty one: {}",
        bodies[1]
    );
}

#[test]
fn low_battery_defers_interval_flushes_but_never_a_final_one() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    pipe.sink()
        .emit(&TelemetryRecord::Device(DeviceCapabilitySnapshot {
            timestamp_ms: 1_000,
            battery_level: Some(10),
            is_charging: false,
            relay_role: RelayRole::Regular,
            changed_fields: 1,
        }));
    emit_failed(&pipe, 1);
    pipe.run_cycle(wake::INTERVAL);
    assert!(client.bodies.lock().unwrap().is_empty(), "deferred");
    pipe.run_cycle(wake::TRANSPORT);
    assert!(client.bodies.lock().unwrap().is_empty(), "still deferred");
    // A deferred cycle still cuts batches into the durable queue; only the
    // send waits. The background flush is final, so both go out.
    pipe.notify_app_state(AppState::Background);
    assert_eq!(
        client.bodies.lock().unwrap().len(),
        2,
        "the background flush is final"
    );

    // Charging lifts the deferral.
    pipe.sink()
        .emit(&TelemetryRecord::Device(DeviceCapabilitySnapshot {
            timestamp_ms: 2_000,
            battery_level: Some(10),
            is_charging: true,
            relay_role: RelayRole::Regular,
            changed_fields: 2,
        }));
    emit_failed(&pipe, 2);
    pipe.run_cycle(wake::INTERVAL);
    assert_eq!(client.bodies.lock().unwrap().len(), 3);
}

#[test]
fn disabling_collection_stops_buffering_but_drains_what_was_queued() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    emit_failed(&pipe, 1);
    pipe.set_enabled(false);
    assert!(!pipe.is_enabled());
    emit_failed(&pipe, 2);
    assert_eq!(
        pipe.stats().buffered,
        1,
        "the second event was never buffered"
    );
    pipe.flush();
    assert_eq!(pipe.stats().sent_events, 1);
}

/// Collection off means the session boundaries stop reporting too.
///
/// A session summary is not bookkeeping: it is a row the ingest stores and
/// bills, carrying the session duration and every counter the pairer
/// accumulated over the window. Emitting one for a user who switched
/// telemetry off in a settings screen would put a record on the wire per
/// background for as long as the app runs, which is precisely what the
/// switch promises not to do. The edge is still tracked, so the next
/// foreground after collection resumes rotates rather than reading as a
/// non-edge.
#[test]
fn a_lifecycle_boundary_with_collection_off_reports_nothing() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    pipe.set_enabled(false);

    clock.set(60_000);
    pipe.notify_app_state(AppState::Background);
    pipe.end_session();
    assert_eq!(
        pipe.stats().buffered,
        0,
        "a summary was buffered with collection off"
    );
    assert!(
        client.bodies.lock().unwrap().is_empty(),
        "collection is off: nothing may leave the device"
    );

    // Back on. The tracked background edge means the next foreground is a
    // real rotation, and the following background reports as usual.
    let before = pipe.stats().session_id;
    pipe.set_enabled(true);
    clock.set(70_000);
    pipe.notify_app_state(AppState::Active);
    assert_ne!(pipe.stats().session_id, before, "the edge was tracked");
    emit_failed(&pipe, 1);
    clock.set(80_000);
    pipe.notify_app_state(AppState::Background);
    assert_eq!(client.bodies.lock().unwrap().len(), 1);
}

/// Enabling in the background bills nothing for the privilege.
///
/// An iOS BLE restoration and an Android headless mesh wake both enable
/// telemetry with the app in the background. Running the background arm of
/// the lifecycle there would emit an all-zero summary with a zero duration
/// and open a socket for it, on every such launch.
#[test]
fn enabling_in_the_background_emits_no_summary_and_opens_no_socket() {
    use crate::telemetry::pipe::host::{TelemetryHost, TelemetryOs};
    use crate::telemetry::pipe::PipeParts;

    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    let pipe = TelemetryPipe::start(
        &test_config(),
        PipeParts {
            host: TelemetryHost::new(TelemetryOs::Ios, 18).with_app_state(AppState::Background),
            backend: Backend::Memory,
            device_id: None,
            client: Box::new(client.clone()),
            clock: clock.clock(),
            jitter: Box::new(|| 0),
            spawn_thread: false,
        },
    )
    .expect("pipe starts");
    assert_eq!(pipe.stats().buffered, 0, "the seed emitted a summary");
    pipe.flush();
    assert!(
        client.bodies.lock().unwrap().is_empty(),
        "the seed opened a socket"
    );

    // The seed still arms the boundary: the first foreground rotates.
    let first = pipe.stats().session_id;
    clock.set(10_000);
    pipe.notify_app_state(AppState::Active);
    assert_ne!(pipe.stats().session_id, first);
}

#[test]
fn three_days_offline_never_exceed_the_caps_and_every_batch_arrives_exactly_once() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    client.status.store(0, Ordering::SeqCst);
    let config = test_config().with_max_batch_bytes(1024);
    let pipe = inline_pipe(&config, &clock, client.clone(), Backend::Memory);

    // 3 days of 30 s ticks, 40 events per tick, against a dead network.
    let ticks = 3 * 24 * 60 * 2;
    let mut emitted = 0u64;
    for tick in 0..ticks {
        clock.set(1_000 + tick * 30_000);
        for n in 0..40 {
            emit_failed(&pipe, n);
            emitted += 1;
        }
        pipe.run_cycle(wake::INTERVAL);
        let store = crate::telemetry::pipe::lock(&pipe.shared().store);
        assert!(store.len() <= MAX_PENDING_BATCHES);
        assert!(store.bytes() <= 4 * 1024 * 1024);
    }
    let stats = pipe.stats();
    assert_eq!(stats.sent_events, 0);
    let queued = {
        let store = crate::telemetry::pipe::lock(&pipe.shared().store);
        store.len()
    };
    assert!(queued > 0);
    assert!(stats.dropped > 0, "the caps trimmed the queue");
    assert!(stats.last_error.is_some());

    // The network returns. Ticks continue; every queued batch that is
    // still inside the TTL arrives exactly once.
    client.status.store(202, Ordering::SeqCst);
    let mut tick = ticks;
    loop {
        tick += 1;
        clock.set(1_000 + tick * 30_000);
        // Let the backoff lift: the clock jumps past the cap each tick.
        clock.set(clock.now() + BACKOFF_MAX_MS + 1);
        pipe.run_cycle(wake::INTERVAL);
        let remaining = crate::telemetry::pipe::lock(&pipe.shared().store).len();
        if remaining == 0 {
            break;
        }
        assert!(
            tick < ticks + 200,
            "the queue must drain once the network is back"
        );
    }
    let batches = client.batches();
    let mut ids: Vec<&str> = batches
        .iter()
        .map(|b| b["batch_id"].as_str().expect("id"))
        .collect();
    let received = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), received, "no batch id was received twice");
    let sent: u64 = batches
        .iter()
        .map(|b| b["events"].as_array().map_or(0, |e| e.len() as u64))
        .sum();
    let stats = pipe.stats();
    assert_eq!(stats.sent_events, sent);
    // The caps trimmed the queue during the outage: those count as dropped,
    // and sent plus dropped is everything that was ever emitted.
    assert_eq!(stats.sent_events + stats.dropped, emitted);
}

#[test]
fn a_stopped_pipe_flushes_once_within_its_budget_and_then_refuses_work() {
    let clock = FakeClock::at(1_000);
    let client = CapturingClient::accepting();
    let pipe = inline_pipe(&test_config(), &clock, client.clone(), Backend::Memory);
    emit_failed(&pipe, 1);
    assert!(pipe.stop(FINAL_FLUSH_BUDGET));
    assert!(pipe.is_stopped());
    assert_eq!(
        client.bodies.lock().unwrap().len(),
        1,
        "the final flush ran"
    );
    assert!(pipe.stop(FINAL_FLUSH_BUDGET), "idempotent");
}

#[test]
fn a_threaded_pipe_starts_flushes_on_request_and_stops_within_the_budget() {
    use crate::telemetry::pipe::host::{TelemetryHost, TelemetryOs};
    use crate::telemetry::pipe::PipeParts;

    let client = CapturingClient::accepting();
    let pipe = TelemetryPipe::start(
        &test_config(),
        PipeParts {
            host: TelemetryHost::new(TelemetryOs::Android, 34),
            backend: Backend::Memory,
            device_id: Some("device".into()),
            client: Box::new(client.clone()),
            clock: FakeClock::at(5_000).clock(),
            jitter: Box::new(|| 0),
            spawn_thread: true,
        },
    )
    .expect("starts");
    emit_failed(&pipe, 1);
    assert!(pipe.flush_blocking(std::time::Duration::from_secs(5)));
    assert_eq!(client.bodies.lock().unwrap().len(), 1);
    let stats = pipe.stats();
    assert_eq!(stats.sent_events, 1);
    assert_eq!(stats.last_flush_at_ms, Some(5_000));
    let started = std::time::Instant::now();
    assert!(pipe.stop(FINAL_FLUSH_BUDGET));
    assert!(started.elapsed() < FINAL_FLUSH_BUDGET);
}
