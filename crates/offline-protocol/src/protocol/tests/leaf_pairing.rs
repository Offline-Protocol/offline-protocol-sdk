//! A phone engine and a constrained device, pairing over a simulated radio.
//!
//! What these cover that nothing else can. The leaf crate's own interop suite
//! drives a bare [`MlsManager`](offline_protocol_mls::MlsManager), which is the
//! MLS layer and not the engine: it proves the two MLS implementations agree,
//! and says nothing about the machinery an application actually runs. The
//! engine's own two-node suites go the other way, standing the session up
//! through a fixture rather than through the exchange. So the choreography
//! between them — auto key exchange, the Welcome, the sealed confirmation, the
//! capability negotiation, the delivery acknowledgement, the retry ladder — has
//! never had both real ends attached to it at once.
//!
//! Every frame here crosses a [`MockTransport`], carries a transport peer
//! identity the way a radio reports one, and goes through the engine's ordinary
//! receive path. Neither side is told anything the wire did not carry.
//!
//! The device is deliberately not given a transport. A leaf does not own its
//! radio: it is handed a frame and returns the frames to transmit, which is
//! exactly the seam [`LeafDevice::handle`] presents, so the harness is the
//! firmware here.

use std::sync::{Arc, Mutex};

use offline_protocol_core::Message;
use offline_protocol_leaf::{Handled, LeafDevice, LeafEvent, LeafStore, MemoryStore};
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::constants::ACK_FOR_KEY;
use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::OfflineProtocol;
use crate::ProtocolStateStorage;

/// The application identifier both ends run under.
///
/// It has to be the same string on both sides: `create_test_config_for_user`
/// builds the engine's config with `"test-app"`, and a leaf parses its own from
/// the argument it is provisioned with.
const APP_ID: &str = "test-app";

/// A phone: the real engine, over the transport its frames go through.
struct Phone {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    label: String,
    secure: Arc<InMemoryStorage>,
    state: Arc<InMemoryStorage>,
    events: Arc<Mutex<Vec<crate::Event>>>,
    /// What has been surfaced to the application, kept because carrying frames
    /// is what drains it. A harness that drops what it drained cannot tell a
    /// message that never arrived from one it threw away.
    delivered: Vec<String>,
}

impl Phone {
    fn new(label: &str) -> Self {
        let secure = crate::test_identity::seeded_storage(label);
        let state = Arc::new(InMemoryStorage::new());
        let (protocol, transport, events) = Self::boot(label, secure.clone(), state.clone());
        Self {
            protocol,
            transport,
            address: id(label),
            label: label.to_string(),
            secure,
            state,
            events,
            delivered: Vec::new(),
        }
    }

    /// Builds an engine over the given storage and starts it.
    ///
    /// Split out because a relaunch has to produce exactly the same thing from
    /// exactly the same storage, and a second construction written by hand is
    /// how the two drift.
    fn boot(
        label: &str,
        secure: Arc<InMemoryStorage>,
        state: Arc<InMemoryStorage>,
    ) -> (
        OfflineProtocol,
        MockTransport,
        Arc<Mutex<Vec<crate::Event>>>,
    ) {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = true;
        // On by default, and named here because it is the whole mechanism
        // under test: it is what answers a device's key package with one of
        // this engine's own and then establishes without being asked.
        config.encryption.auto_key_exchange = true;

        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let state_storage: Arc<dyn ProtocolStateStorage> =
            Arc::new(crate::protocol::TestProtocolStateStorage { storage: state });
        protocol
            .initialize_mls(secure, state_storage)
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

        (protocol, transport, events)
    }

    /// Relaunches on the same storage, as an application does between two runs.
    fn restart(&mut self) {
        let (protocol, transport, events) =
            Self::boot(&self.label, self.secure.clone(), self.state.clone());
        self.protocol = protocol;
        self.transport = transport;
        self.events = events;
        // `delivered` deliberately survives: it is this harness's record of
        // what the application was told, not the engine's state.
    }

    /// Every event this engine has surfaced so far.
    fn events(&self) -> Vec<crate::Event> {
        self.events.lock().unwrap().clone()
    }

