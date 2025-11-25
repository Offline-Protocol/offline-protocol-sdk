//! Reliability layer constants.

/// Default ACK timeout in milliseconds.
pub const DEFAULT_ACK_TIMEOUT_MS: u64 = 5000;

/// Maximum number of pending ACKs to track.
pub const DEFAULT_MAX_PENDING_ACKS: usize = 1000;

/// Default maximum number of retries per message.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Initial retry delay in milliseconds.
pub const DEFAULT_INITIAL_DELAY_MS: u64 = 1000;

/// Maximum retry delay in milliseconds.
pub const DEFAULT_MAX_DELAY_MS: u64 = 30000;

/// Backoff multiplier for exponential backoff.
pub const DEFAULT_BACKOFF_MULTIPLIER: f32 = 2.0;

/// Maximum lifetime for messages in outbox (milliseconds).
pub const DEFAULT_OUTBOX_LIFETIME_MS: u64 = 3600000;

