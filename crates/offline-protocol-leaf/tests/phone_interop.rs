//! A real phone talking to a real device.
//!
//! The phone here is `offline-protocol-mls`, built on OpenMLS, driven through
//! its ordinary public API. The device is this crate, built on mls-rs. So
//! every test below is a genuine two-implementation exchange rather than this
//! crate agreeing with itself, which is the only kind of test that can catch
//! the class of bug that matters here: a default in one library that the other
//! refuses.
//!
//! `tools/mls-interop` covers the same pair out of process and pins the
//! library versions. This file covers the choreography that sits above them:
//! the gates, the reset sequence, and the persist-before-emit rule.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use std::collections::BTreeMap;

use offline_protocol_core::{Message, MessagePriority, UserId};
use offline_protocol_leaf::{
    store::{KEY_TYPE_GROUP_EPOCH, KEY_TYPE_IDENTITY, KEY_TYPE_PEER},
    LeafDevice, LeafError, LeafEvent, LeafStore, MemoryStore, StoreError,
};
use offline_protocol_mls::{storage::InMemoryStorage, MlsManager, MlsStorage};
use offline_protocol_sealed::{
    control_signing_payload, derive_address, prefixes, EncryptedMessage, GroupId,
    KeyPackagePayload, WelcomeMessage, MLS_ENVELOPE_COMPACT_V1,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

const APP_ID: &str = "com.example.lock";
const NOW: u64 = 1_787_314_332;

/// A phone, constructed the way production does it: the identity is minted
/// first and the manager is then built at the address it derives to.
///
/// The bootstrap pass exists because a manager's address is a function of a
/// key it mints on first construction, so there is nothing to name it with
/// until it exists. The second construction loads the same identity out of the
/// same storage.
struct Phone {
    manager: MlsManager,
    address: String,
}

fn new_phone() -> Phone {
    let storage: Arc<dyn MlsStorage> = Arc::new(InMemoryStorage::new());
    let bootstrap = MlsManager::new("bootstrap", Arc::clone(&storage)).expect("bootstrap manager");
    let public = bootstrap
        .get_identity_public_key()
        .expect("identity public key");
    let address = derive_address(&public).expect("derive").to_string();
    drop(bootstrap);

    let manager = MlsManager::new(address.clone(), storage).expect("addressed manager");
    Phone { manager, address }
}

fn device(store: Arc<dyn LeafStore>) -> LeafDevice {
    LeafDevice::open(store, APP_ID).expect("device opens")
}

/// Builds a control frame the way the phone's engine does, and signs it with
/// the phone's identity.
///
/// The canonical payload comes from the sealed layer, which is the one
/// construction both ends use, so this is the phone's own signature rather
/// than a second implementation of one.
fn phone_control_frame(phone: &Phone, to: &str, content: String) -> Message {
    let mut message = Message::new(
        UserId::new(&phone.address).expect("phone address is a user id"),
        UserId::new(to).expect("device address is a user id"),
        offline_protocol_core::AppId::new(APP_ID).expect("app id"),
        content,
    );
    message.priority = MessagePriority::High;
    sign_as(phone, &mut message);
    message
}

fn sign_as(phone: &Phone, message: &mut Message) {
    let payload = control_signing_payload(message).expect("canonical payload");
    let signature = phone.manager.sign_data(&payload).expect("phone signs");
    let public = phone
        .manager
        .get_identity_public_key()
        .expect("phone public key");
    message.metadata.insert(
        offline_protocol_sealed::CTRL_SIG_META_KEY.to_string(),
        BASE64.encode(&signature),
    );
    message.metadata.insert(
        offline_protocol_sealed::CTRL_PK_META_KEY.to_string(),
        BASE64.encode(&public),
    );
}

/// Reads a device's key package frame the way the phone's dispatch does.
fn import_device_key_package(phone: &Phone, frame: &Message) {
    let body = frame
        .content
        .strip_prefix(prefixes::KEY_PACKAGE)
        .expect("frame carries a key package");
    let payload: KeyPackagePayload = serde_json::from_str(body).expect("key package body parses");
    phone
        .manager
        .import_key_package(&payload.user_id, &payload.key_package_data)
        .expect("the phone accepts the device's key package");
}

/// Pairs a phone and a device, returning the device's address.
///
/// This is the whole choreography in one place: the device advertises, the
/// phone establishes and sends a Welcome, the device joins and confirms.
fn pair(phone: &Phone, device: &mut LeafDevice) -> String {
    let device_address = device.address().to_string();

    let advertisement = device
        .key_package_frame(&phone.address, NOW)
        .expect("device mints a key package");
    import_device_key_package(phone, &advertisement);

    let welcome = phone
        .manager
        .create_session(&device_address)
        .expect("phone creates the session");
    let welcome_frame = phone_control_frame(
        phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::WELCOME,
            serde_json::to_string(&welcome).expect("welcome serializes")
        ),
    );

    let handled = device.handle(&welcome_frame, NOW).expect("device joins");
    assert!(
        handled.events.contains(&LeafEvent::SessionEstablished {
            peer: phone.address.clone(),
        }),
        "joining a welcome did not establish a session: {:?}",
        handled.events
    );

    // The confirmation is a group-aware decrypt, so the phone must be able to
    // open it. That is the whole reason it is sealed rather than sent as a
    // plaintext acknowledgement.
    assert_eq!(handled.outbound.len(), 1, "expected one confirmation frame");
    let confirm = envelope_of(&handled.outbound[0]);
    let opened = phone
        .manager
        .decrypt_from_user(&confirm, &device_address)
        .expect("phone opens the confirmation");
    assert_eq!(
        opened.as_deref(),
        Some(prefixes::SESSION_CONFIRM_ENCRYPTED.as_bytes()),
        "the confirmation did not carry the encrypted-confirm marker"
    );

    device_address
}

