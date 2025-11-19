//! Constants used throughout the offline-protocol crate.

/// Default time-to-live for messages.
pub const DEFAULT_INITIAL_TTL: u8 = 8;

/// Metadata key for ACK messages indicating which message ID the ACK is for.
pub const ACK_FOR_KEY: &str = "ack_for";

/// Metadata key for ACK messages indicating the hop count.
pub const ACK_HOP_COUNT_KEY: &str = "ack_hop_count";

/// Metadata key for ACK messages indicating the transport used.
pub const ACK_TRANSPORT_KEY: &str = "ack_transport";

/// Maximum number of entries in the outbox before evicting oldest entries.
/// This prevents unbounded memory growth when messages cannot be delivered.
pub const MAX_OUTBOX_ENTRIES: usize = 500;

/// Maximum number of messages to keep in history for topology visualization.
/// This prevents unbounded memory growth while maintaining enough history
/// for accurate network statistics.
pub const MAX_MESSAGE_HISTORY: usize = 1000;

/// Number of messages to remove when history exceeds the maximum.
/// This provides a buffer to avoid frequent cleanup operations.
pub const HISTORY_CLEANUP_BATCH_SIZE: usize = 100;

/// Threshold for compacting observed stats to prevent unbounded growth.
pub const OBSERVED_STATS_COMPACT_THRESHOLD: u32 = 10_000;

/// EMA (Exponential Moving Average) alpha for latency tracking.
pub const LATENCY_EMA_ALPHA: f32 = 0.3;

/// EMA alpha for hop count tracking.
pub const HOP_COUNT_EMA_ALPHA: f32 = 0.2;

/// Default chunk size for file transfers (32 KB).
pub const DEFAULT_CHUNK_SIZE: usize = 32 * 1024;
