//! Minting the key package a device advertises itself with.
//!
//! Two properties here are load bearing, and each is a library default that
//! produces a package the peer refuses. Both were found by running the two MLS
//! implementations against each other rather than by reading either one's
//! documentation, which is why `tools/mls-interop` restores each in turn and
//! requires the refusal.
//!
//! **`not_before` is backdated.** The peer tests `not_before < now`, strictly,
//! while mls-rs writes `not_before` as exactly the timestamp it is handed. A
//! package stamped with the current second is refused for being not yet valid.
//! The backdate is also the margin that absorbs clock skew between the two
//! devices, which is the form this failure actually takes in the field.
//!
//! **The timestamp is supplied, never read.** This is the one with
//! consequences past the call site. mls-rs stamps `not_before = 0` when it
//! cannot read a clock, so a bare-metal device that lets it try emits a
//! validity window in 1970 and is refused as expired: a device shipping that
//! way never pairs at all. Hence `now_unix_secs` on every entry point here,
//! and hence a leaf node needs a time source at pairing, from its radio stack,
//! its commissioner, or the pairing exchange.
//!
//! Key package validity is a **freshness bound, not an authentication
//! mechanism**. A wrong clock costs availability rather than confidentiality.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use mls_rs::client_builder::MlsConfig;
use mls_rs::time::MlsTime;
use mls_rs::Client;
use mls_rs_core::mls_rs_codec::MlsEncode;
use offline_protocol_core::WIRE_VERSION_V1;
use offline_protocol_sealed::{
    KeyPackagePayload, CTRL_SIGN_V2, LEAF_KEY_PACKAGE_LIFETIME,
    LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS, MLS_ENVELOPE_COMPACT_V1,
};

use crate::error::{LeafError, Result};

/// A freshly minted key package, and the name storage keys it by.
pub(crate) struct Minted {
    /// The bare key package, as it goes on the wire.
    pub(crate) data: Vec<u8>,
    /// The package's reference, hex, which is both the key
    /// [`KeyPackageStorage`](mls_rs_core::key_package::KeyPackageStorage)
    /// files it under and the name a Welcome spends it by.
    pub(crate) reference: String,
}

/// Mints a key package and returns its **bare** encoding and its reference.
///
/// mls-rs's convenience API returns a key package wrapped in an MLS message,
/// and this protocol puts the bare key package on the wire. Both forms are
/// legal MLS and only one of them is what the peer's parser accepts, so the
/// wrapper is removed here rather than left for a caller to notice.
///
/// The reference comes back with it because a package is a **bearer token**,
/// and the only defence against one being spent by whoever copied it off the
/// air is knowing which peer this one went to. See
/// [`PeerRecord::key_package_ref`](crate::adapters::PeerRecord::key_package_ref).
/// It is read from the wrapper before that wrapper is consumed, so it is the
/// reference of exactly the bytes returned beside it rather than a second
/// derivation that could disagree.
pub(crate) fn mint(client: &Client<impl MlsConfig>, now_unix_secs: u64) -> Result<Minted> {
    let not_before = now_unix_secs.saturating_sub(LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS);

    let message = client
        .generate_key_package_message(
            Default::default(),
            Default::default(),
            Some(MlsTime::from(not_before)),
        )
        .map_err(|e| LeafError::Mls(format!("cannot generate a key package: {e:?}")))?;

    let reference = message
        .key_package_reference(&crate::identity::suite_provider()?)
        .map_err(|e| LeafError::Mls(format!("cannot reference the key package: {e:?}")))?
        .ok_or_else(|| LeafError::Mls(String::from("generated message is not a key package")))?;

    let data = message
        .into_key_package()
        .ok_or_else(|| LeafError::Mls(String::from("generated message is not a key package")))?
        .mls_encode_to_vec()
        .map_err(|e| LeafError::Mls(format!("cannot encode the key package: {e:?}")))?;

    Ok(Minted {
        data,
        reference: crate::adapters::hex(&reference),
    })
}

/// Builds the advertisement body that carries the key package.
///
/// # What a leaf advertises
///
/// The compact envelope and the binary hop encoding, because both are pure
/// parsing work that saves radio time on a link that has very little. Nothing
/// else: `rich_versions` and `data_versions` stay empty, so a peer sends plain
/// text and no document sync frames, which by the protocol's own rule is a
/// downgrade to the floor rather than an error. A device that advertised a
/// capability it does not implement would be sent frames it renders as
/// literal text.
pub(crate) fn payload(
    user_id: &str,
    key_package_data: Vec<u8>,
    session_reset: bool,
) -> KeyPackagePayload {
    KeyPackagePayload {
        user_id: user_id.to_string(),
        key_package_data,
        // Relative rather than absolute, so the receiver applies it to their
        // own clock and skew between the two devices cannot expire a package
        // that is perfectly valid.
        remaining_lifetime_ms: LEAF_KEY_PACKAGE_LIFETIME
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        timestamp_ms: 0,
        session_reset,
        wire_versions: alloc::vec![WIRE_VERSION_V1],
        env_versions: alloc::vec![MLS_ENVELOPE_COMPACT_V1],
        rich_versions: Vec::new(),
        data_versions: Vec::new(),
        // A leaf verifies the freshness-bound payload and nothing else, so it
        // says so on the one channel it has for saying anything. There is no
        // legacy leaf to be compatible with: this crate's first release is
        // the one that introduced the device at all.
        ctrl_versions: alloc::vec![CTRL_SIGN_V2],
        nostr_pubkey: None,
    }
}