/// Pulls the envelope out of a device's `__MLS_ENC__` frame.
fn envelope_of(message: &Message) -> EncryptedMessage {
    let body = message
        .content
        .strip_prefix(prefixes::ENCRYPTED)
        .expect("frame carries an envelope");
    if body.starts_with('{') {
        serde_json::from_str(body).expect("json envelope parses")
    } else {
        let bytes = BASE64.decode(body).expect("envelope is base64");
        EncryptedMessage::from_bytes(&bytes).expect("compact envelope parses")
    }
}

/// Wraps a device's sealed frame the way a transport would deliver it.
fn phone_sealed_frame(phone: &Phone, to: &str, envelope: &EncryptedMessage) -> Message {
    let body = BASE64.encode(envelope.to_bytes());
    Message::new(
        UserId::new(&phone.address).expect("phone address"),
        UserId::new(to).expect("device address"),
        offline_protocol_core::AppId::new(APP_ID).expect("app id"),
        format!("{}{}", prefixes::ENCRYPTED, body),
    )
}

#[test]
fn a_phone_and_a_device_pair_and_talk_both_ways() {
    let phone = new_phone();
    let store = Arc::new(MemoryStore::new());
    let mut device = device(store);
    let device_address = pair(&phone, &mut device);

    // Phone to device.
    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"unlock")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = device.handle(&frame, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "unlock".to_string(),
        }]
    );

    // Device to phone.
    let answer = device
        .seal(&phone.address, "unlocked", NOW)
        .expect("device seals");
    let opened = phone
        .manager
        .decrypt_from_user(&envelope_of(&answer), &device_address)
        .expect("phone opens the answer");
    assert_eq!(opened.as_deref(), Some(&b"unlocked"[..]));
}

#[test]
fn a_driven_rekey_reaches_the_device_as_a_session_reset() {
    let phone = new_phone();
    let store = Arc::new(MemoryStore::new());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    let device_address = pair(&phone, &mut device);

    assert!(
        device.has_session(&phone.address).expect("session check"),
        "pairing left no session"
    );

    // A phone-driven rekey: the phone discards its own session first and then
    // sends a fresh key package with the reset flag, which is the shape
    // post-compromise security takes for a member that never commits.
    //
    // The teardown-first ordering is the engine's, and it is deliberate there:
    // it makes convergence symmetric whatever the two addresses sort like, so
    // the device always *joins* rather than racing a tiebreaker.
    phone
        .manager
        .delete_session(&device_address)
        .expect("phone discards its own session");

    let phone_package = phone
        .manager
        .take_push_key_package(&device_address)
        .expect("phone mints a package");
    let mut payload: KeyPackagePayload = serde_json::from_str(&format!(
        r#"{{"user_id":{},"key_package_data":[]}}"#,
        serde_json::to_string(&phone.address).expect("address")
    ))
    .expect("skeleton payload");
    payload.key_package_data = phone_package.bundle.key_package_data.clone();
    payload.session_reset = true;
    payload.env_versions = vec![MLS_ENVELOPE_COMPACT_V1];

    let reset_frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::KEY_PACKAGE,
            serde_json::to_string(&payload).expect("payload serializes")
        ),
    );

    let handled = device.handle(&reset_frame, NOW).expect("device resets");
    assert!(
        handled.events.contains(&LeafEvent::SessionReset {
            peer: phone.address.clone(),
        }),
        "a reset flag did not reset the session: {:?}",
        handled.events
    );
    assert!(
        !device.has_session(&phone.address).expect("session check"),
        "the device kept a session the phone had already discarded"
    );

    // And it answers with a fresh package, so the exchange can begin again.
    assert_eq!(
        handled.outbound.len(),
        1,
        "a reset did not produce a fresh key package"
    );
    let fresh = &handled.outbound[0];
    assert!(fresh.content.starts_with(prefixes::KEY_PACKAGE));

    // The phone can complete a second pairing from it, which is what makes the
    // rekey a heal rather than a permanent break.
    import_device_key_package(&phone, fresh);
    let welcome = phone
        .manager
        .create_session(&device_address)
        .expect("phone re-establishes");
    let welcome_frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::WELCOME,
            serde_json::to_string(&welcome).expect("welcome serializes")
        ),
    );
    let rejoined = device.handle(&welcome_frame, NOW).expect("device rejoins");
    assert!(rejoined.events.contains(&LeafEvent::SessionEstablished {
        peer: phone.address.clone(),
    }));
}

