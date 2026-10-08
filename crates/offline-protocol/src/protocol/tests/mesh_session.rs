//! A session started across the mesh, between two devices that have never
//! been in range of each other.
//!
//! alice and carol each hear only bob. Every frame goes through MLS, the
//! transport, the forwarding governor and the prefix dispatch, with
//! encryption required, so a message cannot leave alice until a session with
//! carol exists. What is pinned is the one property the demo of this SDK
//! leans on: the session forms through bob, the message arrives, and its
//! acknowledgement comes back, with nothing readable on bob's side.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use offline_protocol_core::Message;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::reachability::{Claim, FactSource};
use crate::protocol::tests::create_test_config_for_user;
use crate::protocol::types::WelcomeDeliveryState;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::test_identity::id;
use crate::{Event, ProtocolConfig};

struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    /// An internet carrier that accepts every frame and delivers none: a
    /// relay the peer at the other end of the line is not on.
    internet: Option<MockTransport>,
    address: String,
    events: Arc<Mutex<Vec<Event>>>,
    inbox: Vec<String>,
}

fn strict_config(label: &str) -> ProtocolConfig {
    let mut config = create_test_config_for_user(label);
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.auto_key_exchange = true;
    config.encryption.store_pending = true;
    // No hold before forwarding: the test drives time by stepping.
    config.mesh_relay.jitter_min = Duration::from_millis(0);
    config.mesh_relay.jitter_max = Duration::from_millis(0);
    // The rate budgets refill with wall-clock time, and a round here takes
    // microseconds, so the handshake on three devices would spend the burst
    // in the first rounds and wait for a refill the test never gives it. The
    // budgets' own behaviour is the governor's unit tests' to pin.
    config.mesh_relay.burst = 1_000.0;
    config.mesh_relay.peer_burst = 1_000.0;
    config
}

impl Node {
    fn new(label: &str, config: ProtocolConfig, internet: bool) -> Self {
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
        // A device can only put a frame on a link it actually has, which is
        // what keeps alice and carol apart.
        mock.set_reject_unknown_recipients(true);
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        let internet = internet.then(|| {
            let relay = MockTransport::new(TransportType::Internet);
            relay.start().expect("internet start");
            protocol
                .transport_manager_mut()
                .add_transport(TransportType::Internet, Box::new(relay.clone()));
            relay
        });
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));
        protocol.start().expect("start");
        Self {
            protocol,
            transport,
            internet,
            address: id(label),
            events,
            inbox: Vec::new(),
        }
    }

    fn delivered(&self, message_id: &str) -> Option<u8> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                Event::MessageDelivered {
                    message_id: id,
                    hop_count,
                    ..
                } if id == message_id => Some(*hop_count),
                _ => None,
            })
    }
}

/// alice and carol, each in range of bob only.
struct Line {
    nodes: Vec<Node>,
    /// Every frame that crossed a link, as `(from, to, frame)`.
    carried: Vec<(usize, usize, Message)>,
    /// A one-way link that is down: frames put on it are lost.
    cut: Option<(usize, usize)>,
}

const ALICE: usize = 0;
const BOB: usize = 1;
const CAROL: usize = 2;

impl Line {
    fn new(bob: ProtocolConfig) -> Self {
        Self::with_relay_on_the_ends(bob, false)
    }

    /// The line, with alice and carol also online on a relay that has told
    /// each of them it cannot reach the other.
    fn with_relay_on_the_ends(bob: ProtocolConfig, relay: bool) -> Self {
        let mut line = Self {
            nodes: vec![
                Node::new("alice", strict_config("alice"), relay),
                Node::new("bob", bob, false),
                Node::new("carol", strict_config("carol"), relay),
            ],
            carried: Vec::new(),
            cut: None,
        };
        if relay {
            for (from, to) in [(ALICE, CAROL), (CAROL, ALICE)] {
                let peer = line.nodes[to].address.clone();
                line.nodes[from].protocol.reachability.record(
                    &peer,
                    TransportType::Internet,
                    Claim::Unreachable,
                    FactSource::GatewayVerdict,
                    Instant::now(),
                );
            }
        }
        line.link(ALICE, BOB);
        line.link(BOB, CAROL);
        line
    }

    fn link(&mut self, a: usize, b: usize) {
        let (address_a, address_b) = (self.nodes[a].address.clone(), self.nodes[b].address.clone());
        self.nodes[a]
            .transport
            .add_connected_peer(address_b.clone(), -55);
        self.nodes[b]
            .transport
            .add_connected_peer(address_a.clone(), -55);
        self.nodes[a].protocol.on_neighbor_discovered(&address_b);
        self.nodes[b].protocol.on_neighbor_discovered(&address_a);
    }

