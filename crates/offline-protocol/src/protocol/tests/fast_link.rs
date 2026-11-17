//! The fast link through two real engines (ADR 0038): a genuine 1:1 MLS
//! session, frames sealed by one engine and decrypted by the other, carried
//! over a mock Bluetooth LE link. The state machine's own transitions are
//! tested in `crate::fast_link`; these are about what the engine adds: the
//! gates on a request, sealing, routing, events and directives.

use std::sync::{Arc, Mutex};

use offline_protocol_core::Message;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::fast_link::FastLinkDirectiveKind;
use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::{OfflineProtocol, SessionState, TestProtocolStateStorage};
use crate::{Error, Event};

struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Node {
    fn new(label: &str) -> Self {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = true;
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

    fn take_sent(&self) -> Vec<Message> {
        let sent = self.transport.sent_messages();
        self.transport.clear_sent_messages();
        sent
    }

    fn deliver(&mut self, frames: Vec<Message>) {
        for frame in frames {
            self.transport.queue_message(frame);
        }
        while self.protocol.receive_message().is_some() {}
    }

    /// `(state, reason)` of every fast-link event so far, in order.
    fn link_states(&self) -> Vec<(String, Option<String>)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::FastLinkChanged { state, reason, .. } => {
                    Some((state.clone(), reason.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

/// Two engines with a confirmed session, a live Bluetooth link between them,
/// and the fast-link capability recorded on both sides. `alice` can host and
/// join, as an Android phone can; `bob` can only join, as an iPhone.
fn pair() -> (Node, Node) {
    let mut alice = Node::new("alice");
    let mut bob = Node::new("bob");
    let bob_kp = {
        let manager = bob.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package(&bob.address, &bob_kp.key_package_data)
            .unwrap();
        manager.create_session(&bob.address).unwrap()
    };
    {
        let manager = bob.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    for (node, peer) in [(&mut alice, bob.address.clone()), (&mut bob, id("alice"))] {
        node.protocol.record_encryption_capable(&peer);
        node.protocol
            .persist_session_state(&peer, SessionState::Confirmed, "test")
            .unwrap();
        node.protocol.confirmed_sessions.insert(peer.clone());
        node.protocol.peer_fast_link.insert(peer.clone());
        node.transport.add_connected_peer(peer, -40);
    }
    alice.protocol.set_fast_link_roles(true, true);
    bob.protocol.set_fast_link_roles(false, true);
    (alice, bob)
}

#[test]
fn an_offer_is_sealed_carried_once_and_joined() {
    let (mut alice, mut bob) = pair();

    let link_id = alice.protocol.request_fast_link(&bob.address).unwrap();
    let host = alice.protocol.next_fast_link_directive().unwrap();
    assert_eq!(host.kind, FastLinkDirectiveKind::Host);
    assert_eq!(host.peer_id, bob.address);
    let passphrase = host.passphrase.clone().unwrap();
    assert!(
        alice.take_sent().is_empty(),
        "nothing reaches the peer before the network is up"
    );

    alice
        .protocol
        .fast_link_hosting(&link_id, "192.168.49.1", 8988)
        .unwrap();
    let sent = alice.take_sent();
    assert_eq!(sent.len(), 1, "one frame, sent once");
    let offer = &sent[0];
    assert!(offer.content.starts_with(internal_prefixes::ENCRYPTED));
    assert!(
        !offer.content.contains(&passphrase),
        "the passphrase left in clear"
    );
    assert!(!offer.requires_ack, "a fast-link frame is ephemeral");

    bob.deliver(sent);
    let join = bob.protocol.next_fast_link_directive().unwrap();
    assert_eq!(join.kind, FastLinkDirectiveKind::Join);
    assert_eq!(join.link_id, link_id);
    assert_eq!(join.peer_id, alice.address);
    assert_eq!(join.ssid, host.ssid);
    assert_eq!(join.passphrase.as_deref(), Some(passphrase.as_str()));
    assert_eq!(join.host.as_deref(), Some("192.168.49.1"));
    assert_eq!(join.port, Some(8988));
    assert!(bob.take_sent().is_empty(), "an offer is not acknowledged");

    alice.protocol.fast_link_stream_proved(&bob.address);
    bob.protocol.fast_link_stream_proved(&alice.address);
    assert_eq!(
        alice.link_states(),
        [
            ("hosting".to_string(), None),
            ("offered".to_string(), None),
            ("up".to_string(), None)
        ]
    );
    assert_eq!(
        bob.link_states(),
        [("joining".to_string(), None), ("up".to_string(), None)]
    );

    // The joiner's platform loses the network: both ends close, and the
    // host's platform is told to take its network down.
    bob.protocol.fast_link_ended(&link_id, "background");
    alice.deliver(bob.take_sent());
    let close = alice.protocol.next_fast_link_directive().unwrap();
    assert_eq!(close.kind, FastLinkDirectiveKind::Close);
    assert_eq!(
        alice.link_states().last().unwrap(),
        &("closed".to_string(), Some("background".to_string()))
    );
    assert!(alice.take_sent().is_empty(), "a close is never echoed");
    assert!(
        alice
            .events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !format!("{event:?}").contains(&passphrase)),
        "an event carried the passphrase"
    );
}

#[test]
fn a_joiner_asks_and_the_host_answers_under_its_id() {
    let (mut alice, mut bob) = pair();

    let link_id = bob.protocol.request_fast_link(&alice.address).unwrap();
    assert!(
        bob.protocol.next_fast_link_directive().is_none(),
        "a requester's platform has nothing to do yet"
    );
    alice.deliver(bob.take_sent());

    let host = alice.protocol.next_fast_link_directive().unwrap();
    assert_eq!(host.kind, FastLinkDirectiveKind::Host);
    assert_eq!(host.link_id, link_id);
    alice
        .protocol
        .fast_link_hosting(&link_id, "192.168.49.1", 8988)
        .unwrap();
    bob.deliver(alice.take_sent());

    let join = bob.protocol.next_fast_link_directive().unwrap();
    assert_eq!(join.kind, FastLinkDirectiveKind::Join);
    assert_eq!(join.link_id, link_id);
}

#[test]
fn a_request_is_refused_without_capability_session_or_roles() {
    let (mut alice, bob) = pair();

    alice.protocol.peer_fast_link.remove(&bob.address);
    assert!(matches!(
        alice.protocol.request_fast_link(&bob.address),
        Err(Error::InvalidArgument(_))
    ));
    alice.protocol.peer_fast_link.insert(bob.address.clone());

    alice.protocol.confirmed_sessions.remove(&bob.address);
    assert!(matches!(
        alice.protocol.request_fast_link(&bob.address),
        Err(Error::InvalidState(_))
    ));
    alice
        .protocol
        .confirmed_sessions
        .insert(bob.address.clone());

    alice.protocol.set_fast_link_roles(false, false);
    assert!(matches!(
        alice.protocol.request_fast_link(&bob.address),
        Err(Error::InvalidState(_))
    ));

    alice.transport.remove_connected_peer(&bob.address);
    alice.protocol.set_fast_link_roles(true, true);
    assert!(
        alice.protocol.request_fast_link(&bob.address).is_err(),
        "a request needs a live direct link to carry it"
    );
    assert!(alice.take_sent().is_empty());
    assert!(alice.protocol.next_fast_link_directive().is_none());
}

#[test]
fn a_peer_that_cannot_host_says_so() {
    let (mut alice, mut bob) = pair();
    alice.protocol.set_fast_link_roles(false, true);

    bob.protocol.request_fast_link(&alice.address).unwrap();
    alice.deliver(bob.take_sent());
    assert!(alice.protocol.next_fast_link_directive().is_none());
    bob.deliver(alice.take_sent());
    assert_eq!(
        bob.link_states().last().unwrap(),
        &("closed".to_string(), Some("cannot_host".to_string()))
    );
}

/// A transfer pinned to the peer-stream slot leaves eight 64 KiB chunks before
/// it waits for an acknowledgement, 512 KiB, not four of 32 KiB: at that, a
/// fast link spent its time waiting on receipts rather than on the link (ADR
/// 0038, decision 9). Eight and not more, because each chunk is a ratchet
/// generation.
#[test]
fn a_transfer_on_the_peer_stream_slot_keeps_512_kib_in_flight() {
    use offline_protocol_core::ContentType;
    use offline_protocol_transport::PeerLink;

    use crate::constants::{CHUNK_SIZE_PEER_STREAM, MEDIA_WINDOW_SIZE_PEER_STREAM};

    let (mut alice, bob) = pair();
    let stream = MockTransport::new(TransportType::WiFiDirect);
    stream.start().unwrap();
    stream.set_connected_peers(vec![PeerLink::new(bob.address.clone())]);
    alice
        .protocol
        .transport_manager_mut()
        .add_transport(TransportType::WiFiDirect, Box::new(stream.clone()));
    alice.take_sent();

    let file = vec![0x5a; CHUNK_SIZE_PEER_STREAM * 20];
    alice
        .protocol
        .send_media(&bob.address, file, "big.bin", ContentType::File, None)
        .unwrap();

    let chunks = stream
        .sent_messages()
        .into_iter()
        .chain(stream.peer_sends().into_iter().map(|(_, message)| message))
        .filter(|message| message.content_type == ContentType::FileChunk)
        .count();
    assert_eq!(chunks, MEDIA_WINDOW_SIZE_PEER_STREAM);
    assert_eq!(
        MEDIA_WINDOW_SIZE_PEER_STREAM * CHUNK_SIZE_PEER_STREAM,
        512 * 1024
    );
}

/// A fast-link frame goes over Bluetooth LE when the peer is linked there,
/// even though the engine prefers a peer stream for a direct link: the frame
/// may be ending that very stream, and on two phones a close sent down it
/// arrived 22 s late.
#[test]
fn fast_link_frames_prefer_bluetooth_over_the_peer_stream() {
    use offline_protocol_transport::PeerLink;

    let (mut alice, mut bob) = pair();
    let link_id = alice.protocol.request_fast_link(&bob.address).unwrap();
    // A peer stream to bob comes up before the offer leaves: a LAN one, say,
    // or the engine's own preference would carry the frame down it.
    let stream = MockTransport::new(TransportType::WiFiDirect);
    stream.start().unwrap();
    stream.set_connected_peers(vec![PeerLink::new(bob.address.clone())]);
    alice
        .protocol
        .transport_manager_mut()
        .add_transport(TransportType::WiFiDirect, Box::new(stream.clone()));
    assert_eq!(
        alice
            .protocol
            .transport_manager()
            .mesh_transport_for(&bob.address),
        Some(TransportType::WiFiDirect),
        "the engine prefers the stream for a direct link"
    );

    alice
        .protocol
        .fast_link_hosting(&link_id, "192.168.49.1", 8988)
        .unwrap();
    let over_bluetooth = alice.take_sent();
    assert_eq!(over_bluetooth.len(), 1, "the offer went over Bluetooth");
    let over_stream = stream
        .sent_messages()
        .into_iter()
        .chain(stream.peer_sends().into_iter().map(|(_, message)| message))
        .count();
    assert_eq!(over_stream, 0, "nothing went down the peer stream");
    bob.deliver(over_bluetooth);
    assert!(bob.protocol.next_fast_link_directive().is_some());
}

/// Blocking a peer ends its fast link at once: the platform takes the network
/// down, and nothing is sent to the blocked peer.
#[test]
fn blocking_a_peer_ends_its_fast_link() {
    let (mut alice, bob) = pair();
    let link_id = alice.protocol.request_fast_link(&bob.address).unwrap();
    alice.protocol.next_fast_link_directive();
    alice
        .protocol
        .fast_link_hosting(&link_id, "192.168.49.1", 8988)
        .unwrap();
    alice.protocol.fast_link_stream_proved(&bob.address);
    alice.take_sent();

    alice.protocol.block_user(&bob.address).unwrap();
    let close = alice.protocol.next_fast_link_directive().unwrap();
    assert_eq!(close.kind, FastLinkDirectiveKind::Close);
    assert_eq!(close.link_id, link_id);
    assert!(
        alice.take_sent().is_empty(),
        "nothing goes to a blocked peer"
    );
    assert_eq!(
        alice.link_states().last().unwrap(),
        &("closed".to_string(), Some("closed".to_string()))
    );
}

/// A transfer started over Bluetooth before the link came up stays on
/// Bluetooth, so it does not hold the link off its idle limit; one pinned to
/// the peer-stream slot does.
#[test]
fn only_a_transfer_on_the_peer_stream_holds_the_link() {
    use offline_protocol_core::ContentType;

    let (mut alice, bob) = pair();
    alice
        .protocol
        .send_media(
            &bob.address,
            vec![0x5a; 64 * 1024],
            "slow.bin",
            ContentType::File,
            None,
        )
        .unwrap();
    let on_bluetooth = alice
        .protocol
        .outbound_media_transfers
        .values()
        .next()
        .unwrap();
    assert_eq!(on_bluetooth.pinned_transport, TransportType::BLE);
    assert!(!OfflineProtocol::holds_fast_link(on_bluetooth));

    let mut on_stream = on_bluetooth.clone();
    on_stream.pinned_transport = TransportType::WiFiDirect;
    assert!(OfflineProtocol::holds_fast_link(&on_stream));
}

/// Pausing the engine ends a link: a paused engine runs no timers, so an
/// Android host's group would otherwise stay up until the app returned.
#[test]
fn pausing_the_engine_ends_a_link() {
    let (mut alice, mut bob) = pair();
    let link_id = alice.protocol.request_fast_link(&bob.address).unwrap();
    alice.protocol.next_fast_link_directive();
    alice
        .protocol
        .fast_link_hosting(&link_id, "192.168.49.1", 8988)
        .unwrap();
    bob.deliver(alice.take_sent());

    alice.protocol.pause().unwrap();
    let close = alice.protocol.next_fast_link_directive().unwrap();
    assert_eq!(close.kind, FastLinkDirectiveKind::Close);
    assert_eq!(
        alice.link_states().last().unwrap(),
        &("closed".to_string(), Some("background".to_string()))
    );
    // The peer is told, so its half ends too.
    bob.protocol.next_fast_link_directive();
    bob.deliver(alice.take_sent());
    assert_eq!(
        bob.link_states().last().unwrap(),
        &("closed".to_string(), Some("background".to_string()))
    );
}
