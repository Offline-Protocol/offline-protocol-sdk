//! The device: what it does with a frame, and what it hands back.

use alloc::{
    format,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mls_rs::client_builder::MlsConfig;
use mls_rs::group::ReceivedMessage;
use mls_rs::{Client, Group, MlsMessage};
use offline_protocol_core::{Address, AppId, Message, MessagePriority};
use offline_protocol_sealed::{
    prefixes, EncryptedMessage, GroupId, KeyPackagePayload, MlsMessageType, WelcomeMessage,
    MLS_ENVELOPE_COMPACT_V1,
};

use crate::adapters::{KeyPackageAdapter, PeerRecord, PRIOR_EPOCH_RETENTION};
use crate::error::{LeafError, Result};
use crate::frames;
use crate::identity::{build_client, Identity};
use crate::keypkg;
use crate::store::{
    LeafStore, KEY_TYPE_GROUP_EPOCH, KEY_TYPE_GROUP_STATE, KEY_TYPE_KEY_PACKAGE, KEY_TYPE_PEER,
    KEY_TYPE_PEER_INDEX,
};

/// How many peers an **inbound** frame may add records for.
///
/// Every peer that sends a well-formed key package gets a record and a minted
/// package in return, both of which are writes to flash. The signature on that
/// frame proves the sender holds the key its address derives from, which is
/// exactly as hard as generating a key, so without a bound a stranger fills a
/// device's storage for the cost of some signing. The phone bounds the same
/// exchange for the same reason.
///
/// It bounds what arrives, not what firmware chooses:
/// [`LeafDevice::key_package_frame`] is the integrator deciding to pair and is
/// not held to this. The failure being prevented is a stranger spending a
/// device's flash, not an owner spending their own.
///
/// Sixteen covers a household and its guests. What it costs when full is
/// stated on [`LeafError::TooManyPeers`].
const MAX_PEERS: usize = 16;

/// How far past the retained window [`LeafDevice::unpair`] sweeps, in **both**
/// directions.
///
/// [`PRIOR_EPOCH_RETENTION`] says what a healthy device holds. This is the
/// margin for one that is not, and each side of the window needs it for a
/// different reason. Below: a delete that failed during trimming leaves a
/// record holding an epoch's secrets. Above: the adapter writes epoch records
/// before the state entry that sequences them, so a cut between the two leaves
/// a record that every anchor agrees is not there. Forgetting a peer is the
/// moment to clear both rather than the moment to assume they are absent.
const FORGET_EPOCH_SLACK: u64 = 16;

/// Something that happened, for the firmware to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafEvent {
    /// A peer advertised itself. The device now knows what it may emit.
    PeerAdvertised {
        /// The peer's address.
        peer: String,
    },
    /// A session came up. The device can seal to this peer from here.
    SessionEstablished {
        /// The peer's address.
        peer: String,
    },
    /// A peer discarded its session and the device followed.
    SessionReset {
        /// The peer's address.
        peer: String,
    },
    /// An application message arrived and decrypted.
    ///
    /// `peer` is proven: it is the address the sealing group member's own
    /// signature key derives to. What it is not is permission. Any address in
    /// radio range can complete a pairing, so firmware decides what a message
    /// from this particular peer is allowed to actuate. A lock that opens for
    /// whatever arrives on an established session opens for anyone patient
    /// enough to pair with it.
    MessageReceived {
        /// The peer's address, proven rather than claimed.
        peer: String,
        /// The plaintext.
        text: String,
    },
    /// A commit arrived and was applied. Post-compromise security advances
    /// here, on the cadence the peer sets, because this device never commits.
    CommitApplied {
        /// The peer's address.
        peer: String,
        /// The epoch the session is now in.
        epoch: u64,
    },
    /// A frame was not for this device, or carried nothing to act on.
    ///
    /// Surfaced rather than swallowed so a bench can tell "refused" from
    /// "silently dropped", which are the two states a pairing failure looks
    /// like from the outside.
    Ignored {
        /// Why nothing happened.
        reason: String,
    },
}

/// What a frame produced.
#[derive(Debug, Default)]
pub struct Handled {
    /// Frames to transmit, in order. Every one is already durable.
    pub outbound: Vec<Message>,
    /// What the firmware should know about.
    pub events: Vec<LeafEvent>,
}

/// What a frame proved about the peer it names.
///
/// A frame's sender is a plaintext field, and proving it is the whole business
/// of the handlers [`LeafDevice::dispatch`] routes to. Two of that function's
/// arms prove nothing, because a frame carrying nothing this device acts on
/// gives them nothing to work with, and those two are not failures: they
/// return normally, with a reason.
///
/// So "did not fail" and "proved who sent it" are different questions, and an
/// acknowledgement is owed only for the second. Answering on the first would
/// hand anyone within radio range exactly what
/// [`LeafDevice::acknowledge`]'s known-peer gate exists to withhold, because
/// naming a paired peer costs an attacker nothing beyond having overheard one
/// frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sender {
    /// A control signature verified against the address the frame names, or
    /// the frame opened under a key only that peer holds.
    Proven,
    /// Nothing was checked. The sender is whatever the last hand to touch the
    /// frame wrote there.
    Claimed,
}

/// A leaf node.
///
/// # The never-committing profile
///
/// The peer creates the group, adds this device, and issues every commit. The
/// device joins, opens what arrives, answers, and persists. It never emits a
/// Welcome, a commit or a proposal, and never any group, rich, document or
/// relay frame. Per-commit cost here is two elliptic-curve operations;
/// per-message cost is symmetric only.
///
/// Post-compromise security therefore arrives on the peer's cadence rather
/// than this device's, and a driven rekey reaches it as a key package with
/// `session_reset` set rather than as an unsolicited Welcome.
///
/// # Storage is the source of truth
///
/// No MLS state is cached in this value. Every operation loads the group from
/// the store, does its work, writes back, and only then returns the frame to
/// send. That costs a load per frame, which is the right trade for a device
/// that handles a frame every few minutes and must survive losing power
/// between any two instructions: there is no in-RAM state for a power cut to
/// desynchronize from what is on flash.
///
/// # One writer at a time
///
/// Every operation that advances state takes `&mut self`, so the compiler
/// enforces what the ratchet requires. Two seals running at once would load
/// the same generation, encrypt under the same key and nonce, and emit both
/// frames: the AEAD nonce reuse this crate's whole persist-before-emit rule
/// exists to prevent, arrived at without ever losing power. The same argument
/// covers the send counter, where a race mints one id twice and the peer's
/// deduplicator swallows the second message.
///
/// A device is therefore one value, exclusively held. Two `LeafDevice`s over
/// one [`LeafStore`] would each satisfy the borrow checker and race anyway;
/// construct one per store.
///
/// # What it does not defend against
///
/// **Anyone pairing.** Every gate here answers "is this peer the address it
/// claims to be", and none of them answers "did the owner mean this peer".
/// Producing a key that derives to its own address costs nothing, so an
/// unattended device admits whoever asks, up to the bound above. That is the
/// same position two phones are in, where the out-of-band artifact carries
/// first-contact trust; on a device it is firmware that decides when the radio
/// accepts a pairing at all, and firmware that decides what an opened message
/// may actuate. [`LeafDevice::peers`] is how it audits what accumulated.
///
/// A replayed control frame, **as far as this device's clock is honest**. The
/// signed payload states when the frame was made, so one older than two days
/// is refused outright, and a reset-flagged key package (the destructive case,
/// since it tears down a live session) is additionally refused unless it is
/// newer than the last reset this device acted on. One capture is therefore
/// worth one teardown, and only inside a two-day window.
///
/// What that leaves is the clock. Both checks are made against the time source
/// firmware supplies, so a device that supplies a wrong one either refuses its
/// peer or, if the clock is far enough behind, admits frames it should have
/// refused. The obligation is stated with the others in
/// [the provisioning chapter](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/spec/leaf-provisioning.md),
/// and it is why the time source is a parameter on every entry point here
/// rather than something this crate reaches for.
///
/// `Debug` renders the device's address and nothing else. Everything else it
/// holds is either secret or a handle to secrets, and a device that printed
/// its identity key into a log would undo the part of the threat model that
/// says the key never leaves storage.
pub struct LeafDevice {
    store: Arc<dyn LeafStore>,
    identity: Identity,
    app_id: AppId,
}

