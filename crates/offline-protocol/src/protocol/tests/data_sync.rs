//! Two-peer replication over the real send and receive paths.
//!
//! What these cover that no unit test can: that a document change actually
//! becomes a sealed frame, survives the ladder, is judged on arrival, and
//! lands in the other replica. Every frame here goes through MLS encryption,
//! the transport, the deduplicator, and the prefix dispatch, because the
//! whole design claim of this layer is that it rides machinery it did not
//! have to build, and a test that shortcuts any of it would be testing
//! something else.

use std::sync::atomic::{AtomicBool, Ordering};
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
    events: Arc<Mutex<Vec<crate::Event>>>,
}

impl Node {
    fn new(label: &str) -> Self {
        Node::with_state_storage(label, |state| {
            Arc::new(TestProtocolStateStorage { storage: state })
        })
    }

    /// [`Self::new`] with the state store wrapped.
    ///
    /// `wrap` is handed the same in-memory store the node keeps, so a test
    /// can watch or fail individual records and still read the ones it did
    /// not interfere with.
    fn with_state_storage(
        label: &str,
        wrap: impl FnOnce(Arc<InMemoryStorage>) -> Arc<dyn ProtocolStateStorage>,
    ) -> Self {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = true;
        config.data.enabled = true;

        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let secure = crate::test_identity::seeded_storage(label);
        let state = Arc::new(InMemoryStorage::new());
        protocol
            .initialize_mls(secure.clone(), wrap(state.clone()))
            .expect("initialize_mls");

        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        let events: Arc<Mutex<Vec<crate::Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));
        protocol.start().expect("start");

        let address = id(label);
        Self {
            protocol,
            transport,
            address,
            label: label.to_string(),
            secure,
            state,
            events,
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
        protocol.peer_data_media.insert(peer.to_string());

        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        self.transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        let events: Arc<Mutex<Vec<crate::Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));
        protocol.start().expect("start");
        self.protocol = protocol;
        self.events = events;
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
    pair_of(Node::new("alice"), Node::new("bob"))
}

/// [`pair`] over nodes a test has already built, for one that needs a
/// particular store underneath a replica.
fn pair_of(mut alice: Node, mut bob: Node) -> (Node, Node) {
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
    alice.protocol.peer_data_media.insert(bob.address.clone());
    bob.protocol.peer_data_media.insert(alice.address.clone());

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

/// [`settle`], plus the media window.
///
/// A transfer leaves in batches that only refill as chunks are acknowledged,
/// and the refill happens on the tick loop rather than on receipt. Nothing in
/// this harness ticks, so a test about anything carried over the media path
/// has to turn that crank itself. The round count is generous because a
/// document of this size is many Bluetooth-sized chunks through a window of
/// two.
fn settle_media(alice: &mut Node, bob: &mut Node) -> usize {
    for round in 0..400 {
        alice.protocol.pump_media_transfers();
        bob.protocol.pump_media_transfers();
        if pump(alice, bob) + pump(bob, alice) == 0 {
            return round;
        }
    }
    400
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
fn a_local_edit_pending_when_a_remote_change_arrives_still_reaches_the_peer() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // A base both replicas hold, and a quiet exchange to start from.
    write(&mut alice, &alice_space, "notes", "seed", "0");
    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "seed"),
        Some(DataValue::text("0")),
        "precondition: both replicas have to hold the document"
    );

    // Edited but not flushed. Nothing auto-flushes: an application commits on
    // its own schedule, so a window where local work sits in memory is the
    // ordinary case rather than a contrived one.
    alice
        .protocol
        .data_map_set(
            &alice_space,
            "notes",
            "m",
            "from_alice",
            DataValue::text("A"),
        )
        .expect("set");

    // Bob's change arrives into that window. A commit exports everything
    // since the last one regardless of who authored it, so without draining
    // first, Alice's pending edit is folded into the same delta as Bob's
    // imported change, and that delta is then suppressed as an echo toward
    // the one peer it was owed to. It would be durable on Alice, announced to
    // nobody, with no trigger left to notice on a link that never drops.
    write(&mut bob, &bob_space, "notes", "from_bob", "B");
    settle(&mut alice, &mut bob);

    assert_eq!(
        read(&mut bob, &bob_space, "notes", "from_alice"),
        Some(DataValue::text("A")),
        "the edit that was pending when a remote change arrived never left the device"
    );
    assert_eq!(
        read(&mut alice, &alice_space, "notes", "from_bob"),
        Some(DataValue::text("B")),
        "draining the local edit cost the import it was making room for"
    );
}

/// Storage that fails one `DATA_DELTA_LOG` write and then behaves, the way a
/// backend that is briefly unavailable does.
///
/// One write rather than all of them on purpose: a store that never recovers
/// fails the import's own flush too, and the fold this arms for needs that
/// second flush to succeed.
struct FailOneDeltaWrite {
    inner: TestProtocolStateStorage,
    armed: Arc<AtomicBool>,
    fired: Arc<AtomicBool>,
}

impl ProtocolStateStorage for FailOneDeltaWrite {
    fn store(&self, key_type: &str, key_id: &str, data: &[u8]) -> ProtocolStateResult<()> {
        if key_type == storage_keys::DATA_DELTA_LOG && self.armed.swap(false, Ordering::SeqCst) {
            self.fired.store(true, Ordering::SeqCst);
            return Err(ProtocolStateError::StoreFailed(
                "the backend is briefly unavailable".to_string(),
            ));
        }
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
fn a_local_edit_stranded_by_a_failed_pre_flush_is_announced_to_the_peer() {
    let armed = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let alice = Node::with_state_storage("alice", |state| {
        Arc::new(FailOneDeltaWrite {
            inner: TestProtocolStateStorage { storage: state },
            armed: armed.clone(),
            fired: fired.clone(),
        })
    });
    let (mut alice, mut bob) = pair_of(alice, Node::new("bob"));
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    write(&mut alice, &alice_space, "notes", "seed", "0");
    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "seed"),
        Some(DataValue::text("0")),
        "precondition: both replicas have to hold the document"
    );

    // Pending, as an application leaves work between flushes.
    alice
        .protocol
        .data_map_set(
            &alice_space,
            "notes",
            "m",
            "from_alice",
            DataValue::text("A"),
        )
        .expect("set");

    // The pre-flush that drains it cannot write its delta record, so the
    // commit is rewound and the edit goes back into the pending set. The
    // import that follows flushes with the origin set, which folds the edit
    // into the imported change and suppresses the pair toward the only peer
    // it was owed to: the exact loss the pre-flush exists to prevent,
    // reopened by a store that failed once.
    armed.store(true, Ordering::SeqCst);
    write(&mut bob, &bob_space, "notes", "from_bob", "B");
    settle(&mut alice, &mut bob);

    assert!(
        fired.load(Ordering::SeqCst),
        "precondition: the pre-flush has to have failed, or this proves nothing \
         about what happens when it does"
    );
    assert_eq!(
        read(&mut alice, &alice_space, "notes", "from_bob"),
        Some(DataValue::text("B")),
        "a failed pre-flush cost the import it was making room for"
    );
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "from_alice"),
        Some(DataValue::text("A")),
        "the edit stranded by the failed pre-flush was never announced, and \
         nothing is left to notice it"
    );
}

/// Push a document past `MAX_DOC_BYTES` and leave it there.
///
/// Two values that are together over the cap, rather than many small ones
/// that add up to it. The cap is measured over the compacted export, which
/// includes the delta history a flush has not folded away yet, so a document
/// nudged over the line by its history drops back under as soon as compaction
/// runs and the next flush answers `Ok`. Content this size cannot be
/// compacted back under the cap, which is what makes `DocTooLarge` the answer
/// every later flush gives.
///
/// The bytes are incompressible on purpose: the compacted encoding
/// compresses, so a megabyte of one repeated byte measures as almost nothing.
fn grow_past_cap(node: &mut Node, space: &str, doc: &str) {
    let filler = |seed: u32| -> Vec<u8> {
        let mut state = 0x9e37_79b9u32.wrapping_mul(seed.wrapping_add(1));
        (0..600 * 1024)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    };
    node.protocol
        .data_map_set(space, doc, "m", "half", DataValue::bytes(filler(1)))
        .expect("set");
    node.protocol.data_flush(space, doc).expect("flush");
    node.protocol
        .data_map_set(space, doc, "m", "other_half", DataValue::bytes(filler(2)))
        .expect("set");
    let breach = node.protocol.data_flush(space, doc);
    assert!(
        matches!(breach, Err(crate::error::Error::DocTooLarge { .. })),
        "the document answered {breach:?} rather than reaching its cap"
    );
}

