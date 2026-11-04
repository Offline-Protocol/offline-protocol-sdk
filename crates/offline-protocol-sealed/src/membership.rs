//! Application membership: the byte layouts by which a node proves, to
//! another node of the same application only, that it belongs to that
//! application ([ADR 0037]).
//!
//! An application that configures a publisher key makes its users visible
//! only to each other while every node still carries every application's
//! traffic. Visibility rests on three things every implementation must build
//! identically, because each is either a MAC, a key derivation or a signature,
//! and a byte of difference fails as "not a member" rather than as an error
//! anyone can read:
//!
//! - the **certificate** the publisher signs over a member's address;
//! - the **exchange**, two `__MEMBER__` frames each way between new
//!   neighbours, which reveals membership only to a node holding the same
//!   application key;
//! - the **end-to-end proof** an initiator carries inside the frame that opens
//!   a session with a member it never met as a neighbour.
//!
//! The chapter is [the membership section of the control-message spec][spec].
//!
//! # Why the codec is here and the cryptography is not
//!
//! This module joins and splits bytes and builds the inputs every primitive
//! runs over: the HKDF `info` and `salt`, the HMAC input, the signing
//! payloads, the AEAD plaintext. It runs no HMAC, HKDF, AEAD or signature,
//! because the sealed layer links no library for them (ADR 0022: SHA-256 is
//! the only cryptography here). The engine runs them with its own crates and
//! a leaf with mls-rs's. What this module being the one home buys is that
//! both build the same bytes, which is the half a second implementation gets
//! wrong.
//!
//! The primitives are fixed by the chapter: HKDF-SHA256, HMAC-SHA256
//! truncated to [`MEMBER_TAG_LEN`] bytes, Ed25519, and ChaCha20-Poly1305 with
//! a 12-byte zero nonce and no associated data, under a key used once.
//!
//! # Why nothing here is length-prefixed
//!
//! Every field of every layout below has a fixed size: an address is its
//! 21-byte `version || hash` payload, never its 44-character rendering. A
//! concatenation of fixed-size fields has one parse, so no field can be
//! shifted into its neighbour to make one payload read as another. The one
//! variable field, the application id at the end of [`app_key_info`], is
//! last, which is unambiguous for the same reason.
//!
//! [ADR 0037]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0037-an-applications-users-are-visible-only-to-that-application.md
//! [spec]: https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/spec/control-messages.md#application-membership

use crate::derive::ED25519_PUBLIC_KEY_LEN;
use crate::identity_assertion::ED25519_SIGNATURE_LEN;
use crate::{Result, SealedError};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use offline_protocol_core::Address;
use sha2::{Digest, Sha256};

/// HKDF `info` prefix for the application key; the application id follows.
pub const APP_KEY_INFO: &[u8] = b"offline-protocol-app-v1";

/// Length of the application key, `K_app`, in bytes.
pub const APP_KEY_LEN: usize = 32;

/// Domain the publisher signs a certificate under.
pub const MEMBER_CERT_DOMAIN: &[u8] = b"offline-app-member-v1";

/// HMAC label of the exchange's first frame.
pub const MEMBER_HELLO_LABEL: &[u8] = b"offline-app-member-hello-v1";

/// HKDF `info` prefix for the exchange's sealing key; the sender's address
/// follows.
pub const MEMBER_EXCHANGE_INFO: &[u8] = b"offline-app-member-exchange-v1";

/// Domain of the transcript signature in the exchange's second frame.
pub const MEMBER_TRANSCRIPT_DOMAIN: &[u8] = b"offline-app-member-transcript-v1";

/// Domain of the identity signature inside an end-to-end proof.
pub const MEMBER_E2E_DOMAIN: &[u8] = b"offline-app-member-e2e-v1";

/// HKDF `info` for the end-to-end proof's sealing key.
///
/// Apart from [`MEMBER_E2E_DOMAIN`] so that no byte string is both a key
/// derivation label and a signing domain: one label in two primitives is the
/// shape that lets an output of one stand in for an input of the other.
pub const MEMBER_E2E_KEY_INFO: &[u8] = b"offline-app-member-e2e-key-v1";

/// HMAC label of the Bluetooth LE App tag a configured application serves.
///
/// `tag = HMAC-SHA256(K_app, ble_app_tag_input(device_id))[..8]`. The other
/// identifiers below are HKDF outputs; this one is a MAC because a central
/// recomputes it for each Device id it reads.
pub const BLE_APP_TAG_LABEL: &[u8] = b"offline-app-ble-tag-v1";

/// Length of the Bluetooth LE App tag, in bytes, keyed or not.
pub const BLE_APP_TAG_LEN: usize = 8;

/// HKDF `info` of the value a configured application advertises under the
/// `app` key of its Android Wi-Fi Direct service record.
pub const WIFI_DIRECT_APP_INFO: &[u8] = b"offline-app-wifi-direct-record-v1";

/// HKDF `info` of a configured application's Android Wi-Fi Direct group name.
pub const WIFI_DIRECT_NETWORK_INFO: &[u8] = b"offline-app-wifi-direct-network-v1";

/// HKDF `info` of a configured application's Android Wi-Fi Direct group
/// passphrase.
pub const WIFI_DIRECT_PASSPHRASE_INFO: &[u8] = b"offline-app-wifi-direct-passphrase-v1";

/// HKDF `info` of the secret a configured application's browser nearby room
/// is derived from, in place of the application id.
pub const WEB_NEARBY_INFO: &[u8] = b"offline-app-web-nearby-v1";

/// The HMAC input of a configured application's Bluetooth LE App tag:
/// [`BLE_APP_TAG_LABEL`] followed by the instance's Device id, the `off1…`
/// address exactly as the Device id characteristic serves it (UTF-8).
///
/// The rendering, not the 21-byte payload, because a central computes the tag
/// from the bytes it just read, before anything has proved or parsed them; a
/// Device id that is not an address simply matches no tag. The label is fixed
/// and the Device id last, so the concatenation has one parse.
pub fn ble_app_tag_input(device_id: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(BLE_APP_TAG_LABEL.len() + device_id.len());
    out.extend_from_slice(BLE_APP_TAG_LABEL);
    out.extend_from_slice(device_id.as_bytes());
    out
}

/// Length of an exchange nonce, in bytes.
pub const MEMBER_NONCE_LEN: usize = 16;

/// Length of the truncated hello tag, in bytes.
pub const MEMBER_TAG_LEN: usize = 16;

/// Length of a certificate: address payload, `not_after`, signature.
pub const MEMBER_CERT_LEN: usize = Address::PAYLOAD_LEN + 8 + ED25519_SIGNATURE_LEN;

