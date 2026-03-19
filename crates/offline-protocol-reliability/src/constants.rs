//! Reliability layer constants.

/// Default ACK timeout in milliseconds.
pub const DEFAULT_ACK_TIMEOUT_MS: u64 = 10000;

/// Maximum number of pending ACKs to track.
pub const DEFAULT_MAX_PENDING_ACKS: usize = 1000;

/// Default maximum number of retries per message.
pub const DEFAULT_MAX_RETRIES: u32 = 10;

/// Initial retry delay in milliseconds.
pub const DEFAULT_INITIAL_DELAY_MS: u64 = 1000;

/// Maximum retry delay in milliseconds.
pub const DEFAULT_MAX_DELAY_MS: u64 = 30000;

/// Backoff multiplier for exponential backoff.
pub const DEFAULT_BACKOFF_MULTIPLIER: f32 = 2.0;

/// Maximum lifetime for messages in outbox (milliseconds).
pub const DEFAULT_OUTBOX_LIFETIME_MS: u64 = 3600000;

/// Estimated size of an ACK entry in bytes (message ID + metadata).
pub const ESTIMATED_ACK_SIZE_BYTES: usize = 40;

/// Network size threshold below which all ACKs are relayed.
pub const SMALL_NETWORK_RELAY_THRESHOLD: usize = 5;

/// Density factor numerator for relay probability calculation.
pub const RELAY_DENSITY_FACTOR_NUMERATOR: f32 = 5.0;
