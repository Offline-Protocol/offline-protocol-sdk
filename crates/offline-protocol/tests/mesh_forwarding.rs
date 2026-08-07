//! Multi-device forwarding, exercised over a simulated neighborhood.
//!
//! These tests build several protocol instances and wire them together by
//! topology, then move frames between them exactly the way a radio would:
//! whatever a device hands to a neighbor is delivered to that neighbor and to
//! nobody else. Nothing here reaches for internals — a message is sent by one
//! device and looked for at another, so what is being tested is the behavior
//! the network actually provides.
//!
//! They assert two different things, and both matter:
//!
//! - **Reach** — a message gets to someone no single device can see.
//! - **Cost** — it does so without the network transmitting more than it should.
//!   Reach alone is easy to get by repeating everything endlessly; the counts in
//!   these tests are what separate a working mesh from one that floods.

use offline_protocol::{OfflineProtocol, ProtocolConfig};
use offline_protocol_core::{AppId, Message, UserId};
use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
use std::collections::HashMap;

/// A simulated neighborhood of devices.
///
/// Each device owns one mock radio. Links are declared, not discovered, so a
/// test states its topology and knows exactly which devices can hear each
/// other.
struct Neighborhood {
    nodes: HashMap<String, OfflineProtocol>,
    radios: HashMap<String, MockTransport>,
    /// Who each device can hear directly.
    links: HashMap<String, Vec<String>>,
    /// Every hand-off that has crossed a link, as `(from, to, message_id)`.
    transmissions: Vec<(String, String, String)>,
    /// What each device has surfaced to its app, kept because stepping the
    /// network is what drains it.
    inboxes: HashMap<String, Vec<String>>,
}

impl Neighborhood {
    fn new(node_ids: &[&str]) -> Self {
        let mut neighborhood = Self {
            nodes: HashMap::new(),
            radios: HashMap::new(),
            links: HashMap::new(),
            transmissions: Vec::new(),
            inboxes: HashMap::new(),
        };

        for id in node_ids {
            let mut config = ProtocolConfig::new("mesh-test", *id);
            // These tests are about who carries what, not about encryption.
            config.encryption.require_encryption = false;
            // No hold before forwarding: the tests drive time by stepping, and
            // the hold's own behavior is covered by the governor's unit tests.
            config.mesh_relay.jitter_min = std::time::Duration::from_millis(0);
            config.mesh_relay.jitter_max = std::time::Duration::from_millis(0);

            let radio = MockTransport::new(TransportType::BLE);
            radio.start().unwrap();
            // A device can only put a frame on a link it actually has.
            radio.set_reject_unknown_recipients(true);
            let handle = radio.clone();

            let mut node = OfflineProtocol::new(config).unwrap();
            node.transport_manager_mut()
                .add_transport(TransportType::BLE, Box::new(radio));
            node.start().unwrap();

            neighborhood.nodes.insert(id.to_string(), node);
            neighborhood.radios.insert(id.to_string(), handle);
            neighborhood.links.insert(id.to_string(), Vec::new());
            neighborhood.inboxes.insert(id.to_string(), Vec::new());
        }

        neighborhood
    }

    /// Puts two devices in range of each other.
    fn link(&mut self, a: &str, b: &str) {
        self.radios[a].add_connected_peer(b, -55);
        self.radios[b].add_connected_peer(a, -55);
        self.links.get_mut(a).unwrap().push(b.to_string());
        self.links.get_mut(b).unwrap().push(a.to_string());
        // Each end learns of the other, as it would on discovery.
        self.nodes.get_mut(a).unwrap().on_neighbor_discovered(b);
        self.nodes.get_mut(b).unwrap().on_neighbor_discovered(a);
    }

    /// Takes a device out of range of everyone (it walks away).
    fn unlink_all(&mut self, id: &str) {
        let peers = self.links[id].clone();
        for peer in peers {
            self.radios[&peer].remove_connected_peer(id);
            self.radios[id].remove_connected_peer(&peer);
            self.nodes.get_mut(&peer).unwrap().on_neighbor_lost(id);
            self.links.get_mut(&peer).unwrap().retain(|p| p != id);
        }
        self.links.get_mut(id).unwrap().clear();
    }