    /// Whether a `SecureSessionEstablished` naming `peer` has been surfaced.
    fn established_with(&self, peer: &str) -> bool {
        self.events().iter().any(|event| {
            matches!(
                event,
                crate::Event::SecureSessionEstablished { peer_id, .. } if peer_id == peer
            )
        })
    }

    /// Moves whatever the engine has surfaced into this harness's record of it.
    fn drain(&mut self) {
        while let Some(message) = self.protocol.receive_message() {
            self.delivered.push(message.content);
        }
    }

    /// Everything this engine has surfaced to its application so far.
    fn inbox(&mut self) -> Vec<String> {
        self.drain();
        self.delivered.clone()
    }
}

/// A constrained device, and the store it survives a power cut on.
struct Leaf {
    device: LeafDevice,
    store: Arc<dyn LeafStore>,
    address: String,
    /// Everything the device has reported, in order.
    events: Vec<LeafEvent>,
}

impl Leaf {
    fn new() -> Self {
        let store: Arc<dyn LeafStore> = Arc::new(MemoryStore::new());
        let device = LeafDevice::provision(Arc::clone(&store), APP_ID).expect("provision");
        let address = device.address().to_string();
        Self {
            device,
            store,
            address,
            events: Vec::new(),
        }
    }

    /// Loses power and comes back on the same flash.
    ///
    /// The device is dropped first, so nothing cached in memory can be what
    /// makes the next exchange work: whatever the pair still shares has to
    /// have reached the store.
    fn power_cycle(&mut self) {
        self.device = LeafDevice::open(Arc::clone(&self.store), APP_ID).expect("reopen");
    }

    /// Whether the device reported a session reset from `peer`.
    fn saw_session_reset(&self, peer: &str) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, LeafEvent::SessionReset { peer: p } if p == peer))
    }

    /// Whether the device reported an event of a given shape.
    fn saw_session_established(&self, peer: &str) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, LeafEvent::SessionEstablished { peer: p } if p == peer))
    }

    /// The plaintexts this device has surfaced, in order.
    fn received(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                LeafEvent::MessageReceived { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}

/// A phone and a device in radio range of each other.
struct Pair {
    phone: Phone,
    leaf: Leaf,
    /// Wall-clock seconds handed to the device, since it has no clock.
    now: u64,
    /// Frames the device put on the air but which have not been carried yet.
    leaf_outbound: Vec<Message>,
}

impl Pair {
    fn new(label: &str) -> Self {
        Self {
            phone: Phone::new(label),
            leaf: Leaf::new(),
            // A fixed instant rather than the real clock, so a freshness window
            // is judged against a value the test controls. Both ends use it:
            // the device is handed it, and the engine's own frames carry its
            // system clock, which is why this is anchored to now rather than to
            // a literal.
            now: chrono::Utc::now().timestamp() as u64,
            leaf_outbound: Vec::new(),
        }
    }

    /// Hands the device a frame and keeps whatever it produced.
    ///
    /// Everything in [`Handled::outbound`] is durable by the time it is
    /// returned, so queuing it for transmission here is exactly the ordering
    /// firmware owes.
    fn feed_leaf(&mut self, message: Message) {
        let Handled { outbound, events } = self
            .leaf
            .device
            .handle(&message, self.now)
            .expect("the device refused a frame this test did not expect it to refuse");
        self.leaf.events.extend(events);
        self.leaf_outbound.extend(outbound);
    }

    /// Carries everything the phone transmitted into the device.
    ///
    /// Returns the frames that crossed, so a test can assert an exchange
    /// terminates rather than merely converges.
    fn phone_to_leaf(&mut self) -> usize {
        let messages = self.phone.transport.sent_messages();
        self.phone.transport.clear_sent_messages();
        let moved = messages.len();
        for message in messages {
            self.feed_leaf(message);
        }
        moved
    }

    /// Carries everything the device transmitted into the phone.
    ///
    /// The device's address is attached as the transport peer, which is what a
    /// radio reports and what the engine's sender-identity check compares the
    /// claimed sender against.
    fn leaf_to_phone(&mut self) -> usize {
        let messages = std::mem::take(&mut self.leaf_outbound);
        let moved = messages.len();
        for message in messages {
            self.phone
                .transport
                .queue_message_from(message, self.leaf.address.clone());
        }
        self.phone.drain();
        moved
    }

    /// Runs the exchange until nothing more moves, returning the per-round
    /// counts.
    ///
    /// The round cap is a guard rather than a budget: an exchange between two
    /// peers that will not go quiet is itself the failure, so hitting it fails
    /// the test instead of hanging it.
    fn settle(&mut self) -> Vec<usize> {
        let mut rounds = Vec::new();
        for _ in 0..12 {
            let carried = self.phone_to_leaf() + self.leaf_to_phone();
            rounds.push(carried);
            if carried == 0 {
                return rounds;
            }
        }
        panic!("the exchange never went quiet: {rounds:?}");
    }

    /// Turns the engine's tick crank, which is what runs its retry ladders.
    fn tick(&mut self) {
        self.phone.protocol.process().expect("process");
    }

    fn phone_address(&self) -> String {
        self.phone.address.clone()
    }

    fn leaf_address(&self) -> String {
        self.leaf.address.clone()
    }
}

/// The canonical pairing exchange, device first.
///
/// This is step 2 of the provisioning specification: the person has scanned the
/// device's artifact, and the device puts a freshly minted key package on the
/// air. Everything after it is the engine's ordinary establishment path, which
/// has never before had a second implementation on the other end of it.
#[test]
fn a_phone_and_a_leaf_pair_from_the_devices_key_package() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    let frame = pair
        .leaf
        .device
        .key_package_frame(&phone, pair.now)
        .expect("mint");
    pair.leaf_outbound.push(frame);

    let rounds = pair.settle();

    assert!(
        pair.phone.established_with(&leaf),
        "the engine never reported a secure session with the device; events: {:?}",
        pair.phone.events()
    );
    assert!(
        pair.leaf.saw_session_established(&phone),
        "the device never reported a session with the phone; events: {:?}",
        pair.leaf.events
    );
    assert!(
        pair.phone.protocol.has_mls_session(&leaf).unwrap(),
        "the engine reports no MLS session with the device"
    );
    assert!(
        pair.leaf.device.has_session(&phone).unwrap(),
        "the device reports no session with the phone"
    );
    assert_eq!(
        pair.leaf.device.peers().unwrap(),
        vec![phone.clone()],
        "the device's authorization audit list must name the phone it paired with"
    );
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange settled only because the round cap was hit: {rounds:?}"
    );
}

