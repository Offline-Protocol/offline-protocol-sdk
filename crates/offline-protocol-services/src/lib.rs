#![deny(unsafe_code)]
#![warn(missing_docs)]

//! Mesh service discovery and request/response for the Offline Protocol SDK.

/// Error types for mesh service operations.
pub mod error;
/// Event types emitted by mesh service operations.
pub mod events;
pub(crate) mod payloads;
/// Core service registry and message handling logic.
pub mod services;

pub use error::ServiceError;
pub use events::ServiceEvent;
pub use payloads::{
    DISCOVERY_GOSSIP_MAX_FANOUT, DISCOVERY_INITIAL_BROADCAST_MAX, DISCOVERY_QUERY_DEDUP_TTL_SECS,
    DISCOVERY_QUERY_DEFAULT_MAX_HOPS, DISCOVERY_QUERY_MAX_DEDUP_ENTRIES, MAX_SERVICE_BODY_SIZE,
    MAX_SERVICE_METHOD_LEN, MAX_SERVICE_PAYLOAD_SIZE, SVC_DISCOVER_QUERY, SVC_DISCOVER_RESPONSE,
    SVC_MESSAGE_PREFIX, SVC_REQUEST, SVC_RESPONSE, VALID_SERVICE_STATUSES,
};
pub use services::{
    DiscoverResult, MeshServices, OutboundMessage, SendRequestResult, SendResponseResult,
    ServiceAction,
};
