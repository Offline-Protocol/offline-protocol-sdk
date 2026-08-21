//! `DiscoveryRecordV1`: a signed, non-authoritative claim that a username
//! points at an address.
//!
//! # A claim, never a fact
//!
//! Anyone may publish any username claim at the right tag. There is no
//! first-publisher-wins on a Nostr relay and this layer does not invent one.
//! Every record says exactly one thing: *this key asserts this name*. What a
//! record cannot do is lie about a **key**, because
//! `derive_address(pubkey) == address` is checked on every one — so the worst a
//! squatter achieves is to offer you a name attached to their own real identity,
//! which is precisely NIP-05's threat model and precisely why its "identify,
//! never verify" semantics are the right ones to adopt.
//!
//! The consequence is normative and it belongs at the top of this file: a
//! resolver MUST surface the whole set of claimants and let a human arbitrate.
//! A UI that silently picks the first result has converted a deliberately
//! non-authoritative directory into an authoritative-looking one, which is worse
//! than not shipping it at all. See `docs/spec/username-discovery.md`.
//!
//! # Why a username resolves to a set, always
//!
//! One record per device. Each install signs its own record with its own
//! identity key and publishes it under its own Nostr key, so all of a user's
//! devices coexist at the same tag (addressable replacement is keyed on
//! `(kind, pubkey, d)`, and each device is a different `pubkey`). A resolver
//! queries once and receives the whole set.
//!
//! This is not a limitation to be designed around: no device knows the
//! addresses of its siblings — there is no identity export, import, backup or
//! sync API anywhere in this SDK, by construction — so a record shaped as
//! `{username, [devices]}` could not be produced at all. Aggregating at the tag
//! reaches the identical result with zero coordination. A username is therefore
//! 1:N even for a single-device user, who is simply a set of one.
//!
//! # The `nostr_author` binding
//!
//! Step 5 of verification is what this record has and the published key-package
//! record does not, and it closes a residual that is materially worse for a
//! directory than for a key package.
//!
//! Because the seal key is publicly derivable, a squatter can unseal a record,
//! re-seal the untouched (and genuinely signed) payload under their own author
//! key, and republish it. For a key package the cost is a dead session and one
//! exchange of delay. For a directory entry it defeats **retraction**:
//! addressable replacement is per-`pubkey`, so the owner's tombstone replaces
//! only the owner's own event and never a copy standing under someone else's
//! key. That would keep a rotated-away or compromised address in the directory
//! indefinitely. Binding the author key inside the signed payload makes a
//! re-authored copy fail outright.

use offline_protocol_core::{Address, Username};
use serde::{Deserialize, Serialize};

use crate::error::{MlsError, Result};
use crate::manager::MlsManager;
use offline_protocol_sealed::canonical_payload;

/// Signature domain for discovery records.
///
/// Must not be a prefix of, or prefixed by, any other live signing domain. See
/// `offline_protocol_sealed::canonical`.
pub const DISCOVERY_SIGN_DOMAIN: &[u8] = b"offline-disc-v1";

/// The only discovery record version this build produces or accepts.
pub const DISCOVERY_VERSION: u8 = 1;

/// Length of an Ed25519 public key.
const PUBLIC_KEY_LEN: usize = 32;

/// Length of an Ed25519 signature.
const SIGNATURE_LEN: usize = 64;

/// Length of a Nostr x-only public key.
const NOSTR_KEY_LEN: usize = 32;

/// A signed claim that a username points at an address.
///
/// Serialized as JSON inside a sealed relay record. The byte-level contract is
/// [`Self::signing_payload`], not this struct's JSON: two implementations must
/// agree on the signed bytes, and may disagree on JSON key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryRecordV1 {
    /// Format version. Always [`DISCOVERY_VERSION`] in this build.
    pub v: u8,
    /// The claimed name, in normalized form.
    ///
    /// Typed, so a record naming an unnormalized username fails to deserialize
    /// rather than being compared against a queried name it can never equal.
    pub username: Username,
    /// The address the claim points at.
    pub address: Address,
    /// The Ed25519 identity key. `derive_address(pubkey)` must equal `address`.
    #[serde(with = "base64_bytes")]
    pub pubkey: Vec<u8>,
    /// The x-only Nostr key this record is valid when published under.
    ///
    /// See the module docs: this is what stops a third party from keeping a
    /// retracted claim alive.
    #[serde(with = "hex_bytes")]
    pub nostr_author: Vec<u8>,
    /// Signing time, in milliseconds since the Unix epoch.
    ///
    /// **Advisory only.** See [`verify_discovery_record`].
    pub issued_at_ms: i64,
    /// Ed25519 signature by `pubkey` over [`Self::signing_payload`].
    #[serde(with = "base64_bytes")]
    pub sig: Vec<u8>,
}