/// Length of the plaintext both sealed layouts carry: a certificate and an
/// identity signature.
pub const MEMBER_CREDENTIAL_LEN: usize = MEMBER_CERT_LEN + ED25519_SIGNATURE_LEN;

/// Length of a ChaCha20-Poly1305 tag, in bytes.
pub const MEMBER_AEAD_TAG_LEN: usize = 16;

/// Length of a sealed credential: the ciphertext and its tag.
pub const MEMBER_SEALED_LEN: usize = MEMBER_CREDENTIAL_LEN + MEMBER_AEAD_TAG_LEN;

/// Length of the exchange's first frame payload: nonce and tag.
pub const MEMBER_HELLO_LEN: usize = MEMBER_NONCE_LEN + MEMBER_TAG_LEN;

/// Length of an end-to-end proof's salt, in bytes.
pub const MEMBER_PROOF_SALT_LEN: usize = 16;

/// Length of an end-to-end proof: salt and sealed credential.
pub const MEMBER_PROOF_LEN: usize = MEMBER_PROOF_SALT_LEN + MEMBER_SEALED_LEN;

/// The frame metadata key an end-to-end membership proof travels under,
/// base64 (standard, padded) of its [`MEMBER_PROOF_LEN`] bytes.
///
/// In the frame's metadata rather than inside the payload, so `frame_body`
/// is the payload exactly as it travels (the content after the prefix) and
/// no verifier has to re-serialise anything to hash it. The metadata is not
/// covered by the control signature, which costs nothing here: the proof
/// carries its own signature over the recipient, the kind and the body, so
/// stripping it fails closed and moving it onto another frame fails to
/// verify.
pub const MEMBER_PROOF_META_KEY: &str = "__member_proof";

/// The application id every frame a configured node originates carries on
/// the wire (ADR 0037). The real id travels inside the sealed rich payload,
/// which is authoritative on receipt, so the cleartext field names no
/// application. A valid `AppId`, so an older receiver still decodes it.
/// One constant, here, so the engine and a leaf stamp the same string: two
/// spellings would split configured nodes into two groups by the very field
/// this exists to blank.
pub const WIRE_APP_ID: &str = "offline";

/// The version byte every `__MEMBER__` frame body begins with.
pub const MEMBER_FRAME_VERSION: u8 = 1;

const STEP_HELLO: u8 = 1;
const STEP_RESPONSE: u8 = 2;
/// The kind byte of a provisioning body. No `__MEMBER__` step is 3, so
/// neither parser reads the other's body: a provisioning body handed to
/// [`MemberFrame::parse`] is an unknown step, and a membership frame handed
/// to [`MemberProvision::parse`] is the wrong kind.
const KIND_PROVISION: u8 = 3;

/// The version byte a provisioning body begins with.
pub const MEMBER_PROVISION_VERSION: u8 = 1;

/// Length of a provisioning payload: publisher key, certificate, supplied
/// time.
pub const MEMBER_PROVISION_LEN: usize = ED25519_PUBLIC_KEY_LEN + MEMBER_CERT_LEN + 8;

fn invalid(what: &str) -> SealedError {
    SealedError::Deserialization(format!("membership: {what}"))
}

/// The HKDF `info` the application key is derived under:
/// [`APP_KEY_INFO`] followed by the application id's UTF-8 bytes.
///
/// `K_app = HKDF-SHA256(ikm = publisher_public_key, salt = none, info)`,
/// [`APP_KEY_LEN`] bytes. The id is in the derivation so two applications
/// that share a publisher key still have two keys.
pub fn app_key_info(app_id: &str) -> Vec<u8> {
    let mut info = Vec::with_capacity(APP_KEY_INFO.len() + app_id.len());
    info.extend_from_slice(APP_KEY_INFO);
    info.extend_from_slice(app_id.as_bytes());
    info
}

/// A member's certificate: the publisher's signature binding an address
/// until a time.
///
/// Holding one proves nothing: the fields have the right sizes and nothing
/// more. A verifier owes the publisher signature over
/// [`MemberCert::signing_payload`], the comparison of [`MemberCert::address`]
/// with the peer it came from, and [`MemberCert::is_current`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberCert {
    /// The member's address.
    pub address: Address,
    /// The last second, in Unix time, at which the certificate is current.
    pub not_after: u64,
    /// The publisher's Ed25519 signature over
    /// [`MemberCert::signing_payload`].
    pub signature: [u8; ED25519_SIGNATURE_LEN],
}

impl MemberCert {
    /// The bytes the publisher signs:
    /// `MEMBER_CERT_DOMAIN || address_payload(21) || u64be(not_after)`.
    pub fn signing_payload(address: &Address, not_after: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(MEMBER_CERT_DOMAIN.len() + Address::PAYLOAD_LEN + 8);
        out.extend_from_slice(MEMBER_CERT_DOMAIN);
        out.extend_from_slice(&address.payload());
        out.extend_from_slice(&not_after.to_be_bytes());
        out
    }

    /// `address_payload(21) || u64be(not_after) || signature(64)`.
    pub fn to_bytes(&self) -> [u8; MEMBER_CERT_LEN] {
        let mut out = [0u8; MEMBER_CERT_LEN];
        out[..Address::PAYLOAD_LEN].copy_from_slice(&self.address.payload());
        out[Address::PAYLOAD_LEN..Address::PAYLOAD_LEN + 8]
            .copy_from_slice(&self.not_after.to_be_bytes());
        out[Address::PAYLOAD_LEN + 8..].copy_from_slice(&self.signature);
        out
    }

    /// Reads exactly [`MEMBER_CERT_LEN`] bytes.
    ///
    /// # Errors
    ///
    /// [`SealedError::Deserialization`] for any other length or an address
    /// payload that is not a current-version address.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != MEMBER_CERT_LEN {
            return Err(invalid("certificate length"));
        }
        let address = Address::from_payload(&bytes[..Address::PAYLOAD_LEN])
            .map_err(|_| invalid("certificate address"))?;
        let mut not_after = [0u8; 8];
        not_after.copy_from_slice(&bytes[Address::PAYLOAD_LEN..Address::PAYLOAD_LEN + 8]);
        let mut signature = [0u8; ED25519_SIGNATURE_LEN];
        signature.copy_from_slice(&bytes[Address::PAYLOAD_LEN + 8..]);
        Ok(Self {
            address,
            not_after: u64::from_be_bytes(not_after),
            signature,
        })
    }

    /// Whether the certificate is current at `now`, in Unix seconds:
    /// `now <= not_after`.
    ///
    /// A leaf passes the latest time it was supplied, which bounds freshness
    /// after a long gap and is not authentication.
    pub fn is_current(&self, now: u64) -> bool {
        now <= self.not_after
    }
}

