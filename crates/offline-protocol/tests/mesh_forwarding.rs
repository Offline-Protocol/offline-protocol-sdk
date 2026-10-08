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

use offline_protocol::{CustodyConfig, Event, OfflineProtocol, ProtocolConfig};
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
    /// The frames themselves, in the same order, for a test that asserts on
    /// what a device was actually handed rather than only that it was.
    frames: Vec<(String, String, Message)>,
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
            frames: Vec::new(),
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

    /// An online device whose scoring puts the relay ahead of its radio.
    ///
    /// Without `prefer_online` the selector already demotes the relay below
    /// every mesh transport, so the ordering hides whether the recipient was
    /// ever considered. This is the configuration where reaching a neighbour
    /// has to be decided by a fact about the recipient rather than by carrier
    /// scoring.
    fn add_relay_first_node(&mut self, id: &str) {
        let mut config = default_config(id);
        config.dors.prefer_online = true;
        self.add_node_inner(id, config, true);
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
                self.frames
                    .push((from.clone(), to.clone(), message.clone()));
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
fn an_online_sender_hands_the_frame_straight_to_a_neighbor_it_can_see() {
    // The other half of the mixed neighborhood. Above, carol is two links away
    // and only the relay's verdict can reveal it. Here bob is standing right
    // next to alice, and alice's own scoring prefers the relay — which is a
    // hole in the ground. Waiting for a verdict to discover a neighbour that
    // was addressable from the first instant is latency for nothing.
    let mut net = Neighborhood::new(&["bob"]);
    net.add_relay_first_node("alice");
    net.link("alice", "bob");

    let msg_id = net.send("alice", "bob", "you are right here");
    net.run_until_quiet(6);

    assert_eq!(
        net.inbox("bob"),
        vec!["you are right here".to_string()],
        "a recipient one link away must be reached over that link, \
         with no relay verdict needed first"
    );
    assert_eq!(net.deliveries_to("bob", &msg_id), 1, "and exactly once");
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
    // a forward is due in the round it was queued, and only the copies that
    // land in that same round take their senders off its fan-out. Real timing
    // spreads the forwards out, so more copies arrive while each one waits and
    // the fan-outs narrow further, which only lowers this number.
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
fn a_device_hearing_a_frame_from_every_side_still_carries_it_onward() {
    // Three devices hear each other and the sender, and each also reaches
    // dave, the one way on toward the recipient:
    //
    //     sender — {a, b, c} — dave — erin — far
    //
    // a, b and c each fan out to the three neighbors they have left, so dave
    // receives three copies in one round. Cancelling on a duplicate assumed a
    // neighbor's transmission had covered the region; it had covered a, b and
    // c, never erin. Dave stood down every time, the frame died with its id
    // suppressed everywhere it had been, and the sender's retries could not
    // rescue it (#510). No tie-break is involved here, so this fails every
    // run rather than the ~2% the dense-cluster test did.
    let mut net = Neighborhood::new(&["sender", "a", "b", "c", "dave", "erin", "far"]);
    for relay in ["a", "b", "c"] {
        net.link("sender", relay);
        net.link(relay, "dave");
    }
    net.link("a", "b");
    net.link("a", "c");
    net.link("b", "c");
    net.link("dave", "erin");
    net.link("erin", "far");

    let msg_id = net.send("sender", "far", "around the crowd");

    net.run_until_quiet(16);

    assert_eq!(net.inbox("far"), vec!["around the crowd".to_string()]);
    assert!(
        net.transmissions
            .iter()
            .any(|(from, to, id)| from == "dave" && to == "erin" && id == &msg_id),
        "dave must carry the frame on to the one neighbor that had no copy"
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
    // other, and that copy proves only that its sender holds the frame. Carol
    // holds the only link to dave. If carol dropped her copy because bob's
    // arrived, dave would never hear it, and the id would be suppressed so the
    // sender's retries could not rescue it either. A copy takes only its
    // sender off carol's fan-out.
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

// ============================================================================
// Custody (docs/spec/custody.md)
// ============================================================================

/// A device that holds a neighbour's replication frames, admitting strangers
/// so the deposit below is judged by the quotas rather than refused at the
/// tier (these devices hold no sessions with each other).
fn custodian_config(user_id: &str) -> ProtocolConfig {
    let mut config = default_config(user_id);
    config.custody = CustodyConfig {
        enabled: true,
        stranger_max_entries: 8,
        stranger_max_bytes: 512 * 1024,
        ..CustodyConfig::default()
    };
    config
}

/// The frame a depositor offers into custody: its own sealed replication
/// frame with the class token on it. Built by hand because these devices
/// hold no sessions; the custodian cannot see inside a sealed frame either
/// way, and judges the outer message alone.
fn deposit(from: &str, to: &str) -> Message {
    let mut frame = message(
        from,
        to,
        &format!(
            "{}opaque-ciphertext",
            offline_protocol_sealed::prefixes::ENCRYPTED
        ),
    );
    // The reserved key from the wire-format chapter, as the depositor's
    // engine writes it; pinned against the engine's constant by its own
    // tests.
    frame
        .metadata
        .insert("__custody".to_string(), "data".to_string());
    frame
}

#[test]
fn a_deposited_frame_outlives_the_carrier_walking_away() {
    // The sibling of `a_message_survives_the_carrier_walking_away`. There the
    // carrier that left took the message with it and the sender's own retry
    // was the recovery. Here the carrier holds custody: it keeps the frame
    // past the seconds a forwarder gives it, and delivers it when the
    // recipient appears, with no help from the sender at all.
    let mut net = Neighborhood::new(&["alice", "carol"]);
    net.add_node("bob", custodian_config("bob"));
    net.link("alice", "bob");

    let frame = deposit("alice", "carol");
    let frame_id = frame.id.as_str();
    // Alice hands it over her link to bob, as a depositor does.
    net.radios["bob"].queue_message_from(frame, "alice".to_string());
    net.step();
    assert_eq!(
        net.node("bob").custody_stats().held,
        0,
        "a forward is not a deposit while the mesh can still carry it"
    );

    // Bob walks out of range of everyone with the frame, and nobody can take
    // it from him for longer than a forwarder is willing to wait. That wait
    // is the governor's overdue cut-off, five seconds of wall-clock time,
    // which is why this test is slower than its neighbours.
    net.unlink_all("bob");
    std::thread::sleep(std::time::Duration::from_millis(5_200));
    net.run_until_quiet(8);

    let stats = net.node("bob").custody_stats();
    assert_eq!(stats.accepted, 1, "taken into custody at the drop point");
    assert_eq!(stats.held, 1);
    assert!(net.inbox("carol").is_empty());
    assert_eq!(net.deliveries_to("carol", &frame_id), 0);

    // Carol comes into range of bob, and bob alone.
    net.link("bob", "carol");
    net.run_until_quiet(8);

    assert_eq!(
        net.deliveries_to("carol", &frame_id),
        1,
        "the custodian delivers the held frame to its recipient, once"
    );
    let stats = net.node("bob").custody_stats();
    assert_eq!(stats.delivered, 1);
    assert_eq!(
        stats.held, 0,
        "delivery to the recipient releases the frame"
    );
    let (_, _, delivered) = net
        .frames
        .iter()
        .find(|(from, to, m)| from == "bob" && to == "carol" && m.id.as_str() == frame_id)
        .expect("crossed the bob-carol link");
    assert!(
        !delivered.metadata.contains_key("__custody"),
        "what the custodian transmits never carries the request"
    );
}

#[test]
fn a_device_with_custody_off_still_strips_the_request_and_holds_nothing() {
    // Every forwarder strips the request, custody-enabled or not: it is what
    // keeps a deposit to one hop. And the default is off, so the ordinary
    // mesh is exactly as it was.
    let mut net = Neighborhood::new(&["alice", "bob", "dave"]);
    net.link("alice", "bob");
    net.link("bob", "dave");

    let frame = deposit("alice", "carol");
    let frame_id = frame.id.as_str();
    net.radios["bob"].queue_message_from(frame.clone(), "alice".to_string());
    net.run_until_quiet(8);

    assert_eq!(
        net.deliveries_to("dave", &frame_id),
        1,
        "carried on as before"
    );
    let (_, _, forwarded) = net
        .frames
        .iter()
        .find(|(from, to, m)| from == "bob" && to == "dave" && m.id.as_str() == frame_id)
        .expect("crossed the bob-dave link");
    assert!(
        !forwarded.metadata.contains_key("__custody"),
        "the request travels exactly one hop"
    );
    assert_eq!(
        forwarded.content, frame.content,
        "only the outer metadata is touched"
    );
    assert_eq!(net.node("bob").custody_stats().held, 0);
    assert_eq!(net.node("bob").custody_stats().accepted, 0);
}

#[test]
fn a_message_lost_on_a_dead_direct_link_is_carried_by_the_mesh_on_retry() {
    // The first send took the direct link, which was already dead: the carrier
    // accepted the frame and it never arrived (a half-open stream does exactly
    // this until its keepalive notices). By the time the acknowledgement times
    // out the link is gone, carol is reachable only through bob, and every
    // carrier refuses the resend because none holds a link to her.
    //
    // A refused first send is offered to the neighbors; a refused resend has to
    // be too, or the message waits in the outbox for a direct link that may
    // never come back while a path through the crowd sits unused.
    let mut alice_config = default_config("alice");
    alice_config.reliability.ack.default_timeout_ms = 50;
    alice_config.reliability.retry.initial_delay_ms = 10;

    let mut net = Neighborhood::new(&["bob", "carol"]);
    net.add_node("alice", alice_config);
    net.link("alice", "bob");
    net.link("bob", "carol");
    net.link("alice", "carol");

    let delivered = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&delivered);
    net.node("alice").on_event(move |event| {
        if let Event::MessageDelivered { message_id, .. } = event {
            seen.lock().unwrap().push(message_id);
        }
    });

    let msg_id = net.send("alice", "carol", "around the dead link");

    // The direct link swallows the frame.
    assert_eq!(
        net.radios["alice"].sent_messages().len(),
        1,
        "the first send should have gone straight to carol"
    );
    assert!(net.radios["alice"].peer_sends().is_empty());
    net.radios["alice"].clear_sent_messages();

    // Then the link is gone; bob still hears both of them.
    net.radios["alice"].remove_connected_peer("carol");
    net.radios["carol"].remove_connected_peer("alice");
    net.node("alice").on_neighbor_lost("carol");
    net.node("carol").on_neighbor_lost("alice");
    net.links.get_mut("alice").unwrap().retain(|p| p != "carol");
    net.links.get_mut("carol").unwrap().retain(|p| p != "alice");

    // Let the acknowledgement time out and the retry come due.
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        net.step();
        if !delivered.lock().unwrap().is_empty() {
            break;
        }
    }

    assert_eq!(
        net.inbox("carol"),
        vec!["around the dead link".to_string()],
        "the resend must be carried through bob"
    );
    assert!(
        net.transmissions
            .iter()
            .any(|(from, to, id)| from == "bob" && to == "carol" && id == &msg_id),
        "bob must have carried it"
    );
    assert_eq!(
        *delivered.lock().unwrap(),
        vec![msg_id],
        "and alice must learn it was delivered"
    );
}

#[test]
fn a_flushed_message_no_carrier_takes_is_handed_to_the_neighbors() {
    // A carrier coming up re-drives the whole outbox at once, ignoring the
    // backoff. Alice was alone when she wrote to carol, so nothing took the
    // message and nobody was there to carry it. Bob arrives, and he can reach
    // carol; the flush that follows must offer him the message rather than
    // putting it straight back on the retry queue to wait out its backoff.
    let mut net = Neighborhood::new(&["alice", "bob", "carol"]);
    net.link("bob", "carol");

    let msg_id = net.send("alice", "carol", "carried on the flush");
    assert_eq!(net.transmission_count(&msg_id), 0, "alice was alone");

    net.link("alice", "bob");
    net.node("alice").flush_outbox_all();
    // Rounds only, no sleeping: the retry queue's own timer is a second away
    // and must not be what delivers this.
    for _ in 0..6 {
        net.step();
    }

    assert_eq!(
        net.inbox("carol"),
        vec!["carried on the flush".to_string()],
        "the flush must hand the message to bob"
    );
}

#[test]
fn a_resend_a_carrier_swallows_is_still_handed_to_the_neighbors() {
    // Wi-Fi Direct and Reticulum take a frame for any recipient and report
    // success, so a resend can "succeed" into a carrier that holds no link to
    // the recipient. Alice's first attempt went that way while she was alone;
    // bob has arrived since, and he can reach carol. The resend after the
    // acknowledgement times out is swallowed the same way, and it has to be
    // offered to bob as a first send would be, not counted as sent.
    let mut alice_config = default_config("alice");
    alice_config.reliability.ack.default_timeout_ms = 50;
    alice_config.reliability.retry.initial_delay_ms = 10;

    let mut net = Neighborhood::new(&["bob", "carol"]);
    net.add_node("alice", alice_config);
    let swallowing = MockTransport::new(TransportType::WiFiDirect);
    swallowing.start().unwrap();
    net.node("alice")
        .transport_manager_mut()
        .add_transport(TransportType::WiFiDirect, Box::new(swallowing.clone()));
    net.link("bob", "carol");

    let msg_id = net.send("alice", "carol", "past the swallowing carrier");
    assert!(
        swallowing
            .sent_messages()
            .iter()
            .any(|m| m.id.as_str() == msg_id),
        "the first attempt should have been swallowed"
    );
    assert_eq!(net.transmission_count(&msg_id), 0, "alice was alone");

    net.link("alice", "bob");
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        net.step();
        if !net.inbox("carol").is_empty() {
            break;
        }
    }

    assert_eq!(
        net.inbox("carol"),
        vec!["past the swallowing carrier".to_string()],
        "the swallowed resend must also be handed to bob"
    );
}

#[test]
fn a_flush_a_carrier_swallows_is_still_handed_to_the_neighbors() {
    // The flush counterpart of the case above: the carrier that comes up and
    // triggers the flush is one that takes any recipient, so the flushed
    // message "succeeds" into it. Bob, who can reach carol, must still be
    // offered it.
    let mut net = Neighborhood::new(&["alice", "bob", "carol"]);
    net.link("bob", "carol");

    let msg_id = net.send("alice", "carol", "flushed past the swallowing carrier");
    assert_eq!(net.transmission_count(&msg_id), 0, "alice was alone");

    let swallowing = MockTransport::new(TransportType::WiFiDirect);
    swallowing.start().unwrap();
    net.node("alice")
        .transport_manager_mut()
        .add_transport(TransportType::WiFiDirect, Box::new(swallowing.clone()));
    net.link("alice", "bob");
    net.node("alice").flush_outbox_all();
    assert!(
        swallowing
            .sent_messages()
            .iter()
            .any(|m| m.id.as_str() == msg_id),
        "the flush should have gone to the carrier that accepts anyone"
    );
    for _ in 0..6 {
        net.step();
    }

    assert_eq!(
        net.inbox("carol"),
        vec!["flushed past the swallowing carrier".to_string()],
        "the swallowed flush must also be handed to bob"
    );
}

#[test]
fn a_resend_over_a_live_link_is_not_also_handed_to_the_mesh() {
    // The cost side of the resend offer. Carol is in range and her first
    // acknowledgement is lost, so alice resends over the link she still has.
    // That resend reaches carol; offering it to the mesh as well would put a
    // second copy on the air for every ordinary retransmission.
    let mut alice_config = default_config("alice");
    alice_config.reliability.ack.default_timeout_ms = 50;
    alice_config.reliability.retry.initial_delay_ms = 10;

    let mut net = Neighborhood::new(&["bob", "carol"]);
    net.add_node("alice", alice_config);
    net.link("alice", "bob");
    net.link("alice", "carol");

    let delivered = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&delivered);
    net.node("alice").on_event(move |event| {
        if let Event::MessageDelivered { message_id, .. } = event {
            seen.lock().unwrap().push(message_id);
        }
    });

    let msg_id = net.send("alice", "carol", "say again");
    net.step();
    assert_eq!(net.inbox("carol"), vec!["say again".to_string()]);
    // Carol's acknowledgement is lost on the way back.
    net.node("carol").process().unwrap();
    net.radios["carol"].clear_sent_messages();
    net.radios["carol"].clear_peer_sends();

    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        net.step();
        if !delivered.lock().unwrap().is_empty() {
            break;
        }
    }

    assert_eq!(*delivered.lock().unwrap(), vec![msg_id.clone()]);
    assert_eq!(
        net.deliveries_to("carol", &msg_id),
        2,
        "the first copy and one resend, nothing extra"
    );
    assert_eq!(net.deliveries_to("bob", &msg_id), 0);
}