/// A tombstone: the body a retraction publishes in place of a claim.
///
/// Retraction republishes the same `(kind, pubkey, d)` with this body and a
/// fresh `created_at`, plus a best-effort NIP-09 deletion. It is best-effort by
/// nature — a relay may honour neither — which is one more reason staleness is
/// advisory and the key-package fetch arbitrates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryTombstoneV1 {
    /// Format version. Always [`DISCOVERY_VERSION`].
    pub v: u8,
    /// Always `true`. Present so a tombstone cannot be mistaken for a record
    /// whose fields failed to parse.
    pub retracted: bool,
}

impl DiscoveryTombstoneV1 {
    /// Builds a tombstone body.
    pub fn new() -> Self {
        Self {
            v: DISCOVERY_VERSION,
            retracted: true,
        }
    }
}

impl Default for DiscoveryTombstoneV1 {
    fn default() -> Self {
        Self::new()
    }
}

/// What a resolved record body turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryBody {
    /// A claim, not yet verified.
    Record(Box<DiscoveryRecordV1>),
    /// A retraction. The claimant is withdrawing the name.
    Tombstone,
}

/// Parses a record body, distinguishing a claim from a retraction.
///
/// A body that is neither is an error, which the caller drops: a public tag
/// returns whatever the relay holds there, so junk is ordinary rather than
/// exceptional.
pub fn parse_discovery_body(bytes: &[u8]) -> Result<DiscoveryBody> {
    // Tombstone first: it is the narrower shape, and a record can never satisfy
    // it because `retracted` is required and has no counterpart in a claim.
    if let Ok(tombstone) = serde_json::from_slice::<DiscoveryTombstoneV1>(bytes) {
        if tombstone.retracted {
            return Ok(DiscoveryBody::Tombstone);
        }
    }
    let record: DiscoveryRecordV1 = serde_json::from_slice(bytes)
        .map_err(|e| MlsError::Deserialization(format!("Discovery record is malformed: {}", e)))?;
    Ok(DiscoveryBody::Record(Box::new(record)))
}

impl DiscoveryRecordV1 {
    /// Builds the payload the signature is taken over.
    ///
    /// `domain ‖ u32be‖bytes` over
    /// `[v, username, address, pubkey, nostr_author, issued_at_ms]`, in that
    /// fixed order. `issued_at_ms` is encoded as its 8-byte big-endian form,
    /// not as a decimal string, so two implementations cannot disagree about
    /// leading zeroes or a sign.
    pub fn signing_payload(
        username: &Username,
        address: &Address,
        pubkey: &[u8],
        nostr_author: &[u8],
        issued_at_ms: i64,
    ) -> Result<Vec<u8>> {
        let address_string = address.to_string();
        canonical_payload(
            DISCOVERY_SIGN_DOMAIN,
            &[
                &[DISCOVERY_VERSION],
                username.as_str().as_bytes(),
                address_string.as_bytes(),
                pubkey,
                nostr_author,
                &issued_at_ms.to_be_bytes(),
            ],
        )
        .map_err(MlsError::from)
    }

    /// Builds an unsigned record, ready for [`Self::sign_with`].
    pub fn unsigned(
        username: Username,
        address: Address,
        pubkey: Vec<u8>,
        nostr_author: Vec<u8>,
        issued_at_ms: i64,
    ) -> Self {
        Self {
            v: DISCOVERY_VERSION,
            username,
            address,
            pubkey,
            nostr_author,
            issued_at_ms,
            sig: Vec::new(),
        }
    }

    /// Signs the record with `signer`, which must be the private half of
    /// `self.pubkey`.
    ///
    /// Taking a closure keeps the private key where it lives (the engine's MLS
    /// manager) rather than passing key material into this module.
    pub fn sign_with<F>(mut self, signer: F) -> Result<Self>
    where
        F: FnOnce(&[u8]) -> Result<Vec<u8>>,
    {
        let payload = Self::signing_payload(
            &self.username,
            &self.address,
            &self.pubkey,
            &self.nostr_author,
            self.issued_at_ms,
        )?;
        self.sig = signer(&payload)?;
        Ok(self)
    }
}

