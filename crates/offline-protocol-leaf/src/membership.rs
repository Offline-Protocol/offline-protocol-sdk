//! Application membership on a leaf ([ADR 0037], "Leaves"): the certificate a
//! paired phone provisions, and the `__MEMBER__` exchange the device then
//! runs with its neighbours exactly as a phone does.
//!
//! # Invariants
//!
//! - **Only the paired phone provisions.** The publisher key and the
//!   certificate arrive as the plaintext of a sealed message inside the 1:1
//!   session, so the frame is opened under a key only that peer holds. Found
//!   anywhere else it is not provisioning at all.
//! - **Durable before acknowledged.** What a provisioning carries is written
//!   to the store before the group state that spends its generation, and the
//!   acknowledgement is minted after both. A store that fails leaves the
//!   device unprovisioned, unacknowledged and able to open the retransmission,
//!   so an acknowledged provisioning is one that survives a power cut.
//! - **No clock is read.** A certificate's `not_after`, this device's own and
//!   every neighbour's, is checked against the latest time this device was
//!   supplied: the time a provisioning carried, or the `now_unix_secs`
//!   firmware passes in, whichever is later. That bounds freshness after a
//!   long gap and is not authentication.
//! - **Nothing observable depends on membership.** Once provisioned, the
//!   device sends one hello to each neighbour firmware reports and answers
//!   every fresh hello with one response, both at their fixed sizes, and the
//!   answer costs one identity signature and one seal whether it carries a
//!   credential or random bytes. The frames carry a TTL of one, ask for no
//!   acknowledgement, and are never acknowledged.
//! - **Unprovisioned is today.** A device with no certificate sends no
//!   membership frame and answers none. It remembers the last hello each
//!   neighbour sent, in memory only, so the exchange can be answered the
//!   moment a provisioning arrives instead of waiting out the neighbour's
//!   interval.
//!
//! # Cryptography
//!
//! The layouts are `offline-protocol-sealed`'s, so this module builds the
//! same bytes the engine does (ADR 0022). The primitives are mls-rs's:
//! HKDF-SHA256, HMAC-SHA256 and Ed25519 from the protocol's ciphersuite, and
//! ChaCha20-Poly1305 from the provider's ChaCha suite, since the protocol's
//! own suite seals with AES-GCM and the exchange is pinned to
//! ChaCha20-Poly1305. No new dependency: both suites are the same provider.
//!
//! [ADR 0037]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md

use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mls_rs::{CipherSuite, CipherSuiteProvider, CryptoProvider};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;
use offline_protocol_core::{Address, AppId, Message, MessagePriority, TTL};
use offline_protocol_sealed::membership::{
    app_key_info, exchange_key_info, exchange_salt, hello_tag_input, transcript_signing_payload,
    MemberCert, MemberCredential, MemberFrame, MemberHello, MemberProvision, APP_KEY_LEN,
    MEMBER_CREDENTIAL_LEN, MEMBER_NONCE_LEN, MEMBER_SEALED_LEN, MEMBER_TAG_LEN, WIRE_APP_ID,
};
use offline_protocol_sealed::{prefixes, CTRL_PK_META_KEY};

use crate::device::{ignored, Handled, LeafDevice, LeafEvent};
use crate::error::{LeafError, Result};
use crate::frames;
use crate::identity::{self, suite_provider};
use crate::store::KEY_TYPE_MEMBERSHIP;

/// The one entry under [`KEY_TYPE_MEMBERSHIP`].
const KEY_ID_PROVISION: &str = "provision";

/// Seconds an exchange with one neighbour stands before a later discovery of
/// that neighbour starts a new one: the engine's interval, so a phone and a
/// device restart on the same cadence.
pub(crate) const MEMBER_EXCHANGE_INTERVAL_SECS: u64 = 5 * 60;

/// Neighbours with an exchange in memory. Bounded like the peer table, and
/// for the same reason: a neighbourhood that mints addresses must not grow
/// what a part with a few hundred kilobytes of RAM holds. The least recently
/// started goes first.
pub(crate) const MAX_MEMBER_EXCHANGES: usize = 16;

