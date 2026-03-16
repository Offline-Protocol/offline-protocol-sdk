//! Configuration accessors, diagnostics, and service registration.

use super::*;

impl OfflineProtocol {
    /// Gets the current protocol state.
    pub fn state(&self) -> ProtocolState {
        let Ok(state) = lock_shared_state(&self.shared_state) else {
            error!("Failed to lock shared state in state()");
            return ProtocolState::Stopped;
        };
        state.state
    }

    /// Gets the configuration.
    pub fn config(&self) -> &ProtocolConfig {
        &self.config
    }

    /// Gets a reference to the mesh services registry.
    pub fn mesh_services(&self) -> &MeshServices {
        &self.mesh_services
    }

    /// Gets a mutable reference to the transport manager.
    ///
    /// This allows external code (e.g., FFI) to add transports dynamically.
    pub fn transport_manager_mut(&mut self) -> &mut TransportManager {
        &mut self.transport_manager
    }

    /// Gets a reference to the transport manager.
    pub fn transport_manager(&self) -> &TransportManager {
        &self.transport_manager
    }

    /// Returns a mutable reference to the file transfer manager.
    pub fn file_transfer_manager_mut(&mut self) -> &mut FileTransferManager {
        &mut self.file_transfer_manager
    }

    /// Returns a reference to the file transfer manager.
    pub fn file_transfer_manager(&self) -> &FileTransferManager {
        &self.file_transfer_manager
    }

    /// Gets access to the MLS manager for advanced operations.
    ///
    /// Returns `None` if MLS is not initialized.
    pub fn mls_manager(&self) -> Option<&Arc<RwLock<MlsManager>>> {
        self.mls_manager.as_ref()
    }

    /// Updates the DORS configuration at runtime.
    ///
    /// This replaces the current DORS selector configuration with the provided config.
    pub fn update_dors_config(&mut self, config: DorsConfig) {
        self.transport_manager.update_selector_config(config);
    }

    /// Updates the ACK configuration at runtime.
    ///
    /// Note: This affects new ACK registrations; existing pending ACKs keep their original timeout.
    pub fn update_ack_config(&mut self, config: AckConfig) {
        self.ack_manager = AckManager::with_config(config.clone());
        self.config.reliability.ack = config;
    }

    /// Updates the retry configuration at runtime.
    ///
    /// Note: This affects new retry entries; existing entries keep their original timing.
    pub fn update_retry_config(&mut self, config: RetryConfig) {
        self.retry_queue = RetryQueue::with_config(config.clone());
        self.config.reliability.retry = config;
    }

    /// Updates the deduplication configuration at runtime.
    ///
    /// Note: This clears the deduplication cache and applies the new config.
    pub fn update_dedup_config(&mut self, config: DeduplicatorConfig) {
        self.deduplicator = Deduplicator::with_config(config.clone());
        self.config.reliability.dedup = config;
    }

    /// Gets deduplicator statistics for monitoring.
    pub fn deduplicator_stats(&self) -> DeduplicatorStats {
        self.deduplicator.stats()
    }

    /// Gets pending encrypted message queue counters and gauges.
    pub fn pending_queue_metrics(&self) -> PendingQueueMetrics {
        self.pending_queue_metrics.clone()
    }

    /// Gets the current ACK manager statistics.
    pub fn pending_ack_count(&self) -> usize {
        self.ack_manager.pending_count()
    }

    /// Gets the current retry queue statistics.
    pub fn retry_queue_size(&self) -> usize {
        self.retry_queue.len()
    }

    /// Cleans up expired entries from deduplicator, retry queue, outbox, and ack manager.
    /// Also checks for Internet availability transitions to sync groups with relay.
    pub(crate) fn cleanup_expired_entries(&mut self) {
        self.deduplicator.cleanup_expired();
        self.retry_queue.cleanup_expired();
        self.cleanup_outbox();
        self.mesh_services.cleanup_expired();
        self.cleanup_group_message_dedup();
        self.check_epoch_forks();
        self.check_leave_election_timeouts();
        self.check_relay_group_sync();
        let stale_file_ids = self
            .file_transfer_manager
            .cleanup_stale_transfers(StdDuration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS));
        for file_id in stale_file_ids {
            self.pending_media_metadata.remove(&file_id);
        }
        self.cleanup_stale_media_state(StdDuration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS));
        // Prune old timed-out ACKs that weren't cleaned up by normal retry flow
        self.ack_manager
            .prune_old_timeouts(std::time::Duration::from_secs(300)); // 5 minutes
    }

    // ========================================================================
    // SERVICE DISCOVERY & REQUEST/RESPONSE
    // ========================================================================

    /// Registers a local service that this node offers.
    pub fn register_service(&mut self, descriptor: ServiceDescriptor) -> Result<()> {
        self.mesh_services
            .register_service(descriptor)
            .map_err(Error::Service)
    }

    /// Unregisters a local service. Returns true if the service was found and removed.
    pub fn unregister_service(&mut self, service_id: &str) -> Result<bool> {
        self.mesh_services
            .unregister_service(service_id)
            .map_err(Error::Service)
    }

    /// Broadcasts a service discovery query to all known peers.
    /// Returns a query_id. Responses arrive asynchronously as `ServiceDiscovered` events.
    ///
    /// **Note:** Discovery responses currently travel only one hop back (to the
    /// immediate sender of the query). Multi-hop response relay is not yet
    /// implemented, so services more than one hop away will generate responses
    /// that reach intermediate forwarders but not the original querier.
    pub fn discover_services(&mut self, service_id: Option<&str>) -> Result<String> {
        self.ensure_plaintext_control_send_allowed("discover_services")?;

        let peers: Vec<String> = self.known_peers.iter().cloned().collect();
        let result = self
            .mesh_services
            .discover_services(&self.config.user_id, &peers, service_id)
            .map_err(Error::Service)?;
        let mut send_failures = 0usize;
        for msg in result.messages {
            if self
                .send_internal_message(&msg.recipient, msg.content, msg.priority)
                .is_err()
            {
                send_failures += 1;
            }
        }
        if send_failures > 0 {
            warn!(
                failures = send_failures,
                total = peers.len(),
                "Some discovery broadcasts failed to send"
            );
        }
        Ok(result.query_id)
    }

    /// Sends a typed service request to a specific provider peer.
    /// Returns a request_id. The response arrives as a `ServiceResponseReceived` event.
    pub fn send_service_request(
        &mut self,
        provider: &str,
        service_id: &str,
        method: &str,
        body: &str,
    ) -> Result<String> {
        self.ensure_plaintext_control_send_allowed("send_service_request")?;

        let result = self
            .mesh_services
            .send_service_request(provider, service_id, method, body)
            .map_err(Error::Service)?;
        let msg = result.message;
        self.send_internal_message(&msg.recipient, msg.content, msg.priority)?;
        Ok(result.request_id)
    }

    /// Responds to a service request from another peer.
    pub fn respond_to_service_request(
        &mut self,
        request_id: &str,
        requester: &str,
        service_id: &str,
        status: &str,
        body: &str,
    ) -> Result<MessageId> {
        self.ensure_plaintext_control_send_allowed("respond_to_service_request")?;

        let result = self
            .mesh_services
            .respond_to_service_request(request_id, requester, service_id, status, body)
            .map_err(Error::Service)?;
        let msg = result.message;
        let message_id = self.send_internal_message(&msg.recipient, msg.content, msg.priority)?;
        Ok(message_id)
    }
}
