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

use offline_protocol::{Event, OfflineProtocol, ProtocolConfig};
use offline_protocol_core::{AppId, Message, UserId};
use offline_protocol_transport::{mock::MockTransport, Transport, TransportType};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
            neighborhood.add_node(id, default_config(id));
        }

        neighborhood
    }

    /// Adds one device, which may be configured differently from the rest —
    /// a device that declines to carry traffic, for instance.
    fn add_node(&mut self, id: &str, config: ProtocolConfig) {
        self.add_node_inner(id, config, false);
    }

    /// Adds a device that also has a working relay connection.
    ///
    /// Its relay is a hole in the ground: frames written to it leave the
    /// simulation, because the whole point of the case this exists for is a
    /// recipient the relay cannot deliver to. What comes back instead is the
    /// relay's verdict, delivered by [`Self::relay_says_unreachable`].
    fn add_online_node(&mut self, id: &str) {
        self.add_node_inner(id, default_config(id), true);
    }

    fn add_node_inner(&mut self, id: &str, config: ProtocolConfig, online: bool) {
        let radio = MockTransport::new(TransportType::BLE);
        radio.start().unwrap();
        // A device can only put a frame on a link it actually has.
        radio.set_reject_unknown_recipients(true);
        let handle = radio.clone();

        let mut node = OfflineProtocol::new(config).unwrap();
        node.transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(radio));
        if online {
            let relay = MockTransport::new(TransportType::Internet);
            relay.start().unwrap();
            node.transport_manager_mut()
                .add_transport(TransportType::Internet, Box::new(relay));
        }
        node.start().unwrap();

        self.nodes.insert(id.to_string(), node);
        self.radios.insert(id.to_string(), handle);
        self.links.insert(id.to_string(), Vec::new());
        self.inboxes.insert(id.to_string(), Vec::new());
    }

    /// The relay answering that it cannot reach the recipient of `message_id` —
    /// the one thing a device learns about a *particular* peer, as opposed to
    /// whether its own carriers are up.
    fn relay_says_unreachable(&mut self, id: &str, message_id: &str) {
        self.nodes
            .get_mut(id)
            .unwrap()
            .on_transport_send_failed(
                message_id,
                Some("recipient_unreachable: peer offline".to_string()),
            )
            .unwrap();
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

/// The configuration every simulated device starts from.
fn default_config(user_id: &str) -> ProtocolConfig {
    let mut config = ProtocolConfig::new("mesh-test", user_id);
    // These tests are about who carries what, not about encryption.
    config.encryption.require_encryption = false;
    // No hold before forwarding: the tests drive time by stepping, and the
    // hold's own behavior is covered by the governor's unit tests.
    config.mesh_relay.jitter_min = std::time::Duration::from_millis(0);
    config.mesh_relay.jitter_max = std::time::Duration::from_millis(0);
    config
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
fn an_online_sender_reaches_a_recipient_only_the_mesh_can_see() {
    // The mixed neighborhood, end to end. Alice has internet; carol does not
    // and is two links away. Having a relay connection says nothing about
    // whether carol is on it — and when the relay says she is not, the devices
    // standing next to alice are the only way there.
    let mut net = Neighborhood::new(&["bob", "carol"]);
    net.add_online_node("alice");
    net.link("alice", "bob");
    net.link("bob", "carol");

    let delivered: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let delivered_handle = Arc::clone(&delivered);
    net.node("alice").on_event(move |event| {
        if let Event::MessageDelivered { message_id, .. } = event {
            delivered_handle.lock().unwrap().push(message_id);
        }
    });

    let msg_id = net.send("alice", "carol", "meet at the north gate");

    // With the relay up the frame goes to the relay, and the neighbors are not
    // asked. (Without this the test would pass on the pre-existing failure
    // path, which offers to the mesh when no transport accepts the send.)
    net.step();
    assert_eq!(
        net.transmission_count(&msg_id),
        0,
        "an online device must not spend the mesh before the relay has spoken"
    );

    net.relay_says_unreachable("alice", &msg_id);
    net.run_until_quiet(12);

    assert_eq!(
        net.inbox("carol"),
        vec!["meet at the north gate".to_string()],
        "the relay's verdict must send the message to the neighbors instead"
    );
    assert_eq!(
        net.deliveries_to("carol", &msg_id),
        1,
        "and it must arrive exactly once"
    );
    assert_eq!(
        *delivered.lock().unwrap(),
        vec![msg_id.clone()],
        "alice must learn it was delivered — parking removed the pending ACK, \
         so a delivery she cannot settle would be probed until it expired"
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
    // The crowded-room case: a dense cluster of devices that all hear each
    // other, carrying a message to someone standing just outside it. Without
    // restraint every device in the cluster repeats what it heard to every
    // other, and one message fills the room with copies.
    //
    // The recipient is deliberately outside the cluster. If they were inside
    // it, the sender would simply hand the message over directly and no
    // carrying would happen at all — the test would pass while exercising
    // nothing.
    let cluster = ["n0", "n1", "n2", "n3", "n4", "n5", "n6"];
    let mut net = Neighborhood::new(&["n0", "n1", "n2", "n3", "n4", "n5", "n6", "far"]);
    for (i, a) in cluster.iter().enumerate() {
        for b in cluster.iter().skip(i + 1) {
            net.link(a, b);
        }
    }
    // One device in the cluster — not the sender — can reach the recipient.
    net.link("n6", "far");

    let msg_id = net.send("n0", "far", "in the crowd");

    net.run_until_quiet(24);

    assert_eq!(net.inbox("far"), vec!["in the crowd".to_string()]);

    // Seven devices with a link between every pair is 21 links, plus the one
    // out to the recipient. Delivery costs fewer transmissions than the cluster
    // has links — uncontrolled repetition would cross them all, several times
    // over, until the hop budget ran out.
    //
    // This is the *worst* case rather than the expected one. These devices run
    // with the pre-transmit hold set to zero and are stepped in lock-step, so
    // every forward is due at once and none of them ever gets the chance to
    // stand down for a neighbor. Real timing spreads them out, and the
    // cancellation that follows only lowers this number.
    let crossings = net.transmission_count(&msg_id);
    assert!(
        crossings < 21,
        "one message crossed links {crossings} times in a room of {} devices, \
         which is more than the cluster has links",
        cluster.len()
    );

    // And it really did travel through the cluster rather than going direct.
    assert!(
        net.transmissions
            .iter()
            .any(|(from, to, id)| from == "n6" && to == "far" && id == &msg_id),
        "the message should have reached the recipient through the cluster"
    );
}

#[test]
fn the_only_device_that_can_reach_the_recipient_does_not_stand_down() {
    // Two forwarders that can hear each other, only one of which can reach the
    // recipient:
    //
    //     alice — bob — carol — dave
    //       \_____________/
    //
    // Both bob and carol take a copy. Whichever transmits first hands it to the
    // other, and standing down on a duplicate is normally right — a neighbor
    // covered the same ground. But carol holds the only link to dave. If carol
    // drops her copy because bob's arrived, dave never hears it, and the id is
    // suppressed so the sender's retries cannot rescue it either.
    let mut net = Neighborhood::new(&["alice", "bob", "carol", "dave"]);
    net.link("alice", "bob");
    net.link("alice", "carol");
    net.link("bob", "carol");
    net.link("carol", "dave");

    net.send("alice", "dave", "only carol can finish this");

    net.run_until_quiet(16);

    assert_eq!(
        net.inbox("dave"),
        vec!["only carol can finish this".to_string()],
        "the device holding the last link must deliver, not defer to a neighbor"
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
    let mut config = default_config("bob");
    config.relay.allow_relay = false;

    let mut net = Neighborhood::new(&["alice", "carol"]);
    net.add_node("bob", config);

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
fn a_device_that_carries_nothing_still_gets_its_own_answer_out() {
    // Declining to carry other people's traffic must not cost a device the
    // ability to answer its own. Carol is two links from alice, so her
    // acknowledgement can only get home by being handed to bob — the one thing
    // a relay-declining device still has to be able to do, and the reason
    // handing over our *own* frames is deliberately not gated on that setting.
    // Without it her sender retransmits to exhaustion and reports a failure for
    // a message that was delivered and read.
    let mut config = default_config("carol");
    config.relay.allow_relay = false;

    let mut net = Neighborhood::new(&["alice", "bob"]);
    net.add_node("carol", config);

    net.link("alice", "bob");
    net.link("bob", "carol");

    net.send("alice", "carol", "do you copy?");

    net.run_until_quiet(12);

    assert_eq!(net.inbox("carol"), vec!["do you copy?".to_string()]);
    assert!(
        net.transmissions
            .iter()
            .any(|(from, to, _)| from == "carol" && to == "bob"),
        "a device that carries nothing must still hand its own answer to a neighbor"
    );
    assert!(
        net.transmissions
            .iter()
            .any(|(from, to, _)| from == "bob" && to == "alice"),
        "and that answer must reach the sender"
    );
}

#[test]
fn an_answer_is_carried_even_when_a_transport_accepts_a_peer_it_cannot_reach() {
    // Only BLE refuses a recipient it holds no link to. Wi-Fi Direct and
    // Reticulum enqueue for anyone and return `Ok`, so a frame handed to them
    // for someone out of range is queued for a link that never drains —
    // reported as sent, silently swallowed.
    //
    // That matters most for an acknowledgement. Carol's answer to alice is two
    // links away and has to be carried; if the decision to carry it is inferred
    // from a send failure, the accepting carrier reports success first and the
    // answer dies. Alice then retransmits a message carol already has, and
    // eventually reports failure for a message that was delivered and read.
    let mut carol = OfflineProtocol::new(default_config("carol")).unwrap();

    // The link carol really has: bob, who can reach alice.
    let ble = MockTransport::new(TransportType::BLE);
    ble.start().unwrap();
    ble.set_reject_unknown_recipients(true);
    ble.add_connected_peer("bob", -55);
    let ble_handle = ble.clone();

    // A carrier that is up and takes any recipient without holding a link to
    // one — the shape both Wi-Fi Direct and Reticulum have in production.
    let swallowing = MockTransport::new(TransportType::WiFiDirect);
    swallowing.start().unwrap();
    let swallowing_handle = swallowing.clone();

    carol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));
    carol
        .transport_manager_mut()
        .add_transport(TransportType::WiFiDirect, Box::new(swallowing));
    carol.start().unwrap();

    // Alice's message arrives over the link bob handed it across.
    let msg = message("alice", "carol", "did this get through?");
    ble_handle.queue_message_from(msg, "bob".to_string());
    assert!(
        carol.receive_message().is_some(),
        "carol should have received the message"
    );
    carol.process().unwrap();

    let carried: Vec<_> = ble_handle
        .peer_sends()
        .into_iter()
        .filter(|(_, m)| m.recipient.as_str() == "alice")
        .collect();
    assert_eq!(
        carried.len(),
        1,
        "carol's answer must be handed to a neighbor to carry"
    );
    assert_eq!(carried[0].0, "bob", "and handed to the neighbor she has");

    assert!(
        swallowing_handle
            .sent_messages()
            .iter()
            .all(|m| m.recipient.as_str() != "alice"),
        "the answer must not be spent on a carrier that cannot reach alice"
    );
}

#[test]
fn an_answer_is_carried_when_the_last_hop_arrived_over_the_swallowing_carrier() {
    // The same hazard as above, reached the way it actually happens. Wi-Fi
    // Direct is the *preferred* mesh carrier, so on a real fleet it is usually
    // the link a forwarded frame's last hop crosses — and answering on the link
    // a message arrived over is the first thing an acknowledgement tries.
    //
    // That combination is what makes the swallow reachable: Wi-Fi Direct
    // accepts alice as a recipient it holds no link to, reports success, and
    // the answer is queued for a link that never drains. Alice then
    // retransmits a message carol has already read, until she reports it
    // failed. Answering the arrival link therefore has to be gated on whether
    // that carrier can address the sender at all.
    let mut carol = OfflineProtocol::new(default_config("carol")).unwrap();

    // The link carol really has: bob, who can reach alice.
    let ble = MockTransport::new(TransportType::BLE);
    ble.start().unwrap();
    ble.set_reject_unknown_recipients(true);
    ble.add_connected_peer("bob", -55);
    let ble_handle = ble.clone();

    // Up, holding no links, and accepting anyone — the production shape.
    let swallowing = MockTransport::new(TransportType::WiFiDirect);
    swallowing.start().unwrap();
    let swallowing_handle = swallowing.clone();

    carol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));
    carol
        .transport_manager_mut()
        .add_transport(TransportType::WiFiDirect, Box::new(swallowing));
    carol.start().unwrap();

    // Alice's message arrives over Wi-Fi Direct, handed across by bob.
    let msg = message("alice", "carol", "did this get through?");
    swallowing_handle.queue_message_from(msg, "bob".to_string());
    assert!(
        carol.receive_message().is_some(),
        "carol should have received the message"
    );
    carol.process().unwrap();

    assert!(
        swallowing_handle
            .sent_messages()
            .iter()
            .all(|m| m.recipient.as_str() != "alice"),
        "the answer must not be spent on the carrier that only pretends to reach alice"
    );

    let carried: Vec<_> = ble_handle
        .peer_sends()
        .into_iter()
        .filter(|(_, m)| m.recipient.as_str() == "alice")
        .collect();
    assert_eq!(
        carried.len(),
        1,
        "carol's answer must be carried by the neighbor she has"
    );
    assert_eq!(carried[0].0, "bob");
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
