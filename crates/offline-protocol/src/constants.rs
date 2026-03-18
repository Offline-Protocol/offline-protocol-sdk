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

/// Chunk size for BLE file transfers (4 KB).
/// Reduces BLE double-fragmentation: ~22 BLE fragments per chunk instead of ~240.
pub const CHUNK_SIZE_BLE: usize = 4 * 1024;

/// Chunk size for Internet file transfers (256 KB).
/// Fewer round-trips over WebSocket where bandwidth is plentiful.
pub const CHUNK_SIZE_INTERNET: usize = 256 * 1024;

/// Maximum chunks in flight for BLE media transfers.
/// BLE's low bandwidth means fewer concurrent chunks to avoid congestion.
pub const MEDIA_WINDOW_SIZE_BLE: usize = 2;

/// Maximum chunks in flight for Internet media transfers.
pub const MEDIA_WINDOW_SIZE_INTERNET: usize = 8;

/// Default maximum chunks in flight for media transfers.
pub const DEFAULT_MEDIA_WINDOW_SIZE: usize = 4;

/// Maximum entries in the dedicated media outbox.
/// Smaller than the main outbox since the sliding window limits in-flight chunks.
pub const MAX_MEDIA_OUTBOX_ENTRIES: usize = 100;

/// Metadata key indicating preferred transport for a message.
pub const TRANSPORT_PREFERENCE_KEY: &str = "transport_preference";

/// Transport preference value: prefer internet transport.
pub const TRANSPORT_PREFERENCE_INTERNET: &str = "internet";

/// Metadata key carrying the original content type on file-chunk messages.
pub const ORIGINAL_CONTENT_TYPE_KEY: &str = "original_content_type";

/// Default route quality assigned when learning a route from a relayed message.
/// Conservative (0.5) because the transport layer doesn't expose the immediate
/// peer identity, so the route may be sub-optimal for multi-hop return paths.
pub const RELAY_LEARNED_ROUTE_QUALITY: f32 = 0.5;
