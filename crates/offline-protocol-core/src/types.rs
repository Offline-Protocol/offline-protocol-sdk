//! Core protocol types.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Why an identifier was rejected by [`validate_id_chars`].
///
/// `type_name` is the caller-supplied label (e.g. `"User ID"`) used only to
/// prefix the error message.
// Adding a variant to a public error enum is a breaking change without
// this attribute; downstream crates must carry a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdValidationError {
    /// The identifier is empty.
    #[error("{type_name} cannot be empty")]
    Empty {
        /// Label for the identifier kind being validated.
        type_name: String,
    },
    /// The identifier is a path-traversal component (`.` or `..`).
    #[error("{type_name} cannot be '.' or '..' (path traversal)")]
    PathTraversal {
        /// Label for the identifier kind being validated.
        type_name: String,
    },
    /// The identifier contains ASCII control characters.
    #[error("{type_name} contains ASCII control characters")]
    ControlChars {
        /// Label for the identifier kind being validated.
        type_name: String,
    },
    /// The identifier contains characters hostile to storage backends.
    #[error("{type_name} contains invalid characters ('/', '\\', or ':')")]
    InvalidChars {
        /// Label for the identifier kind being validated.
        type_name: String,
    },
    /// The identifier exceeds [`MAX_ID_LEN`].
    #[error("{type_name} exceeds the maximum length of {max} bytes ({len} bytes)")]
    TooLong {
        /// Label for the identifier kind being validated.
        type_name: String,
        /// Actual byte length that was rejected.
        len: usize,
        /// Maximum permitted byte length.
        max: usize,
    },
}

/// Maximum length, in bytes, of a [`UserId`] or [`AppId`].
///
/// Wire-supplied identifiers become map keys and are cloned into in-memory
/// buffers (the pending-decryption queue, for one, retains a full `Message`
/// clone), so an unbounded id is a resource-exhaustion vector: a peer could
/// inflate `sender` to megabytes to evade the queue byte budgets or leak a
/// per-id map entry. 256 bytes is far larger than any real identifier. MLS
/// group ids keep their own, larger cap because they compose several ids
/// behind a `:` namespace separator.
pub const MAX_ID_LEN: usize = 256;

/// Validates an identifier's length and character set. Shared by [`UserId`]
/// and [`AppId`]. The length check runs first so a hostile megabyte-scale id
/// is rejected before the O(n) character scan.
///
/// Not applied to MLS group ids, which validate per colon-separated segment
/// via [`validate_id_chars`] directly and enforce their own larger total-length
/// cap.
fn validate_id(id: &str, type_name: &str) -> Result<(), IdValidationError> {
    if id.len() > MAX_ID_LEN {
        return Err(IdValidationError::TooLong {
            type_name: type_name.to_string(),
            len: id.len(),
            max: MAX_ID_LEN,
        });
    }
    validate_id_chars(id, type_name)
}

/// Validates that an identifier string is safe for use as a storage key.
///
/// Rejects empty strings, path-traversal components (`.`, `..`), ASCII
/// control characters (including NUL), and characters hostile to storage
/// backends (`/`, `\`, `:` — filesystem path separators and key-separator
/// collisions in KV stores).
///
/// This is the shared policy behind [`UserId`] and [`AppId`]. Other crates
/// that persist wire-derived identifiers as storage keys (e.g. MLS group
/// ids) apply the same policy, per colon-separated segment where the
/// identifier format itself uses `:` as a namespace separator.
///
/// `type_name` is used only to label the error message.
pub fn validate_id_chars(id: &str, type_name: &str) -> Result<(), IdValidationError> {
    if id.is_empty() {
        return Err(IdValidationError::Empty {
            type_name: type_name.to_string(),
        });
    }
    if id == "." || id == ".." {
        return Err(IdValidationError::PathTraversal {
            type_name: type_name.to_string(),
        });
    }
    // Reject ASCII control characters (0x00–0x1F, 0x7F) and characters
    // hostile to storage backends (filesystem path traversal, key-separator
    // collisions in KV stores).
    if id.bytes().any(|b| b.is_ascii_control()) {
        return Err(IdValidationError::ControlChars {
            type_name: type_name.to_string(),
        });
    }
    if id.contains('/') || id.contains('\\') || id.contains(':') {
        return Err(IdValidationError::InvalidChars {
            type_name: type_name.to_string(),
        });
    }
    Ok(())
}