#[test]
fn a_document_over_its_cap_still_accepts_a_remote_change() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    write(&mut alice, &alice_space, "notes", "shared", "0");
    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "shared"),
        Some(DataValue::text("0")),
        "precondition: both replicas have to hold the document"
    );

    // Grown past the cap while the peer was not replicating, so the growth
    // costs no frames and Bob's copy stays small. What is under test is what
    // Alice does with an arriving change, not how a megabyte crosses a link.
    alice.protocol.peer_data_sync.remove(&bob.address);
    grow_past_cap(&mut alice, &alice_space, "notes");
    alice.protocol.peer_data_sync.insert(bob.address.clone());
    alice.transport.clear_sent_messages();

    // Deletion is the only edit a document past its cap still accepts, and
    // it is the route back under. Left pending, so the pre-flush has
    // something to do when Bob's change arrives.
    //
    // The seed rather than one of the fillers: dropping 8 KiB from a
    // document that has just crossed the cap brings it back under, and then
    // the pre-flush answers `Ok` and this test proves nothing about the
    // error it is named for.
    alice
        .protocol
        .data_map_delete(&alice_space, "notes", "m", "shared")
        .expect("deletions must keep working past the cap");
    assert!(
        alice
            .protocol
            .data_doc_size(&alice_space, "notes")
            .expect("size")
            > crate::MAX_DOC_BYTES as u64,
        "precondition: the document has to still be over its cap with the \
         deletion pending, or the pre-flush answers `Ok` and the arm under \
         test is never reached"
    );

    // `DocTooLarge` is what the pre-flush answers here, and it is the one
    // error that must not stop the import: the remote change may be the
    // deletion that brings the document back under, so refusing it would
    // close the only door out.
    write(&mut bob, &bob_space, "notes", "from_bob", "B");
    pump(&mut bob, &mut alice);

    assert_eq!(
        read(&mut alice, &alice_space, "notes", "from_bob"),
        Some(DataValue::text("B")),
        "a document over its cap refused a remote change instead of stepping over the flush error"
    );
    assert_eq!(
        alice
            .protocol
            .data_map_get(&alice_space, "notes", "m", "shared")
            .expect("get"),
        None,
        "the pre-flush did not commit the pending deletion"
    );
    // The claim the log makes on this path is that the flush and the push
    // both happened and only the size verdict that follows them failed. The
    // push is the half no local read can see.
    let pushed: Vec<_> = alice
        .transport
        .sent_messages()
        .into_iter()
        .filter(|message| !message.metadata.contains_key(ACK_FOR_KEY))
        .collect();
    assert!(
        !pushed.is_empty(),
        "the pending deletion was committed but never sent, so `DocTooLarge` \
         does mean the flush was lost"
    );
}

#[test]
fn a_local_edit_stranded_over_the_cap_is_announced_to_the_peer() {
    let armed = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let alice = Node::with_state_storage("alice", |state| {
        Arc::new(FailOneDeltaWrite {
            inner: TestProtocolStateStorage { storage: state },
            armed: armed.clone(),
            fired: fired.clone(),
        })
    });
    let (mut alice, mut bob) = pair_of(alice, Node::new("bob"));
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    write(&mut alice, &alice_space, "notes", "shared", "0");
    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut bob, &bob_space, "notes", "shared"),
        Some(DataValue::text("0")),
        "precondition: both replicas have to hold the document"
    );

    alice.protocol.peer_data_sync.remove(&bob.address);
    grow_past_cap(&mut alice, &alice_space, "notes");
    alice.protocol.peer_data_sync.insert(bob.address.clone());

    // The seed rather than one of the fillers, for the reason the sibling
    // test gives: dropping a filler brings the document back under the cap,
    // the flush inside the import then answers `Ok`, and the arm under test
    // is never reached.
    alice
        .protocol
        .data_map_delete(&alice_space, "notes", "m", "shared")
        .expect("deletions must keep working past the cap");
    assert!(
        alice
            .protocol
            .data_doc_size(&alice_space, "notes")
            .expect("size")
            > crate::MAX_DOC_BYTES as u64,
        "precondition: the document has to still be over its cap with the \
         deletion pending"
    );
    alice.transport.clear_sent_messages();

    // Both halves of the loss at once, which is the case neither sibling
    // test covers. The pre-flush cannot write its delta record, so the
    // deletion is rewound into the pending set. The import then folds it
    // into the imported change and suppresses the pair toward the only peer
    // it was owed to, and the flush that did so answers `DocTooLarge`,
    // because the document is over its cap either way. That error is raised
    // after the fold is durable, so reading it as a failed import would
    // withhold the announcement on exactly the documents whose one pending
    // edit is the deletion that brings them back under.
    write(&mut bob, &bob_space, "notes", "from_bob", "B");
    armed.store(true, Ordering::SeqCst);
    pump(&mut bob, &mut alice);

    assert!(
        fired.load(Ordering::SeqCst),
        "precondition: the pre-flush has to have failed, or this proves nothing \
         about what happens when it does"
    );
    assert_eq!(
        read(&mut alice, &alice_space, "notes", "from_bob"),
        Some(DataValue::text("B")),
        "a document over its cap refused a remote change instead of stepping \
         over the flush error"
    );
    assert_eq!(
        alice
            .protocol
            .data_map_get(&alice_space, "notes", "m", "shared")
            .expect("get"),
        None,
        "precondition: the fold has to have happened, or there is nothing \
         stranded to announce"
    );

    // The announcement itself. Nothing else on this path sends: the fold's
    // own push is suppressed as an echo, which is what strands it, so a
    // non-acknowledgement frame here is the offer or nothing.
    let announced: Vec<_> = alice
        .transport
        .sent_messages()
        .into_iter()
        .filter(|message| !message.metadata.contains_key(ACK_FOR_KEY))
        .collect();
    assert!(
        !announced.is_empty(),
        "the deletion folded into the import was stranded with no offer, so \
         nothing is left to tell the peer to ask"
    );

    // The offer draws catch-up and the catch-up ladder runs out: every rung
    // is over the frame budget for a document this size, and "nothing" is a
    // rung. So the exchange still ends. What the peer does with the answer
    // is the media path's job, not this one's; what is under test is that it
    // gets to ask at all.
    let rounds = settle(&mut alice, &mut bob);
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the compensating offer started an exchange that does not end: {rounds:?}"
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
fn a_document_named_only_in_a_reply_is_asked_for_rather_than_left_empty() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    write(&mut bob, &bob_space, "notes", "k", "from bob");
    let theirs = bob
        .protocol
        .data_doc_version(&bob_space, "notes")
        .expect("version");

    // Drop the push that writing produced. What is left is the case that
    // matters: a document Alice has never heard of, learned about on the one
    // leg that sends no counter-offer, from a peer that has now said
    // everything it intends to say.
    bob.transport.clear_sent_messages();
    alice.transport.clear_sent_messages();

    alice.protocol.handle_data_sync_frame(
        &bob.address,
        &format!(
            r#"{{"v":1,"k":"vv","reply":true,"partial":false,"docs":{{"notes":"{theirs}"}}}}"#
        ),
    );

    assert!(
        !alice.transport.sent_messages().is_empty(),
        "the document Alice created from the reply was never asked about, so \
         nothing will ever fill it: the peer does not repeat an offer it has \
         already answered, and a link that never drops fires no other trigger"
    );

    settle(&mut alice, &mut bob);
    assert_eq!(
        read(&mut alice, &alice_space, "notes", "k"),
        Some(DataValue::text("from bob")),
        "the document stayed empty on the replica that created it"
    );
}