#[test]
fn an_unsigned_control_frame_is_refused() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    let mut frame = phone_control_frame(
        &phone,
        &device.address().to_string(),
        format!("{}{{}}", prefixes::KEY_PACKAGE),
    );
    frame.metadata.clear();

    let err = device
        .handle(&frame, NOW)
        .expect_err("unsigned was accepted");
    assert!(
        matches!(err, LeafError::ControlFrameRefused(_)),
        "unsigned control frame produced {err:?}"
    );
}

#[test]
fn half_a_signature_is_refused() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    let mut frame = phone_control_frame(
        &phone,
        &device.address().to_string(),
        format!("{}{{}}", prefixes::KEY_PACKAGE),
    );
    frame
        .metadata
        .remove(offline_protocol_sealed::CTRL_SIG_META_KEY);

    let err = device
        .handle(&frame, NOW)
        .expect_err("a key without a signature was accepted");
    assert!(
        matches!(err, LeafError::ControlFrameRefused(_)),
        "half a signature produced {err:?}"
    );
}

#[test]
fn a_signing_key_that_does_not_derive_to_the_sender_is_refused() {
    let phone = new_phone();
    let impostor = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    // A frame that claims to be from the phone, signed correctly, but by
    // somebody else's key. The signature verifies; the derivation does not.
    let mut frame = phone_control_frame(
        &phone,
        &device.address().to_string(),
        format!("{}{{}}", prefixes::KEY_PACKAGE),
    );
    sign_as(&impostor, &mut frame);

    let err = device
        .handle(&frame, NOW)
        .expect_err("an impostor's key was accepted");
    assert!(
        matches!(err, LeafError::IdentityBinding(_)),
        "impersonation produced {err:?}"
    );
}

#[test]
fn a_sender_that_is_not_an_address_is_refused() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    // The bypass this gate exists to close: claim a nickname, and a check that
    // skipped unparseable identifiers would never run at all.
    let mut frame = phone_control_frame(
        &phone,
        &device.address().to_string(),
        format!("{}{{}}", prefixes::KEY_PACKAGE),
    );
    frame.sender = UserId::new("alice").expect("nickname is a valid user id");
    sign_as(&phone, &mut frame);

    let err = device
        .handle(&frame, NOW)
        .expect_err("a nickname sender was accepted");
    assert!(
        matches!(err, LeafError::IdentityBinding(_)),
        "a non-address sender produced {err:?}"
    );
}

#[test]
fn a_key_package_that_claims_another_owner_is_refused() {
    let phone = new_phone();
    let other = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    let package = phone
        .manager
        .take_push_key_package(&device.address().to_string())
        .expect("package");
    let payload = KeyPackagePayload {
        // The body says one peer; the frame is signed by another.
        user_id: other.address.clone(),
        key_package_data: package.bundle.key_package_data,
        remaining_lifetime_ms: 0,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: vec![],
        env_versions: vec![],
        rich_versions: vec![],
        data_versions: vec![],
        nostr_pubkey: None,
    };

    let frame = phone_control_frame(
        &phone,
        &device.address().to_string(),
        format!(
            "{}{}",
            prefixes::KEY_PACKAGE,
            serde_json::to_string(&payload).expect("payload")
        ),
    );

    let err = device
        .handle(&frame, NOW)
        .expect_err("a borrowed-name package was accepted");
    assert!(
        matches!(err, LeafError::IdentityBinding(_)),
        "a mismatched package owner produced {err:?}"
    );
}