/// Peers held visible. The one whose certificate ends soonest goes first.
pub(crate) const MAX_VISIBLE_MEMBERS: usize = 16;

/// Nonces of one peer answered under one nonce of ours, as on the engine.
const MAX_ANSWERED_PER_EXCHANGE: usize = 8;

/// What a provisioning left on this device.
#[derive(Clone)]
pub(crate) struct Provisioned {
    publisher_key: [u8; 32],
    cert: MemberCert,
    /// The latest time a provisioning supplied, Unix seconds.
    supplied_time: u64,
    /// `K_app`, derived from the publisher key and this device's app id.
    app_key: [u8; APP_KEY_LEN],
}

struct Exchange {
    /// The `now_unix_secs` the exchange started at.
    started: u64,
    ours: [u8; MEMBER_NONCE_LEN],
    theirs: Option<[u8; MEMBER_NONCE_LEN]>,
    answered: Vec<[u8; MEMBER_NONCE_LEN]>,
    /// A response that arrived ahead of the hello it answers, with the
    /// identity key its frame was signed by.
    pending_response: Option<([u8; MEMBER_SEALED_LEN], Vec<u8>)>,
    verified: bool,
}

/// This device's side of the membership exchange. Only the provisioning is
/// durable; the rest is per-link state a power cut rebuilds on the next
/// discovery, as the engine's is.
#[derive(Default)]
pub(crate) struct Membership {
    provisioned: Option<Provisioned>,
    exchanges: BTreeMap<String, Exchange>,
    /// The last hello each neighbour sent while this device held no
    /// certificate.
    heard: BTreeMap<String, MemberHello>,
    /// Verified members, by address, with their certificate's `not_after`.
    visible: BTreeMap<String, u64>,
}

impl Membership {
    /// Whether this device holds a certificate.
    pub(crate) fn is_provisioned(&self) -> bool {
        self.provisioned.is_some()
    }

    /// The latest time this device was supplied: a provisioning's or the
    /// caller's, whichever is later.
    fn effective_time(&self, now_unix_secs: u64) -> u64 {
        self.provisioned
            .as_ref()
            .map_or(now_unix_secs, |p| p.supplied_time.max(now_unix_secs))
    }
}

/// Reads the provisioning a previous boot stored.
///
/// A record that does not parse, or no longer names `address`, is read as no
/// provisioning rather than as an error: refusing to resume would brick a
/// device over a bound on visibility, and a device without one behaves as it
/// did before provisioning existed until its phone provisions it again. The
/// checks are the ones a fresh provisioning gets, so a record cannot become
/// more than what was accepted.
pub(crate) fn load(
    store: &crate::shared::Arc<dyn crate::store::LeafStore>,
    address: &Address,
    app_id: &AppId,
) -> Result<Membership> {
    let Some(raw) = store
        .load(KEY_TYPE_MEMBERSHIP, KEY_ID_PROVISION)
        .map_err(|e| LeafError::Storage(e.to_string()))?
    else {
        return Ok(Membership::default());
    };
    let provisioned = core::str::from_utf8(&raw)
        .ok()
        .and_then(|body| MemberProvision::parse(body).ok())
        .filter(|p| p.cert.address == *address && cert_verifies(&p.publisher_key, &p.cert))
        .map(|p| derive(p, app_id))
        .transpose()?;
    Ok(Membership {
        provisioned,
        ..Membership::default()
    })
}

fn derive(p: MemberProvision, app_id: &AppId) -> Result<Provisioned> {
    let app_key = hkdf32(&p.publisher_key, &[], &app_key_info(app_id.as_str()))?;
    Ok(Provisioned {
        publisher_key: p.publisher_key,
        cert: p.cert,
        supplied_time: p.supplied_time,
        app_key,
    })
}

