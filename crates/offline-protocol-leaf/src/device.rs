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

use crate::adapters::{PeerRecord, PRIOR_EPOCH_RETENTION};
use crate::error::{LeafError, Result};
use crate::frames;
use crate::identity::{build_client, Identity};
use crate::keypkg;
use crate::store::{
    LeafStore, KEY_TYPE_GROUP_EPOCH, KEY_TYPE_GROUP_STATE, KEY_TYPE_PEER, KEY_TYPE_PEER_INDEX,
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

/// How far below the retained window [`LeafDevice::unpair`] sweeps.
///
/// [`PRIOR_EPOCH_RETENTION`] says what a healthy device holds. This is the
/// margin for one that is not: a delete that failed during trimming leaves a
/// record holding an epoch's secrets, and forgetting a peer is the moment to
/// clear those rather than the moment to assume they are not there.
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
/// A replayed control frame. The signed payload states who, to whom, and what,
/// with nothing that says *when*, so a captured frame verifies forever. The
/// destructive case is a reset-flagged key package, which tears down a live
/// session, and the peer record remembers the last few of those so the same
/// frame cannot spend twice. An attacker holding an older one can still spend
/// it once. Closing that needs freshness inside the signed payload, which is a
/// change to the wire and to both ends rather than to this crate, and is
/// tracked as [issue 403](https://github.com/Offline-Protocol/offline-protocol-sdk/issues/403).
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
        let data = keypkg::mint(&client, now_unix_secs)?;
        let payload = keypkg::payload(&self.identity.address.to_string(), data, session_reset);
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
        let mut record = self.peer_record(peer)?;
        record.key_package_sent = true;
        record
            .save(&self.store, peer)
            .map_err(|e| LeafError::Storage(e.to_string()))?;

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
    pub fn handle(&mut self, message: &Message, now_unix_secs: u64) -> Result<Handled> {
        let content = &message.content;

        // Order matters: the encrypted-confirm prefix is not checked here at
        // all, because it never travels as a frame. It is only ever found
        // inside a decrypted plaintext, which is where this looks for it.
        if let Some(body) = frames::strip_prefix(content, prefixes::KEY_PACKAGE) {
            self.on_key_package(message, body, now_unix_secs)
        } else if let Some(body) = frames::strip_prefix(content, prefixes::WELCOME) {
            self.on_welcome(message, body, now_unix_secs)
        } else if let Some(body) = frames::strip_prefix(content, prefixes::ENCRYPTED) {
            self.on_encrypted(message, body, now_unix_secs)
        } else if frames::strip_prefix(content, prefixes::SESSION_CONFIRM_PROBE).is_some() {
            self.on_probe(message, now_unix_secs)
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
            Ok(Handled {
                outbound: Vec::new(),
                events: vec![LeafEvent::Ignored {
                    reason: String::from(
                        "an acknowledgement arrived for a probe this device never sends",
                    ),
                }],
            })
        } else {
            Ok(Handled {
                outbound: Vec::new(),
                events: vec![LeafEvent::Ignored {
                    reason: String::from("frame carries no prefix this device answers"),
                }],
            })
        }
    }

    fn on_key_package(
        &mut self,
        message: &Message,
        body: &str,
        now_unix_secs: u64,
    ) -> Result<Handled> {
        frames::verify_control_frame(message)?;
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

        if payload.session_reset {
            let frame_id = message.id.as_str();
            if record.has_seen_reset(&frame_id) {
                // A reset already acted on. Tearing down again on the same
                // frame is how a captured one becomes a repeatable way to
                // break a session that has since been rebuilt, so the
                // teardown is what a repeat loses; the record below is still
                // refreshed, because a peer restating its capabilities is
                // harmless and a retransmission is the ordinary reason to see
                // this twice.
                events.push(LeafEvent::Ignored {
                    reason: String::from("a session reset arrived twice on the same frame"),
                });
            } else {
                self.forget_session(sender)?;
                record.remember_reset(&frame_id);
                record.key_package_sent = false;
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
        frames::verify_control_frame(message)?;
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
            ReceivedMessage::Commit(_) => vec![LeafEvent::CommitApplied {
                peer: sender.to_string(),
                epoch,
            }],
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
        frames::verify_control_frame(message)?;
        let sender = message.sender.as_str();

        if !self.has_session(sender)? {
            return Ok(Handled {
                outbound: Vec::new(),
                events: vec![LeafEvent::Ignored {
                    reason: String::from(
                        "a confirmation probe arrived for a peer this device has no session with",
                    ),
                }],
            });
        }

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

        // The marker is the highest epoch ever written, so it is the top of
        // the sweep. Trimming keeps only the newest few below it; the slack
        // covers a delete that failed while it was doing so. A delete of a key
        // that is not there is not an error, which is what makes a fixed
        // window the right shape rather than an enumeration this seam cannot
        // offer.
        //
        // A marker that is missing or unreadable cannot bound anything, and
        // anchoring at zero would delete one record, return `Ok`, and leave
        // every other epoch's secrets on flash: an erasure the owner asked for
        // that reports success and did not happen. The group state names the
        // same neighbourhood of epochs and is about to be deleted anyway, so it
        // is the fallback anchor.
        let highest = match self.max_epoch(&key)? {
            Some(highest) => highest,
            None => self.current_epoch(&group_id).unwrap_or(0),
        };
        let floor = highest.saturating_sub(PRIOR_EPOCH_RETENTION + FORGET_EPOCH_SLACK);
        for epoch in floor..=highest {
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

    /// Forgets a peer's session and what it advertised, leaving the index.
    fn forget_peer(&mut self, peer: &str) -> Result<()> {
        self.forget_session(peer)?;
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
                "sealed by group member '{credential_address}', but the frame claims '{claimed}'"
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

    /// Forgets a peer: its session, its prior epochs, and what it advertised.
    ///
    /// Exposed because a device that is factory reset or unpaired must be able
    /// to forget, and because leaving MLS state behind for a peer the owner
    /// removed is the kind of residue that outlives the reason it existed.
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
