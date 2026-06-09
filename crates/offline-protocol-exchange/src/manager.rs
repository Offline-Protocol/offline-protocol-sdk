//! The exchange state machine.
//!
//! [`ExchangeCore`] owns all exchange state — published listings, the
//! discovered-listing cache, in-flight invocations on both sides, the prepaid
//! ledger, the receipt store, and reputation. Like `MeshServices`, it performs
//! **no I/O**: every method returns the messages to send and the events to
//! emit, and the host (the protocol crate) wires those to transport, the
//! event callback, identity keys, and persistence.
//!
//! ## Priced invocation lifecycle
//!
//! ```text
//! consumer                                          provider
//!   reserve_invocation()    — validate + hold
//!   (host sends __SVC_REQ__)                          handle_request_received()  — billing entry
//!   bind_invocation()                                 [app handles request]
//!                                                     declare_usage()            — metered only
//!                                                     (host sends __SVC_RESP__)
//!                                                     note_responded()           — emits __XCHG_USAGE__ if metered
//!   handle_response_received()
//!     PerCall/Flat: issue receipt, commit hold
//!     metered: wait for __XCHG_USAGE__
//!   handle_usage_message()  — issue receipt, commit hold
//!   (host sends __XCHG_RCPT__, MLS-gated)             handle_receipt_message()   — verify, counter-sign, store
//!   handle_receipt_ack()    — dual-signed             (host sends __XCHG_RCPT_ACK__)
//! ```
//!
//! Receipts on both sides then settle eventually through a
//! [`SettlementBackend`](crate::settlement::SettlementBackend).

use crate::attestation::{attest_listing, verify_listing, ExchangeSigner, ExchangeVerifier};
use crate::error::{ExchangeError, ExchangeResult};
use crate::ledger::{Balance, PrepaidLedger};
use crate::listing::{descriptor_from_discovery, embed_listing, extract_listing};
use crate::receipt::{ReceiptStatus, StoredReceipt, UsageReceipt};
use crate::reputation::{ReputationRead, ReputationTracker};
use crate::settlement::{SettlementBackend, SettlementReport};
use crate::types::{
    ArtifactRef, AttestationStatus, Listing, ListingFilter, ListingKind, Terms, ADAPTER_PULL_METHOD,
};
use offline_protocol_core::ServiceDescriptor;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

/// Prefix for provider→consumer usage declarations (metered billing).
pub const XCHG_USAGE: &str = "__XCHG_USAGE__";
/// Prefix for consumer→provider signed usage receipts.
pub const XCHG_RECEIPT: &str = "__XCHG_RCPT__";
/// Prefix for provider→consumer receipt counter-signatures.
pub const XCHG_RECEIPT_ACK: &str = "__XCHG_RCPT_ACK__";

/// Default time a consumer waits for a response before releasing the hold.
pub const DEFAULT_INVOCATION_TIMEOUT_MS: u64 = 120_000;
/// Default time a consumer waits for a metered usage declaration after the
/// response before falling back to billing a single unit.
pub const DEFAULT_USAGE_WAIT_MS: u64 = 30_000;

/// An exchange control message for the host to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeOutbound {
    /// Recipient peer id.
    pub recipient: String,
    /// Full message content including prefix.
    pub content: String,
    /// When `true`, the host must only send this over a confirmed MLS
    /// session — settlement-bearing messages never ride plaintext.
    pub requires_confirmed_session: bool,
}

/// Events the host surfaces to the application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExchangeEvent {
    /// A listing was discovered (with verification and reputation attached).
    ListingDiscovered {
        /// Discovery query correlation id.
        query_id: String,
        /// The discovered listing.
        listing: Listing,
        /// Outcome of attestation verification.
        attestation_status: AttestationStatus,
        /// Local reputation read for the publisher.
        reputation: ReputationRead,
        /// Hops to the provider (0 = direct neighbor).
        hop_count: u8,
    },
    /// This node (as consumer) issued and signed a usage receipt.
    ReceiptIssued {
        /// The signed receipt.
        receipt: UsageReceipt,
    },
    /// This node (as provider) received, verified, and counter-signed a receipt.
    ReceiptReceived {
        /// The dual-signed receipt.
        receipt: UsageReceipt,
    },
    /// The provider acknowledged (counter-signed) a receipt we issued.
    ReceiptAcknowledged {
        /// The acknowledged receipt id.
        receipt_id: String,
    },
    /// The prepaid balance changed.
    BalanceChanged {
        /// Currency identifier.
        currency: String,
        /// Spendable minor units.
        available_minor: u64,
        /// Minor units held against in-flight invocations.
        held_minor: u64,
    },
    /// A tracked invocation failed (error status, timeout, or refusal).
    InvocationFailed {
        /// The invocation correlation id.
        request_id: String,
        /// Human-readable reason.
        reason: String,
    },
}

/// What the provider-side exchange decided about an incoming request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestDisposition {
    /// The service is not a published listing — the app handles it as a
    /// plain service request.
    NotExchange,
    /// The request targets a published listing; billing is tracked and the
    /// app handles the request normally.
    Tracked,
    /// The request is an adapter pull: the host should respond `ok` and ship
    /// the artifact file, or respond `error` when no artifact path is set.
    AdapterPull {
        /// Local filesystem path of the artifact, when registered.
        artifact_path: Option<String>,
        /// The attested artifact reference.
        artifact: ArtifactRef,
    },
}