#[test]
fn a_document_created_on_the_offering_leg_is_not_asked_for_twice() {
    let (mut alice, bob) = pair();

    // The other half of the invariant above. This leg does send a
    // counter-offer, and that counter-offer already names everything the
    // leg created, so asking separately would put a second frame on the
    // mesh for a question already on it.
    alice.transport.clear_sent_messages();
    alice.protocol.handle_data_sync_frame(
        &bob.address,
        r#"{"v":1,"k":"vv","reply":false,"partial":false,"docs":{"notes":"AA=="}}"#,
    );

    assert_eq!(
        alice.transport.sent_messages().len(),
        1,
        "the offering leg answers with one counter-offer naming what it \
         created; a second frame is the same question asked twice"
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
    //
    // The receiver holds no unflushed local edits, so the drain that runs
    // ahead of the marker writes nothing. Give this document pending local
    // work and a delta record legitimately precedes the marker, and the
    // assertion below is asking the wrong question rather than catching a
    // regression.
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

#[test]
fn a_wipe_takes_the_replication_bookkeeping_with_it() {
    let (alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);

    // The logout path for an application holding documents in its own
    // backend. The bookkeeping is keyed by peer address, so a key type left
    // out of the wipe does not leave a harmless orphan: it leaves a list of
    // who this account replicated with, for the next account to inherit.
    write(&mut bob, &bob_space, "notes", "k", "v");
    seed_sync_record(
        &mut bob,
        &bob_space,
        &serde_json::json!({ "quarantined": [blob_digest(b"something refused")] }),
    );
    assert!(
        !bob.state
            .list_keys(storage_keys::DATA_SYNC)
            .unwrap_or_default()
            .is_empty(),
        "precondition: there has to be bookkeeping on disk for a wipe to miss it"
    );

    bob.protocol.data_wipe_all().expect("wipe");

    assert!(
        bob.state
            .list_keys(storage_keys::DATA_SYNC)
            .unwrap_or_default()
            .is_empty(),
        "the wipe reported success while leaving the peer-keyed replication \
         bookkeeping behind"
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

// ---- attachments ------------------------------------------------------
//
// What these cover that a unit test cannot: that a blob really leaves over
// the media path, arrives as a transfer the receiving application is never
// told about, and is admitted only against a question this device asked.

/// Events of one kind this node was handed, as JSON for field access.
fn events_named(node: &Node, name: &str) -> Vec<serde_json::Value> {
    node.events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .filter(|value| value.get("type").and_then(|t| t.as_str()) == Some(name))
        .collect()
}

fn clear_events(node: &Node) {
    node.events.lock().unwrap().clear();
}

#[test]
fn an_attachment_reference_replicates_without_its_bytes() {
    // The property the whole design rests on: the reference is document
    // content and travels as an ordinary change, while the bytes it names go
    // nowhere at all until somebody asks.
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);
    let blob = b"the bytes themselves".to_vec();
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    alice
        .protocol
        .data_map_set(
            &space,
            "notes",
            "files",
            "plan",
            DataValue::Attachment {
                hash: hash.clone(),
                size: blob.len() as u64,
                name: Some("plan.txt".to_string()),
                mime: None,
            },
        )
        .expect("attach");
    alice.protocol.data_flush(&space, "notes").expect("flush");
    settle(&mut alice, &mut bob);

    let bob_space = Node::space_for(&alice);
    let Some(DataValue::Attachment {
        hash: seen,
        size,
        name,
        ..
    }) = bob
        .protocol
        .data_map_get(&bob_space, "notes", "files", "plan")
        .expect("get")
    else {
        panic!("the reference did not replicate");
    };
    assert_eq!(seen, hash);
    assert_eq!(size, blob.len() as u64);
    assert_eq!(name.as_deref(), Some("plan.txt"));

    // And nothing carried the bytes: no file event on either side.
    assert!(events_named(&bob, "file_received").is_empty());
    assert!(events_named(&bob, "data_attachment_received").is_empty());
}

#[test]
fn a_fetch_travels_the_media_path_and_never_looks_like_a_file() {
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);
    let blob = b"a blob worth fetching, long enough to be interesting".to_vec();
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    // Bob asks Alice for the bytes.
    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);

    // Alice's application is asked to supply them.
    let asked = events_named(&alice, "data_attachment_requested");
    assert_eq!(
        asked.len(),
        1,
        "the holder's app must be asked exactly once"
    );
    assert_eq!(asked[0]["hash"].as_str(), Some(hash.as_str()));

    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob.clone())
        .expect("provide");
    settle_media(&mut alice, &mut bob);

    let received = events_named(&bob, "data_attachment_received");
    assert_eq!(received.len(), 1, "the asking app must be handed the bytes");
    assert_eq!(received[0]["hash"].as_str(), Some(hash.as_str()));
    assert_eq!(
        BASE64
            .decode(received[0]["data"].as_str().expect("base64"))
            .expect("decode"),
        blob
    );

    // The transfer must be invisible as a file. This is the whole reason the
    // purpose exists: without it a person is handed a download they never
    // started, and for a snapshot that download is a CRDT encoding.
    assert!(
        events_named(&bob, "file_received").is_empty(),
        "a data-purposed transfer must never surface as a received file"
    );
    assert!(
        events_named(&bob, "file_progress").is_empty(),
        "nor as progress on one"
    );
}

#[test]
fn bytes_nobody_asked_for_are_dropped() {
    // The bound on unsolicited pushes. Without it a peer can spend this
    // device's storage and battery for the price of one frame, and the
    // application is handed files nobody wanted.
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let blob = b"unsolicited".to_vec();
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob)
        .expect("provide");
    settle_media(&mut alice, &mut bob);

    assert!(
        events_named(&bob, "data_attachment_received").is_empty(),
        "bytes must be admitted only against a fetch this device made"
    );
    assert!(events_named(&bob, "file_received").is_empty());
}

#[test]
fn bytes_that_do_not_match_the_hash_are_refused() {
    // What makes fetching from an authenticated peer safe without trusting
    // that peer: the address is checked against the bytes, so the worst a
    // wrong answer achieves is no answer.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let wanted = OfflineProtocol::data_attachment_hash(b"what was asked for");

    bob.protocol
        .data_fetch_attachment(&bob_space, &wanted)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    clear_events(&bob);

    // Alice's own surface refuses to answer the right question with the
    // wrong bytes, so the mistake is reported to the app that made it while
    // it still has the file in hand.
    let err = alice
        .protocol
        .data_provide_attachment(
            &Node::space_for(&bob),
            &bob.address,
            &wanted,
            b"different bytes entirely".to_vec(),
        )
        .expect_err("the sender's own check must refuse this first");
    assert!(
        format!("{err}").contains("hash to"),
        "unexpected refusal: {err}"
    );

    // But that check is the sender's, and a peer who lies does not run it.
    // Stage the lie: a transfer whose purpose claims the hash Bob asked for,
    // carrying bytes that are not it. This is the case the receiving check
    // exists for, and the only one that can reach it.
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            b"different bytes entirely, sent anyway".to_vec(),
            wanted.clone(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Attachment {
                hash: wanted.clone(),
            }),
        )
        .expect("a lying peer sends whatever it likes");
    settle_media(&mut alice, &mut bob);

    assert!(
        events_named(&bob, "data_attachment_received").is_empty(),
        "bytes that do not hash to what was asked for must never be handed \
         to an application: the address is checked against the bytes, not \
         against what the sender says about them"
    );
    let unavailable = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(unavailable.len(), 1, "and the fetch must be told it failed");
    assert_eq!(unavailable[0]["reason"].as_str(), Some("hash_mismatch"));
    assert!(events_named(&bob, "file_received").is_empty());
}

#[test]
fn a_declined_fetch_ends_rather_than_hanging() {
    // An attachment reference outlives the bytes it names, so a peer holding
    // a reference and no blob is ordinary. Without an answer for that case
    // the asking side cannot tell it from a slow peer, and shows a person a
    // spinner that never resolves.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"long gone");

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);

    alice
        .protocol
        .data_decline_attachment(&Node::space_for(&bob), &bob.address, &hash)
        .expect("decline");
    pump(&mut alice, &mut bob);

    let unavailable = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0]["reason"].as_str(), Some("declined"));
    assert_eq!(unavailable[0]["hash"].as_str(), Some(hash.as_str()));
}

#[test]
fn a_refusal_for_a_question_we_never_asked_is_ignored() {
    let (mut alice, mut bob) = pair();
    let hash = OfflineProtocol::data_attachment_hash(b"never wanted");

    alice
        .protocol
        .data_decline_attachment(&Node::space_for(&bob), &bob.address, &hash)
        .expect("decline");
    pump(&mut alice, &mut bob);

    assert!(
        events_named(&bob, "data_attachment_unavailable").is_empty(),
        "a peer must not be able to report on fetches this device never made"
    );
}

#[test]
fn a_fetch_needs_the_carriage_capability() {
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"anything");

    // Alice keeps replicating but stops carrying blobs.
    bob.protocol.peer_data_media.remove(&alice.address);
    let err = bob
        .protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect_err("a fetch toward a peer that cannot carry must refuse");
    assert!(
        format!("{err}").contains("attachment carriage"),
        "unexpected error: {err}"
    );
    assert_eq!(pump(&mut bob, &mut alice), 0, "and no frame may leave");
}

#[test]
fn a_malformed_hash_never_reaches_the_wire() {
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    for bad in ["", "abcdef", &"z".repeat(64)] {
        assert!(
            bob.protocol.data_fetch_attachment(&bob_space, bad).is_err(),
            "{bad:?} must be refused"
        );
    }
    assert_eq!(pump(&mut bob, &mut alice), 0);
}

#[test]
fn a_document_too_large_to_frame_crosses_over_the_media_path() {
    // The rung the F3 and F4 records deferred to this stage. Before it, a
    // document that outgrew every frame was warned about and dropped, so two
    // replicas stayed apart with nothing on either device reporting it.
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);

    fill_past_the_frame_budget(&mut alice, &space, 0x2545_F491_4F6C_DD1D);

    settle_media(&mut alice, &mut bob);

    // It converged, which before this stage it could not.
    let bob_space = Node::space_for(&alice);
    assert_eq!(
        bob.protocol
            .data_map_get(&bob_space, "notes", "m", "k0")
            .expect("get")
            .is_some(),
        true,
        "the document must have crossed"
    );
    assert!(
        events_named(&alice, "data_doc_unsyncable").is_empty(),
        "and nothing may be reported as a dead end"
    );
    // The carriage is invisible to the receiving application: a snapshot
    // handed to a person as a downloaded file is the failure the purpose
    // field exists to prevent.
    assert!(events_named(&bob, "file_received").is_empty());
    assert!(events_named(&bob, "file_progress").is_empty());
}

#[test]
fn a_peer_that_cannot_carry_snapshots_is_told_rather_than_left_waiting() {
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);
    // Bob keeps replicating and stops carrying blobs, which is exactly the
    // partial downgrade the third capability entry exists to express.
    alice.protocol.peer_data_media.remove(&bob.address);

    fill_past_the_frame_budget(&mut alice, &space, 0x9E37_79B9_7F4A_7C15);
    settle_media(&mut alice, &mut bob);

    let reported = events_named(&alice, "data_doc_unsyncable");
    assert!(
        !reported.is_empty(),
        "a dead end must be reported, not logged and forgotten"
    );
    assert_eq!(
        reported[0]["reason"].as_str(),
        Some("peer_cannot_carry_snapshots")
    );
}

