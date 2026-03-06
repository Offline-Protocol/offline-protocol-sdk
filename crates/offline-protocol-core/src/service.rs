//! Service discovery types for the Offline Protocol SDK.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

/// Unique identifier for a service.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceId(String);

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

/// A discovered service record received from the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRecord {
    /// The service identifier.
    pub service_id: ServiceId,
    /// Version of the service.
    pub version: String,
    /// Peer user ID of the provider.
    pub provider: String,
    /// Arbitrary key-value capabilities.
    pub capabilities: HashMap<String, String>,
    /// Number of hops the discovery response traversed.
    pub hop_count: u8,
}