/// A discovered listing with its verification outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredListing {
    /// The listing.
    pub listing: Listing,
    /// Attestation verification outcome at discovery time.
    pub attestation_status: AttestationStatus,
    /// Hops to the provider when last seen.
    pub hop_count: u8,
    /// Provider peer id the listing was discovered from.
    pub provider_peer_id: String,
    /// Milliseconds since epoch when last seen.
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublishedListing {
    listing: Listing,
    artifact_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Awaiting {
    Response,
    Usage { deadline_ms: u64 },
}

#[derive(Debug, Clone)]
struct PendingInvocation {
    provider: String,
    service_id: String,
    listing_version: String,
    terms: Terms,
    max_units: u64,
    priced: bool,
    awaiting: Awaiting,
    deadline_ms: u64,
}

#[derive(Debug, Clone)]
struct PendingBilling {
    consumer: String,
    service_id: String,
    declared_units: Option<u64>,
    responded: bool,
}

#[derive(Debug, Clone)]
struct PendingPull {
    provider: String,
    service_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsageDeclarationPayload {
    request_id: String,
    service_id: String,
    unit_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReceiptAckPayload {
    receipt_id: String,
    provider_public_key: String,
    provider_signature: String,
}

/// Durable exchange state, persisted by the host between sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExchangeSnapshot {
    ledger: PrepaidLedger,
    receipts: HashMap<String, StoredReceipt>,
    reputation: ReputationTracker,
}

/// Configuration for the exchange core.
#[derive(Debug, Clone)]
pub struct ExchangeConfig {
    /// How long a consumer waits for a response before releasing the hold.
    pub invocation_timeout_ms: u64,
    /// How long a consumer waits for a metered usage declaration after the
    /// response before billing a single unit.
    pub usage_wait_ms: u64,
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        Self {
            invocation_timeout_ms: DEFAULT_INVOCATION_TIMEOUT_MS,
            usage_wait_ms: DEFAULT_USAGE_WAIT_MS,
        }
    }
}

/// The exchange state machine. See the module docs for the lifecycle.
pub struct ExchangeCore {
    user_id: String,
    config: ExchangeConfig,
    published: HashMap<String, PublishedListing>,
    discovered: HashMap<(String, String), DiscoveredListing>,
    pending_invocations: HashMap<String, PendingInvocation>,
    pending_billing: HashMap<String, PendingBilling>,
    pending_pulls: HashMap<String, PendingPull>,
    receipts: HashMap<String, StoredReceipt>,
    ledger: PrepaidLedger,
    reputation: ReputationTracker,
}

impl ExchangeCore {
    /// Creates an empty exchange for the given local identity.
    pub fn new(user_id: impl Into<String>, config: ExchangeConfig) -> Self {
        Self {
            user_id: user_id.into(),
            config,
            published: HashMap::new(),
            discovered: HashMap::new(),
            pending_invocations: HashMap::new(),
            pending_billing: HashMap::new(),
            pending_pulls: HashMap::new(),
            receipts: HashMap::new(),
            ledger: PrepaidLedger::new(),
            reputation: ReputationTracker::new(),
        }
    }

    // ========================================================================
    // PUBLISH
    // ========================================================================

    /// Builds, attests, and records a listing; returns the listing and the
    /// descriptor (with embedded envelope) to register with service discovery.
    pub fn prepare_publish(
        &mut self,
        descriptor: ServiceDescriptor,
        kind: ListingKind,
        terms: Terms,
        artifact: Option<ArtifactRef>,
        now_ms: u64,
        signer: &dyn ExchangeSigner,
    ) -> ExchangeResult<(Listing, ServiceDescriptor)> {
        terms.validate()?;
        match (kind, &artifact) {
            (ListingKind::Adapter, None) => {
                return Err(ExchangeError::InvalidListing(
                    "adapter listings require an artifact reference".into(),
                ))
            }
            (ListingKind::Service, Some(_)) => {
                return Err(ExchangeError::InvalidListing(
                    "service listings must not carry an artifact reference".into(),
                ))
            }
            _ => {}
        }
        if let Some(ref a) = artifact {
            a.validate()?;
        }
        let attestation = attest_listing(
            &descriptor,
            kind,
            &terms,
            artifact.as_ref(),
            &self.user_id,
            now_ms,
            signer,
        )?;
        let listing = Listing {
            descriptor,
            kind,
            terms,
            artifact,
            publisher: self.user_id.clone(),
            attestation,
        };
        let registered = embed_listing(&listing)?;
        info!(
            service_id = %listing.service_id(),
            kind = listing.kind.as_str(),
            priced = listing.terms.is_priced(),
            "Published attested listing"
        );
        self.published.insert(
            listing.service_id().to_string(),
            PublishedListing {
                listing: listing.clone(),
                artifact_path: None,
            },
        );
        Ok((listing, registered))
    }

    /// Records the local filesystem path of a published adapter's artifact so
    /// pulls can be served.
    pub fn set_artifact_path(
        &mut self,
        service_id: &str,
        path: impl Into<String>,
    ) -> ExchangeResult<()> {
        let entry = self
            .published
            .get_mut(service_id)
            .ok_or_else(|| ExchangeError::UnknownListing(service_id.to_string()))?;
        entry.artifact_path = Some(path.into());
        Ok(())
    }

    /// Removes a published listing. Returns `true` if it existed.
    pub fn unpublish(&mut self, service_id: &str) -> bool {
        self.published.remove(service_id).is_some()
    }

    /// The published listing for a service id, if any.
    pub fn published_listing(&self, service_id: &str) -> Option<&Listing> {
        self.published.get(service_id).map(|p| &p.listing)
    }

    // ========================================================================
    // DISCOVERY
    // ========================================================================

    /// Processes a `ServiceDiscovered` event's fields. Returns a
    /// `ListingDiscovered` event when the descriptor carries a listing
    /// envelope, `None` for plain services.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_discovered(
        &mut self,
        query_id: &str,
        service_id: &str,
        version: &str,
        provider_peer_id: &str,
        capabilities: HashMap<String, String>,
        hop_count: u8,
        now_ms: u64,
        verifier: &dyn ExchangeVerifier,
    ) -> Option<ExchangeEvent> {
        let descriptor = match descriptor_from_discovery(service_id, version, capabilities) {
            Ok(d) => d,
            Err(e) => {
                warn!(service_id = %service_id, error = %e, "Discovered service with invalid descriptor");
                return None;
            }
        };
        let listing = match extract_listing(&descriptor) {
            Ok(Some(listing)) => listing,
            Ok(None) => return None,
            Err(e) => {
                // An envelope was present but malformed — a trust signal
                // against whoever served it.
                warn!(service_id = %service_id, provider = %provider_peer_id, error = %e, "Malformed listing envelope");
                self.reputation
                    .record_invalid_attestation(provider_peer_id, now_ms);
                return None;
            }
        };

        let attestation_status = verify_listing(&listing, verifier);
        match attestation_status {
            AttestationStatus::Verified => {
                self.reputation.record_verified_listing(
                    &listing.publisher,
                    &listing.attestation.public_key,
                    now_ms,
                );
            }
            AttestationStatus::Invalid | AttestationStatus::Unsigned => {
                self.reputation
                    .record_invalid_attestation(&listing.publisher, now_ms);
            }
        }
        // Re-read after recording so a key change observed on THIS listing is
        // reflected in the surfaced reputation.
        let reputation = self.reputation.read(&listing.publisher);

        self.discovered.insert(
            (provider_peer_id.to_string(), service_id.to_string()),
            DiscoveredListing {
                listing: listing.clone(),
                attestation_status,
                hop_count,
                provider_peer_id: provider_peer_id.to_string(),
                last_seen_ms: now_ms,
            },
        );
        debug!(
            service_id = %service_id,
            provider = %provider_peer_id,
            status = attestation_status.as_str(),
            "Discovered listing"
        );
        Some(ExchangeEvent::ListingDiscovered {
            query_id: query_id.to_string(),
            listing,
            attestation_status,
            reputation,
            hop_count,
        })
    }

    /// Discovered listings passing a filter, most recently seen first.
    pub fn discovered_listings(&self, filter: &ListingFilter) -> Vec<DiscoveredListing> {
        let mut results: Vec<DiscoveredListing> = self
            .discovered
            .values()
            .filter(|d| filter.matches(&d.listing))
            .cloned()
            .collect();
        results.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms));
        results
    }

    /// The cached discovered listing from a specific provider.
    pub fn discovered_listing(
        &self,
        provider: &str,
        service_id: &str,
    ) -> Option<&DiscoveredListing> {
        self.discovered
            .get(&(provider.to_string(), service_id.to_string()))
    }

    // ========================================================================
    // INVOCATION — CONSUMER SIDE
    // ========================================================================

    /// Validates an invocation against the discovered listing and places a
    /// hold for the worst-case charge under a reservation id. The host sends
    /// the service request next and binds the real request id with
    /// [`bind_invocation`](Self::bind_invocation), or cancels with
    /// [`cancel_reservation`](Self::cancel_reservation) if the send fails.
    ///
    /// Refusals (all before any message leaves the node):
    /// - unknown listing (must be discovered first)
    /// - priced listing whose attestation did not verify
    /// - priced invocation without a confirmed MLS session
    /// - insufficient prepaid balance for `max_units`
    pub fn reserve_invocation(
        &mut self,
        provider: &str,
        service_id: &str,
        max_units: u64,
        session_confirmed: bool,
        now_ms: u64,
    ) -> ExchangeResult<String> {
        let discovered = self
            .discovered
            .get(&(provider.to_string(), service_id.to_string()))
            .ok_or_else(|| {
                ExchangeError::UnknownListing(format!(
                    "{service_id} from {provider} (not discovered)"
                ))
            })?;
        let listing = &discovered.listing;
        let max_units = max_units.max(1);
        let priced = listing.terms.is_priced();

        if priced {
            if discovered.attestation_status != AttestationStatus::Verified {
                return Err(ExchangeError::AttestationNotVerified(format!(
                    "refusing paid invocation of {service_id}: attestation is {}",
                    discovered.attestation_status.as_str()
                )));
            }
            if !session_confirmed {
                return Err(ExchangeError::EncryptionRequired(format!(
                    "priced invocation of {service_id} requires a confirmed MLS session with {provider}"
                )));
            }
        }

        let reservation_id = format!("rsv-{}", uuid::Uuid::new_v4());
        if priced {
            let hold = listing.terms.total_for_units(max_units)?;
            let currency = listing.terms.currency.clone();
            self.ledger.hold(&reservation_id, &currency, hold)?;
        }
        self.pending_invocations.insert(
            reservation_id.clone(),
            PendingInvocation {
                provider: provider.to_string(),
                service_id: service_id.to_string(),
                listing_version: listing.descriptor.version.clone(),
                terms: listing.terms.clone(),
                max_units,
                priced,
                awaiting: Awaiting::Response,
                deadline_ms: now_ms.saturating_add(self.config.invocation_timeout_ms),
            },
        );
        Ok(reservation_id)
    }

    /// Binds a reservation to the request id returned by the service layer.
    pub fn bind_invocation(
        &mut self,
        reservation_id: &str,
        request_id: &str,
    ) -> ExchangeResult<()> {
        let pending = self
            .pending_invocations
            .remove(reservation_id)
            .ok_or_else(|| ExchangeError::UnknownInvocation(reservation_id.to_string()))?;
        if pending.priced {
            self.ledger.rebind_hold(reservation_id, request_id)?;
        }
        self.pending_invocations
            .insert(request_id.to_string(), pending);
        Ok(())
    }

    /// Cancels a reservation whose request never left the node.
    pub fn cancel_reservation(&mut self, reservation_id: &str) {
        if let Some(pending) = self.pending_invocations.remove(reservation_id) {
            if pending.priced {
                let _ = self.ledger.release(reservation_id);
            }
        }
    }

    /// Processes a service response for a tracked invocation. Returns the
    /// messages to send and events to emit; both empty when the response is
    /// not exchange-tracked.
    pub fn handle_response_received(
        &mut self,
        request_id: &str,
        status: &str,
        now_ms: u64,
        signer: &dyn ExchangeSigner,
    ) -> (Vec<ExchangeOutbound>, Vec<ExchangeEvent>) {
        let Some(pending) = self.pending_invocations.get(request_id).cloned() else {
            return (Vec::new(), Vec::new());
        };

        if status != "ok" {
            self.pending_invocations.remove(request_id);
            let mut events = Vec::new();
            if pending.priced {
                let _ = self.ledger.release(request_id);
                events.push(self.balance_event(&pending.terms.currency));
            }
            events.push(ExchangeEvent::InvocationFailed {
                request_id: request_id.to_string(),
                reason: format!("provider responded with status '{status}'"),
            });
            return (Vec::new(), events);
        }

        if !pending.priced {
            self.pending_invocations.remove(request_id);
            return (Vec::new(), Vec::new());
        }

        if pending.terms.unit.is_metered() {
            // Wait for the provider's usage declaration before billing.
            if let Some(entry) = self.pending_invocations.get_mut(request_id) {
                entry.awaiting = Awaiting::Usage {
                    deadline_ms: now_ms.saturating_add(self.config.usage_wait_ms),
                };
            }
            return (Vec::new(), Vec::new());
        }

        // PerCall / Flat: exactly one unit.
        self.finalize_invocation(request_id, &pending, 1, now_ms, signer)
    }

    /// Processes a provider's usage declaration for a metered invocation.
    pub fn handle_usage_message(
        &mut self,
        sender: &str,
        data: &str,
        now_ms: u64,
        signer: &dyn ExchangeSigner,
    ) -> (Vec<ExchangeOutbound>, Vec<ExchangeEvent>) {
        let Ok(payload) = serde_json::from_str::<UsageDeclarationPayload>(data) else {
            warn!(sender = %sender, "Failed to parse usage declaration");
            return (Vec::new(), Vec::new());
        };
        let Some(pending) = self.pending_invocations.get(&payload.request_id).cloned() else {
            debug!(request_id = %payload.request_id, "Usage declaration for unknown invocation");
            return (Vec::new(), Vec::new());
        };
        if pending.provider != sender {
            warn!(
                sender = %sender,
                expected = %pending.provider,
                "Usage declaration from wrong peer, ignoring"
            );
            return (Vec::new(), Vec::new());
        }
        if !matches!(pending.awaiting, Awaiting::Usage { .. }) {
            debug!(request_id = %payload.request_id, "Usage declaration before response, ignoring");
            return (Vec::new(), Vec::new());
        }
        // The hold bounds what the consumer agreed to pay; clamp the declared
        // units so a provider can never charge beyond it.
        let units = payload.unit_count.clamp(1, pending.max_units);
        self.finalize_invocation(&payload.request_id, &pending, units, now_ms, signer)
    }

    /// Issues the receipt, commits the hold, and produces the receipt message.
    fn finalize_invocation(
        &mut self,
        request_id: &str,
        pending: &PendingInvocation,
        units: u64,
        now_ms: u64,
        signer: &dyn ExchangeSigner,
    ) -> (Vec<ExchangeOutbound>, Vec<ExchangeEvent>) {
        self.pending_invocations.remove(request_id);
        let receipt = match UsageReceipt::issue(
            format!("rcpt-{}", uuid::Uuid::new_v4()),
            request_id.to_string(),
            pending.service_id.clone(),
            pending.listing_version.clone(),
            &pending.terms,
            units,
            self.user_id.clone(),
            pending.provider.clone(),
            now_ms,
            signer,
        ) {
            Ok(r) => r,
            Err(e) => {
                warn!(request_id = %request_id, error = %e, "Failed to issue receipt — releasing hold");
                let _ = self.ledger.release(request_id);
                return (
                    Vec::new(),
                    vec![
                        self.balance_event(&pending.terms.currency),
                        ExchangeEvent::InvocationFailed {
                            request_id: request_id.to_string(),
                            reason: format!("failed to issue receipt: {e}"),
                        },
                    ],
                );
            }
        };
        if let Err(e) = self.ledger.commit(request_id, receipt.total_minor) {
            warn!(request_id = %request_id, error = %e, "Failed to commit hold");
        }
        self.receipts.insert(
            receipt.receipt_id.clone(),
            StoredReceipt {
                receipt: receipt.clone(),
                status: ReceiptStatus::PendingSettlement,
                local_role_consumer: true,
            },
        );
        let content = match serde_json::to_string(&receipt) {
            Ok(json) => format!("{XCHG_RECEIPT}{json}"),
            Err(e) => {
                warn!(error = %e, "Failed to serialize receipt");
                return (Vec::new(), vec![ExchangeEvent::ReceiptIssued { receipt }]);
            }
        };
        info!(
            request_id = %request_id,
            units,
            total_minor = receipt.total_minor,
            "Issued usage receipt"
        );
        (
            vec![ExchangeOutbound {
                recipient: pending.provider.clone(),
                content,
                requires_confirmed_session: true,
            }],
            vec![
                ExchangeEvent::ReceiptIssued { receipt },
                self.balance_event(&pending.terms.currency),
            ],
        )
    }

    /// Processes a provider's receipt acknowledgement (counter-signature).
    pub fn handle_receipt_ack(
        &mut self,
        sender: &str,
        data: &str,
        verifier: &dyn ExchangeVerifier,
    ) -> Vec<ExchangeEvent> {
        let Ok(payload) = serde_json::from_str::<ReceiptAckPayload>(data) else {
            warn!(sender = %sender, "Failed to parse receipt ack");
            return Vec::new();
        };
        let Some(stored) = self.receipts.get_mut(&payload.receipt_id) else {
            debug!(receipt_id = %payload.receipt_id, "Ack for unknown receipt");
            return Vec::new();
        };
        if !stored.local_role_consumer || stored.receipt.provider_id != sender {
            warn!(sender = %sender, receipt_id = %payload.receipt_id, "Receipt ack from wrong peer");
            return Vec::new();
        }
        let mut candidate = stored.receipt.clone();
        candidate.provider_public_key = payload.provider_public_key;
        candidate.provider_signature = payload.provider_signature;
        if !candidate.verify_provider_signature(verifier) {
            warn!(receipt_id = %payload.receipt_id, "Invalid provider counter-signature on ack");
            return Vec::new();
        }
        stored.receipt = candidate;
        info!(receipt_id = %payload.receipt_id, "Receipt counter-signed by provider");
        vec![ExchangeEvent::ReceiptAcknowledged {
            receipt_id: payload.receipt_id,
        }]
    }

    // ========================================================================
    // INVOCATION — PROVIDER SIDE
    // ========================================================================

    /// Classifies an incoming service request against published listings and
    /// tracks billing for priced ones.
    pub fn handle_request_received(
        &mut self,
        request_id: &str,
        service_id: &str,
        method: &str,
        sender: &str,
    ) -> RequestDisposition {
        let Some(published) = self.published.get(service_id) else {
            return RequestDisposition::NotExchange;
        };
        if method == ADAPTER_PULL_METHOD {
            if let Some(artifact) = published.listing.artifact.clone() {
                return RequestDisposition::AdapterPull {
                    artifact_path: published.artifact_path.clone(),
                    artifact,
                };
            }
        }
        if published.listing.terms.is_priced() {
            self.pending_billing.insert(
                request_id.to_string(),
                PendingBilling {
                    consumer: sender.to_string(),
                    service_id: service_id.to_string(),
                    declared_units: None,
                    responded: false,
                },
            );
        }
        RequestDisposition::Tracked
    }

    /// Declares the units consumed by a metered invocation. Must be called
    /// before the response is sent; the declaration ships right after it.
    pub fn declare_usage(&mut self, request_id: &str, units: u64) -> ExchangeResult<()> {
        let billing = self
            .pending_billing
            .get_mut(request_id)
            .ok_or_else(|| ExchangeError::UnknownInvocation(request_id.to_string()))?;
        if billing.responded {
            return Err(ExchangeError::InvalidReceipt(
                "usage must be declared before responding".into(),
            ));
        }
        billing.declared_units = Some(units.max(1));
        Ok(())
    }

    /// Notes that the app responded to a tracked request. For metered
    /// listings this produces the usage declaration to send.
    pub fn note_responded(&mut self, request_id: &str) -> Option<ExchangeOutbound> {
        let billing = self.pending_billing.get_mut(request_id)?;
        billing.responded = true;
        let service_id = billing.service_id.clone();
        let consumer = billing.consumer.clone();
        let declared = billing.declared_units;
        let metered = self
            .published
            .get(&service_id)
            .is_some_and(|p| p.listing.terms.unit.is_metered());
        if !metered {
            return None;
        }
        let payload = UsageDeclarationPayload {
            request_id: request_id.to_string(),
            service_id,
            unit_count: declared.unwrap_or(1),
        };
        let json = serde_json::to_string(&payload).ok()?;
        Some(ExchangeOutbound {
            recipient: consumer,
            content: format!("{XCHG_USAGE}{json}"),
            requires_confirmed_session: true,
        })
    }

    /// Processes a consumer's signed receipt: validates it against the
    /// published terms and the tracked billing entry, verifies the consumer
    /// signature, counter-signs, stores it, and produces the ack.
    pub fn handle_receipt_message(
        &mut self,
        sender: &str,
        data: &str,
        signer: &dyn ExchangeSigner,
        verifier: &dyn ExchangeVerifier,
    ) -> (Vec<ExchangeOutbound>, Vec<ExchangeEvent>) {
        let Ok(mut receipt) = serde_json::from_str::<UsageReceipt>(data) else {
            warn!(sender = %sender, "Failed to parse receipt");
            return (Vec::new(), Vec::new());
        };
        let Some(billing) = self.pending_billing.get(&receipt.request_id) else {
            warn!(request_id = %receipt.request_id, "Receipt for untracked invocation, ignoring");
            return (Vec::new(), Vec::new());
        };
        if billing.consumer != sender {
            warn!(sender = %sender, expected = %billing.consumer, "Receipt from wrong peer");
            return (Vec::new(), Vec::new());
        }
        if billing.service_id != receipt.service_id {
            warn!(
                receipt_service = %receipt.service_id,
                billing_service = %billing.service_id,
                "Receipt service mismatch"
            );
            return (Vec::new(), Vec::new());
        }
        let Some(published) = self.published.get(&receipt.service_id) else {
            warn!(service_id = %receipt.service_id, "Receipt for unpublished listing");
            return (Vec::new(), Vec::new());
        };
        if receipt.provider_id != self.user_id || receipt.consumer_id != sender {
            warn!(receipt_id = %receipt.receipt_id, "Receipt identity fields do not match parties");
            return (Vec::new(), Vec::new());
        }
        if let Err(e) = receipt.validate_against_terms(&published.listing.terms) {
            warn!(receipt_id = %receipt.receipt_id, error = %e, "Receipt fails terms validation");
            return (Vec::new(), Vec::new());
        }
        if let Some(declared) = billing.declared_units {
            // The consumer clamps to its max_units; accept at most what was
            // declared, never more.
            if receipt.unit_count > declared {
                warn!(
                    receipt_id = %receipt.receipt_id,
                    declared,
                    billed = receipt.unit_count,
                    "Receipt bills more units than declared"
                );
                return (Vec::new(), Vec::new());
            }
        }
        if !receipt.verify_consumer_signature(verifier) {
            warn!(receipt_id = %receipt.receipt_id, "Invalid consumer signature on receipt");
            return (Vec::new(), Vec::new());
        }
        if let Err(e) = receipt.counter_sign(signer) {
            warn!(receipt_id = %receipt.receipt_id, error = %e, "Failed to counter-sign receipt");
            return (Vec::new(), Vec::new());
        }
        self.pending_billing.remove(&receipt.request_id);
        let ack = ReceiptAckPayload {
            receipt_id: receipt.receipt_id.clone(),
            provider_public_key: receipt.provider_public_key.clone(),
            provider_signature: receipt.provider_signature.clone(),
        };
        let outbound = serde_json::to_string(&ack)
            .ok()
            .map(|json| ExchangeOutbound {
                recipient: sender.to_string(),
                content: format!("{XCHG_RECEIPT_ACK}{json}"),
                requires_confirmed_session: true,
            });
        self.receipts.insert(
            receipt.receipt_id.clone(),
            StoredReceipt {
                receipt: receipt.clone(),
                status: ReceiptStatus::PendingSettlement,
                local_role_consumer: false,
            },
        );
        info!(
            receipt_id = %receipt.receipt_id,
            total_minor = receipt.total_minor,
            "Receipt verified and counter-signed"
        );
        (
            outbound.into_iter().collect(),
            vec![ExchangeEvent::ReceiptReceived { receipt }],
        )
    }

    // ========================================================================
    // ADAPTER PULLS — CONSUMER SIDE
    // ========================================================================

    /// Validates an adapter pull against the discovered listing. Pulls are
    /// free but the attestation must verify — an unverifiable adapter is
    /// unloadable, so pulling it is pointless and unsafe.
    pub fn validate_adapter_pull(&self, provider: &str, service_id: &str) -> ExchangeResult<()> {
        let discovered = self
            .discovered
            .get(&(provider.to_string(), service_id.to_string()))
            .ok_or_else(|| {
                ExchangeError::UnknownListing(format!(
                    "{service_id} from {provider} (not discovered)"
                ))
            })?;
        if discovered.listing.kind != ListingKind::Adapter {
            return Err(ExchangeError::InvalidListing(format!(
                "{service_id} is not an adapter listing"
            )));
        }
        if discovered.attestation_status != AttestationStatus::Verified {
            return Err(ExchangeError::AttestationNotVerified(format!(
                "refusing adapter pull of {service_id}: attestation is {}",
                discovered.attestation_status.as_str()
            )));
        }
        Ok(())
    }

    /// Tracks an in-flight adapter pull by its request id.
    pub fn register_adapter_pull(&mut self, request_id: &str, provider: &str, service_id: &str) {
        self.pending_pulls.insert(
            request_id.to_string(),
            PendingPull {
                provider: provider.to_string(),
                service_id: service_id.to_string(),
            },
        );
    }

    /// The pending pull from a sender, if any (used to match incoming file
    /// transfers to pulls).
    pub fn pending_pull_from(&self, sender: &str) -> Option<(String, String)> {
        self.pending_pulls
            .iter()
            .find(|(_, p)| p.provider == sender)
            .map(|(request_id, p)| (request_id.clone(), p.service_id.clone()))
    }

    /// Completes an adapter pull: verifies the received bytes against the
    /// attested artifact reference and returns the listing for the runtime
    /// gate. A hash or size mismatch rejects the artifact.
    pub fn complete_adapter_pull(
        &mut self,
        request_id: &str,
        bytes: &[u8],
    ) -> ExchangeResult<Listing> {
        let pull = self
            .pending_pulls
            .get(request_id)
            .ok_or_else(|| ExchangeError::UnknownInvocation(request_id.to_string()))?
            .clone();
        let discovered = self
            .discovered
            .get(&(pull.provider.clone(), pull.service_id.clone()))
            .ok_or_else(|| ExchangeError::UnknownListing(pull.service_id.clone()))?;
        let artifact = discovered.listing.artifact.clone().ok_or_else(|| {
            ExchangeError::InvalidListing(format!("{} has no artifact", pull.service_id))
        })?;
        crate::artifact::verify_artifact(bytes, &artifact)?;
        self.pending_pulls.remove(request_id);
        Ok(discovered.listing.clone())
    }

    /// Drops a pending pull (failed transfer or rejected artifact).
    pub fn abort_adapter_pull(&mut self, request_id: &str) {
        self.pending_pulls.remove(request_id);
    }

    // ========================================================================
    // BALANCE, RECEIPTS, SETTLEMENT
    // ========================================================================

    /// Credits the prepaid balance (funding confirmed out-of-band).
    pub fn credit_balance(
        &mut self,
        currency: &str,
        amount_minor: u64,
    ) -> ExchangeResult<ExchangeEvent> {
        self.ledger.credit(currency, amount_minor)?;
        Ok(self.balance_event(currency))
    }

    /// The prepaid balance for a currency.
    pub fn balance(&self, currency: &str) -> Balance {
        self.ledger.balance(currency)
    }

    /// All stored receipts.
    pub fn receipts(&self) -> Vec<StoredReceipt> {
        self.receipts.values().cloned().collect()
    }

    /// Receipts awaiting settlement.
    pub fn pending_receipts(&self) -> Vec<UsageReceipt> {
        self.receipts
            .values()
            .filter(|s| s.status == ReceiptStatus::PendingSettlement)
            .map(|s| s.receipt.clone())
            .collect()
    }

    /// Marks receipts settled (after a clearing backend confirms) and updates
    /// publisher reputation for receipts where this node was the consumer.
    pub fn mark_receipts_settled(&mut self, receipt_ids: &[String], now_ms: u64) {
        for id in receipt_ids {
            if let Some(stored) = self.receipts.get_mut(id) {
                stored.status = ReceiptStatus::Settled;
                if stored.local_role_consumer {
                    let provider = stored.receipt.provider_id.clone();
                    self.reputation.record_settled_receipt(&provider, now_ms);
                }
            }
        }
    }

    /// Marks receipts rejected by a clearing backend.
    pub fn mark_receipts_rejected(&mut self, receipt_ids: &[String]) {
        for id in receipt_ids {
            if let Some(stored) = self.receipts.get_mut(id) {
                stored.status = ReceiptStatus::Rejected;
            }
        }
    }

    /// Submits all pending receipts to a settlement backend and applies the
    /// outcome to the local store.
    pub fn reconcile(
        &mut self,
        backend: &dyn SettlementBackend,
        now_ms: u64,
    ) -> ExchangeResult<SettlementReport> {
        let pending = self.pending_receipts();
        if pending.is_empty() {
            return Ok(SettlementReport::default());
        }
        let report = backend.submit_receipts(&pending)?;
        self.mark_receipts_settled(&report.settled, now_ms);
        let rejected_ids: Vec<String> = report.rejected.iter().map(|(id, _)| id.clone()).collect();
        self.mark_receipts_rejected(&rejected_ids);
        info!(
            backend = backend.backend_id(),
            settled = report.settled.len(),
            rejected = report.rejected.len(),
            "Reconciled receipts with settlement backend"
        );
        Ok(report)
    }

    /// The local reputation read for a publisher.
    pub fn reputation(&self, publisher: &str) -> ReputationRead {
        self.reputation.read(publisher)
    }

    // ========================================================================
    // HOUSEKEEPING & PERSISTENCE
    // ========================================================================

    /// Expires timed-out invocations: releases holds for invocations that
    /// never got a response, and bills a single unit for metered invocations
    /// whose usage declaration never arrived (the response did arrive, so at
    /// least one unit is owed).
    pub fn expire_pending(
        &mut self,
        now_ms: u64,
        signer: &dyn ExchangeSigner,
    ) -> (Vec<ExchangeOutbound>, Vec<ExchangeEvent>) {
        let mut outbound = Vec::new();
        let mut events = Vec::new();

        let expired: Vec<(String, PendingInvocation)> = self
            .pending_invocations
            .iter()
            .filter(|(_, p)| match p.awaiting {
                Awaiting::Response => now_ms > p.deadline_ms,
                Awaiting::Usage { deadline_ms } => now_ms > deadline_ms,
            })
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect();

        for (request_id, pending) in expired {
            match pending.awaiting {
                Awaiting::Response => {
                    self.pending_invocations.remove(&request_id);
                    if pending.priced {
                        let _ = self.ledger.release(&request_id);
                        events.push(self.balance_event(&pending.terms.currency));
                    }
                    events.push(ExchangeEvent::InvocationFailed {
                        request_id,
                        reason: "invocation timed out without a response".into(),
                    });
                }
                Awaiting::Usage { .. } => {
                    let (mut o, mut e) =
                        self.finalize_invocation(&request_id, &pending, 1, now_ms, signer);
                    outbound.append(&mut o);
                    events.append(&mut e);
                }
            }
        }
        (outbound, events)
    }

    /// Durable state for persistence (ledger, receipts, reputation).
    /// Published and discovered listings are runtime state: hosts re-publish
    /// on startup, and discovery refreshes the cache.
    pub fn snapshot(&self) -> ExchangeSnapshot {
        ExchangeSnapshot {
            ledger: self.ledger.clone(),
            receipts: self.receipts.clone(),
            reputation: self.reputation.clone(),
        }
    }

    /// Restores durable state from a snapshot.
    pub fn restore(&mut self, snapshot: ExchangeSnapshot) {
        self.ledger = snapshot.ledger;
        self.receipts = snapshot.receipts;
        self.reputation = snapshot.reputation;
    }

    fn balance_event(&self, currency: &str) -> ExchangeEvent {
        let balance = self.ledger.balance(currency);
        ExchangeEvent::BalanceChanged {
            currency: currency.to_string(),
            available_minor: balance.available_minor,
            held_minor: balance.held_minor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attestation::test_signer::{DalekSigner, DalekVerifier};
    use crate::settlement::MockClearing;
    use crate::types::{BillingUnit, ChunkPlan, Price};
    use offline_protocol_core::ServiceId;

    const NOW: u64 = 1_000_000;

    fn descriptor(id: &str) -> ServiceDescriptor {
        ServiceDescriptor {
            service_id: ServiceId::new(id).unwrap(),
            version: "1.0".into(),
            capabilities: HashMap::new(),
        }
    }

    fn priced_terms(amount: u64, unit: BillingUnit) -> Terms {
        Terms {
            price: Some(Price {
                amount_minor: amount,
            }),
            unit,
            currency: "USD".into(),
            max_payload_kb: 64,
        }
    }

    /// Publishes on the provider core and feeds the discovery result into the
    /// consumer core, returning the emitted event.
    fn publish_and_discover(
        provider: &mut ExchangeCore,
        consumer: &mut ExchangeCore,
        provider_signer: &DalekSigner,
        service_id: &str,
        kind: ListingKind,
        terms: Terms,
        artifact: Option<ArtifactRef>,
    ) -> Option<ExchangeEvent> {
        let (_, registered) = provider
            .prepare_publish(
                descriptor(service_id),
                kind,
                terms,
                artifact,
                NOW,
                provider_signer,
            )
            .unwrap();
        consumer.handle_discovered(
            "q-1",
            service_id,
            "1.0",
            "bob",
            registered.capabilities.clone(),
            1,
            NOW,
            &DalekVerifier,
        )
    }

    fn cores() -> (ExchangeCore, ExchangeCore, DalekSigner, DalekSigner) {
        (
            ExchangeCore::new("alice", ExchangeConfig::default()),
            ExchangeCore::new("bob", ExchangeConfig::default()),
            DalekSigner::new(1), // alice (consumer)
            DalekSigner::new(2), // bob (provider)
        )
    }

    #[test]
    fn publish_discover_surfaces_price_attestation_reputation() {
        let (mut alice, mut bob, _alice_key, bob_key) = cores();
        let event = publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(5, BillingUnit::PerCall),
            None,
        )
        .expect("listing event");
        match event {
            ExchangeEvent::ListingDiscovered {
                listing,
                attestation_status,
                reputation,
                ..
            } => {
                assert_eq!(listing.terms.unit_price_minor(), 5);
                assert_eq!(attestation_status, AttestationStatus::Verified);
                assert_eq!(reputation.level, crate::reputation::ReputationLevel::New);
                assert_eq!(listing.publisher, "bob");
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn plain_service_discovery_yields_no_listing_event() {
        let (mut alice, _, _, _) = cores();
        let event = alice.handle_discovered(
            "q-1",
            "plain.v1",
            "1.0",
            "bob",
            HashMap::new(),
            0,
            NOW,
            &DalekVerifier,
        );
        assert!(event.is_none());
    }

    #[test]
    fn tampered_listing_surfaces_invalid_and_flags_publisher() {
        let (mut alice, mut bob, _, bob_key) = cores();
        let (_, mut registered) = bob
            .prepare_publish(
                descriptor("weather.v1"),
                ListingKind::Service,
                priced_terms(5, BillingUnit::PerCall),
                None,
                NOW,
                &bob_key,
            )
            .unwrap();
        // Tamper with the embedded price.
        let key = crate::types::LISTING_CAPABILITY_KEY;
        let json = registered.capabilities.get(key).unwrap().clone();
        registered.capabilities.insert(
            key.into(),
            json.replace("\"amount_minor\":5", "\"amount_minor\":1"),
        );

        let event = alice
            .handle_discovered(
                "q-1",
                "weather.v1",
                "1.0",
                "bob",
                registered.capabilities,
                0,
                NOW,
                &DalekVerifier,
            )
            .unwrap();
        match event {
            ExchangeEvent::ListingDiscovered {
                attestation_status,
                reputation,
                ..
            } => {
                assert_eq!(attestation_status, AttestationStatus::Invalid);
                assert_eq!(
                    reputation.level,
                    crate::reputation::ReputationLevel::Flagged
                );
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn priced_invocation_requires_confirmed_session() {
        let (mut alice, mut bob, _, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(5, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let err = alice
            .reserve_invocation("bob", "weather.v1", 1, false, NOW)
            .unwrap_err();
        assert!(matches!(err, ExchangeError::EncryptionRequired(_)));
    }

    #[test]
    fn priced_invocation_requires_verified_attestation() {
        let (mut alice, mut bob, _, bob_key) = cores();
        let (_, mut registered) = bob
            .prepare_publish(
                descriptor("weather.v1"),
                ListingKind::Service,
                priced_terms(5, BillingUnit::PerCall),
                None,
                NOW,
                &bob_key,
            )
            .unwrap();
        let key = crate::types::LISTING_CAPABILITY_KEY;
        let json = registered.capabilities.get(key).unwrap().clone();
        registered.capabilities.insert(
            key.into(),
            json.replace("\"amount_minor\":5", "\"amount_minor\":1"),
        );
        alice.handle_discovered(
            "q-1",
            "weather.v1",
            "1.0",
            "bob",
            registered.capabilities,
            0,
            NOW,
            &DalekVerifier,
        );
        alice.credit_balance("USD", 100).unwrap();
        let err = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap_err();
        assert!(matches!(err, ExchangeError::AttestationNotVerified(_)));
    }

    #[test]
    fn priced_invocation_requires_balance() {
        let (mut alice, mut bob, _, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(5, BillingUnit::PerCall),
            None,
        );
        let err = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap_err();
        assert!(matches!(err, ExchangeError::InsufficientBalance { .. }));
    }

    #[test]
    fn full_per_call_loop_with_settlement() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(40, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();

        // Consumer reserves and binds.
        let rsv = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap();
        assert_eq!(alice.balance("USD").available_minor, 60);
        assert_eq!(alice.balance("USD").held_minor, 40);
        alice.bind_invocation(&rsv, "req-1").unwrap();

        // Provider tracks the request and responds.
        let disposition = bob.handle_request_received("req-1", "weather.v1", "get", "alice");
        assert_eq!(disposition, RequestDisposition::Tracked);
        assert!(bob.note_responded("req-1").is_none()); // per-call: no usage msg

        // Consumer processes the response: receipt issued, hold committed.
        let (outbound, events) = alice.handle_response_received("req-1", "ok", NOW + 1, &alice_key);
        assert_eq!(outbound.len(), 1);
        assert!(outbound[0].requires_confirmed_session);
        assert!(outbound[0].content.starts_with(XCHG_RECEIPT));
        assert_eq!(alice.balance("USD").available_minor, 60);
        assert_eq!(alice.balance("USD").held_minor, 0);
        let receipt = events
            .iter()
            .find_map(|e| match e {
                ExchangeEvent::ReceiptIssued { receipt } => Some(receipt.clone()),
                _ => None,
            })
            .expect("receipt issued");
        assert_eq!(receipt.total_minor, 40);

        // Provider verifies, counter-signs, acks.
        let data = outbound[0].content.strip_prefix(XCHG_RECEIPT).unwrap();
        let (acks, provider_events) =
            bob.handle_receipt_message("alice", data, &bob_key, &DalekVerifier);
        assert_eq!(acks.len(), 1);
        assert!(matches!(
            provider_events[0],
            ExchangeEvent::ReceiptReceived { .. }
        ));

        // Consumer attaches the counter-signature.
        let ack_data = acks[0].content.strip_prefix(XCHG_RECEIPT_ACK).unwrap();
        let ack_events = alice.handle_receipt_ack("bob", ack_data, &DalekVerifier);
        assert!(matches!(
            ack_events[0],
            ExchangeEvent::ReceiptAcknowledged { .. }
        ));

        // Both sides settle with the clearing backend (2.5% fee).
        let backend = MockClearing::new(250);
        let report = alice.reconcile(&backend, NOW + 2).unwrap();
        assert_eq!(report.settled.len(), 1);
        let report = bob.reconcile(&backend, NOW + 2).unwrap();
        assert_eq!(report.settled.len(), 1); // idempotent for the same receipt id
        assert_eq!(backend.collected_fees("USD"), 1); // ceil(40 * 0.025)
        assert_eq!(backend.account_balance("bob", "USD"), 39);

        // Consumer's settled history establishes the publisher.
        assert_eq!(
            alice.reputation("bob").level,
            crate::reputation::ReputationLevel::Established
        );
    }

    #[test]
    fn metered_loop_bills_declared_units_clamped_to_max() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "llm.v1",
            ListingKind::Service,
            priced_terms(10, BillingUnit::PerToken),
            None,
        );
        alice.credit_balance("USD", 1000).unwrap();

        let rsv = alice
            .reserve_invocation("bob", "llm.v1", 5, true, NOW)
            .unwrap();
        assert_eq!(alice.balance("USD").held_minor, 50);
        alice.bind_invocation(&rsv, "req-1").unwrap();

        bob.handle_request_received("req-1", "llm.v1", "generate", "alice");
        bob.declare_usage("req-1", 3).unwrap();
        let usage = bob.note_responded("req-1").expect("usage declaration");
        assert!(usage.content.starts_with(XCHG_USAGE));
        assert!(usage.requires_confirmed_session);

        // Response arrives first; consumer waits for usage.
        let (outbound, _) = alice.handle_response_received("req-1", "ok", NOW + 1, &alice_key);
        assert!(outbound.is_empty());
        assert_eq!(alice.balance("USD").held_minor, 50);

        // Usage arrives: bill 3 units (30), return 20 to available.
        let data = usage.content.strip_prefix(XCHG_USAGE).unwrap();
        let (outbound, events) = alice.handle_usage_message("bob", data, NOW + 2, &alice_key);
        assert_eq!(outbound.len(), 1);
        let receipt = events
            .iter()
            .find_map(|e| match e {
                ExchangeEvent::ReceiptIssued { receipt } => Some(receipt.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(receipt.unit_count, 3);
        assert_eq!(receipt.total_minor, 30);
        assert_eq!(alice.balance("USD").available_minor, 970);
        assert_eq!(alice.balance("USD").held_minor, 0);

        // Provider accepts: 3 units were declared.
        let rdata = outbound[0].content.strip_prefix(XCHG_RECEIPT).unwrap();
        let (acks, _) = bob.handle_receipt_message("alice", rdata, &bob_key, &DalekVerifier);
        assert_eq!(acks.len(), 1);
    }

    #[test]
    fn over_declared_usage_clamps_to_consumer_max() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "llm.v1",
            ListingKind::Service,
            priced_terms(10, BillingUnit::PerToken),
            None,
        );
        alice.credit_balance("USD", 1000).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "llm.v1", 2, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        bob.handle_request_received("req-1", "llm.v1", "generate", "alice");
        bob.declare_usage("req-1", 100).unwrap();
        let usage = bob.note_responded("req-1").unwrap();
        alice.handle_response_received("req-1", "ok", NOW, &alice_key);
        let data = usage.content.strip_prefix(XCHG_USAGE).unwrap();
        let (_, events) = alice.handle_usage_message("bob", data, NOW, &alice_key);
        let receipt = events
            .iter()
            .find_map(|e| match e {
                ExchangeEvent::ReceiptIssued { receipt } => Some(receipt.clone()),
                _ => None,
            })
            .unwrap();
        // Charge bounded by the consumer's agreed max (2 units), never 100.
        assert_eq!(receipt.unit_count, 2);
        assert_eq!(receipt.total_minor, 20);
    }

    #[test]
    fn provider_rejects_receipt_with_wrong_price() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(40, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        bob.handle_request_received("req-1", "weather.v1", "get", "alice");
        let (outbound, _) = alice.handle_response_received("req-1", "ok", NOW, &alice_key);
        let data = outbound[0].content.strip_prefix(XCHG_RECEIPT).unwrap();

        // Tamper: lower the total (signature breaks AND terms mismatch).
        let tampered = data.replace("\"total_minor\":40", "\"total_minor\":4");
        let (acks, events) =
            bob.handle_receipt_message("alice", &tampered, &bob_key, &DalekVerifier);
        assert!(acks.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn error_response_releases_hold() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(40, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        let (outbound, events) = alice.handle_response_received("req-1", "error", NOW, &alice_key);
        assert!(outbound.is_empty());
        assert!(events
            .iter()
            .any(|e| matches!(e, ExchangeEvent::InvocationFailed { .. })));
        assert_eq!(alice.balance("USD").available_minor, 100);
        assert_eq!(alice.balance("USD").held_minor, 0);
    }

    #[test]
    fn response_timeout_releases_hold() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(40, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();

        let late = NOW + DEFAULT_INVOCATION_TIMEOUT_MS + 1;
        let (_, events) = alice.expire_pending(late, &alice_key);
        assert!(events
            .iter()
            .any(|e| matches!(e, ExchangeEvent::InvocationFailed { .. })));
        assert_eq!(alice.balance("USD").available_minor, 100);
    }

    #[test]
    fn missing_usage_declaration_bills_one_unit_after_wait() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "llm.v1",
            ListingKind::Service,
            priced_terms(10, BillingUnit::PerToken),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "llm.v1", 5, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        alice.handle_response_received("req-1", "ok", NOW, &alice_key);

        let late = NOW + DEFAULT_USAGE_WAIT_MS + 1;
        let (outbound, events) = alice.expire_pending(late, &alice_key);
        assert_eq!(outbound.len(), 1); // receipt still sent to provider
        let receipt = events
            .iter()
            .find_map(|e| match e {
                ExchangeEvent::ReceiptIssued { receipt } => Some(receipt.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(receipt.unit_count, 1);
        assert_eq!(alice.balance("USD").available_minor, 90);
    }

    #[test]
    fn free_listing_invocation_needs_no_session_or_balance() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "wiki.v1",
            ListingKind::Service,
            Terms::free(),
            None,
        );
        let rsv = alice
            .reserve_invocation("bob", "wiki.v1", 1, false, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        let (outbound, events) = alice.handle_response_received("req-1", "ok", NOW, &alice_key);
        assert!(outbound.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn adapter_pull_verifies_hash_and_rejects_mismatch() {
        let (mut alice, mut bob, _, bob_key) = cores();
        let bytes = b"adapter-weights".to_vec();
        let artifact = ArtifactRef {
            content_hash: crate::artifact::content_hash(&bytes),
            size_bytes: bytes.len() as u64,
            base_model: "gemma-3-1b".into(),
            base_model_version: "1.0".into(),
            chunking: ChunkPlan {
                chunk_size_bytes: 4096,
            },
        };
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "adapter.medical",
            ListingKind::Adapter,
            Terms::free(),
            Some(artifact),
        );
        bob.set_artifact_path("adapter.medical", "/tmp/adapter.bin")
            .unwrap();

        alice
            .validate_adapter_pull("bob", "adapter.medical")
            .unwrap();
        alice.register_adapter_pull("req-1", "bob", "adapter.medical");

        // Provider side recognizes the pull.
        let disposition =
            bob.handle_request_received("req-1", "adapter.medical", ADAPTER_PULL_METHOD, "alice");
        match disposition {
            RequestDisposition::AdapterPull { artifact_path, .. } => {
                assert_eq!(artifact_path.as_deref(), Some("/tmp/adapter.bin"));
            }
            other => panic!("wrong disposition: {other:?}"),
        }

        // Correct bytes verify.
        assert_eq!(
            alice.pending_pull_from("bob"),
            Some(("req-1".into(), "adapter.medical".into()))
        );
        let listing = alice.complete_adapter_pull("req-1", &bytes).unwrap();
        assert_eq!(listing.service_id(), "adapter.medical");

        // Tampered bytes are rejected.
        alice.register_adapter_pull("req-2", "bob", "adapter.medical");
        let mut tampered = bytes.clone();
        tampered[0] ^= 1;
        assert!(matches!(
            alice.complete_adapter_pull("req-2", &tampered),
            Err(ExchangeError::ArtifactVerificationFailed(_))
        ));
    }

    #[test]
    fn unverified_adapter_pull_refused() {
        let (mut alice, mut bob, _, bob_key) = cores();
        let artifact = ArtifactRef {
            content_hash: "c".repeat(64),
            size_bytes: 1,
            base_model: "m".into(),
            base_model_version: "1".into(),
            chunking: ChunkPlan {
                chunk_size_bytes: 1,
            },
        };
        let (_, mut registered) = bob
            .prepare_publish(
                descriptor("adapter.x"),
                ListingKind::Adapter,
                Terms::free(),
                Some(artifact),
                NOW,
                &bob_key,
            )
            .unwrap();
        // Tamper the publisher field so verification fails.
        let key = crate::types::LISTING_CAPABILITY_KEY;
        let json = registered.capabilities.get(key).unwrap().clone();
        registered.capabilities.insert(
            key.into(),
            json.replace("\"publisher\":\"bob\"", "\"publisher\":\"eve\""),
        );
        alice.handle_discovered(
            "q-1",
            "adapter.x",
            "1.0",
            "bob",
            registered.capabilities,
            0,
            NOW,
            &DalekVerifier,
        );
        assert!(matches!(
            alice.validate_adapter_pull("bob", "adapter.x"),
            Err(ExchangeError::AttestationNotVerified(_))
        ));
    }

    #[test]
    fn snapshot_restore_preserves_ledger_receipts_reputation() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(40, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        alice.handle_response_received("req-1", "ok", NOW, &alice_key);

        let snapshot = alice.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored_snapshot: ExchangeSnapshot = serde_json::from_str(&json).unwrap();

        let mut restored = ExchangeCore::new("alice", ExchangeConfig::default());
        restored.restore(restored_snapshot);
        assert_eq!(restored.balance("USD").available_minor, 60);
        assert_eq!(restored.pending_receipts().len(), 1);
        assert_eq!(
            restored.reputation("bob").level,
            crate::reputation::ReputationLevel::New
        );
    }

    #[test]
    fn filters_by_kind_and_price() {
        let (mut alice, mut bob, _, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "free.svc",
            ListingKind::Service,
            Terms::free(),
            None,
        );
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "paid.svc",
            ListingKind::Service,
            priced_terms(5, BillingUnit::PerCall),
            None,
        );

        let free_only = alice.discovered_listings(&ListingFilter {
            free: Some(true),
            ..Default::default()
        });
        assert_eq!(free_only.len(), 1);
        assert_eq!(free_only[0].listing.service_id(), "free.svc");

        let paid_only = alice.discovered_listings(&ListingFilter {
            free: Some(false),
            ..Default::default()
        });
        assert_eq!(paid_only.len(), 1);
        assert_eq!(paid_only[0].listing.service_id(), "paid.svc");

        let adapters = alice.discovered_listings(&ListingFilter {
            kind: Some(ListingKind::Adapter),
            ..Default::default()
        });
        assert!(adapters.is_empty());
    }

    #[test]
    fn receipt_from_wrong_peer_ignored() {
        let (mut alice, mut bob, alice_key, bob_key) = cores();
        publish_and_discover(
            &mut bob,
            &mut alice,
            &bob_key,
            "weather.v1",
            ListingKind::Service,
            priced_terms(40, BillingUnit::PerCall),
            None,
        );
        alice.credit_balance("USD", 100).unwrap();
        let rsv = alice
            .reserve_invocation("bob", "weather.v1", 1, true, NOW)
            .unwrap();
        alice.bind_invocation(&rsv, "req-1").unwrap();
        bob.handle_request_received("req-1", "weather.v1", "get", "alice");
        let (outbound, _) = alice.handle_response_received("req-1", "ok", NOW, &alice_key);
        let data = outbound[0].content.strip_prefix(XCHG_RECEIPT).unwrap();
        // Delivered by "mallory" instead of the tracked consumer "alice".
        let (acks, events) = bob.handle_receipt_message("mallory", data, &bob_key, &DalekVerifier);
        assert!(acks.is_empty());
        assert!(events.is_empty());
    }
}
