//! Service request and response bodies sealed inside the 1:1 MLS session
//! (issue #533).
//!
//! What is pinned: between two peers that both advertise `svc_versions`
//! entry 1, neither body is readable on the wire; the provider's
//! `service_request_received.sender` is the MLS-authenticated sender; a peer
//! that does not advertise it still gets signed plaintext; a request before
//! the session confirms falls back and starts the session; the automatic
//! answer to a sealed request is sealed even on the owner's first decrypt; a
//! sealed request retried after a re-key is sealed again rather than
//! replayed dead; a peer that sent a sealed frame stays sealed through a
//! replayed key package and a restart; a seal failure on a confirmed session
//! is an error rather than plaintext; and the service events say which form
//! arrived.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use offline_protocol_core::{Message, MessagePriority, ServiceDescriptor, ServiceId};
use offline_protocol_services::{
    SVC_DISCOVER_QUERY, SVC_MESSAGE_PREFIX, SVC_REQUEST, SVC_RESPONSE,
};
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use super::{
    create_test_config, create_test_config_for_user, establish_confirmed_session, signed_frame,
};
use crate::mls::{InMemoryStorage, MlsStorage};
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::types::KeyPackagePayload;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage, MAX_ENCRYPTION_CAPABLE_PEERS};
use crate::test_identity::id;
use crate::{Event, ProtocolConfig};
use offline_protocol_sealed::SVC_SEALED_V1;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// One device: a protocol on a mock link, with every event it emitted.
struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Node {
    fn new(label: &str, config: ProtocolConfig) -> Self {
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        protocol
            .initialize_mls(
                crate::test_identity::seeded_storage(label),
                Arc::new(TestProtocolStateStorage {
                    storage: Arc::new(InMemoryStorage::new()),
                }),
            )
            .expect("initialize_mls");
        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));
        protocol.start().expect("start");
        Self {
            protocol,
            transport,
            address: id(label),
            events,
        }
    }

    fn register(&mut self, service: &str) {
        self.protocol
            .register_service(ServiceDescriptor {
                service_id: ServiceId::new(service).unwrap(),
                version: "1.0".to_string(),
                capabilities: HashMap::new(),
            })
            .unwrap();
    }

    fn requests(&self) -> Vec<(String, String, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::ServiceRequestReceived {
                    request_id,
                    body,
                    sender,
                    ..
                } => Some((request_id.clone(), body.clone(), sender.clone())),
                _ => None,
            })
            .collect()
    }

    fn responses(&self) -> Vec<(String, String, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::ServiceResponseReceived {
                    status,
                    body,
                    provider_peer_id,
                    ..
                } => Some((status.clone(), body.clone(), provider_peer_id.clone())),
                _ => None,
            })
            .collect()
    }

    fn chat(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::MessageReceived { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Two devices in range of each other, every frame recorded as it crosses.
struct Pair {
    alice: Node,
    bob: Node,
    /// Every frame that crossed the link, as `(from_alice, frame)`.
    carried: Vec<(bool, Message)>,
}

fn config(label: &str) -> ProtocolConfig {
    let mut config = create_test_config_for_user(label);
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    config.encryption.store_pending = true;
    config
}

impl Pair {
    fn new(alice: ProtocolConfig, bob: ProtocolConfig) -> Self {
        let mut pair = Self {
            alice: Node::new("alice", alice),
            bob: Node::new("bob", bob),
            carried: Vec::new(),
        };
        let (a, b) = (pair.alice.address.clone(), pair.bob.address.clone());
        pair.alice.transport.add_connected_peer(b.clone(), -55);
        pair.bob.transport.add_connected_peer(a.clone(), -55);
        pair.alice.protocol.on_neighbor_discovered(&b);
        pair.bob.protocol.on_neighbor_discovered(&a);
        pair
    }

    /// One round: both devices handle what they hold and tick, then whatever
    /// each put on the link reaches the other.
    fn step(&mut self) -> usize {
        for node in [&mut self.alice, &mut self.bob] {
            while node.protocol.receive_message().is_some() {}
            node.protocol.last_reconciliation_at = None;
            node.protocol.process().expect("process");
        }
        let from_alice = self.alice.transport.sent_messages();
        self.alice.transport.clear_sent_messages();
        let from_bob = self.bob.transport.sent_messages();
        self.bob.transport.clear_sent_messages();
        let moved = from_alice.len() + from_bob.len();
        for frame in from_alice {
            self.carried.push((true, frame.clone()));
            self.bob
                .transport
                .queue_message_from(frame, self.alice.address.clone());
        }
        for frame in from_bob {
            self.carried.push((false, frame.clone()));
            self.alice
                .transport
                .queue_message_from(frame, self.bob.address.clone());
        }
        moved
    }

    fn run(&mut self, rounds: usize) {
        for _ in 0..rounds {
            self.step();
        }
    }

    fn sessions_confirmed(&self) -> bool {
        self.alice
            .protocol
            .confirmed_sessions
            .contains(&self.bob.address)
            && self
                .bob
                .protocol
                .confirmed_sessions
                .contains(&self.alice.address)
    }

    /// The frames that crossed in one direction since the last `clear`.
    fn crossed(&self, from_alice: bool) -> Vec<&Message> {
        self.carried
            .iter()
            .filter(|(direction, _)| *direction == from_alice)
            .map(|(_, frame)| frame)
            .collect()
    }
}

/// The `encrypted` flag of every service request and response event emitted.
fn service_event_flags(events: &Arc<Mutex<Vec<Event>>>) -> Vec<bool> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Event::ServiceRequestReceived { encrypted, .. }
            | Event::ServiceResponseReceived { encrypted, .. } => Some(*encrypted),
            _ => None,
        })
        .collect()
}

