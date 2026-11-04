//! The application membership exchange ([ADR 0037]): how two neighbours of
//! the same application learn that they are, and every other pair learns
//! nothing.
//!
//! # Invariants
//!
//! - **Nothing observable depends on membership.** Every node, configured or
//!   not, sends one hello to each new neighbour and answers every fresh hello
//!   with one response, both at their fixed sizes, and the answer costs one
//!   identity signature and one seal whether it carries a credential or
//!   random bytes. A node that stopped on a mismatch would be an oracle for
//!   "does this peer run that application".
//! - **The frames never leave the link.** A hello and a response are sent
//!   only over a mesh carrier, straight to the neighbour, with a TTL of one
//!   and no acknowledgement requested. One that arrives over any other
//!   carrier, after a hop, or unsigned is consumed and ignored: a relay
//!   mailbox would hold a hello for days, and an acknowledgement is an answer
//!   whose presence would depend on the receiver.
//! - **No sealing key is used twice.** The response key is derived from both
//!   nonces, and a node answers each pair of nonces at most once, so a
//!   replayed hello never makes it seal a second plaintext under a key it
//!   already used.
//! - **Visible means verified.** A peer is visible only after its credential
//!   opened under the key derived for its own address, its certificate
//!   verified under this application's publisher key for that address and is
//!   current, and its identity signature covers this node's fresh nonce.
//!
//! # The visibility gate
//!
//! With a publisher key configured, a peer is reported to the application,
//! and the automatic effects of discovering it run, only once it is visible
//! ([`OfflineProtocol::app_sees_peer`]). A discovery that comes first is held
//! and reported when the exchange proves membership. Carriage is never gated:
//! relay targets, the outbox flush to that peer and custody redelivery run
//! for everyone. Nor is anything the application asked for itself toward a
//! peer it named (a subscription, a shared space, a Welcome, its presence
//! watchlist). With no publisher key every peer is visible, which is every
//! release before this one exactly.
//!
//! [ADR 0037]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md

use std::collections::{HashMap, HashSet};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use chrono::Utc;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_core::{Address, Message, MessagePriority, TTL};
use offline_protocol_sealed::membership::{
    app_key_info, exchange_key_info, exchange_salt, hello_tag_input, transcript_signing_payload,
    MemberCert, MemberCredential, MemberFrame, MemberHello, APP_KEY_LEN, MEMBER_CREDENTIAL_LEN,
    MEMBER_NONCE_LEN, MEMBER_SEALED_LEN, MEMBER_TAG_LEN,
};
use offline_protocol_transport::TransportType;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use tracing::{debug, error, info, warn};

use super::types::CTRL_PK_META_KEY;
use super::{base64_decode, internal_prefixes, OfflineProtocol};
use crate::events::Event;
use crate::{Error, Result};

/// How long an exchange with one neighbour stands before a later discovery
/// of that neighbour starts a new one.
///
/// Discovery fires on every reachability edge, and the exchange is two frames
/// each way plus a signature, so a neighbour that flaps its link would
/// otherwise cost that on every flap. A peer that restarts in the meantime
/// sends a fresh hello, which is answered whatever this says.
pub(crate) const MEMBER_EXCHANGE_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Neighbours with an exchange in memory. The least recently started goes
/// first: a neighbourhood that mints addresses must not grow this without
/// limit, and the oldest exchange is the one least likely to be mid-flight.
pub(crate) const MAX_MEMBER_EXCHANGES: usize = 512;

/// Peers held visible. The one whose certificate ends soonest goes first.
pub(crate) const MAX_VISIBLE_MEMBERS: usize = 1024;

/// Nonces of one peer answered under one nonce of ours. Past this the
/// exchange starts over with a fresh nonce of ours, so the set stays bounded
/// and a replay of an older hello still finds a key never used.
const MAX_ANSWERED_PER_EXCHANGE: usize = 8;

