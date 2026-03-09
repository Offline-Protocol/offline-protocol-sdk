//! Service discovery types for the Offline Protocol SDK.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

/// Unique identifier for a service.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ServiceId(String);

impl TryFrom<String> for ServiceId {
    type Error = Error;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        ServiceId::new(s)
    }
}

impl<'de> Deserialize<'de> for ServiceId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        ServiceId::new(s).map_err(serde::de::Error::custom)
    }
}

impl ServiceId {
    /// Creates a new ServiceId, validating that it is non-empty.
    pub fn new(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        if id.is_empty() {
            return Err(Error::InvalidServiceId("service ID cannot be empty".into()));
        }
        Ok(Self(id))
    }

    /// Returns the service ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServiceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Describes a service that a node offers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDescriptor {
    /// The service identifier.
    pub service_id: ServiceId,
    /// Version of the service (e.g. "1.0").
    pub version: String,
    /// Arbitrary key-value capabilities advertised by this service.
    pub capabilities: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_id_valid() {
        let id = ServiceId::new("my.service").unwrap();
        assert_eq!(id.as_str(), "my.service");
        assert_eq!(id.to_string(), "my.service");
    }

    #[test]
    fn test_service_id_empty_rejected() {
        let err = ServiceId::new("").unwrap_err();
        assert!(matches!(err, Error::InvalidServiceId(_)));
    }

    #[test]
    fn test_service_id_serde_roundtrip() {
        let id = ServiceId::new("weather.v1").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        let parsed: ServiceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_service_id_empty_deserialization_rejected() {
        let result: std::result::Result<ServiceId, _> = serde_json::from_str("\"\"");
        assert!(result.is_err());
    }

    #[test]
    fn test_service_descriptor_serde_roundtrip() {
        let mut caps = HashMap::new();
        caps.insert("format".to_string(), "json".to_string());
        let desc = ServiceDescriptor {
            service_id: ServiceId::new("echo").unwrap(),
            version: "1.0".to_string(),
            capabilities: caps,
        };
        let json = serde_json::to_string(&desc).unwrap();
        let parsed: ServiceDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.service_id.as_str(), "echo");
        assert_eq!(parsed.version, "1.0");
        assert_eq!(parsed.capabilities.get("format").unwrap(), "json");
    }
}