impl LeafDevice {
    /// The `app_id` this device stamps on every frame it sends: the constant
    /// `"offline"` once it holds a certificate, so the wire names no
    /// application (ADR 0037, "Nothing in the clear"), and the id it was
    /// opened with before.
    pub(crate) fn wire_app_id(&self) -> AppId {
        if self.membership.is_provisioned() {
            AppId::new(WIRE_APP_ID).unwrap_or_else(|_| self.app_id.clone())
        } else {
            self.app_id.clone()
        }
    }

    /// The `not_after` of this device's own certificate, while it is current
    /// at the latest time this device was supplied. `None` with no
    /// provisioning, and once it has expired: an expired certificate makes
    /// this device a stranger to every member, so its phone owes it a new one.
    pub fn membership_not_after(&self, now_unix_secs: u64) -> Option<u64> {
        let effective = self.membership.effective_time(now_unix_secs);
        self.membership
            .provisioned
            .as_ref()
            .filter(|p| p.cert.is_current(effective))
            .map(|p| p.cert.not_after)
    }

    /// Whether `peer` has proven, in a membership exchange, that it belongs to
    /// this device's application, and its certificate is still current at
    /// the latest time this device was supplied.
    ///
    /// Always `false` on a device with no provisioning. Like every other gate
    /// in this crate it answers who a peer is, never what it may actuate.
    pub fn is_member_visible(&self, peer: &str, now_unix_secs: u64) -> bool {
        let effective = self.membership.effective_time(now_unix_secs);
        self.membership
            .visible
            .get(peer)
            .is_some_and(|not_after| effective <= *not_after)
    }

    /// Firmware reports that `peer` is now a direct neighbour on the radio.
    ///
    /// On a provisioned device this starts the membership exchange: one hello,
    /// handed back to transmit over that link and nowhere else. Nothing is
    /// sent while an exchange with `peer` stands, which is
    /// five minutes from its start (the engine's interval), and never because
    /// `peer` is already visible: whether a hello goes out must not depend on
    /// membership. A device with no provisioning sends nothing, as before.
    pub fn neighbor_discovered(&mut self, peer: &str, now_unix_secs: u64) -> Result<Handled> {
        if !self.membership.is_provisioned() {
            return Ok(Handled::default());
        }
        if peer.parse::<Address>().is_err() || peer == self.identity.address.to_string() {
            return Ok(ignored("a neighbour that is not another address"));
        }
        if let Some(standing) = self.membership.exchanges.get(peer) {
            if now_unix_secs
                < standing
                    .started
                    .saturating_add(MEMBER_EXCHANGE_INTERVAL_SECS)
            {
                return Ok(Handled::default());
            }
        }
        let exchange = fresh_exchange(now_unix_secs)?;
        let hello = self.member_hello_frame(peer, &exchange.ours, now_unix_secs)?;
        self.insert_exchange(peer, exchange);
        Ok(Handled {
            outbound: vec![hello],
            events: Vec::new(),
        })
    }

