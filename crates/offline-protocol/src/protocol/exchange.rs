//! Capability exchange integration.
//!
//! Wires the I/O-free [`ExchangeCore`] state machine to this protocol
//! instance: the MLS identity key signs listings and receipts, exchange
//! control messages ride `send_internal_message` (signed, TOFU-checked, and
//! MLS-encrypted once a session exists), settlement-bearing messages are
//! gated on a **confirmed** MLS session, adapter artifacts move over the
//! media transfer path with content-hash verification on arrival, and the
//! durable exchange state (ledger, receipts, reputation) persists through
//! the same `MlsStorage` backend as the rest of the protocol.

use super::{base64_encode, storage_keys, OfflineProtocol};
use crate::events::Event;
use crate::{Error, EstablishmentState, Result};
use chrono::Utc;
use offline_protocol_core::{ContentType, MessagePriority, ServiceDescriptor};
use offline_protocol_exchange::{
    AdapterRuntime, ArtifactRef, AttestationStatus, Balance, ChunkPlan, DiscoveredListing,
    ExchangeError, ExchangeEvent, ExchangeOutbound, ExchangeResult, ExchangeSigner,
    ExchangeSnapshot, ExchangeVerifier, Listing, ListingFilter, ListingKind, ReputationRead,
    RequestDisposition, SettlementBackend, SettlementReport, StoredReceipt, Terms, UsageReceipt,
    ADAPTER_PULL_METHOD,
};
use offline_protocol_mls::MlsManager;
use offline_protocol_services::ServiceEvent;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

/// Preferred chunk size hint recorded in adapter artifact references. The
/// actual transfer chunking is chosen per-transport by `send_media`.
const ADAPTER_CHUNK_SIZE_HINT: u32 = 64 * 1024;

/// Signs exchange payloads with this node's MLS Ed25519 identity key.
pub(crate) struct MlsExchangeSigner {
    mls: Option<Arc<RwLock<MlsManager>>>,
}

impl ExchangeSigner for MlsExchangeSigner {
    fn public_key(&self) -> ExchangeResult<Vec<u8>> {
        let mls = self
            .mls
            .as_ref()
            .ok_or_else(|| ExchangeError::SigningFailed("MLS not initialized".into()))?;
        let manager = mls
            .read()
            .map_err(|_| ExchangeError::SigningFailed("MLS lock poisoned".into()))?;
        manager
            .get_identity_public_key()
            .map_err(|e| ExchangeError::SigningFailed(e.to_string()))
    }

    fn sign(&self, data: &[u8]) -> ExchangeResult<Vec<u8>> {
        let mls = self
            .mls
            .as_ref()
            .ok_or_else(|| ExchangeError::SigningFailed("MLS not initialized".into()))?;
        let manager = mls
            .read()
            .map_err(|_| ExchangeError::SigningFailed("MLS lock poisoned".into()))?;
        manager
            .sign_data(data)
            .map_err(|e| ExchangeError::SigningFailed(e.to_string()))
    }
}

/// Verifies exchange signatures using the MLS crypto backend (stateless).
pub(crate) struct MlsExchangeVerifier;

impl ExchangeVerifier for MlsExchangeVerifier {
    fn verify(&self, public_key: &[u8], data: &[u8], signature: &[u8]) -> ExchangeResult<bool> {
        MlsManager::verify_signature(public_key, data, signature)
            .map_err(|e| ExchangeError::VerificationFailed(e.to_string()))
    }
}

impl OfflineProtocol {
    // ========================================================================
    // PUBLIC API — PUBLISH & DISCOVER
    // ========================================================================