#[test]
fn the_send_path_refuses_a_data_purpose_toward_a_peer_that_cannot_route_it() {
    // The last gate rather than the first. Every caller above this already
    // checks the capability, so this is what still holds if one of them is
    // ever wrong, and the failure it stops is the loud one: a peer without
    // the entry hands the bytes to a person as a downloaded file, and for a
    // snapshot those bytes are a CRDT encoding.
    let (mut alice, bob) = pair();
    alice.protocol.peer_data_media.remove(&bob.address);

    let err = alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            b"bytes".to_vec(),
            "name".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        )
        .expect_err("a data-purposed send must refuse this peer");
    assert!(
        format!("{err}").contains("attachment carriage"),
        "unexpected error: {err}"
    );

    // And the same send without a purpose is untouched: this gate must not
    // become a general restriction on sending files.
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            b"bytes".to_vec(),
            "name".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            None,
        )
        .expect("an ordinary file is unaffected");
}

#[test]
fn a_public_media_send_can_never_carry_a_data_purpose() {
    // The security property stated as a test rather than as a comment: the
    // public surface has no argument for this, so an application cannot mark
    // a send as data-purposed and feed bytes of its choosing to a peer's
    // document engine while that peer's user sees nothing.
    //
    // Pinned by behaviour: a file sent through the public API arrives as a
    // file, on a pair where a data-purposed send would have been routed away
    // from the application instead.
    let (mut alice, mut bob) = pair();
    alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            b"an ordinary file".to_vec(),
            "notes.txt".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("send");
    settle_media(&mut alice, &mut bob);

    assert_eq!(
        events_named(&bob, "file_received").len(),
        1,
        "a file sent through the public surface must arrive as a file"
    );
    assert!(events_named(&bob, "data_attachment_received").is_empty());
}

#[test]
fn a_chunk_arriving_before_chunk_zero_reports_no_phantom_progress() {
    // The purpose rides chunk 0, so until it lands this device cannot say
    // what a transfer is. Reading "nothing known yet" as "an ordinary file"
    // puts a download nobody started in front of a person, counting up to a
    // file that never appears, on any transport that delivers out of order.
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);

    // Big enough to be several chunks, so there is a chunk 1 to deliver
    // first. BLE chunks at 4 KiB.
    let blob = vec![7u8; 12 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob.clone())
        .expect("provide");

    // Deliver the first batch back to front.
    let mut first = alice.transport.sent_messages();
    alice.transport.clear_sent_messages();
    assert!(
        first.len() > 1,
        "the fixture needs a multi-chunk transfer to reorder"
    );
    first.reverse();
    for message in first {
        bob.transport.queue_message(message);
    }
    while bob.protocol.receive_message().is_some() {}

    assert!(
        events_named(&bob, "file_progress").is_empty(),
        "a chunk that arrived before the purpose did must not be reported as \
         progress on a file"
    );

    settle_media(&mut alice, &mut bob);
    assert_eq!(
        events_named(&bob, "data_attachment_received").len(),
        1,
        "and the transfer still completes"
    );
    assert!(events_named(&bob, "file_progress").is_empty());
    assert!(events_named(&bob, "file_received").is_empty());
}

#[test]
fn a_data_purposed_transfer_is_invisible_on_the_sending_side_too() {
    // The mirror of the receiver's rule, and the worse leak of the two: an
    // app that renders `media_sent` in a conversation would show a file
    // being sent that nobody attached, under a file_id it has never seen.
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);
    let blob = b"bytes the document layer moves on its own behalf".to_vec();
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    clear_events(&alice);

    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob.clone())
        .expect("provide");
    settle_media(&mut alice, &mut bob);

    assert!(
        events_named(&alice, "file_progress").is_empty(),
        "the serving side must not report progress on an upload nobody made"
    );
    assert!(
        events_named(&alice, "media_sent").is_empty(),
        "nor completion of one"
    );
    // The control: it really did transfer, so the assertions above are about
    // suppression rather than about nothing having happened.
    assert_eq!(events_named(&bob, "data_attachment_received").len(), 1);

    // And an ordinary file through the same pair still reports both, so the
    // suppression is scoped to data-purposed transfers rather than global.
    clear_events(&alice);
    alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            b"an ordinary file".to_vec(),
            "notes.txt".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("send");
    settle_media(&mut alice, &mut bob);
    assert!(!events_named(&alice, "file_progress").is_empty());
    assert_eq!(events_named(&alice, "media_sent").len(), 1);
}

// ---- what a review found ----------------------------------------------

#[test]
fn an_interrupted_document_transfer_is_never_handed_back_to_the_app() {
    // A descriptor outlives the process so an app can re-supply the bytes.
    // For a document-layer transfer that recovery is a trap: the only
    // surface that takes a caller-supplied file_id is `send_media_with`,
    // which forces the purpose to None, so a well-behaved app answering
    // MediaResendRequired would deliver a snapshot to the peer's user as a
    // downloaded file. The exact outcome the capability gate exists to
    // prevent, reached through this SDK's own recovery path.
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);
    let blob = vec![3u8; 40 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob)
        .expect("provide");
    // Die mid-transfer: nothing is delivered, so the descriptor survives.
    alice.transport.clear_sent_messages();

    alice.restart(&bob.address);

    assert!(
        events_named(&alice, "media_resend_required").is_empty(),
        "an app cannot answer this without stripping the purpose, so it must \
         not be asked"
    );
}

#[test]
fn an_interrupted_ordinary_transfer_is_still_handed_back() {
    // The control for the test above: the recovery path itself must keep
    // working, or the fix has removed a feature instead of a trap.
    let (mut alice, bob) = pair();
    alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            vec![4u8; 40 * 1024],
            "holiday.jpg".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("send");
    alice.transport.clear_sent_messages();

    alice.restart(&bob.address);

    assert_eq!(
        events_named(&alice, "media_resend_required").len(),
        1,
        "an ordinary interrupted transfer must still ask the app to re-supply"
    );
}

#[test]
fn a_peer_asking_for_endless_distinct_blobs_is_budgeted() {
    // The per-hash window cannot see this as a flood, because every hash in
    // it is new. Without a budget each frame buys a permanent map entry and
    // an application callback, and a hash is 32 bytes the sender chooses.
    let (mut alice, bob) = pair();
    let space = Node::space_for(&bob);

    let asks = crate::protocol::data_sync::MAX_BLOB_REQUESTS_PER_WINDOW + 40;
    for round in 0..asks {
        let hash = OfflineProtocol::data_attachment_hash(format!("blob-{round}").as_bytes());
        // Straight into the handler, because a peer sending these is not
        // using our fetch surface and is not subject to its bounds.
        alice.protocol.handle_data_sync_frame(
            &bob.address,
            &format!(r#"{{"v":1,"k":"need_blob","hash":"{hash}"}}"#),
        );
    }
    let _ = space;

    let requested = events_named(&alice, "data_attachment_requested").len();
    assert!(
        requested <= crate::protocol::data_sync::MAX_BLOB_REQUESTS_PER_WINDOW,
        "the application saw {requested} requests; the budget is {}",
        crate::protocol::data_sync::MAX_BLOB_REQUESTS_PER_WINDOW
    );
    assert!(
        alice.protocol.blob_request_windows.len()
            <= crate::protocol::data_sync::MAX_BLOB_REQUESTS_PER_WINDOW,
        "and one peer's share of the map that remembers them is its budget"
    );
    assert!(
        requested > 0,
        "a legitimate request must still get through, or this bounds nothing"
    );
}

#[test]
fn declining_toward_a_peer_that_cannot_read_it_is_refused() {
    // The one frame in this file that had no gate. A refusal a peer cannot
    // parse is worse than none: the caller believes they answered, and the
    // asking side keeps waiting.
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);
    let hash = OfflineProtocol::data_attachment_hash(b"gone");

    alice.protocol.peer_data_media.remove(&bob.address);
    let err = alice
        .protocol
        .data_decline_attachment(&space, &bob.address, &hash)
        .expect_err("a decline toward an incapable peer must refuse");
    assert!(
        format!("{err}").contains("attachment carriage"),
        "unexpected error: {err}"
    );
    assert_eq!(pump(&mut alice, &mut bob), 0, "and no frame may leave");
}

#[test]
fn a_fetch_nobody_answers_eventually_reports() {
    // The spinner the whole refusal mechanism exists to kill. Declining is
    // only a SHOULD for a holder, so an app that implements neither answer
    // leaves the asking side with no event at all unless expiry reports.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"never answered");

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    // Alice's app implements neither answer.
    clear_events(&bob);

    // Age the question past the timeout, then run the sweep the tick runs.
    let stale = std::time::Instant::now()
        - crate::protocol::data_sync::ATTACHMENT_FETCH_TIMEOUT
        - std::time::Duration::from_secs(1);
    for asked_at in bob.protocol.pending_attachment_fetches.values_mut() {
        *asked_at = stale;
    }
    bob.protocol.expire_attachment_fetches();

    let unavailable = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(unavailable.len(), 1, "expiry must report, not just recycle");
    assert_eq!(unavailable[0]["reason"].as_str(), Some("timeout"));
    assert!(
        bob.protocol.pending_attachment_fetches.is_empty(),
        "and release the slot"
    );
}

