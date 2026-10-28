use std::collections::HashMap;

/// Events emitted by mesh service operations.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceEvent {
    /// A service was discovered on the mesh in response to a discovery query.
    ServiceDiscovered {
        /// The query ID this response corresponds to.
        query_id: String,
        /// The discovered service's identifier.
        service_id: String,
        /// The discovered service's version.
        version: String,
        /// The peer providing this service.
        provider_peer_id: String,
        /// Capabilities advertised by the service.
        capabilities: HashMap<String, String>,
        /// Number of hops the query traveled to reach the provider.
        hop_count: u8,
    },

    /// A service request was received from another peer.
    ServiceRequestReceived {
        /// Unique request identifier for correlating the response.
        request_id: String,
        /// The requested service's identifier.
        service_id: String,
        /// The method being invoked.
        method: String,
        /// The request body.
        body: String,
        /// The peer that sent the request.
        sender: String,
    },

    /// A response to a service request was received.
    ServiceResponseReceived {
        /// The request ID this response corresponds to.
        request_id: String,
        /// The service's identifier.
        service_id: String,
        /// Response status (`"ok"`, `"not_found"`, or `"error"`).
        status: String,
        /// The response body.
        body: String,
        /// The peer that provided the response.
        provider_peer_id: String,
    },

    /// Provider side: a subscription waits for the application's decision
    /// (`accept_service_subscription` or `reject_service_subscription`).
    ServiceSubscribeReceived {
        /// The subscription's identifier.
        sub_id: String,
        /// The subscribed service.
        service_id: String,
        /// The subscribed topic.
        topic: String,
        /// `"latest"` or `"log"`.
        mode: String,
        /// Application-defined parameters from the subscriber.
        params: String,
        /// The MLS-authenticated subscriber.
        sender: String,
        /// A renewal of a subscription this provider no longer held (it
        /// restarted, or the lease lapsed), rather than a new one.
        renewal: bool,
    },

    /// Either side: a subscription changed state.
    ServiceSubscriptionChanged {
        /// The subscription's identifier.
        sub_id: String,
        /// `"consumer"` or `"provider"`: which side of it this device is.
        role: String,
        /// The subscribed service.
        service_id: String,
        /// The subscribed topic.
        topic: String,
        /// The other side: the provider for a consumer, the subscriber for a
        /// provider.
        peer: String,
        /// `"pending"`, `"active"`, `"paused"` or `"ended"`.
        state: String,
        /// Why, from a fixed vocabulary (`accepted`, `resumed`,
        /// `stream_reset`, `history_lost`, `expired`, `unsubscribed`, ...).
        reason: String,
    },

    /// Consumer side: an update on a subscribed topic.
    ServiceUpdateReceived {
        /// The subscription's identifier.
        sub_id: String,
        /// The subscribed service.
        service_id: String,
        /// The subscribed topic.
        topic: String,
        /// The key, on a `latest` topic.
        key: Option<String>,
        /// The update's position in the provider's stream.
        seq: u64,
        /// The update's body.
        body: String,
        /// Sent to catch this device up (a `latest` key's current value or a
        /// `log` checkpoint) rather than as a fresh publish.
        snapshot: bool,
        /// The provider, MLS-authenticated.
        provider_peer_id: String,
    },
}