/// Why a discovery record was rejected.
///
/// Separate from [`MlsError`] because every variant here is an *ordinary*
/// outcome of querying a public tag, not a fault: a resolver drops the record
/// and keeps going. Carrying them as errors would make junk at a public tag
/// look like a failure of the system reading it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryRejection {
    /// The version is not one this build understands.
    #[error("discovery record version is {found}, expected {expected}")]
    UnsupportedVersion {
        /// The version the record carried.
        found: u8,
        /// The only version this build accepts.
        expected: u8,
    },
    /// A fixed-length field is the wrong size.
    #[error("discovery record field '{field}' is {len} bytes, expected {expected}")]
    FieldLength {
        /// Which field.
        field: &'static str,
        /// Length found.
        len: usize,
        /// Length required.
        expected: usize,
    },
    /// The record claims a different name than the one queried.
    ///
    /// A record is only meaningful at its own tag, so this also catches a
    /// record that was mis-tagged or copied to a foreign tag.
    #[error("discovery record claims '{claimed}' but '{queried}' was queried")]
    UsernameMismatch {
        /// The name the record claims.
        claimed: String,
        /// The name that was resolved.
        queried: String,
    },
    /// `derive_address(pubkey)` is not the claimed address.
    #[error("discovery record address is not the address its public key derives to")]
    AddressNotDerived,
    /// The signature does not verify under `pubkey`.
    #[error("discovery record signature does not verify")]
    BadSignature,
    /// The record was published under a different Nostr key than it names.
    ///
    /// This is a re-authored copy: someone unsealed a genuine record and stood
    /// it back up under their own key. See the module docs.
    #[error("discovery record was published under a foreign Nostr key")]
    ForeignAuthor,
}

/// Verifies a discovery record against the username it was queried for and the
/// Nostr key it was published under.
///
/// The checks run in the order the design fixes, cheap before expensive:
///
/// 1. `v == 1` and every fixed-length field is the right size;
/// 2. the username matches the queried name exactly;
/// 3. `derive_address(pubkey) == address`;
/// 4. the Ed25519 signature verifies under `pubkey`;
/// 5. `nostr_author == event.pubkey`.
///
/// # Staleness is advisory and is deliberately not checked here
///
/// A record is not a liveness signal; the key-package fetch that follows it is.
/// A stale record whose key packages are gone fails at that fetch, which is the
/// honest place to fail. Rejecting on age would instead make a peer who has
/// been offline for a month unreachable *by name* while their key packages sit
/// valid on a relay for 30 days. Surface `issued_at_ms` to the app, let it sort,
/// and let the fetch arbitrate.
pub fn verify_discovery_record(
    record: &DiscoveryRecordV1,
    queried: &Username,
    event_author: &[u8],
) -> std::result::Result<(), DiscoveryRejection> {
    if record.v != DISCOVERY_VERSION {
        return Err(DiscoveryRejection::UnsupportedVersion {
            found: record.v,
            expected: DISCOVERY_VERSION,
        });
    }
    if record.pubkey.len() != PUBLIC_KEY_LEN {
        return Err(DiscoveryRejection::FieldLength {
            field: "pubkey",
            len: record.pubkey.len(),
            expected: PUBLIC_KEY_LEN,
        });
    }
    if record.nostr_author.len() != NOSTR_KEY_LEN {
        return Err(DiscoveryRejection::FieldLength {
            field: "nostr_author",
            len: record.nostr_author.len(),
            expected: NOSTR_KEY_LEN,
        });
    }
    if record.sig.len() != SIGNATURE_LEN {
        return Err(DiscoveryRejection::FieldLength {
            field: "sig",
            len: record.sig.len(),
            expected: SIGNATURE_LEN,
        });
    }

    if record.username != *queried {
        return Err(DiscoveryRejection::UsernameMismatch {
            claimed: record.username.as_str().to_string(),
            queried: queried.as_str().to_string(),
        });
    }

    let derived = MlsManager::derive_address(&record.pubkey)
        .map_err(|_| DiscoveryRejection::AddressNotDerived)?;
    if derived != record.address {
        return Err(DiscoveryRejection::AddressNotDerived);
    }

    let payload = DiscoveryRecordV1::signing_payload(
        &record.username,
        &record.address,
        &record.pubkey,
        &record.nostr_author,
        record.issued_at_ms,
    )
    .map_err(|_| DiscoveryRejection::BadSignature)?;

    match MlsManager::verify_signature(&record.pubkey, &payload, &record.sig) {
        Ok(true) => {}
        _ => return Err(DiscoveryRejection::BadSignature),
    }

    // Last, because it is the check an honest record passes trivially and a
    // re-authored one fails: everything above verifies against the *record*,
    // and this is the only step that involves the event carrying it.
    if record.nostr_author != event_author {
        return Err(DiscoveryRejection::ForeignAuthor);
    }

    Ok(())
}