#[test]
fn a_welcome_for_another_pairs_group_is_refused() {
    let phone = new_phone();
    let stranger = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = device.address().to_string();

    // A Welcome the stranger built for a group of their own, relayed by the
    // phone. Without the group check the device would join a room whose
    // membership it never chose.
    let advertisement = device
        .key_package_frame(&stranger.address, NOW)
        .expect("device advertises");
    import_device_key_package(&stranger, &advertisement);
    let welcome = stranger
        .manager
        .create_session(&device_address)
        .expect("stranger creates a session");

    let frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::WELCOME,
            serde_json::to_string(&welcome).expect("welcome")
        ),
    );

    let err = device
        .handle(&frame, NOW)
        .expect_err("a relayed welcome was accepted");
    assert!(
        matches!(err, LeafError::IdentityBinding(_)),
        "a foreign welcome produced {err:?}"
    );
}

#[test]
fn state_survives_a_power_cycle() {
    let phone = new_phone();
    let store: Arc<dyn LeafStore> = Arc::new(MemoryStore::new());
    let device_address = {
        let mut device = device(Arc::clone(&store));
        pair(&phone, &mut device)
    };

    // The device value is gone; only the store remains. This is what a reboot
    // looks like from the outside, and it is the whole reason no MLS state is
    // cached in the device value.
    let mut revived = LeafDevice::resume(Arc::clone(&store), APP_ID).expect("device resumes");
    assert_eq!(
        revived.address().to_string(),
        device_address,
        "the device came back as a different device"
    );

    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"still there?")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = revived.handle(&frame, NOW).expect("revived device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "still there?".to_string(),
        }],
        "a session did not survive the power cycle"
    );
}

#[test]
fn a_replayed_sealed_frame_is_refused() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = pair(&phone, &mut device);

    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"unlock")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device_address, &sealed);

    device.handle(&frame, NOW).expect("first delivery opens");
    let err = device
        .handle(&frame, NOW)
        .expect_err("a replayed frame was opened a second time");
    assert!(
        matches!(err, LeafError::Mls(_)),
        "a replay produced {err:?}"
    );
}

#[test]
fn the_device_answers_a_probe_with_an_ack() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    pair(&phone, &mut device);

    let probe = phone_control_frame(
        &phone,
        &device.address().to_string(),
        prefixes::SESSION_CONFIRM_PROBE.to_string(),
    );
    let handled = device.handle(&probe, NOW).expect("device answers");

    assert_eq!(handled.outbound.len(), 1);
    assert_eq!(
        handled.outbound[0].content,
        prefixes::SESSION_CONFIRM_ACK,
        "a probe was not answered with an acknowledgement"
    );
    assert!(
        handled.outbound[0]
            .metadata
            .contains_key(offline_protocol_sealed::CTRL_SIG_META_KEY),
        "the acknowledgement went out unsigned"
    );
}

#[test]
fn a_probe_without_a_session_is_not_answered() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    // No pairing: this is a device that was wiped, or one this peer never
    // paired with. The phone confirms its session on the acknowledgement and
    // then flushes everything it had queued into it, so answering here would
    // leave it holding a session confirmed against a device that cannot
    // decrypt a single frame of it, with nothing afterwards to tell it so.
    let probe = phone_control_frame(
        &phone,
        &device.address().to_string(),
        prefixes::SESSION_CONFIRM_PROBE.to_string(),
    );
    let handled = device.handle(&probe, NOW).expect("probe is handled");

    assert!(
        handled.outbound.is_empty(),
        "a device with no session confirmed one anyway"
    );
    assert!(
        matches!(handled.events.as_slice(), [LeafEvent::Ignored { .. }]),
        "a session-less probe produced {:?}",
        handled.events
    );
}

#[test]
fn a_peer_that_already_has_a_key_package_is_not_sent_another() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = device.address().to_string();

    // The device advertises first, which is how pairing starts.
    let _ = device
        .key_package_frame(&phone.address, NOW)
        .expect("device advertises");

    // The phone answers with its own package, as the engine does. If the
    // device answered that in turn, the two would trade packages forever, and
    // each exchange spends an init key.
    let package = phone
        .manager
        .take_push_key_package(&device_address)
        .expect("package");
    let payload = KeyPackagePayload {
        user_id: phone.address.clone(),
        key_package_data: package.bundle.key_package_data,
        remaining_lifetime_ms: 0,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: vec![],
        env_versions: vec![MLS_ENVELOPE_COMPACT_V1],
        rich_versions: vec![],
        data_versions: vec![],
        nostr_pubkey: None,
    };
    let frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::KEY_PACKAGE,
            serde_json::to_string(&payload).expect("payload")
        ),
    );

    let handled = device.handle(&frame, NOW).expect("device records the peer");
    assert!(
        handled.outbound.is_empty(),
        "the device answered a key package with another one, which loops"
    );
    assert!(handled.events.contains(&LeafEvent::PeerAdvertised {
        peer: phone.address.clone(),
    }));
}

