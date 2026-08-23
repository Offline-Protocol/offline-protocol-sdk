//! Does the leaf's MLS stack talk to the phone's?
//!
//! ADR 0020 left a leaf node's payload cryptography open, on the grounds that
//! MLS does not fit on the part. That is true of OpenMLS and of large groups,
//! and a phone paired with one device is neither: two members, a three-node
//! ratchet tree. `tools/embedded-footprint` answers whether the leaf's stack
//! fits. This answers the other half, which no amount of flash measurement can:
//! whether it is talking to the phone or only to itself.
//!
//! The phone side is OpenMLS 0.7.4 configured the way the SDK configures it
//! (`crates/offline-protocol-mls/src/group.rs`): ciphersuite 3, the ratchet
//! tree carried as an extension, the SDK's sender-ratchet tolerance. The leaf
//! side is mls-rs 0.56 on the RustCrypto provider, the configuration the
//! footprint harness measures. Both versions are pinned with `=`, because an
//! interop result is a statement about two specific versions and means nothing
//! if either floats.
//!
//! The flow is a never-committing member's whole life: pair, join, hear,
//! answer, survive a commit, hear again. Everything asserts, so a disagreement
//! between the two stacks exits non-zero rather than printing a warning.
//!
//! # The part worth reading
//!
//! Getting this to pass took corrections to the leaf's key package that neither
//! library signposts. [`leaf_key_package`] documents them, and
//! [`corrections_are_load_bearing`] restores each default in turn and requires
//! the phone to refuse the result. Those negative controls are the point: a
//! correction is a default that someone will eventually "simplify" back, and
//! without a test that fails when they do, the next person to hit these spends
//! their time on a hardware bring-up bench instead of here.
//!
//! Restoring one default at a time rather than all at once is what makes them
//! worth having, and it is also what corrected this file's own account of
//! itself. Two of the three defaults originally listed here are genuinely
//! refused by the phone. The third, a year-long key package lifetime, is not
//! refused by OpenMLS at all: see [`the_lifetime_cap_is_ours_to_apply`]. A control
//! that broke all three at once could not tell the difference, because OpenMLS
//! reports every one of these as `InvalidLifetime`.

use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::OpenMlsProvider;

use mls_rs::client_builder::MlsConfig;
use mls_rs::identity::basic::{BasicCredential, BasicIdentityProvider};
use mls_rs::identity::SigningIdentity;
use mls_rs::mls_rs_codec::MlsEncode;
use mls_rs::time::MlsTime;
use mls_rs::{CipherSuite, CipherSuiteProvider, Client, CryptoProvider, MlsMessage};
use mls_rs_crypto_rustcrypto::RustCryptoProvider;

// The SDK's own declarations, not copies of them. The derivation rule and
// these four numbers are what this harness exists to test the phone against;
// a local copy would leave it green while it stopped testing them.
use offline_protocol_sealed::{
    derive_address, LEAF_KEY_PACKAGE_LIFETIME, LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS,
    MAX_ACCEPTED_KEY_PACKAGE_LIFETIME, SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE,
    SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The SDK pins this in exactly one place and never negotiates it.
const PHONE_SUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
/// The same suite, spelled the way mls-rs spells it.
const LEAF_SUITE: CipherSuite = CipherSuite::CURVE25519_AES128;

/// mls-rs's own default, kept as a named constant because
/// [`corrections_are_load_bearing`] has to be able to restore it.
const MLS_RS_DEFAULT_LIFETIME: Duration = Duration::from_secs(365 * 24 * 3600);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after 1970")
        .as_secs()
}