    /// A `__MEMBER__` frame, already addressed to this device.
    ///
    /// Consumed whatever it says, and never acknowledged. A frame that did
    /// not come straight from its sender (`hop_count > 0`) or whose signature
    /// does not verify is ignored rather than refused: an error is an answer
    /// firmware might act on, and nothing about this frame may be one.
    ///
    /// The control signature is checked under either payload. The
    /// freshness-bound one needs the sender to have learned this device's
    /// capabilities, which only a key package teaches, and a neighbour that
    /// never paired with this device has had none; the exchange carries its
    /// own freshness in its nonces, so the older payload costs nothing here.
    pub(crate) fn on_member_frame(
        &mut self,
        message: &Message,
        body: &str,
        now_unix_secs: u64,
    ) -> Result<Handled> {
        if message.hop_count.value() != 0 {
            return Ok(ignored("a membership frame arrived after a hop"));
        }
        if let Err(e) =
            frames::verify_control_frame(message, now_unix_secs, frames::UndatedPayload::Admitted)
        {
            return Ok(ignored(&format!("a membership frame was refused: {e}")));
        }
        let sender = message.sender.as_str().to_string();
        if sender == self.identity.address.to_string() {
            return Ok(ignored("a membership frame from this device's own address"));
        }
        let frame = match MemberFrame::parse(body) {
            Ok(frame) => frame,
            Err(e) => return Ok(ignored(&format!("a malformed membership frame: {e}"))),
        };

        if !self.membership.is_provisioned() {
            // Remembered, never answered: this device runs no exchange yet,
            // and answering would be the behaviour change provisioning makes.
            if let MemberFrame::Hello(hello) = frame {
                let heard = &mut self.membership.heard;
                if !heard.contains_key(&sender) && heard.len() >= MAX_MEMBER_EXCHANGES {
                    if let Some(victim) = heard.keys().next().cloned() {
                        heard.remove(&victim);
                    }
                }
                heard.insert(sender, hello);
            }
            return Ok(ignored(
                "a membership frame arrived and this device holds no certificate",
            ));
        }

        match frame {
            MemberFrame::Hello(hello) => {
                let outbound = self.on_member_hello(&sender, &hello, now_unix_secs)?;
                let events = self.take_pending_response(&sender, now_unix_secs);
                Ok(Handled { outbound, events })
            }
            MemberFrame::Response(sealed) => {
                let Some(identity_key) = message
                    .metadata
                    .get(CTRL_PK_META_KEY)
                    .and_then(|pk| BASE64.decode(pk).ok())
                else {
                    return Ok(ignored("a membership response carried no key"));
                };
                let Some(exchange) = self.membership.exchanges.get_mut(&sender) else {
                    return Ok(ignored("a membership response with no exchange standing"));
                };
                if exchange.theirs.is_none() {
                    exchange.pending_response = Some((sealed, identity_key));
                    return Ok(Handled::default());
                }
                let events = self
                    .open_member_response(&sender, &sealed, &identity_key, now_unix_secs)
                    .into_iter()
                    .collect();
                Ok(Handled {
                    outbound: Vec::new(),
                    events,
                })
            }
        }
    }

    /// Answers a hello, sending this side's own first when the peer cannot
    /// hold it yet. The engine's rules, step for step: see
    /// `docs/spec/control-messages.md`, "When it runs".
    fn on_member_hello(
        &mut self,
        peer: &str,
        hello: &MemberHello,
        now_unix_secs: u64,
    ) -> Result<Vec<Message>> {
        let mut outbound = Vec::new();
        let standing = self.membership.exchanges.get(peer).is_some_and(|e| {
            now_unix_secs < e.started.saturating_add(MEMBER_EXCHANGE_INTERVAL_SECS)
        });
        if !standing {
            let exchange = fresh_exchange(now_unix_secs)?;
            outbound.push(self.member_hello_frame(peer, &exchange.ours, now_unix_secs)?);
            self.insert_exchange(peer, exchange);
        }

        let Some(exchange) = self.membership.exchanges.get(peer) else {
            return Ok(outbound);
        };
        if exchange.answered.contains(&hello.nonce) {
            return Ok(outbound);
        }
        if exchange.answered.len() >= MAX_ANSWERED_PER_EXCHANGE {
            let fresh = fresh_exchange(now_unix_secs)?;
            outbound.push(self.member_hello_frame(peer, &fresh.ours, now_unix_secs)?);
            self.insert_exchange(peer, fresh);
        } else if !exchange.answered.is_empty() {
            let ours = exchange.ours;
            outbound.push(self.member_hello_frame(peer, &ours, now_unix_secs)?);
        }

        let Some(exchange) = self.membership.exchanges.get_mut(peer) else {
            return Ok(outbound);
        };
        exchange.theirs = Some(hello.nonce);
        exchange.verified = false;
        exchange.answered.push(hello.nonce);
        let ours = exchange.ours;
        outbound.push(self.member_response_frame(peer, &ours, hello, now_unix_secs)?);
        Ok(outbound)
    }