    /// Publishes an attested capability listing.
    ///
    /// The listing is signed with this node's OfflineID (MLS identity) key —
    /// provenance is mandatory, so this requires `initialize_mls` first —
    /// and registered with service discovery. The wire format is the
    /// unchanged `ServiceDescriptor`; the listing rides in its capabilities
    /// map, so exchange-unaware peers still see a plain service.
    ///
    /// Use [`publish_adapter_listing`](Self::publish_adapter_listing) for
    /// adapters; this method rejects `ListingKind::Adapter` without an
    /// artifact reference.
    pub fn publish_listing(
        &mut self,
        descriptor: ServiceDescriptor,
        kind: ListingKind,
        terms: Terms,
        artifact: Option<ArtifactRef>,
    ) -> Result<Listing> {
        if self.mls_manager.is_none() {
            return Err(Error::MlsNotInitialized);
        }
        let signer = self.exchange_signer();
        let (listing, registered) =
            self.exchange
                .prepare_publish(descriptor, kind, terms, artifact, now_ms(), &signer)?;
        self.register_service(registered)?;
        Ok(listing)
    }

    /// Publishes an attested adapter listing from a local artifact file.
    ///
    /// Reads the artifact, computes its content hash and size, signs the
    /// listing (including the hash), registers it for discovery, and records
    /// the path so incoming pulls can be served automatically.
    pub fn publish_adapter_listing(
        &mut self,
        descriptor: ServiceDescriptor,
        terms: Terms,
        base_model: &str,
        base_model_version: &str,
        artifact_path: &str,
    ) -> Result<Listing> {
        let bytes = std::fs::read(artifact_path)
            .map_err(|e| Error::Other(format!("failed to read artifact {artifact_path}: {e}")))?;
        let artifact = ArtifactRef {
            content_hash: offline_protocol_exchange::artifact::content_hash(&bytes),
            size_bytes: bytes.len() as u64,
            base_model: base_model.to_string(),
            base_model_version: base_model_version.to_string(),
            chunking: ChunkPlan {
                chunk_size_bytes: ADAPTER_CHUNK_SIZE_HINT,
            },
        };
        let listing =
            self.publish_listing(descriptor, ListingKind::Adapter, terms, Some(artifact))?;
        self.exchange
            .set_artifact_path(listing.service_id(), artifact_path)
            .map_err(Error::Exchange)?;
        Ok(listing)
    }

    /// Removes a published listing and unregisters its service.
    pub fn unpublish_listing(&mut self, service_id: &str) -> Result<bool> {
        let removed = self.exchange.unpublish(service_id);
        let unregistered = self.unregister_service(service_id)?;
        Ok(removed || unregistered)
    }

    /// Broadcasts a listing discovery query. This is `discover_services`
    /// under the hood: providers respond with their descriptors, and
    /// descriptors carrying listing envelopes additionally surface as
    /// `ListingDiscovered` events with attestation status and reputation.
    pub fn discover_listings(&mut self, service_id: Option<&str>) -> Result<String> {
        self.discover_services(service_id)
    }

    /// Discovered listings passing a filter, most recently seen first.
    pub fn discovered_listings(&self, filter: &ListingFilter) -> Vec<DiscoveredListing> {
        self.exchange.discovered_listings(filter)
    }

    /// The cached discovered listing from a specific provider, if any.
    pub fn discovered_listing(
        &self,
        provider: &str,
        service_id: &str,
    ) -> Option<DiscoveredListing> {
        self.exchange
            .discovered_listing(provider, service_id)
            .cloned()
    }

    /// The local reputation read for a publisher.
    pub fn publisher_reputation(&self, publisher: &str) -> ReputationRead {
        self.exchange.reputation(publisher)
    }

    // ========================================================================
    // PUBLIC API — INVOCATION & BILLING
    // ========================================================================

    /// Invokes a discovered listing.
    ///
    /// For free listings this is `send_service_request` with tracking. For
    /// priced listings the call refuses to start unless the listing's
    /// attestation verified, a **confirmed MLS session** exists with the
    /// provider (priced invocations never ride plaintext), and the prepaid
    /// balance covers `max_units` — which is held until the invocation
    /// completes, then debited for actual usage.
    ///
    /// Returns the `request_id` correlating the eventual
    /// `ServiceResponseReceived` (and, when priced, `ExchangeReceiptIssued`)
    /// events.
    pub fn invoke_listing(
        &mut self,
        provider: &str,
        service_id: &str,
        method: &str,
        body: &str,
        max_units: u64,
    ) -> Result<String> {
        let session_confirmed = self.exchange_session_confirmed(provider);
        let reservation = self.exchange.reserve_invocation(
            provider,
            service_id,
            max_units,
            session_confirmed,
            now_ms(),
        )?;
        match self.send_service_request(provider, service_id, method, body) {
            Ok(request_id) => {
                self.exchange
                    .bind_invocation(&reservation, &request_id)
                    .map_err(Error::Exchange)?;
                self.persist_exchange_state();
                Ok(request_id)
            }
            Err(e) => {
                self.exchange.cancel_reservation(&reservation);
                Err(e)
            }
        }
    }