impl LeafDevice {
    /// Generates an identity and returns a device that holds it.
    ///
    /// Draws from the `getrandom` backend the firmware registered. Refuses if
    /// the store already holds an identity, because replacing one changes the
    /// device's address and silently orphans every peer paired with it.
    pub fn provision(store: Arc<dyn LeafStore>, app_id: &str) -> Result<Self> {
        let identity = Identity::provision(&store)?;
        Ok(Self {
            store,
            identity,
            app_id: parse_app_id(app_id)?,
        })
    }

    /// Loads a device that was provisioned earlier.
    pub fn resume(store: Arc<dyn LeafStore>, app_id: &str) -> Result<Self> {
        let identity = Identity::resume(&store)?;
        Ok(Self {
            store,
            identity,
            app_id: parse_app_id(app_id)?,
        })
    }

    /// Loads a device, provisioning one on first boot.
    pub fn open(store: Arc<dyn LeafStore>, app_id: &str) -> Result<Self> {
        match Self::resume(Arc::clone(&store), app_id) {
            Err(LeafError::NotProvisioned) => Self::provision(store, app_id),
            other => other,
        }
    }

    /// This device's address. Self-certifying: it is the hash of the identity
    /// key, so a peer checks a claim rather than trusting a directory.
    pub fn address(&self) -> &Address {
        &self.identity.address
    }

    /// Mints a key package and wraps it in a signed frame for `peer`.
    ///
    /// `now_unix_secs` is the pairing time source. It is a parameter because a
    /// device has no clock, and passing something wrong here is the difference
    /// between pairing and being refused as expired.
    pub fn key_package_frame(&mut self, peer: &str, now_unix_secs: u64) -> Result<Message> {
        self.key_package_frame_inner(peer, now_unix_secs, false)
    }

    fn key_package_frame_inner(
        &mut self,
        peer: &str,
        now_unix_secs: u64,
        session_reset: bool,
    ) -> Result<Message> {
        let client = self.client()?;
        let minted = keypkg::mint(&client, now_unix_secs)?;
        let payload = keypkg::payload(
            &self.identity.address.to_string(),
            minted.data,
            session_reset,
        );
        let body = serde_json::to_string(&payload)
            .map_err(|e| LeafError::MalformedFrame(format!("cannot encode key package: {e}")))?;

        let mut message = frames::build(
            &self.store,
            &self.identity,
            &self.app_id,
            peer,
            format!("{}{}", prefixes::KEY_PACKAGE, body),
            now_unix_secs,
            MessagePriority::High,
        )?;
        frames::sign_control_frame(&self.identity, &mut message)?;

        // Recorded before the frame is handed back, so a device that emits one
        // and loses power does not emit a second on the next boot and leave
        // the peer holding two init keys of which only one is ever spent.
        //
        // The reference is recorded here for a second reason: it is the only
        // thing that later distinguishes this peer's Welcome from one built by
        // whoever else heard this frame. Minting has already written the
        // package itself, so a cut between the two leaves a package no Welcome
        // can name, which the adapter evicts in its turn. The opposite order
        // would leave a name pointing at nothing.
        let mut record = self.peer_record(peer)?;
        record.key_package_sent = true;
        record.key_package_ref = Some(minted.reference);
        record
            .save(&self.store, peer)
            .map_err(|e| LeafError::Storage(e.to_string()))?;

        // Handing out a package is the moment a pairing with this peer becomes
        // possible, so it is the moment the peer becomes something firmware
        // should be able to see in [`LeafDevice::peers`]. Inbound exchanges
        // reach here having already been admitted, where this is a no-op.
        self.index_peer(peer)?;

        Ok(message)
    }

    /// Seals `plaintext` to `peer`.
    ///
    /// The ratchet advances, the new state is persisted, and only then does
    /// the frame exist. A store that fails produces an error and no frame,
    /// which is the whole point: a device that emitted first and persisted
    /// second would, after a power cut, come back and reuse an AEAD nonce.
    pub fn seal(&mut self, peer: &str, plaintext: &str, now_unix_secs: u64) -> Result<Message> {
        self.seal_content(peer, plaintext.as_bytes(), now_unix_secs)
    }

    fn seal_content(
        &mut self,
        peer: &str,
        plaintext: &[u8],
        now_unix_secs: u64,
    ) -> Result<Message> {
        let client = self.client()?;
        let group_id = self.group_id(peer)?;
        let mut group = self.load_group(&client, peer, &group_id)?;

        // Nothing is sealed into a group that stopped being this pair. The
        // roster is the only thing that says it did, and the commit that
        // changed it is applied and durable by the time anything can read it,
        // so the question has to be asked at the use and not only at the
        // change: a refusal returned once is a value firmware may drop and a
        // reboot certainly does, and the device would then seal every later
        // message into a room with a member the owner never chose.
        self.require_still_a_pair(&group, peer)?;

        let sealed = group
            .encrypt_application_message(plaintext, Vec::new())
            .map_err(|e| LeafError::Mls(format!("cannot seal: {e:?}")))?
            .to_bytes()
            .map_err(|e| LeafError::Mls(format!("cannot encode sealed message: {e:?}")))?;
        let epoch = group.current_epoch();

        // Persist before emit. Everything below this line only shapes bytes
        // that are already accounted for on flash.
        group
            .write_to_storage()
            .map_err(|e| LeafError::Storage(format!("cannot persist group state: {e:?}")))?;

        let envelope = EncryptedMessage {
            group_id,
            message_type: MlsMessageType::Application,
            epoch,
            ciphertext: sealed,
            sender_id: self.identity.address.to_string(),
            timestamp_ms: (now_unix_secs.saturating_mul(1000)),
        };

        let body = self.encode_envelope(peer, &envelope)?;
        frames::build(
            &self.store,
            &self.identity,
            &self.app_id,
            peer,
            format!("{}{}", prefixes::ENCRYPTED, body),
            now_unix_secs,
            MessagePriority::Medium,
        )
    }

    /// Chooses the envelope encoding for this peer.
    ///
    /// Compact only when the peer advertised it. Otherwise the JSON floor,
    /// which every conforming receiver parses unconditionally and which no
    /// negotiation ever removes.
    fn encode_envelope(&self, peer: &str, envelope: &EncryptedMessage) -> Result<String> {
        let record = self.peer_record(peer)?;
        if record.env_versions.contains(&MLS_ENVELOPE_COMPACT_V1) {
            Ok(BASE64.encode(envelope.to_bytes()))
        } else {
            serde_json::to_string(envelope)
                .map_err(|e| LeafError::MalformedFrame(format!("cannot encode envelope: {e}")))
        }
    }

