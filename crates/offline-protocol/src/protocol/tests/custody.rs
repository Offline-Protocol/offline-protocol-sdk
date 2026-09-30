//! Custody over the real send, forward and receive paths.
//!
//! Three replicas: a depositor with a live session to a recipient it cannot
//! reach, a custodian in range of the depositor, and the recipient. Every
//! frame goes through MLS sealing, the transport, the forwarding governor and
//! the prefix dispatch, because the chapter's claims are about that machinery:
//! the deposit request rides the depositor's own sealed frame, acceptance
//! happens at the end of the forwarding queue, and redelivery is an ordinary
//! forward. What is pinned here is what a device can observe on its links and
//! in its store, plus the one thing a device must never do (blank its own
//! route) and the one thing a receipt must never do (settle anything).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use offline_protocol_data::DataValue;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::config::CustodyConfig;
use crate::mls::InMemoryStorage;
use crate::protocol::custody::{encode_receipt, CUSTODY_CLASS_DATA, CUSTODY_META_KEY};
use crate::protocol::mesh_relay::RELAY_QUEUE_MAX_OVERDUE;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::types::{storage_keys, SessionState};
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::ProtocolConfig;
use offline_protocol_core::{Message, MessageId, MessagePriority, Timestamp};
use offline_protocol_mls::MlsStorage;

/// One device, with the radio its frames actually go through.
struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    label: String,
    secure: Arc<InMemoryStorage>,
    state: Arc<InMemoryStorage>,
    events: Arc<Mutex<Vec<crate::Event>>>,
}

/// A custodian that admits strangers, so a deposit from a peer it holds no
/// session with is judged by the quotas rather than refused at the tier.
fn open_custody() -> CustodyConfig {
    CustodyConfig {
        enabled: true,
        stranger_max_entries: 8,
        stranger_max_bytes: 512 * 1024,
        ..CustodyConfig::default()
    }
}

fn base_config(label: &str) -> ProtocolConfig {
    let mut config = create_test_config_for_user(label);
    config.encryption.enabled = true;
    config.data.enabled = true;
    // No hold before forwarding: the tests drive the queue by hand.
    config.mesh_relay.jitter_min = Duration::from_millis(0);
    config.mesh_relay.jitter_max = Duration::from_millis(0);
    config
}

impl Node {
    fn new(label: &str) -> Self {
        Self::with_config(label, base_config(label))
    }

    fn custodian(label: &str, custody: CustodyConfig) -> Self {
        let mut config = base_config(label);
        config.custody = custody;
        Self::with_config(label, config)
    }

    fn with_config(label: &str, config: ProtocolConfig) -> Self {
        let secure = crate::test_identity::seeded_storage(label);
        let state = Arc::new(InMemoryStorage::new());
        Self::launch(label, config, secure, state)
    }

    fn launch(
        label: &str,
        config: ProtocolConfig,
        secure: Arc<InMemoryStorage>,
        state: Arc<InMemoryStorage>,
    ) -> Self {
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        protocol
            .initialize_mls(
                secure.clone(),
                Arc::new(TestProtocolStateStorage {
                    storage: state.clone(),
                }),
            )
            .expect("initialize_mls");

        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        // A device can only put a frame on a link it actually has, which is
        // what makes a recipient out of range in the first place.
        mock.set_reject_unknown_recipients(true);
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        let events: Arc<Mutex<Vec<crate::Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));
        protocol.start().expect("start");

        Self {
            protocol,
            transport,
            address: id(label),
            label: label.to_string(),
            secure,
            state,
            events,
        }
    }

    /// Relaunch on the same storage with a different configuration, as an
    /// application does between two sessions.
    fn relaunch(self, custody: CustodyConfig) -> Self {
        let mut config = base_config(&self.label);
        config.custody = custody;
        let Node {
            label,
            secure,
            state,
            ..
        } = self;
        Self::launch(&label, config, secure, state)
    }

    /// The space this node replicates with `peer`: the peer's own address.
    fn space_for(peer: &Node) -> String {
        peer.address.clone()
    }

    /// Puts `peer` in range, as discovery would.
    fn link(&mut self, peer: &Node) {
        self.transport.add_connected_peer(peer.address.clone(), -55);
        self.protocol.on_neighbor_discovered(&peer.address);
    }

    fn unlink(&mut self, peer: &Node) {
        self.transport.remove_connected_peer(&peer.address);
        self.protocol.on_neighbor_lost(&peer.address);
    }

    /// Everything handed to a neighbour since the last take, as
    /// `(neighbour, frame)`.
    fn take_peer_sends(&mut self) -> Vec<(String, Message)> {
        let sends = self.transport.peer_sends();
        self.transport.clear_peer_sends();
        sends
    }

    /// Hands `message` to this node over the link from `from`, and lets it
    /// process everything it holds.
    fn receive_from(&mut self, message: Message, from: &str) {
        self.transport.queue_message_from(message, from.to_string());
        while self.protocol.receive_message().is_some() {}
    }

    /// Runs the forwarding queue as if `elapsed` had passed since now.
    fn flush_after(&mut self, elapsed: Duration) {
        self.protocol.flush_mesh_relays_at(Instant::now() + elapsed);
    }

    fn held_records(&self) -> Vec<String> {
        self.state
            .list_keys(storage_keys::CUSTODY)
            .unwrap_or_default()
    }
}