/// What [`OfflineProtocol::app_membership_status`] reports: this node's own
/// configuration, never anything about a particular peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppMembershipStatus {
    /// Whether [`crate::ProtocolConfig::app_publisher_key`] is set.
    pub configured: bool,
    /// The `not_after` of this node's own certificate, once
    /// [`OfflineProtocol::set_app_membership`] accepted one. `None` also once
    /// it has expired: an expired certificate makes this node a stranger to
    /// every member, so the application is told to fetch a new one.
    pub certificate_not_after: Option<u64>,
    /// How many neighbours are visible as members right now.
    pub visible_members: u32,
}

/// Signs a membership certificate for `address`, valid until `not_after`
/// (Unix seconds), with the publisher's Ed25519 private key: embedded mode in
/// [ADR 0037].
///
/// Pure: it keeps no state, and the key is never logged, stored, emitted or
/// sent. The application calls it and hands the result to
/// [`OfflineProtocol::set_app_membership`]. An application that embeds its
/// publisher key lets anyone holding its binary mint members, so a backend
/// that signs the address it is sent is the recommended mode; the
/// certificate is the same 93 bytes either way.
///
/// # Errors
///
/// [`Error::InvalidArgument`] for a key that is not 32 bytes or an address
/// that does not parse.
///
/// [ADR 0037]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md
pub fn sign_app_membership(
    publisher_private_key: &[u8],
    address: &str,
    not_after: u64,
) -> Result<Vec<u8>> {
    use ed25519_dalek::Signer;

    let seed = <[u8; 32]>::try_from(publisher_private_key).map_err(|_| {
        Error::InvalidArgument("The publisher private key must be 32 bytes".to_string())
    })?;
    let address: Address = address
        .parse()
        .map_err(|e| Error::InvalidArgument(format!("The address does not parse: {e}")))?;
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let signature = signing
        .sign(&MemberCert::signing_payload(&address, not_after))
        .to_bytes();
    let cert = MemberCert {
        address,
        not_after,
        signature,
    };
    Ok(cert.to_bytes().to_vec())
}

/// This node's side of the membership exchange.
#[derive(Default)]
pub(crate) struct Membership {
    /// `K_app`, when a publisher key is configured.
    app_key: Option<[u8; APP_KEY_LEN]>,
    /// The publisher key certificates are verified under.
    publisher_key: Option<[u8; 32]>,
    /// This node's own certificate, once the application supplied one.
    cert: Option<MemberCert>,
    /// One exchange per neighbour address.
    exchanges: HashMap<String, Exchange>,
    /// Verified members, by address, with their certificate's `not_after`.
    visible: HashMap<String, u64>,
    /// Neighbours a carrier reported before they were visible, with the
    /// carrier's name, reported to the application once they are.
    held_reports: HashMap<String, String>,
    /// Neighbours reported to the application, so that only those are
    /// reported lost.
    reported: HashSet<String>,
}

struct Exchange {
    started: Instant,
    ours: [u8; MEMBER_NONCE_LEN],
    theirs: Option<[u8; MEMBER_NONCE_LEN]>,
    /// Every nonce of theirs answered under `ours`.
    answered: Vec<[u8; MEMBER_NONCE_LEN]>,
    /// A response that arrived before the hello it answers, held for it
    /// with the identity key its frame was signed by.
    pending_response: Option<([u8; MEMBER_SEALED_LEN], Option<Vec<u8>>)>,
    /// Whether a response for (`ours`, `theirs`) has verified.
    verified: bool,
}

impl Exchange {
    fn fresh() -> Self {
        Self {
            started: Instant::now(),
            ours: random_array(),
            theirs: None,
            answered: Vec::new(),
            pending_response: None,
            verified: false,
        }
    }
}

impl Membership {
    /// The state for a configuration: `K_app` from the publisher key and the
    /// application id, or nothing to derive when there is no key.
    pub(crate) fn new(publisher_key: Option<[u8; 32]>, app_id: &str) -> Self {
        let app_key = publisher_key.map(|pk| hkdf32(&pk, None, &app_key_info(app_id)));
        Self {
            app_key,
            publisher_key,
            ..Self::default()
        }
    }
}

fn random_array<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    OsRng.fill_bytes(&mut out);
    out
}

