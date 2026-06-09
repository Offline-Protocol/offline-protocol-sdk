//! # Offline Protocol Capability Exchange
//!
//! An open capability exchange over the Offline Protocol mesh. Any
//! participant — a human user or an autonomous agent — can publish a
//! **listing**, discover listings across the mesh, invoke them, and settle
//! payment for metered invocations. Free to publish, metered to use.
//!
//! A listing is one of two things:
//! - a **service**: a request/response capability another node invokes, or
//! - an **adapter**: a model adapter a consumer pulls (content-addressed,
//!   integrity-checked) to gain a local capability.
//!
//! ## Design
//!
//! - **Wraps, never changes, service discovery.** A listing is the existing
//!   `ServiceDescriptor` plus a versioned envelope carried in the
//!   `capabilities` map. Exchange-unaware nodes interoperate untouched.
//! - **Provenance is mandatory.** Every listing is signed by the publisher's
//!   stable Ed25519 identity (OfflineID). Consumers verify before paying or
//!   loading, and a local reputation read is surfaced with every discovery.
//! - **The metered event is the invocation.** Settlement uses a prepaid
//!   balance with two-phase holds and **signed usage receipts** as the
//!   durable claim; clearing is eventual via a [`SettlementBackend`].
//! - **Priced messages never ride plaintext.** Receipts and usage
//!   declarations are marked `requires_confirmed_session`, and priced
//!   invocations refuse to start without a confirmed MLS session.
//! - **No I/O in this crate.** [`ExchangeCore`] returns messages-to-send and
//!   events-to-emit; the protocol crate owns transport, identity keys,
//!   events, and persistence (mirroring how `MeshServices` integrates).
//!
//! [`SettlementBackend`]: settlement::SettlementBackend
//! [`ExchangeCore`]: manager::ExchangeCore

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod artifact;
pub mod attestation;
pub mod canonical;
pub mod error;
pub mod ledger;
pub mod listing;
pub mod manager;
pub mod receipt;
pub mod reputation;
pub mod runtime;
pub mod settlement;
pub mod types;

pub use attestation::{ExchangeSigner, ExchangeVerifier};
pub use error::{ExchangeError, ExchangeResult};
pub use ledger::{Balance, PrepaidLedger};
pub use manager::{
    DiscoveredListing, ExchangeConfig, ExchangeCore, ExchangeEvent, ExchangeOutbound,
    ExchangeSnapshot, RequestDisposition, XCHG_RECEIPT, XCHG_RECEIPT_ACK, XCHG_USAGE,
};
pub use receipt::{ReceiptStatus, StoredReceipt, UsageReceipt};
pub use reputation::{ReputationLevel, ReputationRead, ReputationTracker};
pub use runtime::{AdapterRuntime, StubAdapterRuntime};
pub use settlement::{MockClearing, SettlementBackend, SettlementReport};
pub use types::{
    ArtifactRef, Attestation, AttestationStatus, BillingUnit, ChunkPlan, Listing, ListingEnvelope,
    ListingFilter, ListingKind, Price, Terms, ADAPTER_PULL_METHOD, LISTING_CAPABILITY_KEY,
    LISTING_ENVELOPE_VERSION,
};
