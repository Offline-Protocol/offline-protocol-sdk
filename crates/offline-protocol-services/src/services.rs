use crate::error::ServiceError;
use crate::events::ServiceEvent;
use crate::payloads::{
    ServiceDiscoveryQueryPayload, ServiceDiscoveryResponsePayload, ServiceRequestPayload,
    ServiceResponsePayload, DISCOVERY_GOSSIP_MAX_FANOUT, DISCOVERY_INITIAL_BROADCAST_MAX,
    DISCOVERY_QUERY_DEDUP_TTL_SECS, DISCOVERY_QUERY_DEFAULT_MAX_HOPS,
    DISCOVERY_QUERY_MAX_DEDUP_ENTRIES, MAX_SERVICE_BODY_SIZE, MAX_SERVICE_METHOD_LEN,
    MAX_SERVICE_PAYLOAD_SIZE, SVC_DISCOVER_QUERY, SVC_DISCOVER_RESPONSE, SVC_REQUEST, SVC_RESPONSE,
    VALID_SERVICE_STATUSES,
};
use offline_protocol_core::{MessagePriority, ServiceDescriptor};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Result of initiating a service discovery broadcast.
#[must_use]
pub struct DiscoverResult {
    /// Unique query identifier for correlating responses.
    pub query_id: String,
    /// Messages to send: (recipient, content, priority).
    pub messages: Vec<(String, String, MessagePriority)>,
}

/// Result of sending a service request.
#[must_use]
pub struct SendRequestResult {
    /// Unique request identifier for correlating the response.
    pub request_id: String,
    /// Message to send: (recipient, content, priority).
    pub message: (String, String, MessagePriority),
}

/// Result of responding to a service request.
#[must_use]
pub struct SendResponseResult {
    /// Message to send: (recipient, content, priority).
    pub message: (String, String, MessagePriority),
}

/// Action returned from handling an incoming service message.
pub enum ServiceAction {
    /// The message was not a service message; caller should continue processing.
    NotHandled,
    /// The message was consumed as a service message.
    Consumed {
        /// Messages to send: `(recipient, content, priority)`.
        messages_to_send: Vec<(String, String, MessagePriority)>,
        /// Events to emit to the application.
        events_to_emit: Vec<ServiceEvent>,
    },
}

/// Mesh service registry and message handler.
///
/// Manages local service registration, discovery query generation and handling,
/// and service request/response routing. All methods return **actions** (messages
/// to send, events to emit) rather than performing I/O directly.
pub struct MeshServices {
    local_services: HashMap<String, ServiceDescriptor>,
    seen_discovery_queries: HashMap<String, Instant>,
    /// Insertion-order index for O(1) oldest-entry eviction.
    seen_discovery_order: VecDeque<String>,
}

impl MeshServices {
    /// Creates a new empty service registry.
    pub fn new() -> Self {
        Self {
            local_services: HashMap::new(),
            seen_discovery_queries: HashMap::new(),
            seen_discovery_order: VecDeque::new(),
        }
    }

    /// Registers a local service.
    pub fn register_service(&mut self, descriptor: ServiceDescriptor) -> Result<(), ServiceError> {
        let key = descriptor.service_id.as_str().to_string();
        self.local_services.insert(key, descriptor);
        Ok(())
    }

    /// Unregisters a local service. Returns `true` if it was found and removed.
    pub fn unregister_service(&mut self, service_id: &str) -> Result<bool, ServiceError> {
        Ok(self.local_services.remove(service_id).is_some())
    }

    /// Returns `true` if a service with the given ID is registered locally.
    pub fn has_service(&self, service_id: &str) -> bool {
        self.local_services.contains_key(service_id)
    }

    /// Generates a discovery broadcast to known peers, capped at
    /// [`DISCOVERY_INITIAL_BROADCAST_MAX`] recipients.
    pub fn discover_services(
        &mut self,
        user_id: &str,
        known_peers: &[String],
        service_id: Option<&str>,
    ) -> Result<DiscoverResult, ServiceError> {
        let query_id = uuid::Uuid::new_v4().to_string();

        let payload = ServiceDiscoveryQueryPayload {
            query_id: query_id.clone(),
            originator: user_id.to_string(),
            service_id: service_id.map(|s| s.to_string()),
            remaining_hops: DISCOVERY_QUERY_DEFAULT_MAX_HOPS,
        };
        let serialized = serde_json::to_string(&payload)
            .map_err(|e| ServiceError::Serialization(e.to_string()))?;
        let content = format!("{}{}", SVC_DISCOVER_QUERY, serialized);

        self.record_seen_query(query_id.clone());

        // Apply fanout limit to the initial broadcast to bound message generation.
        let selected_peers = select_fanout_peers(
            known_peers,
            DISCOVERY_INITIAL_BROADCAST_MAX,
            &query_id,
            user_id,
        );

        let messages: Vec<(String, String, MessagePriority)> = selected_peers
            .into_iter()
            .map(|peer| (peer, content.clone(), MessagePriority::Medium))
            .collect();

        info!(query_id = %query_id, service_id = ?service_id, peer_count = messages.len(), "Broadcast service discovery query");
        Ok(DiscoverResult { query_id, messages })
    }