#[test]
fn the_envelope_encoding_follows_what_the_peer_advertised() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = pair(&phone, &mut device);

    // Pairing recorded nothing about the phone's capabilities, because the
    // phone never sent a key package in this flow. Absent means the floor.
    assert!(device
        .peer_env_versions(&phone.address)
        .expect("record")
        .is_empty());
    let floor = device
        .seal(&phone.address, "floor", NOW)
        .expect("device seals");
    let body = floor
        .content
        .strip_prefix(prefixes::ENCRYPTED)
        .expect("envelope");
    assert!(
        body.starts_with('{'),
        "a peer that advertised nothing was sent a compact envelope"
    );

    // And the phone opens it, because the JSON envelope is the permanent floor
    // rather than a legacy path anything may stop parsing.
    let opened = phone
        .manager
        .decrypt_from_user(&envelope_of(&floor), &device_address)
        .expect("phone opens the floor envelope");
    assert_eq!(opened.as_deref(), Some(&b"floor"[..]));
}

/// A store that fails every write after it is armed.
///
/// Used as a negative control for the rule this crate is built around: if a
/// persist fails, no frame may exist. A device that emitted first and
/// persisted second would come back from a power cut and reuse an AEAD nonce.
#[derive(Default)]
struct FailingStore {
    inner: MemoryStore,
    failing: AtomicBool,
    writes_after_arming: Mutex<usize>,
}

impl FailingStore {
    fn arm(&self) {
        self.failing.store(true, Ordering::SeqCst);
    }
}

impl LeafStore for FailingStore {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError> {
        if self.failing.load(Ordering::SeqCst) {
            if let Ok(mut count) = self.writes_after_arming.lock() {
                *count += 1;
            }
            return Err(StoreError::Store("flash is on fire".to_string()));
        }
        self.inner.store(key_type, key_id, data)
    }