/// A leaf client and the address its identity key derives to.
fn leaf_client(lifetime: Duration) -> (Client<impl MlsConfig>, String) {
    // Only the pinned suite is enabled. On the device this also keeps three
    // other curves out of the image.
    let crypto = RustCryptoProvider::with_enabled_cipher_suites(vec![LEAF_SUITE]);
    let suite = crypto
        .cipher_suite_provider(LEAF_SUITE)
        .expect("the RustCrypto provider supports ciphersuite 3");
    let (secret, public) = suite
        .signature_key_generate()
        .expect("signature key generation");

    let address = derive_address(public.as_bytes())
        .expect("Ed25519 public keys are 32 bytes")
        .to_string();

    // The credential content *is* the address, which is what the SDK's leaf
    // identity binding requires of every member of every group.
    let credential = BasicCredential::new(address.as_bytes().to_vec());
    let identity = SigningIdentity::new(credential.into_credential(), public);

    let client = Client::builder()
        .identity_provider(BasicIdentityProvider)
        .crypto_provider(crypto)
        .signing_identity(identity, secret, LEAF_SUITE)
        .key_package_lifetime(lifetime)
        .build();

    (client, address)
}

/// The leaf's pairing artifact, with the corrections applied.
///
/// 1. **`not_before` is backdated.** See [`LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS`].
/// 2. **The timestamp is passed in, not read.** A bare-metal leaf has no
///    clock, and mls-rs stamps `not_before = 0` when it cannot read one, which
///    puts the validity window in 1970 and gets the package refused as expired.
///    Passing it explicitly here is not a test convenience: it is the shape the
///    device firmware has to have, and it means **a leaf needs a time source at
///    pairing**, from its radio stack, its commissioner, or the pairing frame
///    itself. That obligation is real and belongs in the ADR.
///
/// The shortened lifetime is a third difference from mls-rs's defaults but not
/// a correction, because the phone accepts the default: see
/// [`LEAF_KEY_PACKAGE_LIFETIME`].
///
/// The framing correction is separate and smaller: the SDK puts a bare
/// `KeyPackage` on the wire, while mls-rs's convenience API returns one wrapped
/// in an `MLSMessage`. Both are legal; they just have to agree.
///
/// `not_before` is a parameter rather than something computed here so that
/// [`corrections_are_load_bearing`] can restore one default at a time. Note in
/// particular that a clockless leaf is reproduced by passing 0, never by
/// passing `None`: this harness runs on a host, so `None` would have mls-rs
/// read a real clock and the 1970 window the device actually emits would never
/// appear.
fn leaf_key_package(client: &Client<impl MlsConfig>, not_before: u64) -> Vec<u8> {
    client
        .generate_key_package_message(
            Default::default(),
            Default::default(),
            Some(MlsTime::from(not_before)),
        )
        .expect("leaf generates a key package")
        .into_key_package()
        .expect("the message is a key package")
        .mls_encode_to_vec()
        .expect("re-encode the key package as bare KeyPackage bytes")
}

/// A phone, configured as `MlsGroupCreateConfig` is configured in the SDK.
fn phone() -> (OpenMlsRustCrypto, SignatureKeyPair, CredentialWithKey) {
    let provider = OpenMlsRustCrypto::default();
    let keys =
        SignatureKeyPair::new(PHONE_SUITE.signature_algorithm()).expect("phone signature keys");
    keys.store(provider.storage()).expect("store phone keys");

    let address = derive_address(keys.public())
        .expect("Ed25519 public keys are 32 bytes")
        .to_string();
    let credential = CredentialWithKey {
        credential: Credential::new(CredentialType::Basic, address.as_bytes().to_vec()),
        signature_key: keys.public().into(),
    };

    (provider, keys, credential)
}

/// Runs the phone's side of key package admission: parse, then validate.
/// What OpenMLS alone will admit: parse and `KeyPackageIn::validate`, nothing
/// else. Kept separate from [`phone_admits`] so a refusal can be attributed to
/// the library rather than to the SDK's policy on top of it.
fn openmls_admits(provider: &OpenMlsRustCrypto, bytes: &[u8]) -> Result<KeyPackage, String> {
    KeyPackageIn::tls_deserialize_exact(bytes)
        .map_err(|e| format!("parse: {e}"))?
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .map_err(|e| format!("{e:?}"))
}

