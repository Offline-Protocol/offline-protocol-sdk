use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// TTL for deduplicating service discovery queries (seconds).
pub const DISCOVERY_QUERY_DEDUP_TTL_SECS: u64 = 60;
/// Default maximum hops for service discovery query gossip forwarding.
pub const DISCOVERY_QUERY_DEFAULT_MAX_HOPS: u8 = 10;
/// Maximum number of peers to forward a discovery query to per hop.
pub const DISCOVERY_GOSSIP_MAX_FANOUT: usize = 5;
/// Maximum number of peers the originator broadcasts an initial discovery query to.
pub const DISCOVERY_INITIAL_BROADCAST_MAX: usize = 20;
/// Maximum number of seen discovery query IDs to track for dedup.
pub const DISCOVERY_QUERY_MAX_DEDUP_ENTRIES: usize = 10_000;

/// Maximum allowed size (bytes) for the raw payload string after stripping the prefix.
pub const MAX_SERVICE_PAYLOAD_SIZE: usize = 131_072; // 128 KB
/// Maximum allowed size (bytes) for a service request/response body field.
pub const MAX_SERVICE_BODY_SIZE: usize = 65_536; // 64 KB
/// Maximum allowed length for a service request method field.
pub const MAX_SERVICE_METHOD_LEN: usize = 256;

/// Valid values for service response status.
pub const VALID_SERVICE_STATUSES: &[&str] = &["ok", "not_found", "error"];

/// Common prefix shared by all service message types.
pub const SVC_MESSAGE_PREFIX: &str = "__SVC_";
/// Prefix for service discovery query.
pub const SVC_DISCOVER_QUERY: &str = "__SVC_DISC_Q__";
/// Prefix for service discovery response.
pub const SVC_DISCOVER_RESPONSE: &str = "__SVC_DISC_R__";
/// Prefix for service request.
pub const SVC_REQUEST: &str = "__SVC_REQ__";
/// Prefix for service response.
pub const SVC_RESPONSE: &str = "__SVC_RESP__";

fn default_discovery_max_hops() -> u8 {
    DISCOVERY_QUERY_DEFAULT_MAX_HOPS
}

/// Wire-format payload for a service discovery query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceDiscoveryQueryPayload {
    pub query_id: String,
    pub originator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(default = "default_discovery_max_hops")]
    pub remaining_hops: u8,
}

/// Wire-format payload for a service discovery response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceDiscoveryResponsePayload {
    pub query_id: String,
    pub service_id: String,
    pub version: String,
    pub provider_peer_id: String,
    pub capabilities: HashMap<String, String>,
    pub hop_count: u8,
}

/// Wire-format payload for a service request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceRequestPayload {
    pub request_id: String,
    pub service_id: String,
    pub method: String,
    pub body: String,
}

/// Wire-format payload for a service response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServiceResponsePayload {
    pub request_id: String,
    pub service_id: String,
    pub status: String,
    pub body: String,
}