    /// Opens a response that arrived before the hello it answers, now that
    /// the hello is in.
    fn take_pending_response(&mut self, peer: &str, now_unix_secs: u64) -> Vec<LeafEvent> {
        let pending = self
            .membership
            .exchanges
            .get_mut(peer)
            .and_then(|e| e.pending_response.take());
        pending
            .and_then(|(sealed, key)| self.open_member_response(peer, &sealed, &key, now_unix_secs))
            .into_iter()
            .collect()
    }

    /// Verifies a peer's response and, on a pass, makes it visible.
    fn open_member_response(
        &mut self,
        peer: &str,
        sealed: &[u8; MEMBER_SEALED_LEN],
        identity_key: &[u8],
        now_unix_secs: u64,
    ) -> Option<LeafEvent> {
        let provisioned = self.membership.provisioned.as_ref()?;
        let exchange = self.membership.exchanges.get(peer)?;
        let (Some(theirs), false) = (exchange.theirs, exchange.verified) else {
            return None;
        };
        let ours = exchange.ours;
        let local = self.identity.address;
        let sender: Address = peer.parse().ok()?;
        let salt = exchange_salt(&local, &ours, &sender, &theirs).ok()?;
        let key = hkdf32(&provisioned.app_key, &salt, &exchange_key_info(&sender)).ok()?;
        // A stranger's random bytes, or a response to a nonce of ours since
        // replaced. Neither is reported.
        let plaintext = aead_open(&key, sealed)?;
        let credential = MemberCredential::from_bytes(&plaintext).ok()?;
        let cert = &credential.cert;
        if cert.address != sender || !cert_verifies(&provisioned.publisher_key, cert) {
            return None;
        }
        if !cert.is_current(self.membership.effective_time(now_unix_secs)) {
            return None;
        }
        let transcript = transcript_signing_payload(&sender, &theirs, &local, &ours);
        identity::verify(identity_key, &credential.signature, &transcript).ok()?;

        let not_after = cert.not_after;
        if let Some(exchange) = self.membership.exchanges.get_mut(peer) {
            exchange.verified = true;
        }
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
        Some(LeafEvent::MemberVisible {
            peer: peer.to_string(),
            not_after,
        })
    }

