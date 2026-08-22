//! Errors a leaf node can produce.

use alloc::string::String;
use thiserror::Error;

/// Result type for this crate.
pub type Result<T> = core::result::Result<T, LeafError>;

/// What can go wrong on a leaf node.
///
/// Deliberately not `#[non_exhaustive]`, for the reason
/// [ADR 0022](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/adr/0022-one-sealed-layer-shared-with-the-leaf.md)
/// gives for `SealedError`: firmware that maps these onto its own error space
/// should get a compile error when a variant is added, rather than a wildcard
/// arm that silently renders a new failure as an old one.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LeafError {
    /// The backing store failed.
    #[error("Storage failed: {0}")]
    Storage(String),

    /// The device has no identity yet, and an operation needed one.
    #[error("Device is not provisioned")]
    NotProvisioned,

    /// The device already has an identity, and provisioning would replace it.
    ///
    /// Overwriting an identity is not a recoverable state: the device's
    /// address changes, every peer's paired record names a device that no
    /// longer exists, and nothing on the wire says why.
    #[error("Device is already provisioned")]
    AlreadyProvisioned,

    /// A cryptographic operation failed.
    #[error("Crypto failed: {0}")]
    Crypto(String),

    /// MLS refused an operation.
    #[error("MLS failed: {0}")]
    Mls(String),

    /// A frame did not parse.
    #[error("Malformed frame: {0}")]
    MalformedFrame(String),

    /// A control frame arrived unsigned, or its signature did not verify.
    ///
    /// Unsigned is a refusal rather than a downgrade: every control frame in
    /// this protocol carries a signature, and one that does not is either an
    /// implementation that skipped the step or an injection.
    #[error("Control frame refused: {0}")]
    ControlFrameRefused(String),

    /// A presented key did not derive to the address that claimed it, or the
    /// claimed identifier is not an address at all.
    ///
    /// Both are the same refusal on purpose. An identifier that does not parse
    /// as an address has no derivation to check, and answering "acceptable"
    /// for it is the bypass rather than a lenience.
    #[error("Identity binding failed: {0}")]
    IdentityBinding(String),

    /// No session exists with this peer.
    #[error("No session with {0}")]
    NoSession(String),

    /// A Welcome asked this device to join on a key package it did not mint
    /// for the peer that sent it.
    ///
    /// A key package is a **bearer token**. It rides in a frame that is signed
    /// but not encrypted, so anyone in radio range copies one off the air, and
    /// every other gate on a Welcome then passes for them honestly: they do
    /// hold the key their own address derives from, and they did build the
    /// group this pair's id names. Only this refusal separates the peer the
    /// package was minted for from whoever else heard it.
    ///
    /// Its own variant rather than an identity binding, because the two send
    /// firmware to different places. An identity binding failure says a peer
    /// is not who it claims; this says the peer is exactly who it claims and
    /// is spending something that was never given to it.
    #[error("Unsolicited welcome: {0}")]
    UnsolicitedWelcome(String),

    /// A Welcome spends the key package this device minted for its sender, and
    /// that package is no longer held.
    ///
    /// Neither an attack nor an identity failure: the peer is exactly who it
    /// claims and is spending exactly what it was given. The package is simply
    /// gone, because an earlier join consumed it (an init key is single use,
    /// so a Welcome is not replayable) or because later mints pushed it out of
    /// the bounded ring.
    ///
    /// Its own variant because the repair is a fresh package rather than a
    /// retry, and because the alternative is the same condition arriving from
    /// inside MLS as a Welcome that will not decode, which reads as a broken
    /// peer and sends a bench to the wire.
    #[error("Stale key package: {0}")]
    StaleKeyPackage(String),

    /// The device already holds as many peers as it keeps room for, and none
    /// of them is an incomplete pairing that could be recycled.
    ///
    /// Refusing rather than evicting an established peer is deliberate. A
    /// device with a full table is one a stranger cannot displace the owner
    /// from; the owner clears a slot with
    /// [`LeafDevice::unpair`](crate::LeafDevice::unpair).
    #[error("Peer table is full")]
    TooManyPeers,

    /// The sealed layer refused a value.
    #[error("{0}")]
    Sealed(String),
}

impl From<offline_protocol_sealed::SealedError> for LeafError {
    fn from(e: offline_protocol_sealed::SealedError) -> Self {
        // The inner text passes through rather than the rendered `Display` of
        // a wrapper, so a failure reads the same here as it does on the phone.
        use offline_protocol_sealed::SealedError as S;
        match e {
            S::Serialization(m) => LeafError::Sealed(alloc::format!("Serialization failed: {m}")),
            S::Deserialization(m) => {
                LeafError::Sealed(alloc::format!("Deserialization failed: {m}"))
            }
            S::InvalidGroupId(m) => LeafError::Sealed(alloc::format!("Invalid group id: {m}")),
            S::InvalidPublicKey(m) => LeafError::Sealed(alloc::format!("Invalid public key: {m}")),
            S::FieldTooLarge(n) => LeafError::Sealed(alloc::format!(
                "Field too large for canonical payload length prefix: {n} bytes"
            )),
        }
    }
}