    /// Generates a service request message to a specific provider peer.
    pub fn send_service_request(
        &self,
        provider: &str,
        service_id: &str,
        method: &str,
        body: &str,
    ) -> Result<SendRequestResult, ServiceError> {
        if body.len() > MAX_SERVICE_BODY_SIZE {
            return Err(ServiceError::PayloadTooLarge(format!(
                "request body size {} exceeds maximum {}",
                body.len(),
                MAX_SERVICE_BODY_SIZE
            )));
        }
        if method.len() > MAX_SERVICE_METHOD_LEN {
            return Err(ServiceError::PayloadTooLarge(format!(
                "method length {} exceeds maximum {}",
                method.len(),
                MAX_SERVICE_METHOD_LEN
            )));
        }

        let request_id = uuid::Uuid::new_v4().to_string();

        let payload = ServiceRequestPayload {
            request_id: request_id.clone(),
            service_id: service_id.to_string(),
            method: method.to_string(),
            body: body.to_string(),
        };
        let serialized = serde_json::to_string(&payload)
            .map_err(|e| ServiceError::Serialization(e.to_string()))?;
        let content = format!("{}{}", SVC_REQUEST, serialized);

        info!(request_id = %request_id, provider = %provider, service_id = %service_id, method = %method, "Sent service request");
        Ok(SendRequestResult {
            request_id,
            message: (provider.to_string(), content, MessagePriority::High),
        })
    }

    /// Generates a service response message.
    pub fn respond_to_service_request(
        &self,
        request_id: &str,
        requester: &str,
        service_id: &str,
        status: &str,
        body: &str,
    ) -> Result<SendResponseResult, ServiceError> {
        if !VALID_SERVICE_STATUSES.contains(&status) {
            return Err(ServiceError::InvalidStatus(status.to_string()));
        }
        if body.len() > MAX_SERVICE_BODY_SIZE {
            return Err(ServiceError::PayloadTooLarge(format!(
                "response body size {} exceeds maximum {}",
                body.len(),
                MAX_SERVICE_BODY_SIZE
            )));
        }

        let payload = ServiceResponsePayload {
            request_id: request_id.to_string(),
            service_id: service_id.to_string(),
            status: status.to_string(),
            body: body.to_string(),
        };
        let serialized = serde_json::to_string(&payload)
            .map_err(|e| ServiceError::Serialization(e.to_string()))?;
        let content = format!("{}{}", SVC_RESPONSE, serialized);

        info!(request_id = %request_id, requester = %requester, status = %status, "Sent service response");
        Ok(SendResponseResult {
            message: (requester.to_string(), content, MessagePriority::High),
        })
    }

    /// Handles an incoming message, returning a `ServiceAction`.
    ///
    /// `hop_count` is the hop count from the incoming message (used in discovery responses).
    /// `known_peers` should exclude the sender.
    pub fn handle_incoming_message(
        &mut self,
        content: &str,
        sender: &str,
        hop_count: u8,
        user_id: &str,
        known_peers: &[String],
    ) -> ServiceAction {
        if let Some(action) =
            self.try_handle_discover_query(content, sender, hop_count, user_id, known_peers)
        {
            return action;
        }
        if let Some(action) = self.try_handle_discover_response(content, sender) {
            return action;
        }
        if let Some(action) = self.try_handle_request(content, sender) {
            return action;
        }
        if let Some(action) = self.try_handle_response(content, sender) {
            return action;
        }
        ServiceAction::NotHandled
    }

    /// Removes expired discovery query dedup entries.
    pub fn cleanup_expired(&mut self) {
        let cutoff = Instant::now() - Duration::from_secs(DISCOVERY_QUERY_DEDUP_TTL_SECS);
        self.seen_discovery_queries.retain(|_, ts| *ts > cutoff);
        self.seen_discovery_order
            .retain(|id| self.seen_discovery_queries.contains_key(id));
    }

    /// Provides read access to `seen_discovery_queries` for testing / inspection.
    pub fn seen_discovery_queries(&self) -> &HashMap<String, Instant> {
        &self.seen_discovery_queries
    }

