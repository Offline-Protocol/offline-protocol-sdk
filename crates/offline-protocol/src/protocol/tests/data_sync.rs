//! Two-peer replication over the real send and receive paths.
//!
//! What these cover that no unit test can: that a document change actually
//! becomes a sealed frame, survives the ladder, is judged on arrival, and
//! lands in the other replica. Every frame here goes through MLS encryption,
//! the transport, the deduplicator, and the prefix dispatch, because the
//! whole design claim of this layer is that it rides machinery it did not
//! have to build, and a test that shortcuts any of it would be testing
//! something else.

use std::sync::Arc;

use offline_protocol_data::DataValue;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::constants::ACK_FOR_KEY;
use crate::mls::InMemoryStorage;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::types::SessionState;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};

/// One replica, with the transport its frames actually go through.
struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
}

impl Node {
    fn new(label: &str) -> Self {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = true;
        config.data.enabled = true;

        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let secure = crate::test_identity::seeded_storage(label);
        protocol
            .initialize_mls(
                secure.clone(),
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
        protocol.start().expect("start");

        let address = id(label);
        Self {
            protocol,
            transport,
            address,
        }
    }

    /// The space this node replicates with `peer`: the peer's own address.
    fn space_for(peer: &Node) -> String {
        peer.address.clone()
    }
}

/// Two replicas with a real MLS session and the sync capability recorded on
/// both sides, as a key-package exchange would leave them.
fn pair() -> (Node, Node) {
    let mut alice = Node::new("alice");
    let mut bob = Node::new("bob");

    // A genuine 1:1 group: Alice imports Bob's key package, creates the
    // session, and Bob joins the Welcome it produced. Both managers now hold
    // the same group, which is what makes the frames below real ciphertext
    // rather than a fixture.
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

    confirm(&mut alice, &bob.address);
    confirm(&mut bob, &alice.address);

    // What a key package advertising `data_versions` leaves behind. The
    // negotiation itself has its own tests; these are about what happens
    // once it has succeeded.
    alice.protocol.peer_data_sync.insert(bob.address.clone());
    bob.protocol.peer_data_sync.insert(alice.address.clone());

    (alice, bob)
}

fn confirm(node: &mut Node, peer: &str) {
    node.protocol.record_encryption_capable(peer);
    node.protocol
        .persist_session_state(peer, SessionState::Confirmed, "test")
        .unwrap();
    node.protocol.confirmed_sessions.insert(peer.to_string());
}

/// Carry everything `from` has sent into `to`, and let `to` process it.
///
/// Returns how many frames moved, so a test can assert an exchange
/// terminates rather than merely converges.
fn pump(from: &mut Node, to: &mut Node) -> usize {
    let messages = from.transport.sent_messages();
    from.transport.clear_sent_messages();
    let moved = messages.len();
    for message in messages {
        to.transport.queue_message(message);
    }
    while to.protocol.receive_message().is_some() {}
    moved
}

/// Run the exchange to quiescence, returning the frames carried each round.
fn settle(alice: &mut Node, bob: &mut Node) -> Vec<usize> {
    let mut rounds = Vec::new();
    for _ in 0..8 {
        let carried = pump(alice, bob) + pump(bob, alice);
        rounds.push(carried);
        if carried == 0 {
            break;
        }
    }
    rounds
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

#[test]
fn a_local_change_reaches_the_other_replica() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    write(&mut alice, &alice_space, "notes", "title", "hello");
    assert!(
        !alice.transport.sent_messages().is_empty(),
        "a flushed change produced no frame"
    );

    settle(&mut alice, &mut bob);

    assert_eq!(
        read(&mut bob, &bob_space, "notes", "title"),
        Some(DataValue::text("hello")),
        "the change never arrived"
    );
}

#[test]
fn edits_made_on_both_sides_while_apart_converge_on_reconnect() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // Partitioned: both write, and nothing is carried between them. Bob has
    // to create his side first, because a document nobody has ever named on
    // his device is not one he can edit — which is itself the case the
    // version exchange exists to fix.
    bob.protocol
        .data_create_doc(&bob_space, "notes")
        .expect("create");
    write(&mut alice, &alice_space, "notes", "from_alice", "A");
    write(&mut bob, &bob_space, "notes", "from_bob", "B");
    alice.transport.clear_sent_messages();
    bob.transport.clear_sent_messages();

    // Reconnect. Each side offers what it holds and answers with the gap.
    alice.protocol.kick_data_sync(&alice_space, "test");
    bob.protocol.kick_data_sync(&bob_space, "test");
    let rounds = settle(&mut alice, &mut bob);

    for (node, space, label) in [
        (&mut alice, alice_space.as_str(), "alice"),
        (&mut bob, bob_space.as_str(), "bob"),
    ] {
        assert_eq!(
            read(node, space, "notes", "from_alice"),
            Some(DataValue::text("A")),
            "{label} is missing alice's edit"
        );
        assert_eq!(
            read(node, space, "notes", "from_bob"),
            Some(DataValue::text("B")),
            "{label} is missing bob's edit"
        );
    }

    // And it stopped. Two replicas that answer each other's answers converge
    // just as well and never shut up, which on a mesh is the difference
    // between a feature and a battery complaint.
    assert_eq!(
        rounds.last().copied(),
        Some(0),
        "the exchange never went quiet: {rounds:?}"
    );
}