    /// Handles one inbound frame.
    ///
    /// Returns the frames to send and what happened. Everything in
    /// [`Handled::outbound`] is already durable by the time it is returned.
    ///
    /// # A frame addressed elsewhere is answered by nothing
    ///
    /// A radio hears what it is not the recipient of, so this is the first
    /// question asked, before a signature is verified or a prefix is read.
    /// Nothing further down asks it again, and neither kind of frame answers
    /// it on its own: a control frame's signature **covers** the recipient
    /// rather than checking it, so one honestly signed for somebody else
    /// verifies perfectly here, and a sealed frame carries no signature at
    /// all, so its recipient is whatever the last hand to touch it wrote
    /// there. Being able to open a frame is a different claim from having been
    /// sent it.
    ///
    /// Three things follow from not asking. An overheard key package admits a
    /// peer, spends flash on a record, mints a private init key nobody asked
    /// this device for, and answers a phone that never addressed it. A sealed
    /// frame this device really can open is acted on after anyone who captured
    /// it rewrote the recipient, because that field is not inside the AEAD.
    /// And every other prefix arrives as an identity-binding failure, so
    /// ordinary traffic between two neighbours reaches firmware wearing the
    /// shape of an attack, on a device whose only account of itself is that
    /// error stream.
    ///
    /// What the check does not do is keep anyone else's ciphertext readable
    /// here: the group and credential gates below hold either way.
    ///
    /// It is [`LeafEvent::Ignored`] rather than an error because overhearing is
    /// what a shared radio does, and because firmware that carries frames for
    /// its neighbours needs "not mine" to be a fact it can act on rather than a
    /// failure it has to interpret.
    pub fn handle(&mut self, message: &Message, now_unix_secs: u64) -> Result<Handled> {
        if !self.is_addressed_to_me(message) {
            return Ok(ignored("frame is addressed to another node"));
        }

        // Before the frame is opened, because opening it a second time is what
        // fails: the ratchet has spent that generation and refuses it, and the
        // peer is only asking again because the first answer went missing.
        if let Some(repeat) = self.repeat_acknowledgement(message, now_unix_secs)? {
            return Ok(repeat);
        }

        let (mut handled, sender) = self.dispatch(message, now_unix_secs)?;

        // Only a frame that proved who sent it is answered, which is narrower
        // than "was not refused". An `Err` above never reaches here, and that
        // much is the phone's own rule and the reason for it: an
        // acknowledgement is a receipt, and handing one to whoever just failed
        // the signature gate tells them their frames are being processed. But
        // a frame carrying no prefix fails nothing, so it needs the other half
        // of the question: see [`Sender`].
        if sender == Sender::Proven {
            match self.acknowledge(message, now_unix_secs) {
                Ok(Some(ack)) => handled.outbound.push(ack),
                Ok(None) => {}
                // Never propagated, because by here the frame is open and what
                // it produced is already owed to firmware: the ratchet has
                // spent that generation, so returning the error would discard
                // an unlock this device really did receive, and the
                // retransmission that followed would be refused as a replay
                // with nothing remembered to answer it from. The answer is
                // dropped instead, which costs the retry ladder that ran
                // before any of this existed and is recovered by it.
                Err(e) => handled.events.push(LeafEvent::Ignored {
                    reason: format!("the answer this frame is owed could not be stored: {e}"),
                }),
            }
        }

        Ok(handled)
    }

    /// Routes a frame to the handler for its prefix, and says whether anything
    /// on that route proved the sender the frame names.
    ///
    /// Every handler below either verifies a control signature as its first
    /// act or opens the frame under the pair's group key, on every path that
    /// returns normally. That is what makes their success [`Sender::Proven`],
    /// and a handler that stopped doing it would have to say so here.
    fn dispatch(&mut self, message: &Message, now_unix_secs: u64) -> Result<(Handled, Sender)> {
        let content = &message.content;

        // Order matters: the encrypted-confirm prefix is not checked here at
        // all, because it never travels as a frame. It is only ever found
        // inside a decrypted plaintext, which is where this looks for it.
        if let Some(body) = frames::strip_prefix(content, prefixes::KEY_PACKAGE) {
            Ok((
                self.on_key_package(message, body, now_unix_secs)?,
                Sender::Proven,
            ))
        } else if let Some(body) = frames::strip_prefix(content, prefixes::WELCOME) {
            Ok((
                self.on_welcome(message, body, now_unix_secs)?,
                Sender::Proven,
            ))
        } else if let Some(body) = frames::strip_prefix(content, prefixes::ENCRYPTED) {
            Ok((
                self.on_encrypted(message, body, now_unix_secs)?,
                Sender::Proven,
            ))
        } else if frames::strip_prefix(content, prefixes::SESSION_CONFIRM_PROBE).is_some() {
            Ok((self.on_probe(message, now_unix_secs)?, Sender::Proven))
        } else if frames::strip_prefix(content, prefixes::SESSION_CONFIRM_ACK).is_some() {
            // Never acted on, and not a frame this device accepts at all. A
            // leaf emits an acknowledgement and never a probe, so it has none
            // outstanding and every inbound one is unsolicited.
            //
            // Reading one as proof of a session is the bypass: producing a
            // frame that derives to its own address costs an attacker nothing,
            // so acting on it would let anyone holding a keypair tell firmware
            // a session exists that this device would refuse to seal into. The
            // phone gates the same frame on holding a session of its own; the
            // profile in the spec lists this prefix under what a leaf emits and
            // not under what it accepts.
            Ok((
                ignored("an acknowledgement arrived for a probe this device never sends"),
                Sender::Claimed,
            ))
        } else {
            Ok((
                ignored("frame carries no prefix this device answers"),
                Sender::Claimed,
            ))
        }
    }

    fn on_key_package(
        &mut self,
        message: &Message,
        body: &str,
        now_unix_secs: u64,
    ) -> Result<Handled> {
        // The one frame class that admits the older payload, because it is the
        // frame that *teaches* a peer which payload this device accepts: a
        // sender that has never held this device's key package signs the older
        // one, whatever release it runs, and refusing it makes a
        // phone-initiated pairing impossible rather than making the device
        // safer. See [`frames`] for the full reasoning and for what it costs.
        let signed_under = frames::verify_control_frame(message, now_unix_secs, true)?;
        let payload: KeyPackagePayload = serde_json::from_str(body)
            .map_err(|e| LeafError::MalformedFrame(format!("key package body: {e}")))?;

        let sender = message.sender.as_str();

        // The body names its own owner, and the frame names its sender. A
        // package that claims to belong to someone other than the peer that
        // signed the frame is one being relayed under a borrowed name, so the
        // two must agree before anything is stored under either.
        if payload.user_id != sender {
            return Err(LeafError::IdentityBinding(format!(
                "key package claims to be '{}' but the frame is signed by '{}'",
                payload.user_id, sender
            )));
        }

        // Bounded before the first byte is written under this peer's name.
        // Everything below here stores something, and a signature that only
        // proves a key derives to its own address is not a scarce thing to
        // produce.
        self.admit_peer(sender)?;

        let mut events = Vec::new();
        let mut record = self.peer_record(sender)?;

        // A reset carried under the older payload is refused its teardown and
        // nothing else. That payload leaves the timestamp outside the
        // signature, so the high-water mark below would be comparing a number
        // an attacker rewrote at will: parked in the future once, it would
        // deny this pair every future reset, which is a worse failure than the
        // replay it would be defending against. The rest of the frame is still
        // acted on, because capability advertisement is unauthenticated hint
        // data everywhere in this protocol, and the peer re-sends a reset
        // under the newer payload as soon as this device's key package has
        // taught it to.
        if payload.session_reset && signed_under != frames::ControlPayload::Fresh {
            events.push(LeafEvent::Ignored {
                reason: String::from(
                    "a session reset arrived on a frame that states no time, so its age \
                     cannot be judged and its teardown is refused",
                ),
            });
        } else if payload.session_reset {
            // The frame's own stamp, which `verify_control_frame` has already
            // proved is the sender's and inside the window this device
            // accepts. Everything below rests on both of those: an unproved
            // stamp would let anyone park the mark past every future reset.
            let stamp = message.timestamp.as_millis();
            if !record.reset_is_unspent(stamp) {
                // A reset already acted on, or older than one that was.
                // Tearing down again is how a captured frame becomes a
                // repeatable way to break a session that has since been
                // rebuilt, so the teardown is what a replay loses; the record
                // below is still refreshed, because a peer restating its
                // capabilities is harmless and a retransmission is the
                // ordinary reason to see this twice.
                events.push(LeafEvent::Ignored {
                    reason: String::from("a session reset arrived that this device has spent"),
                });
            } else {
                // Marked before the teardown, never after. The teardown is
                // followed by a fresh pairing, so a power cut in between would
                // leave this frame able to break the replacement too, and a
                // power cut is the one event this crate assumes will happen.
                record.remember_reset(stamp);
                record.key_package_sent = false;
                // Written here *and* again at the end of this function, which
                // is deliberate rather than redundant: the second write
                // carries the peer's refreshed capabilities and cannot be
                // moved earlier, and this one cannot be moved later without
                // putting a power cut between the mark and the teardown it
                // authorizes. Two writes on a reset frame, which arrives about
                // as often as a rekey; the ordinary frame still writes once.
                record
                    .save(&self.store, sender)
                    .map_err(|e| LeafError::Storage(e.to_string()))?;
                self.forget_session(sender)?;
                events.push(LeafEvent::SessionReset {
                    peer: sender.to_string(),
                });
            }
        }

        record.env_versions = payload.env_versions.clone();
        record.wire_versions = payload.wire_versions.clone();
        record
            .save(&self.store, sender)
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        events.push(LeafEvent::PeerAdvertised {
            peer: sender.to_string(),
        });

        // Answer with our own package only if this peer has not already had
        // one. Without that guard two peers that both answer trade key
        // packages forever, each one spending an init key.
        let outbound = if record.key_package_sent {
            Vec::new()
        } else {
            vec![self.key_package_frame_inner(sender, now_unix_secs, false)?]
        };

        Ok(Handled { outbound, events })
    }

