//! Retry queue with exponential backoff.

/// Retry queue for managing message retries.
pub struct RetryQueue {
    // Placeholder - will be implemented in Phase 4
}

impl RetryQueue {
    /// Creates a new retry queue.
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for RetryQueue {
    fn default() -> Self {
        Self::new()
    }
}