    fn index_of(&self, address: &str) -> usize {
        self.nodes
            .iter()
            .position(|node| node.address == address)
            .expect("a frame went to a device outside the line")
    }

    /// One round: every device handles what it holds and runs its tick, then
    /// whatever it put on a link reaches the device at the other end.
    fn step(&mut self) -> usize {
        for node in &mut self.nodes {
            while let Some(message) = node.protocol.receive_message() {
                node.inbox.push(message.content);
            }
            // The reconciliation scan is throttled to once every two
            // seconds; the test steps far faster than that.
            node.protocol.last_reconciliation_at = None;
            node.protocol.process().expect("process");
        }
        let mut moved = 0;
        for from in 0..self.nodes.len() {
            let mut handed: Vec<(String, Message)> = self.nodes[from]
                .transport
                .sent_messages()
                .into_iter()
                .map(|m| (m.recipient.as_str().to_string(), m))
                .collect();
            handed.extend(self.nodes[from].transport.peer_sends());
            self.nodes[from].transport.clear_sent_messages();
            self.nodes[from].transport.clear_peer_sends();
            for (to, frame) in handed {
                let to = self.index_of(&to);
                assert_eq!(
                    from.abs_diff(to),
                    1,
                    "alice and carol share no link, so nothing passes between them directly"
                );
                if self.cut == Some((from, to)) {
                    continue;
                }
                self.carried.push((from, to, frame.clone()));
                let sender = self.nodes[from].address.clone();
                self.nodes[to].transport.queue_message_from(frame, sender);
                moved += 1;
            }
        }
        moved
    }

    fn run(&mut self, rounds: usize) {
        for _ in 0..rounds {
            self.step();
        }
    }
}

#[test]
fn a_session_forms_across_the_mesh_between_devices_that_never_met() {
    let mut line = Line::new(strict_config("bob"));
    let carol = line.nodes[CAROL].address.clone();

    let message_id = line.nodes[ALICE]
        .protocol
        .send_message(&carol, "meet at the north gate", None, None::<String>)
        .expect("queued until a session exists")
        .as_str();

    line.run(40);
    assert_eq!(
        line.nodes[CAROL]
            .inbox
            .iter()
            .filter(|content| content.as_str() == "meet at the north gate")
            .count(),
        1,
        "carol receives the message from a device she has never been in range of, once"
    );
    assert_eq!(
        line.nodes[ALICE].delivered(&message_id),
        Some(1),
        "and alice holds carol's acknowledgement, which crossed bob on its way back"
    );

    let alice = line.nodes[ALICE].address.clone();
    let through_bob: Vec<&Message> = line
        .carried
        .iter()
        .filter(|(from, to, frame)| {
            *from == BOB
                && *to == CAROL
                && frame.sender.as_str() == alice
                && frame.recipient.as_str() == carol
        })
        .map(|(_, _, frame)| frame)
        .collect();
    assert!(
        through_bob
            .iter()
            .any(|frame| frame.content.starts_with(internal_prefixes::KEY_PACKAGE)),
        "alice's key package reached carol through bob"
    );
    assert!(
        through_bob
            .iter()
            .all(|frame| !frame.content.contains("meet at the north gate")),
        "bob carried nothing he could read"
    );
    let back_through_bob: Vec<&Message> = line
        .carried
        .iter()
        .filter(|(from, to, frame)| *from == BOB && *to == ALICE && frame.sender.as_str() == carol)
        .map(|(_, _, frame)| frame)
        .collect();
    assert!(
        back_through_bob
            .iter()
            .any(|frame| frame.content.starts_with(internal_prefixes::WELCOME)),
        "carol's Welcome reached alice through bob"
    );
    let first_handshake_frame = back_through_bob.iter().find(|frame| {
        frame.content.starts_with(internal_prefixes::WELCOME)
            || frame.content.starts_with(internal_prefixes::KEY_PACKAGE)
    });
    assert!(
        first_handshake_frame
            .is_some_and(|frame| frame.content.starts_with(internal_prefixes::WELCOME)),
        "and carol answered with the Welcome, not with a package of her own: one group is \
         built, not two for a tiebreak to settle"
    );
    assert!(
        !line.nodes[BOB]
            .inbox
            .iter()
            .any(|content| content.contains("north gate")),
        "and surfaced nothing of it to his own application"
    );
}