fn hkdf32(ikm: &[u8], salt: Option<&[u8]>, info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // 32 bytes is far below HKDF-SHA256's 8160-byte ceiling, so expand
    // cannot fail; the fallback keeps the function total without a panic.
    // It is logged, because a random `K_app` would turn a member into a
    // stranger to every peer with nothing else to say why.
    if Hkdf::<Sha256>::new(salt, ikm)
        .expand(info, &mut out)
        .is_err()
    {
        error!("HKDF-SHA256 refused a 32-byte expand; membership keys are random");
        out = random_array();
    }
    out
}

/// HMAC-SHA256 keyed by `K_app`. HMAC takes a key of any length, so the
/// `None` arm is unreachable; it keeps the function free of a panic.
fn hello_mac(app_key: &[u8; APP_KEY_LEN]) -> Option<Hmac<Sha256>> {
    <Hmac<Sha256> as Mac>::new_from_slice(app_key).ok()
}

fn unix_now() -> u64 {
    u64::try_from(Utc::now().timestamp()).unwrap_or(0)
}

/// Whether a frame arrived the only way a `__MEMBER__` frame may: over a
/// mesh carrier, from the neighbour that made it.
fn arrived_on_the_link(message: &Message, arrival: Option<TransportType>) -> bool {
    matches!(
        arrival,
        Some(TransportType::BLE | TransportType::WiFiDirect)
    ) && message.hop_count.value() == 0
}

impl OfflineProtocol {
    /// Supplies this node's membership certificate for the application named
    /// by [`crate::ProtocolConfig::app_publisher_key`] and
    /// [`crate::ProtocolConfig::app_id`].
    ///
    /// The certificate is the 93-byte layout of the control-message chapter,
    /// issued by the application's backend or by `sign_app_membership` in
    /// embedded mode. It is checked before it is kept: it must name this
    /// node's address, verify under the configured publisher key, and be
    /// current, because a node that presents a bad certificate is a stranger
    /// to every member and would never learn why. Held in memory: the
    /// application supplies it on every launch.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidConfiguration`] with no publisher key configured, and
    /// [`Error::InvalidArgument`] for a certificate that is malformed, names
    /// another address, does not verify, or has expired. Before
    /// `initialize_mls` there is no address to check it against, which is
    /// [`Error::InvalidArgument`] too.
    pub fn set_app_membership(&mut self, cert: &[u8]) -> Result<()> {
        let Some(publisher_key) = self.membership.publisher_key else {
            return Err(Error::InvalidConfiguration(
                "set_app_membership needs ProtocolConfig.app_publisher_key".to_string(),
            ));
        };
        let local: Address = self
            .local_address()
            .ok_or_else(|| {
                Error::InvalidArgument(
                    "set_app_membership before initialize_mls: no address to bind".to_string(),
                )
            })?
            .parse()
            .map_err(|e| {
                Error::InvalidArgument(format!("The local address does not parse: {e}"))
            })?;
        let cert = MemberCert::from_bytes(cert).map_err(|e| {
            Error::InvalidArgument(format!("Malformed membership certificate: {e}"))
        })?;
        if cert.address != local {
            return Err(Error::InvalidArgument(
                "The membership certificate names another address".to_string(),
            ));
        }
        if !cert_verifies(&publisher_key, &cert) {
            return Err(Error::InvalidArgument(
                "The membership certificate does not verify under the publisher key".to_string(),
            ));
        }
        if !cert.is_current(unix_now()) {
            return Err(Error::InvalidArgument(
                "The membership certificate has expired".to_string(),
            ));
        }
        if self.membership.cert.as_ref() == Some(&cert) {
            // The same certificate again: an application that supplies it on
            // every launch or foreground. Restarting would make every
            // neighbour sign and seal again, and a hello burst with no link
            // change is something only a configured node produces.
            return Ok(());
        }
        info!(
            not_after = cert.not_after,
            "Application membership certificate set"
        );
        self.membership.cert = Some(cert);
        self.restart_member_exchanges();
        Ok(())
    }

