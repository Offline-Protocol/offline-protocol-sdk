//! Generic fixed-window rate limiter for event sampling.

use std::collections::HashMap;
use std::sync::Mutex;

const DEFAULT_WINDOW_MS: i64 = 1_000;
const DEFAULT_MAX_EVENTS: u32 = 10;
const DEFAULT_MAX_KEYS: usize = 4096;

#[derive(Debug, Clone)]
struct EventWindow {
    window_start_ms: i64,
    count: u32,
}

/// Best-effort fixed-window sampler that limits events per category key.
///
/// Each unique key gets its own window. When a key's budget is exhausted for
/// the current window, further events with that key are suppressed until the
/// window resets. An eviction policy prevents unbounded memory growth when
/// the number of distinct keys exceeds `max_keys`.
#[derive(Debug)]
pub struct CategorySampler {
    max_events_per_window: u32,
    window_ms: i64,
    max_keys: usize,
    windows: Mutex<HashMap<String, EventWindow>>,
}

impl Default for CategorySampler {
    fn default() -> Self {
        Self {
            max_events_per_window: DEFAULT_MAX_EVENTS,
            window_ms: DEFAULT_WINDOW_MS,
            max_keys: DEFAULT_MAX_KEYS,
            windows: Mutex::new(HashMap::new()),
        }
    }
}

impl CategorySampler {
    /// Creates a sampler with custom parameters.
    pub fn new(max_events_per_window: u32, window_ms: i64, max_keys: usize) -> Self {
        Self {
            max_events_per_window,
            window_ms,
            max_keys,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Returns `true` when an event with the given key should be emitted.
    ///
    /// If the mutex is poisoned the call conservatively returns `true` so
    /// events are never silently dropped due to an unrelated panic.
    pub fn allow(&self, key: &str, now_ms: i64) -> bool {
        let Ok(mut windows) = self.windows.lock() else {
            return true;
        };

        if windows.len() >= self.max_keys && !windows.contains_key(key) {
            windows.retain(|_, window| {
                now_ms.saturating_sub(window.window_start_ms) <= self.window_ms
            });
            if windows.len() >= self.max_keys {
                windows.clear();
            }
        }

        let window = windows.entry(key.to_string()).or_insert(EventWindow {
            window_start_ms: now_ms,
            count: 0,
        });

        if now_ms.saturating_sub(window.window_start_ms) >= self.window_ms {
            window.window_start_ms = now_ms;
            window.count = 0;
        }

        if window.count >= self.max_events_per_window {
            return false;
        }

        window.count = window.count.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_budget_exhausted() {
        let sampler = CategorySampler::default();
        let now = 1000_i64;
        let key = "k";
        for _ in 0..DEFAULT_MAX_EVENTS {
            assert!(sampler.allow(key, now));
        }
        assert!(!sampler.allow(key, now));
    }

    #[test]
    fn window_resets_after_elapsed() {
        let sampler = CategorySampler::default();
        let now = 1000_i64;
        for _ in 0..DEFAULT_MAX_EVENTS {
            sampler.allow("k", now);
        }
        assert!(!sampler.allow("k", now));
        assert!(sampler.allow("k", now + DEFAULT_WINDOW_MS));
    }

    #[test]
    fn independent_keys_have_separate_budgets() {
        let sampler = CategorySampler::default();
        let now = 1000_i64;
        for _ in 0..DEFAULT_MAX_EVENTS {
            sampler.allow("a", now);
        }
        assert!(!sampler.allow("a", now));
        assert!(sampler.allow("b", now));
    }

    #[test]
    fn eviction_under_key_pressure() {
        let sampler = CategorySampler::new(10, 1_000, 2);
        let now = 1000_i64;
        assert!(sampler.allow("a", now));
        assert!(sampler.allow("b", now));
        // Third key triggers eviction — should still be accepted.
        assert!(sampler.allow("c", now));
    }
}