#[test]
fn a_device_that_declines_to_carry_starts_no_session_between_others() {
    let mut bob = strict_config("bob");
    bob.relay.allow_relay = false;
    let mut line = Line::new(bob);
    let carol = line.nodes[CAROL].address.clone();
    let alice = line.nodes[ALICE].address.clone();

    let message_id = line.nodes[ALICE]
        .protocol
        .send_message(&carol, "meet at the north gate", None, None::<String>)
        .expect("queued until a session exists")
        .as_str();

    line.run(40);

    assert!(
        !line.nodes[CAROL]
            .inbox
            .iter()
            .any(|content| content == "meet at the north gate"),
        "nothing crosses a device that declined to carry"
    );
    assert_eq!(line.nodes[ALICE].delivered(&message_id), None);
    assert!(
        !line
            .carried
            .iter()
            .any(|(from, _, frame)| *from == BOB && frame.sender.as_str() == alice),
        "bob passed on nothing of alice's, her key package included"
    );
}

#[test]
fn a_session_forms_across_the_mesh_when_the_relay_takes_what_it_cannot_deliver() {
    // alice and carol are both online, and the relay has told each that the
    // other is not on it. The internet carrier still accepts every frame,
    // so a handshake sent "successfully" there and nowhere else never
    // arrives: the key package and the Welcome have to cross bob anyway.
    let mut line = Line::with_relay_on_the_ends(strict_config("bob"), true);
    let carol = line.nodes[CAROL].address.clone();

    let message_id = line.nodes[ALICE]
        .protocol
        .send_message(&carol, "meet at the north gate", None, None::<String>)
        .expect("queued until a session exists")
        .as_str();

    line.run(40);
    assert!(
        line.nodes[ALICE]
            .internet
            .as_ref()
            .expect("alice is online")
            .sent_messages()
            .iter()
            .any(
                |frame| frame.content.starts_with(internal_prefixes::KEY_PACKAGE)
                    && frame.recipient.as_str() == carol
            ),
        "the relay accepted alice's key package, so the direct send reported success"
    );
    assert_eq!(
        line.nodes[CAROL]
            .inbox
            .iter()
            .filter(|content| content.as_str() == "meet at the north gate")
            .count(),
        1,
        "and the session formed through bob all the same"
    );
    assert_eq!(line.nodes[ALICE].delivered(&message_id), Some(1));
}

#[test]
fn a_welcome_lost_while_the_path_was_down_is_sent_again_on_the_next_package() {
    // carol builds the session on alice's package, and her Welcome and fresh
    // package are lost on the way back. Her Welcome's lifecycle then expires.
    // A neighbour would re-arm it on rediscovery, but alice is two hops away
    // and is never rediscovered; alice's next package is the only sign that
    // she still holds no session.
    let mut line = Line::new(strict_config("bob"));
    let alice = line.nodes[ALICE].address.clone();
    let carol = line.nodes[CAROL].address.clone();
    line.cut = Some((CAROL, BOB));

    let message_id = line.nodes[ALICE]
        .protocol
        .send_message(&carol, "meet at the north gate", None, None::<String>)
        .expect("queued until a session exists")
        .as_str();

    for _ in 0..40 {
        line.step();
        if line.nodes[CAROL]
            .protocol
            .welcome_lifecycles
            .contains_key(&alice)
        {
            break;
        }
    }
    line.run(5);
    let lifecycle = line.nodes[CAROL]
        .protocol
        .welcome_lifecycles
        .get_mut(&alice)
        .expect("carol built a session on alice's package");
    lifecycle.state = WelcomeDeliveryState::Expired;
    lifecycle.next_retry_at = None;
    assert_eq!(line.nodes[ALICE].delivered(&message_id), None);

    // The path comes back, and alice's re-push ladder comes due.
    line.cut = None;
    let due = Instant::now()
        .checked_sub(Duration::from_secs(3_600))
        .expect("the clock is past an hour");
    for (sent_at, _) in line.nodes[ALICE].protocol.key_package_sent_to.values_mut() {
        *sent_at = due;
    }
    line.run(40);

    assert_eq!(
        line.nodes[CAROL]
            .inbox
            .iter()
            .filter(|content| content.as_str() == "meet at the north gate")
            .count(),
        1,
        "alice's re-push re-armed carol's Welcome, and the message arrived"
    );
    assert_eq!(line.nodes[ALICE].delivered(&message_id), Some(1));
}