    /// This node's membership configuration: whether a publisher key is set,
    /// its own certificate's expiry, and how many neighbours are visible.
    pub fn app_membership_status(&self) -> AppMembershipStatus {
        let now = unix_now();
        let visible = self
            .membership
            .visible
            .values()
            .filter(|not_after| now <= **not_after)
            .count();
        AppMembershipStatus {
            configured: self.membership.publisher_key.is_some(),
            certificate_not_after: self
                .membership
                .cert
                .as_ref()
                .filter(|cert| cert.is_current(now))
                .map(|cert| cert.not_after),
            visible_members: u32::try_from(visible).unwrap_or(u32::MAX),
        }
    }

    /// Starts a fresh exchange with every current mesh neighbour.
    ///
    /// A certificate that arrives after this node met its neighbours (the
    /// backend issues it once the application is running) would otherwise
    /// reach none of them: every standing exchange already answered with
    /// random bytes, and holds off a new one for
    /// [`MEMBER_EXCHANGE_INTERVAL`]. Each neighbour sees a fresh nonce, which
    /// reads as a restart, and answers it.
    fn restart_member_exchanges(&mut self) {
        self.membership.exchanges.clear();
        let neighbours: Vec<String> = self
            .transport_manager
            .mesh_neighbors()
            .into_iter()
            .map(|n| n.peer_id)
            .collect();
        for peer in neighbours {
            self.start_member_exchange(&peer);
        }
    }

    /// Installs a certificate without the checks of
    /// [`Self::set_app_membership`], so a test can stand in for a node that
    /// presents a certificate that is not its own.
    #[cfg(test)]
    pub(crate) fn set_app_membership_unchecked(&mut self, cert: MemberCert) {
        self.membership.cert = Some(cert);
    }

    /// Ages every standing exchange past [`MEMBER_EXCHANGE_INTERVAL`], so a
    /// test can ask what the next discovery does once the interval is over.
    #[cfg(test)]
    pub(crate) fn expire_member_exchanges(&mut self) {
        for exchange in self.membership.exchanges.values_mut() {
            if let Some(earlier) = Instant::now().checked_sub(MEMBER_EXCHANGE_INTERVAL) {
                exchange.started = earlier;
            }
        }
    }

    /// Whether the application may be told about `peer`, and the automatic
    /// effects of discovering it may run: always with no publisher key
    /// configured, otherwise only once `peer` is visible.
    pub fn app_sees_peer(&self, peer: &str) -> bool {
        self.membership.publisher_key.is_none() || self.is_member_visible(peer)
    }

    /// A carrier reports `peer` reachable: runs discovery, and returns
    /// whether the caller reports `NeighborDiscovered` for it now.
    ///
    /// `false` for a blocked peer, and, with a publisher key configured, for
    /// a peer not yet visible. Such a report is held, and the engine emits
    /// `NeighborDiscovered` itself, naming `transport`, when the exchange
    /// makes the peer visible. Every carrier's discovery goes through here,
    /// so the decision is made once, in the core.
    pub fn neighbor_reachable(&mut self, peer: &str, transport: &str) -> bool {
        let blocked = self.is_user_blocked(peer);
        self.on_neighbor_discovered(peer);
        if blocked {
            return false;
        }
        if self.membership.publisher_key.is_none() {
            return true;
        }
        if self.is_member_visible(peer) {
            self.membership.reported.insert(peer.to_string());
            return true;
        }
        let held = &mut self.membership.held_reports;
        if !held.contains_key(peer) && held.len() >= MAX_MEMBER_EXCHANGES {
            // Bounded like the exchanges it waits on. An arbitrary entry
            // goes: one that never resolves is a stranger's.
            if let Some(victim) = held.keys().next().cloned() {
                held.remove(&victim);
            }
        }
        held.insert(peer.to_string(), transport.to_string());
        false
    }

    /// A carrier reports `peer` lost: runs the loss, and returns whether the
    /// caller reports `NeighborLost` for it. With a publisher key configured,
    /// only a peer the application was told about is reported lost.
    pub fn neighbor_unreachable(&mut self, peer: &str) -> bool {
        self.on_neighbor_lost(peer);
        if self.membership.publisher_key.is_none() {
            return true;
        }
        self.membership.held_reports.remove(peer);
        self.membership.reported.remove(peer)
    }