/// User identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct UserId(String);

impl UserId {
    /// Creates a new UserId.
    ///
    /// # Arguments
    ///
    /// * `id` - The user identifier string
    ///
    /// # Returns
    ///
    /// Returns `Ok(UserId)` if valid, `Err` if the ID is empty.
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id_str = id.into();
        validate_id(&id_str, "User ID").map_err(|e| crate::Error::InvalidUserId(e.to_string()))?;
        Ok(Self(id_str))
    }

    /// Returns the user ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for UserId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        UserId::new(s).map_err(serde::de::Error::custom)
    }
}

/// Application identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct AppId(String);

impl AppId {
    /// Creates a new AppId.
    ///
    /// # Arguments
    ///
    /// * `id` - The application identifier string
    ///
    /// # Returns
    ///
    /// Returns `Ok(AppId)` if valid, `Err` if the ID is empty.
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id_str = id.into();
        validate_id(&id_str, "App ID").map_err(|e| crate::Error::InvalidAppId(e.to_string()))?;
        Ok(Self(id_str))
    }

    /// Returns the app ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for AppId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        AppId::new(s).map_err(serde::de::Error::custom)
    }
}

/// Time-To-Live: maximum number of hops a message can traverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TTL(u8);

impl TTL {
    /// Maximum TTL value.
    pub const MAX: u8 = 255;

    /// Default TTL value (8 hops as per specification).
    pub const DEFAULT: u8 = 8;

    /// Creates a new TTL.
    ///
    /// # Arguments
    ///
    /// * `value` - The TTL value (must be > 0)
    ///
    /// # Returns
    ///
    /// Returns `Ok(TTL)` if valid, `Err` if the value is 0.
    pub fn new(value: u8) -> crate::Result<Self> {
        if value == 0 {
            return Err(crate::Error::InvalidTTL(value));
        }
        Ok(Self(value))
    }

    /// Creates a TTL with the default value.
    pub fn default_value() -> Self {
        Self(Self::DEFAULT)
    }

    /// Returns the TTL value.
    pub fn value(&self) -> u8 {
        self.0
    }

    /// Reconstructs a TTL from a raw wire value **without** enforcing the
    /// nonzero invariant.
    ///
    /// The JSON wire path deserializes `TTL` via a derived, transparent
    /// `Deserialize` that performs no validation, so a received `ttl` of 0 is
    /// representable on the wire. This constructor mirrors that behavior for the
    /// binary wire codec, keeping the two encodings equivalent in what they
    /// accept. Prefer [`TTL::new`] for locally-originated values.
    pub fn from_value(value: u8) -> Self {
        Self(value)
    }

    /// Decrements the TTL by 1.
    ///
    /// # Returns
    ///
    /// Returns `Some(TTL)` if TTL > 1, `None` if TTL would become 0.
    pub fn decrement(&self) -> Option<Self> {
        if self.0 > 1 {
            Some(Self(self.0 - 1))
        } else {
            None
        }
    }

    /// Checks if the TTL is exhausted (value is 1 or less).
    pub fn is_exhausted(&self) -> bool {
        self.0 <= 1
    }
}

impl Default for TTL {
    fn default() -> Self {
        Self::default_value()
    }
}

/// Hop count: number of hops a message has traversed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HopCount(u8);

impl HopCount {
    /// Creates a new HopCount starting at 0.
    pub fn new() -> Self {
        Self(0)
    }

    /// Returns the hop count value.
    pub fn value(&self) -> u8 {
        self.0
    }

