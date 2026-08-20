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

/// Absolute cap on a restored outbox entry's total lifetime, as a multiple
/// of `outbox_max_lifetime_ms`, measured from `first_sent_at`.
///
/// The restore path refreshes the carrier-relative TTL of an entry that
/// lapsed while the process was down (restart ⇒ fresh delivery window).
/// Without an absolute bound, an app used briefly once past each lifetime
/// keeps re-granting the entry a fresh window forever, so "expiry is
/// terminal" would only hold in-process. Past this cap the entry is dropped
/// at restore with a terminal `message_failed` instead of refreshed
/// (4 × 7 days = 28 days at the default lifetime).
pub const OUTBOX_ABSOLUTE_LIFETIME_FACTOR: i32 = 4;

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

/// Maximum persisted outbound media transfer descriptors restored after a
/// restart (newest-first). Small on purpose: each maps to a
/// `MediaResendRequired` the app is expected to act on, and concurrent
/// transfers are already capped per peer.
pub const MAX_MEDIA_DESCRIPTORS: usize = 16;

/// Maximum concurrent outbound media transfers per recipient when chunks are
/// MLS-encrypted.
///
/// Encrypted chunks share the recipient's 1:1 session ratchet with text, and
/// the receiver retains out-of-order message keys for only
/// `SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE` (32) generations. Each transfer
/// keeps up to `MEDIA_WINDOW_SIZE_INTERNET` (8) chunks in flight, so two
/// concurrent transfers bound the media-side in-flight gap at 16
/// generations, leaving room for up to 16 further messages interleaved on
/// the same ratchet before a delayed chunk's key is deleted. That residual
/// is not a guarantee: text sends are unbounded, and a chunk delayed while
/// 32+ interleaved messages decrypt first is permanently undecryptable (the
/// receiver surfaces this as `MessageDecryptionFailed`). Without this cap,
/// concurrent transfers alone could push a delayed chunk beyond the
/// tolerance and permanently stall its transfer.
pub const MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER: usize = 2;

/// How many of a peer's transfer slots the SDK's own document-layer traffic
/// may occupy at once.
///
/// Strictly below [`MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER`], and that is
/// the whole point. A document-layer transfer is invisible to the
/// application on both sides, so an application whose own `send_media` fails
/// with [`crate::Error::MediaTransferLimit`] cannot see what is holding the
/// slots or wait for it to finish. Leaving one slot always reachable keeps
/// that error explainable by what the application itself is doing.
pub const MAX_CONCURRENT_INTERNAL_MEDIA_TRANSFERS_PER_PEER: usize = 1;

/// Metadata key indicating preferred transport for a message.
pub const TRANSPORT_PREFERENCE_KEY: &str = "transport_preference";

/// Transport preference value: prefer internet transport.
pub const TRANSPORT_PREFERENCE_INTERNET: &str = "internet";

/// Metadata key carrying the original content type on file-chunk messages.
pub const ORIGINAL_CONTENT_TYPE_KEY: &str = "original_content_type";

/// Maximum number of times a message may be forwarded. Prevents unbounded
/// forwarding chains from malicious or buggy clients.
pub const MAX_FORWARD_COUNT: u32 = 100;

/// Maximum messages to send in a single flush operation.
/// Prevents blocking the process loop when many messages are pending.
pub const FLUSH_BATCH_LIMIT: usize = 20;
