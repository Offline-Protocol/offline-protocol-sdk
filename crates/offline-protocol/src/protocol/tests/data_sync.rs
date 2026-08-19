//! Two-peer replication over the real send and receive paths.
//!
//! What these cover that no unit test can: that a document change actually
//! becomes a sealed frame, survives the ladder, is judged on arrival, and
//! lands in the other replica. Every frame here goes through MLS encryption,
//! the transport, the deduplicator, and the prefix dispatch, because the
//! whole design claim of this layer is that it rides machinery it did not
//! have to build, and a test that shortcuts any of it would be testing
//! something else.

use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use offline_protocol_data::DataValue;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::constants::ACK_FOR_KEY;
use crate::mls::InMemoryStorage;
use crate::protocol::data_sync::{blob_digest, MAX_DOCS_PER_SPACE, MAX_QUARANTINED_BLOBS};
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::types::{storage_keys, SessionState};
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::{ProtocolStateError, ProtocolStateResult, ProtocolStateStorage};
use offline_protocol_mls::MlsStorage;

/// One replica, with the transport its frames actually go through.
struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    label: String,
    secure: Arc<InMemoryStorage>,
    state: Arc<InMemoryStorage>,
}

impl Node {
    fn new(label: &str) -> Self {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = true;
        config.data.enabled = true;

        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let secure = crate::test_identity::seeded_storage(label);
        let state = Arc::new(InMemoryStorage::new());
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
            label: label.to_string(),
            secure,
            state,
        }
    }

    /// Relaunch on the same storage, as an application does between two
    /// sessions.
    ///
    /// This is not a convenience. Compaction writes a trimmed snapshot to
    /// disk but leaves the open document holding its full history, so the
    /// trim point only becomes real for a document that is opened again,
    /// and a replica that has never been relaunched cannot refuse anything.
    ///
    /// `peer` is re-announced because the pair fixture records the sync
    /// capability directly rather than through a key-package exchange, so
    /// there is no durable record for the relaunch to restore.
    fn restart(&mut self, peer: &str) {
        let mut config = create_test_config_for_user(&self.label);
        config.encryption.enabled = true;
        config.data.enabled = true;

        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        protocol
            .initialize_mls(
                self.secure.clone(),
                Arc::new(TestProtocolStateStorage {
                    storage: self.state.clone(),
                }),
            )
            .expect("initialize_mls");
        protocol.peer_data_sync.insert(peer.to_string());

        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        self.transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        protocol.start().expect("start");
        self.protocol = protocol;
    }

    /// The space this node replicates with `peer`: the peer's own address.
    fn space_for(peer: &Node) -> String {
        peer.address.clone()
    }

    /// Delta records still on disk for one document.
    ///
    /// Compaction is what trims a document's history, and it deletes these
    /// as it folds them in. There is no API that says "compaction ran", so
    /// this is how a test asserts that the thing it depends on happened.
    fn delta_records(&self, space: &str, doc: &str) -> usize {
        let prefix = format!("{space}/{doc}/");
        self.state
            .list_keys(storage_keys::DATA_DELTA_LOG)
            .unwrap_or_default()
            .iter()
            .filter(|key| key.starts_with(&prefix))
            .count()
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

/// The replication bookkeeping on disk for one space, as JSON.
///
/// Read back through the protocol's own sealed reader rather than out of the
/// store, because the record is sealed and what this asserts about is what
/// the next launch will actually be able to open.
fn sync_record(node: &mut Node, space: &str) -> serde_json::Value {
    let storage = node
        .protocol
        .data_storage_for_sync()
        .expect("the data layer is on");
    let bytes = node
        .protocol
        .read_state_record(storage.as_ref(), storage_keys::DATA_SYNC, space)
        .expect("read the replication bookkeeping")
        .expect("the replication bookkeeping exists");
    serde_json::from_slice(&bytes).expect("the record is JSON")
}

/// Write a space's replication bookkeeping, as a previous run would have left
/// it.
///
/// A crash in the middle of an import is not a thing a test can stage, so the
/// state one leaves behind is staged instead: the marker on disk, written
/// before the engine was called, that the dead process never reached the line
/// to clear. It goes through the protocol's own sealed writer, because a
/// record this build cannot open is not the record it has to survive.
fn seed_sync_record(node: &mut Node, space: &str, record: &serde_json::Value) {
    let storage = node
        .protocol
        .data_storage_for_sync()
        .expect("the data layer is on");
    let bytes = serde_json::to_vec(record).expect("encode");
    node.protocol
        .write_state_record(storage.as_ref(), storage_keys::DATA_SYNC, space, &bytes)
        .expect("seed the replication bookkeeping");
}

/// The digests a space is refusing.
fn quarantined(node: &mut Node, space: &str) -> Vec<String> {
    sync_record(node, space)
        .get("quarantined")
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A `snap` frame carrying `blob`, as the peer would send it.
fn snapshot_frame(doc: &str, blob: &[u8]) -> String {
    format!(
        r#"{{"v":1,"k":"snap","doc":"{doc}","blob":"{}"}}"#,
        BASE64.encode(blob)
    )
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

/// Write until compaction folds the delta log away, and say so.
///
/// Compaction is triggered by the size of the log rather than by a call, so
/// a test that needs a trimmed history has to earn one. The return value is
/// asserted rather than ignored: without the trim there is no gap to refuse,
/// and every test below would pass while exercising nothing.
fn write_until_compacted(node: &mut Node, space: &str, doc: &str) -> bool {
    let filler = "x".repeat(8 * 1024);
    for n in 0..32 {
        write(node, space, doc, &format!("filler{n}"), &filler);
        if node.delta_records(space, doc) == 0 {
            return true;
        }
    }
    false
}

#[test]
fn a_fork_below_the_peers_trim_point_converges_instead_of_talking_forever() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // A base both replicas hold.
    write(&mut alice, &alice_space, "notes", "seed", "0");
    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "seed"),
        Some(DataValue::text("0")),
        "precondition: the replicas have to share a base for one to fork below it"
    );

    // Partitioned. Alice writes enough to trip compaction, which trims her
    // history above that shared base, and Bob never sees any of it.
    assert!(
        write_until_compacted(&mut alice, &alice_space, "notes"),
        "compaction never fired, so there is no trimmed history to fork below"
    );
    // The trim is only real for a document that has been opened again, so
    // without this the refusal below never happens and this test passes
    // while exercising nothing. It failed exactly that way first.
    alice.restart(&bob.address);
    alice.transport.clear_sent_messages();

    // Bob edits from the base, which is now below Alice's trim point. His
    // delta is ordinary partition traffic and it is exactly the shape the
    // engine aborts on, so Alice must refuse it — and then be able to say
    // what would work instead. Answering with her versions cannot: Bob would
    // recompute the same refused delta from them, for as long as both sides
    // kept at it.
    write(&mut bob, &bob_space, "notes", "from_bob", "B");

    let rounds = settle(&mut alice, &mut bob);

    // What this cannot promise is convergence. A replica that trimmed its
    // history deleted the ancestors this branch depends on, and no frame
    // brings them back: not a run of changes, and not the whole document
    // either. Both are refused, which is the only thing keeping the process
    // alive, and the replicas stay apart.
    //
    // What it must promise is that the attempt ends. Answering the refusal
    // with our versions instead makes the peer recompute the same refused
    // delta from them, and the two sides trade it until something stops
    // them, which on a mesh is the battery.
    assert_eq!(
        rounds.last().copied(),
        Some(0),
        "the exchange never went quiet: {rounds:?}"
    );

    // And nothing was lost to the attempt: a refusal is not a rollback.
    assert_eq!(
        read(&mut alice, &alice_space, "notes", "seed"),
        Some(DataValue::text("0")),
        "the refused exchange damaged the document it could not merge"
    );
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "from_bob"),
        Some(DataValue::text("B")),
        "the peer lost its own edit to a refusal on the other side"
    );
}