    /// Reconstructs a hop count from a raw wire value.
    ///
    /// Mirrors the derived, transparent `Deserialize` used on the JSON wire path
    /// so the binary wire codec accepts the same values. Prefer [`HopCount::new`]
    /// for locally-originated counters.
    pub fn from_value(value: u8) -> Self {
        Self(value)
    }

    /// Increments the hop count by 1.
    ///
    /// # Returns
    ///
    /// Returns `Some(HopCount)` if not at max, `None` if at u8::MAX.
    pub fn increment(&self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl Default for HopCount {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall-clock timestamp in milliseconds since Unix epoch.
///
/// Use for human-readable display ("2:34 PM") only. Do NOT use for message
/// ordering or cross-device duration calculations — wall clocks are unreliable
/// across devices in offline/mesh networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WallClockTimestamp(i64);

impl WallClockTimestamp {
    /// Captures the current wall-clock time.
    pub fn now() -> Self {
        Self(chrono::Utc::now().timestamp_millis())
    }

    /// Creates a wall-clock timestamp from milliseconds since Unix epoch.
    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Returns the timestamp as milliseconds since Unix epoch.
    pub fn as_millis(&self) -> i64 {
        self.0
    }
}

impl Default for WallClockTimestamp {
    fn default() -> Self {
        Self::now()
    }
}

/// Unix timestamp in milliseconds (legacy alias for `WallClockTimestamp`).
///
/// Retained for backward compatibility. Prefer `WallClockTimestamp` for new code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Creates a new timestamp with the current time.
    pub fn now() -> Self {
        Self(chrono::Utc::now().timestamp_millis())
    }

    /// Creates a timestamp from milliseconds since Unix epoch.
    pub fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Returns the timestamp as milliseconds since Unix epoch.
    pub fn as_millis(&self) -> i64 {
        self.0
    }

    /// Returns the elapsed time since this timestamp in milliseconds.
    ///
    /// # Safety Contract
    ///
    /// This is only meaningful for timestamps created on the **same device**.
    /// Using this on a timestamp received from another device will produce
    /// incorrect results due to clock skew. Prefer `LocalInstant` for
    /// single-device elapsed time measurements.
    #[deprecated(note = "Use LocalInstant for elapsed time measurements")]
    pub fn elapsed_millis(&self) -> i64 {
        Self::now().0 - self.0
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

/// Lamport logical clock for causal message ordering.
///
/// Provides a monotonically increasing counter that respects causality:
/// if message B was sent after receiving message A, then B's clock > A's clock.
///
/// Use this — not wall-clock timestamps — for sorting messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct LamportClock(u64);

impl<'de> serde::Deserialize<'de> for LamportClock {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = u64::deserialize(deserializer)?;
        Ok(Self(raw.min(LAMPORT_CLOCK_MAX)))
    }
}

/// Upper bound for accepted Lamport clock values.
/// Rejects absurdly large values from malicious or buggy peers.
pub const LAMPORT_CLOCK_MAX: u64 = u64::MAX / 2;

impl LamportClock {
    /// Creates a clock at the initial value (0).
    pub fn new() -> Self {
        Self(0)
    }

    /// Creates a clock from a raw value (for deserialization / migration).
    pub fn from_value(value: u64) -> Self {
        Self(value.min(LAMPORT_CLOCK_MAX))
    }

    /// Returns the raw counter value.
    pub fn value(&self) -> u64 {
        self.0
    }

    /// Advances the clock for a local send event and returns the new value.
    pub fn tick(&mut self) -> Self {
        self.0 = self.0.saturating_add(1);
        *self
    }

    /// Advances the clock after receiving a message with the given clock value.
    /// Sets local clock to `max(local, received) + 1`, clamping the received
    /// value to [`LAMPORT_CLOCK_MAX`] to prevent adversarial inflation.
    pub fn merge(&mut self, received: LamportClock) -> Self {
        let clamped = received.0.min(LAMPORT_CLOCK_MAX);
        self.0 = self.0.max(clamped).saturating_add(1);
        *self
    }
}

impl Default for LamportClock {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for LamportClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.0)
    }
}

/// Local-only monotonic instant for measuring elapsed time on this device.
///
/// Cannot be serialized — physically cannot cross a device boundary.
/// Use this instead of `Timestamp::elapsed_millis()` for single-device timing.
#[derive(Debug, Clone, Copy)]
pub struct LocalInstant(std::time::Instant);

impl LocalInstant {
    /// Captures the current monotonic instant.
    pub fn now() -> Self {
        Self(std::time::Instant::now())
    }