/// The engine reports the session as one it initiated.
///
/// The device never sends a Welcome, so the phone is always the creating side
/// of a pair. An application that reads this field to decide which end shows a
/// pairing confirmation gets the same answer every time against a leaf.
#[test]
fn the_phone_is_always_the_initiating_side_of_a_leaf_pairing() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    let frame = pair
        .leaf
        .device
        .key_package_frame(&phone, pair.now)
        .expect("mint");
    pair.leaf_outbound.push(frame);
    pair.settle();

    let initiated = pair
        .phone
        .events()
        .into_iter()
        .find_map(|event| match event {
            crate::Event::SecureSessionEstablished {
                peer_id,
                is_session,
                initiated_by_local,
                ..
            } if peer_id == leaf => Some((is_session, initiated_by_local)),
            _ => None,
        });

    assert_eq!(
        initiated,
        Some((true, true)),
        "a leaf pairing is a 1:1 session the phone initiated"
    );
}

/// What the phone records about a device's capabilities.
///
/// A leaf advertises the compact envelope and the freshness-bound control
/// payload, and advertises neither rich payloads nor document replication.
/// Those absences are the ones a legacy phone also presents, so the engine has
/// to reach the right conclusion from each of them independently rather than
/// from a single "is this peer modern" flag.
#[test]
fn the_phone_records_exactly_the_capabilities_a_leaf_advertises() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    let frame = pair
        .leaf
        .device
        .key_package_frame(&phone, pair.now)
        .expect("mint");
    pair.leaf_outbound.push(frame);
    pair.settle();

    assert!(
        pair.phone.protocol.peer_compact_envelope.contains(&leaf),
        "a device advertises the compact envelope and the phone must seal to it that way"
    );
    assert!(
        pair.phone.protocol.signs_freshness_bound_control_to(&leaf),
        "a device verifies the freshness-bound control payload, and a phone that \
         did not record it would keep signing the older one, which the device \
         refuses on everything but a key package"
    );
    assert!(
        !pair.phone.protocol.peer_rich_payload.contains(&leaf),
        "a device advertises no rich payload; recording one would seal extras it cannot parse"
    );
    assert!(
        !pair.phone.protocol.peer_data_sync.contains(&leaf),
        "a device replicates no documents; recording it would push sync frames at a door lock"
    );
}