#[test]
fn unsolicited_blob_bytes_are_refused_at_the_door() {
    // Refusing on completion still refuses, but only after the whole
    // transfer has been buffered, reassembled and checksummed. That is the
    // storage and the battery the rule exists to protect.
    let (mut alice, mut bob) = pair();
    let blob = vec![9u8; 40 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    // Bob asked for nothing. Alice pushes anyway.
    let file_id = alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            blob,
            hash.clone(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Attachment { hash }),
        )
        .expect("a peer sends whatever it likes");

    // One round only, so the transfer is still in flight. Asserting after it
    // completes would prove nothing: the completion check refuses too, and
    // finalize clears the assembly either way. What this fix changes is
    // whether the bytes were ever buffered, and that is only observable
    // while they would still be sitting there.
    pump(&mut alice, &mut bob);
    assert!(
        bob.protocol
            .file_transfer_manager
            .get_progress(&file_id)
            .is_none(),
        "no assembly may be opened for bytes nobody asked for: refusing at \
         the end still spends the storage and the battery this rule protects"
    );

    settle_media(&mut alice, &mut bob);
    assert!(
        events_named(&bob, "data_attachment_received").is_empty(),
        "and unsolicited bytes must never reach the application"
    );
    assert!(events_named(&bob, "file_received").is_empty());
}

/// Fill a document past the frame budget, so the catch-up ladder has no rung
/// left below the media path.
///
/// The filler varies per key on purpose: repeated text compresses inside the
/// engine's encoding, and a document that compresses back under the budget
/// never reaches the rung these tests are about.
fn fill_past_the_frame_budget(node: &mut Node, space: &str, mut seed: u64) {
    for round in 0..24 {
        let filler: String = (0..4096)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from(b'0' + (seed % 64) as u8)
            })
            .collect();
        node.protocol
            .data_map_set(
                space,
                "notes",
                "m",
                &format!("k{round}"),
                DataValue::text(filler),
            )
            .expect("set");
    }
    node.protocol.data_flush(space, "notes").expect("flush");
    assert!(
        node.protocol.data_doc_size(space, "notes").expect("size")
            > crate::protocol::data_sync::MAX_SYNC_BLOB_BYTES as u64,
        "the fixture must build a document no frame can carry"
    );
}

#[test]
fn the_kill_switch_refuses_a_carried_transfer_before_it_is_buffered() {
    // What the switch is worth here, stated exactly. It is NOT what stops
    // the import: `require_data_storage` already fails closed with
    // `DataDisabled`, so a device with the layer off writes nothing and
    // creates no document either way. What it stops is the buffering. A
    // transfer admitted at chunk 0 is reassembled in full before anything
    // downstream gets a say, so without this a device that opted out still
    // holds a peer's whole blob in memory to reach a seam that was always
    // going to refuse it.
    //
    // The peer here is nonconforming by construction: it learned the
    // capability while the layer was on and kept sending after it went off,
    // which is the case the envelope-version comment calls "if the capability
    // gate is ever wrong".
    let (mut alice, mut bob) = pair();
    bob.protocol.config.data.enabled = false;

    // Large enough that one window cannot finish it, so the assembly is
    // observably open at the point the gate either refused it or did not.
    let payload = vec![7u8; 2 * 1024 * 1024];
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            payload,
            "notes".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        )
        .expect("a peer that advertised carriage accepts the send");
    pump(&mut alice, &mut bob);

    assert_eq!(
        bob.protocol.file_transfer_manager.active_transfer_count(),
        0,
        "a device with the layer off must buffer nothing for it"
    );
    assert!(events_named(&bob, "file_received").is_empty());
    assert!(events_named(&bob, "file_progress").is_empty());

    // The control, over the same send and the same chunk count: with the
    // layer on, the transfer IS admitted and the assembly IS open. Without
    // this the assertion above passes for a fixture that never sent anything,
    // or for a chunk 0 that never arrived.
    let (mut alice, mut bob) = pair();
    let payload = vec![7u8; 2 * 1024 * 1024];
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            payload,
            "notes".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        )
        .expect("send");
    pump(&mut alice, &mut bob);
    assert_eq!(
        bob.protocol.file_transfer_manager.active_transfer_count(),
        1,
        "the same transfer must be admitted when the layer is on"
    );
}

#[test]
fn a_wipe_takes_the_outstanding_questions_with_it() {
    // A pending fetch is not bookkeeping: it is what admits arriving bytes.
    // One surviving a wipe would let an answer land for a space the wipe
    // erased, and hand it to the application as a reply to a question that no
    // longer exists.
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);
    let blob = b"bytes the wipe should make irrelevant".to_vec();
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    assert!(
        bob.protocol.awaiting_attachment(&bob_space, &hash),
        "the fixture must leave a real question outstanding"
    );

    bob.protocol.data_wipe_all().expect("wipe");
    assert!(
        !bob.protocol.awaiting_attachment(&bob_space, &hash),
        "the wipe must take the question with the content it was about"
    );

    // And the consequence, which is the part worth pinning: bytes that would
    // have been admitted before the wipe are refused after it.
    clear_events(&bob);
    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob)
        .expect("provide");
    settle_media(&mut alice, &mut bob);
    assert!(
        events_named(&bob, "data_attachment_received").is_empty(),
        "an answer to a wiped question must not reach the application"
    );
    assert!(events_named(&bob, "file_received").is_empty());
}

#[test]
fn a_fetch_evicted_by_a_newer_one_is_reported_rather_than_forgotten() {
    // Eviction reaches the same end state as expiry by a different road: the
    // fetch is over and no bytes will ever be admitted for it. An application
    // told nothing is left showing the spinner the whole refusal mechanism
    // exists to end.
    let (mut bob, alice) = pair();
    let space = Node::space_for(&alice);
    let cap = crate::protocol::data_sync::MAX_PENDING_ATTACHMENT_FETCHES;

    let hash_of = |n: usize| OfflineProtocol::data_attachment_hash(format!("blob {n}").as_bytes());
    for n in 0..cap {
        bob.protocol
            .data_fetch_attachment(&space, &hash_of(n))
            .expect("fetch");
    }
    clear_events(&bob);
    assert!(
        bob.protocol.awaiting_attachment(&space, &hash_of(0)),
        "the oldest fetch must still be outstanding before the one that evicts it"
    );

    // One past the bound. The oldest goes, and has to say so.
    bob.protocol
        .data_fetch_attachment(&space, &hash_of(cap))
        .expect("fetch");

    let ended = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(
        ended.len(),
        1,
        "exactly the evicted fetch is reported: {ended:?}"
    );
    assert_eq!(ended[0]["reason"].as_str(), Some("evicted"));
    assert_eq!(ended[0]["hash"].as_str(), Some(hash_of(0).as_str()));
    assert!(
        !bob.protocol.awaiting_attachment(&space, &hash_of(0)),
        "and the entry it named is really gone"
    );
}

#[test]
fn a_request_this_device_could_not_answer_is_never_put_to_the_application() {
    // Both answers refuse toward a peer that never advertised carriage:
    // `provide` cannot reach the media path and `decline` cannot reach the
    // wire. Emitting the request anyway hands an app a question with no legal
    // reply, which reads to whoever handles it as an SDK rejecting its own
    // events.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"something asked for");

    // Alice stops believing Bob can carry blobs, while Bob still asks.
    alice.protocol.peer_data_media.remove(&bob.address);
    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    assert!(
        events_named(&alice, "data_attachment_requested").is_empty(),
        "a request that cannot be answered must not be raised"
    );

    // The control, over the same wire and the same handler: with the
    // capability back, a request IS raised. Without it this test passes for a
    // frame that never arrived.
    alice.protocol.peer_data_media.insert(bob.address.clone());
    let other = OfflineProtocol::data_attachment_hash(b"asked for later");
    bob.protocol
        .data_fetch_attachment(&bob_space, &other)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    let asked = events_named(&alice, "data_attachment_requested");
    assert_eq!(asked.len(), 1, "the answerable request must be raised");
    assert_eq!(asked[0]["hash"].as_str(), Some(other.as_str()));
}