    /// Declares the units consumed by a metered invocation this node is
    /// serving. Call before `respond_to_service_request`; the declaration is
    /// sent to the consumer right after the response.
    pub fn declare_invocation_usage(&mut self, request_id: &str, units: u64) -> Result<()> {
        self.exchange
            .declare_usage(request_id, units)
            .map_err(Error::Exchange)
    }

    // ========================================================================
    // PUBLIC API — ADAPTERS
    // ========================================================================

    /// Pulls a discovered adapter's artifact from its provider.
    ///
    /// Pulls are free but the listing attestation must have verified. The
    /// artifact arrives over the media transfer path and is checked against
    /// the attested content hash: a match emits `AdapterPullCompleted` with
    /// the verified bytes, a mismatch emits `AdapterPullRejected` and the
    /// bytes are discarded.
    pub fn pull_adapter(&mut self, provider: &str, service_id: &str) -> Result<String> {
        self.exchange
            .validate_adapter_pull(provider, service_id)
            .map_err(Error::Exchange)?;
        let request_id =
            self.send_service_request(provider, service_id, ADAPTER_PULL_METHOD, "{}")?;
        self.exchange
            .register_adapter_pull(&request_id, provider, service_id);
        Ok(request_id)
    }

    /// Installs the adapter runtime that verified artifacts are loaded into.
    pub fn set_adapter_runtime(&mut self, runtime: Arc<dyn AdapterRuntime>) {
        self.adapter_runtime = Some(runtime);
    }

    /// Loads a pulled adapter into the installed runtime.
    ///
    /// This is the load gate: the listing attestation must have verified at
    /// discovery, and the file at `artifact_path` is re-verified against the
    /// attested content hash immediately before the load. An unverified
    /// adapter is unloadable.
    pub fn load_adapter(
        &self,
        provider: &str,
        service_id: &str,
        artifact_path: &str,
    ) -> Result<()> {
        let runtime = self
            .adapter_runtime
            .as_ref()
            .ok_or_else(|| Error::Other("no adapter runtime installed".into()))?;
        let discovered = self
            .exchange
            .discovered_listing(provider, service_id)
            .ok_or_else(|| {
                Error::Exchange(ExchangeError::UnknownListing(format!(
                    "{service_id} from {provider} (not discovered)"
                )))
            })?;
        if discovered.attestation_status != AttestationStatus::Verified {
            return Err(Error::Exchange(ExchangeError::AttestationNotVerified(
                format!(
                    "refusing to load {service_id}: attestation is {}",
                    discovered.attestation_status.as_str()
                ),
            )));
        }
        let artifact = discovered.listing.artifact.as_ref().ok_or_else(|| {
            Error::Exchange(ExchangeError::InvalidListing(format!(
                "{service_id} has no artifact reference"
            )))
        })?;
        let bytes = std::fs::read(artifact_path)
            .map_err(|e| Error::Other(format!("failed to read artifact {artifact_path}: {e}")))?;
        offline_protocol_exchange::artifact::verify_artifact(&bytes, artifact)
            .map_err(Error::Exchange)?;
        runtime
            .load(&discovered.listing, artifact_path)
            .map_err(Error::Exchange)?;
        info!(service_id = %service_id, "Verified adapter loaded into runtime");
        Ok(())
    }

    // ========================================================================
    // PUBLIC API — BALANCE, RECEIPTS, SETTLEMENT
    // ========================================================================