    /// Runs what was held back from `peer`'s discovery, now that it is
    /// visible: its `NeighborDiscovered`, and the automatic key package push.
    fn release_held_discovery(&mut self, peer: &str) {
        let Some(transport) = self.membership.held_reports.remove(peer) else {
            return;
        };
        if !self.has_mesh_link(peer) && !self.known_peers.contains_key(peer) {
            // Gone since: nothing to report, and its loss was not reported.
            return;
        }
        self.membership.reported.insert(peer.to_string());
        self.emit_event(Event::NeighborDiscovered {
            peer_id: peer.to_string(),
            transport,
            rssi: None,
        });
        self.push_key_package_on_discovery(peer);
    }

    /// Whether `peer` has proven, in a membership exchange, that it belongs to
    /// this node's application, and its certificate is still current.
    ///
    /// Always `false` with no publisher key configured.
    pub(crate) fn is_member_visible(&self, peer: &str) -> bool {
        self.membership
            .visible
            .get(peer)
            .is_some_and(|not_after| unix_now() <= *not_after)
    }

    /// Starts the exchange with a neighbour discovery just reported, unless
    /// one is standing.
    ///
    /// Called for every node, configured or not: an unconfigured node's hello
    /// is random bytes of the same size, so sending it reveals nothing.
    ///
    /// Never skipped because the neighbour is already visible. Whether a
    /// hello goes out would then depend on membership: a node that labels
    /// its own link with a recorded address (R16) would learn, from one
    /// connection, whether this node holds that address as a member, and a
    /// passive listener would see member pairs fall silent on a rediscovery.
    /// The visible set decides [`Self::is_member_visible`] and nothing that
    /// goes on the air.
    pub(super) fn start_member_exchange(&mut self, peer: &str) {
        if self.mls_manager.is_none() || peer.parse::<Address>().is_err() {
            return;
        }
        // Discovery also reports peers reached through the relay, Nostr or
        // the LAN, which a membership frame never rides. Asked before the
        // hello is built, so such a peer costs no clock tick and no signature.
        if !self.has_mesh_link(peer) {
            return;
        }
        if let Some(standing) = self.membership.exchanges.get(peer) {
            if standing.started.elapsed() < MEMBER_EXCHANGE_INTERVAL {
                return;
            }
        }
        let exchange = Exchange::fresh();
        let ours = exchange.ours;
        // Recorded only once the hello is on a link: a discovery that came
        // from the relay's presence, with no mesh link to the peer, must not
        // hold off the exchange the next mesh discovery would start.
        if self.send_member_hello(peer, &ours) {
            self.insert_member_exchange(peer, exchange);
        }
    }

    fn has_mesh_link(&self, peer: &str) -> bool {
        [TransportType::BLE, TransportType::WiFiDirect]
            .into_iter()
            .any(|carrier| self.transport_manager.can_address_via(carrier, peer))
    }

    fn insert_member_exchange(&mut self, peer: &str, exchange: Exchange) {
        let exchanges = &mut self.membership.exchanges;
        if !exchanges.contains_key(peer) && exchanges.len() >= MAX_MEMBER_EXCHANGES {
            if let Some(oldest) = exchanges
                .iter()
                .min_by_key(|(_, e)| e.started)
                .map(|(id, _)| id.clone())
            {
                exchanges.remove(&oldest);
            }
        }
        exchanges.insert(peer.to_string(), exchange);
    }

    /// A `__MEMBER__` frame, past the control gate.
    ///
    /// Consumed whatever it says. Nothing here sends anything that depends
    /// on whether this node is a member: a hello is answered by every node,
    /// and a response is answered by none.
    pub(super) fn handle_member_frame(
        &mut self,
        message: &Message,
        body: &str,
        signed: bool,
        arrival: Option<TransportType>,
    ) {
        let sender = message.sender.as_str();
        if !signed
            || !arrived_on_the_link(message, arrival)
            || message.recipient.as_str() != self.local_id
        {
            debug!(
                sender = %sender,
                signed,
                arrival = ?arrival,
                hop_count = message.hop_count.value(),
                "Ignored a membership frame that did not come straight over a mesh link"
            );
            return;
        }
        if sender == self.local_id || sender.parse::<Address>().is_err() {
            return;
        }
        match MemberFrame::parse(body) {
            Ok(MemberFrame::Hello(hello)) => self.on_member_hello(sender, &hello),
            Ok(MemberFrame::Response(sealed)) => self.on_member_response(message, &sealed),
            Err(e) => debug!(sender = %sender, error = %e, "Refused a malformed membership frame"),
        }
    }