/// What both sealed layouts carry: a certificate and an identity signature
/// by the member's own key, over [`transcript_signing_payload`] in the
/// exchange and over [`e2e_proof_signing_payload`] in a proof.
///
/// The certificate alone would be a bearer token anyone who saw it could
/// present; the identity signature is what ties it to this exchange or this
/// frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberCredential {
    /// The member's certificate.
    pub cert: MemberCert,
    /// The member's identity signature.
    pub signature: [u8; ED25519_SIGNATURE_LEN],
}

impl MemberCredential {
    /// `cert(93) || signature(64)`: the AEAD plaintext.
    pub fn to_bytes(&self) -> [u8; MEMBER_CREDENTIAL_LEN] {
        let mut out = [0u8; MEMBER_CREDENTIAL_LEN];
        out[..MEMBER_CERT_LEN].copy_from_slice(&self.cert.to_bytes());
        out[MEMBER_CERT_LEN..].copy_from_slice(&self.signature);
        out
    }

    /// Reads exactly [`MEMBER_CREDENTIAL_LEN`] bytes, an opened AEAD
    /// plaintext.
    ///
    /// # Errors
    ///
    /// [`SealedError::Deserialization`] for any other length or a
    /// certificate [`MemberCert::from_bytes`] refuses.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != MEMBER_CREDENTIAL_LEN {
            return Err(invalid("credential length"));
        }
        let cert = MemberCert::from_bytes(&bytes[..MEMBER_CERT_LEN])?;
        let mut signature = [0u8; ED25519_SIGNATURE_LEN];
        signature.copy_from_slice(&bytes[MEMBER_CERT_LEN..]);
        Ok(Self { cert, signature })
    }
}

/// The exchange's first frame: a fresh nonce and the tag over it.
///
/// A node with no publisher key, or another application's, sends
/// [`MEMBER_HELLO_LEN`] random bytes in the same shape, so the frame says
/// nothing about who sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberHello {
    /// The sender's fresh nonce.
    pub nonce: [u8; MEMBER_NONCE_LEN],
    /// `HMAC-SHA256(K_app, hello_tag_input(..))[..16]`.
    pub tag: [u8; MEMBER_TAG_LEN],
}

/// The HMAC input of a hello's tag:
/// `MEMBER_HELLO_LABEL || nonce_self(16) || addr_self(21) || addr_peer(21)`.
///
/// Both addresses are in it so a hello recorded on one link proves nothing on
/// another: a receiver recomputes it with its own address as the peer.
pub fn hello_tag_input(
    nonce_self: &[u8; MEMBER_NONCE_LEN],
    addr_self: &Address,
    addr_peer: &Address,
) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(MEMBER_HELLO_LABEL.len() + MEMBER_NONCE_LEN + 2 * Address::PAYLOAD_LEN);
    out.extend_from_slice(MEMBER_HELLO_LABEL);
    out.extend_from_slice(nonce_self);
    out.extend_from_slice(&addr_self.payload());
    out.extend_from_slice(&addr_peer.payload());
    out
}

/// The HKDF `salt` of the exchange's sealing keys: the nonce of the side whose
/// address payload sorts lower, then the other's.
///
/// Ordered by address so both ends build the same salt from the two hellos,
/// whichever arrived first.
///
/// # Errors
///
/// [`SealedError::Deserialization`] when the two addresses are equal: a node
/// does not exchange with itself, and an echo of its own hello is the frame
/// that would try.
pub fn exchange_salt(
    a: &Address,
    nonce_a: &[u8; MEMBER_NONCE_LEN],
    b: &Address,
    nonce_b: &[u8; MEMBER_NONCE_LEN],
) -> Result<[u8; 2 * MEMBER_NONCE_LEN]> {
    let (lo, hi) = match a.payload().cmp(&b.payload()) {
        core::cmp::Ordering::Less => (nonce_a, nonce_b),
        core::cmp::Ordering::Greater => (nonce_b, nonce_a),
        core::cmp::Ordering::Equal => return Err(invalid("exchange with itself")),
    };
    let mut out = [0u8; 2 * MEMBER_NONCE_LEN];
    out[..MEMBER_NONCE_LEN].copy_from_slice(lo);
    out[MEMBER_NONCE_LEN..].copy_from_slice(hi);
    Ok(out)
}

/// The HKDF `info` of one direction's sealing key:
/// `MEMBER_EXCHANGE_INFO || addr_sender(21)`.
///
/// `k = HKDF-SHA256(ikm = K_app, salt = exchange_salt(..), info)`, 32 bytes,
/// and the response is sealed under it with a zero nonce and no associated
/// data. The sender is in the derivation because both sides send a response
/// from the same two nonces: one key for both would seal two plaintexts under
/// one key and one nonce, which under ChaCha20-Poly1305 gives away their XOR
/// and the authentication key.
pub fn exchange_key_info(sender: &Address) -> Vec<u8> {
    let mut out = Vec::with_capacity(MEMBER_EXCHANGE_INFO.len() + Address::PAYLOAD_LEN);
    out.extend_from_slice(MEMBER_EXCHANGE_INFO);
    out.extend_from_slice(&sender.payload());
    out
}

/// The bytes a member signs with its identity key in its response:
/// `MEMBER_TRANSCRIPT_DOMAIN || addr_signer(21) || addr_peer(21) ||
/// nonce_signer(16) || nonce_peer(16)`.
///
/// The peer's fresh nonce is what stops a recorded response from replaying:
/// a credential sealed for one exchange names a nonce no later exchange
/// repeats.
pub fn transcript_signing_payload(
    signer: &Address,
    nonce_signer: &[u8; MEMBER_NONCE_LEN],
    peer: &Address,
    nonce_peer: &[u8; MEMBER_NONCE_LEN],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        MEMBER_TRANSCRIPT_DOMAIN.len() + 2 * Address::PAYLOAD_LEN + 2 * MEMBER_NONCE_LEN,
    );
    out.extend_from_slice(MEMBER_TRANSCRIPT_DOMAIN);
    out.extend_from_slice(&signer.payload());
    out.extend_from_slice(&peer.payload());
    out.extend_from_slice(nonce_signer);
    out.extend_from_slice(nonce_peer);
    out
}

/// A `__MEMBER__` frame body.
///
/// Both steps are sent by every node whatever the result, each at one fixed
/// size, so the step byte and the length say nothing either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberFrame {
    /// Step 1: a nonce and a tag, or random bytes of that size.
    Hello(MemberHello),
    /// Step 2: a sealed [`MemberCredential`], or random bytes of that size.
    Response([u8; MEMBER_SEALED_LEN]),
}

