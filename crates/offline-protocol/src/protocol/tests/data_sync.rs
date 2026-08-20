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

    // Varied filler: repeated text compresses inside the engine's encoding,
    // and a document that compresses back under the budget never reaches the
    // rung this test is about.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for round in 0..24 {
        let filler: String = (0..4096)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from(b'0' + (seed % 64) as u8)
            })
            .collect();
        alice
            .protocol
            .data_map_set(
                &space,
                "notes",
                "m",
                &format!("k{round}"),
                DataValue::text(filler),
            )
            .expect("set");
    }
    alice.protocol.data_flush(&space, "notes").expect("flush");
    assert!(
        alice.protocol.data_doc_size(&space, "notes").expect("size")
            > crate::protocol::data_sync::MAX_SYNC_BLOB_BYTES as u64,
        "the fixture must build a document no frame can carry"
    );

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

    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for round in 0..24 {
        let filler: String = (0..4096)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from(b'0' + (seed % 64) as u8)
            })
            .collect();
        alice
            .protocol
            .data_map_set(
                &space,
                "notes",
                "m",
                &format!("k{round}"),
                DataValue::text(filler),
            )
            .expect("set");
    }
    alice.protocol.data_flush(&space, "notes").expect("flush");
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