    /// Runs one round: every device processes what it holds, and whatever it
    /// hands to a neighbor is delivered to that neighbor.
    ///
    /// Returns how many hand-offs crossed a link this round, so a caller can
    /// run until the network goes quiet.
    fn step(&mut self) -> usize {
        // Let each device act on what it has received, keeping whatever it
        // surfaced to its app.
        for id in self.node_ids() {
            let node = self.nodes.get_mut(&id).unwrap();
            let mut delivered = Vec::new();
            while let Some(message) = node.receive_message() {
                delivered.push(message.content);
            }
            node.process().unwrap();
            self.inboxes.get_mut(&id).unwrap().extend(delivered);
        }

        // Move what they put on the air. Two ways a frame leaves a device: it
        // was addressed to a neighbor and sent normally, or it was handed to a
        // neighbor to carry. Both cross exactly one link.
        let mut moved = 0;
        for from in self.node_ids() {
            let mut handed: Vec<(String, Message)> = self.radios[&from]
                .sent_messages()
                .into_iter()
                .map(|m| (m.recipient.as_str().to_string(), m))
                .collect();
            handed.extend(self.radios[&from].peer_sends());
            self.radios[&from].clear_sent_messages();
            self.radios[&from].clear_peer_sends();

            for (to, message) in handed {
                // A device can only be handed something by a device in range.
                assert!(
                    self.links[&from].contains(&to),
                    "{from} transmitted to {to}, which it has no link to"
                );
                self.transmissions
                    .push((from.clone(), to.clone(), message.id.as_str()));
                // The receiver sees which link it arrived on, as a radio reports.
                self.radios[&to].queue_message_from(message, from.clone());
                moved += 1;
            }
        }

        moved
    }

    /// Steps until nothing more moves, or the round limit is hit.
    ///
    /// The limit is a guard: a mesh that will not go quiet is the failure this
    /// whole design is meant to prevent, so hitting it fails the test rather
    /// than hanging it.
    fn run_until_quiet(&mut self, max_rounds: usize) -> usize {
        for round in 0..max_rounds {
            if self.step() == 0 {
                return round + 1;
            }
        }
        panic!("network never went quiet after {max_rounds} rounds");
    }

    fn node_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.nodes.keys().cloned().collect();
        ids.sort();
        ids
    }

    fn node(&mut self, id: &str) -> &mut OfflineProtocol {
        self.nodes.get_mut(id).unwrap()
    }

    /// Sends a message the way an app would, returning its id.
    ///
    /// Going through the real send path is the point: whether the recipient is
    /// reachable, and what happens when they are not, is exactly what these
    /// tests are about.
    fn send(&mut self, from: &str, to: &str, content: &str) -> String {
        self.nodes
            .get_mut(from)
            .unwrap()
            .send_message(to, content, None, None::<String>)
            .expect("send should be accepted locally even with no route")
            .as_str()
    }

    /// Everything `id` has surfaced to its app so far.
    fn inbox(&mut self, id: &str) -> Vec<String> {
        // Catch anything delivered since the last step.
        let node = self.nodes.get_mut(id).unwrap();
        let mut delivered = Vec::new();
        while let Some(message) = node.receive_message() {
            delivered.push(message.content);
        }
        self.inboxes.get_mut(id).unwrap().extend(delivered);
        self.inboxes[id].clone()
    }

    /// How many times a message crossed any link.
    fn transmission_count(&self, message_id: &str) -> usize {
        self.transmissions
            .iter()
            .filter(|(_, _, id)| id == message_id)
            .count()
    }

    /// How many times a message was handed to a particular device.
    fn deliveries_to(&self, node_id: &str, message_id: &str) -> usize {
        self.transmissions
            .iter()
            .filter(|(_, to, id)| to == node_id && id == message_id)
            .count()
    }
}

fn message(from: &str, to: &str, content: &str) -> Message {
    Message::new(
        UserId::new(from).unwrap(),
        UserId::new(to).unwrap(),
        AppId::new("mesh-test").unwrap(),
        content,
    )
}

