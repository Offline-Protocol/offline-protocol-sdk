//! Core protocol types.

use serde::{Deserialize, Serialize};
use std::fmt;

/// User identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        if id_str.is_empty() {
            return Err(crate::Error::InvalidUserId(
                "User ID cannot be empty".into(),
            ));
        }
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

/// Application identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        if id_str.is_empty() {
            return Err(crate::Error::InvalidAppId("App ID cannot be empty".into()));
        }
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

/// Unix timestamp in milliseconds.
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
    pub fn elapsed_millis(&self) -> i64 {
        Self::now().0 - self.0
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_id_creation() {
        let user_id = UserId::new("user123").unwrap();
        assert_eq!(user_id.as_str(), "user123");

        let empty_result = UserId::new("");
        assert!(empty_result.is_err());
    }

    #[test]
    fn test_app_id_creation() {
        let app_id = AppId::new("my-app").unwrap();
        assert_eq!(app_id.as_str(), "my-app");

        let empty_result = AppId::new("");
        assert!(empty_result.is_err());
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
        assert!(ts1.elapsed_millis() >= 10);
    }

    #[test]
    fn test_ttl_default() {
        let ttl = TTL::default();
        assert_eq!(ttl.value(), TTL::DEFAULT);
    }
}