fn service_frames<'a>(frames: &[&'a Message]) -> Vec<&'a Message> {
    frames
        .iter()
        .copied()
        .filter(|frame| frame.content.starts_with(SVC_MESSAGE_PREFIX))
        .collect()
}

/// A started, MLS-enabled protocol on a mock link, with `tweak` applied to its
/// config first and a sink for every event it emits.
fn encrypted(
    label: &str,
    tweak: impl FnOnce(&mut ProtocolConfig),
) -> (OfflineProtocol, MockTransport, Arc<Mutex<Vec<Event>>>) {
    encrypted_on(label, Arc::new(InMemoryStorage::new()), tweak)
}

/// [`encrypted`] over a storage the test supplies.
fn encrypted_on(
    label: &str,
    storage: Arc<dyn MlsStorage>,
    tweak: impl FnOnce(&mut ProtocolConfig),
) -> (OfflineProtocol, MockTransport, Arc<Mutex<Vec<Event>>>) {
    let mut config = create_test_config_for_user(label);
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    tweak(&mut config);
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    protocol.on_event(move |event| sink.lock().unwrap().push(event));
    protocol.start().unwrap();
    (protocol, handle, events)
}

/// Hands every frame `from` sent to `to`, as arriving from `sender`, lets `to`
/// handle them, and returns what was handed.
fn deliver(
    from: &MockTransport,
    sender: &str,
    to: &mut OfflineProtocol,
    to_transport: &MockTransport,
) -> Vec<Message> {
    let frames = from.sent_messages();
    from.clear_sent_messages();
    for frame in frames.clone() {
        to_transport.queue_message_from(frame, sender.to_string());
    }
    while to.receive_message().is_some() {}
    frames
}

/// The frames that carry something, leaving out the delivery acknowledgements
/// (an empty body) a received request is answered with.
fn without_acks(frames: Vec<Message>) -> Vec<Message> {
    frames
        .into_iter()
        .filter(|frame| !frame.content.is_empty())
        .collect()
}

fn register(protocol: &mut OfflineProtocol, service: &str) {
    protocol
        .register_service(ServiceDescriptor {
            service_id: ServiceId::new(service).unwrap(),
            version: "1.0".to_string(),
            capabilities: HashMap::new(),
        })
        .unwrap();
}

/// A key package body from `sender` advertising `svc_versions`.
fn key_package_body(sender: &str, svc_versions: Vec<u8>) -> String {
    let payload = KeyPackagePayload {
        user_id: sender.to_string(),
        key_package_data: vec![1, 2, 3, 4],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
        data_versions: Vec::new(),
        ctrl_versions: Vec::new(),
        svc_versions,
        ack_versions: Vec::new(),
        nostr_pubkey: None,
    };
    serde_json::to_string(&payload).unwrap()
}

/// Feeds `protocol` a signed key package from `sender`, as the gate admits it.
fn feed_signed_key_package(protocol: &mut OfflineProtocol, sender: &str, svc_versions: Vec<u8>) {
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        key_package_body(sender, svc_versions)
    );
    let message = signed_frame(sender, &id("user123"), &content);
    protocol.process_internal_message(&message);
}

/// Records that `peer` advertised the sealed form, as a signed key package
/// that passed the gate would: the gate marks the signed sender
/// encryption-capable first, and the advertised set is a subset of that one.
fn advertise(protocol: &mut OfflineProtocol, peer: &str) {
    protocol.record_encryption_capable(peer);
    protocol.record_svc_sealed_peer(peer);
}

fn negotiating_protocol(tweak: impl FnOnce(&mut ProtocolConfig)) -> OfflineProtocol {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    tweak(&mut config);
    OfflineProtocol::new(config).unwrap()
}

// ---------------------------------------------------------------------------
// Negotiation
// ---------------------------------------------------------------------------

#[test]
fn svc_sealed_negotiation_marks_a_capable_peer() {
    let mut protocol = negotiating_protocol(|_| {});
    feed_signed_key_package(&mut protocol, &id("provider"), vec![SVC_SEALED_V1]);
    assert!(protocol.peer_svc_sealed.contains(&id("provider")));
}