/// A real 1:1 MLS session between two nodes, with the sync capabilities
/// recorded on both sides as a key-package exchange would leave them.
fn pair(alice: &mut Node, carol: &mut Node) {
    let carol_kp = {
        let manager = carol.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package(&carol.address, &carol_kp.key_package_data)
            .unwrap();
        manager.create_session(&carol.address).unwrap()
    };
    {
        let manager = carol.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    confirm(alice, &carol.address);
    confirm(carol, &alice.address);
    let alice_address = alice.address.clone();
    let carol_address = carol.address.clone();
    for (node, peer) in [(&mut *alice, carol_address), (&mut *carol, alice_address)] {
        node.protocol.peer_data_sync.insert(peer.clone());
        node.protocol.peer_data_tombstones.insert(peer.clone());
        node.protocol.peer_data_interest.insert(peer.clone());
        node.protocol.peer_data_custody.insert(peer);
    }
}

fn confirm(node: &mut Node, peer: &str) {
    node.protocol.record_encryption_capable(peer);
    node.protocol
        .persist_session_state(peer, SessionState::Confirmed, "test")
        .unwrap();
    node.protocol.confirmed_sessions.insert(peer.to_string());
}

fn write(node: &mut Node, space: &str, doc: &str, key: &str, value: &str) {
    node.protocol
        .data_map_set(space, doc, "m", key, DataValue::text(value))
        .expect("set");
    node.protocol.data_flush(space, doc).expect("flush");
}

fn read(node: &mut Node, space: &str, doc: &str, key: &str) -> Option<DataValue> {
    node.protocol
        .data_map_get(space, doc, "m", key)
        .expect("get")
}

/// The depositor's frame as it leaves for the custodian: alice edits a
/// document she replicates with carol while only bob is in range.
///
/// Returns the frame handed to bob, which carries the deposit request.
fn deposit_from(alice: &mut Node, bob: &Node, carol: &Node) -> Message {
    write(alice, &Node::space_for(carol), "notes", "k", "v");
    let sends = alice.take_peer_sends();
    let (to, frame) = sends
        .into_iter()
        .find(|(_, frame)| frame.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("the sync frame was handed to a neighbour");
    assert_eq!(to, bob.address, "handed to the one neighbour in range");
    assert_eq!(frame.recipient.as_str(), carol.address);
    assert_eq!(
        frame.metadata.get(CUSTODY_META_KEY).map(String::as_str),
        Some(CUSTODY_CLASS_DATA),
        "the depositor writes the class token on its own sealed replication frame"
    );
    frame
}

/// Three nodes in the chapter's opening topology: alice paired with carol,
/// alice in range of bob only, bob in range of alice only.
fn topology(custody: CustodyConfig) -> (Node, Node, Node) {
    let mut alice = Node::new("alice");
    let mut carol = Node::new("carol");
    let mut bob = Node::custodian("bob", custody);
    pair(&mut alice, &mut carol);
    alice.link(&bob);
    bob.link(&alice);
    // Bob knows alice parses receipts, as her key package would have said.
    bob.protocol.peer_data_custody.insert(alice.address.clone());
    (alice, bob, carol)
}

/// Walks bob's queue to the drop point for a frame he could not forward.
fn drop_point(bob: &mut Node) {
    // Due, nowhere to go (the only link is the depositor's), requeued.
    bob.flush_after(Duration::ZERO);
    // Past the overdue cut-off: abandoned, and judged.
    bob.flush_after(RELAY_QUEUE_MAX_OVERDUE + Duration::from_millis(1));
}

#[test]
fn a_deposited_frame_is_held_and_delivered_when_the_recipient_appears() {
    let (mut alice, mut bob, mut carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    let frame_id = frame.id.clone();

    bob.receive_from(frame.clone(), &alice.address);
    assert_eq!(
        bob.protocol.custody_stats().held,
        0,
        "a forward is not a deposit yet"
    );
    drop_point(&mut bob);

    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.accepted, 1);
    assert_eq!(stats.held, 1);
    assert_eq!(
        bob.held_records(),
        vec![frame_id.as_str()],
        "sealed record on disk"
    );
    assert_eq!(
        bob.protocol.mesh_relay_stats().abandoned_overdue,
        1,
        "custody begins only where forwarding ends"
    );
    assert!(
        !bob.protocol.mesh_relay.is_suppressed(&frame_id.as_str()),
        "acceptance must not record the id in the suppression cache"
    );

    // The receipt: once, over the arrival link, signed, no acknowledgement
    // requested, and it settles nothing at the depositor.
    let sends = bob.take_peer_sends();
    let (to, receipt) = sends
        .into_iter()
        .find(|(_, m)| m.content.starts_with(internal_prefixes::CUSTODY_RECEIPT))
        .expect("a receipt was sent");
    assert_eq!(to, alice.address);
    assert!(
        !receipt.requires_ack,
        "a receipt never requests an acknowledgement"
    );
    assert!(
        receipt.metadata.contains_key("__ctrl_sig"),
        "signed like every control frame"
    );
    assert_eq!(bob.protocol.custody_stats().receipts_sent, 1);

    assert!(alice.protocol.outbox.contains_key(&frame_id));
    let retries_before = alice.protocol.retry_queue_size();
    alice.receive_from(receipt, &bob.address);
    assert_eq!(alice.protocol.custody_stats().receipts_received, 1);
    assert!(
        alice.protocol.outbox.contains_key(&frame_id),
        "a receipt settles nothing: the outbox entry stands"
    );
    assert_eq!(
        alice.protocol.retry_queue_size(),
        retries_before,
        "and the retry entry stands"
    );

    // A re-offer toward the custodian that answered carries no request; the
    // frame itself still goes, as an ordinary forward attempt. (Contact with
    // bob also re-drove alice's outbox toward him before the receipt was
    // read; only what leaves after it counts.)
    alice.take_peer_sends();
    let stored = alice
        .protocol
        .outbox
        .get(&frame_id)
        .expect("still in the outbox")
        .message
        .clone();
    assert!(
        !stored.metadata.contains_key(CUSTODY_META_KEY),
        "the request is written on the copy handed over, never on the stored frame"
    );
    alice.protocol.offer_to_mesh(&stored);
    let sends = alice.take_peer_sends();
    let reoffers: Vec<&Message> = sends
        .iter()
        .filter(|(to, m)| *to == bob.address && m.id == frame_id)
        .map(|(_, m)| m)
        .collect();
    assert!(!reoffers.is_empty(), "re-offered to bob");
    assert!(
        reoffers
            .iter()
            .all(|m| !m.metadata.contains_key(CUSTODY_META_KEY)),
        "a live receipt suppresses the deposit request toward that custodian"
    );

    // The duplicate deposit bob absorbs: not stored twice, not answered.
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.held, 1);
    assert_eq!(stats.duplicates, 1);
    assert_eq!(stats.receipts_sent, 1, "a duplicate is not answered");
    for reason in crate::protocol::custody::CustodyRefusal::ALL {
        if *reason != crate::protocol::custody::CustodyRefusal::Duplicate {
            assert_eq!(stats.refusals(*reason), 0, "{reason:?}");
        }
    }

    // Alice walks away; carol appears. The held frame is delivered.
    bob.unlink(&alice);
    bob.link(&carol);
    bob.flush_after(Duration::ZERO);
    let sends = bob.take_peer_sends();
    let (to, delivered) = sends
        .into_iter()
        .find(|(_, m)| m.id == frame_id)
        .expect("the held frame went out");
    assert_eq!(to, carol.address, "handed straight to the recipient");
    assert!(
        !delivered.metadata.contains_key(CUSTODY_META_KEY),
        "what the custodian transmits never carries the request"
    );
    assert_eq!(
        delivered.content, frame.content,
        "the ciphertext is untouched"
    );
    assert_eq!(delivered.hop_count.value(), 1, "one hop, as stored");

    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.delivered, 1);
    assert_eq!(
        stats.held, 0,
        "delivery to the recipient releases the frame"
    );
    assert!(bob.held_records().is_empty(), "and its record");
    assert!(
        bob.events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, crate::Event::MessageRelayed { message_id, .. } if *message_id == frame_id.as_str())),
        "a redelivery is an ordinary forward to the application"
    );

    // Carol opens it and the document lands, hours after alice wrote it.
    carol.receive_from(delivered, &bob.address);
    assert_eq!(
        read(&mut carol, &Node::space_for(&alice), "notes", "k"),
        Some(DataValue::text("v"))
    );
}