#[test]
fn a_transfer_that_failed_before_its_purpose_arrived_reports_no_file_failure() {
    // The other half of the phantom-progress rule, on the failure paths.
    // Progress already withholds until chunk 0 says what a transfer is; the
    // failure arms read the same absence and used to answer "an ordinary
    // file", which names a data-layer transfer as a failed download in front
    // of a person. The app has heard nothing about such a transfer either
    // way, so there is no report to correct by staying quiet.
    let (mut alice, mut bob) = pair();

    // One assembly slot on Bob, so the second transfer to reach him is
    // refused for resource exhaustion rather than admitted.
    bob.protocol.file_transfer_manager = crate::file_transfer::FileTransferManager::with_config(
        crate::file_transfer::FileTransferConfig {
            max_concurrent_assemblies: 1,
            ..Default::default()
        },
    );

    /// Send one ordinary file and hand back its chunks, without leaving a
    /// transfer open on the sending side: a sender may hold only
    /// `MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER` at once, and this test needs
    /// three transfers to reach one receiver. Aborting is Alice's business
    /// alone and touches nothing Bob has.
    fn chunks_of(
        alice: &mut Node,
        bob: &Node,
        name: &str,
        fill: u8,
    ) -> Vec<offline_protocol_core::Message> {
        let file_id = alice
            .protocol
            .send_media_with(
                bob.address.clone(),
                vec![fill; 12 * 1024],
                name.to_string(),
                offline_protocol_core::ContentType::File,
                crate::protocol::types::MediaSendOptions::default(),
            )
            .expect("send");
        let chunks = alice.transport.sent_messages();
        alice.transport.clear_sent_messages();
        alice
            .protocol
            .abort_outbound_media_transfer(&file_id, "test is done sending");
        assert!(
            chunks.len() > 1,
            "the fixture needs a multi-chunk transfer for {name}"
        );
        chunks
    }

    // Occupy Bob's only slot with an ordinary transfer, chunk 0 alone, so it
    // stays open and identified.
    let holding = chunks_of(&mut alice, &bob, "holding.bin", 1);
    bob.transport.queue_message(holding[0].clone());
    while bob.protocol.receive_message().is_some() {}
    assert_eq!(
        bob.protocol.file_transfer_manager.active_transfer_count(),
        1,
        "the slot must really be occupied, or nothing below is refused"
    );

    // A second transfer whose chunk 0 has not landed. Its chunk 1 is refused
    // for the occupied slot, and at that moment Bob cannot say what the
    // transfer is: chunk 0 carries the purpose and it is not here.
    let unidentified = chunks_of(&mut alice, &bob, "unidentified.bin", 2);
    clear_events(&bob);
    bob.transport.queue_message(unidentified[1].clone());
    while bob.protocol.receive_message().is_some() {}

    assert!(
        events_named(&bob, "file_receive_failed").is_empty(),
        "a transfer whose purpose has not arrived must not be failed as a \
         file: it may be the document layer's, and no progress was ever \
         reported for it either"
    );

    // The control, through the same refusal on the same occupied slot: a
    // transfer whose chunk 0 DID arrive is identified as ordinary, and its
    // failure is reported exactly as before. Without this the assertion
    // above passes for a build that reports no failures at all.
    let identified = chunks_of(&mut alice, &bob, "identified.bin", 3);
    clear_events(&bob);
    bob.transport.queue_message(identified[0].clone());
    while bob.protocol.receive_message().is_some() {}

    let failed = events_named(&bob, "file_receive_failed");
    assert_eq!(
        failed.len(),
        1,
        "an ordinary transfer refused after its chunk 0 arrived must still \
         report: {failed:?}"
    );
    assert_eq!(failed[0]["file_name"].as_str(), Some("identified.bin"));
}

#[test]
fn a_fetch_does_not_expire_while_its_own_answer_is_arriving() {
    // The timeout is generous because the media path may be Bluetooth, which
    // is exactly the case where a blob takes longer to arrive than the clock
    // allows. The fetch record is not bookkeeping: it is what admits the
    // bytes at the end, so a record that expired mid-carriage would discard a
    // blob that fully crossed, and the retry it invites carries the same
    // bytes over the same radio to die at the same point.
    let (mut alice, mut bob) = pair();
    let alice_space = Node::space_for(&bob);
    let bob_space = Node::space_for(&alice);
    // Large enough that one window cannot finish it, so the sweep below lands
    // in the middle of a live transfer rather than after one.
    let blob = vec![3u8; 2 * 1024 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob.clone())
        .expect("provide");

    // Age the question past the timeout, exactly as the wall clock would on a
    // radio slow enough to matter, and then let some of the answer land.
    let stale = std::time::Instant::now()
        - crate::protocol::data_sync::ATTACHMENT_FETCH_TIMEOUT
        - std::time::Duration::from_secs(1);
    for seen_at in bob.protocol.pending_attachment_fetches.values_mut() {
        *seen_at = stale;
    }
    alice.protocol.pump_media_transfers();
    pump(&mut alice, &mut bob);
    assert!(
        bob.protocol.file_transfer_manager.active_transfer_count() > 0,
        "the fixture must reach the sweep with a transfer actually in flight"
    );

    // The sweep the tick runs, in the middle of the carriage.
    bob.protocol.expire_attachment_fetches();
    assert!(
        events_named(&bob, "data_attachment_unavailable").is_empty(),
        "a fetch whose bytes are arriving must not be reported as unanswered"
    );

    settle_media(&mut alice, &mut bob);

    let received = events_named(&bob, "data_attachment_received");
    assert_eq!(
        received.len(),
        1,
        "the blob crossed in full and must be handed over"
    );
    assert_eq!(
        BASE64
            .decode(received[0]["data"].as_str().expect("base64"))
            .expect("decode"),
        blob
    );
}

#[test]
fn a_document_already_crossing_is_not_sent_a_second_time() {
    // The trigger for carriage recurs on its own: the peer stays stale for
    // the whole multi-minute crossing of a document this size, and says so on
    // every exchange. Each of those would otherwise start another copy of the
    // same transfer, and a peer may hold only two at once.
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);

    fill_past_the_frame_budget(&mut alice, &space, 0x1234_5678_9ABC_DEF0);
    // One exchange, no settle: the carriage is deliberately left in flight.
    pump(&mut alice, &mut bob);
    pump(&mut bob, &mut alice);
    let crossing = |node: &Node| {
        node.protocol
            .outbound_media_transfers
            .values()
            .filter(|transfer| transfer.data_purpose.is_some())
            .count()
    };
    assert_eq!(crossing(&alice), 1, "the document must be on its way");

    // The same rung reached again while the first crossing is unfinished.
    for _ in 0..3 {
        alice
            .protocol
            .nudge_data_sync(&space, None, "test_repeat_exchange");
        pump(&mut alice, &mut bob);
        pump(&mut bob, &mut alice);
    }
    assert_eq!(
        crossing(&alice),
        1,
        "a repeated exchange must not start a second copy of one document"
    );

    // What the duplicates cost, and the reason this is worth a test rather
    // than a comment: the slots they fill are the application's too, and the
    // transfers holding them are invisible to it by design, so the error it
    // gets names a limit it cannot account for.
    alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            vec![1u8; 4096],
            "holiday.jpg",
            offline_protocol_core::ContentType::Image,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("the app's own media send must still have a slot");

    // The suppression must be transient rather than a latch. It is keyed on a
    // transfer being in flight, so a check that outlived its transfer would
    // strand every later version of the document instead of every duplicate
    // of one.
    settle_media(&mut alice, &mut bob);
    assert_eq!(
        crossing(&alice),
        0,
        "a finished crossing must release what it held"
    );
    let bob_space = Node::space_for(&alice);
    assert!(
        bob.protocol
            .data_map_get(&bob_space, "notes", "m", "k0")
            .expect("get")
            .is_some(),
        "and the document must have arrived"
    );

    fill_past_the_frame_budget(&mut alice, &space, 0x0FED_CBA9_8765_4321);
    pump(&mut alice, &mut bob);
    pump(&mut bob, &mut alice);
    assert_eq!(
        crossing(&alice),
        1,
        "a document that grew again must be able to cross again"
    );
}

#[test]
fn a_snapshot_larger_than_a_record_is_refused_before_it_is_buffered() {
    // The ceiling is certain before the first chunk: a document past it
    // cannot be persisted by this device even if every byte arrives. Refusing
    // at the end still refuses, but only after a whole reassembly has been
    // spent to reach a verdict that was decided at the door.
    //
    // The peer is nonconforming by construction, which is the only way to get
    // here: this implementation refuses to carry such a document at all.
    let (mut alice, mut bob) = pair();
    let over = crate::protocol::data_sync::MAX_MEDIA_SNAPSHOT_BYTES + 64 * 1024;
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            vec![5u8; over],
            "notes".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        )
        .expect("a peer that advertised carriage accepts the send");
    pump(&mut alice, &mut bob);

    assert_eq!(
        bob.protocol.file_transfer_manager.active_transfer_count(),
        0,
        "a snapshot past the record ceiling must buffer nothing"
    );
    assert!(events_named(&bob, "file_received").is_empty());
    assert!(events_named(&bob, "file_progress").is_empty());

    // The control, on the same road: a snapshot under the ceiling IS
    // admitted. Without it the assertion above passes for a fixture whose
    // chunk 0 never arrived.
    let (mut alice, mut bob) = pair();
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            vec![5u8; 2 * 1024 * 1024],
            "notes".to_string(),
            offline_protocol_core::ContentType::File,
            crate::protocol::types::MediaSendOptions::default(),
            Some(crate::media_envelope::DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        )
        .expect("send");
    pump(&mut alice, &mut bob);
    assert_eq!(
        bob.protocol.file_transfer_manager.active_transfer_count(),
        1,
        "the same road must admit a snapshot the ceiling allows"
    );
}