#[test]
fn a_snapshot_request_for_an_unknown_document_creates_nothing() {
    let (mut alice, bob) = pair();
    let alice_space = Node::space_for(&bob);

    // Serving this would mean opening a document to export it, which is a
    // way to spend a peer's storage that does not even need a blob.
    alice.transport.clear_sent_messages();
    alice.protocol.handle_data_sync_frame(
        &bob.address,
        r#"{"v":1,"k":"need_snap","doc":"never-heard-of-it"}"#,
    );

    assert!(
        alice
            .protocol
            .data_list_docs(&alice_space)
            .expect("list")
            .is_empty(),
        "a snapshot request created a document"
    );
    assert!(
        alice.transport.sent_messages().is_empty(),
        "a snapshot request for an unknown document was answered"
    );
}

#[test]
fn a_partial_offer_does_not_snapshot_the_documents_it_leaves_out() {
    let (mut alice, bob) = pair();
    let alice_space = Node::space_for(&bob);

    write(&mut alice, &alice_space, "one", "k", "1");
    write(&mut alice, &alice_space, "two", "k", "2");
    let ours = alice
        .protocol
        .data_doc_version(&alice_space, "one")
        .expect("version");

    // An offer naming `one` at the version Alice already holds. Marked
    // partial, because a space with more documents than one frame carries
    // sends several and no single frame is the whole list.
    let offer = |reply: bool, partial: bool| {
        format!(
            r#"{{"v":1,"k":"vv","reply":{reply},"partial":{partial},"docs":{{"one":"{ours}"}}}}"#
        )
    };

    alice.transport.clear_sent_messages();
    alice
        .protocol
        .handle_data_sync_frame(&bob.address, &offer(true, true));
    assert!(
        alice.transport.sent_messages().is_empty(),
        "a partial offer was read as the complete list, so every document \
         missing from it was sent in full"
    );

    // The control: the identical frame claiming to be complete does mean the
    // peer has never seen `two`, and answering with the whole document is
    // then correct. Without this half the assertion above would pass with
    // the inference deleted outright.
    alice
        .protocol
        .handle_data_sync_frame(&bob.address, &offer(true, false));
    assert!(
        !alice.transport.sent_messages().is_empty(),
        "a complete offer must still carry a document the peer has never seen"
    );
}