#[test]
fn a_held_frame_is_re_originated_at_most_once_per_neighbour_and_stays_held() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().held, 1);

    // Dave is not the recipient: the frame is re-originated toward him
    // once, and stays held.
    let dave = Node::new("dave");
    bob.link(&dave);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 1);
    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.re_originated, 1);
    assert_eq!(stats.held, 1, "a re-origination is not a delivery");

    // Seen again: nothing more toward dave during this hold.
    bob.protocol.on_neighbor_discovered(&dave.address);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 1);
    assert_eq!(bob.protocol.custody_stats().re_originated, 1);

    // The depositor never gets its own frame back.
    bob.protocol.on_neighbor_discovered(&alice.address);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 1);

    // The suppression cache saw none of it: alice's own retransmission of
    // the same id is still carried, which is the black hole the chapter
    // names.
    assert!(!bob.protocol.mesh_relay.is_suppressed(&frame.id.as_str()));
    bob.receive_from(frame.clone(), &alice.address);
    bob.flush_after(Duration::ZERO);
    assert_eq!(
        bob.transport.peer_send_count_for(&frame.id.as_str()),
        2,
        "the depositor's retransmission is forwarded, not suppressed"
    );
}

#[test]
fn a_marked_forward_that_reaches_no_link_returns_to_the_store_uncounted() {
    let (mut alice, mut bob, mut carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    let before = bob.protocol.mesh_relay_stats();

    // Carol appears and is gone again before the queue is flushed.
    bob.link(&carol);
    bob.transport.remove_connected_peer(&carol.address);
    bob.flush_after(Duration::ZERO);
    bob.flush_after(RELAY_QUEUE_MAX_OVERDUE + Duration::from_millis(1));

    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.held, 1, "still held");
    assert_eq!(stats.delivered, 0);
    assert_eq!(stats.re_originated, 0);
    for reason in crate::protocol::custody::CustodyRefusal::ALL {
        assert_eq!(
            stats.refusals(*reason),
            0,
            "not judged as a deposit: {reason:?}"
        );
    }
    assert_eq!(
        bob.protocol.mesh_relay_stats().abandoned_overdue,
        before.abandoned_overdue,
        "a marked forward is not an abandoned forward"
    );
    assert!(!bob.protocol.mesh_relay.is_suppressed(&frame.id.as_str()));

    // The recipient is always worth trying again.
    bob.transport.add_connected_peer(carol.address.clone(), -55);
    bob.protocol.on_neighbor_discovered(&carol.address);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.protocol.custody_stats().delivered, 1);
    let sends = bob.take_peer_sends();
    let (_, delivered) = sends.into_iter().find(|(_, m)| m.id == frame.id).unwrap();
    carol.receive_from(delivered, &bob.address);
    assert_eq!(
        read(&mut carol, &Node::space_for(&alice), "notes", "k"),
        Some(DataValue::text("v"))
    );
}