impl MemberFrame {
    /// The frame body, the text after the prefix: base64 (standard, padded)
    /// of `version(1) || step(1) || payload`.
    pub fn to_body(&self) -> String {
        let mut raw = Vec::with_capacity(2 + MEMBER_SEALED_LEN);
        raw.push(MEMBER_FRAME_VERSION);
        match self {
            Self::Hello(hello) => {
                raw.push(STEP_HELLO);
                raw.extend_from_slice(&hello.nonce);
                raw.extend_from_slice(&hello.tag);
            }
            Self::Response(sealed) => {
                raw.push(STEP_RESPONSE);
                raw.extend_from_slice(sealed);
            }
        }
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
    }

    /// Reads a frame body.
    ///
    /// # Errors
    ///
    /// [`SealedError::Deserialization`] for a body that is not base64, an
    /// unknown version or step, or a payload not exactly its step's size.
    /// The encoded length is checked before decoding, so an oversized body
    /// costs no allocation.
    pub fn parse(body: &str) -> Result<Self> {
        // base64 of 2 + MEMBER_SEALED_LEN bytes, the larger step.
        const MAX_BODY: usize = (2 + MEMBER_SEALED_LEN).div_ceil(3) * 4;
        if body.len() > MAX_BODY {
            return Err(invalid("frame body too long"));
        }
        let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body)
            .map_err(|_| invalid("frame body is not base64"))?;
        let [version, step, payload @ ..] = raw.as_slice() else {
            return Err(invalid("frame body too short"));
        };
        if *version != MEMBER_FRAME_VERSION {
            return Err(invalid("frame version"));
        }
        match *step {
            STEP_HELLO if payload.len() == MEMBER_HELLO_LEN => {
                let mut nonce = [0u8; MEMBER_NONCE_LEN];
                let mut tag = [0u8; MEMBER_TAG_LEN];
                nonce.copy_from_slice(&payload[..MEMBER_NONCE_LEN]);
                tag.copy_from_slice(&payload[MEMBER_NONCE_LEN..]);
                Ok(Self::Hello(MemberHello { nonce, tag }))
            }
            STEP_RESPONSE if payload.len() == MEMBER_SEALED_LEN => {
                let mut sealed = [0u8; MEMBER_SEALED_LEN];
                sealed.copy_from_slice(payload);
                Ok(Self::Response(sealed))
            }
            STEP_HELLO | STEP_RESPONSE => Err(invalid("frame payload length")),
            _ => Err(invalid("frame step")),
        }
    }
}

/// The frame an end-to-end proof is carried in, and so binds.
///
/// The discriminant is a wire value: it is in every proof's signing payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProofKind {
    /// A key package payload.
    KeyPackage = 1,
    /// A 1:1 Welcome.
    Welcome = 2,
    /// A group Welcome (`__GRP_MLS_WELCOME__`).
    GroupWelcome = 3,
    /// A service discovery query.
    DiscoveryQuery = 4,
    /// A service discovery response.
    DiscoveryResponse = 5,
}

/// The bytes an initiator signs with its identity key inside an end-to-end
/// proof: `MEMBER_E2E_DOMAIN || recipient(21) || kind(1) ||
/// SHA-256(frame_body)(32)`.
///
/// `frame_body` is the package, the Welcome or the discovery payload with the
/// proof field absent. A discovery query is gossip and has no single
/// recipient, so its recipient field is 21 zero bytes, which no address
/// payload is (the version byte is never zero); the query is bound through
/// its body, which carries its `query_id` and `originator`.
///
/// # Errors
///
/// [`SealedError::Deserialization`] when a recipient is given for a
/// discovery query or missing for any other kind: a proof that bound no
/// recipient where one exists would replay to every member.
pub fn e2e_proof_signing_payload(
    recipient: Option<&Address>,
    kind: ProofKind,
    frame_body: &[u8],
) -> Result<Vec<u8>> {
    let recipient = match (kind, recipient) {
        (ProofKind::DiscoveryQuery, None) => [0u8; Address::PAYLOAD_LEN],
        (ProofKind::DiscoveryQuery, Some(_)) => {
            return Err(invalid("a discovery query names no recipient"))
        }
        (_, Some(address)) => address.payload(),
        (_, None) => return Err(invalid("proof recipient missing")),
    };
    let mut out = Vec::with_capacity(MEMBER_E2E_DOMAIN.len() + Address::PAYLOAD_LEN + 1 + 32);
    out.extend_from_slice(MEMBER_E2E_DOMAIN);
    out.extend_from_slice(&recipient);
    out.push(kind as u8);
    out.extend_from_slice(&Sha256::digest(frame_body));
    Ok(out)
}

/// An end-to-end membership proof: a fresh salt and a credential sealed under
/// `HKDF-SHA256(ikm = K_app, salt, info = MEMBER_E2E_KEY_INFO)` with a zero
/// nonce and no associated data.
///
/// The salt is fresh per proof because `K_app` is shared by every install of
/// the application: a fixed key and a fixed nonce would repeat across
/// installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberProof {
    /// The HKDF salt, fresh per proof.
    pub salt: [u8; MEMBER_PROOF_SALT_LEN],
    /// The sealed [`MemberCredential`].
    pub sealed: [u8; MEMBER_SEALED_LEN],
}

impl MemberProof {
    /// `salt(16) || sealed(173)`.
    pub fn to_bytes(&self) -> [u8; MEMBER_PROOF_LEN] {
        let mut out = [0u8; MEMBER_PROOF_LEN];
        out[..MEMBER_PROOF_SALT_LEN].copy_from_slice(&self.salt);
        out[MEMBER_PROOF_SALT_LEN..].copy_from_slice(&self.sealed);
        out
    }

    /// Reads exactly [`MEMBER_PROOF_LEN`] bytes.
    ///
    /// # Errors
    ///
    /// [`SealedError::Deserialization`] for any other length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != MEMBER_PROOF_LEN {
            return Err(invalid("proof length"));
        }
        let mut salt = [0u8; MEMBER_PROOF_SALT_LEN];
        let mut sealed = [0u8; MEMBER_SEALED_LEN];
        salt.copy_from_slice(&bytes[..MEMBER_PROOF_SALT_LEN]);
        sealed.copy_from_slice(&bytes[MEMBER_PROOF_SALT_LEN..]);
        Ok(Self { salt, sealed })
    }
}