    fn on_welcome(&mut self, message: &Message, body: &str, now_unix_secs: u64) -> Result<Handled> {
        // The older payload is refused here and needs no accommodation: a
        // Welcome can only follow this device's own key package reaching the
        // peer, and that is the frame which teaches the peer to sign the
        // freshness-bound one.
        frames::verify_control_frame(message, now_unix_secs, false)?;
        let welcome: WelcomeMessage = serde_json::from_str(body)
            .map_err(|e| LeafError::MalformedFrame(format!("welcome body: {e}")))?;

        let sender = message.sender.as_str();

        // The inviter named inside the body must be the peer that signed the
        // frame, and the group must be the one this pair would build. Without
        // the second check a peer could hand over a Welcome for a group it
        // built with somebody else, and the device would join it and seal into
        // a room whose membership it never checked.
        if welcome.inviter_id != sender {
            return Err(LeafError::IdentityBinding(format!(
                "welcome names inviter '{}' but the frame is signed by '{}'",
                welcome.inviter_id, sender
            )));
        }
        let expected = self.group_id(sender)?;
        if welcome.group_id != expected {
            return Err(LeafError::IdentityBinding(format!(
                "welcome is for group '{}', not this pair's '{}'",
                welcome.group_id, expected
            )));
        }

        let client = self.client()?;
        let welcome_message = MlsMessage::from_bytes(&welcome.welcome_data)
            .map_err(|e| LeafError::Mls(format!("welcome does not decode: {e:?}")))?;

        // And the Welcome must spend the key package this device minted **for
        // this peer**. The two checks above are about the peer and the group;
        // this one is about the material, and nothing else in the exchange
        // covers it, because a key package rides in a frame that is signed but
        // not encrypted and is therefore spendable by whoever copied it off the
        // air.
        //
        // Checked before the join, which is the whole point: joining spends the
        // init key, and the failure being prevented is not a stranger reading
        // anything (the group and credential gates hold either way) but this
        // device's package being **burned by somebody it was not meant for**,
        // which leaves the intended peer holding a Welcome that can no longer
        // be joined and a pairing that only a driven reset recovers.
        self.require_own_key_package(sender, &welcome_message)?;

        // `tree_data: None` because the peer puts the ratchet tree in the
        // Welcome. A device that needed it out of band would need a side
        // channel it does not have.
        let (mut group, _info) = client
            .join_group(None, &welcome_message, None)
            .map_err(|e| LeafError::Mls(format!("cannot join from the welcome: {e:?}")))?;

        // The group checked above was the one the body *claimed*. This is the
        // one that was actually joined, and they are separate values: the
        // claim is a JSON field beside the Welcome, and the Welcome carries
        // its own group id inside. A frame that puts an honest claim in front
        // of somebody else's Welcome passes the first check and fails here.
        //
        // Refused before the state is written, because joining has already
        // spent this device's init key, and persisting the group as well would
        // leave a room it never chose sitting on flash for every later gate to
        // keep refusing.
        if group.group_id() != expected.as_str().as_bytes() {
            return Err(LeafError::IdentityBinding(format!(
                "welcome claimed group '{expected}' but joined a different one"
            )));
        }

        group
            .write_to_storage()
            .map_err(|e| LeafError::Storage(format!("cannot persist group state: {e:?}")))?;

        // A session exists from here, so the peer has to be in the index
        // whatever route it took to get one: a session firmware cannot see in
        // [`LeafDevice::peers`] is one it can neither audit nor
        // [`LeafDevice::unpair`].
        //
        // Ordinarily it is already there, put there by the same mint that
        // recorded the reference this Welcome just spent, and an eviction
        // since then would have taken that reference with it and been refused
        // above. What is left is the one order that separates them: the mint
        // saves the peer record and indexes second, so an index write that
        // failed leaves a peer holding a live reference and no slot. That peer
        // arrives here.
        self.index_peer(sender)?;

        // The confirmation is a group-aware decrypt, sealed inside an ordinary
        // envelope. A peer that created a session of its own confirms only on
        // a successful decrypt, so a plaintext acknowledgement would leave it
        // unconfirmed however many times it was sent.
        let confirm = self.seal_content(
            sender,
            prefixes::SESSION_CONFIRM_ENCRYPTED.as_bytes(),
            now_unix_secs,
        )?;

        Ok(Handled {
            outbound: vec![confirm],
            events: vec![LeafEvent::SessionEstablished {
                peer: sender.to_string(),
            }],
        })
    }