#[test]
fn a_forwarder_strips_the_request_whether_or_not_it_holds_custody() {
    let (mut alice, mut bob, carol) = topology(CustodyConfig::default());
    let frame = deposit_from(&mut alice, &bob, &carol);
    let dave = Node::new("dave");
    bob.link(&dave);

    bob.receive_from(frame.clone(), &alice.address);
    bob.flush_after(Duration::ZERO);

    let sends = bob.take_peer_sends();
    let (to, forwarded) = sends
        .into_iter()
        .find(|(_, m)| m.id == frame.id)
        .expect("forwarded onward");
    assert_eq!(to, dave.address);
    assert!(
        !forwarded.metadata.contains_key(CUSTODY_META_KEY),
        "the request travels exactly one hop"
    );
    assert_eq!(
        forwarded.content, frame.content,
        "only the outer metadata is touched"
    );
    assert_eq!(
        bob.protocol.custody_stats().accepted,
        0,
        "a transmitted frame is never held"
    );
}

#[test]
fn the_acceptance_table_refuses_what_the_chapter_refuses() {
    // Custody off: judged, refused, counted, so "off" can be told from
    // "nobody asked".
    let (mut alice, mut bob, carol) = topology(CustodyConfig::default());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().refused_disabled, 1);
    assert!(bob.held_records().is_empty());
    assert!(
        bob.take_peer_sends().is_empty(),
        "nothing answers a refusal"
    );

    // Stranger tier closed, which is the default.
    let (mut alice, mut bob, carol) = topology(CustodyConfig {
        enabled: true,
        ..CustodyConfig::default()
    });
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().refused_stranger, 1);

    // The same peer with an established session is admitted under the
    // session tier.
    bob.protocol
        .confirmed_sessions
        .insert(alice.address.clone());
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().accepted, 1);

    // Through a forwarder: the sender is not the arrival peer.
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    let dave = Node::new("dave");
    bob.link(&dave);
    bob.unlink(&alice);
    bob.receive_from(frame.clone(), &dave.address);
    // Dave is excluded as the arrival peer, alice as the sender: nowhere.
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().refused_not_depositor, 1);
    assert_eq!(bob.protocol.custody_stats().held, 0);

    // No request on the frame at all.
    let (mut alice, mut bob, carol) = topology(open_custody());
    let mut frame = deposit_from(&mut alice, &bob, &carol);
    frame.metadata.remove(CUSTODY_META_KEY);
    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().refused_no_request, 1);

    // An unknown class token.
    let (mut alice, mut bob, carol) = topology(open_custody());
    let mut frame = deposit_from(&mut alice, &bob, &carol);
    frame
        .metadata
        .insert(CUSTODY_META_KEY.to_string(), "media".to_string());
    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().refused_unknown_class, 1);

    // Below the soft relay floor.
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.protocol.set_device_battery(5, false);
    bob.receive_from(frame.clone(), &alice.address);
    // Relaying is refused outright below the floor, so the frame is never
    // queued; raise the battery to queue it, then drop it before the drop
    // point.
    assert_eq!(bob.protocol.mesh_relay_stats().queued, 0);
    bob.protocol.set_device_battery(90, false);
    bob.receive_from(frame, &alice.address);
    bob.protocol.set_device_battery(5, false);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().refused_battery, 1);
}