    /// Credits the prepaid exchange balance after out-of-band funding (e.g.
    /// the clearing backend confirmed a payment). Returns the new balance.
    pub fn credit_exchange_balance(
        &mut self,
        currency: &str,
        amount_minor: u64,
    ) -> Result<Balance> {
        let event = self
            .exchange
            .credit_balance(currency, amount_minor)
            .map_err(Error::Exchange)?;
        self.emit_event(Event::from(event));
        self.persist_exchange_state();
        Ok(self.exchange.balance(currency))
    }

    /// The prepaid exchange balance for a currency.
    pub fn exchange_balance(&self, currency: &str) -> Balance {
        self.exchange.balance(currency)
    }

    /// All stored usage receipts with their settlement status.
    pub fn exchange_receipts(&self) -> Vec<StoredReceipt> {
        self.exchange.receipts()
    }

    /// Receipts awaiting settlement — export these to a clearing backend.
    pub fn pending_exchange_receipts(&self) -> Vec<UsageReceipt> {
        self.exchange.pending_receipts()
    }

    /// Marks receipts settled after a clearing backend confirms them.
    pub fn mark_exchange_receipts_settled(&mut self, receipt_ids: &[String]) {
        self.exchange.mark_receipts_settled(receipt_ids, now_ms());
        self.persist_exchange_state();
    }

    /// Submits all pending receipts to a settlement backend and applies the
    /// outcome (settled / rejected) locally. The protocol fee is applied by
    /// the backend at settlement.
    pub fn reconcile_exchange(
        &mut self,
        backend: &dyn SettlementBackend,
    ) -> Result<SettlementReport> {
        let report = self
            .exchange
            .reconcile(backend, now_ms())
            .map_err(Error::Exchange)?;
        self.persist_exchange_state();
        Ok(report)
    }

    // ========================================================================
    // INTERNAL — DISPATCH & HANDLERS
    // ========================================================================

    /// Routes a service event through the exchange before (or instead of)
    /// surfacing it to the application.
    pub(crate) fn dispatch_service_event(&mut self, event: ServiceEvent) {
        match &event {
            ServiceEvent::ServiceDiscovered {
                query_id,
                service_id,
                version,
                provider_peer_id,
                capabilities,
                hop_count,
            } => {
                let exchange_event = self.exchange.handle_discovered(
                    query_id,
                    service_id,
                    version,
                    provider_peer_id,
                    capabilities.clone(),
                    *hop_count,
                    now_ms(),
                    &MlsExchangeVerifier,
                );
                if let Some(ex) = exchange_event {
                    self.emit_event(Event::from(ex));
                }
                self.emit_event(Event::from(event));
            }
            ServiceEvent::ServiceRequestReceived {
                request_id,
                service_id,
                method,
                sender,
                ..
            } => {
                match self
                    .exchange
                    .handle_request_received(request_id, service_id, method, sender)
                {
                    RequestDisposition::AdapterPull { artifact_path, .. } => {
                        let (request_id, sender, service_id) =
                            (request_id.clone(), sender.clone(), service_id.clone());
                        self.serve_adapter_pull(&request_id, &sender, &service_id, artifact_path);
                        // Pull requests are protocol-level; not surfaced to the app.
                    }
                    RequestDisposition::Tracked | RequestDisposition::NotExchange => {
                        self.emit_event(Event::from(event));
                    }
                }
            }
            ServiceEvent::ServiceResponseReceived {
                request_id, status, ..
            } => {
                let signer = self.exchange_signer();
                let (outbound, events) =
                    self.exchange
                        .handle_response_received(request_id, status, now_ms(), &signer);
                let had_exchange_activity = !events.is_empty();
                self.dispatch_exchange_actions(outbound, events);
                if had_exchange_activity {
                    self.persist_exchange_state();
                }
                self.emit_event(Event::from(event));
            }
        }
    }

    /// Handles an inbound usage declaration (`__XCHG_USAGE__`).
    pub(crate) fn handle_exchange_usage_message(&mut self, sender: &str, data: &str) {
        let signer = self.exchange_signer();
        let (outbound, events) =
            self.exchange
                .handle_usage_message(sender, data, now_ms(), &signer);
        let had_activity = !events.is_empty() || !outbound.is_empty();
        self.dispatch_exchange_actions(outbound, events);
        if had_activity {
            self.persist_exchange_state();
        }
    }