#[test]
fn svc_sealed_negotiation_ignores_a_legacy_peer() {
    let mut protocol = negotiating_protocol(|_| {});
    feed_signed_key_package(&mut protocol, &id("provider"), Vec::new());
    assert!(!protocol.peer_svc_sealed.contains(&id("provider")));
}

#[test]
fn svc_sealed_negotiation_downgrade_removes_the_capability() {
    let mut protocol = negotiating_protocol(|_| {});
    feed_signed_key_package(&mut protocol, &id("provider"), vec![SVC_SEALED_V1]);
    assert!(protocol.peer_svc_sealed.contains(&id("provider")));
    feed_signed_key_package(&mut protocol, &id("provider"), Vec::new());
    assert!(
        !protocol.peer_svc_sealed.contains(&id("provider")),
        "a fresh key package that stops advertising it must stop the sealing"
    );
}

#[test]
fn svc_sealed_kill_switch_stops_recording_and_advertising() {
    let mut protocol = negotiating_protocol(|config| {
        config.encryption.encrypt_service_messages = false;
    });
    feed_signed_key_package(&mut protocol, &id("provider"), vec![SVC_SEALED_V1]);
    assert!(!protocol.peer_svc_sealed.contains(&id("provider")));

    let mut on = negotiating_protocol(|_| {});
    on.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let mut off = negotiating_protocol(|config| {
        config.encryption.encrypt_service_messages = false;
    });
    off.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let bundle = {
        let manager = on.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    assert_eq!(
        on.build_key_package_payload(&bundle, false).svc_versions,
        vec![SVC_SEALED_V1]
    );
    assert!(off
        .build_key_package_payload(&bundle, false)
        .svc_versions
        .is_empty());
}

/// The list sits beside a confidentiality decision, so it is honoured only
/// from a signed key package, like `nostr_pubkey`: neither recorded in memory
/// nor persisted for a restart to pick up.
#[test]
fn svc_sealed_is_never_learned_from_an_unsigned_key_package() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = super::protocol_with_mls_storage(storage.clone());
    protocol.handle_key_package_message(
        &id("provider"),
        &key_package_body(&id("provider"), vec![SVC_SEALED_V1]),
        false,
        None,
    );
    assert!(!protocol.peer_svc_sealed.contains(&id("provider")));

    let restarted = super::protocol_with_mls_storage(storage);
    assert!(
        !restarted.peer_svc_sealed.contains(&id("provider")),
        "an unsigned advertisement must not reach the durable record either"
    );
}

#[test]
fn svc_sealed_capability_persists_across_restart_and_respects_the_switch() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = super::protocol_with_mls_storage(storage.clone());
    feed_signed_key_package(&mut protocol, &id("provider"), vec![SVC_SEALED_V1]);
    assert!(protocol.peer_svc_sealed.contains(&id("provider")));

    let restarted = super::protocol_with_mls_storage(storage.clone());
    assert!(
        restarted.peer_svc_sealed.contains(&id("provider")),
        "a peer met before the restart must keep receiving sealed service bodies"
    );

    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.encrypt_service_messages = false;
    let mut gated = OfflineProtocol::new(config).unwrap();
    gated.initialize_mls_for_test(storage.clone()).unwrap();
    assert!(!gated.peer_svc_sealed.contains(&id("provider")));

    // The record survives the switched-off run, as for every capability.
    let again = super::protocol_with_mls_storage(storage);
    assert!(again.peer_svc_sealed.contains(&id("provider")));
}

// ---------------------------------------------------------------------------
// End to end, over a real handshake
// ---------------------------------------------------------------------------

/// The headline. Both peers learn each other's `svc_versions` from genuine key
/// packages, and then neither the request body nor the response body is
/// readable on the wire; the provider sees the MLS-authenticated requester.
#[test]
fn service_request_and_response_are_sealed_between_capable_peers() {
    let mut pair = Pair::new(config("alice"), config("bob"));
    pair.run(40);
    assert!(pair.sessions_confirmed(), "the handshake must converge");
    assert!(pair
        .alice
        .protocol
        .peer_svc_sealed
        .contains(&pair.bob.address));
    assert!(pair
        .bob
        .protocol
        .peer_svc_sealed
        .contains(&pair.alice.address));

    pair.bob.register("echo");
    pair.carried.clear();
    let bob = pair.bob.address.clone();
    let alice = pair.alice.address.clone();
    let request_id = pair
        .alice
        .protocol
        .send_service_request(&bob, "echo", "ping", "the-request-secret")
        .unwrap();
    pair.run(10);

    let requests = pair.bob.requests();
    assert_eq!(
        requests,
        vec![(
            request_id.clone(),
            "the-request-secret".to_string(),
            alice.clone()
        )],
        "the provider must receive the request once, from the authenticated requester"
    );
    for frame in pair.crossed(true) {
        assert!(
            !frame.content.contains("the-request-secret"),
            "the request body crossed the link readable: {}",
            frame.content
        );
    }
    assert!(service_frames(&pair.crossed(true)).is_empty());

    pair.carried.clear();
    pair.bob
        .protocol
        .respond_to_service_request(&request_id, &alice, "echo", "ok", "the-response-secret")
        .unwrap();
    pair.run(10);

    assert_eq!(
        pair.alice.responses(),
        vec![(
            "ok".to_string(),
            "the-response-secret".to_string(),
            bob.clone()
        )]
    );
    for frame in pair.crossed(false) {
        assert!(
            !frame.content.contains("the-response-secret"),
            "the response body crossed the link readable: {}",
            frame.content
        );
    }
    assert!(service_frames(&pair.crossed(false)).is_empty());
    assert!(
        pair.alice.chat().is_empty() && pair.bob.chat().is_empty(),
        "a sealed service frame must never surface as a chat message"
    );
    assert_eq!(
        service_event_flags(&pair.bob.events),
        vec![true],
        "the provider must be told the request arrived sealed"
    );
    assert_eq!(
        service_event_flags(&pair.alice.events),
        vec![true],
        "the requester must be told the response arrived sealed"
    );
}

