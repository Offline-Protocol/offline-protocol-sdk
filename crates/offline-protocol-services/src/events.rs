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
}
