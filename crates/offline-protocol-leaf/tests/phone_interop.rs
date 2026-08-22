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
    store::{
        KEY_TYPE_GROUP_EPOCH, KEY_TYPE_GROUP_STATE, KEY_TYPE_IDENTITY, KEY_TYPE_KEY_PACKAGE,
        KEY_TYPE_PEER,
    },
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
fn an_unsolicited_acknowledgement_does_not_establish_a_session() {
    let stranger = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));

    // A leaf emits acknowledgements and never probes, so it never has one
    // outstanding and every inbound acknowledgement is unsolicited. Acting on
    // one would hand a session to anyone holding a keypair: the frame is
    // signed, and a signature that derives to its own address costs nothing to
    // produce. The phone gates the same frame on holding a session of its own,
    // and the leaf profile lists this prefix under what a device emits rather
    // than under what it accepts.
    let frame = phone_control_frame(
        &stranger,
        &device.address().to_string(),
        prefixes::SESSION_CONFIRM_ACK.to_string(),
    );

    let handled = device
        .handle(&frame, NOW)
        .expect("the acknowledgement is handled");
    assert!(
        !handled
            .events
            .iter()
            .any(|event| matches!(event, LeafEvent::SessionEstablished { .. })),
        "an unsolicited acknowledgement established a session: {:?}",
        handled.events
    );
    assert!(
        handled.outbound.is_empty(),
        "an unsolicited acknowledgement produced a frame"
    );
    assert!(
        !device
            .has_session(&stranger.address)
            .expect("session check"),
        "an unsolicited acknowledgement left a session on flash"
    );

    // And what firmware would have acted on is refused where it counts: there
    // is no session to seal into, whatever the event said.
    let err = device
        .seal(&stranger.address, "unlock", NOW)
        .expect_err("sealed to a peer that only sent an acknowledgement");
    assert!(matches!(err, LeafError::NoSession(_)), "produced {err:?}");
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