#[test]
fn a_held_frame_expires_at_the_end_of_the_hold_in_force() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.protocol.custody_stats().held, 1);
    // The receipt at acceptance is the last thing that leaves.
    bob.take_peer_sends();

    let hold_ms = bob.protocol.custody_config().hold_ms;
    let now_ms = Utc::now().timestamp_millis();
    bob.protocol
        .sweep_custody_now(Instant::now(), now_ms + hold_ms as i64 - 1);
    assert_eq!(bob.protocol.custody_stats().held, 1, "inside the hold");

    bob.protocol
        .sweep_custody_now(Instant::now(), now_ms + hold_ms as i64 + 1);
    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.expired, 1);
    assert_eq!(stats.held, 0);
    assert!(bob.held_records().is_empty(), "the record goes with it");
    assert!(
        bob.take_peer_sends().is_empty(),
        "expiry sends nothing to anybody"
    );
    // Carol appearing now finds nothing to deliver.
    bob.link(&carol);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 0);
}

#[test]
fn held_frames_survive_a_relaunch_and_a_lowered_hold_expires_them_at_restore() {
    let (mut alice, mut bob, mut carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.held_records().len(), 1);

    // Same configuration: restored as stored, and deliverable.
    let mut bob = bob.relaunch(open_custody());
    assert_eq!(bob.protocol.custody_stats().held, 1);
    assert_eq!(
        bob.protocol.custody_stats().accepted,
        0,
        "restore counts no acceptance"
    );
    bob.link(&carol);
    bob.flush_after(Duration::ZERO);
    let sends = bob.take_peer_sends();
    let (_, delivered) = sends
        .into_iter()
        .find(|(_, m)| m.id == frame.id)
        .expect("delivered after the relaunch");
    carol.receive_from(delivered, &bob.address);
    assert_eq!(
        read(&mut carol, &Node::space_for(&alice), "notes", "k"),
        Some(DataValue::text("v"))
    );

    // A fresh deposit, then a relaunch under a hold shorter than the record's
    // age: dropped at restore, record and all.
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    std::thread::sleep(Duration::from_millis(5));
    let bob = bob.relaunch(CustodyConfig {
        hold_ms: 1,
        ..open_custody()
    });
    assert_eq!(bob.protocol.custody_stats().held, 0);
    assert!(bob.held_records().is_empty());

    // A relaunch with custody disabled erases what it finds rather than
    // keeping it: nothing would ever deliver it.
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    let bob = bob.relaunch(CustodyConfig::default());
    assert_eq!(bob.protocol.custody_stats().held, 0);
    assert!(bob.held_records().is_empty());
}