    fn load(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.load(key_type, key_id)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> Result<(), StoreError> {
        self.inner.delete(key_type, key_id)
    }
}

#[test]
fn a_failing_store_produces_no_frame() {
    let phone = new_phone();
    let store = Arc::new(FailingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    pair(&phone, &mut device);

    store.arm();

    let err = device
        .seal(&phone.address, "unlock", NOW)
        .expect_err("a frame was produced despite the store failing");
    assert!(
        matches!(err, LeafError::Storage(_)),
        "a failing store produced {err:?}"
    );

    // The control: the write really was attempted, so this test is failing at
    // the persist rather than short-circuiting somewhere earlier and passing
    // for the wrong reason.
    assert!(
        *store.writes_after_arming.lock().expect("count") > 0,
        "no write was even attempted, so this proves nothing about ordering"
    );
}

#[test]
fn a_device_refuses_to_replace_its_own_identity() {
    let store: Arc<dyn LeafStore> = Arc::new(MemoryStore::new());
    let first = LeafDevice::provision(Arc::clone(&store), APP_ID).expect("first provisioning");

    let err = LeafDevice::provision(Arc::clone(&store), APP_ID)
        .expect_err("a second provisioning replaced the identity");
    assert!(matches!(err, LeafError::AlreadyProvisioned));

    // And `open` is the safe door: it resumes rather than replacing.
    let reopened = LeafDevice::open(store, APP_ID).expect("open resumes");
    assert_eq!(reopened.address().to_string(), first.address().to_string());
}

#[test]
fn a_device_address_derives_from_its_own_key() {
    // The property every trust gate in this protocol rests on, checked from
    // the outside: what the device calls itself is a function of the key it
    // signs with, so a peer can refute a claim without a directory.
    let mut device = device(Arc::new(MemoryStore::new()));
    let probe_target = device.address().to_string();

    let phone = new_phone();
    // Paired first, because a probe is only answered by a device that holds a
    // session; the acknowledgement is just the most convenient signed frame to
    // read the key out of.
    pair(&phone, &mut device);
    let mut frame = phone_control_frame(
        &phone,
        &probe_target,
        prefixes::SESSION_CONFIRM_PROBE.to_string(),
    );
    sign_as(&phone, &mut frame);

    let handled = device.handle(&frame, NOW).expect("probe answered");
    let ack = &handled.outbound[0];
    let key = BASE64
        .decode(
            ack.metadata
                .get(offline_protocol_sealed::CTRL_PK_META_KEY)
                .expect("ack carries a key"),
        )
        .expect("key is base64");

    assert_eq!(
        derive_address(&key).expect("derive").to_string(),
        device.address().to_string(),
        "the device signed with a key that does not derive to its own address"
    );
}

/// A store that can be asked what is in it.
///
/// `LeafStore` deliberately offers no enumeration, which is right for the
/// seam and useless for a test that has to prove something was *removed*.
#[derive(Debug, Default)]
struct CountingStore {
    entries: Mutex<BTreeMap<(String, String), Vec<u8>>>,
}

impl CountingStore {
    /// The key ids held under one key type.
    fn keys_of(&self, key_type: &str) -> Vec<String> {
        self.entries
            .lock()
            .expect("lock")
            .keys()
            .filter(|(held, _)| held == key_type)
            .map(|(_, id)| id.clone())
            .collect()
    }

    /// Prior-epoch records, which are the `:max` marker's siblings.
    fn epoch_records(&self) -> Vec<String> {
        self.keys_of(KEY_TYPE_GROUP_EPOCH)
            .into_iter()
            .filter(|id| !id.ends_with(":max"))
            .collect()
    }
}

impl LeafStore for CountingStore {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError> {
        self.entries
            .lock()
            .expect("lock")
            .insert((key_type.to_string(), key_id.to_string()), data.to_vec());
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .entries
            .lock()
            .expect("lock")
            .get(&(key_type.to_string(), key_id.to_string()))
            .cloned())
    }

    fn delete(&self, key_type: &str, key_id: &str) -> Result<(), StoreError> {
        self.entries
            .lock()
            .expect("lock")
            .remove(&(key_type.to_string(), key_id.to_string()));
        Ok(())
    }
}

/// Drives one phone-side commit and delivers it to the device.
fn commit_to(phone: &Phone, device: &mut LeafDevice, device_address: &str) {
    let group_id = GroupId::for_session(&phone.address, device_address).expect("pair group id");
    let commit = phone.manager.update_keys(&group_id).expect("phone commits");
    let frame = phone_sealed_frame(phone, device_address, &commit);
    let handled = device
        .handle(&frame, NOW)
        .expect("device applies the commit");
    assert!(
        handled
            .events
            .iter()
            .any(|event| matches!(event, LeafEvent::CommitApplied { .. })),
        "a commit did not apply: {:?}",
        handled.events
    );
}

#[test]
fn a_commit_trims_the_prior_epochs_it_leaves_behind() {
    let phone = new_phone();
    let store = Arc::new(CountingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    let device_address = pair(&phone, &mut device);

    // Post-compromise security arrives on the phone's cadence, so on a device
    // that lives for years this is the loop that runs forever. Every commit
    // leaves the epoch it departed behind as a record, and mls-rs leaves it to
    // the storage provider to decide how many of those to keep: a provider
    // that keeps all of them fills a part with a few hundred kilobytes of
    // flash, and keeps every one of those epochs' secrets while it does, so a
    // device taken apart later reads back everything it ever received.
    for _ in 0..12 {
        commit_to(&phone, &mut device, &device_address);
    }

    let records = store.epoch_records();
    assert!(
        records.len() <= 3,
        "twelve commits left {} prior-epoch records: {records:?}",
        records.len()
    );

    // And the window that is kept is still a working one: the session did not
    // survive by being trimmed into uselessness.
    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"still talking")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = device.handle(&frame, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "still talking".to_string(),
        }]
    );
}