#[test]
fn a_document_the_peer_has_never_seen_arrives_whole() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // Written before Bob is listening at all, so no delta of it was ever
    // pushed: the only way it reaches him is the version exchange noticing
    // he has never heard of the document.
    write(&mut alice, &alice_space, "recipes", "dish", "soup");
    alice.transport.clear_sent_messages();

    alice.protocol.kick_data_sync(&alice_space, "test");
    settle(&mut alice, &mut bob);

    assert_eq!(
        read(&mut bob, &bob_space, "recipes", "dish"),
        Some(DataValue::text("soup")),
        "a document the peer had never seen did not arrive"
    );
}

#[test]
fn nothing_is_sent_to_a_peer_that_has_not_advertised_sync() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // The peer downgraded, or never advertised at all.
    alice.protocol.peer_data_sync.remove(&bob.address);
    alice.transport.clear_sent_messages();

    write(&mut alice, &alice_space, "notes", "title", "private");
    alice.protocol.kick_data_sync(&alice_space, "test");

    assert!(
        alice.transport.sent_messages().is_empty(),
        "a frame was sent to a peer that never asked for one"
    );

    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "title"),
        None,
        "the change reached a peer that had not advertised sync"
    );
}

#[test]
fn a_change_applied_from_a_peer_is_not_echoed_back() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);

    write(&mut alice, &alice_space, "notes", "title", "hello");
    bob.transport.clear_sent_messages();

    // Exactly one hop: Bob receives the change, applies it, and flushes. That
    // flush exports the change he just imported, because a commit hands out
    // everything since the last one regardless of who authored it. Sending
    // that straight back is harmless — the merge absorbs it — but it is a
    // frame nobody needed, and on a mesh those are the expensive kind.
    //
    // Asserting after a full settle would prove nothing: by then the flush
    // has no pending change left to export, so the suppression would never
    // be reached and the test would pass with it deleted.
    pump(&mut alice, &mut bob);

    assert_eq!(
        read(&mut bob, &Node::space_for(&alice), "notes", "title"),
        Some(DataValue::text("hello")),
        "precondition: the change has to have arrived for the echo to exist"
    );
    let echoed: Vec<_> = bob
        .transport
        .sent_messages()
        .into_iter()
        .filter(|message| !message.metadata.contains_key(ACK_FOR_KEY))
        .collect();
    assert!(
        echoed.is_empty(),
        "a change was echoed straight back to the peer it came from"
    );
}

#[test]
fn a_replicated_change_survives_a_restart_of_the_receiver() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    write(&mut alice, &alice_space, "notes", "title", "durable");
    settle(&mut alice, &mut bob);

    // A change accepted from a peer has to reach storage during the accept.
    // The sender has already been told it arrived, by the acknowledgement
    // the frame rode in on, so leaving it in memory would turn an ordinary
    // relaunch into silent loss with nothing left to ask for it again.
    let stored = bob
        .protocol
        .data_map_get(&bob_space, "notes", "m", "title")
        .expect("get");
    assert_eq!(stored, Some(DataValue::text("durable")));

    let docs = bob.protocol.data_list_docs(&bob_space).expect("list");
    assert!(
        docs.contains(&"notes".to_string()),
        "the replicated document is not in the space index"
    );
}

#[test]
fn a_frame_from_a_peer_only_ever_touches_that_peer_s_space() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);

    // Alice keeps a private space that is not named after any peer.
    alice
        .protocol
        .data_map_set("private", "diary", "m", "secret", DataValue::text("mine"))
        .expect("set");
    alice
        .protocol
        .data_flush("private", "diary")
        .expect("flush");

    // Nothing about a local-only space is ever offered: the space is not a
    // peer, so there is nobody to offer it to.
    alice.transport.clear_sent_messages();
    alice.protocol.kick_data_sync("private", "test");
    assert!(
        alice.transport.sent_messages().is_empty(),
        "a local-only space produced a frame"
    );

    // And a real exchange with Bob never mentions it.
    write(&mut alice, &alice_space, "notes", "title", "shared");
    settle(&mut alice, &mut bob);
    assert!(
        !bob.protocol
            .data_list_spaces()
            .expect("spaces")
            .contains(&"private".to_string()),
        "a private space leaked into the peer's store"
    );
}

#[test]
fn rediscovery_flapping_does_not_re_offer_every_time() {
    let (mut alice, bob) = pair();
    let alice_space = Node::space_for(&bob);

    write(&mut alice, &alice_space, "notes", "title", "hello");
    alice.transport.clear_sent_messages();

    // A peer walking in and out of Bluetooth range fires discovery over and
    // over. The offer is a reconciliation sweep, not a delivery — anything
    // committed locally was already pushed when it happened — so repeating it
    // per discovery event is mesh traffic that buys nothing.
    for _ in 0..5 {
        alice
            .protocol
            .kick_data_sync(&alice_space, "peer_rediscovered");
    }

    assert_eq!(
        alice.transport.sent_messages().len(),
        1,
        "five rediscoveries produced more than one offer"
    );
}

#[test]
fn the_startup_sweep_sees_a_space_that_has_only_ever_been_written_to() {
    let (mut alice, bob) = pair();
    let alice_space = Node::space_for(&bob);

    // Nothing has ever been received in this space: Alice wrote, and that is
    // all. Sweeping the replication bookkeeping instead of the space index
    // would miss it, because a bookkeeping record only exists once a blob has
    // been accepted from a peer — so the documents nobody else has seen are
    // exactly the ones a cold start would skip.
    write(&mut alice, &alice_space, "notes", "title", "unshared");

    assert!(
        alice
            .protocol
            .data_list_spaces()
            .expect("spaces")
            .contains(&alice_space),
        "the startup sweep cannot see a space that has only ever been written to"
    );
}