#[test]
fn erase_drops_every_record_and_the_data_wipe_calls_it() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.held_records().len(), 1);

    bob.protocol.erase_custody().unwrap();
    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.held, 0);
    assert_eq!(stats.accepted, 0, "counters reset");
    assert!(bob.held_records().is_empty());

    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    assert_eq!(bob.held_records().len(), 1);
    bob.protocol.data_wipe_all().unwrap();
    assert_eq!(bob.protocol.custody_stats().held, 0);
    assert!(
        bob.held_records().is_empty(),
        "the data-layer wipe erases custody"
    );
}

#[test]
fn a_receipt_naming_nothing_or_arriving_unsigned_changes_nothing() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);

    // A receipt for an identifier the outbox does not hold: ignored, counted.
    let unknown = encode_receipt(&MessageId::new().as_str(), 1_000);
    let mut receipt = bob
        .protocol
        .create_message(&alice.address, unknown, Some(MessagePriority::Low), None)
        .unwrap();
    receipt.requires_ack = false;
    bob.protocol.sign_control_message(&mut receipt).unwrap();
    alice.receive_from(receipt, &bob.address);
    let stats = alice.protocol.custody_stats();
    assert_eq!(stats.receipts_ignored, 1);
    assert_eq!(stats.receipts_received, 0);
    assert!(alice.protocol.custody_receipts.is_empty());

    // An unsigned receipt for a real entry is refused by the control gate
    // before it is read.
    let mut unsigned = bob
        .protocol
        .create_message(
            &alice.address,
            encode_receipt(&frame.id.as_str(), 1_000),
            Some(MessagePriority::Low),
            None,
        )
        .unwrap();
    unsigned.requires_ack = false;
    alice.receive_from(unsigned, &bob.address);
    assert_eq!(alice.protocol.custody_stats().receipts_received, 0);
    assert!(alice.protocol.custody_receipts.is_empty());

    // A malformed body past the gate: refused silently.
    let mut malformed = bob
        .protocol
        .create_message(
            &alice.address,
            format!(
                "{}{{\"v\":9,\"id\":\"x\"}}",
                internal_prefixes::CUSTODY_RECEIPT
            ),
            Some(MessagePriority::Low),
            None,
        )
        .unwrap();
    malformed.requires_ack = false;
    bob.protocol.sign_control_message(&mut malformed).unwrap();
    alice.receive_from(malformed, &bob.address);
    assert_eq!(alice.protocol.custody_stats().receipts_ignored, 2);

    // No receipt without the capability entry, whatever the quotas say.
    bob.protocol.peer_data_custody.remove(&alice.address);
    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    assert_eq!(
        bob.protocol.custody_stats().accepted,
        1,
        "the deposit is still taken"
    );
    assert_eq!(bob.protocol.custody_stats().receipts_sent, 0);
    assert_eq!(bob.protocol.custody_stats().receipts_dropped, 1);
    assert!(bob.take_peer_sends().is_empty());
}