    /// Handles an inbound signed receipt (`__XCHG_RCPT__`).
    pub(crate) fn handle_exchange_receipt_message(&mut self, sender: &str, data: &str) {
        let signer = self.exchange_signer();
        let (outbound, events) =
            self.exchange
                .handle_receipt_message(sender, data, &signer, &MlsExchangeVerifier);
        let had_activity = !events.is_empty() || !outbound.is_empty();
        self.dispatch_exchange_actions(outbound, events);
        if had_activity {
            self.persist_exchange_state();
        }
    }

    /// Handles an inbound receipt counter-signature (`__XCHG_RCPT_ACK__`).
    pub(crate) fn handle_exchange_receipt_ack(&mut self, sender: &str, data: &str) {
        let events = self
            .exchange
            .handle_receipt_ack(sender, data, &MlsExchangeVerifier);
        let had_activity = !events.is_empty();
        for event in events {
            self.emit_event(Event::from(event));
        }
        if had_activity {
            self.persist_exchange_state();
        }
    }

    /// Sends exchange control messages (gating settlement-bearing ones on a
    /// confirmed MLS session) and emits exchange events.
    pub(crate) fn dispatch_exchange_actions(
        &mut self,
        outbound: Vec<ExchangeOutbound>,
        events: Vec<ExchangeEvent>,
    ) {
        for msg in outbound {
            if msg.requires_confirmed_session && !self.exchange_session_confirmed(&msg.recipient) {
                // Never let a settlement message ride plaintext. The local
                // receipt store still holds the claim; settlement remains
                // possible from this side alone.
                warn!(
                    recipient = %msg.recipient,
                    "Dropping settlement-bearing exchange message: no confirmed MLS session"
                );
                continue;
            }
            if let Err(e) =
                self.send_internal_message(&msg.recipient, msg.content, MessagePriority::High)
            {
                warn!(recipient = %msg.recipient, error = %e, "Failed to send exchange message");
            }
        }
        for event in events {
            self.emit_event(Event::from(event));
        }
    }

    /// Serves an incoming adapter pull: responds and ships the artifact over
    /// the media transfer path, or responds `error` when unavailable.
    fn serve_adapter_pull(
        &mut self,
        request_id: &str,
        requester: &str,
        service_id: &str,
        artifact_path: Option<String>,
    ) {
        let bytes = artifact_path.as_ref().and_then(|path| {
            std::fs::read(path)
                .map_err(|e| {
                    warn!(service_id = %service_id, path = %path, error = %e, "Failed to read adapter artifact");
                })
                .ok()
        });
        match bytes {
            Some(bytes) => {
                if let Err(e) = self.respond_to_service_request(
                    request_id,
                    requester,
                    service_id,
                    "ok",
                    r#"{"transfer":"media"}"#,
                ) {
                    warn!(request_id = %request_id, error = %e, "Failed to respond to adapter pull");
                    return;
                }
                match self.send_media(
                    requester,
                    bytes,
                    format!("{service_id}.adapter"),
                    ContentType::File,
                    None,
                ) {
                    Ok(file_id) => {
                        info!(
                            service_id = %service_id,
                            requester = %requester,
                            file_id = %file_id,
                            "Serving adapter artifact"
                        );
                    }
                    Err(e) => {
                        warn!(service_id = %service_id, error = %e, "Failed to ship adapter artifact");
                    }
                }
            }
            None => {
                debug!(service_id = %service_id, "Adapter pull with no artifact available");
                let _ = self.respond_to_service_request(
                    request_id,
                    requester,
                    service_id,
                    "error",
                    "artifact unavailable",
                );
            }
        }
    }