#[test]
fn a_corrupt_group_state_is_not_reported_as_a_missing_session() {
    let phone = new_phone();
    let store = Arc::new(CountingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    pair(&phone, &mut device);

    // A store handing back bytes this device did not write. Reported as a
    // missing session it sends a bench chasing a re-pair, which is the one
    // repair that cannot work: the pairing is fine and the flash is not.
    let key = store
        .keys_of(KEY_TYPE_GROUP_STATE)
        .into_iter()
        .next()
        .expect("pairing wrote group state");
    store
        .store(KEY_TYPE_GROUP_STATE, &key, b"not a group")
        .expect("store");

    let err = device
        .seal(&phone.address, "unlock", NOW)
        .expect_err("a corrupt group state still produced a frame");
    assert!(
        matches!(err, LeafError::Storage(_)),
        "a corrupt group state produced {err:?}"
    );

    // The control: an absent session is still the other error, so this test is
    // not simply asserting that everything is a storage failure.
    let stranger = new_phone();
    let err = device
        .seal(&stranger.address, "unlock", NOW)
        .expect_err("sealed to a peer with no session");
    assert!(
        matches!(err, LeafError::NoSession(_)),
        "an absent session produced {err:?}"
    );
}

#[test]
fn unpairing_sweeps_epochs_a_corrupt_marker_cannot_bound() {
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

    // The marker is what bounds the sweep, and it is one more record on a part
    // whose flash can hand back something else. Anchoring at zero when it does
    // deletes one record, returns `Ok`, and leaves the rest of the epochs'
    // secrets on flash under a name the next session with this peer answers to.
    let marker = store
        .keys_of(KEY_TYPE_GROUP_EPOCH)
        .into_iter()
        .find(|id| id.ends_with(":max"))
        .expect("a marker was written");
    store
        .store(KEY_TYPE_GROUP_EPOCH, &marker, b"not eight bytes")
        .expect("store");

    device.unpair(&phone.address).expect("device unpairs");

    assert!(
        store.epoch_records().is_empty(),
        "a corrupt marker left epoch records behind: {:?}",
        store.epoch_records()
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

/// A store that refuses writes of one key type while armed.
///
/// A power cut lands between two entries rather than inside one, and this is
/// how that looks to a caller: everything before the cut is durable, the entry
/// at the cut is not, and the device is asked to carry on afterwards.
struct TornStore {
    inner: MemoryStore,
    refuse_type: Mutex<Option<String>>,
    refusals: Mutex<usize>,
}

impl TornStore {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            refuse_type: Mutex::new(None),
            refusals: Mutex::new(0),
        }
    }

    fn cut(&self, key_type: &str) {
        *self.refuse_type.lock().expect("lock") = Some(key_type.to_string());
    }

    fn restore_power(&self) {
        *self.refuse_type.lock().expect("lock") = None;
    }

    fn refusals(&self) -> usize {
        *self.refusals.lock().expect("lock")
    }
}

impl LeafStore for TornStore {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError> {
        if self.refuse_type.lock().expect("lock").as_deref() == Some(key_type) {
            *self.refusals.lock().expect("lock") += 1;
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
fn a_cut_between_the_epoch_records_and_the_state_does_not_wedge_the_session() {
    let phone = new_phone();
    let store = Arc::new(TornStore::new());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    let device_address = pair(&phone, &mut device);

    // A healthy commit first, so the group has a marker to get ahead of. The
    // wedge this guards needs a marker that already exists.
    commit_to(&phone, &mut device, &device_address);

    // The cut: the commit's epoch records reach flash, the state does not.
    // mls-rs sequences every later insert against the marker, so a marker that
    // landed here without its state refuses every commit that follows, for
    // good: the retry offers the epoch id the marker has already counted.
    store.cut(KEY_TYPE_GROUP_STATE);
    let group_id = GroupId::for_session(&phone.address, &device_address).expect("pair group id");
    let commit = phone.manager.update_keys(&group_id).expect("phone commits");
    let frame = phone_sealed_frame(&phone, &device_address, &commit);
    let err = device
        .handle(&frame, NOW)
        .expect_err("a state write that failed still reported success");
    assert!(
        matches!(err, LeafError::Storage(_)),
        "a cut state write produced {err:?}"
    );

    // The control: the write really was refused, so this test is exercising
    // the torn window rather than passing because nothing was attempted.
    assert!(
        store.refusals() > 0,
        "no state write was attempted, so this proves nothing about the window"
    );

    // Power comes back and the phone retries the frame it never saw answered,
    // which is what a radio does with anything unacknowledged.
    store.restore_power();
    let retried = device
        .handle(&frame, NOW)
        .expect("the retried commit was refused, so the session is wedged");
    assert!(
        retried
            .events
            .iter()
            .any(|event| matches!(event, LeafEvent::CommitApplied { .. })),
        "the retried commit did not apply: {:?}",
        retried.events
    );

    // And the session is a working one rather than one that merely stopped
    // reporting errors.
    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"unlock")
        .expect("phone seals");
    let app_frame = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = device.handle(&app_frame, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "unlock".to_string(),
        }]
    );

    // The device answers too, so its own ratchet advanced rather than merely
    // its reader.
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
fn a_peer_that_paired_through_a_welcome_is_in_the_audit_list() {
    let phone = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    pair(&phone, &mut device);

    // Authorization is the obligation this crate cannot discharge, and
    // `peers` is what it offers firmware instead: the list of who a device
    // ended up holding. A session missing from it is one nobody can audit and
    // nobody can point `unpair` at, and advertise-then-Welcome is the ordinary
    // way a session comes to exist, not an unusual one.
    assert!(
        device.has_session(&phone.address).expect("session check"),
        "the pairing left no session, so this test would pass vacuously"
    );
    let peers = device.peers().expect("peers");
    assert!(
        peers.contains(&phone.address),
        "a peer this device holds a session with is missing from the audit list: {peers:?}"
    );

    // And the list is actionable: what it names can be removed.
    device.unpair(&phone.address).expect("device unpairs");
    assert!(
        !device.has_session(&phone.address).expect("session check"),
        "unpairing a peer from the audit list left its session"
    );
    assert!(
        device.peers().expect("peers").is_empty(),
        "unpairing left the peer in the audit list"
    );
}

#[test]
fn a_probe_against_a_group_state_that_will_not_load_is_not_answered() {
    let phone = new_phone();
    let store = Arc::new(CountingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    pair(&phone, &mut device);

    // Bytes on flash are not a session. A peer confirms its session on the
    // acknowledgement and then flushes everything it queued into it, so a
    // device that answered on the strength of a state it cannot load would
    // confirm a session it cannot open one frame of, and its silence
    // afterwards is indistinguishable from a quiet link.
    let key = store
        .keys_of(KEY_TYPE_GROUP_STATE)
        .into_iter()
        .next()
        .expect("pairing wrote group state");
    store
        .store(KEY_TYPE_GROUP_STATE, &key, b"not a group")
        .expect("store");

    let probe = phone_control_frame(
        &phone,
        &device.address().to_string(),
        prefixes::SESSION_CONFIRM_PROBE.to_string(),
    );
    let err = device
        .handle(&probe, NOW)
        .expect_err("a device with an unloadable session confirmed one anyway");

    // And it is reported as what it is. A store handing back bytes this device
    // did not write is not a missing session, and sending a bench to re-pair
    // is sending it after the one repair that cannot work.
    assert!(
        matches!(err, LeafError::Storage(_)),
        "an unloadable state produced {err:?}"
    );
}

#[test]
fn a_resumed_device_refuses_a_public_key_its_secret_did_not_make() {
    let store: Arc<dyn LeafStore> = Arc::new(MemoryStore::new());
    let original = LeafDevice::provision(Arc::clone(&store), APP_ID).expect("provisioning");
    let address = original.address().to_string();
    let own = store
        .load(KEY_TYPE_IDENTITY, "signature_public")
        .expect("load")
        .expect("provisioning wrote a public key");

    // The identity is two entries, and the reason this crate has a durability
    // contract at all is that a part can hand back something other than what
    // was written. Somebody else's public key is the shape that does the most
    // damage quietly: the device comes back at an address no peer knows it by
    // and signs frames that verify nowhere, and every gate in the protocol
    // refuses it while naming a different failure than the one that happened.
    let other_store: Arc<dyn LeafStore> = Arc::new(MemoryStore::new());
    LeafDevice::provision(Arc::clone(&other_store), APP_ID).expect("second device");
    let foreign = other_store
        .load(KEY_TYPE_IDENTITY, "signature_public")
        .expect("load")
        .expect("the second device wrote a public key");
    assert_ne!(
        foreign, own,
        "the two devices minted the same key, so this test would prove nothing"
    );

    store
        .store(KEY_TYPE_IDENTITY, "signature_public", &foreign)
        .expect("store");
    let err = LeafDevice::resume(Arc::clone(&store), APP_ID)
        .expect_err("a device resumed on a key its secret does not derive to");
    assert!(
        matches!(err, LeafError::Storage(_)),
        "a mismatched identity pair produced {err:?}"
    );

    // The control: with its own key back, the same device resumes as itself.
    // This refuses a mismatch rather than refusing everything.
    store
        .store(KEY_TYPE_IDENTITY, "signature_public", &own)
        .expect("store");
    let resumed = LeafDevice::resume(store, APP_ID).expect("the intact pair still resumes");
    assert_eq!(resumed.address().to_string(), address);
}

#[test]
fn unpairing_sweeps_epochs_when_the_state_entry_cannot_be_read() {
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

    // The sweep's first anchor is the marker inside the state entry, and this
    // is the case that anchor cannot cover: the entry itself is unreadable, so
    // neither the marker nor the group's own epoch can bound anything. The
    // separate high-water record exists for exactly this, and without it the
    // sweep would anchor at zero, delete one record, return `Ok`, and leave
    // the rest of the epochs' secrets on flash under a name the next session
    // with this peer answers to.
    let key = store
        .keys_of(KEY_TYPE_GROUP_STATE)
        .into_iter()
        .next()
        .expect("pairing wrote group state");
    store
        .store(KEY_TYPE_GROUP_STATE, &key, b"nope")
        .expect("store");

    device.unpair(&phone.address).expect("device unpairs");

    assert!(
        store.epoch_records().is_empty(),
        "an unreadable state entry left epoch records behind: {:?}",
        store.epoch_records()
    );
}

/// Builds a key package advertisement body, with the package data left as
/// junk because a device records what a peer advertises and never parses the
/// package itself: the phone builds the group, not the device.
fn advertisement_body(user_id: &str) -> String {
    let payload = KeyPackagePayload {
        user_id: user_id.to_string(),
        key_package_data: vec![7],
        remaining_lifetime_ms: 0,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: vec![],
        env_versions: vec![MLS_ENVELOPE_COMPACT_V1],
        rich_versions: vec![],
        data_versions: vec![],
        nostr_pubkey: None,
    };
    format!(
        "{}{}",
        prefixes::KEY_PACKAGE,
        serde_json::to_string(&payload).expect("payload")
    )
}

#[test]
fn a_frame_addressed_to_another_node_is_ignored() {
    let phone = new_phone();
    let bystander = new_phone();
    let store = Arc::new(CountingStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);
    let device_address = device.address().to_string();

    // One body, sent twice, to two different recipients. That is the whole
    // test: the frames are otherwise identical, so whatever the device does
    // differently it did because of the addressing and nothing else.
    let body = advertisement_body(&phone.address);

    // A radio hears what it is not the recipient of. This frame is honestly
    // signed and its signature covers the recipient, so every gate below the
    // dispatch verifies it happily; only the addressing says it is not ours.
    let overheard = phone_control_frame(&phone, &bystander.address, body.clone());
    let handled = device
        .handle(&overheard, NOW)
        .expect("overhearing a neighbour is not a failure");

    assert!(
        handled.outbound.is_empty(),
        "the device answered a frame addressed to somebody else: {:?}",
        handled.outbound.len()
    );
    assert!(
        matches!(handled.events.as_slice(), [LeafEvent::Ignored { .. }]),
        "an overheard frame produced {:?}",
        handled.events
    );

    // And it spent nothing. Each of these is a write to a part with a finite
    // number of them, and the key package is a private init key minted for a
    // pairing nobody asked this device for.
    assert!(
        store.keys_of(KEY_TYPE_PEER).is_empty(),
        "an overheard frame wrote a peer record: {:?}",
        store.keys_of(KEY_TYPE_PEER)
    );
    assert!(
        store.keys_of(KEY_TYPE_KEY_PACKAGE).is_empty(),
        "an overheard frame minted a key package: {:?}",
        store.keys_of(KEY_TYPE_KEY_PACKAGE)
    );
    assert!(
        device.peers().expect("peers").is_empty(),
        "an overheard frame put a peer in the audit list"
    );

    // The control: the same body addressed to this device is acted on. Without
    // this the test above would pass on a device that answers nothing at all.
    let addressed = phone_control_frame(&phone, &device_address, body);
    let handled = device
        .handle(&addressed, NOW)
        .expect("device handles a frame addressed to it");
    assert_eq!(
        handled.outbound.len(),
        1,
        "a frame addressed to this device was not answered: {:?}",
        handled.events
    );
    assert!(
        device.peers().expect("peers").contains(&phone.address),
        "a frame addressed to this device did not record its sender"
    );
}

#[test]
fn a_sealed_frame_addressed_elsewhere_is_not_opened() {
    let phone = new_phone();
    let bystander = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    let device_address = pair(&phone, &mut device);

    // A frame this device really can open, addressed to somebody else. The
    // data plane carries no signature, by design: MLS authenticates its own
    // sender, so the AEAD covers the ciphertext and nothing covers the
    // addressing beside it. Anyone who captured this frame can therefore
    // rewrite its recipient and hand it back.
    //
    // Openable is not the same question as addressed here, and without the
    // dispatch gate the device never asks the second one: it opens the
    // ciphertext and reports an ordinary message. Firmware that also carries
    // frames for its neighbours would then act on the same frame it forwards.
    let sealed = phone
        .manager
        .encrypt_for_user(&device_address, b"unlock")
        .expect("phone seals");
    let elsewhere = phone_sealed_frame(&phone, &bystander.address, &sealed);

    let handled = device
        .handle(&elsewhere, NOW)
        .expect("a frame addressed elsewhere is not a failure");
    assert!(
        matches!(handled.events.as_slice(), [LeafEvent::Ignored { .. }]),
        "a sealed frame addressed elsewhere produced {:?}",
        handled.events
    );

    // The control: the same ciphertext addressed here opens. What the device
    // refused was the addressing rather than the frame.
    let addressed = phone_sealed_frame(&phone, &device_address, &sealed);
    let handled = device.handle(&addressed, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "unlock".to_string(),
        }]
    );
}

#[test]
fn traffic_between_two_neighbours_is_ignored_rather_than_reported_as_a_failure() {
    let phone = new_phone();
    let bystander = new_phone();
    let mut device = device(Arc::new(MemoryStore::new()));
    pair(&phone, &mut device);

    // Two other nodes talking, overheard. This device is in neither end of it
    // and cannot open a byte, so nothing here is a confidentiality question.
    // What it is is a reporting one: every gate downstream refuses this as an
    // identity binding failure, and on a device whose only account of itself
    // is its error stream that makes ordinary neighbour traffic arrive wearing
    // the shape of an attack on it.
    let stranger = new_phone();
    let envelope = EncryptedMessage {
        group_id: GroupId::for_session(&stranger.address, &bystander.address)
            .expect("their pair's group id"),
        message_type: offline_protocol_sealed::MlsMessageType::Application,
        epoch: 1,
        ciphertext: vec![9, 9, 9],
        sender_id: stranger.address.clone(),
        timestamp_ms: 0,
    };
    let overheard = phone_sealed_frame(&stranger, &bystander.address, &envelope);

    let handled = device
        .handle(&overheard, NOW)
        .expect("two neighbours talking is not this device's failure");
    assert!(
        matches!(handled.events.as_slice(), [LeafEvent::Ignored { .. }]),
        "overheard neighbour traffic produced {:?}",
        handled.events
    );

    // The control: the device still has its own working session, so this is a
    // device that ignores what is not its business rather than one that has
    // stopped listening.
    let sealed = phone
        .manager
        .encrypt_for_user(&device.address().to_string(), b"unlock")
        .expect("phone seals");
    let frame = phone_sealed_frame(&phone, &device.address().to_string(), &sealed);
    let handled = device.handle(&frame, NOW).expect("device opens");
    assert_eq!(
        handled.events,
        vec![LeafEvent::MessageReceived {
            peer: phone.address.clone(),
            text: "unlock".to_string(),
        }]
    );
}

/// A store that writes and reads normally and refuses every delete.
///
/// The shape a flash part takes when erasing a sector fails: the write path is
/// fine and the reclaim path is not.
#[derive(Default)]
struct UndeletableStore {
    inner: MemoryStore,
    keys: Mutex<Vec<(String, String)>>,
}

impl UndeletableStore {
    /// The key ids held under one key type, in insertion order.
    fn keys_of(&self, key_type: &str) -> Vec<String> {
        self.keys
            .lock()
            .expect("lock")
            .iter()
            .filter(|(held, _)| held == key_type)
            .map(|(_, id)| id.clone())
            .collect()
    }
}

impl LeafStore for UndeletableStore {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> Result<(), StoreError> {
        self.inner.store(key_type, key_id, data)?;
        let mut keys = self.keys.lock().expect("lock");
        if !keys.iter().any(|(t, i)| t == key_type && i == key_id) {
            keys.push((key_type.to_string(), key_id.to_string()));
        }
        Ok(())
    }

    fn load(&self, key_type: &str, key_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.inner.load(key_type, key_id)
    }

    fn delete(&self, _key_type: &str, _key_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Delete("the sector will not erase".to_string()))
    }
}