    fn on_encrypted(&mut self, message: &Message, body: &str, _now: u64) -> Result<Handled> {
        // Not signature-gated: this is the data plane, and MLS authenticates
        // its own sender. A second signature on the outside would state what
        // the AEAD already proves on the inside.
        let envelope = parse_envelope(body)?;
        let sender = message.sender.as_str();

        // The envelope names a sender too, and it is inside nothing: it rides
        // in the clear beside the ciphertext. Binding it to the wire sender
        // before the group is loaded keeps a relayed frame from being
        // attributed to whoever forwarded it.
        if envelope.sender_id != sender {
            return Err(LeafError::IdentityBinding(format!(
                "envelope claims sender '{}' but the frame came from '{}'",
                envelope.sender_id, sender
            )));
        }

        let client = self.client()?;
        let group_id = self.group_id(sender)?;
        if envelope.group_id != group_id {
            return Err(LeafError::IdentityBinding(format!(
                "envelope is for group '{}', not this pair's '{}'",
                envelope.group_id, group_id
            )));
        }

        let mut group = self.load_group(&client, sender, &group_id)?;

        // Asked before the frame is opened, for the reason a seal asks it
        // before sealing: a group that stopped being this pair is one this
        // device holds no conversation with in either direction. The commit
        // that widens a roster is caught below instead, after it applies,
        // because that is the only moment it can be seen at all; this is what
        // keeps every frame after that one from arriving as ordinary traffic.
        self.require_still_a_pair(&group, sender)?;

        let inbound = MlsMessage::from_bytes(&envelope.ciphertext)
            .map_err(|e| LeafError::Mls(format!("sealed payload does not decode: {e:?}")))?;

        let received = group
            .process_incoming_message(inbound)
            .map_err(|e| LeafError::Mls(format!("cannot open: {e:?}")))?;

        let epoch = group.current_epoch();
        group
            .write_to_storage()
            .map_err(|e| LeafError::Storage(format!("cannot persist group state: {e:?}")))?;

        let events = match received {
            ReceivedMessage::ApplicationMessage(app) => {
                // The leaf identity binding, applied here as it is on the
                // phone: the group member that actually sealed this must be
                // the peer the frame claims to come from. MLS proves a member
                // sealed it; only re-deriving the address from that member's
                // signature key proves *which* member, and in a group this
                // device did not build, membership is not something it chose.
                self.bind_sender_credential(&group, app.sender_index, sender)?;

                let text = String::from_utf8_lossy(app.data()).to_string();
                // Consumed and never surfaced: it exists to be a decrypt, not
                // to be read.
                if text == prefixes::SESSION_CONFIRM_ENCRYPTED {
                    vec![LeafEvent::SessionEstablished {
                        peer: sender.to_string(),
                    }]
                } else {
                    vec![LeafEvent::MessageReceived {
                        peer: sender.to_string(),
                        text,
                    }]
                }
            }
            ReceivedMessage::Commit(commit) => {
                // Two questions a commit has to answer, ordered so the
                // first failure is the informative one: is this still the
                // pair this device joined, and did the peer whose frame
                // carried this actually author it. The roster answers the
                // first. Only the committer's own credential answers the
                // second, because the frame states who sent it and a peer is
                // free to relay a third member's work under its own name,
                // which without this reaches firmware wearing the peer's
                // address.
                //
                // Reported rather than rolled back. A member cannot skip one
                // commit and keep decrypting the next, so by the time there
                // is anything to read the commit is applied and durable. What
                // these two buy is that firmware hears a group stopped being
                // the pair it agreed to, rather than nothing at all.
                self.require_still_a_pair(&group, sender)?;
                self.bind_sender_credential(&group, commit.committer, sender)?;
                vec![LeafEvent::CommitApplied {
                    peer: sender.to_string(),
                    epoch,
                }]
            }
            _ => vec![LeafEvent::Ignored {
                reason: String::from("sealed frame carried nothing this device acts on"),
            }],
        };

        Ok(Handled {
            outbound: Vec::new(),
            events,
        })
    }

    /// Answers a liveness probe, but only for a peer this device can still
    /// talk to.
    ///
    /// The acknowledgement is not a pleasantry: a peer treats it as proof the
    /// session is usable and confirms on it, then flushes everything it had
    /// queued into that session. A device that answered after losing its store
    /// would confirm a session it cannot decrypt a single frame of, and the
    /// peer would have no way to find that out, because the device's silence
    /// afterwards is indistinguishable from a quiet link. Staying quiet here
    /// leaves the peer unconfirmed, which is a state it knows how to repair.
    ///
    /// This is the peer's own rule, applied on the device: it answers a probe
    /// only while it holds a session of its own.
    fn on_probe(&mut self, message: &Message, now_unix_secs: u64) -> Result<Handled> {
        // The older payload is refused here too, and for the same reason it is
        // refused on a Welcome: a probe only ever follows an established
        // session, which this device's own key package is what starts.
        frames::verify_control_frame(message, now_unix_secs, false)?;
        let sender = message.sender.as_str();

        // Loaded rather than counted. "State is on flash" and "this device can
        // decrypt" are different claims, and an acknowledgement asserts the
        // second: a device that answered on the strength of bytes it cannot
        // load would confirm a session it cannot open one frame of, which is
        // the failure staying quiet exists to avoid. A state that is present
        // and unloadable is also not silence, it is a store handing back what
        // this device did not write, and it propagates for the reason
        // [`LeafDevice::load_group`] separates the two.
        let client = self.client()?;
        let group_id = self.group_id(sender)?;
        let group = match self.load_group(&client, sender, &group_id) {
            Ok(group) => group,
            Err(LeafError::NoSession(_)) => {
                return Ok(ignored(
                    "a confirmation probe arrived for a peer this device has no session with",
                ))
            }
            Err(e) => return Err(e),
        };

        // And it must still be this pair. An acknowledgement is a promise to
        // use the session rather than a report that bytes exist, and a device
        // that will refuse to seal into this group has no such promise to
        // make: the peer would confirm, flush everything it had queued, and
        // hear nothing further. Refused rather than ignored, because unlike an
        // absent session this is a state firmware needs to hear about.
        self.require_still_a_pair(&group, sender)?;

        let mut ack = frames::build(
            &self.store,
            &self.identity,
            &self.app_id,
            sender,
            String::from(prefixes::SESSION_CONFIRM_ACK),
            now_unix_secs,
            MessagePriority::High,
        )?;
        frames::sign_control_frame(&self.identity, &mut ack)?;
        Ok(Handled {
            outbound: vec![ack],
            events: Vec::new(),
        })
    }

    /// Discards everything belonging to a session with `peer`.
    ///
    /// Called when a peer says it has discarded its own. A device that kept
    /// the old session would hold one the peer has already thrown away, and
    /// every later frame from it would decrypt to nothing.
    ///
    /// # The prior epochs go too
    ///
    /// A session is the group state, the marker, **and** the prior-epoch
    /// records, and the last of those is the part that is easy to leave
    /// behind. Each holds an epoch's secrets, so records that outlive the
    /// session they belong to are key material surviving an erasure the owner
    /// asked for. They also outlive it under a name the next session answers
    /// to: a pair's group id is derived from the two addresses, so a device
    /// that re-pairs with the same peer rebuilds the same id and starts
    /// writing epochs beside the last session's.
    fn forget_session(&mut self, peer: &str) -> Result<()> {
        let group_id = self.group_id(peer)?;
        let key = crate::adapters::hex(group_id.as_str().as_bytes());

        // The anchor is the middle of the sweep rather than its top. Trimming
        // keeps only the newest few below it and the slack covers a delete
        // that failed while it was doing so, but a record can also sit
        // **above** every anchor: the adapter writes epoch records before the
        // state entry that names them, so a cut between the two leaves an
        // epoch on flash that the marker, the high-water record and the
        // group's own epoch all agree is not there. Sweeping only downward
        // would delete every record but that one and report success, which is
        // the same erasure-that-did-not-happen the three anchors below exist
        // to prevent. A delete of a key that is not there is not an error,
        // which is what makes a fixed window the right shape rather than an
        // enumeration this seam cannot offer.
        //
        // Three sources, tried in order, because an anchor that is missing or
        // unreadable cannot bound anything and anchoring at zero would delete
        // one record, return `Ok`, and leave every other epoch's secrets on
        // flash: an erasure the owner asked for that reports success and did
        // not happen. The marker inside the state entry is first because it is
        // the one written atomically with the state. The separate high-water
        // record is second and covers the case that one cannot: a state entry
        // the part hands back as something else. The group's own epoch is
        // last, and names the same neighbourhood.
        let anchor = match crate::adapters::state_marker(&self.store, &key) {
            Some(highest) => Some(highest),
            None => self.max_epoch(&key)?,
        };
        let highest = anchor
            .or_else(|| self.current_epoch(&group_id))
            .unwrap_or(0);
        let floor = highest.saturating_sub(PRIOR_EPOCH_RETENTION + FORGET_EPOCH_SLACK);
        let ceiling = highest.saturating_add(FORGET_EPOCH_SLACK);
        for epoch in floor..=ceiling {
            self.store
                .delete(KEY_TYPE_GROUP_EPOCH, &format!("{key}:{epoch}"))
                .map_err(|e| LeafError::Storage(e.to_string()))?;
        }

        self.store
            .delete(KEY_TYPE_GROUP_STATE, &key)
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        self.store
            .delete(KEY_TYPE_GROUP_EPOCH, &format!("{key}:max"))
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        Ok(())
    }