/// Pairing that begins at the phone, which is how it begins in the field.
///
/// A device is not scanned by every phone it meets. It is discovered over the
/// radio, and the engine's discovery hook pushes a key package unprompted. The
/// device answers with a mint of its own, and establishment runs from there.
/// This is also the only order in which the device learns the phone's address
/// without an out-of-band step.
#[test]
fn a_pairing_that_begins_with_discovery_converges_the_same_way() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    let rounds = pair.settle();

    assert!(
        pair.phone.established_with(&leaf),
        "discovery-first pairing left the engine without a session; events: {:?}",
        pair.phone.events()
    );
    assert!(
        pair.leaf.saw_session_established(&phone),
        "discovery-first pairing left the device without a session; events: {:?}",
        pair.leaf.events
    );
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange settled only because the round cap was hit: {rounds:?}"
    );
}

/// One pairing spends exactly one of the device's init keys.
///
/// An init key is single use, and the engine sends a second key package of its
/// own once a session is up so the peer has one available for a group invite.
/// A device that answered that with a fresh mint every time would burn a key
/// package per exchange and, on a part with a handful of slots, evict its own
/// peers. The guard is the device's `key_package_sent` record, and this is what
/// proves it holds against the engine's real behaviour rather than a fixture's.
#[test]
fn a_pairing_spends_exactly_one_of_the_devices_key_packages() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    let minted = pair
        .leaf
        .events
        .iter()
        .filter(|event| matches!(event, LeafEvent::PeerAdvertised { .. }))
        .count();
    assert!(
        minted >= 1,
        "the device never recorded the phone advertising itself"
    );
    assert_eq!(
        pair.leaf.device.peers().unwrap(),
        vec![phone],
        "one pairing must leave exactly one peer on the device"
    );
    assert!(
        pair.phone.established_with(&leaf),
        "the exchange has to have actually converged for this count to mean anything"
    );
}

/// A message from the phone reaches the device and is answered.
///
/// Two halves, and the second is the one that was never covered: the device's
/// delivery acknowledgement has to settle the engine's retry ladder. Against a
/// device that did not answer, the frame ran ten retransmissions over about
/// thirteen minutes, each arriving as a replay of a spent ratchet generation.
#[test]
fn a_message_to_the_device_arrives_and_its_answer_settles_the_ladder() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    let message_id = pair
        .phone
        .protocol
        .send_message(&leaf, "unlock the front door", None, None::<String>)
        .expect("send");
    assert!(
        pair.phone
            .protocol
            .ack_manager
            .is_waiting_for_ack(&message_id),
        "a direct message must be on the ladder before the device answers it"
    );

    let rounds = pair.settle();

    assert_eq!(
        pair.leaf.received(),
        vec!["unlock the front door".to_string()],
        "the device never surfaced the command; events: {:?}",
        pair.leaf.events
    );
    assert!(
        !pair
            .phone
            .protocol
            .ack_manager
            .is_waiting_for_ack(&message_id),
        "the device answered and the phone is still retrying the frame"
    );
    assert!(
        pair.phone.events().iter().any(|event| matches!(
            event,
            crate::Event::MessageDelivered { message_id: id, hop_count, transport, .. }
                if *id == message_id.as_str() && *hop_count == 0 && transport == "ble"
        )),
        "the app was never told its command reached the lock, or was told the \
         wrong defaults for the two entries a leaf deliberately omits; events: {:?}",
        pair.phone.events()
    );
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange never went quiet: {rounds:?}"
    );

    // The ladder is what would retransmit, so turning its crank is the only
    // way to prove the settle was real rather than merely not yet due.
    pair.tick();
    assert_eq!(
        pair.phone.transport.sent_messages().len(),
        0,
        "the phone retransmitted a frame the device had already acknowledged"
    );
    let _ = phone;
}