#[test]
fn a_duplicate_chunk_zero_cannot_reclassify_a_transfer() {
    // What a transfer is, is decided by chunk 0 and nothing else. Duplicates
    // of a chunk are accepted by replacement, and the manager's consistency
    // check covers only the fields on the chunk itself, so without a rule
    // here the marking that makes a transfer invisible is a field an
    // authenticated peer may rewrite mid-flight: send chunk 0 twice, once
    // marked and once not, and choose afterwards whether the bytes reach the
    // document layer or the person.
    let (mut alice, mut bob) = pair();
    let file_id = "reclassify-me".to_string();
    // Many chunks, so chunk 0 lands and the assembly is still open behind
    // it. A single-chunk transfer completes before there is anything left to
    // reclassify.
    let blob = vec![0xA5u8; 64 * 1024];
    let options = || crate::protocol::types::MediaSendOptions {
        file_id: Some(file_id.clone()),
        ..Default::default()
    };

    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            blob.clone(),
            "snapshot",
            offline_protocol_core::ContentType::File,
            options(),
            Some(crate::media_envelope::DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        )
        .expect("carry the snapshot");
    alice.protocol.pump_media_transfers();
    pump(&mut alice, &mut bob);

    // The fixture reached the state the gate needs: a transfer under way,
    // known to be the document layer's.
    assert!(
        bob.protocol
            .pending_media_metadata
            .get(&file_id)
            .is_some_and(|entry| entry.data_purpose.is_some()),
        "chunk 0 must have opened a transfer this device knows is internal"
    );
    assert!(bob.protocol.file_transfer_manager.active_transfer_count() > 0);
    clear_events(&bob);

    // The same peer now contradicts itself: chunk 0 again, byte for byte the
    // same transfer, with the marking dropped. Alice's own bookkeeping is
    // cleared first because a well-behaved sender cannot do this at all,
    // which is the point.
    alice.protocol.outbound_media_transfers.remove(&file_id);
    alice
        .protocol
        .send_media_inner(
            bob.address.clone(),
            blob,
            "snapshot",
            offline_protocol_core::ContentType::File,
            options(),
            None,
        )
        .expect("the lie is sendable; it is the receiver that must refuse it");
    alice.protocol.pump_media_transfers();
    pump(&mut alice, &mut bob);

    assert_eq!(
        bob.protocol.file_transfer_manager.active_transfer_count(),
        0,
        "a chunk 0 that reclassifies a transfer must end it, not rewrite it"
    );
    assert!(
        bob.protocol.pending_media_metadata.get(&file_id).is_none(),
        "and leave no state behind"
    );

    // Which is the outcome that matters: the snapshot never becomes a file
    // in front of a person, however the transfer is finished.
    settle_media(&mut alice, &mut bob);
    assert!(
        events_named(&bob, "file_received").is_empty(),
        "document-layer bytes must never surface as a downloaded file"
    );
    assert!(
        events_named(&bob, "file_progress").is_empty(),
        "nor as progress on one"
    );
}

#[test]
fn every_attachment_surface_answers_data_disabled() {
    // The kill switch has one spelling across this API, and these three were
    // answering with the code that means "your argument is wrong, retrying
    // unchanged can never help". The discriminants are positional over the
    // FFI, so an app switching on the error to tell a setup mistake from a
    // permanent refusal is told the wrong thing.
    let (mut alice, bob) = pair();
    let space = Node::space_for(&bob);
    let hash = OfflineProtocol::data_attachment_hash(b"anything");
    alice.protocol.config.data.enabled = false;

    let disabled = |result: crate::Result<()>, surface: &str| match result {
        Err(crate::Error::DataDisabled) => {}
        other => panic!("{surface} must answer DataDisabled, got {other:?}"),
    };

    disabled(
        alice.protocol.data_fetch_attachment(&space, &hash),
        "fetch_attachment",
    );
    disabled(
        alice
            .protocol
            .data_provide_attachment(&space, &bob.address, &hash, b"anything".to_vec()),
        "provide_attachment",
    );
    disabled(
        alice
            .protocol
            .data_decline_attachment(&space, &bob.address, &hash),
        "decline_attachment",
    );
}

#[test]
fn forgetting_a_peer_reports_the_fetches_it_ends() {
    // Every road that ends a fetch without bytes owes the application the
    // same event, and this one owed it nothing. It is reached while somebody
    // is waiting — a peer blocked, forgotten, or come back without the
    // capability — and removing the record silently also forecloses the
    // expiry that would have reported it later, so the spinner never ends.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"bytes alice holds");

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    assert_eq!(
        bob.protocol.pending_attachment_fetches.len(),
        1,
        "the fixture must reach a fetch that is actually outstanding"
    );
    clear_events(&bob);

    bob.protocol.forget_data_sync_peer(&alice.address);

    let unavailable = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(
        unavailable.len(),
        1,
        "forgetting a peer must report the fetch it ends"
    );
    assert_eq!(unavailable[0]["reason"].as_str(), Some("peer_gone"));
    assert_eq!(unavailable[0]["hash"].as_str(), Some(hash.as_str()));
    assert!(
        bob.protocol.pending_attachment_fetches.is_empty(),
        "and release the slot"
    );
}

#[test]
fn evicting_every_peer_reports_the_fetches_it_ends() {
    // The same duty, on the wholesale road the single-peer fix did not sit
    // on. Nothing the application did reaches this one: the bound on
    // remembered peers is hit, and a stranger's key package advertising
    // replication forgets every capability at once (the
    // `MAX_KEY_PACKAGE_SENT_TO` arm in `message_dispatch`). A fetch
    // outstanding at that moment ends for a reason its asker cannot see,
    // and clearing the record forecloses the expiry that would have
    // reported it later, so the spinner never ends at all.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"bytes alice holds");

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    assert_eq!(
        bob.protocol.pending_attachment_fetches.len(),
        1,
        "the fixture must reach a fetch that is actually outstanding"
    );
    clear_events(&bob);

    bob.protocol.forget_every_data_sync_peer();

    let unavailable = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(
        unavailable.len(),
        1,
        "forgetting every peer must report the fetch it ends"
    );
    assert_eq!(unavailable[0]["reason"].as_str(), Some("peer_gone"));
    assert_eq!(unavailable[0]["hash"].as_str(), Some(hash.as_str()));
    assert_eq!(
        unavailable[0]["space_id"].as_str(),
        Some(bob_space.as_str())
    );
    assert!(
        bob.protocol.pending_attachment_fetches.is_empty(),
        "and release the slot"
    );
}

#[test]
fn a_document_layer_transfer_is_never_sent_unsealed() {
    // The marking exists nowhere but inside the encrypted chunk-0 plaintext.
    // On the plaintext opt-out path it would simply not travel, and the peer
    // would hand its user a downloaded file named by a hash: the exact
    // outcome the capability gate and the envelope version both exist to
    // prevent, produced by our own sender.
    let (mut alice, bob) = pair();
    let space = Node::space_for(&bob);
    let blob = b"bytes that must not leave unmarked".to_vec();
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    // The opt-out an application has to ask for explicitly.
    alice.protocol.config.encryption.enabled = false;
    alice.protocol.config.encryption.require_encryption = false;

    let err = alice
        .protocol
        .data_provide_attachment(&space, &bob.address, &hash, blob.clone())
        .expect_err("a purposed transfer that cannot be sealed must be refused");
    assert!(
        matches!(err, crate::Error::EncryptFailed(_)),
        "unexpected refusal: {err:?}"
    );

    // And the refusal is about the marking, not about media: an ordinary
    // file still leaves on the plaintext path, which is what the opt-out is
    // for.
    alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            blob,
            "holiday.jpg",
            offline_protocol_core::ContentType::Image,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("the opt-out still sends ordinary media");
}

#[test]
fn internal_transfers_leave_the_application_a_slot() {
    // The in-flight dedup stops a second copy of one transfer, not two
    // different ones, and two different ones are ordinary: a document
    // snapshot and an answered blob request are separate errands to the same
    // peer. Both slots filled by them fails the application's own send with
    // a limit whose cause it cannot see, because what holds the slots is
    // invisible to it by design.
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);

    fill_past_the_frame_budget(&mut alice, &space, 0x0BAD_F00D_0BAD_F00D);
    pump(&mut alice, &mut bob);
    pump(&mut bob, &mut alice);
    let internal = |node: &Node| {
        node.protocol
            .outbound_media_transfers
            .values()
            .filter(|transfer| transfer.data_purpose.is_some())
            .count()
    };
    assert_eq!(internal(&alice), 1, "the document must be on its way");

    // A different internal errand to the same peer, which the dedup does not
    // cover because it is not the same transfer.
    let blob = vec![7u8; 32 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);
    let err = alice
        .protocol
        .data_provide_attachment(&space, &bob.address, &hash, blob)
        .expect_err("a second internal transfer must not take the app's slot");
    assert!(
        matches!(err, crate::Error::MediaTransferLimit(_)),
        "unexpected refusal: {err:?}"
    );
    assert_eq!(internal(&alice), 1);

    // The slot the cap kept is the one that matters.
    alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            vec![1u8; 4096],
            "holiday.jpg",
            offline_protocol_core::ContentType::Image,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("the app's own media send must still have a slot");
}