/// What a phone hands a paired leaf so the leaf can run the exchange: the
/// publisher key, the leaf's own certificate, and the time.
///
/// Carried as the plaintext of an ordinary sealed message inside the 1:1
/// session, never as a frame of its own, so only the peer holding that
/// session can provision the device. The leaf stores it durably before it
/// acknowledges the message ([`prefixes::MEMBER_PROVISION`]).
///
/// `supplied_time` is the phone's clock in Unix seconds. A leaf reads no
/// clock, so it checks every certificate's `not_after`, its own and its
/// neighbours', against the latest time it was supplied. That bounds
/// freshness after a long gap and is not authentication.
///
/// Holding one proves nothing either: the receiver owes the check that the
/// certificate names its own address and verifies under `publisher_key`.
///
/// [`prefixes::MEMBER_PROVISION`]: crate::prefixes::MEMBER_PROVISION
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberProvision {
    /// The application's publisher Ed25519 public key, the input keying
    /// material of `K_app`.
    pub publisher_key: [u8; ED25519_PUBLIC_KEY_LEN],
    /// The receiving leaf's certificate.
    pub cert: MemberCert,
    /// The sender's clock, Unix seconds.
    pub supplied_time: u64,
}

impl MemberProvision {
    /// The body, the text after the prefix: base64 (standard, padded) of
    /// `version(1) || kind(1) = 3 || publisher_key(32) || cert(93) ||
    /// u64be(supplied_time)`.
    pub fn to_body(&self) -> String {
        let mut raw = Vec::with_capacity(2 + MEMBER_PROVISION_LEN);
        raw.push(MEMBER_PROVISION_VERSION);
        raw.push(KIND_PROVISION);
        raw.extend_from_slice(&self.publisher_key);
        raw.extend_from_slice(&self.cert.to_bytes());
        raw.extend_from_slice(&self.supplied_time.to_be_bytes());
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
    }

    /// Reads a body.
    ///
    /// # Errors
    ///
    /// [`SealedError::Deserialization`] for a body that is not base64, an
    /// unknown version or kind, a payload not exactly
    /// [`MEMBER_PROVISION_LEN`] bytes, or a certificate
    /// [`MemberCert::from_bytes`] refuses. The encoded length is checked
    /// before decoding, so an oversized body costs no allocation.
    pub fn parse(body: &str) -> Result<Self> {
        const MAX_BODY: usize = (2 + MEMBER_PROVISION_LEN).div_ceil(3) * 4;
        if body.len() > MAX_BODY {
            return Err(invalid("provisioning body too long"));
        }
        let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body)
            .map_err(|_| invalid("provisioning body is not base64"))?;
        let [version, kind, payload @ ..] = raw.as_slice() else {
            return Err(invalid("provisioning body too short"));
        };
        if *version != MEMBER_PROVISION_VERSION {
            return Err(invalid("provisioning version"));
        }
        if *kind != KIND_PROVISION {
            return Err(invalid("provisioning kind"));
        }
        if payload.len() != MEMBER_PROVISION_LEN {
            return Err(invalid("provisioning payload length"));
        }
        let mut publisher_key = [0u8; ED25519_PUBLIC_KEY_LEN];
        publisher_key.copy_from_slice(&payload[..ED25519_PUBLIC_KEY_LEN]);
        let cert_end = ED25519_PUBLIC_KEY_LEN + MEMBER_CERT_LEN;
        let cert = MemberCert::from_bytes(&payload[ED25519_PUBLIC_KEY_LEN..cert_end])?;
        let mut supplied_time = [0u8; 8];
        supplied_time.copy_from_slice(&payload[cert_end..]);
        Ok(Self {
            publisher_key,
            cert,
            supplied_time: u64::from_be_bytes(supplied_time),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn addr(byte: u8) -> Address {
        Address::from_hash_bytes([byte; Address::HASH_LEN])
    }

    /// The sizes are wire constants: a frame of another size is refused by
    /// every conforming peer, and the exchange's privacy rests on every node
    /// sending exactly these.
    #[test]
    fn the_sizes_are_pinned() {
        assert_eq!(MEMBER_CERT_LEN, 93);
        assert_eq!(MEMBER_CREDENTIAL_LEN, 157);
        assert_eq!(MEMBER_SEALED_LEN, 173);
        assert_eq!(MEMBER_HELLO_LEN, 32);
        assert_eq!(MEMBER_PROOF_LEN, 189);
    }

    /// Each label is a wire constant shared with implementations not compiled
    /// against this crate, and no two may be equal: a shared domain lets a
    /// signature or key from one step stand in for another.
    #[test]
    fn the_labels_are_pinned_and_distinct() {
        let labels: [&[u8]; 12] = [
            APP_KEY_INFO,
            MEMBER_CERT_DOMAIN,
            MEMBER_HELLO_LABEL,
            MEMBER_EXCHANGE_INFO,
            MEMBER_TRANSCRIPT_DOMAIN,
            MEMBER_E2E_DOMAIN,
            MEMBER_E2E_KEY_INFO,
            BLE_APP_TAG_LABEL,
            WIFI_DIRECT_APP_INFO,
            WIFI_DIRECT_NETWORK_INFO,
            WIFI_DIRECT_PASSPHRASE_INFO,
            WEB_NEARBY_INFO,
        ];
        assert_eq!(BLE_APP_TAG_LABEL, b"offline-app-ble-tag-v1");
        assert_eq!(WIFI_DIRECT_APP_INFO, b"offline-app-wifi-direct-record-v1");
        assert_eq!(
            WIFI_DIRECT_NETWORK_INFO,
            b"offline-app-wifi-direct-network-v1"
        );
        assert_eq!(
            WIFI_DIRECT_PASSPHRASE_INFO,
            b"offline-app-wifi-direct-passphrase-v1"
        );
        assert_eq!(WEB_NEARBY_INFO, b"offline-app-web-nearby-v1");
        assert_eq!(APP_KEY_INFO, b"offline-protocol-app-v1");
        assert_eq!(MEMBER_CERT_DOMAIN, b"offline-app-member-v1");
        assert_eq!(MEMBER_HELLO_LABEL, b"offline-app-member-hello-v1");
        assert_eq!(MEMBER_EXCHANGE_INFO, b"offline-app-member-exchange-v1");
        assert_eq!(
            MEMBER_TRANSCRIPT_DOMAIN,
            b"offline-app-member-transcript-v1"
        );
        assert_eq!(MEMBER_E2E_DOMAIN, b"offline-app-member-e2e-v1");
        assert_eq!(MEMBER_E2E_KEY_INFO, b"offline-app-member-e2e-key-v1");
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert!(!a.starts_with(b) && !b.starts_with(a));
            }
        }
    }

    #[test]
    fn the_ble_app_tag_input_is_the_label_then_the_device_id_as_read() {
        use alloc::string::ToString;
        let device_id = addr(3).to_string();
        let input = ble_app_tag_input(&device_id);
        assert_eq!(&input[..BLE_APP_TAG_LABEL.len()], BLE_APP_TAG_LABEL);
        assert_eq!(&input[BLE_APP_TAG_LABEL.len()..], device_id.as_bytes());
        assert_eq!(ble_app_tag_input(""), BLE_APP_TAG_LABEL);
    }