#[test]
fn a_neighbour_without_a_mesh_link_queues_nothing_and_stays_untried() {
    let (mut alice, mut bob, mut carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    bob.take_peer_sends();

    // A presence edge from a carrier holding no link to carol runs the
    // discovery hook; nothing is queued, and nothing is spent.
    bob.protocol.on_neighbor_discovered(&carol.address);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 0);
    assert_eq!(bob.protocol.mesh_relay_stats().awaiting_transmission, 0);
    assert_eq!(bob.protocol.custody_stats().held, 1);

    // The mesh link appears: delivered on the first flush.
    bob.link(&carol);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.protocol.custody_stats().delivered, 1);
    let sends = bob.take_peer_sends();
    let (_, delivered) = sends.into_iter().find(|(_, m)| m.id == frame.id).unwrap();
    carol.receive_from(delivered, &bob.address);
    assert_eq!(
        read(&mut carol, &Node::space_for(&alice), "notes", "k"),
        Some(DataValue::text("v"))
    );
}

#[test]
fn an_abandoned_forward_leaves_the_neighbour_untried() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    bob.take_peer_sends();

    // Dave is not the recipient. Queued toward him, gone before the flush,
    // abandoned at the overdue cut-off: returned, and dave untried.
    let dave = Node::new("dave");
    bob.link(&dave);
    bob.transport.remove_connected_peer(&dave.address);
    bob.flush_after(Duration::ZERO);
    bob.flush_after(RELAY_QUEUE_MAX_OVERDUE + Duration::from_millis(1));
    assert_eq!(bob.protocol.custody_stats().re_originated, 0);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 0);

    // Seen again with a link: the one attempt the hold allows toward dave
    // is still available.
    bob.transport.add_connected_peer(dave.address.clone(), -55);
    bob.protocol.on_neighbor_discovered(&dave.address);
    bob.flush_after(Duration::ZERO);
    assert_eq!(bob.transport.peer_send_count_for(&frame.id.as_str()), 1);
    assert_eq!(bob.protocol.custody_stats().re_originated, 1);
}

#[test]
fn a_frame_asking_for_nothing_on_an_off_device_is_no_request() {
    let (mut alice, mut bob, carol) = topology(CustodyConfig::default());
    let mut frame = deposit_from(&mut alice, &bob, &carol);
    frame.metadata.remove(CUSTODY_META_KEY);
    bob.receive_from(frame, &alice.address);
    drop_point(&mut bob);
    let stats = bob.protocol.custody_stats();
    assert_eq!(stats.refused_no_request, 1, "nobody asked");
    assert_eq!(
        stats.refused_disabled, 0,
        "so nothing was turned away for being off"
    );
}