/// What `MlsManager::import_key_package` will admit: everything OpenMLS checks,
/// plus the validity-window cap RFC 9420 puts on the application and the SDK
/// applies in `verify_lifetime_bound`.
///
/// The cap is here rather than in [`openmls_admits`] because it is the SDK's,
/// and a harness that modelled the phone as bare OpenMLS would go on reporting
/// that a year-long package is accepted long after it stopped being.
fn phone_admits(provider: &OpenMlsRustCrypto, bytes: &[u8]) -> Result<KeyPackage, String> {
    let key_package = openmls_admits(provider, bytes)?;

    let window = key_package.life_time();
    let width = window.not_after().saturating_sub(window.not_before());
    if width > MAX_ACCEPTED_KEY_PACKAGE_LIFETIME.as_secs() {
        return Err(format!(
            "validity window is {width} seconds, wider than the {} the SDK admits",
            MAX_ACCEPTED_KEY_PACKAGE_LIFETIME.as_secs()
        ));
    }
    Ok(key_package)
}

/// The negative controls: one per correction, each restoring a single default.
///
/// One at a time is the whole design. With several defaults wrong at once a
/// package is refused for whichever reason OpenMLS happens to check first, and
/// a control like that cannot say which correction is doing the work. Splitting
/// them is what turned up that one of the three originally claimed here is not
/// enforced by the phone at all: see
/// [`the_lifetime_cap_is_ours_to_apply`].
///
/// If either case below is ACCEPTED, one of the two stacks changed its validity
/// rules and whether that correction is still load-bearing has to be re-derived
/// rather than discovered by someone deleting it.
fn corrections_are_load_bearing() {
    let (provider, _, _) = phone();
    let now = unix_now();

    // Both cases are the same OpenMLS rule: `not_before < now`, tested strictly
    // in `Lifetime::is_valid`. The first stamps `not_before` an hour into the
    // future rather than at exactly `now`, which is what dropping the backdate
    // literally produces. That is deliberate: `not_before == now` is refused
    // only when the phone validates inside the same second the package was
    // minted, so a control built on it would be a coin flip. An hour of skew
    // exercises the same rule deterministically, and skew between two devices
    // is the form this failure takes in the field anyway.
    let cases: [(&str, &str, u64); 2] = [
        (
            "a leaf whose clock leads the phone's",
            "LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS",
            now + LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS,
        ),
        (
            "a leaf with no clock at all, stamping 1970",
            "the supplied pairing timestamp",
            0,
        ),
    ];

    for (index, (what, correction, not_before)) in cases.iter().enumerate() {
        let (client, _) = leaf_client(LEAF_KEY_PACKAGE_LIFETIME);
        let bytes = leaf_key_package(&client, *not_before);

        match openmls_admits(&provider, &bytes) {
            Err(e) => println!("  0.{} {what} is refused ({e})", index + 1),
            Ok(_) => panic!(
                "the phone ACCEPTED a key package from {what}.\n\
                 OpenMLS or mls-rs changed its key package validity rules, so \
                 {correction} may no longer be load-bearing. Re-derive that \
                 before touching it."
            ),
        }
    }
}

/// The third default: refused, but by the SDK rather than by the library.
///
/// RFC 9420 tells an application to define a maximum total lifetime for a leaf
/// node and reject anything longer, and OpenMLS 0.7.4 ships both halves of
/// that: the constant `MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS` (an hour plus
/// three months) and the predicate `Lifetime::has_acceptable_range`. Nothing
/// calls the predicate. `KeyPackageIn::validate` checks only
/// `Lifetime::is_valid`, which is the `not_before < now < not_after` window,
/// and returns `InvalidLifetime` when it fails, which is the error name that
/// made a year-long lifetime look like it was being refused for its range when
/// it was really being refused for its `not_before`.
///
/// That left the SDK admitting a key package with an arbitrarily long window
/// from any peer, which was issue 396. The cap is now
/// [`MAX_ACCEPTED_KEY_PACKAGE_LIFETIME`], applied in
/// `MlsManager::verify_lifetime_bound` and modelled here by [`phone_admits`].
///
/// Both halves are asserted, because they fail for different reasons and only
/// one of them is ours:
///
/// 1. **OpenMLS still accepts it.** If that changes, the library wired up its
///    own cap, and where the SDK's refusal comes from is no longer what this
///    says it is.
/// 2. **The SDK refuses it.** If that changes, the cap has been removed or
///    widened past a year and issue 396 is open again.
fn the_lifetime_cap_is_ours_to_apply() {
    let (provider, _, _) = phone();
    let (client, _) = leaf_client(MLS_RS_DEFAULT_LIFETIME);
    let bytes = leaf_key_package(&client, unix_now() - LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS);

    if let Err(e) = openmls_admits(&provider, &bytes) {
        panic!(
            "OpenMLS REFUSED a year-long key package lifetime ({e}).\n\
             It now enforces MAX_LEAF_NODE_LIFETIME_RANGE_SECONDS itself, so the \
             SDK's cap is no longer what refuses this and ADR 0021's account of \
             where the bound comes from needs updating."
        );
    }

    match phone_admits(&provider, &bytes) {
        Err(e) => println!(
            "  0.3 mls-rs's one-year lifetime is refused by the SDK's cap ({e}); \
             OpenMLS alone accepts it"
        ),
        Ok(_) => panic!(
            "the phone ACCEPTED a year-long key package lifetime.\n\
             MAX_ACCEPTED_KEY_PACKAGE_LIFETIME is not being applied at import, \
             which is issue 396 reopened."
        ),
    }
}