#[test]
fn a_peer_cannot_name_unlimited_documents_into_our_storage() {
    let (mut alice, bob) = pair();
    let alice_space = Node::space_for(&bob);

    // Every unknown name in an offer becomes a stored document, and one
    // exchange can carry as many names as the peer cares to send.
    let mut sent = 0usize;
    while sent < MAX_DOCS_PER_SPACE + 64 {
        let docs: Vec<String> = (sent..sent + 128)
            .map(|n| format!(r#""doc{n}":"""#))
            .collect();
        alice.protocol.handle_data_sync_frame(
            &bob.address,
            &format!(
                r#"{{"v":1,"k":"vv","reply":true,"partial":true,"docs":{{{}}}}}"#,
                docs.join(",")
            ),
        );
        sent += 128;
    }

    assert_eq!(
        alice
            .protocol
            .data_list_docs(&alice_space)
            .expect("list")
            .len(),
        MAX_DOCS_PER_SPACE,
        "a peer talked this device past its document ceiling"
    );

    // The other door into the same storage: a blob for a document nobody
    // has heard of creates one too, so it is bounded by the same ceiling.
    alice.protocol.handle_data_sync_frame(
        &bob.address,
        r#"{"v":1,"k":"delta","doc":"one-more","blob":"AAAA"}"#,
    );
    assert!(
        !alice
            .protocol
            .data_list_docs(&alice_space)
            .expect("list")
            .contains(&"one-more".to_string()),
        "a blob created a document past the ceiling"
    );
}

#[test]
fn a_change_too_large_to_inline_still_tells_the_peer_it_happened() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // Over the frame budget, so no delta can carry it. Leaving it at that
    // would be silence: on a link that never drops there is no rediscovery
    // and no restart, so nothing would ever ask, and both replicas would go
    // on believing they agree.
    write(
        &mut alice,
        &alice_space,
        "notes",
        "huge",
        &"x".repeat(48 * 1024),
    );
    settle(&mut alice, &mut bob);

    assert!(
        bob.protocol
            .data_list_docs(&bob_space)
            .expect("list")
            .contains(&"notes".to_string()),
        "the peer was never told the document exists, so nothing will ask for it"
    );
}

#[test]
fn nothing_is_sent_to_a_blocked_peer_and_unblocking_forgets_them() {
    let (mut alice, bob) = pair();
    let alice_space = Node::space_for(&bob);

    // Every public send surface checks blocking for itself. Replication is
    // not reached through one: a commit or a discovery triggers it, and a
    // blocked peer still holds a live session, so without a gate of its own
    // the frames would go out.
    alice.protocol.block_user(&bob.address).expect("block");
    alice.transport.clear_sent_messages();

    write(&mut alice, &alice_space, "notes", "title", "private");
    alice.protocol.kick_data_sync(&alice_space, "test");
    assert!(
        alice.transport.sent_messages().is_empty(),
        "a document reached a blocked peer"
    );

    // Unblock runs the clean-slate cleanup, which forgets every advertised
    // capability so the fresh exchange re-learns them. Skipping this one
    // would leave the memory disagreeing with the durable record that the
    // same cleanup just deleted.
    alice.protocol.unblock_user(&bob.address).expect("unblock");
    assert!(
        !alice.protocol.peer_data_sync.contains(&bob.address),
        "the clean slate kept the sync capability it had just deleted from disk"
    );
}

/// Protocol-state storage that remembers which categories were written, in
/// the order they were written.
///
/// The one thing about the crash record that no assertion on its contents can
/// reach: a marker written *after* the import would hold exactly the same
/// bytes, and would be worthless.
struct RecordingStateStorage {
    inner: TestProtocolStateStorage,
    writes: Arc<Mutex<Vec<String>>>,
}

impl ProtocolStateStorage for RecordingStateStorage {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        self.writes.lock().unwrap().push(key_type.to_string());
        self.inner.store(key_type, key_id, data)
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        self.inner.load(key_type, key_id)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        self.inner.delete(key_type, key_id)
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        self.inner.list_keys(key_type)
    }
}

