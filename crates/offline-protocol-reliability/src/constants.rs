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
///
/// 5 minutes. Delivery latency does not ride on this ceiling: outbox
/// flushes (start, transport reconnect, peer rediscovery, session
/// establishment) bypass backoff timers entirely. The periodic retry is a
/// safety net, and a lower ceiling only multiplies futile send attempts —
/// and per-failure `MessageRetrying` events — while a peer stays offline
/// within the 7-day outbox window.
pub const DEFAULT_MAX_DELAY_MS: u64 = 300_000;

/// Backoff multiplier for exponential backoff.
pub const DEFAULT_BACKOFF_MULTIPLIER: f32 = 2.0;

/// Maximum lifetime for messages in outbox (milliseconds).
///
/// 7 days, matching the app-layer presence-flush window: a peer is
/// considered recoverable for up to 7 days offline, so queued messages
/// must survive at least that long before the outbox gives up on them.
pub const DEFAULT_OUTBOX_LIFETIME_MS: u64 = 604_800_000;

/// Maximum lifetime for messages waiting on MLS session establishment.
///
/// This is deliberately finite so an unresolved or permanently invalid
/// recipient cannot remain in durable `pending_messages` forever.
pub const DEFAULT_PENDING_MESSAGE_LIFETIME_MS: u64 = 604_800_000;

/// Estimated size of an ACK entry in bytes (message ID + metadata).
pub const ESTIMATED_ACK_SIZE_BYTES: usize = 40;

/// Network size threshold below which all ACKs are relayed.
pub const SMALL_NETWORK_RELAY_THRESHOLD: usize = 5;

/// Density factor numerator for relay probability calculation.
pub const RELAY_DENSITY_FACTOR_NUMERATOR: f32 = 5.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// The RN native bridges rebuild the whole `RetryConfig` from JSON in
    /// `updateRetryConfig`, filling absent fields from hardcoded fallbacks.
    /// A fallback that drifts from the Rust defaults silently overrides the
    /// SDK default for every field the app didn't set (this happened once:
    /// `maxRetries` 3 vs 10). This test pins the bridge literals to the
    /// constants above; when a default changes here, it fails until both
    /// bridge files are updated.
    #[test]
    fn rn_bridge_retry_fallbacks_match_rust_defaults() {
        let rn_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/react-native");
        let kotlin_path =
            rn_root.join("android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt");
        let swift_path = rn_root.join("ios/OfflineProtocolModule.swift");

        // The guard only applies in the repo checkout; skip when the
        // bindings tree isn't present (e.g. a vendored crate).
        let (Ok(kotlin), Ok(swift)) = (
            std::fs::read_to_string(&kotlin_path),
            std::fs::read_to_string(&swift_path),
        ) else {
            eprintln!("bindings tree not present, skipping RN fallback drift check");
            return;
        };

        let ack_expected = [
            (
                format!("json.optLong(\"defaultTimeoutMs\", {DEFAULT_ACK_TIMEOUT_MS})"),
                format!(
                    "(config[\"defaultTimeoutMs\"] as? NSNumber)?.uint64Value ?? {DEFAULT_ACK_TIMEOUT_MS}"
                ),
            ),
            (
                format!("json.optLong(\"maxPendingAcks\", {DEFAULT_MAX_PENDING_ACKS})"),
                format!(
                    "(config[\"maxPendingAcks\"] as? NSNumber)?.uint64Value ?? {DEFAULT_MAX_PENDING_ACKS}"
                ),
            ),
        ];
        for (kotlin_fallback, swift_fallback) in &ack_expected {
            assert!(
                kotlin.contains(kotlin_fallback),
                "RN Android bridge ACK fallback drifted from the Rust default: \
                 expected `{kotlin_fallback}` in {}",
                kotlin_path.display()
            );
            assert!(
                swift.contains(swift_fallback),
                "RN iOS bridge ACK fallback drifted from the Rust default: \
                 expected `{swift_fallback}` in {}",
                swift_path.display()
            );
        }

        let kotlin_expected = [
            format!("json.optInt(\"maxRetries\", {DEFAULT_MAX_RETRIES})"),
            format!("json.optLong(\"initialDelayMs\", {DEFAULT_INITIAL_DELAY_MS})"),
            format!("json.optLong(\"maxDelayMs\", {DEFAULT_MAX_DELAY_MS})"),
            format!("json.optDouble(\"backoffMultiplier\", {DEFAULT_BACKOFF_MULTIPLIER:.1})"),
            format!("json.optLong(\"outboxMaxLifetimeMs\", {DEFAULT_OUTBOX_LIFETIME_MS})"),
            format!(
                "json.optLong(\"pendingMessageMaxLifetimeMs\", {DEFAULT_PENDING_MESSAGE_LIFETIME_MS})"
            ),
        ];
        for expected in &kotlin_expected {
            assert!(
                kotlin.contains(expected),
                "RN Android bridge retry fallback drifted from the Rust default: \
                 expected `{expected}` in {}",
                kotlin_path.display()
            );
        }

        let swift_expected = [
            format!("(config[\"maxRetries\"] as? NSNumber)?.uint32Value ?? {DEFAULT_MAX_RETRIES}"),
            format!(
                "(config[\"initialDelayMs\"] as? NSNumber)?.uint64Value ?? {DEFAULT_INITIAL_DELAY_MS}"
            ),
            format!("(config[\"maxDelayMs\"] as? NSNumber)?.uint64Value ?? {DEFAULT_MAX_DELAY_MS}"),
            format!(
                "(config[\"backoffMultiplier\"] as? NSNumber)?.floatValue ?? {DEFAULT_BACKOFF_MULTIPLIER:.1}"
            ),
            format!(
                "(config[\"outboxMaxLifetimeMs\"] as? NSNumber)?.uint64Value ?? {DEFAULT_OUTBOX_LIFETIME_MS}"
            ),
            format!(
                "(config[\"pendingMessageMaxLifetimeMs\"] as? NSNumber)?.uint64Value ?? {DEFAULT_PENDING_MESSAGE_LIFETIME_MS}"
            ),
        ];
        for expected in &swift_expected {
            assert!(
                swift.contains(expected),
                "RN iOS bridge retry fallback drifted from the Rust default: \
                 expected `{expected}` in {}",
                swift_path.display()
            );
        }
    }
}