    fn insert_exchange(&mut self, peer: &str, exchange: Exchange) {
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

    /// A hello: a fresh nonce and the tag over it under `K_app`.
    fn member_hello_frame(
        &mut self,
        peer: &str,
        ours: &[u8; MEMBER_NONCE_LEN],
        now_unix_secs: u64,
    ) -> Result<Message> {
        let provisioned =
            self.membership
                .provisioned
                .as_ref()
                .ok_or(LeafError::MembershipRefused(String::from(
                    "this device holds no certificate",
                )))?;
        let peer_addr: Address = peer
            .parse()
            .map_err(|e| LeafError::IdentityBinding(format!("'{peer}' is not an address: {e}")))?;
        let full = hmac(
            &provisioned.app_key,
            &hello_tag_input(ours, &self.identity.address, &peer_addr),
        )?;
        let mut tag = [0u8; MEMBER_TAG_LEN];
        tag.copy_from_slice(&full[..MEMBER_TAG_LEN]);
        let frame = MemberFrame::Hello(MemberHello { nonce: *ours, tag });
        self.member_frame(peer, &frame, now_unix_secs)
    }

    /// A response: a sealed credential when the hello's tag verifies, and
    /// otherwise random bytes made by the same signature, derivation and seal,
    /// so neither the size nor the cost says which ran.
    fn member_response_frame(
        &mut self,
        peer: &str,
        ours: &[u8; MEMBER_NONCE_LEN],
        hello: &MemberHello,
        now_unix_secs: u64,
    ) -> Result<Message> {
        let provisioned =
            self.membership
                .provisioned
                .clone()
                .ok_or(LeafError::MembershipRefused(String::from(
                    "this device holds no certificate",
                )))?;
        let local = self.identity.address;
        let peer_addr: Address = peer
            .parse()
            .map_err(|e| LeafError::IdentityBinding(format!("'{peer}' is not an address: {e}")))?;
        let expected = hmac(
            &provisioned.app_key,
            &hello_tag_input(&hello.nonce, &peer_addr, &local),
        )?;
        let member = constant_time_eq(&expected[..MEMBER_TAG_LEN], &hello.tag);
        let salt = exchange_salt(&local, ours, &peer_addr, &hello.nonce)?;

        let transcript = transcript_signing_payload(&local, ours, &peer_addr, &hello.nonce);
        let signature = identity::sign(&self.identity, &transcript)?;
        let ikm = if member {
            provisioned.app_key
        } else {
            random_array::<APP_KEY_LEN>()?
        };
        let key = hkdf32(&ikm, &salt, &exchange_key_info(&local))?;
        let mut plaintext = [0u8; MEMBER_CREDENTIAL_LEN];
        if member {
            let cert = provisioned.cert.to_bytes();
            plaintext[..cert.len()].copy_from_slice(&cert);
        }
        let at = MEMBER_CREDENTIAL_LEN
            .checked_sub(signature.len())
            .ok_or_else(|| LeafError::Crypto(String::from("an identity signature too long")))?;
        plaintext[at..].copy_from_slice(&signature);
        let sealed = aead_seal(&key, &plaintext)?;
        let sealed = <[u8; MEMBER_SEALED_LEN]>::try_from(sealed.as_slice())
            .map_err(|_| LeafError::Crypto(String::from("a sealed credential of another size")))?;
        self.member_frame(peer, &MemberFrame::Response(sealed), now_unix_secs)
    }

    /// One membership frame for the neighbour's link: signed, TTL one, no
    /// acknowledgement requested.
    fn member_frame(
        &mut self,
        peer: &str,
        frame: &MemberFrame,
        now_unix_secs: u64,
    ) -> Result<Message> {
        let mut message = frames::build(
            &self.store,
            &self.identity,
            &self.wire_app_id(),
            peer,
            format!("{}{}", prefixes::MEMBER, frame.to_body()),
            now_unix_secs,
            MessagePriority::Low,
        )?;
        message.ttl = TTL::from_value(1);
        frames::sign_control_frame(&self.identity, &mut message)?;
        Ok(message)
    }

    /// Checks a provisioning and makes it durable, without yet acting on it.
    ///
    /// Called before the group state that spends the provisioning's
    /// generation is written, so a refusal or a store that fails leaves the
    /// device able to open the retransmission, and nothing is acknowledged.
    pub(crate) fn store_provision(&mut self, body: &str) -> Result<Provisioned> {
        let provision = MemberProvision::parse(body)?;
        if provision.cert.address != self.identity.address {
            return Err(LeafError::MembershipRefused(String::from(
                "the certificate names another address than this device",
            )));
        }
        if !cert_verifies(&provision.publisher_key, &provision.cert) {
            return Err(LeafError::MembershipRefused(String::from(
                "the certificate does not verify under the publisher key it came with",
            )));
        }
        // Never backwards: a provisioning that supplies an earlier time than
        // one already held keeps the later one, so a replayed or delayed
        // frame cannot make an expired certificate current again.
        let supplied_time = self
            .membership
            .provisioned
            .as_ref()
            .map_or(provision.supplied_time, |p| {
                p.supplied_time.max(provision.supplied_time)
            });
        if !provision.cert.is_current(supplied_time) {
            return Err(LeafError::MembershipRefused(String::from(
                "the certificate has expired against the time supplied with it",
            )));
        }
        let provision = MemberProvision {
            supplied_time,
            ..provision
        };
        let provisioned = derive(provision.clone(), &self.app_id)?;
        self.store
            .store(
                KEY_TYPE_MEMBERSHIP,
                KEY_ID_PROVISION,
                provision.to_body().as_bytes(),
            )
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        Ok(provisioned)
    }

    /// Acts on a provisioning [`Self::store_provision`] made durable: holds
    /// it, and when the certificate or the key changed, starts a fresh
    /// exchange with every neighbour this device has heard from and with the
    /// phone that sent it, answering any hello already heard.
    ///
    /// The same certificate again only refreshes the time, and restarts
    /// nothing: a burst of hellos with no link change is something only a
    /// configured node produces.
    pub(crate) fn install_provision(
        &mut self,
        provisioned: Provisioned,
        sender: &str,
        now_unix_secs: u64,
    ) -> Result<Handled> {
        let not_after = provisioned.cert.not_after;
        let changed = self.membership.provisioned.as_ref().is_none_or(|held| {
            held.cert != provisioned.cert || held.publisher_key != provisioned.publisher_key
        });
        let key_changed = self
            .membership
            .provisioned
            .as_ref()
            .is_some_and(|held| held.publisher_key != provisioned.publisher_key);
        self.membership.provisioned = Some(provisioned);
        let mut events = vec![LeafEvent::MembershipProvisioned {
            peer: sender.to_string(),
            not_after,
        }];
        if !changed {
            return Ok(Handled {
                outbound: Vec::new(),
                events,
            });
        }
        if key_changed {
            // Verified under a key this application no longer holds.
            self.membership.visible.clear();
        }

        let heard = core::mem::take(&mut self.membership.heard);
        let mut neighbours: Vec<String> = self.membership.exchanges.keys().cloned().collect();
        for peer in heard.keys().chain(core::iter::once(&sender.to_string())) {
            if !neighbours.contains(peer) {
                neighbours.push(peer.clone());
            }
        }
        self.membership.exchanges.clear();

        let mut outbound = Vec::new();
        for peer in neighbours.iter().take(MAX_MEMBER_EXCHANGES) {
            let exchange = fresh_exchange(now_unix_secs)?;
            outbound.push(self.member_hello_frame(peer, &exchange.ours, now_unix_secs)?);
            self.insert_exchange(peer, exchange);
            if let Some(hello) = heard.get(peer) {
                // A hello heard before this device could answer it: answered
                // now, under this side's fresh nonce, so the key is new. The
                // exchange just started stands, so this is the answer alone.
                outbound.extend(self.on_member_hello(peer, hello, now_unix_secs)?);
                events.extend(self.take_pending_response(peer, now_unix_secs));
            }
        }
        Ok(Handled { outbound, events })
    }
}

fn fresh_exchange(now_unix_secs: u64) -> Result<Exchange> {
    Ok(Exchange {
        started: now_unix_secs,
        ours: random_array()?,
        theirs: None,
        answered: Vec::new(),
        pending_response: None,
        verified: false,
    })
}

/// Whether the publisher signed `cert`.
fn cert_verifies(publisher_key: &[u8; 32], cert: &MemberCert) -> bool {
    identity::verify(
        publisher_key,
        &cert.signature,
        &MemberCert::signing_payload(&cert.address, cert.not_after),
    )
    .is_ok()
}

/// `HKDF-SHA256(ikm, salt, info)`, 32 bytes. An empty salt is HKDF's
/// default, a hash-length string of zeros, which is what the chapter's
/// `salt = none` means.
pub(crate) fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> Result<[u8; 32]> {
    let suite = suite_provider()?;
    let prk = suite
        .kdf_extract(salt, ikm)
        .map_err(|e| LeafError::Crypto(format!("HKDF extract: {e:?}")))?;
    let okm = suite
        .kdf_expand(&prk, info, 32)
        .map_err(|e| LeafError::Crypto(format!("HKDF expand: {e:?}")))?;
    <[u8; 32]>::try_from(okm.as_slice())
        .map_err(|_| LeafError::Crypto(String::from("HKDF returned another length")))
}

/// `HMAC-SHA256(key, data)`, all 32 bytes.
fn hmac(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    suite_provider()?
        .mac(key, data)
        .map_err(|e| LeafError::Crypto(format!("HMAC: {e:?}")))
}

/// The ChaCha20-Poly1305 provider. The protocol's own suite seals with
/// AES-GCM, and the exchange is pinned to ChaCha20-Poly1305.
fn chacha() -> Result<<RustCryptoProvider as CryptoProvider>::CipherSuiteProvider> {
    RustCryptoProvider::with_enabled_cipher_suites(vec![CipherSuite::CURVE25519_CHACHA])
        .cipher_suite_provider(CipherSuite::CURVE25519_CHACHA)
        .ok_or_else(|| LeafError::Crypto(String::from("the provider has no ChaCha20-Poly1305")))
}

const ZERO_NONCE: [u8; 12] = [0u8; 12];

fn aead_seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    chacha()?
        .aead_seal(key, plaintext, None, &ZERO_NONCE)
        .map_err(|e| LeafError::Crypto(format!("seal: {e:?}")))
}