    fn on_member_hello(&mut self, peer: &str, hello: &MemberHello) {
        // A neighbour that discovered this node first, or a standing exchange
        // gone stale: this side's hello goes out before the answer, since the
        // answer's key needs the peer to hold this side's nonce.
        let standing = self
            .membership
            .exchanges
            .get(peer)
            .is_some_and(|e| e.started.elapsed() < MEMBER_EXCHANGE_INTERVAL);
        if !standing {
            let exchange = Exchange::fresh();
            let ours = exchange.ours;
            if !self.send_member_hello(peer, &ours) {
                return;
            }
            self.insert_member_exchange(peer, exchange);
        }

        let Some(exchange) = self.membership.exchanges.get(peer) else {
            return;
        };
        if exchange.answered.contains(&hello.nonce) {
            // The same hello again: a duplicate on the link or a replay.
            // Answering it twice would seal a second time under one key.
            return;
        }
        if exchange.answered.len() >= MAX_ANSWERED_PER_EXCHANGE {
            let fresh = Exchange::fresh();
            let ours = fresh.ours;
            if !self.send_member_hello(peer, &ours) {
                return;
            }
            self.insert_member_exchange(peer, fresh);
        } else if !exchange.answered.is_empty() {
            // The peer has a new nonce after an answer: it restarted and no
            // longer holds this side's nonce, so it gets the hello again. The
            // same nonce, never a new one: a new one would read to the peer
            // as this side restarting, and two nodes answering that with
            // fresh nonces would never settle.
            let ours = exchange.ours;
            if !self.send_member_hello(peer, &ours) {
                return;
            }
        }

        let Some(exchange) = self.membership.exchanges.get_mut(peer) else {
            return;
        };
        exchange.theirs = Some(hello.nonce);
        exchange.verified = false;
        exchange.answered.push(hello.nonce);
        let ours = exchange.ours;
        let pending = exchange.pending_response.take();

        self.send_member_response(peer, &ours, hello);
        if let Some((sealed, identity_key)) = pending {
            self.open_member_response(peer, &sealed, identity_key);
        }
    }

    fn on_member_response(&mut self, message: &Message, sealed: &[u8; MEMBER_SEALED_LEN]) {
        let peer = message.sender.as_str();
        let identity_key = message
            .metadata
            .get(CTRL_PK_META_KEY)
            .and_then(|pk| base64_decode(pk).ok());
        let Some(exchange) = self.membership.exchanges.get_mut(peer) else {
            return;
        };
        if exchange.theirs.is_none() {
            // Ahead of the hello it answers. One is held: the link delivers
            // in order almost always, and a second early response would be a
            // peer that answered twice before saying hello at all.
            exchange.pending_response = Some((*sealed, identity_key));
            return;
        }
        self.open_member_response(peer, sealed, identity_key);
    }