/// A peer that does not advertise the sealed form (a release from before it,
/// modelled by the switch off on its side) is sent signed plaintext, and the
/// round trip still works: the plaintext floor is the interoperability
/// requirement.
#[test]
fn service_request_to_a_legacy_peer_stays_signed_plaintext() {
    let mut legacy = config("bob");
    legacy.encryption.encrypt_service_messages = false;
    let mut pair = Pair::new(config("alice"), legacy);
    pair.run(40);
    assert!(pair.sessions_confirmed());
    assert!(!pair
        .alice
        .protocol
        .peer_svc_sealed
        .contains(&pair.bob.address));

    pair.bob.register("echo");
    pair.carried.clear();
    let bob = pair.bob.address.clone();
    pair.alice
        .protocol
        .send_service_request(&bob, "echo", "ping", "plain-body")
        .unwrap();
    pair.run(10);

    let sent = service_frames(&pair.crossed(true));
    assert_eq!(sent.len(), 1);
    assert!(sent[0].content.starts_with(SVC_REQUEST));
    // The gate refuses unsigned control frames, so the provider handling it
    // proves it left signed.
    assert_eq!(pair.bob.requests().len(), 1);
    assert_eq!(
        service_event_flags(&pair.bob.events),
        vec![false],
        "a plaintext request must not be reported sealed"
    );
}

/// A request to a capable peer before the session confirms leaves as signed
/// plaintext, and is what starts the session (the encrypt attempt sends the
/// Welcome, exactly as for a direct message); once the session confirms, the
/// next request is sealed.
#[test]
fn a_request_before_the_session_confirms_falls_back_and_starts_the_session() {
    // Alice does not exchange key packages on her own, so she holds bob's
    // package (and knows he routes the sealed form) without a session.
    let mut quiet = config("alice");
    quiet.encryption.auto_key_exchange = false;
    let mut pair = Pair::new(quiet, config("bob"));
    let bob = pair.bob.address.clone();
    for _ in 0..10 {
        pair.step();
        if pair.alice.protocol.peer_svc_sealed.contains(&bob) {
            break;
        }
    }
    assert!(pair.alice.protocol.peer_svc_sealed.contains(&bob));
    assert!(!pair.alice.protocol.confirmed_sessions.contains(&bob));
    assert!(
        !pair.alice.protocol.has_mls_session(&bob).unwrap(),
        "no session yet: the request below is what must start it"
    );

    pair.bob.register("echo");
    pair.alice
        .protocol
        .send_service_request(&bob, "echo", "ping", "first")
        .unwrap();
    let sent = pair.alice.transport.sent_messages();
    let requests: Vec<&Message> = sent
        .iter()
        .filter(|frame| frame.content.starts_with(SVC_MESSAGE_PREFIX))
        .collect();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].content.starts_with(SVC_REQUEST),
        "with no confirmed session the request must leave signed, not be dropped"
    );
    assert_eq!(
        sent.iter()
            .filter(|frame| frame.content.starts_with(internal_prefixes::WELCOME))
            .count(),
        1,
        "the encrypt attempt must have started the session"
    );

    pair.run(40);
    assert_eq!(pair.bob.requests().len(), 1, "the first request arrived");
    assert!(pair.alice.protocol.confirmed_sessions.contains(&bob));

    pair.carried.clear();
    pair.alice
        .protocol
        .send_service_request(&bob, "echo", "ping", "second-secret")
        .unwrap();
    pair.run(10);
    assert_eq!(pair.bob.requests().len(), 2);
    for frame in pair.crossed(true) {
        assert!(!frame.content.contains("second-secret"));
    }
}

// ---------------------------------------------------------------------------
// The receive side
// ---------------------------------------------------------------------------