fn aead_open(key: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    chacha()
        .ok()?
        .aead_open(key, ciphertext, None, &ZERO_NONCE)
        .ok()
        .map(|plaintext| plaintext.to_vec())
}

fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    suite_provider()?
        .random_bytes(&mut out)
        .map_err(|e| LeafError::Crypto(format!("no entropy: {e:?}")))?;
    Ok(out)
}

/// Compares two tags in time that does not depend on where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// This module's primitives against the membership vectors, which the
/// engine's are pinned to as well: a byte of difference between the two
/// would be a leaf every phone reads as a stranger.
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::{eprintln, fs, path::PathBuf};

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// Read at run time rather than compiled in: the file lives in the
    /// sealed crate, which a packaged `.crate` of this one does not carry.
    fn vectors() -> Option<serde_json::Value> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../offline-protocol-sealed/tests/data/app-membership-v1.vectors.json");
        match fs::read_to_string(path) {
            Ok(text) => Some(serde_json::from_str(&text).expect("JSON")),
            Err(_) => {
                eprintln!("skipping: the membership vectors are not readable from here");
                None
            }
        }
    }

    #[test]
    fn the_application_key_and_both_exchange_keys_match_the_vectors() {
        let Some(v) = vectors() else { return };
        let s = |path: &[&str]| -> String {
            path.iter()
                .fold(&v, |v, key| &v[*key])
                .as_str()
                .expect("a string")
                .to_string()
        };
        let app_id = AppId::new(s(&["app_key", "app_id"])).expect("an app id");
        let pk = unhex(&s(&["app_key", "publisher_public_key_hex"]));
        let k_app = hkdf32(&pk, &[], &app_key_info(app_id.as_str())).expect("K_app");
        assert_eq!(k_app.to_vec(), unhex(&s(&["app_key", "k_app_hex"])));

        let salt = unhex(&s(&["exchange", "salt_hex"]));
        for side in ["a", "b"] {
            let info = unhex(&s(&["exchange", &format!("key_info_{side}_hex")]));
            assert_eq!(
                hkdf32(&k_app, &salt, &info).expect("a key").to_vec(),
                unhex(&s(&["exchange", &format!("key_{side}_hex")]))
            );
        }

        let tag = hmac(&k_app, &unhex(&s(&["hello", "tag_input_hex"]))).expect("a tag");
        assert_eq!(
            tag[..MEMBER_TAG_LEN].to_vec(),
            unhex(&s(&["hello", "tag_hex"]))
        );
    }

    /// The exchange seals under ChaCha20-Poly1305 with a zero nonce and no
    /// associated data: what one end seals the other opens, and nothing else
    /// does.
    #[test]
    fn a_seal_opens_under_its_key_only() {
        let key = [7u8; 32];
        let sealed = aead_seal(&key, &[1u8; MEMBER_CREDENTIAL_LEN]).expect("sealed");
        assert_eq!(sealed.len(), MEMBER_SEALED_LEN);
        assert_eq!(
            aead_open(&key, &sealed),
            Some(vec![1u8; MEMBER_CREDENTIAL_LEN])
        );
        assert_eq!(aead_open(&[8u8; 32], &sealed), None);
    }
}