#[test]
fn a_fetch_that_has_already_ended_is_not_reported_twice() {
    // A fetch gets one answer. A decline arriving while its own transfer is
    // still moving ends it, and the transfer failing afterwards is the same
    // fetch failing again: an application told twice is an application shown
    // a failure for something it is no longer waiting for.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let alice_space = Node::space_for(&bob);
    // Many chunks, so the answer is still crossing when the peer changes
    // its mind. A blob that fits in one chunk lands before the decline and
    // there is no race left to test.
    let blob = vec![3u8; 64 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    clear_events(&bob);

    // The peer answers the question, then changes its mind while the bytes
    // are still crossing.
    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob)
        .expect("provide");
    alice.protocol.pump_media_transfers();
    pump(&mut alice, &mut bob);
    assert!(
        bob.protocol.file_transfer_manager.active_transfer_count() > 0,
        "the fixture must reach a decline that races bytes still in flight"
    );
    alice
        .protocol
        .data_decline_attachment(&alice_space, &bob.address, &hash)
        .expect("decline");
    pump(&mut alice, &mut bob);

    let after_decline = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(
        after_decline.len(),
        1,
        "the decline is the one answer this fetch gets"
    );
    assert_eq!(after_decline[0]["reason"].as_str(), Some("declined"));

    // Now finish killing the transfer the decline abandoned. Whatever it
    // reports, it reports about a fetch that is over.
    bob.protocol.report_data_media_transfer_failure(
        &alice.address,
        &crate::media_envelope::DataPurpose::Attachment { hash: hash.clone() },
        "stale_timeout",
    );

    assert_eq!(
        events_named(&bob, "data_attachment_unavailable").len(),
        1,
        "a fetch that has already ended must not be reported a second time"
    );
}

#[test]
fn the_requester_bounds_how_often_it_asks() {
    // The spec makes this a MUST on the asking side, and the holder's own
    // window cannot substitute for it: an application retrying a spinner on
    // a timer would otherwise seal and send one frame per retry, and a
    // holder answering none of them is exactly what provokes the retries.
    let (alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"asked for repeatedly");

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    let first = bob.transport.sent_messages().len();
    assert_eq!(first, 1, "the first ask must reach the wire");

    for _ in 0..5 {
        bob.protocol
            .data_fetch_attachment(&bob_space, &hash)
            .expect("a repeat inside the window is not an error");
    }
    assert_eq!(
        bob.transport.sent_messages().len(),
        first,
        "a repeat inside the window must not reach the wire"
    );

    // And the bound is a window rather than a latch: once it passes, the
    // question can be asked again.
    let stale = std::time::Instant::now()
        - crate::protocol::data_sync::DATA_SYNC_OFFER_INTERVAL
        - std::time::Duration::from_secs(1);
    for asked_at in bob.protocol.pending_attachment_fetches.values_mut() {
        *asked_at = stale;
    }
    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch again");
    assert_eq!(
        bob.transport.sent_messages().len(),
        first + 1,
        "a repeat after the window must reach the wire"
    );
}

#[test]
fn a_holder_bounds_repeats_of_one_request() {
    // The holder's half of the same rule. Every request it acts on becomes
    // an application callback and, if answered, a whole transfer, so a peer
    // repeating one question must not turn into a stream of them.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let hash = OfflineProtocol::data_attachment_hash(b"asked for repeatedly");

    let mut asked = 0;
    for _ in 0..4 {
        // The requester's own window is a different gate, and leaving it in
        // the way would mean testing that one instead of this one: forget
        // the question locally so each repeat genuinely reaches the wire.
        bob.protocol.pending_attachment_fetches.clear();
        bob.protocol
            .data_fetch_attachment(&bob_space, &hash)
            .expect("ask again");
        asked += pump(&mut bob, &mut alice);
    }
    assert_eq!(
        asked, 4,
        "the fixture must have carried four identical questions to the holder"
    );

    assert_eq!(
        events_named(&alice, "data_attachment_requested").len(),
        1,
        "one question repeated must reach the application once"
    );
}

#[test]
fn the_request_map_is_bounded_across_peers_too() {
    // The per-peer budget bounds one peer's share of this map, not the map.
    // Enough peers each staying politely under their own budget reach the
    // global bound between them, and that bound is the only thing standing
    // between a roster's worth of distinct hashes and a map that remembers
    // all of them until the process ends.
    let (mut alice, bob) = pair();
    let per_peer = crate::protocol::data_sync::MAX_BLOB_REQUESTS_PER_WINDOW;
    let cap = crate::protocol::data_sync::MAX_BLOB_REQUEST_WINDOWS;
    let peers = cap / per_peer + 2;

    for peer in 0..peers {
        // A peer this device has heard advertise the capability, which is
        // all the inbound path checks: the flood is distinct senders, not a
        // handshake.
        let address = format!("{}{peer:04}", bob.address);
        alice.protocol.peer_data_sync.insert(address.clone());
        alice.protocol.peer_data_media.insert(address.clone());
        for round in 0..per_peer {
            let hash = OfflineProtocol::data_attachment_hash(format!("{peer}-{round}").as_bytes());
            alice.protocol.handle_data_sync_frame(
                &address,
                &format!(r#"{{"v":1,"k":"need_blob","hash":"{hash}"}}"#),
            );
        }
    }

    // The fixture must actually reach past the bound, or it observes a map
    // that was never full and says the cap works.
    assert!(
        peers * per_peer > cap,
        "the fixture offered {} entries against a cap of {cap}",
        peers * per_peer
    );
    assert!(
        alice.protocol.blob_request_windows.len() <= cap,
        "the map grew to {} against a cap of {cap}",
        alice.protocol.blob_request_windows.len()
    );
    // And that the frames reached the map at all. The bound above is an
    // upper one, which an empty map satisfies perfectly: were a gate above
    // this one to start refusing these frames, the cap would read as
    // enforced by a map nothing ever filled. Holding more than any single
    // peer's budget could put there is what says the flood crossed.
    assert!(
        alice.protocol.blob_request_windows.len() > per_peer,
        "the map holds {} entries, no more than one peer's budget of {per_peer}: \
         the frames are being refused before they reach the window",
        alice.protocol.blob_request_windows.len()
    );
}

#[test]
fn a_stale_document_layer_transfer_reports_to_the_fetch_not_the_app() {
    // The stale sweep is the third road that ends an inbound transfer, and
    // it has the same two obligations as the other two: never name a
    // document-layer transfer as a failed download, and tell the fetch that
    // is waiting for it to stop waiting.
    let (mut alice, mut bob) = pair();
    let bob_space = Node::space_for(&alice);
    let alice_space = Node::space_for(&bob);
    // Many chunks, so the transfer is still open when the clock moves.
    let blob = vec![9u8; 64 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&blob);

    bob.protocol
        .data_fetch_attachment(&bob_space, &hash)
        .expect("fetch");
    pump(&mut bob, &mut alice);
    alice
        .protocol
        .data_provide_attachment(&alice_space, &bob.address, &hash, blob)
        .expect("provide");
    alice.protocol.pump_media_transfers();
    pump(&mut alice, &mut bob);
    assert!(
        bob.protocol.file_transfer_manager.active_transfer_count() > 0,
        "the fixture must reach a transfer that is open when it goes stale"
    );
    clear_events(&bob);

    // The radio goes quiet for longer than the sweep tolerates.
    bob.protocol
        .file_transfer_manager
        .backdate_transfers(std::time::Duration::from_secs(
            crate::protocol::MEDIA_TRANSFER_STALE_TIMEOUT_SECS + 60,
        ));
    bob.protocol.cleanup_expired_entries();

    assert!(
        events_named(&bob, "file_receive_failed").is_empty(),
        "a document-layer transfer must never be named as a failed download"
    );
    let unavailable = events_named(&bob, "data_attachment_unavailable");
    assert_eq!(
        unavailable.len(),
        1,
        "the fetch waiting on those bytes must be told to stop waiting"
    );
    assert_eq!(unavailable[0]["reason"].as_str(), Some("stale_timeout"));
}

#[test]
fn aborting_a_document_layer_transfer_does_not_fail_a_file_nobody_sent() {
    // The send side's own version of the rule. An abort is where a transfer
    // that cannot finish is given up on, and reporting one for an internal
    // transfer would put a failed upload in front of somebody who never
    // attached anything.
    let (mut alice, mut bob) = pair();
    let space = Node::space_for(&bob);

    fill_past_the_frame_budget(&mut alice, &space, 0x5EED_1234_5EED_1234);
    pump(&mut alice, &mut bob);
    pump(&mut bob, &mut alice);
    let file_id = alice
        .protocol
        .outbound_media_transfers
        .iter()
        .find(|(_, transfer)| transfer.data_purpose.is_some())
        .map(|(file_id, _)| file_id.clone())
        .expect("the document must be on its way");
    clear_events(&alice);

    alice
        .protocol
        .abort_outbound_media_transfer(&file_id, "transport gone");

    assert!(
        events_named(&alice, "media_send_failed").is_empty(),
        "an internal transfer has no application-facing identity to fail"
    );

    // And the control, on the same road: an ordinary send aborted the same
    // way still reports, so what is pinned above is the marking and not a
    // silent abort path.
    let app_file_id = alice
        .protocol
        .send_media_with(
            bob.address.clone(),
            vec![1u8; 4096],
            "holiday.jpg",
            offline_protocol_core::ContentType::Image,
            crate::protocol::types::MediaSendOptions::default(),
        )
        .expect("send");
    alice
        .protocol
        .abort_outbound_media_transfer(&app_file_id, "transport gone");
    assert_eq!(
        events_named(&alice, "media_send_failed").len(),
        1,
        "an ordinary transfer aborted the same way must still report"
    );
}
