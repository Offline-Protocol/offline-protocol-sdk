//! Stable test identities: readable labels that stand for derived addresses.
//!
//! Identities are no longer chosen, so a fixture cannot simply *be* `"alice"`.
//! What it can do is hold the same identity every time it says `"alice"`, and
//! that is what this module provides: a label maps to one identity key for the
//! lifetime of the test process, and therefore to one address.
//!
//! Tests keep writing `"alice"` and `"bob"`. Where the string is a *wire*
//! identity — a recipient, an expected `sender`, a group member — they wrap it
//! in [`id`], which resolves the label to the address that label's protocol
//! instance will actually run as. Where the string is anything else (a group
//! name, a message body, a storage key for an unrelated record) it stays a
//! plain string, because nothing derives those.
//!
//! The mapping is process-local and generated, not pinned: no wire format
//! depends on it, and pinning it would only invite a fixture to hardcode an
//! address instead of asking for one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::mls::InMemoryStorage;
use offline_protocol_mls::{MlsManager, MlsStorage};

/// Everything derived from a label, computed once.
#[derive(Clone)]
struct TestIdentity {
    /// The serialized `("identity", "key_pair")` record.
    record: Vec<u8>,
    /// The Ed25519 signing key, for standing in as this peer's device.
    signing: ed25519_dalek::SigningKey,
    /// The address the record derives to.
    address: String,
}

/// Label → identity, and address → label.
///
/// Both directions are memoized because both are hot. Deriving an address costs
/// a storage round-trip plus a hash, and `id(..)` is called thousands of times
/// per run; resolving a *sender* back to its label happens once per signed test
/// frame, and scanning every label to find it would make that quadratic.
type IdentityCaches = (HashMap<String, TestIdentity>, HashMap<String, String>);

fn caches() -> &'static Mutex<IdentityCaches> {
    static CACHES: OnceLock<Mutex<IdentityCaches>> = OnceLock::new();
    CACHES.get_or_init(|| Mutex::new((HashMap::new(), HashMap::new())))
}

/// The cache guard, recovered rather than re-panicked if a poisoned lock is
/// found.
///
/// A test that fails while holding this lock would otherwise poison it for the
/// whole process and turn one real failure into hundreds of unrelated ones,
/// burying the diagnosis. Recovery is sound here because the contents are pure
/// memoized derivations of the label — there is no partially-updated state a
/// panic could leave behind that is unsafe to read.
fn cache_guard() -> std::sync::MutexGuard<'static, IdentityCaches> {
    caches().lock().unwrap_or_else(|e| e.into_inner())
}

/// The identity this label runs as.
///
/// Seeded from the label rather than randomly generated, and that is
/// load-bearing rather than tidiness: addresses sort by key hash, and several
/// behaviours under test are decided by that sort (leave election, admin
/// auto-promotion, fork leader). A randomly minted key would reshuffle the
/// order on every run and make those tests pass or fail by luck.
fn identity(label: &str) -> TestIdentity {
    use sha2::{Digest, Sha256};

    let mut guard = cache_guard();
    if let Some(found) = guard.0.get(label) {
        return found.clone();
    }

    let seed: [u8; 32] = Sha256::digest(label.as_bytes()).into();
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let public = signing.verifying_key().to_bytes().to_vec();
    let keys = openmls_basic_credential::SignatureKeyPair::from_raw(
        openmls_traits::types::SignatureScheme::ED25519,
        signing.to_bytes().to_vec(),
        public.clone(),
    );
    let record = serde_json::to_vec(&keys).expect("serializing a test identity");
    // The same derivation `load_or_create_identity` would perform, without the
    // storage round-trip. Pinned against that path by
    // `test_identity_address_matches_the_bootstrap_derivation`.
    let address = MlsManager::derive_address(&public)
        .expect("deriving a test address")
        .to_string();

    let entry = TestIdentity {
        record,
        signing,
        address: address.clone(),
    };
    guard.0.insert(label.to_string(), entry.clone());
    guard.1.insert(address, label.to_string());
    entry
}

/// The raw `("identity", "key_pair")` record this label runs as.
fn identity_record(label: &str) -> Vec<u8> {
    identity(label).record
}

