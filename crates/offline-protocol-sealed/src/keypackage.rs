//! The key package payload, which is how a peer says who it is and what it
//! can parse.
//!
//! This is the body of a [`prefixes::KEY_PACKAGE`] frame, and it is the only
//! channel in this protocol by which capabilities are advertised. A device
//! with no key package to mint has no way to say it understands anything, so
//! every frame sent to it falls to the protocol floor forever. That is the
//! reason a leaf node runs MLS rather than something smaller: not the
//! sealing, the advertising.
//!
//! Both ends must agree on this shape, and they build it with different MLS
//! implementations, so it lives here rather than in the engine.
//!
//! [`prefixes::KEY_PACKAGE`]: crate::prefixes::KEY_PACKAGE

use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

/// Payload for key package exchange.
///
/// Every field except `user_id` and `key_package_data` carries
/// `#[serde(default)]`, so a payload written by an older peer that has never
/// heard of a field decodes cleanly with that field empty. Empty selects the
/// floor, by rule and never by error, which is what makes a mixed fleet safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPackagePayload {
    /// User ID of the key package owner.
    pub user_id: String,
    /// Raw key package data.
    ///
    /// A **bare** key package, not one wrapped in an MLS message. Both forms
    /// are legal MLS and only one of them is this protocol's wire, so an
    /// implementation whose convenience API returns the wrapped form has to
    /// unwrap it before encoding.
    pub key_package_data: Vec<u8>,
    /// Remaining valid lifetime in milliseconds (relative, not absolute).
    /// Receiver applies this to their local clock, avoiding clock skew issues.
    #[serde(default)]
    pub remaining_lifetime_ms: u64,
    /// Legacy absolute timestamp field, ignored on receive, kept for
    /// backward compatibility with old nodes that may still send it.
    #[serde(default)]
    pub timestamp_ms: u64,
    /// When `true`, the sender has reset their MLS session state and the
    /// receiver should discard any existing session for this peer before
    /// establishing a new one.
    ///
    /// Two senders set it. Post-unblock convergence: when Alice unblocks Bob,
    /// Alice deletes her MLS session and sends a fresh key package with this
    /// flag, so Bob deletes his now-orphaned session and both converge on one
    /// fresh group. And a driven session rekey, which is how post-compromise
    /// security reaches a peer that never commits: the rekey arrives as this
    /// flag on a key package, never as an unsolicited Welcome.
    ///
    /// A receiver that treats a reset as an ordinary key package refresh keeps
    /// a session the sender has already discarded, and every later frame from
    /// it decrypts to nothing.
    #[serde(default)]
    pub session_reset: bool,

    /// Wire-format versions the sender can decode (e.g. `[1]` for binary v1).
    /// Absent on legacy nodes (`#[serde(default)]` gives empty, meaning JSON
    /// only), so an old peer is never sent a binary frame it cannot parse.
    ///
    /// Trust boundary: this rides in the plaintext payload *alongside* the
    /// signed MLS `key_package_data`, not *inside* the signature, so it is not
    /// cryptographically bound to the sender. An attacker on the pre-session
    /// bootstrap could strip it (a harmless JSON downgrade) or forge `[1]`
    /// onto a legacy peer (making us emit binary that peer drops, a targeted
    /// delivery denial of service). This grants no new capability: such an
    /// attacker already controls key package delivery and could deny service
    /// outright. The negotiation is a performance optimization, never a
    /// security control.
    #[serde(default)]
    pub wire_versions: Vec<u8>,

    /// MLS envelope formats the sender can parse (e.g. `[1]` for the compact
    /// envelope, [`MLS_ENVELOPE_COMPACT_V1`]). Absent on legacy nodes
    /// (`#[serde(default)]` gives empty, meaning the legacy JSON envelope
    /// only), so an old peer is never sent an envelope it cannot parse.
    ///
    /// Distinct from `wire_versions`: that one is hop-local (which *frames*
    /// the peer decodes), this one is end-to-end (which
    /// [`prefixes::ENCRYPTED`] payload encodings the *recipient* parses after
    /// any number of relay hops).
    ///
    /// Trust boundary: identical to `wire_versions` above, a plaintext field
    /// and not signature-bound. Stripping it downgrades to the JSON envelope
    /// (harmless); forging it onto a legacy peer makes us emit envelopes that
    /// peer rejects with a `message_decryption_failed` event (a targeted
    /// delivery denial of service an attacker in that position already has). A
    /// performance optimization, never a security control.
    ///
    /// [`prefixes::ENCRYPTED`]: crate::prefixes::ENCRYPTED
    #[serde(default)]
    pub env_versions: Vec<u8>,

    /// Sealed rich-payload versions the sender can parse (e.g. `[1]` for
    /// `RICH_PAYLOAD_V1`). Absent on legacy nodes (`#[serde(default)]` gives
    /// empty, meaning plain text only), so an old peer is never sent a
    /// `__RICH_V1__` body it would surface as raw JSON text.
    ///
    /// End-to-end like `env_versions` (what the *recipient* parses inside the
    /// decrypted MLS plaintext), not hop-local like `wire_versions`.
    ///
    /// Trust boundary: identical to the two fields above, a plaintext field
    /// and not signature-bound. Stripping it downgrades to plain text with the
    /// rich extras dropped (harmless); forging it onto a legacy peer makes us
    /// seal bodies that peer renders as JSON text (a nuisance an attacker in
    /// that position could match by corrupting delivery outright). A feature
    /// negotiation, never a security control.
    #[serde(default)]
    pub rich_versions: Vec<u8>,

    /// Replicated-document sync versions the sender speaks (e.g. `[1]` for
    /// `DATA_SYNC_V1`). Absent on legacy nodes and on installs with the data
    /// layer switched off (`#[serde(default)]` gives empty), and toward a peer
    /// that advertises nothing here no `__DATA_V1__` frame is ever sent.
    ///
    /// End-to-end like `env_versions` and `rich_versions`: it says what the
    /// *recipient* parses inside the decrypted MLS plaintext, not what a
    /// directly connected neighbour decodes on the wire.
    ///
    /// The version doubles as the engine's encoding generation. The CRDT
    /// engine promises that new code reads old encodings and says nothing
    /// about the reverse, so a future encoding change is a new version byte
    /// here and a mixed fleet declines what it cannot read instead of
    /// discovering it at import.
    ///
    /// Trust boundary: identical to the capability lists above, a plaintext
    /// field and not signature-bound. Stripping it stops documents replicating
    /// with that peer (they stay editable locally and converge whenever a
    /// genuine advertisement arrives); forging it onto a peer that cannot
    /// parse the frames makes us send bodies they consume and drop. Neither
    /// grants an attacker anything they could not achieve by dropping the
    /// packets outright. A feature negotiation, never a security control.
    #[serde(default)]
    pub data_versions: Vec<u8>,

    /// Control-frame signing versions the sender **verifies** (e.g. `[2]` for
    /// [`CTRL_SIGN_V2`], the payload that binds the frame's timestamp).
    /// Absent on legacy nodes (`#[serde(default)]` gives empty, meaning the v1
    /// payload only), so a peer that has never heard of the freshness-bound
    /// domain is never sent a signature it would read as invalid.
    ///
    /// Unlike every capability list above, this one says what the sender
    /// *accepts*, not what it emits. It has to: a signature is produced once
    /// and verified by the far end, so the choice of domain belongs to whoever
    /// is going to check it.
    ///
    /// Trust boundary: this is a plaintext field like the others, and an
    /// attacker who strips it makes us sign the older payload toward a peer
    /// that would have accepted the newer one. That downgrade is real and it
    /// is why stripping it is not the end of the story: a receiver that has
    /// **once** verified a v2 signature from a peer records that durably and
    /// refuses that peer's v1 control frames from then on, so the strip works
    /// only until the first genuine v2 frame arrives and never afterwards. The
    /// field is what makes the first one possible; the record is what makes it
    /// stick.
    #[serde(default)]
    pub ctrl_versions: Vec<u8>,

    /// This install's Nostr public key (x-only, 64-char lowercase hex), so a
    /// peer can seal Nostr gift wraps to a key only this install holds.
    ///
    /// `None` on legacy nodes and on installs with Nostr disabled. A peer
    /// without it seals to our *publicly computable* key instead, deliverable
    /// either way, but readable by anyone who guesses our user id, so the
    /// difference is real privacy rather than a mere optimization.
    ///
    /// Trust boundary: **unlike the capability lists above, this one is only
    /// honoured from a signed key package.** All four ride in the same
    /// plaintext payload, but this field is consumed as a *destination key*,
    /// not as a feature hint, so the distinction matters: a wrong capability
    /// costs a fallback, whereas a wrong key here means envelope metadata is
    /// sealed *to whoever supplied it* and is then readable off a public
    /// relay, passively, for as long as the value stands.
    ///
    /// The canonical signing payload covers the whole
    /// [`prefixes::KEY_PACKAGE`] body under the sender's Ed25519 signature,
    /// which the gate verifies against the key their address derives from, so
    /// on this prefix an unsigned frame does not reach dispatch at all. The
    /// receiving path still consumes this field only when the gate reports the
    /// frame was actually signed, which costs nothing: a key package exists
    /// only once MLS is initialized, and the sender signs unconditionally in
    /// that state, so every genuine package carrying this field is signed.
    ///
    /// Stripping it is still possible for a network attacker and downgrades us
    /// to the bootstrap key, which is a privacy downgrade, not a disclosure to
    /// the attacker, and one they could equally achieve by dropping the packet.
    ///
    /// [`prefixes::KEY_PACKAGE`]: crate::prefixes::KEY_PACKAGE
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nostr_pubkey: Option<String>,
}