    /// Verifies a peer's response and, on a pass, makes it visible.
    ///
    /// `identity_key` is the key that signed the response's frame, which the
    /// control gate already bound to the sender's address. `None` only for a
    /// frame that carried no key, which the gate admits as unsigned and the
    /// handler has already refused, so it fails closed here.
    fn open_member_response(
        &mut self,
        peer: &str,
        sealed: &[u8; MEMBER_SEALED_LEN],
        identity_key: Option<Vec<u8>>,
    ) {
        let (Some(app_key), Some(publisher_key)) =
            (self.membership.app_key, self.membership.publisher_key)
        else {
            return;
        };
        let Some(exchange) = self.membership.exchanges.get(peer) else {
            return;
        };
        let (Some(theirs), false) = (exchange.theirs, exchange.verified) else {
            return;
        };
        let ours = exchange.ours;
        let (Ok(local), Ok(sender)) = (self.local_id.parse::<Address>(), peer.parse::<Address>())
        else {
            return;
        };
        let Ok(salt) = exchange_salt(&local, &ours, &sender, &theirs) else {
            return;
        };
        let key = hkdf32(&app_key, Some(&salt), &exchange_key_info(&sender));
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let Ok(plaintext) = cipher.decrypt(Nonce::from_slice(&[0u8; 12]), sealed.as_slice()) else {
            // A stranger's random bytes, or a response to a nonce of ours
            // this side has since replaced. Neither is reported.
            debug!(peer = %peer, "Membership response did not open: not a member");
            return;
        };
        let Ok(credential) = MemberCredential::from_bytes(&plaintext) else {
            return;
        };
        let cert = &credential.cert;
        if cert.address != sender || !cert_verifies(&publisher_key, cert) {
            warn!(peer = %peer, "Membership response opened but its certificate is not this peer's");
            return;
        }
        if !cert.is_current(unix_now()) {
            debug!(peer = %peer, not_after = cert.not_after, "Membership certificate has expired");
            return;
        }
        let Some(identity_key) = identity_key else {
            return;
        };
        let transcript = transcript_signing_payload(&sender, &theirs, &local, &ours);
        let signed = offline_protocol_mls::MlsManager::verify_signature(
            &identity_key,
            &transcript,
            &credential.signature,
        )
        .unwrap_or(false);
        if !signed {
            warn!(peer = %peer, "Membership response's transcript signature does not verify");
            return;
        }

        if let Some(exchange) = self.membership.exchanges.get_mut(peer) {
            exchange.verified = true;
        }
        self.mark_member_visible(peer, cert.not_after);
        info!(peer = %peer, not_after = cert.not_after, "Neighbour is a member of this application");
        self.release_held_discovery(peer);
    }

    /// Holds `peer` visible until `not_after`, evicting the certificate that
    /// ends soonest once [`MAX_VISIBLE_MEMBERS`] are held.
    pub(crate) fn mark_member_visible(&mut self, peer: &str, not_after: u64) {
        let visible = &mut self.membership.visible;
        if !visible.contains_key(peer) && visible.len() >= MAX_VISIBLE_MEMBERS {
            if let Some(soonest) = visible
                .iter()
                .min_by_key(|(_, not_after)| **not_after)
                .map(|(id, _)| id.clone())
            {
                visible.remove(&soonest);
            }
        }
        visible.insert(peer.to_string(), not_after);
    }

    /// How many neighbours have an exchange in memory.
    #[cfg(test)]
    pub(crate) fn member_exchange_count(&self) -> usize {
        self.membership.exchanges.len()
    }

    fn send_member_hello(&mut self, peer: &str, ours: &[u8; MEMBER_NONCE_LEN]) -> bool {
        let tag = match (
            self.membership.app_key,
            self.local_id.parse::<Address>(),
            peer.parse::<Address>(),
        ) {
            (Some(app_key), Ok(local), Ok(peer_addr)) => match hello_mac(&app_key) {
                Some(mut mac) => {
                    mac.update(&hello_tag_input(ours, &local, &peer_addr));
                    let full = mac.finalize().into_bytes();
                    let mut tag = [0u8; MEMBER_TAG_LEN];
                    tag.copy_from_slice(&full[..MEMBER_TAG_LEN]);
                    tag
                }
                None => random_array(),
            },
            // No application key: random bytes in the tag's place.
            _ => random_array(),
        };
        let frame = MemberFrame::Hello(MemberHello { nonce: *ours, tag });
        self.send_member_frame(peer, &frame)
    }