/// Bob owns the session and has decrypted nothing from alice yet, so it is not
/// confirmed on his side when her sealed request arrives. The automatic
/// `not_found` answer is sent from inside the handler, so it is sealed only
/// because the decrypt confirms the session before the frame is handled.
#[test]
fn the_automatic_reply_to_the_first_sealed_request_is_sealed() {
    let (mut alice, alice_h, alice_events) = encrypted("alice", |_| {});
    let (mut bob, bob_h, _) = encrypted("bob", |_| {});
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    bob.confirmed_sessions.remove(&id("alice"));
    advertise(&mut alice, &id("bob"));
    advertise(&mut bob, &id("alice"));

    alice
        .send_service_request(&id("bob"), "nobody-here", "ping", "x")
        .unwrap();
    let request = deliver(&alice_h, &id("alice"), &mut bob, &bob_h);
    assert!(request[0].content.starts_with(internal_prefixes::ENCRYPTED));

    let replies = deliver(&bob_h, &id("bob"), &mut alice, &alice_h);
    assert!(
        !replies.is_empty(),
        "the unknown service must be answered automatically"
    );
    for reply in &replies {
        assert!(
            !reply.content.starts_with(SVC_MESSAGE_PREFIX),
            "the automatic answer to a sealed request left in plaintext: {}",
            reply.content
        );
    }
    let statuses: Vec<String> = alice_events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            Event::ServiceResponseReceived { status, .. } => Some(status.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, vec!["not_found".to_string()]);
}

/// Discovery is never sealed. A sealed discovery query that arrives anyway is
/// dropped: routing it would have the receiver forward it onward as plaintext
/// gossip, and it must never surface as a message either.
#[test]
fn a_sealed_discovery_query_is_dropped_not_forwarded_or_surfaced() {
    let (mut alice, alice_h, _) = encrypted("alice", |_| {});
    let (mut bob, bob_h, bob_events) = encrypted("bob", |_| {});
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    register(&mut bob, "echo");
    // A third peer bob would forward a routed query to.
    bob.on_neighbor_discovered(&id("carol"));
    alice.on_neighbor_discovered(&id("bob"));

    alice_h.clear_sent_messages();
    alice.discover_services(Some("echo")).unwrap();
    let query = alice_h
        .sent_messages()
        .into_iter()
        .find(|frame| frame.content.starts_with(SVC_DISCOVER_QUERY))
        .expect("discovery sends a query")
        .content;
    alice_h.clear_sent_messages();
    let sealed = alice
        .encrypt_content_for_recipient(&id("bob"), &query, MessagePriority::Medium)
        .unwrap();
    alice
        .send_sealed_internal_message(&id("bob"), sealed, MessagePriority::Medium, None)
        .unwrap();

    bob_h.clear_sent_messages();
    deliver(&alice_h, &id("alice"), &mut bob, &bob_h);
    assert!(
        !bob_h
            .sent_messages()
            .iter()
            .any(|frame| frame.content.starts_with(SVC_MESSAGE_PREFIX)),
        "a sealed discovery query was routed and forwarded in plaintext"
    );
    assert!(!bob_events.lock().unwrap().iter().any(|event| matches!(
        event,
        Event::MessageReceived { .. } | Event::ServiceDiscovered { .. }
    )));
}

/// A sealed service frame proves its sender routes them, which a replayed key
/// package cannot, so bob records alice in the proved ratchet and answers
/// sealed even though he never received a key package from her that
/// advertised it.
#[test]
fn a_sealed_service_request_marks_its_sender_capable() {
    let (mut alice, alice_h, _) = encrypted("alice", |_| {});
    let (mut bob, bob_h, bob_events) = encrypted("bob", |_| {});
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    advertise(&mut alice, &id("bob"));
    assert!(!bob.peer_svc_sealed.contains(&id("alice")));
    register(&mut bob, "echo");

    let request_id = alice
        .send_service_request(&id("bob"), "echo", "ping", "x")
        .unwrap();
    deliver(&alice_h, &id("alice"), &mut bob, &bob_h);
    assert!(bob.svc_sealed_proved_peers.contains(&id("alice")));
    assert!(bob_events
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event, Event::ServiceRequestReceived { .. })));

    bob.respond_to_service_request(&request_id, &id("alice"), "echo", "ok", "y")
        .unwrap();
    let response = without_acks(bob_h.sent_messages());
    assert_eq!(response.len(), 1);
    assert!(response[0]
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
}