/// Compact MLS envelope version advertised in
/// [`KeyPackagePayload::env_versions`]: the [`prefixes::ENCRYPTED`] payload is
/// base64 of [`EncryptedMessage::to_bytes`] instead of the legacy JSON form
/// (whose `ciphertext` field renders as a roughly 3.6x integer array).
/// Receivers distinguish the two by the byte after the prefix: `{` opens the
/// JSON envelope and never occurs in base64.
///
/// [`prefixes::ENCRYPTED`]: crate::prefixes::ENCRYPTED
/// [`EncryptedMessage::to_bytes`]: crate::EncryptedMessage::to_bytes
pub const MLS_ENVELOPE_COMPACT_V1: u8 = 1;

/// Control-frame signing version advertised in
/// [`KeyPackagePayload::ctrl_versions`]: the sender verifies signatures over
/// [`control_signing_payload_v2`], which binds the frame's timestamp and so
/// can be refused for being stale.
///
/// There is no `CTRL_SIGN_V1` constant, and the absence is deliberate: an
/// empty list already means "v1 only", and a named constant for it would
/// invite a peer to advertise `[1]` as though declining the newer payload were
/// a capability rather than the floor.
///
/// [`control_signing_payload_v2`]: crate::canonical::control_signing_payload_v2
pub const CTRL_SIGN_V2: u8 = 2;