#[test]
fn a_failed_eviction_leaves_no_key_package_the_index_has_forgotten() {
    // Where the list of unspent packages lives. Mirrored from the adapter,
    // which keeps it private; it cannot collide with a package id because
    // every one of those is hex and `_` is not a hex digit.
    const KEY_PACKAGE_INDEX: &str = "__index__";
    /// What the adapter keeps, past which a mint evicts the oldest.
    const MAX_UNSPENT: usize = 4;

    let store = Arc::new(UndeletableStore::default());
    let mut device = device(Arc::clone(&store) as Arc<dyn LeafStore>);

    // Fill the window. Nothing is evicted yet, so no delete is attempted and
    // the failing erase is not in play.
    let peers: Vec<String> = (0..MAX_UNSPENT).map(|_| new_phone().address).collect();
    for peer in &peers {
        device
            .key_package_frame(peer, NOW)
            .expect("device mints a key package");
    }
    let held: Vec<String> = store
        .keys_of(KEY_TYPE_KEY_PACKAGE)
        .into_iter()
        .filter(|id| id != KEY_PACKAGE_INDEX)
        .collect();
    assert_eq!(
        held.len(),
        MAX_UNSPENT,
        "the window did not fill, so the eviction below would never run: {held:?}"
    );

    // One more. This one evicts, the erase fails, and the mint fails with it.
    let err = device
        .key_package_frame(&new_phone().address, NOW)
        .expect_err("a package was minted although its eviction could not be erased");

    // The invariant: nothing is on flash that the index has stopped naming.
    // A package the index still names but which is gone costs one slot and is
    // evicted in its turn; the reverse is private key material nothing
    // reclaims, because no sweep covers this key type and an eviction the
    // index has forgotten is never attempted again.
    let raw = store
        .load(KEY_TYPE_KEY_PACKAGE, KEY_PACKAGE_INDEX)
        .expect("load")
        .expect("minting wrote an index");
    let index: Vec<String> = serde_json::from_slice(&raw).expect("the index parses");
    let orphaned: Vec<String> = store
        .keys_of(KEY_TYPE_KEY_PACKAGE)
        .into_iter()
        .filter(|id| id != KEY_PACKAGE_INDEX)
        .filter(|id| !index.contains(id))
        .collect();
    assert!(
        orphaned.is_empty(),
        "a failed eviction left key package private material no index names: \
         {orphaned:?} (error was {err:?})"
    );
}