#[test]
fn unpairing_erases_the_prior_epoch_records_too() {
    let phone = new_phone();
    let store = Arc::new(CountingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    let device_address = pair(&phone, &mut device);

    commit_to(&phone, &mut device, &device_address);
    commit_to(&phone, &mut device, &device_address);
    assert!(
        !store.epoch_records().is_empty(),
        "the setup wrote no epoch records, so this test would pass vacuously"
    );

    device.unpair(&phone.address).expect("device unpairs");

    // Each of those records holds an epoch's secrets. Leaving them behind is
    // key material outliving the erasure the owner asked for, under a name the
    // next session with the same peer answers to, because a pair's group id is
    // derived from the two addresses and does not change on a re-pair.
    assert!(
        store.epoch_records().is_empty(),
        "unpairing left epoch records behind: {:?}",
        store.epoch_records()
    );
    assert!(
        store.keys_of(KEY_TYPE_PEER).is_empty(),
        "unpairing left the peer record behind"
    );
    assert!(
        !device.has_session(&phone.address).expect("session check"),
        "unpairing left a session"
    );
}

/// A store that refuses to write one key, to cut power at a chosen moment.
struct CutStore {
    inner: MemoryStore,
    refuse: Mutex<Option<String>>,
}

impl CutStore {
    fn cutting(key_id: &str) -> Self {
        Self {
            inner: MemoryStore::new(),
            refuse: Mutex::new(Some(key_id.to_string())),
        }
    }

    fn restore_power(&self) {
        *self.refuse.lock().expect("lock") = None;
    }
}

impl LeafStore for CutStore {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError> {
        if self.refuse.lock().expect("lock").as_deref() == Some(key_id) {
            return Err(StoreError::Store("power cut".to_string()));
        }
        self.inner.store(key_type, key_id, data)
    }

    fn load(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.load(key_type, key_id)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> Result<(), StoreError> {
        self.inner.delete(key_type, key_id)
    }
}

#[test]
fn a_provisioning_cut_between_the_two_identity_writes_is_recoverable() {
    // An identity is two entries and the store is atomic per entry, not across
    // a pair, so a cut lands between them. The secret is written last and is
    // the marker `provision` refuses on, which is what makes the torn state one
    // the next boot overwrites. Written the other way round the two checks
    // disagree, `open` has no third door, and a device that lost power once on
    // its very first boot answers every call with an error forever.
    let store = Arc::new(CutStore::cutting("signature_secret"));

    let err = LeafDevice::provision(Arc::clone(&store) as Arc<dyn LeafStore>, APP_ID)
        .expect_err("the cut write reported success");
    assert!(matches!(err, LeafError::Storage(_)), "cut produced {err:?}");

    // The control: the first write really did land, so this is the torn state
    // and not simply an empty store.
    assert!(
        store
            .load(KEY_TYPE_IDENTITY, "signature_public")
            .expect("load")
            .is_some(),
        "nothing was written at all, so this proves nothing about ordering"
    );

    store.restore_power();
    let device = LeafDevice::open(Arc::clone(&store) as Arc<dyn LeafStore>, APP_ID)
        .expect("a device that was cut mid-provisioning could not be opened");

    // And it is a working identity, not a half of one.
    let resumed = LeafDevice::resume(store as Arc<dyn LeafStore>, APP_ID).expect("device resumes");
    assert_eq!(resumed.address().to_string(), device.address().to_string());
}

#[test]
fn a_replayed_session_reset_does_not_tear_down_the_new_session() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = pair(&phone, &mut device);

    // A reset frame, captured off the air the first time it was sent.
    phone
        .manager
        .delete_session(&device_address)
        .expect("phone discards its own session");
    let package = phone
        .manager
        .take_push_key_package(&device_address)
        .expect("phone mints a package");
    let payload = KeyPackagePayload {
        user_id: phone.address.clone(),
        key_package_data: package.bundle.key_package_data,
        remaining_lifetime_ms: 0,
        timestamp_ms: 0,
        session_reset: true,
        wire_versions: vec![],
        env_versions: vec![MLS_ENVELOPE_COMPACT_V1],
        rich_versions: vec![],
        data_versions: vec![],
        nostr_pubkey: None,
    };
    let reset_frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::KEY_PACKAGE,
            serde_json::to_string(&payload).expect("payload")
        ),
    );

    let handled = device.handle(&reset_frame, NOW).expect("device resets");
    assert!(handled.events.contains(&LeafEvent::SessionReset {
        peer: phone.address.clone(),
    }));

    // The pair rebuilds, as a driven rekey is meant to.
    import_device_key_package(&phone, &handled.outbound[0]);
    let welcome = phone
        .manager
        .create_session(&device_address)
        .expect("phone re-establishes");
    let welcome_frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::WELCOME,
            serde_json::to_string(&welcome).expect("welcome")
        ),
    );
    device.handle(&welcome_frame, NOW).expect("device rejoins");
    assert!(device.has_session(&phone.address).expect("session check"));

    // Now the captured frame is sent again. Nothing in the signed payload says
    // when it was made, so it verifies exactly as well as it did the first
    // time; what stops it is the device remembering that it already acted on
    // it. Without that, one captured frame is a session teardown that can be
    // replayed at will.
    let replayed = device
        .handle(&reset_frame, NOW)
        .expect("the replay is handled");
    assert!(
        !replayed.events.contains(&LeafEvent::SessionReset {
            peer: phone.address.clone(),
        }),
        "a replayed reset tore down the session again: {:?}",
        replayed.events
    );
    assert!(
        device.has_session(&phone.address).expect("session check"),
        "a replayed reset discarded a session the peer still holds"
    );

    // And the session it kept is the working one.
    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"unlock")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = device.handle(&frame, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "unlock".to_string(),
        }]
    );
}

