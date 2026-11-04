//! Service discovery on a node whose application configured a publisher key
//! ([ADR 0037], "Service discovery").
//!
//! # Invariants
//!
//! - **Fan-out never reads membership.** Which peers a query goes to, and
//!   whether a response is relayed toward its originator, never depends on
//!   whether a body opened, a proof verified or a service matched. The caller
//!   hands the mesh neighbours in as the targets; this module only decides
//!   what this node *answers* and what it *reports*.
//! - **Only the routing fields are in the clear.** A configured node's query
//!   and response carry `query_id`, `originator` and (for a query) the hop
//!   budget readable; every other field travels sealed under the
//!   application key, and the cleartext `service_id` holds
//!   [`DISCOVERY_SEALED_SERVICE_ID`], which names no service.
//! - **Both ends prove membership.** A provider answers only a query whose
//!   proof verifies for its originator, and an originator reports only a
//!   response whose proof verifies for the provider that sent it.
//! - **A provider answers once per query, while it is fresh.** Freshness is
//!   the originator's own `issued_at`, sealed and proof-bound, never a hop's
//!   frame timestamp, which every forwarder signs anew.
//!
//! The cryptography is not here: the engine lends it through
//! [`DiscoverySealer`], so this crate stays free of MLS and of every cipher.
//!
//! [ADR 0037]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use offline_protocol_core::Address;
use offline_protocol_sealed::membership::discovery_bound_body;
pub use offline_protocol_sealed::membership::{ProofKind, DISCOVERY_SEALED_SERVICE_ID};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::payloads::{ServiceDiscoveryQueryPayload, ServiceDiscoveryResponsePayload};

/// How long after its originator issued it a configured provider still
/// answers a discovery query, in seconds, either side of `issued_at`.
///
/// No longer than the query dedup window, so "fresh" and "answered once"
/// cannot drift apart: the provider remembers a query it answered for as
/// long as the query could still be judged fresh (see
/// `MeshServices::answered_queries`), and a capture replayed after that is
/// stale. A provider whose clock is off from the originator's by more than
/// this answers nothing, which costs a result, never a leak.
pub const DISCOVERY_QUERY_LIFETIME_SECS: u64 = 60;

const _: () = assert!(DISCOVERY_QUERY_LIFETIME_SECS <= crate::DISCOVERY_QUERY_DEDUP_TTL_SECS);

/// What a configured node lends service discovery: the application key, its
/// certificate, its identity key and a clock.
///
/// The engine implements it when `ProtocolConfig::app_publisher_key` is set
/// and passes it on every discovery call; with no publisher key it passes
/// none, and discovery runs exactly as before this trait existed.
pub trait DiscoverySealer {
    /// Unix seconds now: the originator's `issued_at`, and the provider's
    /// freshness check against it.
    fn now_unix(&self) -> u64;

    /// This node's Ed25519 identity public key. It travels inside the sealed
    /// body so a verifier several hops away holds the key the proof's
    /// identity signature is checked under; the verifier checks that it
    /// derives to the address it speaks for.
    fn identity_key(&self) -> Option<Vec<u8>>;

    /// `salt || ChaCha20-Poly1305(k, nonce = 0, aad = empty, plaintext)`
    /// with `k = HKDF-SHA256(K_app, salt, "offline-app-svc-v1")` and a fresh
    /// salt.
    fn seal(&self, plaintext: &[u8]) -> Option<Vec<u8>>;

    /// The inverse of [`Self::seal`]; `None` for a body sealed under another
    /// application's key, or random bytes.
    fn open(&self, sealed: &[u8]) -> Option<Vec<u8>>;

    /// An end-to-end membership proof of `kind` toward `recipient` (`None`
    /// for a query) over `bound`. A node that holds no certificate returns
    /// random bytes of a proof's length, so whether a proof is real is not
    /// visible to anyone who cannot open it.
    fn prove(&self, recipient: Option<&str>, kind: ProofKind, bound: &[u8]) -> Vec<u8>;

    /// Whether `proof` opens under the application key, carries a current
    /// certificate for `signer` signed by the publisher, and `identity_key`
    /// (which must derive to `signer`) signed `recipient`, `kind` and
    /// `bound`.
    fn verify(
        &self,
        proof: &[u8],
        signer: &str,
        identity_key: &[u8],
        recipient: Option<&str>,
        kind: ProofKind,
        bound: &[u8],
    ) -> bool;
}

/// The sealed body of a configured node's query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SealedQueryBody {
    /// Unix seconds when the originator issued the query.
    pub issued_at: u64,
    /// The originator's identity public key, base64.
    pub identity_key: String,
    /// The service asked for; absent asks for every service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
}

/// The sealed body of a configured provider's response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SealedResponseBody {
    /// The provider's address. Must be the response's sender.
    pub provider: String,
    /// The provider's identity public key, base64.
    pub identity_key: String,
    pub service_id: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: HashMap<String, String>,
    #[serde(default)]
    pub hop_count: u8,
}

/// The bytes a discovery proof binds, from the payload's routing fields and
/// its sealed body (base64 decoded).
fn bound_body(query_id: &str, originator: &str, sealed: &[u8]) -> Option<Vec<u8>> {
    let originator: Address = originator.parse().ok()?;
    discovery_bound_body(query_id, &originator, sealed).ok()
}

fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