    /// The highest epoch id ever written for a group, by storage key.
    ///
    /// `None` covers both a marker that was never written and one that does
    /// not decode. The two are the same answer here, which is safe only
    /// because the caller treats `None` as "no bound available" and finds
    /// another anchor, rather than as "no epochs".
    fn max_epoch(&self, group_key: &str) -> Result<Option<u64>> {
        let raw = self
            .store
            .load(KEY_TYPE_GROUP_EPOCH, &format!("{group_key}:max"))
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        Ok(raw
            .and_then(|bytes| <[u8; 8]>::try_from(bytes.as_slice()).ok())
            .map(u64::from_be_bytes))
    }

    /// Forgets a peer's session, its key package and what it advertised,
    /// leaving the index.
    fn forget_peer(&mut self, peer: &str) -> Result<()> {
        // Read before the record holding it is deleted, because that reference
        // is the only pointer this device has to the key package minted for
        // this peer, and the package is private key material. An init key is
        // single use, so one nobody spends is never consumed, and the only
        // other thing that reclaims it is four later mints pushing it out of
        // the ring. On a device that pairs twice a year that is years of
        // holding an init key for a peer the owner asked it to forget.
        //
        // Best effort, though. A record that will not decode has no reference
        // to give, and refusing the whole call for that would leave the owner
        // unable to forget a peer at all: the one escape hatch a device has,
        // closed by exactly the corruption it exists to recover from. An
        // unreclaimed package is the smaller residue, and it is the same
        // reading [`LeafDevice::peer_index`] gives an index that does not
        // parse.
        let minted = self
            .peer_record(peer)
            .ok()
            .and_then(|record| record.key_package_ref);

        self.forget_session(peer)?;

        // Erased before the record stops naming it, which is the ordering the
        // adapter's own eviction uses and for the same reason: a failed erase
        // leaves a pointer a retry can still follow, where the other order
        // leaves key material that nothing names and nothing reclaims.
        if let Some(reference) = minted {
            KeyPackageAdapter::new(Arc::clone(&self.store))
                .erase(&reference)
                .map_err(|e| LeafError::Storage(e.to_string()))?;
        }

        self.store
            .delete(KEY_TYPE_PEER, peer)
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        Ok(())
    }

