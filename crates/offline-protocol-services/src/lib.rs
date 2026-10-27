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
/// Subscriptions: a consumer's lease on a provider's topic, and the updates
/// sent to it (ADR 0035).
pub mod subscriptions;

pub use error::ServiceError;
pub use events::ServiceEvent;
pub use payloads::{
    is_subscription_frame, SubscribeStatus, SubscriptionMode, DEFAULT_SUBSCRIPTION_LEASE_MS,
    MAX_CONSUMER_SUBSCRIPTIONS, MAX_LATEST_BYTES, MAX_LATEST_KEYS, MAX_LOG_BYTES, MAX_LOG_ENTRIES,
    MAX_PROVIDER_SUBSCRIPTIONS, MAX_SUBSCRIPTIONS_PER_PEER, MAX_SUBSCRIPTION_KEY_LEN,
    MAX_SUBSCRIPTION_LEASE_MS, MAX_SUBSCRIPTION_PARAMS_SIZE, MAX_SUBSCRIPTION_REASON_LEN,
    MAX_SUBSCRIPTION_TOPIC_LEN, MIN_SUBSCRIPTION_LEASE_MS, SVC_SUBSCRIBE, SVC_SUBSCRIBE_REPLY,
    SVC_SUBSCRIPTION_PREFIXES, SVC_UNSUBSCRIBE, SVC_UPDATE,
};
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
pub use subscriptions::{
    Admission, Provider, PublishOutcome, RulesFallback, SubscriberInfo, SubscriptionDelivery,
    SubscriptionFrame, SubscriptionOutput, SubscriptionPolicy, SubscriptionRole, SubscriptionState,
};