#[test]
fn a_blob_that_did_not_survive_its_last_import_is_refused_when_the_sender_retries_it() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // A real blob, exported exactly as a catch-up exports one.
    write(&mut alice, &alice_space, "notes", "title", "hello");
    let blob = alice
        .protocol
        .data_export_snapshot(&alice_space, "notes")
        .expect("snapshot");
    let digest = blob_digest(&blob);

    // What a run that died inside the engine leaves behind.
    seed_sync_record(
        &mut bob,
        &bob_space,
        &serde_json::json!({ "in_flight": digest }),
    );
    bob.restart(&alice.address);

    bob.protocol
        .handle_data_sync_frame(&alice.address, &snapshot_frame("notes", &blob));

    assert!(
        bob.protocol
            .data_list_docs(&bob_space)
            .expect("list")
            .is_empty(),
        "the blob that did not survive its last import was handed to the engine again"
    );
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "title"),
        None,
        "the refused blob applied anyway"
    );

    // The marker became a refusal rather than being merely dropped, and it is
    // on disk: the sender's ladder will retry this blob across relaunches, so
    // a quarantine that lived in memory would survive nothing that matters.
    assert!(
        quarantined(&mut bob, &bob_space).contains(&digest),
        "the marker left by the dead run was not promoted to a refusal"
    );
    assert!(
        sync_record(&mut bob, &bob_space).get("in_flight").is_none(),
        "the promoted marker was left in flight, so the next open promotes it again"
    );

    // It survives the next launch too, which is the whole point: the retry
    // that ends the process is the one after the process came back.
    bob.restart(&alice.address);
    bob.protocol
        .handle_data_sync_frame(&alice.address, &snapshot_frame("notes", &blob));
    assert!(
        bob.protocol
            .data_list_docs(&bob_space)
            .expect("list")
            .is_empty(),
        "the quarantine did not survive a relaunch, so the retry ladder gets another go"
    );

    // And it refuses one blob, not one peer. Anything else would turn a
    // single bad frame into a peer that silently stops replicating.
    write(&mut alice, &alice_space, "notes", "title", "hello again");
    let fresh = alice
        .protocol
        .data_export_snapshot(&alice_space, "notes")
        .expect("snapshot");
    assert_ne!(
        blob_digest(&fresh),
        digest,
        "precondition: the second blob has to be a different one to prove anything"
    );
    bob.protocol
        .handle_data_sync_frame(&alice.address, &snapshot_frame("notes", &fresh));
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "title"),
        Some(DataValue::text("hello again")),
        "a quarantined blob stopped the peer replicating rather than stopping itself"
    );
}

