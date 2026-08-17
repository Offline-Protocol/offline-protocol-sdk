//! Configuration accessors, diagnostics, and service registration.

use super::mesh_relay::{MeshRelayConfig, MeshRelayStats};
use super::{
    lock_shared_state, OfflineProtocol, PendingQueueMetrics, ProtocolState, KNOWN_PEER_TTL_SECS,
    MEDIA_TRANSFER_STALE_TIMEOUT_SECS,
};
use crate::events::Event;
use crate::file_transfer::FileTransferManager;
use crate::{Error, ProtocolConfig, Result, TransportManager};
use offline_protocol_core::{MessageId, ServiceDescriptor};
use offline_protocol_mls::MlsManager;
use offline_protocol_reliability::{
    AckConfig, AckManager, Deduplicator, DeduplicatorConfig, DeduplicatorStats, RetryConfig,
    RetryQueue,
};
use offline_protocol_router::{DorsConfig, RelayConfig};
use offline_protocol_services::MeshServices;
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;
use tracing::{error, warn};

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

    /// Records the host's battery reading for this device.
    ///
    /// The platform owns this number — no transport can observe it — and until
    /// it arrives every battery-dependent policy in the SDK runs in its
    /// unknown-level branch: DORS energy scoring skips its battery term, the
    /// relay role is never evaluated (so `relay_promoted`/`relay_demoted` can
    /// never fire), and message forwarding takes the "unknown means willing"
    /// path past [`RelayConfig::min_battery_for_relay`]. Feeding it is what
    /// makes those policies live.
    ///
    /// `is_charging` is not cosmetic: a charging device is deliberately
    /// excused the soft relay minimum and stopped only by the hard
    /// `CRITICAL_RELAY_BATTERY_LEVEL` floor, so reporting a level without the
    /// charging state strips relay duty from plugged-in devices that should
    /// keep it.
    ///
    /// `level` is clamped to 0-100.
    pub fn set_device_battery(&mut self, level: u8, is_charging: bool) {
        self.transport_manager
            .set_device_battery(level, is_charging);
    }

    /// Returns the host-reported `(battery_level, is_charging)` pair, or
    /// `(None, false)` when no platform reading has been supplied.
    pub fn device_battery(&self) -> (Option<u8>, bool) {
        self.transport_manager.device_battery()
    }

    /// Updates the relay configuration at runtime.
    ///
    /// Validated the same way [`Self::update_ack_config`] is — by building the
    /// candidate configuration and running the real validator — so a
    /// constraint added to `ProtocolConfig::validate` applies on this path too.
    ///
    /// Takes effect on the next forwarding decision and the next `process()`
    /// tick, which is where the relay standing is re-derived. One copy, read by
    /// every relay decision: the forwarding gate, the capability bias pushed
    /// into the governor, and the battery floor the demotion event reports. An
    /// earlier shape kept a second copy on the router's relay manager and had
    /// to write both, which is exactly the split that lets a device decline the
    /// relay role while still carrying other people's traffic.
    pub fn update_relay_config(&mut self, config: RelayConfig) -> crate::Result<()> {
        let mut candidate = self.config.clone();
        candidate.relay = config.clone();
        candidate.validate()?;
        self.config.relay = config;
        Ok(())
    }

    /// Returns the current relay configuration.
    pub fn relay_config(&self) -> &RelayConfig {
        &self.config.relay
    }

    /// Whether this device is currently acting as a relay.
    ///
    /// Reports what this device has been *doing* — carrying traffic for other
    /// devices — not what its battery and neighbor count suggest it could. It
    /// is the standing the `relay_promoted` / `relay_demoted` /
    /// `relay_demoted_battery` events announce, re-derived on each `process()`
    /// tick from the forwarding governor's own record of frames carried.
    ///
    /// Consequently it answers `false` on a device that has every capability to
    /// relay but has had no third-party traffic to carry — including any device
    /// whose peers all have their own route to each other, and any device on
    /// which an infrastructure carrier is up, since the mesh is offered frames
    /// only when nothing else can reach the recipient.
    ///
    /// Needs no battery feed. The feed scales *how much* this device forwards
    /// (see [`Self::set_device_battery`]); forwarding is observable either way.
    pub fn is_relay(&self) -> bool {
        self.mesh_relay.is_active_relay()
    }

    /// Updates the ACK configuration at runtime.
    ///
    /// Note: This affects new ACK registrations; existing pending ACKs keep their original timeout.
    /// Validated the same way [`Self::update_retry_config`] is — by building
    /// the candidate configuration and running the real validator — rather than
    /// by repeating its checks inline. Hand-rolled copies drift: a constraint
    /// added to `ProtocolConfig::validate` for a new `AckConfig` field would be
    /// enforced at construction and silently skipped on the runtime-update path.
    pub fn update_ack_config(&mut self, config: AckConfig) -> crate::Result<()> {
        let mut candidate = self.config.clone();
        candidate.reliability.ack = config.clone();
        candidate.validate()?;
        self.ack_manager = AckManager::with_config(config.clone());
        self.config.reliability.ack = config;
        Ok(())
    }

    /// Updates the retry configuration at runtime.
    ///
    /// Note: This affects new retry entries; existing entries keep their original timing.
    pub fn update_retry_config(&mut self, config: RetryConfig) -> crate::Result<()> {
        let mut candidate = self.config.clone();
        candidate.reliability.retry = config.clone();
        candidate.validate()?;
        self.retry_queue = RetryQueue::with_config(config.clone());
        self.config.reliability.retry = config;
        self.recompute_next_pending_message_expiry();
        self.cleanup_expired_pending_messages();
        Ok(())
    }

    /// Updates the deduplication configuration at runtime.
    ///
    /// Note: This clears the deduplication cache and applies the new config.
    ///
    /// Validated the same way its two siblings are, and for a reason that only
    /// appeared once they were. Both of those run the *whole*
    /// `ProtocolConfig::validate`, which checks the Bloom parameters this
    /// method installs — so while this one accepted anything, a configuration
    /// it had already stored could make a perfectly valid
    /// [`Self::update_retry_config`] fail, complaining about a Bloom filter the
    /// caller never mentioned. `Deduplicator::with_config` fails safe on those
    /// values rather than panicking, so the cost was a confusing rejection
    /// rather than a crash — but a rejection attributed to the wrong call is
    /// its own kind of expensive.
    pub fn update_dedup_config(&mut self, config: DeduplicatorConfig) -> crate::Result<()> {
        let mut candidate = self.config.clone();
        candidate.reliability.dedup = config.clone();
        candidate.validate()?;
        self.deduplicator = Deduplicator::with_config(config.clone());
        self.config.reliability.dedup = config;
        Ok(())
    }

    /// Gets deduplicator statistics for monitoring.
    pub fn deduplicator_stats(&self) -> DeduplicatorStats {
        self.deduplicator.stats()
    }

    /// Gets pending encrypted message queue counters and gauges.
    pub fn pending_queue_metrics(&self) -> PendingQueueMetrics {
        self.pending_queue.metrics().clone()
    }

    /// Gets the current ACK manager statistics.
    pub fn pending_ack_count(&self) -> usize {
        self.ack_manager.pending_count()
    }

    /// Gets the current retry queue statistics.
    pub fn retry_queue_size(&self) -> usize {
        self.retry_queue.len()
    }

    /// Returns whether `peer_id` is currently tracked as a discovered
    /// neighbor. Populated by [`Self::on_neighbor_discovered`] and cleared
    /// by [`Self::on_neighbor_lost`], by the periodic TTL sweep when the
    /// peer has not been re-seen for `KNOWN_PEER_TTL_SECS`, or by
    /// least-recently-seen eviction when tracking is at capacity — so this
    /// can flip to false without any explicit loss signal. Blocked users
    /// are never tracked.
    pub fn is_known_peer(&self, peer_id: &str) -> bool {
        self.known_peers.contains_key(peer_id)
    }

    /// Reports how much traffic this device is carrying for other people.
    ///
    /// Useful for showing a user what their device is contributing, and for
    /// spotting a neighborhood in trouble: a rising `rate_deferred` means
    /// forwarding is hitting its ceiling, and `peer_rate_limited` means a
    /// single neighbor is sending more than its share. `dropped_for_capacity`
    /// is expected to stay at zero — see [`MeshRelayStats::dropped_for_capacity`].
    pub fn mesh_relay_stats(&self) -> MeshRelayStats {
        let counters = self.mesh_relay.counters();
        MeshRelayStats {
            forwarded: counters.forwarded,
            transmissions: counters.transmissions,
            queued: counters.queued,
            awaiting_transmission: self.mesh_relay.pending_len(),
            duplicates_suppressed: counters.duplicates_suppressed,
            covered_by_a_neighbor: counters.cancelled_by_duplicate,
            peer_rate_limited: counters.peer_rate_limited,
            rate_deferred: counters.rate_deferred,
            hop_limit_reached: counters.hop_limit_reached,
            reach_clamped: counters.ttl_clamped,
            dropped_for_capacity: self.mesh_relay.seen_capacity_evictions(),
        }
    }

    /// Reports the mesh forwarding tunables currently in force.
    ///
    /// Read from the governor itself, not from the stored [`ProtocolConfig`],
    /// so this reports what forwarding decisions actually use. The section is
    /// fixed at construction today, which makes the two identical — reading
    /// the consumer is what keeps that an implementation detail rather than
    /// something a caller has to know.
    pub fn mesh_relay_config(&self) -> MeshRelayConfig {
        self.mesh_relay.config().clone()
    }

    /// Returns a mutable reference to the retry queue (test-only).
    #[cfg(test)]
    pub(crate) fn retry_queue_mut(&mut self) -> &mut offline_protocol_reliability::RetryQueue {
        &mut self.retry_queue
    }

    /// Cleans up expired entries from deduplicator, retry queue, outbox, and ack manager.
    /// Also checks for Internet availability transitions to sync groups with relay.
    pub(crate) fn cleanup_expired_entries(&mut self) {
        self.deduplicator.cleanup_expired();
        self.retry_queue.cleanup_expired();
        self.mesh_relay.maintain(std::time::Instant::now());
        self.prune_stale_known_peers(std::time::Instant::now());
        self.cleanup_expired_pending_messages_if_due();
        self.cleanup_outbox();
        self.mesh_services.cleanup_expired();
        self.cleanup_group_message_dedup();
        self.check_epoch_forks();
        self.check_leave_election_timeouts();
        self.check_relay_group_sync();
        let stale_transfers = self
            .file_transfer_manager
            .cleanup_stale_transfers(StdDuration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS));
        for stale in stale_transfers {
            self.pending_media_metadata.remove(&stale.file_id);
            // Like the resource-limit drops, a stale transfer is
            // unrecoverable (its chunks were ACKed and will not be
            // retransmitted) — tell the app instead of going silent.
            self.emit_event(Event::file_receive_failed(
                stale.file_id,
                stale.file_name,
                stale.sender,
                "stale_timeout".to_string(),
            ));
        }
        self.cleanup_stale_media_state(StdDuration::from_secs(MEDIA_TRANSFER_STALE_TIMEOUT_SECS));
        // Prune old timed-out ACKs that weren't cleaned up by normal retry flow
        self.ack_manager
            .prune_old_timeouts(std::time::Duration::from_secs(300)); // 5 minutes
    }

    /// Evicts known peers not re-seen within `KNOWN_PEER_TTL_SECS`.
    ///
    /// This is the only removal path for peers on carriers without a
    /// disconnect signal (Internet, Reticulum, Nostr, WiFi Direct
    /// message-path senders). `now` is injected for test determinism.
    pub(crate) fn prune_stale_known_peers(&mut self, now: std::time::Instant) {
        let ttl = StdDuration::from_secs(KNOWN_PEER_TTL_SECS);
        let stale: Vec<String> = self
            .known_peers
            .iter()
            .filter(|(_, seen)| now.saturating_duration_since(**seen) > ttl)
            .map(|(id, _)| id.clone())
            .collect();
        if stale.is_empty() {
            return;
        }
        tracing::debug!(
            count = stale.len(),
            ttl_secs = KNOWN_PEER_TTL_SECS,
            "Evicting stale known peers"
        );
        for peer_id in &stale {
            self.evict_known_peer(peer_id);
        }
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
        // Service discovery messages are internal control messages (not user
        // content), so they are exempt from require_encryption.

        let peers: Vec<String> = self.known_peers.keys().cloned().collect();
        let result = self
            .mesh_services
            .discover_services(&self.local_id, &peers, service_id)
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
        // Service requests are internal control messages (not user content),
        // so they are exempt from require_encryption.
        Self::validate_outbound_recipient(provider)?;

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
        // Service responses are internal control messages (not user content),
        // so they are exempt from require_encryption.
        Self::validate_outbound_recipient(requester)?;

        let result = self
            .mesh_services
            .respond_to_service_request(request_id, requester, service_id, status, body)
            .map_err(Error::Service)?;
        let msg = result.message;
        let message_id = self.send_internal_message(&msg.recipient, msg.content, msg.priority)?;
        Ok(message_id)
    }
}
