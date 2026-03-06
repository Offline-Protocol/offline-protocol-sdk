#![deny(unsafe_code)]

pub mod error;
pub mod events;
pub mod payloads;
pub mod services;

pub use error::ServiceError;
pub use events::ServiceEvent;
pub use payloads::{
    ServiceDiscoveryQueryPayload, ServiceDiscoveryResponsePayload, ServiceRequestPayload,
    ServiceResponsePayload, DISCOVERY_GOSSIP_MAX_FANOUT, DISCOVERY_QUERY_DEDUP_TTL_SECS,
    DISCOVERY_QUERY_DEFAULT_MAX_HOPS, DISCOVERY_QUERY_MAX_DEDUP_ENTRIES, SVC_DISCOVER_QUERY,
    SVC_DISCOVER_RESPONSE, SVC_MESSAGE_PREFIX, SVC_REQUEST, SVC_RESPONSE,
};
pub use services::{
    DiscoverResult, MeshServices, SendRequestResult, SendResponseResult, ServiceAction,
};