/// Routing a sealed service frame is parsing, and parsing is never gated by a
/// kill switch: with the switch off bob still hands alice's sealed request to
/// his service layer (rather than surfacing it as chat text). He records the
/// proof, which is knowledge rather than policy, but does not seal his answer.
#[test]
fn a_receiver_with_the_switch_off_still_routes_a_sealed_service_frame() {
    let (mut alice, alice_h, _) = encrypted("alice", |_| {});
    let (mut bob, bob_h, bob_events) = encrypted("bob", |config| {
        config.encryption.encrypt_service_messages = false;
    });
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    advertise(&mut alice, &id("bob"));
    register(&mut bob, "echo");

    let request_id = alice
        .send_service_request(&id("bob"), "echo", "ping", "x")
        .unwrap();
    let request = deliver(&alice_h, &id("alice"), &mut bob, &bob_h);
    assert!(request[0].content.starts_with(internal_prefixes::ENCRYPTED));

    let events = bob_events.lock().unwrap().clone();
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::ServiceRequestReceived { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event, Event::MessageReceived { .. })));
    assert!(!bob.peer_svc_sealed.contains(&id("alice")));
    assert!(bob.svc_sealed_proved_peers.contains(&id("alice")));

    bob.respond_to_service_request(&request_id, &id("alice"), "echo", "ok", "y")
        .unwrap();
    let response = without_acks(bob_h.sent_messages());
    assert_eq!(response.len(), 1);
    assert!(response[0].content.starts_with(SVC_RESPONSE));
}

// ---------------------------------------------------------------------------
// Resend after a re-key
// ---------------------------------------------------------------------------

/// A sealed request still in the outbox when the session is re-keyed is sealed
/// again against the new epoch on resend, not replayed as ciphertext nobody
/// can open: the frame stages the same re-seal provenance a direct message
/// does.
#[test]
fn a_sealed_service_request_is_resealed_on_resend_after_a_rekey() {
    let (mut alice, _alice_h, _) = encrypted("alice", |config| {
        // The JSON envelope, for a deterministic decode below.
        config.encryption.compact_envelope_enabled = false;
    });
    let bob =
        crate::test_identity::manager_for("bob", Arc::new(crate::mls::InMemoryStorage::new()));
    let join = |alice: &mut OfflineProtocol| {
        let package = bob.get_or_create_key_package().unwrap();
        let welcome = {
            let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
            manager
                .import_key_package(&id("bob"), &package.key_package_data)
                .unwrap();
            manager.create_session(&id("bob")).unwrap()
        };
        bob.join_session(&welcome).unwrap();
        alice.confirmed_sessions.insert(id("bob"));
    };
    join(&mut alice);
    advertise(&mut alice, &id("bob"));

    alice
        .send_service_request(&id("bob"), "echo", "ping", "retry-me")
        .unwrap();
    let entry = alice
        .outbox
        .values()
        .find(|entry| entry.message.recipient.as_str() == id("bob"))
        .expect("a sealed service frame must enter the outbox")
        .clone();
    assert!(entry
        .message
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
    let reseal = entry
        .reseal
        .as_ref()
        .expect("a sealed service frame must store re-seal provenance");
    assert!(reseal.content.starts_with(SVC_REQUEST));

    // Re-key exactly as the desync heal does: both sessions torn down and a
    // fresh one built at a new epoch.
    alice.manual_mls_delete_session(&id("bob")).unwrap();
    bob.delete_session(&id("alice")).unwrap();
    join(&mut alice);

    let mut resend = entry.message.clone();
    alice.reseal_for_resend_in_place(&mut resend);
    assert_ne!(
        resend.content, entry.message.content,
        "the resend must be sealed afresh"
    );
    assert_eq!(resend.id, entry.message.id);
    let sealed: offline_protocol_mls::EncryptedMessage = serde_json::from_str(
        resend
            .content
            .strip_prefix(internal_prefixes::ENCRYPTED)
            .unwrap(),
    )
    .unwrap();
    let plaintext = bob
        .decrypt_from_user(&sealed, &id("alice"))
        .unwrap()
        .unwrap();
    let plaintext = String::from_utf8(plaintext).unwrap();
    assert!(plaintext.starts_with(SVC_REQUEST) && plaintext.contains("retry-me"));
}

// ---------------------------------------------------------------------------
// The require_encryption exemption, per frame class
// ---------------------------------------------------------------------------

fn strict(label: &str) -> (OfflineProtocol, MockTransport, Arc<Mutex<Vec<Event>>>) {
    encrypted(label, |config| {
        config.encryption.require_encryption = true;
    })
}

/// Discovery is a gossip every hop reads, so it stays signed plaintext even
/// under `require_encryption`, toward any peer.
#[test]
fn require_encryption_sends_discovery_as_signed_plaintext() {
    let (mut alice, alice_h, _) = strict("alice");
    alice.on_neighbor_discovered(&id("bob"));
    alice_h.clear_sent_messages();
    alice.discover_services(None).unwrap();
    let sent = alice_h.sent_messages();
    assert!(!sent.is_empty());
    assert!(sent
        .iter()
        .all(|frame| frame.content.starts_with(SVC_DISCOVER_QUERY)));
}

/// Toward a peer that has not advertised the sealed form, requests and
/// responses stay signed plaintext under `require_encryption`: the exemption
/// for service frames still holds there.
#[test]
fn require_encryption_sends_service_frames_to_a_legacy_peer_as_signed_plaintext() {
    let (mut alice, alice_h, _) = strict("alice");
    alice
        .send_service_request(&id("bob"), "echo", "ping", "{}")
        .unwrap();
    alice
        .respond_to_service_request("req-1", &id("bob"), "echo", "ok", "pong")
        .unwrap();
    let sent: Vec<Message> = alice_h
        .sent_messages()
        .into_iter()
        .filter(|frame| frame.content.starts_with(SVC_MESSAGE_PREFIX))
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].content.starts_with(SVC_REQUEST));
    assert!(sent[1].content.starts_with(SVC_RESPONSE));
}