    /// Returns the elapsed time since this instant in milliseconds.
    pub fn elapsed_millis(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }

    /// Returns the underlying `std::time::Instant`.
    pub fn as_std(&self) -> std::time::Instant {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_id_chars_variant_per_rejection() {
        assert!(matches!(
            validate_id_chars("", "User ID"),
            Err(IdValidationError::Empty { .. })
        ));
        assert!(matches!(
            validate_id_chars(".", "User ID"),
            Err(IdValidationError::PathTraversal { .. })
        ));
        assert!(matches!(
            validate_id_chars("..", "User ID"),
            Err(IdValidationError::PathTraversal { .. })
        ));
        assert!(matches!(
            validate_id_chars("a\x00b", "User ID"),
            Err(IdValidationError::ControlChars { .. })
        ));
        assert!(matches!(
            validate_id_chars("a/b", "User ID"),
            Err(IdValidationError::InvalidChars { .. })
        ));
        assert!(matches!(
            validate_id_chars("a\\b", "User ID"),
            Err(IdValidationError::InvalidChars { .. })
        ));
        assert!(matches!(
            validate_id_chars("a:b", "User ID"),
            Err(IdValidationError::InvalidChars { .. })
        ));
        assert!(validate_id_chars("alice-123", "User ID").is_ok());
    }

    #[test]
    fn test_id_validation_error_display_includes_type_name() {
        let err = validate_id_chars("", "App ID").unwrap_err();
        assert_eq!(err.to_string(), "App ID cannot be empty");
    }

    #[test]
    fn test_user_and_app_id_reject_oversized() {
        // Boundary: exactly MAX_ID_LEN is accepted, one over is rejected.
        assert!(UserId::new("a".repeat(MAX_ID_LEN)).is_ok());
        assert!(AppId::new("a".repeat(MAX_ID_LEN)).is_ok());
        assert!(matches!(
            UserId::new("a".repeat(MAX_ID_LEN + 1)),
            Err(crate::Error::InvalidUserId(_))
        ));
        assert!(matches!(
            AppId::new("a".repeat(MAX_ID_LEN + 1)),
            Err(crate::Error::InvalidAppId(_))
        ));
    }

    #[test]
    fn test_user_id_deserialize_rejects_oversized() {
        // The wire path (custom Deserialize) is capped too, so a hostile
        // megabyte-scale `sender` cannot be decoded into a UserId at all.
        let oversized = format!("\"{}\"", "a".repeat(MAX_ID_LEN + 1));
        assert!(serde_json::from_str::<UserId>(&oversized).is_err());
        let ok = format!("\"{}\"", "a".repeat(MAX_ID_LEN));
        assert!(serde_json::from_str::<UserId>(&ok).is_ok());
    }

    #[test]
    fn test_user_id_creation() {
        let user_id = UserId::new("user123").unwrap();
        assert_eq!(user_id.as_str(), "user123");

        let empty_result = UserId::new("");
        assert!(empty_result.is_err());
    }

    #[test]
    fn test_user_id_rejects_storage_hostile_chars() {
        assert!(UserId::new("user/evil").is_err());
        assert!(UserId::new("user\\evil").is_err());
        assert!(UserId::new("user:evil").is_err());
        assert!(UserId::new("user\0evil").is_err());
        assert!(UserId::new(".").is_err());
        assert!(UserId::new("..").is_err());
        // All ASCII control characters should be rejected
        assert!(UserId::new("user\nevil").is_err());
        assert!(UserId::new("user\revil").is_err());
        assert!(UserId::new("user\tevil").is_err());
        assert!(UserId::new("user\x7Fevil").is_err()); // DEL
                                                       // Valid characters should still work
        assert!(UserId::new("user-123_test.name@domain").is_ok());
    }

    #[test]
    fn test_user_id_deserialize_rejects_hostile_chars() {
        let result: Result<UserId, _> = serde_json::from_str(r#""evil/path""#);
        assert!(
            result.is_err(),
            "Deserialization should reject '/' in UserId"
        );

        let result: Result<UserId, _> = serde_json::from_str(r#""..""#);
        assert!(
            result.is_err(),
            "Deserialization should reject '..' as UserId"
        );

        let result: Result<UserId, _> = serde_json::from_str(r#""valid-user""#);
        assert!(result.is_ok(), "Deserialization should accept valid UserId");
    }

    #[test]
    fn test_app_id_creation() {
        let app_id = AppId::new("my-app").unwrap();
        assert_eq!(app_id.as_str(), "my-app");

        let empty_result = AppId::new("");
        assert!(empty_result.is_err());
    }

    #[test]
    fn test_app_id_rejects_storage_hostile_chars() {
        assert!(AppId::new("app/evil").is_err());
        assert!(AppId::new("app\\evil").is_err());
        assert!(AppId::new("app:evil").is_err());
        assert!(AppId::new("app\0evil").is_err());
        assert!(AppId::new(".").is_err());
        assert!(AppId::new("..").is_err());
        // ASCII control characters
        assert!(AppId::new("app\nevil").is_err());
        assert!(AppId::new("app\x1Bevil").is_err()); // ESC
        assert!(AppId::new("my-app_v2.0").is_ok());
    }

    #[test]
    fn test_app_id_deserialize_rejects_hostile_chars() {
        let result: Result<AppId, _> = serde_json::from_str(r#""app/evil""#);
        assert!(
            result.is_err(),
            "Deserialization should reject '/' in AppId"
        );

        let result: Result<AppId, _> = serde_json::from_str(r#""..""#);
        assert!(
            result.is_err(),
            "Deserialization should reject '..' as AppId"
        );

        let result: Result<AppId, _> = serde_json::from_str(r#""valid-app""#);
        assert!(result.is_ok(), "Deserialization should accept valid AppId");
    }

    #[test]
    fn test_ttl_operations() {
        let ttl = TTL::new(5).unwrap();
        assert_eq!(ttl.value(), 5);
        assert!(!ttl.is_exhausted());

        let ttl_dec = ttl.decrement().unwrap();
        assert_eq!(ttl_dec.value(), 4);

        let ttl_one = TTL::new(1).unwrap();
        assert!(ttl_one.is_exhausted());
        assert_eq!(ttl_one.decrement(), None);

        let ttl_zero = TTL::new(0);
        assert!(ttl_zero.is_err());
    }

    #[test]
    fn test_hop_count_operations() {
        let hop = HopCount::new();
        assert_eq!(hop.value(), 0);

        let hop_inc = hop.increment().unwrap();
        assert_eq!(hop_inc.value(), 1);
    }

    #[test]
    fn test_timestamp_operations() {
        let ts1 = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let ts2 = Timestamp::now();

        assert!(ts2.as_millis() > ts1.as_millis());
        #[allow(deprecated)]
        {
            assert!(ts1.elapsed_millis() >= 10);
        }
    }

    #[test]
    fn test_ttl_default() {
        let ttl = TTL::default();
        assert_eq!(ttl.value(), TTL::DEFAULT);
    }

    #[test]
    fn test_lamport_clock_tick() {
        let mut clock = LamportClock::new();
        assert_eq!(clock.value(), 0);

        let t1 = clock.tick();
        assert_eq!(t1.value(), 1);

        let t2 = clock.tick();
        assert_eq!(t2.value(), 2);
    }

    #[test]
    fn test_lamport_clock_merge() {
        let mut local = LamportClock::from_value(5);
        let received = LamportClock::from_value(10);

        let merged = local.merge(received);
        assert_eq!(merged.value(), 11);
        assert_eq!(local.value(), 11);
    }

    #[test]
    fn test_lamport_clock_merge_local_higher() {
        let mut local = LamportClock::from_value(20);
        let received = LamportClock::from_value(5);

        let merged = local.merge(received);
        assert_eq!(merged.value(), 21);
    }

    #[test]
    fn test_lamport_clock_ordering() {
        let a = LamportClock::from_value(3);
        let b = LamportClock::from_value(7);
        assert!(a < b);
    }

    #[test]
    fn test_lamport_clock_saturating_tick() {
        let mut clock = LamportClock::from_value(LAMPORT_CLOCK_MAX);
        let ticked = clock.tick();
        assert_eq!(ticked.value(), LAMPORT_CLOCK_MAX.saturating_add(1));
        // Doesn't panic or wrap
    }

    #[test]
    fn test_lamport_clock_merge_clamps_adversarial_value() {
        let mut local = LamportClock::from_value(5);
        let adversarial = LamportClock(u64::MAX); // bypass from_value to simulate wire data
        let merged = local.merge(adversarial);
        // Should clamp to LAMPORT_CLOCK_MAX + 1, not overflow
        assert_eq!(merged.value(), LAMPORT_CLOCK_MAX.saturating_add(1));
    }

    #[test]
    fn test_lamport_clock_from_value_clamps() {
        let clock = LamportClock::from_value(u64::MAX);
        assert_eq!(clock.value(), LAMPORT_CLOCK_MAX);
    }

    #[test]
    fn test_wall_clock_timestamp() {
        let ts = WallClockTimestamp::now();
        assert!(ts.as_millis() > 0);

        let from = WallClockTimestamp::from_millis(1000);
        assert_eq!(from.as_millis(), 1000);
    }

    #[test]
    fn test_local_instant_elapsed() {
        let instant = LocalInstant::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(instant.elapsed_millis() >= 10);
    }

    #[test]
    fn test_lamport_clock_serde_roundtrip() {
        let clock = LamportClock::from_value(42);
        let json = serde_json::to_string(&clock).unwrap();
        let deserialized: LamportClock = serde_json::from_str(&json).unwrap();
        assert_eq!(clock, deserialized);
    }

    #[test]
    fn test_lamport_clock_deserialize_default() {
        // Simulates receiving a legacy message without a lamport_clock field
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct LegacyMsg {
            content: String,
            #[serde(default)]
            lamport_clock: LamportClock,
        }
        let json = r#"{"content":"hello"}"#;
        let msg: LegacyMsg = serde_json::from_str(json).unwrap();
        assert_eq!(msg.lamport_clock.value(), 0);
    }

    #[test]
    fn test_lamport_clock_merge_with_zero() {
        // Merging with a legacy (zero) clock should still advance by 1
        let mut local = LamportClock::from_value(10);
        let legacy = LamportClock::new(); // value 0
        let merged = local.merge(legacy);
        // max(10, 0) + 1 = 11
        assert_eq!(merged.value(), 11);
    }

    #[test]
    fn test_lamport_clock_consecutive_merges_advance() {
        let mut clock = LamportClock::new();
        // Simulates receiving multiple messages with the same clock value
        let peer_clock = LamportClock::from_value(5);

        let m1 = clock.merge(peer_clock);
        assert_eq!(m1.value(), 6); // max(0, 5) + 1

        let m2 = clock.merge(peer_clock);
        assert_eq!(m2.value(), 7); // max(6, 5) + 1

        let m3 = clock.merge(peer_clock);
        assert_eq!(m3.value(), 8); // max(7, 5) + 1
    }

    #[test]
    fn test_lamport_clock_display() {
        let clock = LamportClock::from_value(42);
        assert_eq!(format!("{}", clock), "L42");
    }
}