/// Base64 for byte fields that are naturally binary.
mod base64_bytes {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(&encoded).map_err(serde::de::Error::custom)
    }
}

/// Hex for the Nostr author key, which is hex everywhere else in the Nostr
/// layer — matching it here means a comparison never crosses an encoding.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        hex::decode(&encoded).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};

    fn identity(seed: u8) -> (SigningKey, Vec<u8>, Address) {
        let signing = SigningKey::from_bytes(&[seed; 32]);
        let public = signing.verifying_key().to_bytes().to_vec();
        let address = MlsManager::derive_address(&public).expect("derive");
        (signing, public, address)
    }

    fn username(s: &str) -> Username {
        s.parse().expect("username")
    }

    fn signed_record(seed: u8, name: &str, author: &[u8]) -> DiscoveryRecordV1 {
        let (signing, public, address) = identity(seed);
        DiscoveryRecordV1::unsigned(
            username(name),
            address,
            public,
            author.to_vec(),
            1_700_000_000_000,
        )
        .sign_with(|payload| Ok(signing.sign(payload).to_bytes().to_vec()))
        .expect("sign")
    }

    fn author(seed: u8) -> Vec<u8> {
        vec![seed; NOSTR_KEY_LEN]
    }

    #[test]
    fn discovery_record_verifies() {
        let author = author(9);
        let record = signed_record(1, "alice", &author);
        assert_eq!(
            verify_discovery_record(&record, &username("alice"), &author),
            Ok(())
        );
    }

    #[test]
    fn discovery_record_round_trips_through_json() {
        let author = author(9);
        let record = signed_record(1, "alice", &author);
        let json = serde_json::to_vec(&record).expect("serialize");
        match parse_discovery_body(&json).expect("parse") {
            DiscoveryBody::Record(parsed) => assert_eq!(*parsed, record),
            DiscoveryBody::Tombstone => panic!("a record must not parse as a tombstone"),
        }
    }

    #[test]
    fn discovery_tombstone_parses_as_a_retraction() {
        let json = serde_json::to_vec(&DiscoveryTombstoneV1::new()).expect("serialize");
        assert_eq!(
            parse_discovery_body(&json).expect("parse"),
            DiscoveryBody::Tombstone
        );
    }

    /// The negative control for the `nostr_author` binding, and the single most
    /// important test in this module: a squatter unseals a genuine record and
    /// republishes it verbatim under their own key. Everything inside the
    /// record still verifies — the signature is real — and the record must
    /// still be refused.
    #[test]
    fn discovery_record_refuses_a_re_authored_copy() {
        let genuine_author = author(9);
        let record = signed_record(1, "alice", &genuine_author);

        // Untouched payload, republished under the squatter's key.
        let squatter_author = author(200);
        assert_eq!(
            verify_discovery_record(&record, &username("alice"), &squatter_author),
            Err(DiscoveryRejection::ForeignAuthor)
        );
    }

    /// The squatter's other option — re-sign the record naming their own author
    /// key — must fail too, because they do not hold the identity key.
    #[test]
    fn discovery_record_refuses_a_rebound_author_without_the_identity_key() {
        let record = signed_record(1, "alice", &author(9));
        let mut rebound = record.clone();
        rebound.nostr_author = author(200);
        assert_eq!(
            verify_discovery_record(&rebound, &username("alice"), &author(200)),
            Err(DiscoveryRejection::BadSignature)
        );
    }

    #[test]
    fn discovery_record_refuses_a_foreign_address() {
        let author = author(9);
        let mut record = signed_record(1, "alice", &author);
        let (_, _, other) = identity(2);
        record.address = other;
        assert_eq!(
            verify_discovery_record(&record, &username("alice"), &author),
            Err(DiscoveryRejection::AddressNotDerived)
        );
    }

    /// A record is only meaningful at its own tag. This is what catches a
    /// genuine record for `bob` copied to `alice`'s tag.
    #[test]
    fn discovery_record_refuses_a_username_it_was_not_queried_for() {
        let author = author(9);
        let record = signed_record(1, "bob", &author);
        assert_eq!(
            verify_discovery_record(&record, &username("alice"), &author),
            Err(DiscoveryRejection::UsernameMismatch {
                claimed: "bob".to_string(),
                queried: "alice".to_string(),
            })
        );
    }

    #[test]
    fn discovery_record_refuses_a_tampered_username() {
        let author = author(9);
        let mut record = signed_record(1, "alice", &author);
        record.username = username("mallory");
        assert_eq!(
            verify_discovery_record(&record, &username("mallory"), &author),
            Err(DiscoveryRejection::BadSignature)
        );
    }

    #[test]
    fn discovery_record_refuses_a_tampered_issue_time() {
        let author = author(9);
        let mut record = signed_record(1, "alice", &author);
        record.issued_at_ms += 1;
        assert_eq!(
            verify_discovery_record(&record, &username("alice"), &author),
            Err(DiscoveryRejection::BadSignature)
        );
    }

    #[test]
    fn discovery_record_refuses_an_unsupported_version() {
        let author = author(9);
        let mut record = signed_record(1, "alice", &author);
        record.v = 2;
        assert!(matches!(
            verify_discovery_record(&record, &username("alice"), &author),
            Err(DiscoveryRejection::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn discovery_record_refuses_wrong_length_fields() {
        let author = author(9);
        for mutate in [
            (|r: &mut DiscoveryRecordV1| r.pubkey.truncate(31)) as fn(&mut DiscoveryRecordV1),
            |r: &mut DiscoveryRecordV1| r.nostr_author.truncate(31),
            |r: &mut DiscoveryRecordV1| r.sig.truncate(63),
        ] {
            let mut record = signed_record(1, "alice", &author);
            mutate(&mut record);
            assert!(matches!(
                verify_discovery_record(&record, &username("alice"), &author),
                Err(DiscoveryRejection::FieldLength { .. })
            ));
        }
    }

    /// Staleness must not reject. A record signed long ago still verifies; the
    /// key-package fetch is what decides whether the peer is reachable.
    #[test]
    fn discovery_record_accepts_an_old_issue_time() {
        let author = author(9);
        let (signing, public, address) = identity(1);
        let ancient = DiscoveryRecordV1::unsigned(
            username("alice"),
            address,
            public,
            author.clone(),
            1_000_000_000_000,
        )
        .sign_with(|payload| Ok(signing.sign(payload).to_bytes().to_vec()))
        .expect("sign");

        assert_eq!(
            verify_discovery_record(&ancient, &username("alice"), &author),
            Ok(())
        );
    }

    /// A record naming an unnormalized username must not deserialize: the
    /// repaired form would verify against a tag the record was never published
    /// at, which is the silent-miss failure the `Username` type exists to stop.
    #[test]
    fn discovery_record_refuses_an_unnormalized_username_on_the_wire() {
        let author = author(9);
        let record = signed_record(1, "alice", &author);
        let mut value: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&record).expect("serialize"))
                .expect("value");
        value["username"] = serde_json::Value::String("Alice".to_string());
        let json = serde_json::to_vec(&value).expect("reserialize");
        assert!(parse_discovery_body(&json).is_err());
    }

    /// Golden vector for the signed bytes. A second implementation must produce
    /// this payload exactly, or the two sign different things and every
    /// cross-implementation verification fails — silently, since a bad
    /// signature is indistinguishable from a squatted record.
    ///
    /// **Computed independently of this code** by a Python script that builds
    /// the payload from the written construction and derives the address with
    /// the BIP-350 reference bech32m. Regenerate it the same way.
    ///
    /// The identity is the all-`0x01` Ed25519 seed and the author key is 32
    /// bytes of `0x09`, so a second implementation can reproduce it without
    /// this repository.
    #[test]
    fn discovery_signing_payload_golden_vector() {
        let (_, public, address) = identity(1);
        assert_eq!(
            address.to_string(),
            "off1qy682ruch4vlely5dkj94247jva7z49yk5xpqee0",
            "the golden identity's address changed"
        );

        let payload = DiscoveryRecordV1::signing_payload(
            &username("alice"),
            &address,
            &public,
            &author(9),
            1_700_000_000_000,
        )
        .expect("payload");

        assert_eq!(
            base64::engine::general_purpose::STANDARD.encode(&payload),
            "b2ZmbGluZS1kaXNjLXYxAAAAAQEAAAAFYWxpY2UAAAAsb2ZmMXF5NjgycnVjaDR2bGVseTVka2o5NDI0N2p2YTd6NDl5azV4cHFlZTAAAAAgiojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1wAAAAgCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkAAAAIAAABi8/laAA="
        );
    }
}