/// Toward a capable peer with a confirmed session, requests and responses are
/// sealed under `require_encryption` as everywhere else.
#[test]
fn require_encryption_seals_service_frames_to_a_capable_confirmed_peer() {
    let (mut alice, alice_h, _) = strict("alice");
    let (mut bob, _bob_h, _) = strict("bob");
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    advertise(&mut alice, &id("bob"));
    alice_h.clear_sent_messages();

    alice
        .send_service_request(&id("bob"), "echo", "ping", "{}")
        .unwrap();
    alice
        .respond_to_service_request("req-1", &id("bob"), "echo", "ok", "pong")
        .unwrap();
    let sent = alice_h.sent_messages();
    assert_eq!(sent.len(), 2);
    assert!(sent
        .iter()
        .all(|frame| frame.content.starts_with(internal_prefixes::ENCRYPTED)));
}

// ---------------------------------------------------------------------------
// The proof ratchet
// ---------------------------------------------------------------------------

/// The review finding this pins: a signed key package that does not advertise
/// the sealed form, replayed after the peer has sent us a sealed service
/// frame, must not send that peer's service bodies in plaintext again. The
/// advertised half follows the key package; the proved half does not, and
/// sealing needs only one of them.
#[test]
fn a_replayed_key_package_cannot_unseal_a_peer_that_sent_a_sealed_frame() {
    let (mut alice, alice_h, _) = encrypted("alice", |_| {});
    let (mut bob, bob_h, _) = encrypted("bob", |_| {});
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    advertise(&mut alice, &id("bob"));
    register(&mut bob, "echo");

    let request_id = alice
        .send_service_request(&id("bob"), "echo", "ping", "x")
        .unwrap();
    deliver(&alice_h, &id("alice"), &mut bob, &bob_h);
    assert!(bob.svc_sealed_proved_peers.contains(&id("alice")));

    // A key package from before alice upgraded, signed by her and replayed.
    let replay = signed_frame(
        &id("alice"),
        &id("bob"),
        format!(
            "{}{}",
            internal_prefixes::KEY_PACKAGE,
            key_package_body(&id("alice"), Vec::new())
        ),
    );
    bob.process_internal_message(&replay);
    assert!(
        !bob.peer_svc_sealed.contains(&id("alice")),
        "the advertised half follows the key package"
    );
    assert!(bob.svc_sealed_proved_peers.contains(&id("alice")));

    bob_h.clear_sent_messages();
    bob.respond_to_service_request(&request_id, &id("alice"), "echo", "ok", "y")
        .unwrap();
    let sent = bob_h.sent_messages();
    assert!(
        !sent
            .iter()
            .any(|frame| frame.content.starts_with(SVC_MESSAGE_PREFIX)),
        "the replay downgraded the response to plaintext"
    );
    assert!(sent
        .iter()
        .any(|frame| frame.content.starts_with(internal_prefixes::ENCRYPTED)));
}

/// The proof survives a restart, and survives every other write of the
/// record it lives in: each one rewrites the whole record, so a writer that
/// forgot the field would clear the ratchet silently.
#[test]
fn the_sealed_service_proof_is_durable_and_no_other_write_clears_it() {
    let storage = Arc::new(InMemoryStorage::new());
    let peer = id("provider");
    let mut protocol = super::protocol_with_mls_storage(storage.clone());
    protocol.record_encryption_capable(&peer);
    protocol.record_svc_sealed_proved(&peer);
    protocol.record_reset_spent(&peer, 5);
    protocol.record_control_freshness_proved(&peer);

    let mut restarted = super::protocol_with_mls_storage(storage);
    assert!(
        restarted.svc_sealed_proved_peers.contains(&peer),
        "a restart must not cost a proved peer its sealed service bodies"
    );
    feed_signed_key_package(&mut restarted, &peer, Vec::new());
    assert!(restarted.svc_sealed_proved_peers.contains(&peer));
}

/// Refused outside the capped encryption-capable set, like the
/// control-freshness ratchet it sits beside, so the set the cap bounds also
/// bounds this one.
#[test]
fn the_sealed_service_proof_is_a_subset_of_the_capable_set() {
    let mut protocol = super::protocol_with_mls_storage(Arc::new(InMemoryStorage::new()));
    protocol.record_svc_sealed_proved(&id("stranger"));
    assert!(!protocol.svc_sealed_proved_peers.contains(&id("stranger")));
}