#[test]
fn the_in_flight_marker_reaches_disk_before_the_engine_sees_the_blob() {
    // The ordering is the mechanism, and it is invisible in the record it
    // writes: a marker persisted after the import holds the same digest and
    // remembers only the imports that already worked. So this asserts on the
    // order the store was written in, which is the only place the difference
    // shows.
    let mut alice = Node::new("alice");
    let sender = alice.address.clone();
    let space = id("bob");
    write(&mut alice, &space, "notes", "title", "hello");
    let blob = alice
        .protocol
        .data_export_snapshot(&space, "notes")
        .expect("snapshot");

    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.data.enabled = true;
    let mut receiver = OfflineProtocol::new(config).expect("protocol");
    let writes = Arc::new(Mutex::new(Vec::new()));
    receiver
        .initialize_mls(
            crate::test_identity::seeded_storage("bob"),
            Arc::new(RecordingStateStorage {
                inner: TestProtocolStateStorage {
                    storage: Arc::new(InMemoryStorage::new()),
                },
                writes: writes.clone(),
            }),
        )
        .expect("initialize_mls");
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().expect("transport start");
    receiver
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    receiver.start().expect("start");

    writes.lock().unwrap().clear();
    receiver.handle_data_sync_frame(&sender, &snapshot_frame("notes", &blob));

    assert_eq!(
        receiver
            .data_map_get(&sender, "notes", "m", "title")
            .expect("get"),
        Some(DataValue::text("hello")),
        "precondition: the blob has to reach the engine for the ordering to mean anything"
    );

    // Only the categories this ordering is about. The space index is written
    // on its own schedule and says nothing either way.
    let order: Vec<String> = writes
        .lock()
        .unwrap()
        .iter()
        .filter(|key_type| {
            [
                storage_keys::DATA_SYNC,
                storage_keys::DATA_DOCS,
                storage_keys::DATA_DELTA_LOG,
            ]
            .contains(&key_type.as_str())
        })
        .cloned()
        .collect();
    assert_eq!(
        order.first().map(String::as_str),
        Some(storage_keys::DATA_SYNC),
        "the document was written before the marker that says an import was \
         under way, so a blob that ends the process leaves nothing behind: {order:?}"
    );
    assert!(
        order
            .iter()
            .any(|key_type| key_type != storage_keys::DATA_SYNC),
        "nothing was written for the document, so this proves only that the \
         marker came first among no other writes: {order:?}"
    );
}