#[test]
fn a_message_reaches_someone_two_rooms_away() {
    // alice — bob — carol. Alice and carol cannot hear each other at all.
    let mut net = Neighborhood::new(&["alice", "bob", "carol"]);
    net.link("alice", "bob");
    net.link("bob", "carol");

    let msg_id = net.send("alice", "carol", "meet at the north gate");

    net.run_until_quiet(12);

    assert_eq!(
        net.inbox("carol"),
        vec!["meet at the north gate".to_string()],
        "carol must receive a message from someone she cannot hear"
    );
    assert_eq!(
        net.deliveries_to("carol", &msg_id),
        1,
        "and receive it exactly once"
    );
}

#[test]
fn the_answer_finds_its_way_back() {
    // The reverse path matters as much as the forward one: without it the
    // sender keeps retransmitting a message that was delivered.
    let mut net = Neighborhood::new(&["alice", "bob", "carol"]);
    net.link("alice", "bob");
    net.link("bob", "carol");

    net.send("alice", "carol", "are you here?");

    net.run_until_quiet(12);

    assert_eq!(net.inbox("carol").len(), 1);

    // Carol's acknowledgement is addressed to alice, who is two links away, so
    // it can only have arrived by being carried.
    let acked_back = net
        .transmissions
        .iter()
        .any(|(from, to, _)| from == "bob" && to == "alice");
    assert!(
        acked_back,
        "carol's answer must be carried back toward alice"
    );
}

#[test]
fn a_message_crosses_a_line_of_five_devices() {
    // Depth, not just one hop: a — b — c — d — e, with each device able to hear
    // only its immediate neighbors.
    let mut net = Neighborhood::new(&["a", "b", "c", "d", "e"]);
    net.link("a", "b");
    net.link("b", "c");
    net.link("c", "d");
    net.link("d", "e");

    let msg_id = net.send("a", "e", "all the way down");

    net.run_until_quiet(20);

    assert_eq!(net.inbox("e"), vec!["all the way down".to_string()]);
    assert_eq!(net.deliveries_to("e", &msg_id), 1);
}

#[test]
fn a_message_with_two_paths_arrives_once() {
    // A diamond: both b and c can carry alice's message to dave. Both should
    // try — that redundancy is the point of a mesh — but dave must surface it
    // to his app exactly once.
    let mut net = Neighborhood::new(&["alice", "b", "c", "dave"]);
    net.link("alice", "b");
    net.link("alice", "c");
    net.link("b", "dave");
    net.link("c", "dave");

    let msg_id = net.send("alice", "dave", "two ways round");

    net.run_until_quiet(16);

    assert_eq!(
        net.inbox("dave"),
        vec!["two ways round".to_string()],
        "arriving twice must not mean being read twice"
    );

    // And the redundancy stays redundancy: the message does not circulate.
    // Six links exist; a message that kept going would cross far more.
    assert!(
        net.transmission_count(&msg_id) <= 6,
        "message crossed links {} times, which is more than the topology needs",
        net.transmission_count(&msg_id)
    );
}

#[test]
fn a_message_does_not_circle_a_ring_forever() {
    // A loop is where uncontrolled forwarding shows itself: with no suppression
    // a frame goes round and round until its hop budget runs out, multiplying
    // at every device on the way.
    let mut net = Neighborhood::new(&["a", "b", "c", "d"]);
    net.link("a", "b");
    net.link("b", "c");
    net.link("c", "d");
    net.link("d", "a");

    let msg_id = net.send("a", "c", "round the ring");

    let rounds = net.run_until_quiet(20);

    assert_eq!(net.inbox("c"), vec!["round the ring".to_string()]);
    assert!(
        net.transmission_count(&msg_id) <= 8,
        "message crossed links {} times going round a four-device ring",
        net.transmission_count(&msg_id)
    );
    assert!(rounds < 20, "the ring must settle, not keep turning");
}