/// A configured originator's query payload: the routing fields in the clear,
/// the rest sealed and proven.
pub(crate) fn seal_query(
    sealer: &dyn DiscoverySealer,
    query_id: &str,
    originator: &str,
    service_id: Option<&str>,
    remaining_hops: u8,
) -> Option<ServiceDiscoveryQueryPayload> {
    let body = SealedQueryBody {
        issued_at: sealer.now_unix(),
        identity_key: b64(&sealer.identity_key()?),
        service_id: service_id.map(str::to_string),
    };
    let sealed = sealer.seal(&serde_json::to_vec(&body).ok()?)?;
    let bound = bound_body(query_id, originator, &sealed)?;
    let proof = sealer.prove(None, ProofKind::DiscoveryQuery, &bound);
    Some(ServiceDiscoveryQueryPayload {
        query_id: query_id.to_string(),
        originator: originator.to_string(),
        service_id: Some(DISCOVERY_SEALED_SERVICE_ID.to_string()),
        remaining_hops,
        sealed: Some(b64(&sealed)),
        member_proof: Some(b64(&proof)),
    })
}

/// A query this node may answer: its body opened, and its proof verifies for
/// its originator. `None` for anything else, which is forwarded and never
/// matched.
pub(crate) fn open_query(
    sealer: &dyn DiscoverySealer,
    payload: &ServiceDiscoveryQueryPayload,
) -> Option<SealedQueryBody> {
    let sealed = unb64(payload.sealed.as_deref()?)?;
    let proof = unb64(payload.member_proof.as_deref()?)?;
    let body: SealedQueryBody = serde_json::from_slice(&sealer.open(&sealed)?).ok()?;
    let identity_key = unb64(&body.identity_key)?;
    let bound = bound_body(&payload.query_id, &payload.originator, &sealed)?;
    sealer
        .verify(
            &proof,
            &payload.originator,
            &identity_key,
            None,
            ProofKind::DiscoveryQuery,
            &bound,
        )
        .then_some(body)
}

/// Whether a query issued at `issued_at` is fresh at `now`: within the
/// lifetime on either side, so a provider whose clock runs a little behind
/// the originator's still answers.
pub(crate) fn is_fresh(issued_at: u64, now: u64) -> bool {
    now.abs_diff(issued_at) <= DISCOVERY_QUERY_LIFETIME_SECS
}

/// A configured provider's response payload toward `originator`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn seal_response(
    sealer: &dyn DiscoverySealer,
    query_id: &str,
    originator: &str,
    provider: &str,
    service_id: &str,
    version: &str,
    capabilities: &HashMap<String, String>,
    hop_count: u8,
) -> Option<ServiceDiscoveryResponsePayload> {
    let body = SealedResponseBody {
        provider: provider.to_string(),
        identity_key: b64(&sealer.identity_key()?),
        service_id: service_id.to_string(),
        version: version.to_string(),
        capabilities: capabilities.clone(),
        hop_count,
    };
    let sealed = sealer.seal(&serde_json::to_vec(&body).ok()?)?;
    let bound = bound_body(query_id, originator, &sealed)?;
    let proof = sealer.prove(Some(originator), ProofKind::DiscoveryResponse, &bound);
    Some(ServiceDiscoveryResponsePayload {
        query_id: query_id.to_string(),
        service_id: DISCOVERY_SEALED_SERVICE_ID.to_string(),
        version: String::new(),
        provider_peer_id: String::new(),
        capabilities: HashMap::new(),
        hop_count: 0,
        originator: originator.to_string(),
        sealed: Some(b64(&sealed)),
        member_proof: Some(b64(&proof)),
    })
}

/// A response this node may report: it names this node as its originator,
/// its body opened, the sealed provider is the frame's sender, and its proof
/// verifies for that provider toward this node.
pub(crate) fn open_response(
    sealer: &dyn DiscoverySealer,
    payload: &ServiceDiscoveryResponsePayload,
    sender: &str,
    user_id: &str,
) -> Option<SealedResponseBody> {
    if payload.originator != user_id {
        return None;
    }
    let sealed = unb64(payload.sealed.as_deref()?)?;
    let proof = unb64(payload.member_proof.as_deref()?)?;
    let body: SealedResponseBody = serde_json::from_slice(&sealer.open(&sealed)?).ok()?;
    if body.provider != sender {
        return None;
    }
    let identity_key = unb64(&body.identity_key)?;
    let bound = bound_body(&payload.query_id, &payload.originator, &sealed)?;
    sealer
        .verify(
            &proof,
            sender,
            &identity_key,
            Some(user_id),
            ProofKind::DiscoveryResponse,
            &bound,
        )
        .then_some(body)
}

fn response_payload(content: &str) -> Option<ServiceDiscoveryResponsePayload> {
    let data = content.strip_prefix(crate::SVC_DISCOVER_RESPONSE)?;
    if data.len() > crate::MAX_SERVICE_PAYLOAD_SIZE {
        return None;
    }
    serde_json::from_str(data).ok()
}

/// Whether `content` is a discovery response that ends at `user_id`: one
/// that names it as the originator, or names none.
///
/// Such a response is an end-to-end frame, so a configured node answers it
/// (even with an acknowledgement) only when its sender is admitted or it
/// carries a proof ([`discovery_response_is_proven`]). Every other discovery
/// frame, a query or a response on its way to someone else, is carriage.
pub fn discovery_response_ends_at(content: &str, user_id: &str) -> bool {
    response_payload(content)
        .is_some_and(|payload| payload.originator.is_empty() || payload.originator == user_id)
}

/// Whether `content` is a discovery response to `user_id` that `sender`
/// proved it may send: what the configured originator reports, and the
/// one discovery frame from a sender not otherwise admitted it answers.
pub fn discovery_response_is_proven(
    content: &str,
    sender: &str,
    user_id: &str,
    sealer: &dyn DiscoverySealer,
) -> bool {
    response_payload(content)
        .is_some_and(|payload| open_response(sealer, &payload, sender, user_id).is_some())
}

#[cfg(test)]
#[path = "sealing_tests.rs"]
mod tests;
