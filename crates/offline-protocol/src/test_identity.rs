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

/// The serialized identity record for each label, keyed by label.
fn identities() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static IDENTITIES: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    IDENTITIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The raw `("identity", "key_pair")` record this label runs as.
///
/// Seeded from the label rather than randomly generated, and that is
/// load-bearing rather than tidiness: addresses sort by key hash, and several
/// behaviours under test are decided by that sort (leave election, admin
/// auto-promotion, fork leader). A randomly minted key would reshuffle the
/// order on every run and make those tests pass or fail by luck.
fn identity_record(label: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut cache = identities().lock().expect("test identity cache poisoned");
    if let Some(record) = cache.get(label) {
        return record.clone();
    }

    let seed: [u8; 32] = Sha256::digest(label.as_bytes()).into();
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let keys = openmls_basic_credential::SignatureKeyPair::from_raw(
        openmls_traits::types::SignatureScheme::ED25519,
        signing.to_bytes().to_vec(),
        signing.verifying_key().to_bytes().to_vec(),
    );
    let record = serde_json::to_vec(&keys).expect("serializing a test identity");

    cache.insert(label.to_string(), record.clone());
    record
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
    let record = identity_record(label);
    let storage: Arc<dyn MlsStorage> = Arc::new(InMemoryStorage::default());
    storage
        .store("identity", "key_pair", &record)
        .expect("staging a test identity");
    let (_, address) =
        MlsManager::load_or_create_identity(&storage).expect("deriving a test address");
    address.to_string()
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