    /// Inserts a query ID into the dedup map, evicting the oldest entry if at capacity.
    fn record_seen_query(&mut self, query_id: String) {
        if self.seen_discovery_queries.len() >= DISCOVERY_QUERY_MAX_DEDUP_ENTRIES {
            // Evict the oldest entry using insertion-order index (O(1)).
            if let Some(oldest) = self.seen_discovery_order.pop_front() {
                self.seen_discovery_queries.remove(&oldest);
            }
        }
        self.seen_discovery_queries
            .insert(query_id.clone(), Instant::now());
        self.seen_discovery_order.push_back(query_id);
    }

    // --- private handlers ---

    fn try_handle_discover_query(
        &mut self,
        content: &str,
        sender: &str,
        hop_count: u8,
        user_id: &str,
        known_peers: &[String],
    ) -> Option<ServiceAction> {
        let data = content.strip_prefix(SVC_DISCOVER_QUERY)?;

        if data.len() > MAX_SERVICE_PAYLOAD_SIZE {
            warn!(sender = %sender, size = data.len(), "Discovery query payload too large, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        let payload: ServiceDiscoveryQueryPayload = match serde_json::from_str(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse service discovery query payload");
                return Some(ServiceAction::Consumed {
                    messages_to_send: vec![],
                    events_to_emit: vec![],
                });
            }
        };

        // Dedup
        if self.seen_discovery_queries.contains_key(&payload.query_id) {
            debug!(query_id = %payload.query_id, "Duplicate discovery query, skipping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }
        self.record_seen_query(payload.query_id.clone());

        let mut messages = Vec::new();

        // Check local services for matches
        let matches: Vec<_> = self
            .local_services
            .values()
            .filter(|svc| {
                payload
                    .service_id
                    .as_ref()
                    .is_none_or(|q| q == svc.service_id.as_str())
            })
            .cloned()
            .collect();

        // Send discovery responses back to the *sender* (not the originator) to
        // prevent a spoofed originator field from leaking our service list to an
        // arbitrary peer. The sender relays the response back along the path.
        for svc in &matches {
            let response = ServiceDiscoveryResponsePayload {
                query_id: payload.query_id.clone(),
                service_id: svc.service_id.as_str().to_string(),
                version: svc.version.clone(),
                provider_peer_id: user_id.to_string(),
                capabilities: svc.capabilities.clone(),
                hop_count,
            };
            if let Ok(serialized) = serde_json::to_string(&response) {
                let resp_content = format!("{}{}", SVC_DISCOVER_RESPONSE, serialized);
                messages.push((sender.to_string(), resp_content, MessagePriority::Medium));
            }
        }

        // Forward query to other known peers (gossip) if hops remain
        let forwarded_count = if payload.remaining_hops > 0 {
            let mut fwd_payload = payload.clone();
            fwd_payload.remaining_hops -= 1;
            let fwd_content = match serde_json::to_string(&fwd_payload) {
                Ok(serialized) => format!("{}{}", SVC_DISCOVER_QUERY, serialized),
                Err(e) => {
                    warn!("Failed to re-serialize discovery query for forwarding: {e}");
                    return Some(ServiceAction::Consumed {
                        messages_to_send: messages,
                        events_to_emit: vec![],
                    });
                }
            };
            let eligible_peers: Vec<String> = known_peers
                .iter()
                .filter(|p| p.as_str() != sender && p.as_str() != payload.originator)
                .cloned()
                .collect();
            let forward_peers = select_fanout_peers(
                &eligible_peers,
                DISCOVERY_GOSSIP_MAX_FANOUT,
                &payload.query_id,
                user_id,
            );
            for peer in &forward_peers {
                messages.push((peer.clone(), fwd_content.clone(), MessagePriority::Medium));
            }
            forward_peers.len()
        } else {
            debug!(query_id = %payload.query_id, "Discovery query reached max hops, not forwarding");
            0
        };

        debug!(
            query_id = %payload.query_id,
            matches = matches.len(),
            forwarded_to = forwarded_count,
            "Processed service discovery query"
        );

        Some(ServiceAction::Consumed {
            messages_to_send: messages,
            events_to_emit: vec![],
        })
    }

    fn try_handle_discover_response(&self, content: &str, sender: &str) -> Option<ServiceAction> {
        let data = content.strip_prefix(SVC_DISCOVER_RESPONSE)?;

        if data.len() > MAX_SERVICE_PAYLOAD_SIZE {
            warn!(sender = %sender, size = data.len(), "Discovery response payload too large, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        let payload: ServiceDiscoveryResponsePayload = match serde_json::from_str(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse service discovery response payload");
                return Some(ServiceAction::Consumed {
                    messages_to_send: vec![],
                    events_to_emit: vec![],
                });
            }
        };

        info!(
            query_id = %payload.query_id,
            service_id = %payload.service_id,
            provider = %payload.provider_peer_id,
            "Service discovered"
        );

        Some(ServiceAction::Consumed {
            messages_to_send: vec![],
            events_to_emit: vec![ServiceEvent::ServiceDiscovered {
                query_id: payload.query_id,
                service_id: payload.service_id,
                version: payload.version,
                provider_peer_id: payload.provider_peer_id,
                capabilities: payload.capabilities,
                hop_count: payload.hop_count,
            }],
        })
    }