#[test]
fn a_receipt_is_judged_from_its_own_timestamp_not_from_arrival() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    let hold_ms = bob.protocol.custody_config().hold_ms;
    let now_ms = Utc::now().timestamp_millis();
    let depositor = alice.address.clone();
    let held_id = frame.id.as_str();

    let receipt_at = |bob: &mut Node, minted_at_ms: i64| -> Message {
        let mut receipt = bob
            .protocol
            .create_message(
                &depositor,
                encode_receipt(&held_id, hold_ms),
                Some(MessagePriority::Low),
                None,
            )
            .unwrap();
        receipt.requires_ack = false;
        receipt.timestamp = Timestamp::from_millis(minted_at_ms);
        bob.protocol.sign_control_message(&mut receipt).unwrap();
        receipt
    };

    // Minted longer ago than its hold: received, and nothing left to
    // suppress, so the next offer toward bob carries the request.
    let stale = receipt_at(&mut bob, now_ms - hold_ms as i64 - 60_000);
    alice.receive_from(stale, &bob.address);
    assert_eq!(alice.protocol.custody_stats().receipts_received, 1);
    assert!(!alice
        .protocol
        .custody_suppressed_toward(&frame.id, &bob.address));

    // One stamped a day ahead of this clock is judged from local now: the
    // suppression ends by now + hold, not a day later.
    let ahead = receipt_at(&mut bob, now_ms + 86_400_000);
    alice.receive_from(ahead, &bob.address);
    assert!(alice
        .protocol
        .custody_suppressed_toward(&frame.id, &bob.address));
    let until = alice.protocol.custody_receipts[&frame.id][&bob.address];
    assert!(
        until <= Utc::now().timestamp_millis() + hold_ms as i64,
        "a future timestamp must not extend the suppression"
    );
    alice.protocol.custody_receipts.clear();

    // A fresh one suppresses until its hold elapses on the wall clock.
    let fresh = receipt_at(&mut bob, now_ms);
    alice.receive_from(fresh, &bob.address);
    assert!(alice
        .protocol
        .custody_suppressed_toward(&frame.id, &bob.address));
    alice
        .protocol
        .sweep_custody_now(Instant::now(), now_ms + hold_ms as i64 + 1);
    assert!(
        !alice
            .protocol
            .custody_suppressed_toward(&frame.id, &bob.address),
        "the sweep drops a suppression whose hold has ended"
    );
}

#[test]
fn erase_drops_redeliveries_already_queued() {
    let (mut alice, mut bob, carol) = topology(open_custody());
    let frame = deposit_from(&mut alice, &bob, &carol);
    bob.receive_from(frame.clone(), &alice.address);
    drop_point(&mut bob);
    bob.take_peer_sends();

    // Carol appears: a marked forward is queued and not yet flushed.
    bob.link(&carol);
    bob.protocol.erase_custody().unwrap();
    bob.flush_after(Duration::ZERO);
    assert_eq!(
        bob.transport.peer_send_count_for(&frame.id.as_str()),
        0,
        "a redelivery queued before the erase must not go out after it"
    );
    assert_eq!(bob.protocol.custody_stats().delivered, 0);
}

#[test]
fn only_a_class_a_frame_with_retained_plaintext_carries_a_request() {
    let mut alice = Node::new("alice");
    let mut carol = Node::new("carol");
    let bob = Node::new("bob");
    pair(&mut alice, &mut carol);
    alice.link(&bob);

    // A direct message is sealed the same way and is never deposited.
    alice
        .protocol
        .send_message(&carol.address, "hello", None, None::<String>)
        .unwrap();
    let sends = alice.take_peer_sends();
    assert!(!sends.is_empty(), "offered to bob");
    assert!(
        sends
            .iter()
            .all(|(_, m)| !m.metadata.contains_key(CUSTODY_META_KEY)),
        "a direct message is Class C and carries no request"
    );

    // After a relaunch the retained plaintext is gone, so the same frame is
    // offered without the key.
    write(&mut alice, &Node::space_for(&carol), "notes", "k", "v");
    let sends = alice.take_peer_sends();
    let (_, frame) = sends
        .into_iter()
        .find(|(_, m)| m.metadata.contains_key(CUSTODY_META_KEY))
        .expect("deposited while the plaintext is retained");
    let mut alice = alice.relaunch(CustodyConfig::default());
    alice.protocol.peer_data_sync.insert(carol.address.clone());
    alice.link(&bob);
    let restored = alice
        .protocol
        .outbox
        .get(&frame.id)
        .expect("the outbox entry was restored")
        .message
        .clone();
    alice.protocol.offer_to_mesh(&restored);
    let sends = alice.take_peer_sends();
    let (_, reoffer) = sends
        .into_iter()
        .find(|(_, m)| m.id == frame.id)
        .expect("re-offered after the relaunch");
    assert!(
        !reoffer.metadata.contains_key(CUSTODY_META_KEY),
        "a frame whose plaintext this device no longer holds is offered without the key"
    );
}
