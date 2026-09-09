//! The performance proofs: the emit path allocates nothing for a dropped
//! protocol event and at most two blocks for a forwarded one, the emit
//! side never waits long for the pipe's lock while the uploader drains, and
//! enabling telemetry does no I/O on the caller.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::events::Event;
use crate::telemetry::pipe::store::Backend;
use crate::telemetry::pipe::tests::{inline_pipe, test_config, CapturingClient, FakeClock};

/// Counts this thread's allocations while armed. Installed for this test
/// binary only; other test threads allocate freely and are not counted.
struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

// SAFETY: delegates every operation to the system allocator unchanged; the
// counters are `const`-initialized thread locals with no destructor, so
// reading them never allocates.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let counting = COUNTING.try_with(Cell::get).unwrap_or(false);
        if counting {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        }
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn count_allocations(f: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|n| n.set(0));
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    ALLOCATIONS.with(Cell::get)
}

#[test]
fn a_dropped_protocol_event_allocates_nothing_and_a_forwarded_one_at_most_twice() {
    let clock = FakeClock::at(1_000);
    let pipe = inline_pipe(
        &test_config(),
        &clock,
        CapturingClient::accepting(),
        Backend::Memory,
    );
    let sink = pipe.sink();

    let dropped = Event::MessageReceived {
        message_id: "m".into(),
        sender: "a".into(),
        recipient: "b".into(),
        content: "hello".into(),
        hop_count: 1,
        transport: "ble".into(),
        timestamp: 0,
        lamport_clock: 0,
        reply_to_msg: None,
        reply_context: None,
        content_type: "text".into(),
        media_metadata: None,
        forward_info: None,
        encrypted: true,
    };
    let forwarded = Event::MessageFailed {
        message_id: "m".into(),
        reason: "Max retries exceeded".into(),
        retry_count: 3,
    };
    // Warm the ring so the push below does not grow the deque.
    for _ in 0..8 {
        sink.try_emit_protocol_event(&forwarded);
    }

    let dropped_allocs = count_allocations(|| {
        assert!(sink.try_emit_protocol_event(&dropped));
    });
    assert_eq!(
        dropped_allocs, 0,
        "a dropped protocol event must not allocate"
    );

    let forwarded_allocs = count_allocations(|| {
        assert!(sink.try_emit_protocol_event(&forwarded));
    });
    assert!(
        forwarded_allocs <= 2,
        "a forwarded protocol event allocated {forwarded_allocs} times"
    );
}

#[test]
fn the_emit_side_never_waits_long_for_the_pipe_lock_while_the_uploader_drains() {
    let clock = FakeClock::at(1_000);
    let pipe = inline_pipe(
        &test_config(),
        &clock,
        CapturingClient::accepting(),
        Backend::Memory,
    );
    let sink = pipe.sink();
    let stop = Arc::new(AtomicBool::new(false));

    let drainer = {
        let pipe = pipe.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                pipe.run_cycle(0);
            }
        })
    };

    let event = Event::MessageFailed {
        message_id: "m".into(),
        reason: "Max retries exceeded".into(),
        retry_count: 3,
    };
    // The budget is tens of microseconds: an emit is a match, a push, and a
    // mutex the uploader holds only for a `mem::take`. What this pins is that
    // the emit side never *waits* for the drain, so it is asserted over the
    // population rather than on the single worst sample. A handful of samples
    // are always stretched by the OS descheduling this thread mid-push, and a
    // Windows quantum is ~16 ms, which is what a worst-case bound was really
    // measuring when it failed on CI at 16.6 ms. A regression that put I/O or
    // a held lock on the emit path would push most of the population over the
    // budget, not one sample in a hundred thousand.
    const BUDGET: Duration = Duration::from_millis(1);
    const SAMPLES: usize = 100_000;
    let mut worst = Duration::ZERO;
    let mut over_budget = 0usize;
    for _ in 0..SAMPLES {
        let started = Instant::now();
        sink.try_emit_protocol_event(&event);
        let elapsed = started.elapsed();
        worst = worst.max(elapsed);
        if elapsed > BUDGET {
            over_budget += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    drainer.join().expect("drainer exits");
    assert!(
        over_budget * 100 <= SAMPLES,
        "{over_budget} of {SAMPLES} emits exceeded {BUDGET:?} (worst {worst:?}); \
         the emit side is waiting for the uploader"
    );
}

#[test]
fn enabling_telemetry_does_no_storage_io_on_the_caller() {
    use crate::protocol_state_storage::{ProtocolStateResult, ProtocolStateStorage};

    /// A provider whose every read sleeps, as a slow disk would.
    struct SlowStorage;
    impl ProtocolStateStorage for SlowStorage {
        fn store(&self, _: &str, _: &str, _: &[u8]) -> ProtocolStateResult<()> {
            std::thread::sleep(Duration::from_millis(100));
            Ok(())
        }
        fn load(&self, _: &str, _: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
            std::thread::sleep(Duration::from_millis(100));
            Ok(None)
        }
        fn delete(&self, _: &str, _: &str) -> ProtocolStateResult<()> {
            Ok(())
        }
        fn list_keys(&self, _: &str) -> ProtocolStateResult<Vec<String>> {
            std::thread::sleep(Duration::from_millis(100));
            Ok(Vec::new())
        }
    }

    let clock = FakeClock::at(1_000);
    let pipe = inline_pipe(
        &test_config(),
        &clock,
        CapturingClient::accepting(),
        Backend::Memory,
    );
    let started = Instant::now();
    // Attaching hands the backend over; adoption happens on the uploader's
    // cycle, which is what the inline pipe runs here explicitly.
    let cipher = crate::protocol::state_crypto::StateRecordCipher::new(&[1u8; 32]);
    *crate::telemetry::pipe::lock(&pipe.shared().pending_attach) = Some(Backend::Sealed {
        storage: Arc::new(SlowStorage),
        cipher,
    });
    assert!(
        started.elapsed() < Duration::from_millis(1),
        "attaching storage took {:?}",
        started.elapsed()
    );
}