    /// Intercepts a completed inbound file transfer when the sender has a
    /// pending adapter pull. Returns `true` when the file was consumed as an
    /// adapter artifact (verified or rejected) and must not surface as a
    /// regular `FileReceived` event.
    pub(crate) fn try_complete_adapter_pull(&mut self, sender: &str, file_data: &[u8]) -> bool {
        let Some((request_id, service_id)) = self.exchange.pending_pull_from(sender) else {
            return false;
        };
        match self.exchange.complete_adapter_pull(&request_id, file_data) {
            Ok(listing) => {
                let content_hash = listing
                    .artifact
                    .as_ref()
                    .map(|a| a.content_hash.clone())
                    .unwrap_or_default();
                info!(
                    service_id = %service_id,
                    size = file_data.len(),
                    "Adapter artifact verified against attested content hash"
                );
                self.emit_event(Event::AdapterPullCompleted {
                    request_id,
                    service_id,
                    provider_peer_id: sender.to_string(),
                    size_bytes: file_data.len() as u64,
                    content_hash,
                    data: base64_encode(file_data),
                });
            }
            Err(e) => {
                self.exchange.abort_adapter_pull(&request_id);
                warn!(
                    service_id = %service_id,
                    error = %e,
                    "Adapter artifact REJECTED — discarding bytes"
                );
                self.emit_event(Event::AdapterPullRejected {
                    request_id,
                    service_id,
                    provider_peer_id: sender.to_string(),
                    reason: e.to_string(),
                });
            }
        }
        true
    }

    /// Expires timed-out invocations (releasing holds) and overdue usage
    /// waits. Called from the periodic cleanup cycle.
    pub(crate) fn tick_exchange(&mut self) {
        let signer = self.exchange_signer();
        let (outbound, events) = self.exchange.expire_pending(now_ms(), &signer);
        if outbound.is_empty() && events.is_empty() {
            return;
        }
        self.dispatch_exchange_actions(outbound, events);
        self.persist_exchange_state();
    }

    // ========================================================================
    // INTERNAL — PERSISTENCE & HELPERS
    // ========================================================================

    /// Best-effort persistence of durable exchange state (ledger, receipts,
    /// reputation). No-op until a storage backend is attached.
    pub(crate) fn persist_exchange_state(&self) {
        let Some(storage) = self.message_storage.as_ref() else {
            return;
        };
        match serde_json::to_vec(&self.exchange.snapshot()) {
            Ok(bytes) => {
                if let Err(e) = storage.store(
                    storage_keys::EXCHANGE_STATE,
                    storage_keys::EXCHANGE_STATE_ID,
                    &bytes,
                ) {
                    warn!(error = %e, "Failed to persist exchange state");
                }
            }
            Err(e) => warn!(error = %e, "Failed to serialize exchange state"),
        }
    }

    /// Restores durable exchange state from storage, if present.
    pub(crate) fn restore_exchange_state(&mut self) {
        let Some(storage) = self.message_storage.as_ref() else {
            return;
        };
        match storage.load(
            storage_keys::EXCHANGE_STATE,
            storage_keys::EXCHANGE_STATE_ID,
        ) {
            Ok(Some(bytes)) => match serde_json::from_slice::<ExchangeSnapshot>(&bytes) {
                Ok(snapshot) => {
                    self.exchange.restore(snapshot);
                    info!("Restored exchange state (ledger, receipts, reputation)");
                }
                Err(e) => warn!(error = %e, "Failed to deserialize persisted exchange state"),
            },
            Ok(None) => {}
            Err(e) => warn!(error = %e, "Failed to load persisted exchange state"),
        }
    }

    /// A signer over this node's MLS identity key (errors at use when MLS is
    /// not initialized — priced flows cannot reach signing without it).
    pub(crate) fn exchange_signer(&self) -> MlsExchangeSigner {
        MlsExchangeSigner {
            mls: self.mls_manager.clone(),
        }
    }

    /// Whether a confirmed MLS session exists with the peer (the gate for
    /// priced invocations and settlement-bearing messages).
    pub(crate) fn exchange_session_confirmed(&self, peer_id: &str) -> bool {
        matches!(
            self.establishment_state(peer_id),
            Ok(EstablishmentState::SessionConfirmed)
        )
    }
}

/// Wall-clock milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}