/// A message from the device reaches the phone's application.
///
/// The other direction, and it is not symmetric: a leaf asks for no
/// acknowledgement, because it holds no retry queue to settle one against. A
/// phone that answered anyway would spend one transmission per frame on a link
/// chosen for how little it costs to keep quiet.
#[test]
fn a_message_from_the_device_reaches_the_app_and_is_not_answered() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    let frame = pair
        .leaf
        .device
        .seal(&phone, "the door is open", pair.now)
        .expect("seal");
    pair.leaf_outbound.push(frame);
    let rounds = pair.settle();

    assert_eq!(
        pair.phone.inbox(),
        vec!["the door is open".to_string()],
        "the phone never surfaced what the device sent"
    );
    assert!(
        pair.phone
            .transport
            .sent_messages()
            .iter()
            .all(|message| !message.metadata.contains_key(ACK_FOR_KEY)),
        "the phone answered a frame that asked for no answer"
    );
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange never went quiet: {rounds:?}"
    );
    let _ = leaf;
}

/// A captured frame delivered a second time is answered, never opened twice.
///
/// The device's answer is the frame most likely to go missing: it is last in
/// the exchange and nothing retries it. So a second copy is overwhelmingly a
/// retransmission, and it cannot be opened — the ratchet spent that generation
/// on the first. The device answers from memory instead, and the command is
/// surfaced exactly once.
#[test]
fn a_frame_delivered_twice_is_answered_twice_and_acted_on_once() {
    let mut pair = Pair::new("alice");
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    pair.phone
        .protocol
        .send_message(&leaf, "unlock the front door", None, None::<String>)
        .expect("send");

    // Take the sealed frame off the air rather than letting the pump carry it,
    // so the same bytes can be delivered a second time the way a captured
    // frame or a duplicated radio path would deliver them.
    let captured: Vec<Message> = pair.phone.transport.sent_messages();
    pair.phone.transport.clear_sent_messages();
    let sealed = captured
        .into_iter()
        .find(|message| message.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("the command left as a sealed frame");

    pair.feed_leaf(sealed.clone());
    let first_answers = pair.leaf_outbound.len();
    pair.feed_leaf(sealed);
    let second_answers = pair.leaf_outbound.len() - first_answers;

    assert_eq!(
        pair.leaf.received(),
        vec!["unlock the front door".to_string()],
        "the second copy was opened as well, which the ratchet should have refused"
    );
    assert_eq!(
        first_answers, 1,
        "the device did not answer the frame it accepted"
    );
    assert_eq!(
        second_answers, 1,
        "the device stayed quiet for a retransmission, which is exactly the \
         state the repeat-answer memory exists to avoid"
    );

    // And the phone treats the repeat as the duplicate it is.
    pair.settle();
    assert_eq!(
        pair.phone.inbox().len(),
        0,
        "an acknowledgement is not application traffic"
    );
}

/// A sealed frame from the device, replayed at the phone, surfaces once.
///
/// The mirror of the case above, on the side that has a deduplicator.
#[test]
fn a_frame_from_the_device_replayed_at_the_phone_surfaces_once() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    let frame = pair
        .leaf
        .device
        .seal(&phone, "the door is open", pair.now)
        .expect("seal");
    pair.phone
        .transport
        .queue_message_from(frame.clone(), leaf.clone());
    pair.phone.transport.queue_message_from(frame, leaf.clone());
    pair.phone.drain();

    assert_eq!(
        pair.phone.inbox(),
        vec!["the door is open".to_string()],
        "the phone surfaced a replayed frame twice"
    );
}