    #[test]
    fn a_certificate_round_trips_and_refuses_other_lengths() {
        let cert = MemberCert {
            address: addr(7),
            not_after: 1_800_000_000,
            signature: [9; ED25519_SIGNATURE_LEN],
        };
        let bytes = cert.to_bytes();
        assert_eq!(MemberCert::from_bytes(&bytes), Ok(cert.clone()));
        assert!(MemberCert::from_bytes(&bytes[1..]).is_err());
        let mut long = bytes.to_vec();
        long.push(0);
        assert!(MemberCert::from_bytes(&long).is_err());
        let mut versionless = bytes;
        versionless[0] = 0;
        assert!(MemberCert::from_bytes(&versionless).is_err());
    }

    #[test]
    fn a_certificate_is_current_through_its_last_second() {
        let cert = MemberCert {
            address: addr(1),
            not_after: 100,
            signature: [0; ED25519_SIGNATURE_LEN],
        };
        assert!(cert.is_current(99));
        assert!(cert.is_current(100));
        assert!(!cert.is_current(101));
    }

    #[test]
    fn a_credential_and_a_proof_round_trip() {
        let credential = MemberCredential {
            cert: MemberCert {
                address: addr(3),
                not_after: 5,
                signature: [1; ED25519_SIGNATURE_LEN],
            },
            signature: [2; ED25519_SIGNATURE_LEN],
        };
        assert_eq!(
            MemberCredential::from_bytes(&credential.to_bytes()),
            Ok(credential)
        );
        let proof = MemberProof {
            salt: [4; MEMBER_PROOF_SALT_LEN],
            sealed: [5; MEMBER_SEALED_LEN],
        };
        assert_eq!(MemberProof::from_bytes(&proof.to_bytes()), Ok(proof));
        assert!(MemberProof::from_bytes(&[0; MEMBER_PROOF_LEN - 1]).is_err());
    }

    /// Both ends must build one salt from the same two hellos, whichever
    /// order they hold them in.
    #[test]
    fn the_exchange_salt_is_the_same_from_both_ends() {
        let (a, b) = (addr(1), addr(2));
        let (na, nb) = ([0xaa; MEMBER_NONCE_LEN], [0xbb; MEMBER_NONCE_LEN]);
        let from_a = exchange_salt(&a, &na, &b, &nb).unwrap();
        let from_b = exchange_salt(&b, &nb, &a, &na).unwrap();
        assert_eq!(from_a, from_b);
        assert_eq!(&from_a[..MEMBER_NONCE_LEN], &na);
        assert!(exchange_salt(&a, &na, &a, &nb).is_err());
    }

    /// The two directions of one exchange must never share a sealing key.
    #[test]
    fn each_direction_has_its_own_key_info() {
        assert_ne!(exchange_key_info(&addr(1)), exchange_key_info(&addr(2)));
    }

    #[test]
    fn both_frames_round_trip_at_their_one_size() {
        let hello = MemberFrame::Hello(MemberHello {
            nonce: [1; MEMBER_NONCE_LEN],
            tag: [2; MEMBER_TAG_LEN],
        });
        let response = MemberFrame::Response([3; MEMBER_SEALED_LEN]);
        for frame in [hello, response] {
            assert_eq!(MemberFrame::parse(&frame.to_body()), Ok(frame));
        }
    }

    #[test]
    fn a_frame_of_any_other_shape_is_refused() {
        let encode =
            |raw: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw);
        let mut hello = vec![MEMBER_FRAME_VERSION, STEP_HELLO];
        hello.extend_from_slice(&[0; MEMBER_HELLO_LEN]);
        assert!(MemberFrame::parse(&encode(&hello)).is_ok());

        let mut short = hello.clone();
        short.pop();
        let mut wrong_version = hello.clone();
        wrong_version[0] = 2;
        let mut wrong_step = hello.clone();
        wrong_step[1] = 3;
        let mut swapped = hello.clone();
        swapped[1] = STEP_RESPONSE;
        for (name, raw) in [
            ("short", short),
            ("version", wrong_version),
            ("step", wrong_step),
            ("a hello's size under the response step", swapped),
            ("empty", vec![]),
            ("version only", vec![MEMBER_FRAME_VERSION]),
        ] {
            assert!(MemberFrame::parse(&encode(&raw)).is_err(), "{name}");
        }
        assert!(MemberFrame::parse("not base64!").is_err());
        assert!(MemberFrame::parse(&"A".repeat(1024)).is_err());
    }

    #[test]
    fn a_proof_binds_a_recipient_except_for_a_query() {
        let b = addr(2);
        assert!(e2e_proof_signing_payload(Some(&b), ProofKind::KeyPackage, b"kp").is_ok());
        assert!(e2e_proof_signing_payload(None, ProofKind::KeyPackage, b"kp").is_err());
        assert!(e2e_proof_signing_payload(None, ProofKind::DiscoveryQuery, b"q").is_ok());
        assert!(e2e_proof_signing_payload(Some(&b), ProofKind::DiscoveryQuery, b"q").is_err());
    }

    fn provision() -> MemberProvision {
        MemberProvision {
            publisher_key: [6; ED25519_PUBLIC_KEY_LEN],
            cert: MemberCert {
                address: addr(4),
                not_after: 1_800_000_000,
                signature: [7; ED25519_SIGNATURE_LEN],
            },
            supplied_time: 1_790_000_000,
        }
    }

    #[test]
    fn a_provisioning_body_round_trips_at_its_one_size() {
        assert_eq!(MEMBER_PROVISION_LEN, 133);
        let body = provision().to_body();
        assert_eq!(MemberProvision::parse(&body), Ok(provision()));
    }

    #[test]
    fn a_provisioning_body_of_any_other_shape_is_refused() {
        let encode =
            |raw: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw);
        let good = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            provision().to_body(),
        )
        .unwrap();
        let mut short = good.clone();
        short.pop();
        let mut long = good.clone();
        long.push(0);
        let mut wrong_version = good.clone();
        wrong_version[0] = 2;
        let mut wrong_kind = good.clone();
        wrong_kind[1] = STEP_HELLO;
        let mut versionless_cert = good.clone();
        versionless_cert[2 + ED25519_PUBLIC_KEY_LEN] = 0;
        for (name, raw) in [
            ("short", short),
            ("long", long),
            ("version", wrong_version),
            ("a membership step as the kind", wrong_kind),
            ("a certificate address with no version", versionless_cert),
            ("empty", vec![]),
        ] {
            assert!(MemberProvision::parse(&encode(&raw)).is_err(), "{name}");
        }
        assert!(MemberProvision::parse("not base64!").is_err());
        assert!(MemberProvision::parse(&"A".repeat(1024)).is_err());
    }

    /// Neither parser reads the other's body, so a provisioning body that
    /// reached the exchange, or a hello that reached a leaf's provisioning
    /// path, is refused rather than read as something it is not.
    #[test]
    fn a_provisioning_body_and_a_membership_frame_never_parse_as_each_other() {
        assert!(MemberFrame::parse(&provision().to_body()).is_err());
        let hello = MemberFrame::Hello(MemberHello {
            nonce: [1; MEMBER_NONCE_LEN],
            tag: [2; MEMBER_TAG_LEN],
        });
        let response = MemberFrame::Response([3; MEMBER_SEALED_LEN]);
        for frame in [hello, response] {
            assert!(MemberProvision::parse(&frame.to_body()).is_err());
        }
    }

    #[test]
    fn the_configured_wire_app_id_is_pinned() {
        assert_eq!(WIRE_APP_ID, "offline");
        assert!(offline_protocol_core::AppId::new(WIRE_APP_ID).is_ok());
    }

    /// A proof for one kind of frame must not verify as another over the same
    /// bytes: a key package's proof replayed onto a Welcome.
    #[test]
    fn the_kind_separates_two_proofs_over_one_body() {
        let b = addr(2);
        let kinds = [
            ProofKind::KeyPackage,
            ProofKind::Welcome,
            ProofKind::GroupWelcome,
            ProofKind::DiscoveryResponse,
        ];
        let payloads: Vec<_> = kinds
            .iter()
            .map(|kind| e2e_proof_signing_payload(Some(&b), *kind, b"same").unwrap())
            .collect();
        for (i, a) in payloads.iter().enumerate() {
            for other in &payloads[i + 1..] {
                assert_ne!(a, other);
            }
        }
    }
}