    /// The peers this device holds records for.
    ///
    /// An index that does not decode is read as empty rather than as an error.
    /// It is a bound on storage, not a security claim, and a device that
    /// refused to pair because a list of previous peers is unreadable would
    /// have turned a recoverable annoyance into a brick.
    fn peer_index(&self) -> Result<Vec<String>> {
        let Some(raw) = self
            .store
            .load(KEY_TYPE_PEER_INDEX, "peers")
            .map_err(|e| LeafError::Storage(e.to_string()))?
        else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_slice(&raw).unwrap_or_default())
    }

    fn save_peer_index(&self, index: &[String]) -> Result<()> {
        let encoded = serde_json::to_vec(index)
            .map_err(|e| LeafError::MalformedFrame(format!("cannot encode peer index: {e}")))?;
        self.store
            .store(KEY_TYPE_PEER_INDEX, "peers", &encoded)
            .map_err(|e| LeafError::Storage(e.to_string()))
    }

    /// Records `peer` in the index without holding it to [`MAX_PEERS`].
    ///
    /// The bound exists to stop a stranger spending a device's flash, and
    /// neither caller here is a stranger: one is firmware choosing to pair,
    /// the other is a Welcome, and a Welcome is only reached by a peer whose
    /// recorded key package reference it spends. That reference is written
    /// only by a mint, and a mint is reached only through firmware or through
    /// [`LeafDevice::admit_peer`], which is where the bound is applied. So
    /// every peer arriving here has already been counted or chosen.
    ///
    /// That is a claim about [`LeafDevice::require_own_key_package`] and would
    /// be false without it: a key package travels unencrypted, so before that
    /// gate any listener could copy one, build this pair's group around it and
    /// arrive here having been counted by nothing.
    ///
    /// It is the index rather than the bound that has to be complete. A peer
    /// missing from it holds a session nothing can audit and
    /// [`LeafDevice::unpair`] cannot be pointed at, and a session is exactly
    /// what the authorization obligation asks firmware to review, since none
    /// of the gates in this crate answers whether the owner meant this peer.
    fn index_peer(&mut self, peer: &str) -> Result<()> {
        let mut index = self.peer_index()?;
        if index.iter().any(|held| held == peer) {
            return Ok(());
        }
        index.push(peer.to_string());
        self.save_peer_index(&index)
    }

    /// Makes room for `peer` in the bounded set, or refuses.
    ///
    /// A peer already held is admitted for free. A new one at capacity takes
    /// the place of a pairing that never completed, which is what a record
    /// with no session is; if every slot holds a real session, the answer is
    /// [`LeafError::TooManyPeers`] rather than the eviction of somebody the
    /// owner actually paired with. That direction is the whole point of having
    /// the rule: a stranger who can provoke a record must not be able to
    /// provoke the removal of one.
    fn admit_peer(&mut self, peer: &str) -> Result<()> {
        let mut index = self.peer_index()?;
        if index.iter().any(|held| held == peer) {
            return Ok(());
        }

        if index.len() >= MAX_PEERS {
            // A store that fails the session check counts as holding one, so a
            // read error protects the peer rather than evicting it.
            let victim = index
                .iter()
                .find(|held| !self.has_session(held).unwrap_or(true))
                .cloned();
            let Some(victim) = victim else {
                return Err(LeafError::TooManyPeers);
            };
            self.forget_peer(&victim)?;
            index.retain(|held| held != &victim);
        }

        index.push(peer.to_string());
        self.save_peer_index(&index)
    }

    /// Requires the group to still be the pair this device agreed to.
    ///
    /// The Welcome gate is what keeps this device out of a room it never
    /// chose, and it runs once. Nothing repeats it afterwards, and a commit is
    /// free to add a member without the group id changing, so a device that
    /// checked only at the join would follow its peer into a room one member
    /// at a time and never see it happen. The never-committing profile makes
    /// that entirely the peer's decision, which is exactly why it is worth
    /// checking rather than trusting.
    ///
    /// So the roster is re-read on **every use of the group**, and not only on
    /// the commit that can change it: two members, and both of them
    /// **derived** rather than read off a credential, since a basic credential
    /// is a bare assertion.
    ///
    /// Checking only at the commit would be checking at the one moment whose
    /// answer cannot be kept. A commit is applied and durable by the time
    /// there is a roster to read, so the refusal is a returned error and
    /// nothing else: it survives no reboot, and a device that came back up
    /// would seal its next message into the widened room and call it an
    /// ordinary session. Reading the roster where the group is used puts the
    /// answer somewhere it cannot be lost, which is the same reason nothing
    /// else in this crate caches MLS state.
    ///
    /// It costs a roster read and two SHA-256 derivations per frame. Both are
    /// symmetric work, so the profile's promise that per-message cost is
    /// symmetric only still holds.
    ///
    /// Reported rather than rolled back. A member cannot skip one commit and
    /// keep decrypting the next, so the choice is not whether to follow the
    /// peer but whether firmware is told, and what it does about it is
    /// [`LeafDevice::unpair`] or the peer's own reset.
    fn require_still_a_pair(&self, group: &Group<impl MlsConfig>, peer: &str) -> Result<()> {
        let roster = group.roster();
        let members = roster.members();
        if members.len() != 2 {
            return Err(LeafError::IdentityBinding(format!(
                "a commit left this group holding {} members, and a leaf node's group is a pair",
                members.len()
            )));
        }

        let mine = self.identity.address.to_string();
        for member in &members {
            let credential = member
                .signing_identity
                .credential
                .as_basic()
                .ok_or_else(|| {
                    LeafError::IdentityBinding(String::from(
                        "a group member presents no basic credential, so it names no address",
                    ))
                })?;
            let address = crate::adapters::credential_address(credential)?;
            if address != mine && address != peer {
                return Err(LeafError::IdentityBinding(format!(
                    "a commit left '{address}' in this group, which is neither this device nor '{peer}'"
                )));
            }
            frames::verify_sender_derivation(
                address,
                member.signing_identity.signature_key.as_bytes(),
            )?;
        }
        Ok(())
    }

    /// Requires a Welcome to spend the key package minted for its sender.
    ///
    /// A key package is a bearer token: this device hands one to a peer in a
    /// frame addressed to that peer, and a shared radio carries it to everyone
    /// else as well. A listener that copies it can build a group whose id is
    /// the one this pair would build, sign the Welcome with its own key, and
    /// name itself as inviter, so the inviter check and the group check both
    /// pass honestly. What it cannot do is present the reference of a package
    /// this device minted for **it**.
    ///
    /// A peer with no recorded reference is refused for the same reason an
    /// unparseable identifier is elsewhere in this crate: there is nothing to
    /// compare, and "nothing to compare" is the bypass rather than a lenience.
    /// It also restores the bound: every peer reaching a session has been
    /// through [`LeafDevice::admit_peer`] or was chosen by firmware, which is
    /// what lets [`LeafDevice::index_peer`] skip [`MAX_PEERS`].
    fn require_own_key_package(&self, peer: &str, welcome: &MlsMessage) -> Result<()> {
        let Some(minted) = self.peer_record(peer)?.key_package_ref else {
            return Err(LeafError::UnsolicitedWelcome(format!(
                "no key package was ever minted for '{peer}', so nothing it sends can spend one"
            )));
        };

        let spends_ours = welcome
            .welcome_key_package_references()
            .into_iter()
            .any(|reference| crate::adapters::hex(reference) == minted);

        if !spends_ours {
            return Err(LeafError::UnsolicitedWelcome(format!(
                "welcome from '{peer}' spends a key package this device did not mint for it"
            )));
        }

        // The reference matches, so this is the peer's own package. Whether it
        // is still on flash is a separate question, and it is asked here
        // because the answer decides which failure firmware is handed. An init
        // key is single use, so a package an earlier join consumed is gone,
        // and the ring bounding unspent packages evicts on the fourth later
        // mint. Left to the join, either arrives from inside MLS as a Welcome
        // that will not decode, which reads as a broken peer and sends a bench
        // to the wire when the repair is a fresh package.
        if self
            .store
            .load(KEY_TYPE_KEY_PACKAGE, &minted)
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .is_none()
        {
            return Err(LeafError::StaleKeyPackage(format!(
                "the key package '{peer}' spends was minted for it and is no longer held"
            )));
        }
        Ok(())
    }

    /// Requires the MLS member at `index` to derive to `claimed`.
    ///
    /// The refusal is deliberately the same for a member whose credential is
    /// not an address at all: a credential with no derivation to check is not
    /// one to wave through, it is the bypass. That is the rule
    /// [ADR 0010](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0010-unconditional-leaf-identity-binding.md)
    /// makes unconditional on the phone, and a device that skipped it would be
    /// the weaker end of a protocol whose whole claim is that both ends are
    /// the same.
    fn bind_sender_credential(
        &self,
        group: &Group<impl MlsConfig>,
        index: u32,
        claimed: &str,
    ) -> Result<()> {
        let member = group.member_at_index(index).ok_or_else(|| {
            LeafError::IdentityBinding(format!("no group member at index {index}"))
        })?;

        let credential = member
            .signing_identity
            .credential
            .as_basic()
            .ok_or_else(|| {
                LeafError::IdentityBinding(String::from(
                    "group member presents no basic credential, so it names no address",
                ))
            })?;

        let credential_address = crate::adapters::credential_address(credential)?;
        if credential_address != claimed {
            return Err(LeafError::IdentityBinding(format!(
                "authored by group member '{credential_address}', but the frame claims '{claimed}'"
            )));
        }

        // And the credential's claim is itself checked rather than trusted: a
        // basic credential is self-asserted, so the address in it means
        // something only because it is the hash of the key beside it.
        frames::verify_sender_derivation(
            credential_address,
            member.signing_identity.signature_key.as_bytes(),
        )
    }

    /// Whether this frame names this device as its recipient.
    ///
    /// The comparison is between parsed addresses rather than between strings,
    /// which costs nothing and is the same test every other identity claim in
    /// this protocol gets. A recipient that is not an address at all is not
    /// this device: this device is named by one, and by exactly one spelling
    /// of it, since [`Address`] refuses anything but the canonical rendering.
    fn is_addressed_to_me(&self, message: &Message) -> bool {
        message
            .recipient
            .as_str()
            .parse::<Address>()
            .is_ok_and(|recipient| recipient == self.identity.address)
    }

    fn client(&self) -> Result<Client<impl MlsConfig>> {
        build_client(&self.identity, &self.store)
    }

    /// Loads the group for a session with `peer`, and says which failure it is.
    ///
    /// A group that will not load is two different things wearing one error.
    /// State that is **absent** is a device that never paired with this peer,
    /// or one that unpaired, and re-pairing is the repair. State that is
    /// **present and unloadable** is a store handing back bytes this device did
    /// not write, which no amount of re-pairing fixes and which a bench needs
    /// to be told about rather than sent chasing a pairing problem.
    fn load_group<C: MlsConfig>(
        &self,
        client: &Client<C>,
        peer: &str,
        group_id: &GroupId,
    ) -> Result<Group<C>> {
        match client.load_group(group_id.as_str().as_bytes()) {
            Ok(group) => Ok(group),
            Err(e) if self.state_present(group_id)? => Err(LeafError::Storage(format!(
                "group state for {peer} is on flash but does not load: {e:?}"
            ))),
            Err(_) => Err(LeafError::NoSession(peer.to_string())),
        }
    }

    /// The epoch a group is in, read from its stored state.
    ///
    /// Best effort by construction: the one caller is the sweep in
    /// [`LeafDevice::forget_session`], which needs an anchor when the marker
    /// cannot give it one, and a state that will not load leaves nothing to
    /// read. A device with neither has nothing above the window to have
    /// written.
    fn current_epoch(&self, group_id: &GroupId) -> Option<u64> {
        let client = self.client().ok()?;
        client
            .load_group(group_id.as_str().as_bytes())
            .ok()
            .map(|group| group.current_epoch())
    }

    fn group_id(&self, peer: &str) -> Result<GroupId> {
        Ok(GroupId::for_session(
            &self.identity.address.to_string(),
            peer,
        )?)
    }

    /// Repeats an acknowledgement for a frame this device already answered.
    ///
    /// The peer only retransmits because it heard nothing, and the frame it
    /// sends again is the one it sent before: the same bytes and the same id,
    /// frozen in its outbox. Opening it a second time cannot work, because the
    /// ratchet spent that generation on the first copy, so before this the
    /// device refused it and stayed quiet and the peer went right on asking.
    ///
    /// Answering from memory is honest. The claim an acknowledgement makes is
    /// that the frame with this id reached its recipient, and it did.
    fn repeat_acknowledgement(
        &mut self,
        message: &Message,
        now_unix_secs: u64,
    ) -> Result<Option<Handled>> {
        if !message.requires_ack {
            return Ok(None);
        }

        let Some(record) = self.known_peer(message.sender.as_str())? else {
            return Ok(None);
        };
        if !record.was_acknowledged(message.id.to_string().as_str()) {
            return Ok(None);
        }

        Ok(Some(Handled {
            outbound: alloc::vec![frames::acknowledge(
                &self.store,
                &self.identity,
                &self.app_id,
                message,
                now_unix_secs,
            )?],
            events: alloc::vec![LeafEvent::Ignored {
                reason: String::from("a frame this device already answered arrived again"),
            }],
        }))
    }

    /// Mints the acknowledgement owed for a frame that was accepted, and
    /// remembers it before the caller can send it.
    ///
    /// # Why only a peer this device already knows
    ///
    /// A record exists for a peer that got through the key package gate, which
    /// means a verified signature and an address that derives to the key that
    /// made it. Answering anyone else would hand a stranger within radio range
    /// two things they do not have today: a way to make the device transmit on
    /// demand, and an answer to "is there a node at this address", which on a
    /// lock is the question worth asking first. It would also let a flood of
    /// forged senders each write a record on a part with a few hundred
    /// kilobytes of flash.
    ///
    /// Nothing is lost by the restriction. The peer that sends frames worth
    /// acknowledging is the one this device is paired with, and an unknown
    /// sender gets exactly the silence it got before.
    ///
    /// # Why the record is not the whole gate
    ///
    /// It answers "has this address ever paired", and the frame in hand names
    /// its sender in plaintext, so on its own the record clears anyone who
    /// overheard the pair once. The caller asks [`Sender`] first, and only a
    /// frame that verified a signature or opened under the pair's key reaches
    /// here. The two are one gate: this one says the peer is known, that one
    /// says the frame is from it.
    fn acknowledge(&mut self, message: &Message, now_unix_secs: u64) -> Result<Option<Message>> {
        if !message.requires_ack {
            return Ok(None);
        }

        let peer = message.sender.as_str();
        let Some(mut record) = self.known_peer(peer)? else {
            return Ok(None);
        };

        let ack = frames::acknowledge(
            &self.store,
            &self.identity,
            &self.app_id,
            message,
            now_unix_secs,
        )?;

        // Durable before it is emitted, which is this profile's third
        // obligation. A device that answered and then lost the record would
        // meet the retransmission with silence, which is the state this
        // whole mechanism exists to leave.
        record.remember_acknowledged(message.id.to_string().as_str());
        record
            .save(&self.store, peer)
            .map_err(|e| LeafError::Storage(e.to_string()))?;

        Ok(Some(ack))
    }

    /// The record for a peer this device has paired with, or `None`.
    ///
    /// Distinct from [`Self::peer_record`], which returns a default for an
    /// unknown peer. Here the difference between "known" and "not" is the
    /// decision being made, so it must not be flattened away.
    fn known_peer(&self, peer: &str) -> Result<Option<PeerRecord>> {
        PeerRecord::load(&self.store, peer).map_err(|e| LeafError::Storage(e.to_string()))
    }

    fn peer_record(&self, peer: &str) -> Result<PeerRecord> {
        Ok(PeerRecord::load(&self.store, peer)
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .unwrap_or_default())
    }

    /// Whether a session with `peer` exists on flash.
    pub fn has_session(&self, peer: &str) -> Result<bool> {
        let group_id = self.group_id(peer)?;
        self.state_present(&group_id)
    }

    /// Whether group state for this id is on flash, whatever its condition.
    fn state_present(&self, group_id: &GroupId) -> Result<bool> {
        Ok(self
            .store
            .load(
                KEY_TYPE_GROUP_STATE,
                &crate::adapters::hex(group_id.as_str().as_bytes()),
            )
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .is_some())
    }

    /// The peers this device holds records for.
    ///
    /// Firmware's way to audit what a device accumulated. A session proves who
    /// a peer is and never that the owner meant to have them, so a device that
    /// has been in a hallway for a year may hold peers nobody chose; this is
    /// how they are found, and [`LeafDevice::unpair`] is how they are removed.
    /// Not every entry holds a session: a pairing that stopped after the first
    /// frame leaves a record too, and those are the slots a new peer recycles.
    pub fn peers(&self) -> Result<Vec<String>> {
        self.peer_index()
    }

    /// What this device recorded about a peer's capabilities.
    pub fn peer_env_versions(&self, peer: &str) -> Result<Vec<u8>> {
        Ok(self.peer_record(peer)?.env_versions)
    }

    /// Forgets a peer: its session, its prior epochs, the key package minted
    /// for it, and what it advertised.
    ///
    /// Exposed because a device that is factory reset or unpaired must be able
    /// to forget, and because leaving MLS state behind for a peer the owner
    /// removed is the kind of residue that outlives the reason it existed.
    ///
    /// The key package is part of that and is easy to miss, because the only
    /// pointer to it is the reference inside the record this call deletes. It
    /// holds an init key, which is single use and therefore never consumed if
    /// the pairing it was minted for never completed.
    ///
    /// Also releases the slot it held, so unpairing is how an owner makes room
    /// on a device whose peer table is full.
    pub fn unpair(&mut self, peer: &str) -> Result<()> {
        self.forget_peer(peer)?;

        let mut index = self.peer_index()?;
        if index.iter().any(|held| held == peer) {
            index.retain(|held| held != peer);
            self.save_peer_index(&index)?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for LeafDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LeafDevice")
            .field("address", &self.identity.address.to_string())
            .finish_non_exhaustive()
    }
}