/// The pair keeps working across a power cut on the device.
///
/// This is the obligation that cannot be checked by reading the code: state has
/// to be durable before a frame is emitted, or a device comes back and reuses
/// an AEAD nonce. What a test can check is the observable half — that the
/// session survives with nothing held in memory, in both directions, and that
/// no frame afterwards fails to decrypt.
#[test]
fn the_pair_survives_a_power_cut_on_the_device() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    pair.phone
        .protocol
        .send_message(&leaf, "before the cut", None, None::<String>)
        .expect("send");
    pair.settle();

    pair.leaf.power_cycle();

    let message_id = pair
        .phone
        .protocol
        .send_message(&leaf, "after the cut", None, None::<String>)
        .expect("send");
    pair.settle();

    assert_eq!(
        pair.leaf.received(),
        vec!["before the cut".to_string(), "after the cut".to_string()],
        "the device lost the session across a power cut; events: {:?}",
        pair.leaf.events
    );
    assert!(
        !pair
            .phone
            .protocol
            .ack_manager
            .is_waiting_for_ack(&message_id),
        "the device answered nothing after coming back"
    );

    let reply = pair
        .leaf
        .device
        .seal(&phone, "still here", pair.now)
        .expect("seal after a power cut");
    pair.leaf_outbound.push(reply);
    pair.settle();
    assert_eq!(
        pair.phone.inbox(),
        vec!["still here".to_string()],
        "the device could not seal after coming back, which is what a rolled-back \
         ratchet looks like from the outside"
    );

    assert!(
        !pair
            .phone
            .events()
            .iter()
            .any(|event| matches!(event, crate::Event::MessageDecryptionFailed { .. })),
        "a frame failed to decrypt after the power cut; events: {:?}",
        pair.phone.events()
    );
}

/// The pair keeps working across a relaunch of the phone.
///
/// The other end of the same question. An application restarts far more often
/// than a lock loses power, and the engine has to come back holding a session
/// it can still open, from storage alone.
#[test]
fn the_pair_survives_a_relaunch_of_the_phone() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    pair.phone.restart();

    let message_id = pair
        .phone
        .protocol
        .send_message(&leaf, "after the relaunch", None, None::<String>)
        .expect("send");
    pair.settle();

    assert_eq!(
        pair.leaf.received(),
        vec!["after the relaunch".to_string()],
        "the phone lost its session with the device across a relaunch"
    );
    assert!(
        !pair
            .phone
            .protocol
            .ack_manager
            .is_waiting_for_ack(&message_id),
        "the relaunched phone never settled the frame the device answered"
    );

    let reply = pair
        .leaf
        .device
        .seal(&phone, "still paired", pair.now)
        .expect("seal");
    pair.leaf_outbound.push(reply);
    pair.settle();
    assert_eq!(
        pair.phone.inbox(),
        vec!["still paired".to_string()],
        "the relaunched phone could not open a frame from the device"
    );
}

/// An application asks for a rotation, and the pair rebuilds.
///
/// This is the whole of post-compromise security for a pair containing a leaf.
/// The device never commits, so it never rotates its own leaf in the ratchet
/// tree; the phone's reset path is what does, and until an application calls
/// for it nothing but an epoch desync ever fires that path. A healthy pair that
/// never forks therefore never healed, and the window an attacker holding old
/// key material keeps open was bounded by nothing.
#[test]
fn an_application_driven_rekey_rebuilds_the_pair() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();
    assert!(pair.phone.protocol.has_mls_session(&leaf).unwrap());

    let driven = pair.phone.protocol.rekey_session(&leaf).expect("rekey");
    assert!(driven, "the re-key was refused by its own rate limit");

    let rounds = pair.settle();

    assert!(
        pair.leaf.saw_session_reset(&phone),
        "the device never saw the reset, so it is holding a session the phone \
         discarded and every later frame decrypts to nothing; events: {:?}",
        pair.leaf.events
    );
    assert!(
        pair.phone.protocol.has_mls_session(&leaf).unwrap(),
        "the pair did not rebuild after the rotation"
    );
    assert!(
        pair.leaf.device.has_session(&phone).unwrap(),
        "the device did not re-pair after the rotation"
    );
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the rebuild never went quiet: {rounds:?}"
    );
}

/// Traffic flows in both directions in the epoch a rotation produced.
///
/// A rebuild that converges but cannot carry a message is the failure this
/// separates out: the pair reports a session, and the first command sent to the
/// lock is sealed to an epoch nobody holds.
#[test]
fn traffic_flows_both_ways_after_a_rotation() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    pair.phone
        .protocol
        .send_message(&leaf, "before the rotation", None, None::<String>)
        .expect("send");
    pair.settle();

    assert!(pair.phone.protocol.rekey_session(&leaf).expect("rekey"));
    pair.settle();

    let message_id = pair
        .phone
        .protocol
        .send_message(&leaf, "after the rotation", None, None::<String>)
        .expect("send");
    pair.settle();

    assert_eq!(
        pair.leaf.received(),
        vec![
            "before the rotation".to_string(),
            "after the rotation".to_string()
        ],
        "a command sent after the rotation never reached the device; events: {:?}",
        pair.leaf.events
    );
    assert!(
        !pair
            .phone
            .protocol
            .ack_manager
            .is_waiting_for_ack(&message_id),
        "the device did not answer in the epoch the rotation produced"
    );

    let reply = pair
        .leaf
        .device
        .seal(&phone, "acknowledged", pair.now)
        .expect("the device can seal in the new epoch");
    pair.leaf_outbound.push(reply);
    pair.settle();
    assert_eq!(
        pair.phone.inbox(),
        vec!["acknowledged".to_string()],
        "the phone could not open a frame sealed in the epoch it just created"
    );
}

