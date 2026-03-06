use std::collections::HashMap;

/// Events emitted by mesh service operations.
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    /// A service was discovered on the mesh in response to a discovery query.
    ServiceDiscovered {
        query_id: String,
        service_id: String,
        version: String,
        provider_peer_id: String,
        capabilities: HashMap<String, String>,
        hop_count: u8,
    },

    /// A service request was received from another peer.
    ServiceRequestReceived {
        request_id: String,
        service_id: String,
        method: String,
        body: String,
        sender: String,
    },

    /// A response to a service request was received.
    ServiceResponseReceived {
        request_id: String,
        service_id: String,
        status: String,
        body: String,
        provider_peer_id: String,
    },
}