#[test]
fn the_quarantine_forgets_its_oldest_entry_rather_than_growing() {
    let (alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);

    // A full list, and one more marker to promote into it. Every entry is a
    // change this device has decided never to apply, so an unbounded list is
    // a liability that grows with every crash rather than a safety net.
    let existing: Vec<String> = (0..MAX_QUARANTINED_BLOBS)
        .map(|n| format!("{n:032x}"))
        .collect();
    let newest = "f".repeat(32);
    seed_sync_record(
        &mut bob,
        &bob_space,
        &serde_json::json!({ "in_flight": newest, "quarantined": existing }),
    );

    // Any blob for this space opens the record, which is when a marker left
    // behind by a previous run is promoted. This one is unreadable, which is
    // beside the point: it is the open that matters.
    bob.protocol.handle_data_sync_frame(
        &alice.address,
        r#"{"v":1,"k":"delta","doc":"notes","blob":"AAAA"}"#,
    );

    let held = quarantined(&mut bob, &bob_space);
    assert_eq!(
        held.len(),
        MAX_QUARANTINED_BLOBS,
        "the quarantine grew past its ceiling"
    );
    assert!(
        !held.contains(&existing[0]),
        "the oldest refusal was kept and something else was dropped instead"
    );
    assert_eq!(
        held.last().map(String::as_str),
        Some(newest.as_str()),
        "the marker left by the previous run is not the newest refusal"
    );
}

/// Storage that cannot answer for one document, the way a backend having a
/// bad day cannot.
///
/// `LoadFailed` rather than `Corrupted` on purpose: a record that is
/// permanently gone leaves the document readable but empty, while one that is
/// merely unavailable this session is the case that used to fail the read.
struct OneUnreadableDoc {
    inner: TestProtocolStateStorage,
    doc_suffix: String,
}

impl ProtocolStateStorage for OneUnreadableDoc {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        self.inner.store(key_type, key_id, data)
    }

    fn load(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<Option<Vec<u8>>> {
        if key_type == storage_keys::DATA_DOCS && key_id.ends_with(&self.doc_suffix) {
            return Err(ProtocolStateError::LoadFailed(
                "this document cannot be read this session".to_string(),
            ));
        }
        self.inner.load(key_type, key_id)
    }

    fn delete(&self, key_type: &str, key_id: &str) -> ProtocolStateResult<()> {
        self.inner.delete(key_type, key_id)
    }

    fn list_keys(&self, key_type: &str) -> ProtocolStateResult<Vec<String>> {
        self.inner.list_keys(key_type)
    }
}

#[test]
fn a_document_that_cannot_be_read_is_left_out_of_the_offer_rather_than_failing_it() {
    let (mut alice, bob) = pair();
    let space = Node::space_for(&bob);

    write(&mut alice, &space, "healthy", "k", "v");
    write(&mut alice, &space, "broken", "k", "v");

    // Relaunched on a store that cannot answer for one of them. Failing the
    // whole read here is worse than it sounds: the offering leg reports and
    // gives up, and the answering leg treats the failure as an empty space,
    // so both replicas conclude they have nothing to say to each other over
    // one unreadable record.
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.data.enabled = true;
    let mut relaunched = OfflineProtocol::new(config).expect("protocol");
    relaunched
        .initialize_mls(
            alice.secure.clone(),
            Arc::new(OneUnreadableDoc {
                inner: TestProtocolStateStorage {
                    storage: alice.state.clone(),
                },
                doc_suffix: "/broken".to_string(),
            }),
        )
        .expect("initialize_mls");

    let versions = relaunched
        .data_sync_versions(&space)
        .expect("one unreadable document failed the whole space");
    assert!(
        versions.contains_key("healthy"),
        "the readable document was dropped along with the unreadable one"
    );
    assert!(
        !versions.contains_key("broken"),
        "precondition: the document has to actually be unreadable, or this \
         proves nothing about what happens when one is"
    );
}