/// Installs `label`'s identity into `storage`, so a protocol initialized
/// against it comes up as [`id(label)`].
///
/// Does nothing if the storage already holds an identity: a fixture that
/// reinitializes the same storage — a restart — must keep the identity it had,
/// which is the property those tests are usually there to check.
pub(crate) fn seed_identity(storage: &Arc<dyn MlsStorage>, label: &str) {
    if storage
        .load("identity", "key_pair")
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    let record = identity_record(label);
    storage
        .store("identity", "key_pair", &record)
        .expect("seeding a test identity");
}

/// The address the protocol running as `label` will have.
///
/// Use wherever a test needs a *wire* identity: a `recipient`, an expected
/// `sender`, a group member id, a session partner.
pub(crate) fn id(label: &str) -> String {
    identity(label).address
}

/// Fresh storage already holding `label`'s identity.
///
/// A manager constructed at a derived address over *empty* storage is refused
/// — it would be claiming an address whose key it does not hold — so a fixture
/// that stands up a bare `MlsManager` for a peer has to seed the identity the
/// same way a real device would already have it.
pub(crate) fn seeded_storage(label: &str) -> Arc<InMemoryStorage> {
    let storage = Arc::new(InMemoryStorage::default());
    let as_trait: Arc<dyn MlsStorage> = storage.clone();
    seed_identity(&as_trait, label);
    storage
}

/// A bare `MlsManager` running as `label`, over `storage`.
///
/// Seeds at the point of use rather than where the storage is declared: test
/// fixtures reuse variable names like `storage_b` across unrelated tests, so
/// pairing a name to a label file-wide silently hands one fixture another
/// fixture's identity.
pub(crate) fn manager_for(label: &str, storage: Arc<InMemoryStorage>) -> MlsManager {
    let as_trait: Arc<dyn MlsStorage> = storage.clone();
    seed_identity(&as_trait, label);
    MlsManager::new(id(label), storage).expect("test manager for a seeded identity")
}

/// Signs `message` as whoever its `sender` address belongs to.
///
/// Control traffic is unconditionally signature-gated, and the signature is
/// checked against the *sender's own address* — so a fixture that builds a
/// control frame and hands it to `process_internal_message` has to sign it
/// exactly the way the peer's own protocol instance would, or it is testing
/// the rejection path rather than the path it names.
///
/// Resolving the identity from `message.sender` rather than from a label
/// argument is what keeps that cheap: the fixtures already say who the sender
/// is, in the one place that has to be right anyway.
///
/// Panics if the sender is not a known test address. That is deliberate — the
/// alternative, silently leaving the frame unsigned, turns "this test signs"
/// into "this test quietly stopped signing" the first time a fixture is
/// renamed. Tests that *want* an unsigned or misattributed frame simply do not
/// call this.
pub(crate) fn sign_as_sender(message: &mut offline_protocol_core::Message) {
    assert!(
        try_sign_as_sender(message),
        "sign_as_sender: '{}' is not a known test identity — build the sender with id(\"label\")",
        message.sender.as_str()
    );
}

/// Signs `message` with `label`'s key **regardless of what its sender says**.
///
/// This is how a forged frame is built: a real, verifiable Ed25519 signature
/// over the real canonical payload, produced by an identity that is not the one
/// the frame claims. It is the only interesting attack shape left once the
/// signature itself is mandatory — an attacker who cannot forge a signature can
/// still sign their *own* frame and lie about who they are, and the derivation
/// check is what answers that.
pub(crate) fn sign_as(label: &str, message: &mut offline_protocol_core::Message) {
    use ed25519_dalek::Signer;

    let signing = identity(label).signing;
    let canonical = crate::OfflineProtocol::build_canonical_payload(message)
        .expect("canonical payload for a test control message");

    message.metadata.insert(
        crate::protocol::CTRL_SIG_META_KEY.to_string(),
        crate::protocol::base64_encode(&signing.sign(&canonical).to_bytes()),
    );
    message.metadata.insert(
        crate::protocol::CTRL_PK_META_KEY.to_string(),
        crate::protocol::base64_encode(&signing.verifying_key().to_bytes()),
    );
}