    /// Answers a hello: a sealed credential when the hello's tag verifies and
    /// this node holds a certificate, otherwise random bytes made by the same
    /// work.
    fn send_member_response(
        &mut self,
        peer: &str,
        ours: &[u8; MEMBER_NONCE_LEN],
        hello: &MemberHello,
    ) {
        let (Ok(local), Ok(peer_addr)) =
            (self.local_id.parse::<Address>(), peer.parse::<Address>())
        else {
            return;
        };
        let member = match (self.membership.app_key, &self.membership.cert) {
            (Some(app_key), Some(cert)) => hello_mac(&app_key).and_then(|mut mac| {
                mac.update(&hello_tag_input(&hello.nonce, &peer_addr, &local));
                mac.verify_truncated_left(&hello.tag)
                    .is_ok()
                    .then(|| (app_key, cert.clone()))
            }),
            _ => None,
        };
        let Ok(salt) = exchange_salt(&local, ours, &peer_addr, &hello.nonce) else {
            return;
        };

        // The same signature, derivation and seal on both paths, so neither
        // the size nor the cost of the answer says which one ran. On a
        // mismatch the key is derived from random input keying material,
        // which makes the sealed bytes random to everyone.
        let transcript = transcript_signing_payload(&local, ours, &peer_addr, &hello.nonce);
        let signature = match self.sign_with_identity(&transcript) {
            Some(signature) => signature,
            None => return,
        };
        let (key, cert_bytes) = match &member {
            Some((app_key, cert)) => (
                hkdf32(app_key, Some(&salt), &exchange_key_info(&local)),
                Some(cert.to_bytes()),
            ),
            None => (
                hkdf32(
                    &random_array::<32>(),
                    Some(&salt),
                    &exchange_key_info(&local),
                ),
                None,
            ),
        };
        let mut plaintext = [0u8; MEMBER_CREDENTIAL_LEN];
        if let Some(cert_bytes) = cert_bytes {
            plaintext[..cert_bytes.len()].copy_from_slice(&cert_bytes);
        }
        plaintext[MEMBER_CREDENTIAL_LEN - signature.len()..].copy_from_slice(&signature);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let Ok(sealed) = cipher.encrypt(Nonce::from_slice(&[0u8; 12]), plaintext.as_slice()) else {
            return;
        };
        let Ok(sealed) = <[u8; MEMBER_SEALED_LEN]>::try_from(sealed.as_slice()) else {
            return;
        };
        self.send_member_frame(peer, &MemberFrame::Response(sealed));
    }

    /// An Ed25519 signature by this node's identity key, always 64 bytes.
    fn sign_with_identity(&self, payload: &[u8]) -> Option<[u8; 64]> {
        let manager = self.mls_manager.as_ref()?.read().ok()?;
        let signature = manager.sign_data(payload).ok()?;
        <[u8; 64]>::try_from(signature.as_slice()).ok()
    }

    /// Puts one membership frame on the neighbour's link: signed, TTL one,
    /// no acknowledgement, mesh carriers only. Returns whether it went out.
    fn send_member_frame(&mut self, peer: &str, frame: &MemberFrame) -> bool {
        let content = format!("{}{}", internal_prefixes::MEMBER, frame.to_body());
        let mut message = match self.create_message(peer, content, Some(MessagePriority::Low), None)
        {
            Ok(message) => message,
            Err(e) => {
                debug!(peer = %peer, error = %e, "Could not build a membership frame");
                return false;
            }
        };
        message.requires_ack = false;
        message.ttl = TTL::from_value(1);
        if let Err(e) = self.sign_control_message(&mut message) {
            debug!(peer = %peer, error = %e, "Could not sign a membership frame");
            return false;
        }
        self.deduplicator.mark_seen_local(message.id.clone());
        self.mesh_relay.mark_handled(&message.id.as_str());
        match self.transport_manager.send_to_neighbor(peer, &message) {
            Ok(_) => true,
            Err(e) => {
                debug!(peer = %peer, error = %e, "No mesh link for a membership frame");
                false
            }
        }
    }
}

/// Whether the publisher signed `cert`.
fn cert_verifies(publisher_key: &[u8; 32], cert: &MemberCert) -> bool {
    offline_protocol_mls::MlsManager::verify_signature(
        publisher_key,
        &MemberCert::signing_payload(&cert.address, cert.not_after),
        &cert.signature,
    )
    .unwrap_or(false)
}
