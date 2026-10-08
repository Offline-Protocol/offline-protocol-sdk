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
use std::time::Duration;

use offline_protocol_core::Message;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::create_test_config_for_user;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::test_identity::id;
use crate::{Event, ProtocolConfig};

struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
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
        // A device can only put a frame on a link it actually has, which is
        // what keeps alice and carol apart.
        mock.set_reject_unknown_recipients(true);
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
}

const ALICE: usize = 0;
const BOB: usize = 1;
const CAROL: usize = 2;

impl Line {
    fn new(bob: ProtocolConfig) -> Self {
        let mut line = Self {
            nodes: vec![
                Node::new("alice", strict_config("alice")),
                Node::new("bob", bob),
                Node::new("carol", strict_config("carol")),
            ],
            carried: Vec::new(),
        };
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