/// A frame that produced nothing, and why.
///
/// Surfaced rather than swallowed so a bench can tell "refused" from "silently
/// dropped", which are the two states a pairing failure looks like from
/// outside a device with no console.
fn ignored(reason: &str) -> Handled {
    Handled {
        outbound: Vec::new(),
        events: vec![LeafEvent::Ignored {
            reason: String::from(reason),
        }],
    }
}

fn parse_app_id(app_id: &str) -> Result<AppId> {
    AppId::new(app_id).map_err(|e| LeafError::MalformedFrame(format!("app id: {e}")))
}

/// Parses an inbound envelope in every form a conforming sender may emit.
///
/// Three forms, and none of them is gated on what this device advertised.
/// Parsing is unconditional in this protocol: a device that decoded only the
/// form it asked for would drop frames from a peer that legitimately believed
/// it capable, which is exactly what happens after a partial fleet upgrade.
///
/// The sniff works because `{` opens JSON and, read as the little-endian
/// length prefix the compact codec starts with, is a number far above the
/// codec's own string cap.
fn parse_envelope(body: &str) -> Result<EncryptedMessage> {
    if body.starts_with('{') {
        return serde_json::from_str(body)
            .map_err(|e| LeafError::MalformedFrame(format!("json envelope: {e}")));
    }
    let bytes = BASE64
        .decode(body)
        .map_err(|e| LeafError::MalformedFrame(format!("envelope is not base64: {e}")))?;

    if let Ok(envelope) = EncryptedMessage::from_bytes(&bytes) {
        return Ok(envelope);
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| LeafError::MalformedFrame(format!("base64 envelope: {e}")))
}
