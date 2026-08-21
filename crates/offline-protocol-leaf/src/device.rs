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
use mls_rs::{Client, MlsMessage};
use offline_protocol_core::{Address, AppId, Message, MessagePriority};
use offline_protocol_sealed::{
    prefixes, EncryptedMessage, GroupId, KeyPackagePayload, MlsMessageType, WelcomeMessage,
    MLS_ENVELOPE_COMPACT_V1,
};

use crate::adapters::PeerRecord;
use crate::error::{LeafError, Result};
use crate::frames;
use crate::identity::{build_client, Identity};
use crate::keypkg;
use crate::store::{LeafStore, KEY_TYPE_GROUP_EPOCH, KEY_TYPE_GROUP_STATE, KEY_TYPE_PEER};

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
    MessageReceived {
        /// The peer's address.
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
    pub fn key_package_frame(&self, peer: &str, now_unix_secs: u64) -> Result<Message> {
        self.key_package_frame_inner(peer, now_unix_secs, false)
    }

    fn key_package_frame_inner(
        &self,
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
    pub fn seal(&self, peer: &str, plaintext: &str, now_unix_secs: u64) -> Result<Message> {
        self.seal_content(peer, plaintext.as_bytes(), now_unix_secs)
    }

    fn seal_content(&self, peer: &str, plaintext: &[u8], now_unix_secs: u64) -> Result<Message> {
        let client = self.client()?;
        let group_id = self.group_id(peer)?;
        let mut group = client
            .load_group(group_id.as_str().as_bytes())
            .map_err(|_| LeafError::NoSession(peer.to_string()))?;

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
    pub fn handle(&self, message: &Message, now_unix_secs: u64) -> Result<Handled> {
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
            frames::verify_control_frame(message)?;
            Ok(Handled {
                outbound: Vec::new(),
                events: vec![LeafEvent::SessionEstablished {
                    peer: message.sender.as_str().to_string(),
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

    fn on_key_package(&self, message: &Message, body: &str, now_unix_secs: u64) -> Result<Handled> {
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

        let mut events = Vec::new();
        let mut record = self.peer_record(sender)?;

        if payload.session_reset {
            self.forget_session(sender)?;
            record.key_package_sent = false;
            events.push(LeafEvent::SessionReset {
                peer: sender.to_string(),
            });
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

    fn on_welcome(&self, message: &Message, body: &str, now_unix_secs: u64) -> Result<Handled> {
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

    fn on_encrypted(&self, message: &Message, body: &str, _now: u64) -> Result<Handled> {
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

        let mut group = client
            .load_group(group_id.as_str().as_bytes())
            .map_err(|_| LeafError::NoSession(sender.to_string()))?;

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

    fn on_probe(&self, message: &Message, now_unix_secs: u64) -> Result<Handled> {
        frames::verify_control_frame(message)?;
        let sender = message.sender.as_str();
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
    fn forget_session(&self, peer: &str) -> Result<()> {
        let group_id = self.group_id(peer)?;
        let key = crate::adapters::hex(group_id.as_str().as_bytes());
        self.store
            .delete(KEY_TYPE_GROUP_STATE, &key)
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        self.store
            .delete(KEY_TYPE_GROUP_EPOCH, &format!("{key}:max"))
            .map_err(|e| LeafError::Storage(e.to_string()))?;
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
        group: &mls_rs::Group<impl MlsConfig>,
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
        Ok(self
            .store
            .load(
                KEY_TYPE_GROUP_STATE,
                &crate::adapters::hex(group_id.as_str().as_bytes()),
            )
            .map_err(|e| LeafError::Storage(e.to_string()))?
            .is_some())
    }

    /// What this device recorded about a peer's capabilities.
    pub fn peer_env_versions(&self, peer: &str) -> Result<Vec<u8>> {
        Ok(self.peer_record(peer)?.env_versions)
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

/// Erases every trace of a peer.
///
/// Exposed because a device that is factory reset or unpaired must be able to
/// forget, and because leaving MLS state behind for a peer the owner removed
/// is the kind of residue that outlives the reason it existed.
impl LeafDevice {
    /// Forgets a peer: its session, its prior epochs, and what it advertised.
    pub fn unpair(&self, peer: &str) -> Result<()> {
        self.forget_session(peer)?;
        self.store
            .delete(KEY_TYPE_PEER, peer)
            .map_err(|e| LeafError::Storage(e.to_string()))?;
        Ok(())
    }
}