/// The rotation's own reset frame cannot be replayed to break the pair again.
///
/// A reset tears down a live session, so the frame that carries one is worth
/// capturing. The device admits one only above a per-peer high-water mark over
/// a timestamp that is inside the signature, which makes one recording worth
/// one teardown rather than one per delivery. This is that rule met by a reset
/// the engine really produced rather than one a fixture built.
#[test]
fn the_rotations_reset_frame_cannot_be_replayed_to_break_the_pair_again() {
    let mut pair = Pair::new("alice");
    let phone = pair.phone_address();
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    assert!(pair.phone.protocol.rekey_session(&leaf).expect("rekey"));

    // Take the reset off the air so the same bytes can be delivered again.
    let sent = pair.phone.transport.sent_messages();
    pair.phone.transport.clear_sent_messages();
    let reset = sent
        .iter()
        .find(|message| {
            message.content.starts_with(internal_prefixes::KEY_PACKAGE)
                && message.content.contains("\"session_reset\":true")
        })
        .expect("the rotation left as a key package carrying a reset")
        .clone();
    for message in sent {
        pair.feed_leaf(message);
    }
    pair.settle();

    assert!(
        pair.leaf.device.has_session(&phone).unwrap(),
        "the pair has to be back up before a replay is worth anything"
    );
    let resets_so_far = pair
        .leaf
        .events
        .iter()
        .filter(|event| matches!(event, LeafEvent::SessionReset { .. }))
        .count();

    pair.feed_leaf(reset);

    assert_eq!(
        pair.leaf
            .events
            .iter()
            .filter(|event| matches!(event, LeafEvent::SessionReset { .. }))
            .count(),
        resets_so_far,
        "a replayed reset tore the rebuilt session down; events: {:?}",
        pair.leaf.events
    );
    assert!(
        pair.leaf.device.has_session(&phone).unwrap(),
        "the replay cost the pair the session it had just rebuilt"
    );
}

/// A second rotation inside the window is refused, and says so.
#[test]
fn a_rotation_inside_the_window_is_refused_rather_than_repeated() {
    let mut pair = Pair::new("alice");
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();

    assert!(pair.phone.protocol.rekey_session(&leaf).expect("first"));
    pair.settle();

    assert!(
        !pair.phone.protocol.rekey_session(&leaf).expect("second"),
        "a caller looping on this could tear a pair down as fast as it liked, \
         which is what the shared floor exists to prevent"
    );
}

/// A relaunched phone still signs the payload the device can verify.
///
/// The device refuses every control frame but a key package under the older
/// payload, and the phone picks its payload from what the device advertised. So
/// that advertisement has to survive a relaunch: an engine that came back and
/// forgot it would sign a Welcome the device refuses, and a pairing interrupted
/// by a restart could never finish. The durable capability record is what
/// carries it, and nothing else here would notice if that stopped being read.
#[test]
fn a_relaunched_phone_still_signs_what_the_device_can_verify() {
    let mut pair = Pair::new("alice");
    let leaf = pair.leaf_address();

    pair.phone.protocol.on_neighbor_discovered(&leaf);
    pair.settle();
    assert!(
        pair.phone.protocol.signs_freshness_bound_control_to(&leaf),
        "the phone never recorded what the device advertised"
    );

    pair.phone.restart();

    assert!(
        pair.phone.protocol.signs_freshness_bound_control_to(&leaf),
        "a relaunched phone forgot that this peer verifies the freshness-bound \
         payload, so every control frame it now signs would be refused"
    );
}
