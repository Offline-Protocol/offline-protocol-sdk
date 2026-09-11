//! The background thread and what wakes it.
//!
//! The pipe's uploader is the one production thread in the workspace. It is
//! parked on a condition variable between flushes, woken by a bounded set of
//! reasons, and stopped with a deadline: `stop` sets the flag, notifies, and
//! polls the join handle until the deadline, after which the thread is
//! detached to finish on the HTTP client's own timeout. A detached thread
//! holds only the pipe's shared state, never the engine, so it can outlive
//! `disable_telemetry` without keeping anything else alive.

use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Why the worker woke. Bits, so several reasons coalesce into one cycle.
pub(crate) mod wake {
    pub(crate) const INTERVAL: u8 = 1;
    pub(crate) const SIZE: u8 = 2;
    pub(crate) const EXPLICIT: u8 = 4;
    pub(crate) const TRANSPORT: u8 = 8;
    pub(crate) const BACKGROUND: u8 = 16;
    pub(crate) const STOP: u8 = 32;
    pub(crate) const ATTACH: u8 = 64;

    /// Reasons whose flush is never deferred for battery and never waits for
    /// the next interval.
    pub(crate) const FINAL: u8 = EXPLICIT | BACKGROUND | STOP;
}

#[derive(Debug, Default)]
struct State {
    pending: u8,
    stop: bool,
    /// Flush generations: a caller waiting on a flush records the generation
    /// it requested, and the worker publishes the one it completed.
    requested: u64,
    completed: u64,
}

/// The wake channel between the emit side and the worker.
#[derive(Debug, Default)]
pub(crate) struct WakeSignal {
    state: Mutex<State>,
    cv: Condvar,
}

impl WakeSignal {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records a wake reason and wakes the worker.
    pub(crate) fn wake(&self, reason: u8) {
        let mut state = self.lock();
        state.pending |= reason;
        drop(state);
        self.cv.notify_all();
    }

    /// Requests a flush and returns the generation to wait on.
    pub(crate) fn request_flush(&self, reason: u8) -> u64 {
        let mut state = self.lock();
        state.pending |= reason;
        state.requested += 1;
        let generation = state.requested;
        drop(state);
        self.cv.notify_all();
        generation
    }

    /// Asks the worker to stop after one final flush.
    pub(crate) fn request_stop(&self) {
        let mut state = self.lock();
        state.stop = true;
        state.pending |= wake::STOP;
        drop(state);
        self.cv.notify_all();
    }

    /// Blocks until a reason is pending or `deadline` passes, then takes the
    /// pending reasons. A deadline that passes with nothing pending is an
    /// `INTERVAL` wake. Also returns the flush generation the caller should
    /// publish once this cycle completes.
    pub(crate) fn wait(&self, deadline: Instant) -> (u8, u64) {
        let mut state = self.lock();
        loop {
            if state.pending != 0 || state.stop {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                state.pending |= wake::INTERVAL;
                break;
            }
            let (guard, _) = self
                .cv
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
        }
        let reasons = state.pending | if state.stop { wake::STOP } else { 0 };
        state.pending = 0;
        (reasons, state.requested)
    }

    /// Publishes that every flush requested up to `generation` completed.
    pub(crate) fn complete(&self, generation: u64) {
        let mut state = self.lock();
        if generation > state.completed {
            state.completed = generation;
        }
        drop(state);
        self.cv.notify_all();
    }

    /// Waits for `generation` to complete. Returns `false` on the deadline.
    pub(crate) fn wait_completed(&self, generation: u64, deadline: Instant) -> bool {
        let mut state = self.lock();
        loop {
            if state.completed >= generation {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _) = self
                .cv
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
        }
    }
}

/// The worker thread's handle.
pub(crate) struct Scheduler {
    handle: Option<JoinHandle<()>>,
}

impl Scheduler {
    /// Spawns the worker.
    pub(crate) fn spawn<F>(body: F) -> std::io::Result<Self>
    where
        F: FnOnce() + Send + 'static,
    {
        let handle = std::thread::Builder::new()
            .name("offline-protocol-telemetry".into())
            .spawn(body)?;
        Ok(Self {
            handle: Some(handle),
        })
    }