#[test]
fn a_flood_of_strangers_cannot_displace_an_established_peer() {
    let phone = new_phone();
    let store = Arc::new(CountingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    let device_address = pair(&phone, &mut device);

    // The phone advertises, so it holds a record like any other peer and is a
    // candidate for eviction on the same terms.
    let package = phone
        .manager
        .take_push_key_package(&device_address)
        .expect("package");
    let payload = KeyPackagePayload {
        user_id: phone.address.clone(),
        key_package_data: package.bundle.key_package_data,
        remaining_lifetime_ms: 0,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: vec![],
        env_versions: vec![MLS_ENVELOPE_COMPACT_V1],
        rich_versions: vec![],
        data_versions: vec![],
        nostr_pubkey: None,
    };
    let frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::KEY_PACKAGE,
            serde_json::to_string(&payload).expect("payload")
        ),
    );
    device
        .handle(&frame, NOW)
        .expect("device records the phone");

    // Producing a signature that derives to its own address is as hard as
    // generating a key, so a stranger's frame costs nothing to make. Each one
    // that lands writes a peer record and mints a package, both to flash.
    for index in 0..24 {
        let stranger = new_phone();
        let payload = KeyPackagePayload {
            user_id: stranger.address.clone(),
            key_package_data: vec![index],
            remaining_lifetime_ms: 0,
            timestamp_ms: 0,
            session_reset: false,
            wire_versions: vec![],
            env_versions: vec![],
            rich_versions: vec![],
            data_versions: vec![],
            nostr_pubkey: None,
        };
        let frame = phone_control_frame(
            &stranger,
            &device_address,
            format!(
                "{}{}",
                prefixes::KEY_PACKAGE,
                serde_json::to_string(&payload).expect("payload")
            ),
        );
        // Admitted or refused, both are fine. What is not fine is unbounded.
        let _ = device.handle(&frame, NOW);
    }

    assert!(
        store.keys_of(KEY_TYPE_PEER).len() <= 16,
        "a flood grew the peer table to {}",
        store.keys_of(KEY_TYPE_PEER).len()
    );

    // The property that matters: whoever the flood displaced, it was not the
    // peer the owner actually paired with.
    assert!(
        device.has_session(&phone.address).expect("session check"),
        "a flood of strangers evicted the established peer"
    );
    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"unlock")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = device.handle(&frame, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "unlock".to_string(),
        }]
    );
}

#[test]
fn a_welcome_whose_body_lies_about_its_group_is_refused() {
    let phone = new_phone();
    let stranger = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = device.address().to_string();

    // The stranger builds a real group with this device, using a package the
    // device minted for it.
    let advertisement = device
        .key_package_frame(&stranger.address, NOW)
        .expect("device advertises");
    import_device_key_package(&stranger, &advertisement);
    let foreign = stranger
        .manager
        .create_session(&device_address)
        .expect("stranger creates a session");

    // The phone relays that Welcome under an honest-looking body: it names
    // itself as the inviter and this pair's own group id, so both of the
    // checks that read the body pass. Only the Welcome inside disagrees, and
    // it is the one that decides which group is actually joined.
    let forged = WelcomeMessage {
        group_id: GroupId::for_session(&phone.address, &device_address).expect("pair group id"),
        welcome_data: foreign.welcome_data.clone(),
        inviter_id: phone.address.clone(),
        group_name: None,
        timestamp_ms: 0,
    };
    let frame = phone_control_frame(
        &phone,
        &device_address,
        format!(
            "{}{}",
            prefixes::WELCOME,
            serde_json::to_string(&forged).expect("welcome")
        ),
    );

    let err = device
        .handle(&frame, NOW)
        .expect_err("a welcome whose body lied was accepted");
    assert!(
        matches!(err, LeafError::IdentityBinding(_)),
        "a mismatched welcome produced {err:?}"
    );
    assert!(
        !device.has_session(&phone.address).expect("session check"),
        "the refused welcome still left a session on flash"
    );
}