    fn try_handle_request(&self, content: &str, sender: &str) -> Option<ServiceAction> {
        let data = content.strip_prefix(SVC_REQUEST)?;

        if data.len() > MAX_SERVICE_PAYLOAD_SIZE {
            warn!(sender = %sender, size = data.len(), "Service request payload too large, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        let payload: ServiceRequestPayload = match serde_json::from_str(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse service request payload");
                return Some(ServiceAction::Consumed {
                    messages_to_send: vec![],
                    events_to_emit: vec![],
                });
            }
        };

        // Validate field sizes from untrusted input
        if payload.body.len() > MAX_SERVICE_BODY_SIZE {
            warn!(sender = %sender, size = payload.body.len(), "Service request body too large, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }
        if payload.method.len() > MAX_SERVICE_METHOD_LEN {
            warn!(sender = %sender, len = payload.method.len(), "Service request method too long, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        if !self.local_services.contains_key(&payload.service_id) {
            // Auto-respond with not_found
            let response = ServiceResponsePayload {
                request_id: payload.request_id.clone(),
                service_id: payload.service_id.clone(),
                status: "not_found".to_string(),
                body: String::new(),
            };
            let mut messages = Vec::new();
            if let Ok(serialized) = serde_json::to_string(&response) {
                let resp_content = format!("{}{}", SVC_RESPONSE, serialized);
                messages.push((sender.to_string(), resp_content, MessagePriority::High));
            }
            debug!(
                request_id = %payload.request_id,
                service_id = %payload.service_id,
                "Service not found, auto-responded not_found"
            );
            return Some(ServiceAction::Consumed {
                messages_to_send: messages,
                events_to_emit: vec![],
            });
        }

        info!(
            request_id = %payload.request_id,
            service_id = %payload.service_id,
            method = %payload.method,
            "Service request received"
        );

        Some(ServiceAction::Consumed {
            messages_to_send: vec![],
            events_to_emit: vec![ServiceEvent::ServiceRequestReceived {
                request_id: payload.request_id,
                service_id: payload.service_id,
                method: payload.method,
                body: payload.body,
                sender: sender.to_string(),
            }],
        })
    }

    fn try_handle_response(&self, content: &str, sender: &str) -> Option<ServiceAction> {
        let data = content.strip_prefix(SVC_RESPONSE)?;

        if data.len() > MAX_SERVICE_PAYLOAD_SIZE {
            warn!(sender = %sender, size = data.len(), "Service response payload too large, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        let payload: ServiceResponsePayload = match serde_json::from_str(data) {
            Ok(p) => p,
            Err(_) => {
                warn!(sender = %sender, "Failed to parse service response payload");
                return Some(ServiceAction::Consumed {
                    messages_to_send: vec![],
                    events_to_emit: vec![],
                });
            }
        };

        // Validate body size from untrusted input
        if payload.body.len() > MAX_SERVICE_BODY_SIZE {
            warn!(sender = %sender, size = payload.body.len(), "Service response body too large, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        // Validate status from untrusted input
        if !VALID_SERVICE_STATUSES.contains(&payload.status.as_str()) {
            warn!(sender = %sender, status = %payload.status, "Service response with invalid status, dropping");
            return Some(ServiceAction::Consumed {
                messages_to_send: vec![],
                events_to_emit: vec![],
            });
        }

        info!(
            request_id = %payload.request_id,
            service_id = %payload.service_id,
            status = %payload.status,
            "Service response received"
        );

        Some(ServiceAction::Consumed {
            messages_to_send: vec![],
            events_to_emit: vec![ServiceEvent::ServiceResponseReceived {
                request_id: payload.request_id,
                service_id: payload.service_id,
                status: payload.status,
                body: payload.body,
                provider_peer_id: sender.to_string(),
            }],
        })
    }
}

/// Selects up to `max` peers from `peers` using a deterministic pseudo-random
/// subset selection. When `peers.len() <= max`, all peers are returned.
///
/// The selection uses a simple hash-based seed derived from `query_id` and
/// `user_id` so that different nodes select different subsets for the same query,
/// improving mesh coverage. The distribution has some bias because we use a
/// lightweight hash rather than a full PRNG — this is intentional as uniform
/// randomness is not critical for gossip fanout and avoids adding a `rand`
/// dependency.
fn select_fanout_peers(peers: &[String], max: usize, query_id: &str, user_id: &str) -> Vec<String> {
    if peers.len() <= max {
        return peers.to_vec();
    }

    let seed = query_id
        .bytes()
        .chain(user_id.bytes())
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
        ^ peers.len() as u64;

    let mut indices: Vec<usize> = (0..peers.len()).collect();
    // Partial Fisher-Yates shuffle for the first `max` elements.
    for i in 0..max {
        let remaining = peers.len() - i;
        let j = i + (seed.wrapping_mul((i as u64).wrapping_add(7)) as usize % remaining);
        indices.swap(i, j);
    }

    indices[..max]
        .iter()
        .map(|&idx| peers[idx].clone())
        .collect()
}

impl Default for MeshServices {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::ServiceId;

    fn make_descriptor(id: &str) -> ServiceDescriptor {
        ServiceDescriptor {
            service_id: ServiceId::new(id).unwrap(),
            version: "1.0".to_string(),
            capabilities: HashMap::new(),
        }
    }

    #[test]
    fn test_register_and_unregister() {
        let mut svc = MeshServices::new();
        svc.register_service(make_descriptor("echo.v1")).unwrap();
        assert!(svc.has_service("echo.v1"));

        assert!(svc.unregister_service("echo.v1").unwrap());
        assert!(!svc.has_service("echo.v1"));

        assert!(!svc.unregister_service("echo.v1").unwrap());
    }

    #[test]
    fn test_discover_services_generates_messages() {
        let mut svc = MeshServices::new();
        let peers = vec!["alice".to_string(), "bob".to_string()];
        let result = svc.discover_services("user1", &peers, None).unwrap();
        assert!(!result.query_id.is_empty());
        assert_eq!(result.messages.len(), 2);
        assert!(svc.seen_discovery_queries().contains_key(&result.query_id));
    }

    #[test]
    fn test_discover_services_no_peers() {
        let mut svc = MeshServices::new();
        let result = svc
            .discover_services("user1", &[], Some("weather"))
            .unwrap();
        assert!(result.messages.is_empty());
    }

    #[test]
    fn test_discover_services_initial_broadcast_fanout_limit() {
        let mut svc = MeshServices::new();
        let peers: Vec<String> = (0..50).map(|i| format!("peer-{i}")).collect();
        let result = svc.discover_services("user1", &peers, None).unwrap();
        assert_eq!(result.messages.len(), DISCOVERY_INITIAL_BROADCAST_MAX);
    }

    #[test]
    fn test_send_service_request() {
        let svc = MeshServices::new();
        let result = svc
            .send_service_request("bob", "echo.v1", "ping", "{}")
            .unwrap();
        assert!(!result.request_id.is_empty());
        assert_eq!(result.message.0, "bob");
        assert!(result.message.1.starts_with(SVC_REQUEST));
    }

    #[test]
    fn test_send_service_request_body_too_large() {
        let svc = MeshServices::new();
        let large_body = "x".repeat(MAX_SERVICE_BODY_SIZE + 1);
        let result = svc.send_service_request("bob", "echo.v1", "ping", &large_body);
        assert!(matches!(result, Err(ServiceError::PayloadTooLarge(_))));
    }

    #[test]
    fn test_send_service_request_method_too_long() {
        let svc = MeshServices::new();
        let long_method = "m".repeat(MAX_SERVICE_METHOD_LEN + 1);
        let result = svc.send_service_request("bob", "echo.v1", &long_method, "{}");
        assert!(matches!(result, Err(ServiceError::PayloadTooLarge(_))));
    }

    #[test]
    fn test_respond_to_service_request() {
        let svc = MeshServices::new();
        let result = svc
            .respond_to_service_request("req-1", "alice", "echo.v1", "ok", "pong")
            .unwrap();
        assert_eq!(result.message.0, "alice");
        assert!(result.message.1.starts_with(SVC_RESPONSE));
    }

    #[test]
    fn test_respond_to_service_request_invalid_status() {
        let svc = MeshServices::new();
        let result = svc.respond_to_service_request("req-1", "alice", "echo.v1", "invalid", "");
        assert!(matches!(result, Err(ServiceError::InvalidStatus(_))));
    }

    #[test]
    fn test_respond_to_service_request_body_too_large() {
        let svc = MeshServices::new();
        let large_body = "x".repeat(MAX_SERVICE_BODY_SIZE + 1);
        let result = svc.respond_to_service_request("req-1", "alice", "echo.v1", "ok", &large_body);
        assert!(matches!(result, Err(ServiceError::PayloadTooLarge(_))));
    }

    #[test]
    fn test_respond_valid_statuses() {
        let svc = MeshServices::new();
        for status in VALID_SERVICE_STATUSES {
            let result =
                svc.respond_to_service_request("req-1", "alice", "echo.v1", status, "body");
            assert!(result.is_ok(), "status '{status}' should be valid");
        }
    }

    #[test]
    fn test_handle_discover_query_with_match() {
        let mut svc = MeshServices::new();
        svc.register_service(ServiceDescriptor {
            service_id: ServiceId::new("weather").unwrap(),
            version: "2.0".to_string(),
            capabilities: {
                let mut m = HashMap::new();
                m.insert("format".to_string(), "json".to_string());
                m
            },
        })
        .unwrap();

        let payload = ServiceDiscoveryQueryPayload {
            query_id: "q-001".to_string(),
            originator: "alice".to_string(),
            service_id: Some("weather".to_string()),
            remaining_hops: 5,
        };
        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::to_string(&payload).unwrap()
        );

        let peers = vec!["charlie".to_string()];
        let action = svc.handle_incoming_message(&content, "alice", 1, "user1", &peers);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                assert!(!messages_to_send.is_empty());
                assert!(events_to_emit.is_empty());
                // Response should go to the sender ("alice"), not the originator field
                assert!(messages_to_send
                    .iter()
                    .any(|(r, c, _)| r == "alice" && c.starts_with(SVC_DISCOVER_RESPONSE)));
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
        assert!(svc.seen_discovery_queries().contains_key("q-001"));
    }

    #[test]
    fn test_handle_discover_query_dedup() {
        let mut svc = MeshServices::new();

        let payload = ServiceDiscoveryQueryPayload {
            query_id: "q-dedup".to_string(),
            originator: "alice".to_string(),
            service_id: None,
            remaining_hops: 5,
        };
        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::to_string(&payload).unwrap()
        );

        // First time: processes
        let a1 = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        assert!(matches!(a1, ServiceAction::Consumed { .. }));

        // Second time: deduped
        let a2 = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        assert!(matches!(a2, ServiceAction::Consumed { .. }));
    }

    #[test]
    fn test_handle_discover_response() {
        let mut svc = MeshServices::new();

        let payload = ServiceDiscoveryResponsePayload {
            query_id: "q-123".to_string(),
            service_id: "weather".to_string(),
            version: "2.0".to_string(),
            provider_peer_id: "bob".to_string(),
            capabilities: HashMap::new(),
            hop_count: 1,
        };
        let content = format!(
            "{}{}",
            SVC_DISCOVER_RESPONSE,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "bob", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed { events_to_emit, .. } => {
                assert_eq!(events_to_emit.len(), 1);
                match &events_to_emit[0] {
                    ServiceEvent::ServiceDiscovered {
                        query_id,
                        service_id,
                        ..
                    } => {
                        assert_eq!(query_id, "q-123");
                        assert_eq!(service_id, "weather");
                    }
                    other => panic!("Wrong event: {:?}", other),
                }
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_handle_request_unregistered() {
        let mut svc = MeshServices::new();

        let payload = ServiceRequestPayload {
            request_id: "req-001".to_string(),
            service_id: "nonexistent".to_string(),
            method: "get".to_string(),
            body: "{}".to_string(),
        };
        let content = format!(
            "{}{}",
            SVC_REQUEST,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                assert_eq!(messages_to_send.len(), 1);
                assert!(messages_to_send[0].1.contains("not_found"));
                assert!(events_to_emit.is_empty());
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_handle_request_registered() {
        let mut svc = MeshServices::new();
        svc.register_service(make_descriptor("echo")).unwrap();

        let payload = ServiceRequestPayload {
            request_id: "req-002".to_string(),
            service_id: "echo".to_string(),
            method: "ping".to_string(),
            body: "hello".to_string(),
        };
        let content = format!(
            "{}{}",
            SVC_REQUEST,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                assert!(messages_to_send.is_empty());
                assert_eq!(events_to_emit.len(), 1);
                match &events_to_emit[0] {
                    ServiceEvent::ServiceRequestReceived {
                        request_id,
                        service_id,
                        method,
                        ..
                    } => {
                        assert_eq!(request_id, "req-002");
                        assert_eq!(service_id, "echo");
                        assert_eq!(method, "ping");
                    }
                    other => panic!("Wrong event: {:?}", other),
                }
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_handle_response() {
        let mut svc = MeshServices::new();

        let payload = ServiceResponsePayload {
            request_id: "req-003".to_string(),
            service_id: "echo".to_string(),
            status: "ok".to_string(),
            body: "pong".to_string(),
        };
        let content = format!(
            "{}{}",
            SVC_RESPONSE,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "bob", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed { events_to_emit, .. } => {
                assert_eq!(events_to_emit.len(), 1);
                match &events_to_emit[0] {
                    ServiceEvent::ServiceResponseReceived {
                        request_id, status, ..
                    } => {
                        assert_eq!(request_id, "req-003");
                        assert_eq!(status, "ok");
                    }
                    other => panic!("Wrong event: {:?}", other),
                }
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_not_handled() {
        let mut svc = MeshServices::new();
        let action = svc.handle_incoming_message("Hello, world!", "alice", 0, "user1", &[]);
        assert!(matches!(action, ServiceAction::NotHandled));
    }

    #[test]
    fn test_cleanup_expired() {
        let mut svc = MeshServices::new();
        svc.seen_discovery_queries.insert(
            "old-query".to_string(),
            Instant::now() - Duration::from_secs(120),
        );
        svc.seen_discovery_order.push_back("old-query".to_string());
        svc.seen_discovery_queries
            .insert("fresh-query".to_string(), Instant::now());
        svc.seen_discovery_order
            .push_back("fresh-query".to_string());

        svc.cleanup_expired();

        assert!(!svc.seen_discovery_queries().contains_key("old-query"));
        assert!(svc.seen_discovery_queries().contains_key("fresh-query"));
    }

    #[test]
    fn test_gossip_fanout_limit() {
        let mut svc = MeshServices::new();

        let payload = ServiceDiscoveryQueryPayload {
            query_id: "q-fanout".to_string(),
            originator: "alice".to_string(),
            service_id: None,
            remaining_hops: 5,
        };
        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::to_string(&payload).unwrap()
        );

        // Create more peers than the fanout limit
        let peers: Vec<String> = (0..20).map(|i| format!("peer-{i}")).collect();
        let action = svc.handle_incoming_message(&content, "alice", 1, "user1", &peers);
        match action {
            ServiceAction::Consumed {
                messages_to_send, ..
            } => {
                // Should be capped at DISCOVERY_GOSSIP_MAX_FANOUT forwards
                assert_eq!(messages_to_send.len(), DISCOVERY_GOSSIP_MAX_FANOUT);
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_dedup_cap_evicts_oldest() {
        let mut svc = MeshServices::new();

        // Fill to capacity using record_seen_query for consistent state
        let base = Instant::now() - Duration::from_secs(30);
        for i in 0..DISCOVERY_QUERY_MAX_DEDUP_ENTRIES {
            let key = format!("q-{i}");
            svc.seen_discovery_queries
                .insert(key.clone(), base + Duration::from_millis(i as u64));
            svc.seen_discovery_order.push_back(key);
        }
        assert_eq!(
            svc.seen_discovery_queries().len(),
            DISCOVERY_QUERY_MAX_DEDUP_ENTRIES
        );

        // Insert one more — should evict the oldest (q-0)
        svc.record_seen_query("q-new".to_string());
        assert_eq!(
            svc.seen_discovery_queries().len(),
            DISCOVERY_QUERY_MAX_DEDUP_ENTRIES
        );
        assert!(!svc.seen_discovery_queries().contains_key("q-0"));
        assert!(svc.seen_discovery_queries().contains_key("q-new"));
    }

    #[test]
    fn test_remaining_hops_zero_no_forwarding() {
        let mut svc = MeshServices::new();

        let payload = ServiceDiscoveryQueryPayload {
            query_id: "q-nohop".to_string(),
            originator: "alice".to_string(),
            service_id: None,
            remaining_hops: 0,
        };
        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::to_string(&payload).unwrap()
        );

        let peers = vec!["bob".to_string(), "charlie".to_string()];
        let action = svc.handle_incoming_message(&content, "alice", 1, "user1", &peers);
        match action {
            ServiceAction::Consumed {
                messages_to_send, ..
            } => {
                // No local services, no forwarding — should be empty
                assert!(
                    messages_to_send.is_empty(),
                    "Should not forward when remaining_hops=0"
                );
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_oversized_payload_dropped() {
        let mut svc = MeshServices::new();

        // Build a request with oversized body
        let huge_body = "x".repeat(MAX_SERVICE_PAYLOAD_SIZE + 1);
        let content = format!("{}{}", SVC_REQUEST, huge_body);

        let action = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        assert!(matches!(
            action,
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } if messages_to_send.is_empty() && events_to_emit.is_empty()
        ));
    }

    #[test]
    fn test_oversized_request_body_dropped() {
        let mut svc = MeshServices::new();
        svc.register_service(make_descriptor("echo")).unwrap();

        let large_body = "x".repeat(MAX_SERVICE_BODY_SIZE + 1);
        let payload = ServiceRequestPayload {
            request_id: "req-big".to_string(),
            service_id: "echo".to_string(),
            method: "ping".to_string(),
            body: large_body,
        };
        let content = format!(
            "{}{}",
            SVC_REQUEST,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                assert!(messages_to_send.is_empty());
                assert!(
                    events_to_emit.is_empty(),
                    "Should not emit event for oversized body"
                );
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_oversized_method_dropped() {
        let mut svc = MeshServices::new();
        svc.register_service(make_descriptor("echo")).unwrap();

        let long_method = "m".repeat(MAX_SERVICE_METHOD_LEN + 1);
        let payload = ServiceRequestPayload {
            request_id: "req-longmethod".to_string(),
            service_id: "echo".to_string(),
            method: long_method,
            body: "{}".to_string(),
        };
        let content = format!(
            "{}{}",
            SVC_REQUEST,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "alice", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                assert!(messages_to_send.is_empty());
                assert!(
                    events_to_emit.is_empty(),
                    "Should not emit event for oversized method"
                );
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_handle_response_invalid_status_dropped() {
        let mut svc = MeshServices::new();

        let payload = ServiceResponsePayload {
            request_id: "req-bad".to_string(),
            service_id: "echo".to_string(),
            status: "invalid_status".to_string(),
            body: "data".to_string(),
        };
        let content = format!(
            "{}{}",
            SVC_RESPONSE,
            serde_json::to_string(&payload).unwrap()
        );

        let action = svc.handle_incoming_message(&content, "bob", 0, "user1", &[]);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                assert!(messages_to_send.is_empty());
                assert!(
                    events_to_emit.is_empty(),
                    "Should not emit event for invalid status"
                );
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_handle_discover_query_with_match_and_forwarding() {
        let mut svc = MeshServices::new();
        svc.register_service(ServiceDescriptor {
            service_id: ServiceId::new("weather").unwrap(),
            version: "2.0".to_string(),
            capabilities: HashMap::new(),
        })
        .unwrap();

        let payload = ServiceDiscoveryQueryPayload {
            query_id: "q-combined".to_string(),
            originator: "alice".to_string(),
            service_id: Some("weather".to_string()),
            remaining_hops: 5,
        };
        let content = format!(
            "{}{}",
            SVC_DISCOVER_QUERY,
            serde_json::to_string(&payload).unwrap()
        );

        // Provide peers to forward to (excluding sender "alice" and originator "alice")
        let peers = vec!["bob".to_string(), "charlie".to_string(), "dave".to_string()];
        let action = svc.handle_incoming_message(&content, "alice", 1, "user1", &peers);
        match action {
            ServiceAction::Consumed {
                messages_to_send,
                events_to_emit,
            } => {
                // Should have a discovery response back to sender
                let responses: Vec<_> = messages_to_send
                    .iter()
                    .filter(|(_, c, _)| c.starts_with(SVC_DISCOVER_RESPONSE))
                    .collect();
                assert_eq!(responses.len(), 1, "Should have one discovery response");
                assert_eq!(responses[0].0, "alice", "Response should go to sender");

                // Should also have forwarded query messages to other peers
                let forwards: Vec<_> = messages_to_send
                    .iter()
                    .filter(|(_, c, _)| c.starts_with(SVC_DISCOVER_QUERY))
                    .collect();
                assert_eq!(forwards.len(), 3, "Should forward to all 3 eligible peers");
                // Forwarded queries should NOT go to the sender
                assert!(
                    forwards.iter().all(|(r, _, _)| r != "alice"),
                    "Should not forward back to sender"
                );

                // No events emitted for queries — events only on discovery responses
                assert!(events_to_emit.is_empty());
            }
            ServiceAction::NotHandled => panic!("Should have been handled"),
        }
    }

    #[test]
    fn test_select_fanout_peers_all_when_under_limit() {
        let peers: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let selected = select_fanout_peers(&peers, 5, "q-1", "user1");
        assert_eq!(selected.len(), 3);
    }

    #[test]
    fn test_select_fanout_peers_caps_at_max() {
        let peers: Vec<String> = (0..50).map(|i| format!("p-{i}")).collect();
        let selected = select_fanout_peers(&peers, 5, "q-1", "user1");
        assert_eq!(selected.len(), 5);
    }

    #[test]
    fn test_select_fanout_peers_deterministic() {
        let peers: Vec<String> = (0..20).map(|i| format!("p-{i}")).collect();
        let a = select_fanout_peers(&peers, 5, "q-1", "user1");
        let b = select_fanout_peers(&peers, 5, "q-1", "user1");
        assert_eq!(a, b, "Same inputs should produce same selection");
    }

    #[test]
    fn test_select_fanout_peers_varies_by_user() {
        let peers: Vec<String> = (0..20).map(|i| format!("p-{i}")).collect();
        let a = select_fanout_peers(&peers, 5, "q-1", "user1");
        let b = select_fanout_peers(&peers, 5, "q-1", "user2");
        // Different users should (usually) select different subsets
        // This is probabilistic but with 20 peers and 5 selected, collision is unlikely
        assert_ne!(
            a, b,
            "Different users should typically select different peers"
        );
    }
}