/// The advertised set is a subset of the capped encryption-capable set and
/// refuses at that cap rather than evicting. The failure this pins: a set that
/// cleared itself when full let a flood of fresh identities, one keygen each,
/// send every established peer's service bodies in plaintext.
#[test]
fn a_flood_of_identities_cannot_unseal_an_established_peer() {
    let mut protocol = negotiating_protocol(|_| {});
    advertise(&mut protocol, &id("established"));
    for i in 0..MAX_ENCRYPTION_CAPABLE_PEERS + 10 {
        advertise(&mut protocol, &format!("flood-{i}"));
    }
    assert!(
        protocol.peer_svc_sealed.contains(&id("established")),
        "the flood evicted an established peer from the advertised set"
    );
    assert!(protocol.peer_svc_sealed.len() <= MAX_ENCRYPTION_CAPABLE_PEERS);
    assert!(!protocol.peer_svc_sealed.contains("flood-8200"));
}

/// Outside the encryption-capable set the advertisement is refused, so the
/// cap that bounds that set bounds this one.
#[test]
fn the_advertised_set_is_a_subset_of_the_capable_set() {
    let mut protocol = negotiating_protocol(|_| {});
    protocol.record_svc_sealed_peer(&id("stranger"));
    assert!(!protocol.peer_svc_sealed.contains(&id("stranger")));
}

// ---------------------------------------------------------------------------
// A seal failure on a confirmed session
// ---------------------------------------------------------------------------

/// MLS storage that refuses the group-state write encryption makes, on
/// demand, and passes everything else through, so a confirmed session fails
/// to seal while signing a plaintext frame still works.
struct GroupStateWriteFails {
    inner: InMemoryStorage,
    fail: AtomicBool,
}

impl MlsStorage for GroupStateWriteFails {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        if self.fail.load(Ordering::SeqCst)
            && key_type == offline_protocol_mls::types::StorageKeyType::GroupState.as_str()
        {
            return Err(offline_protocol_mls::StorageError::StoreFailed(
                "forced group-state write failure".to_string(),
            ));
        }
        self.inner.store(key_type, key_id, data)
    }

    fn load(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Option<Vec<u8>>> {
        self.inner.load(key_type, key_id)
    }

    fn delete(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.inner.delete(key_type, key_id)
    }

    fn list_keys(
        &self,
        key_type: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Vec<String>> {
        self.inner.list_keys(key_type)
    }
}

/// The fallback to signed plaintext is for a peer we cannot seal to yet. On a
/// confirmed session a seal failure is returned to the caller, as it is for a
/// direct message, and nothing readable leaves.
#[test]
fn a_seal_failure_on_a_confirmed_session_is_an_error_not_plaintext() {
    let storage = Arc::new(GroupStateWriteFails {
        inner: InMemoryStorage::new(),
        fail: AtomicBool::new(false),
    });
    let (mut alice, alice_h, _) = encrypted_on("alice", storage.clone(), |_| {});
    let (mut bob, _bob_h, _) = encrypted("bob", |_| {});
    establish_confirmed_session(&mut alice, &id("alice"), &mut bob, &id("bob"));
    advertise(&mut alice, &id("bob"));
    alice_h.clear_sent_messages();

    storage.fail.store(true, Ordering::SeqCst);
    let result = alice.send_service_request(&id("bob"), "echo", "ping", "must-not-leak");
    assert!(
        result.is_err(),
        "a seal failure on a confirmed session must be reported"
    );
    for frame in alice_h.sent_messages() {
        assert!(
            !frame.content.contains("must-not-leak"),
            "the body left readable: {}",
            frame.content
        );
    }
}

// ---------------------------------------------------------------------------
// A sealed frame that arrives before the session
// ---------------------------------------------------------------------------

/// A sealed request that reaches the provider before its session does is
/// queued undecrypted, and the drain that runs when the session is ready
/// routes it to the service layer like a live one: never surfaced as chat.
#[test]
fn a_sealed_request_queued_before_the_session_is_routed_when_it_drains() {
    let (mut alice, alice_h, _) = encrypted("alice", |_| {});
    let (mut bob, bob_h, bob_events) = encrypted("bob", |_| {});
    register(&mut bob, "echo");

    // Alice holds a session bob has not joined yet.
    let welcome = {
        let package = {
            let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
            manager.get_or_create_key_package().unwrap()
        };
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package(&id("bob"), &package.key_package_data)
            .unwrap();
        manager.create_session(&id("bob")).unwrap()
    };
    alice.confirmed_sessions.insert(id("bob"));
    advertise(&mut alice, &id("bob"));

    alice
        .send_service_request(&id("bob"), "echo", "ping", "early")
        .unwrap();
    let request = deliver(&alice_h, &id("alice"), &mut bob, &bob_h);
    assert!(request[0].content.starts_with(internal_prefixes::ENCRYPTED));
    assert!(
        service_event_flags(&bob_events).is_empty(),
        "with no session the frame must wait, not be handled"
    );

    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    bob.process_pending_decryption(&id("alice"));

    assert_eq!(service_event_flags(&bob_events), vec![true]);
    assert!(!bob_events
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event, Event::MessageReceived { .. })));
}