/// The conformance vectors in `tests/data/app-membership-v1.vectors.json`.
///
/// The chapter these pin is the application membership section of
/// `docs/spec/control-messages.md`. They were computed by
/// `tools/spec-vectors/generate.py` from the construction stated there, not by
/// running the codec above. A wrong byte here is a member every other
/// implementation reads as a stranger, and the exchange is built so that a
/// stranger looks like nothing at all, so these are the only place it shows.
#[cfg(all(test, feature = "std"))]
mod vectors {
    use super::*;
    use hmac::Mac;

    const VECTORS: &str = include_str!("../tests/data/app-membership-v1.vectors.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).expect("the vector file is JSON")
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        assert!(text.len().is_multiple_of(2), "hex has an odd length");
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    fn field<'a>(v: &'a serde_json::Value, path: &[&str]) -> &'a str {
        path.iter()
            .fold(v, |v, key| &v[*key])
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is a string"))
    }

    fn address(v: &serde_json::Value, key: &str) -> Address {
        field(v, &["addresses", key]).parse().expect("an address")
    }

    fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
        bytes.try_into().expect("the vector field's size")
    }

    fn hkdf(ikm: &[u8], salt: Option<&[u8]>, info: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        hkdf::Hkdf::<Sha256>::new(salt, ikm)
            .expand(info, &mut out)
            .expect("32 bytes is a valid HKDF length");
        out
    }

    /// Asserts the file still carries what it carried, before anything reads
    /// it: a block a bad merge dropped would make the tests below vacuous.
    #[test]
    fn the_vector_file_is_the_size_it_was() {
        let v = vectors();
        for block in [
            "app_key",
            "addresses",
            "certificate",
            "hello",
            "exchange",
            "proof",
            "provision",
            "identifiers",
        ] {
            assert!(v[block].is_object(), "{block} is present");
        }
        assert_eq!(v["rejects"].as_array().expect("rejects").len(), 6);
    }

    #[test]
    fn the_application_key_matches() {
        let v = vectors();
        let info = app_key_info(field(&v, &["app_key", "app_id"]));
        assert_eq!(hex(&info), field(&v, &["app_key", "info_hex"]));
        let pk = unhex(field(&v, &["app_key", "publisher_public_key_hex"]));
        assert_eq!(
            hex(&hkdf(&pk, None, &info)),
            field(&v, &["app_key", "k_app_hex"])
        );
    }

    #[test]
    fn the_addresses_are_their_payloads() {
        let v = vectors();
        for key in ["a", "b"] {
            assert_eq!(
                hex(&address(&v, key).payload()),
                field(&v, &["addresses", &format!("{key}_payload_hex")])
            );
        }
    }

    #[test]
    fn the_certificate_matches() {
        let v = vectors();
        let addr_a = address(&v, "a");
        let not_after = v["certificate"]["not_after"].as_u64().expect("not_after");
        assert_eq!(
            hex(&MemberCert::signing_payload(&addr_a, not_after)),
            field(&v, &["certificate", "signing_payload_hex"])
        );
        let cert = MemberCert {
            address: addr_a,
            not_after,
            signature: array(&unhex(field(&v, &["certificate", "signature_hex"]))),
        };
        let bytes = cert.to_bytes();
        assert_eq!(hex(&bytes), field(&v, &["certificate", "bytes_hex"]));
        assert_eq!(MemberCert::from_bytes(&bytes), Ok(cert));
    }

    #[test]
    fn the_hello_matches() {
        let v = vectors();
        let (a, b) = (address(&v, "a"), address(&v, "b"));
        let nonce = array(&unhex(field(&v, &["hello", "nonce_hex"])));
        let input = hello_tag_input(&nonce, &a, &b);
        assert_eq!(hex(&input), field(&v, &["hello", "tag_input_hex"]));

        let k_app = unhex(field(&v, &["app_key", "k_app_hex"]));
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(&k_app).expect("any key length");
        mac.update(&input);
        let tag: [u8; MEMBER_TAG_LEN] = array(&mac.finalize().into_bytes()[..MEMBER_TAG_LEN]);
        assert_eq!(hex(&tag), field(&v, &["hello", "tag_hex"]));

        let frame = MemberFrame::Hello(MemberHello { nonce, tag });
        assert_eq!(frame.to_body(), field(&v, &["hello", "body"]));
        assert_eq!(MemberFrame::parse(field(&v, &["hello", "body"])), Ok(frame));
    }

    #[test]
    fn the_exchange_matches() {
        let v = vectors();
        let (a, b) = (address(&v, "a"), address(&v, "b"));
        let na = array(&unhex(field(&v, &["exchange", "nonce_a_hex"])));
        let nb = array(&unhex(field(&v, &["exchange", "nonce_b_hex"])));
        let salt = exchange_salt(&a, &na, &b, &nb).expect("two addresses");
        assert_eq!(hex(&salt), field(&v, &["exchange", "salt_hex"]));
        assert_eq!(exchange_salt(&b, &nb, &a, &na), Ok(salt));

        let k_app = unhex(field(&v, &["app_key", "k_app_hex"]));
        for (key, sender) in [("a", &a), ("b", &b)] {
            let info = exchange_key_info(sender);
            assert_eq!(
                hex(&info),
                field(&v, &["exchange", &format!("key_info_{key}_hex")])
            );
            assert_eq!(
                hex(&hkdf(&k_app, Some(&salt), &info)),
                field(&v, &["exchange", &format!("key_{key}_hex")])
            );
        }
        assert_ne!(
            field(&v, &["exchange", "key_a_hex"]),
            field(&v, &["exchange", "key_b_hex"])
        );

        assert_eq!(
            hex(&transcript_signing_payload(&a, &na, &b, &nb)),
            field(&v, &["exchange", "transcript_a_hex"])
        );

        let credential_bytes = unhex(field(&v, &["exchange", "credential_hex"]));
        let credential = MemberCredential::from_bytes(&credential_bytes).expect("a credential");
        assert_eq!(
            hex(&credential.cert.to_bytes()),
            field(&v, &["certificate", "bytes_hex"])
        );
        assert_eq!(credential.to_bytes().to_vec(), credential_bytes);

        let sealed = array(&unhex(field(&v, &["exchange", "response_sealed_hex"])));
        let frame = MemberFrame::Response(sealed);
        assert_eq!(frame.to_body(), field(&v, &["exchange", "response_body"]));
        assert_eq!(
            MemberFrame::parse(field(&v, &["exchange", "response_body"])),
            Ok(frame)
        );
    }

    #[test]
    fn the_proof_matches() {
        let v = vectors();
        let b = address(&v, "b");
        let kp = &v["proof"]["key_package"];
        assert_eq!(field(kp, &["recipient"]), field(&v, &["addresses", "b"]));
        assert_eq!(kp["kind"].as_u64(), Some(ProofKind::KeyPackage as u64));
        assert_eq!(
            hex(&e2e_proof_signing_payload(
                Some(&b),
                ProofKind::KeyPackage,
                &unhex(field(kp, &["frame_body_hex"]))
            )
            .expect("a payload")),
            field(kp, &["signing_payload_hex"])
        );

        let query = &v["proof"]["discovery_query"];
        assert_eq!(
            query["kind"].as_u64(),
            Some(ProofKind::DiscoveryQuery as u64)
        );
        assert_eq!(
            hex(&e2e_proof_signing_payload(
                None,
                ProofKind::DiscoveryQuery,
                &unhex(field(query, &["frame_body_hex"]))
            )
            .expect("a payload")),
            field(query, &["signing_payload_hex"])
        );

        let salt = unhex(field(&v, &["proof", "salt_hex"]));
        let k_app = unhex(field(&v, &["app_key", "k_app_hex"]));
        assert_eq!(
            hex(&hkdf(&k_app, Some(&salt), MEMBER_E2E_KEY_INFO)),
            field(&v, &["proof", "key_hex"])
        );
        let bytes = unhex(field(&v, &["proof", "bytes_hex"]));
        let proof = MemberProof::from_bytes(&bytes).expect("a proof");
        assert_eq!(proof.salt.to_vec(), salt);
        assert_eq!(proof.to_bytes().to_vec(), bytes);
    }

    /// The labels and inputs of what a configured application shows on the
    /// air. The engine and the browser crate run the primitives; this pins
    /// what they run them over.
    #[test]
    fn the_identifiers_match() {
        let v = vectors();
        let k_app = unhex(field(&v, &["app_key", "k_app_hex"]));
        for key in ["a", "b"] {
            let input = ble_app_tag_input(field(&v, &["addresses", key]));
            assert_eq!(
                hex(&input),
                field(
                    &v,
                    &["identifiers", "ble_app_tag", &format!("{key}_input_hex")]
                )
            );
            let mut mac = hmac::Hmac::<Sha256>::new_from_slice(&k_app).expect("any key length");
            mac.update(&input);
            assert_eq!(
                hex(&mac.finalize().into_bytes()[..BLE_APP_TAG_LEN]),
                field(
                    &v,
                    &["identifiers", "ble_app_tag", &format!("{key}_tag_hex")]
                )
            );
        }
        let wd = |info: &[u8], n: usize| hex(&hkdf(&k_app, None, info)[..n]);
        assert_eq!(
            wd(WIFI_DIRECT_APP_INFO, 4),
            field(&v, &["identifiers", "wifi_direct", "record_app"])
        );
        assert_eq!(
            format!("DIRECT-op-{}", wd(WIFI_DIRECT_NETWORK_INFO, 4)),
            field(&v, &["identifiers", "wifi_direct", "network_name"])
        );
        assert_eq!(
            wd(WIFI_DIRECT_PASSPHRASE_INFO, 16),
            field(&v, &["identifiers", "wifi_direct", "passphrase"])
        );
        assert_eq!(
            hex(&hkdf(&k_app, None, WEB_NEARBY_INFO)),
            field(&v, &["identifiers", "web_nearby", "secret_hex"])
        );
    }

    #[test]
    fn the_provisioning_body_matches() {
        let v = vectors();
        let p = &v["provision"];
        assert_eq!(
            field(p, &["publisher_public_key_hex"]),
            field(&v, &["app_key", "publisher_public_key_hex"])
        );
        let cert_bytes = unhex(field(p, &["certificate_hex"]));
        assert_eq!(hex(&cert_bytes), field(&v, &["certificate", "bytes_hex"]));
        let provision = MemberProvision {
            publisher_key: array(&unhex(field(p, &["publisher_public_key_hex"]))),
            cert: MemberCert::from_bytes(&cert_bytes).expect("a certificate"),
            supplied_time: p["supplied_time"].as_u64().expect("supplied_time"),
        };
        assert_eq!(provision.to_body(), field(p, &["body"]));
        assert_eq!(MemberProvision::parse(field(p, &["body"])), Ok(provision));
        assert!(
            MemberFrame::parse(field(p, &["body"])).is_err(),
            "a provisioning body is never a membership frame"
        );
    }

    #[test]
    fn every_rejected_provisioning_body_is_refused() {
        let v = vectors();
        let rejects = v["provision_rejects"]
            .as_array()
            .expect("provision_rejects");
        assert_eq!(rejects.len(), 5);
        for case in rejects {
            let name = field(case, &["name"]);
            assert!(
                MemberProvision::parse(field(case, &["body"])).is_err(),
                "[{name}] parsed"
            );
        }
    }

    #[test]
    fn every_rejected_body_is_refused() {
        for case in vectors()["rejects"].as_array().expect("rejects") {
            let name = field(case, &["name"]);
            assert!(
                MemberFrame::parse(field(case, &["body"])).is_err(),
                "[{name}] parsed"
            );
        }
    }
}