/// Signs `message` if its sender is a known test identity, and reports whether
/// it did.
///
/// This is the form the shared fixture constructors use, and the `false` case is
/// not a loophole — it is the situation being modelled. A sender nobody holds
/// the key for *cannot* produce a valid signature, which is exactly what a
/// forged or third-party-attributed frame looks like on the wire. Tests that
/// name a hostile id as the sender want precisely that.
///
/// Use [`sign_as_sender`] where a test's premise is that the frame *is* signed,
/// so a renamed fixture fails loudly instead of quietly changing what it tests.
pub(crate) fn try_sign_as_sender(message: &mut offline_protocol_core::Message) -> bool {
    use ed25519_dalek::Signer;

    let sender = message.sender.as_str().to_string();
    // Resolved and the guard dropped *before* any panic: panicking while
    // holding it would poison the cache for every later test in the process
    // and bury one real failure under hundreds of spurious ones.
    let resolved = {
        let guard = cache_guard();
        guard
            .1
            .get(&sender)
            .and_then(|label| guard.0.get(label))
            .map(|entry| entry.signing.clone())
    };
    let Some(signing) = resolved else {
        return false;
    };

    // Signed here with `ed25519_dalek` rather than by standing up an
    // `MlsManager`, which would cost an OpenMLS provider per frame. The bytes
    // are identical: `MlsManager::sign_data` signs with the same Ed25519 key,
    // and `verify_signature` is plain `ed25519_dalek` verification. Pinned
    // against the production signer by
    // `test_identity_signature_matches_the_production_signer`.
    let canonical = crate::OfflineProtocol::build_canonical_payload(message)
        .expect("canonical payload for a test control message");
    let signature = signing.sign(&canonical).to_bytes().to_vec();
    let public_key = signing.verifying_key().to_bytes().to_vec();

    message.metadata.insert(
        crate::protocol::CTRL_SIG_META_KEY.to_string(),
        crate::protocol::base64_encode(&signature),
    );
    message.metadata.insert(
        crate::protocol::CTRL_PK_META_KEY.to_string(),
        crate::protocol::base64_encode(&public_key),
    );
    true
}

/// The 1:1 session slot shared by the protocols running as `a` and `b`.
///
/// Computed rather than written out: the slot orders its two halves by
/// `Address` bytes, which is not the order the labels or their rendered
/// strings would suggest, so a hand-written `session:<a>:<b>` literal is right
/// only by accident.
pub(crate) fn session_slot(a: &str, b: &str) -> String {
    offline_protocol_mls::GroupId::for_session(&id(a), &id(b))
        .expect("session slot for two derived addresses")
        .as_str()
        .to_string()
}

/// The two shortcuts this module takes for speed, pinned against the paths they
/// stand in for.
///
/// Both exist because the honest versions are too slow to run per call across a
/// four-figure test suite — `id` was a storage round-trip and `sign_as_sender`
/// an OpenMLS provider — and both would fail *silently* if they drifted: a test
/// frame signed with the wrong bytes, or addressed to the wrong id, simply
/// exercises the rejection path while still asserting whatever it asserts.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_address_matches_the_bootstrap_derivation() {
        let storage: Arc<dyn MlsStorage> = Arc::new(InMemoryStorage::default());
        seed_identity(&storage, "alice");
        let (_, bootstrapped) =
            MlsManager::load_or_create_identity(&storage).expect("bootstrapping the seeded key");

        assert_eq!(
            bootstrapped.to_string(),
            id("alice"),
            "the memoized address drifted from the production bootstrap"
        );
    }

    #[test]
    fn test_identity_signature_matches_the_production_signer() {
        let mut direct = offline_protocol_core::Message::new(
            offline_protocol_core::UserId::new(id("alice")).unwrap(),
            offline_protocol_core::UserId::new(id("bob")).unwrap(),
            offline_protocol_core::AppId::new("test-app").unwrap(),
            "__CONN_REQ__{}",
        );
        let mut via_manager = direct.clone();

        sign_as_sender(&mut direct);

        let manager = manager_for("alice", seeded_storage("alice"));
        crate::OfflineProtocol::sign_control_message_with(&mut via_manager, &manager)
            .expect("signing through the production path");

        // Ed25519 is deterministic (RFC 8032), so equal keys over equal payloads
        // give byte-equal signatures — this compares the whole stamp, not just
        // that both happen to verify.
        assert_eq!(
            direct.metadata.get(crate::protocol::CTRL_SIG_META_KEY),
            via_manager.metadata.get(crate::protocol::CTRL_SIG_META_KEY),
            "the direct signature drifted from MlsManager::sign_data"
        );
        assert_eq!(
            direct.metadata.get(crate::protocol::CTRL_PK_META_KEY),
            via_manager.metadata.get(crate::protocol::CTRL_PK_META_KEY),
            "the direct public key drifted from the manager's identity key"
        );
    }
}