#[test]
fn everyone_hearing_everyone_does_not_multiply_the_traffic() {
    // The crowded-room case. Eight devices, all in range of each other, and one
    // message. Without restraint each device would repeat what it heard to
    // everyone else and the room would fill with copies of one message.
    let ids = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    let mut net = Neighborhood::new(&ids);
    for (i, a) in ids.iter().enumerate() {
        for b in ids.iter().skip(i + 1) {
            net.link(a, b);
        }
    }

    let msg_id = net.send("n0", "n7", "in the crowd");

    net.run_until_quiet(24);

    assert_eq!(net.inbox("n7"), vec!["in the crowd".to_string()]);

    // Eight devices with a link between every pair is 28 links. Uncontrolled
    // forwarding would cross most of them repeatedly; the bound here is well
    // under a single pass over the topology.
    let crossings = net.transmission_count(&msg_id);
    assert!(
        crossings <= 20,
        "one message crossed links {crossings} times in a room of {} devices",
        ids.len()
    );
}

#[test]
fn a_message_survives_the_carrier_walking_away() {
    // Handing a message to a neighbor is not delivery. If that neighbor leaves
    // before passing it on, the sender's own retry has to be what recovers it —
    // which is why forwarding never settles the outbox.
    let mut net = Neighborhood::new(&["alice", "bob", "carol"]);
    net.link("alice", "bob");
    net.link("bob", "carol");

    net.send("alice", "carol", "still gets there");

    // One round: alice hands it to bob, and bob has not passed it on yet.
    net.step();
    // Bob walks out of range of everyone, taking the message with him.
    net.unlink_all("bob");
    net.run_until_quiet(8);

    assert!(
        net.inbox("carol").is_empty(),
        "with the only carrier gone, nothing should have arrived"
    );

    // Alice is now in range of carol directly, and her retry delivers.
    net.link("alice", "carol");
    net.node("alice").process().unwrap();
    net.run_until_quiet(12);

    assert_eq!(
        net.inbox("carol"),
        vec!["still gets there".to_string()],
        "the sender's own retry must still be able to deliver"
    );
}

#[test]
fn a_device_that_opts_out_carries_nothing() {
    // Carrying other people's traffic costs battery, and a device is allowed to
    // decline. It must still be able to send and receive its own.
    let mut config = ProtocolConfig::new("mesh-test", "bob");
    config.encryption.require_encryption = false;
    config.relay.allow_relay = false;

    let mut net = Neighborhood::new(&["alice", "carol"]);

    let radio = MockTransport::new(TransportType::BLE);
    radio.start().unwrap();
    radio.set_reject_unknown_recipients(true);
    let handle = radio.clone();
    let mut bob = OfflineProtocol::new(config).unwrap();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(radio));
    bob.start().unwrap();
    net.nodes.insert("bob".to_string(), bob);
    net.radios.insert("bob".to_string(), handle);
    net.links.insert("bob".to_string(), Vec::new());
    net.inboxes.insert("bob".to_string(), Vec::new());

    net.link("alice", "bob");
    net.link("bob", "carol");

    let msg_id = net.send("alice", "carol", "please pass this on");

    net.run_until_quiet(10);

    assert!(
        net.inbox("carol").is_empty(),
        "a device that declined to carry traffic must not have carried it"
    );
    assert_eq!(
        net.transmissions
            .iter()
            .filter(|(from, _, id)| from == "bob" && id == &msg_id)
            .count(),
        0,
        "and must not have transmitted it at all"
    );
}

#[test]
fn a_frame_claiming_an_absurd_reach_is_cut_down() {
    // Nothing authenticates how far a frame says it may travel, so a device
    // must not take that claim at face value. The claim can only come off the
    // wire, so it is injected at b as though a had sent it.
    let mut net = Neighborhood::new(&["a", "b", "c"]);
    net.link("a", "b");
    net.link("b", "c");

    let mut msg = message("a", "c", "travel forever");
    msg.ttl = offline_protocol_core::TTL::new(255).unwrap();
    net.radios["b"].queue_message_from(msg, "a".to_string());

    net.step();

    // What b handed onward carries a budget b vouches for, not the one that
    // arrived.
    let onward = net.radios["c"]
        .receive()
        .unwrap()
        .expect("b should have carried it to c");
    assert!(
        onward.ttl.value() <= 8,
        "a frame claiming 255 hops was passed on with {}",
        onward.ttl.value()
    );
}