    /// Whether the worker has exited.
    #[cfg(test)]
    pub(crate) fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for the worker to exit until `deadline`, then detaches it.
    /// Returns whether it was joined.
    pub(crate) fn join_until(&mut self, deadline: Instant) -> bool {
        let Some(handle) = self.handle.take() else {
            return true;
        };
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                // Detached: the thread finishes on its own timeouts and
                // holds nothing but the pipe's shared state.
                drop(handle);
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = handle.join();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// The calling thread's kernel id: `/proc/thread-self` links to
    /// `<pid>/task/<tid>`.
    #[cfg(target_os = "linux")]
    fn current_tid() -> std::ffi::OsString {
        std::fs::read_link("/proc/thread-self")
            .expect("procfs")
            .file_name()
            .expect("a task id")
            .to_owned()
    }

    #[test]
    fn a_thousand_start_stop_cycles_leave_no_thread_behind() {
        // Each worker records its kernel thread id, and the check is that
        // none of those ids is still in this process's task list. Nothing
        // else is counted. The harness runs this binary's tests in parallel,
        // so a process-wide thread count is a bound on the siblings: it
        // failed on CI twice, at an absolute bound and then at a delta
        // against a baseline, with nothing leaked here. Counting threads by
        // the scheduler's name carries the same noise, because the threaded
        // pipe tests run workers under that name and the detach test below
        // leaves one running after it returns.
        #[cfg(target_os = "linux")]
        let tids = Arc::new(Mutex::new(Vec::new()));
        for _ in 0..1_000 {
            let signal = Arc::new(WakeSignal::default());
            let worker_signal = signal.clone();
            #[cfg(target_os = "linux")]
            let worker_tids = tids.clone();
            let mut scheduler = Scheduler::spawn(move || {
                #[cfg(target_os = "linux")]
                worker_tids.lock().expect("tids").push(current_tid());
                loop {
                    let (reasons, gen) =
                        worker_signal.wait(Instant::now() + Duration::from_secs(60));
                    worker_signal.complete(gen);
                    if reasons & wake::STOP != 0 {
                        break;
                    }
                }
            })
            .expect("spawn");
            signal.request_stop();
            assert!(scheduler.join_until(Instant::now() + Duration::from_secs(5)));
            assert!(scheduler.is_finished());
        }
        #[cfg(target_os = "linux")]
        {
            let tids = tids.lock().expect("tids");
            assert_eq!(tids.len(), 1_000, "every worker recorded its id");
            // Polled rather than read once: the kernel wakes a joiner before
            // it removes the exiting thread from the task list, so a joined
            // thread can stay listed for a moment. A thread id is not reused
            // until the id space wraps, so an id still listed is one of these
            // workers.
            let task = std::path::Path::new("/proc/self/task");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let alive = tids.iter().filter(|tid| task.join(tid).exists()).count();
                if alive == 0 {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{alive} of the {} workers this test started are still running",
                    tids.len()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn a_stop_while_the_worker_is_blocked_returns_at_the_deadline_and_detaches() {
        let started = Arc::new(AtomicUsize::new(0));
        let worker_started = started.clone();
        let mut scheduler = Scheduler::spawn(move || {
            worker_started.store(1, Ordering::SeqCst);
            // Blocked in something that ignores the stop flag, like a
            // request waiting on a server that never answers.
            std::thread::sleep(Duration::from_millis(600));
        })
        .expect("spawn");
        while started.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let before = Instant::now();
        let joined = scheduler.join_until(Instant::now() + Duration::from_millis(100));
        assert!(!joined);
        assert!(before.elapsed() < Duration::from_millis(300));
        assert!(
            scheduler.is_finished(),
            "a detached handle reads as finished"
        );
    }

    #[test]
    fn wake_reasons_coalesce_and_a_deadline_is_an_interval_wake() {
        let signal = WakeSignal::default();
        signal.wake(wake::SIZE);
        signal.wake(wake::TRANSPORT);
        let (reasons, _) = signal.wait(Instant::now());
        assert_eq!(reasons, wake::SIZE | wake::TRANSPORT);
        let (reasons, _) = signal.wait(Instant::now());
        assert_eq!(reasons, wake::INTERVAL);
    }

    #[test]
    fn a_flush_generation_completes_or_times_out() {
        let signal = WakeSignal::default();
        let generation = signal.request_flush(wake::EXPLICIT);
        assert!(!signal.wait_completed(generation, Instant::now() + Duration::from_millis(20)));
        let (_, gen) = signal.wait(Instant::now());
        signal.complete(gen);
        assert!(signal.wait_completed(generation, Instant::now()));
    }
}