fn step(n: u32, what: &str) {
    println!("  {n}. {what}");
}

fn main() {
    println!("OpenMLS 0.7.4 (phone) <-> mls-rs 0.56.0 (leaf), ciphersuite 3\n");

    corrections_are_load_bearing();
    the_lifetime_cap_is_ours_to_apply();

    // ---- The leaf builds an identity and a pairing artifact ---------------
    let (leaf, leaf_address) = leaf_client(LEAF_KEY_PACKAGE_LIFETIME);
    step(1, &format!("leaf identity derives to {leaf_address}"));

    let key_package_bytes = leaf_key_package(&leaf, unix_now() - LEAF_KEY_PACKAGE_NOT_BEFORE_BACKDATE_SECONDS);
    step(
        2,
        &format!("leaf key package, {} bytes", key_package_bytes.len()),
    );

    // ---- The phone admits it, exactly as `import_key_package` does --------
    let (provider, phone_keys, phone_credential) = phone();
    let leaf_key_package = phone_admits(&provider, &key_package_bytes)
        .expect("phone parses and validates the leaf's key package");
    step(3, "phone parsed and validated it");

    // `MlsManager::verify_address_binding` derives from the signature key in the
    // package that *arrived*, not from one the process already holds, and this
    // check is written the same way for the same reason. Comparing a credential
    // against the locally-generated address would hold by construction: it
    // would pass even if mls-rs and OpenMLS disagreed about where the signature
    // key sits in the encoding, which is exactly the disagreement an interop
    // harness exists to catch.
    let presented_key = leaf_key_package.leaf_node().signature_key().as_slice();
    let claimed = std::str::from_utf8(
        leaf_key_package
            .leaf_node()
            .credential()
            .serialized_content(),
    )
    .expect("utf8 credential");

    assert_eq!(
        derive_address(presented_key)
            .expect("a leaf node credential carries a 32-byte Ed25519 key")
            .to_string(),
        claimed,
        "derive(presented_key) must equal the address the credential claims"
    );
    assert_eq!(
        claimed, leaf_address,
        "and the address the phone recovered must be the leaf's own"
    );
    step(
        4,
        "derive(presented_key) == claimed address, on the phone's copy",
    );

    // ---- The phone creates the group and adds the leaf -------------------
    let group_config = MlsGroupCreateConfig::builder()
        .ciphersuite(PHONE_SUITE)
        .use_ratchet_tree_extension(true)
        .sender_ratchet_configuration(SenderRatchetConfiguration::new(
            SENDER_RATCHET_OUT_OF_ORDER_TOLERANCE,
            SENDER_RATCHET_MAXIMUM_FORWARD_DISTANCE,
        ))
        .build();

    let mut phone_group = MlsGroup::new(&provider, &phone_keys, &group_config, phone_credential)
        .expect("phone creates the group");

    let (_commit, welcome, _group_info) = phone_group
        .add_members(&provider, &phone_keys, &[leaf_key_package])
        .expect("phone commits the Add");
    phone_group
        .merge_pending_commit(&provider)
        .expect("phone merges its own commit");
    step(5, "phone created the group and committed the Add");

    // ---- The leaf joins from the Welcome ---------------------------------
    let welcome_bytes = welcome.tls_serialize_detached().expect("serialize welcome");
    let welcome_message =
        MlsMessage::from_bytes(&welcome_bytes).expect("leaf parses the phone's Welcome");

    // `tree_data` is None on purpose: the SDK sets `use_ratchet_tree_extension`,
    // so the tree rides inside the Welcome. A device that had to fetch the tree
    // out of band would need a side channel it does not have.
    let (mut leaf_group, _info) = leaf
        .join_group(None, &welcome_message, None)
        .expect("leaf joins from the Welcome");
    step(6, "leaf joined from the Welcome with no out-of-band tree");

    assert_eq!(
        leaf_group.current_epoch(),
        phone_group.epoch().as_u64(),
        "both sides must agree on the epoch after the join"
    );

    // ---- Phone speaks, leaf hears ----------------------------------------
    let phone_msg = phone_group
        .create_message(&provider, &phone_keys, b"unlock the door")
        .expect("phone encrypts")
        .tls_serialize_detached()
        .expect("serialize");

    match leaf_group
        .process_incoming_message(MlsMessage::from_bytes(&phone_msg).expect("leaf parses"))
        .expect("leaf decrypts the phone's message")
    {
        mls_rs::group::ReceivedMessage::ApplicationMessage(app) => {
            assert_eq!(app.data(), b"unlock the door");
            step(7, "leaf decrypted the phone's application message");
        }
        other => panic!("expected an application message, got {other:?}"),
    }

    // ---- Leaf answers, phone hears ---------------------------------------
    let leaf_answer = leaf_group
        .encrypt_application_message(b"unlocked", Vec::new())
        .expect("leaf encrypts its answer")
        .to_bytes()
        .expect("serialize the answer");

    let inbound = MlsMessageIn::tls_deserialize_exact(&leaf_answer)
        .expect("phone parses the leaf's answer")
        .try_into_protocol_message()
        .expect("the answer is a protocol message");

    match phone_group
        .process_message(&provider, inbound)
        .expect("phone decrypts the leaf's answer")
        .into_content()
    {
        ProcessedMessageContent::ApplicationMessage(app) => {
            assert_eq!(app.into_bytes(), b"unlocked");
            step(8, "phone decrypted the leaf's answer");
        }
        _ => panic!("expected an application message from the leaf"),
    }

    // ---- The phone commits; the leaf has to survive it -------------------
    let (commit, _welcome, _gi) = phone_group
        .self_update(&provider, &phone_keys, LeafNodeParameters::default())
        .expect("phone self-updates")
        .into_contents();
    phone_group
        .merge_pending_commit(&provider)
        .expect("phone merges the update");

    let commit_bytes = commit
        .tls_serialize_detached()
        .expect("serialize the commit");
    match leaf_group
        .process_incoming_message(MlsMessage::from_bytes(&commit_bytes).expect("leaf parses"))
        .expect("leaf processes the phone's commit")
    {
        mls_rs::group::ReceivedMessage::Commit(_) => step(9, "leaf processed the phone's commit"),
        other => panic!("expected a commit, got {other:?}"),
    }

    assert_eq!(
        leaf_group.current_epoch(),
        phone_group.epoch().as_u64(),
        "both sides must agree on the epoch after the commit"
    );

    // ---- And still hears in the new epoch --------------------------------
    let after = phone_group
        .create_message(&provider, &phone_keys, b"post-commit")
        .expect("phone encrypts in the new epoch")
        .tls_serialize_detached()
        .expect("serialize");

    match leaf_group
        .process_incoming_message(MlsMessage::from_bytes(&after).expect("leaf parses"))
        .expect("leaf decrypts in the new epoch")
    {
        mls_rs::group::ReceivedMessage::ApplicationMessage(app) => {
            assert_eq!(app.data(), b"post-commit");
            step(10, "leaf decrypted in the new epoch");
        }
        other => panic!("expected an application message, got {other:?}"),
    }

    println!("\nPASS: the leaf stack interoperates with the phone's, both directions,");
    println!("across a commit, with the SDK's ciphersuite and credential shape.");
}
