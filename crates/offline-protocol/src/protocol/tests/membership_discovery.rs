//! Service discovery on configured nodes (ADR 0037, "Service discovery",
//! step 7c), over the real send, relay and receive paths of a line of
//! nodes with a mock BLE radio each.
//!
//! What is pinned: two members several hops apart find each other's service
//! through nodes of other applications, which carry the query and the answer
//! and learn nothing from them; a query or an answer a member cannot prove is
//! carried and never matched or reported; and a forged answer to a member is
//! not even acknowledged.

use super::*;
use crate::constants::ACK_FOR_KEY;
use offline_protocol_core::{MessagePriority, ServiceDescriptor, ServiceId};
use offline_protocol_services::{
    DISCOVERY_SEALED_SERVICE_ID, SVC_DISCOVER_QUERY, SVC_DISCOVER_RESPONSE,
};
use std::collections::HashMap;

const SERVICE: &str = "weather";

/// A node whose forwarding holds nothing back, so a line settles by
/// stepping. `key` configures the publisher key; `certified` also gives the
/// node its own certificate under it.
fn relay_node(label: &str, key: Option<&SigningKey>, certified: bool) -> Node {
    let mut node = Node::launch_configured(
        label,
        key.map(|k| k.verifying_key().to_bytes()),
        false,
        |config| {
            config.mesh_relay.jitter_min = std::time::Duration::from_millis(0);
            config.mesh_relay.jitter_max = std::time::Duration::from_millis(0);
        },
    );
    if let (Some(key), true) = (key, certified) {
        let cert = certificate(key, &node.address);
        node.protocol
            .set_app_membership(&cert.to_bytes())
            .expect("the node's own certificate");
    }
    node
}

fn provide(node: &mut Node) {
    node.protocol
        .register_service(ServiceDescriptor {
            service_id: ServiceId::new(SERVICE).expect("a service id"),
            version: "1.0".to_string(),
            capabilities: HashMap::new(),
        })
        .expect("registered");
}

/// Nodes in a line, each in range of the next only.
struct Line {
    nodes: Vec<Node>,
    events: Vec<Arc<Mutex<Vec<Event>>>>,
    /// Every frame that crossed a link, as `(from, to, frame)`.
    frames: Vec<(String, String, Message)>,
}

impl Line {
    fn new(mut nodes: Vec<Node>) -> Self {
        let events = nodes.iter_mut().map(record_events).collect();
        for i in 0..nodes.len().saturating_sub(1) {
            let (left, right) = nodes.split_at_mut(i + 1);
            left[i].link(&right[0]);
            right[0].link(&left[i]);
        }
        let mut line = Self {
            nodes,
            events,
            frames: Vec::new(),
        };
        line.run();
        line.frames.clear();
        line
    }

    fn index_of(&self, address: &str) -> usize {
        self.nodes
            .iter()
            .position(|n| n.address == address)
            .unwrap_or_else(|| panic!("{address} is not on the line"))
    }

    /// One round: every node processes what it holds, then what each put
    /// on its radio crosses to the neighbour it was handed to.
    fn step(&mut self) -> usize {
        for node in &mut self.nodes {
            while node.protocol.receive_message().is_some() {}
            node.protocol.process().expect("process");
        }
        let mut moved = 0;
        for i in 0..self.nodes.len() {
            let radio = &self.nodes[i].transport;
            let mut handed: Vec<(String, Message)> = radio
                .sent_messages()
                .into_iter()
                .map(|m| (m.recipient.as_str().to_string(), m))
                .collect();
            handed.extend(radio.peer_sends());
            radio.clear_sent_messages();
            radio.clear_peer_sends();
            let from = self.nodes[i].address.clone();
            for (to, message) in handed {
                let j = self.index_of(&to);
                assert_eq!(
                    i.abs_diff(j),
                    1,
                    "{from} handed a frame to {to}, out of range"
                );
                self.frames
                    .push((from.clone(), to.clone(), message.clone()));
                self.nodes[j]
                    .transport
                    .queue_message_from(message, from.clone());
                moved += 1;
            }
        }
        moved
    }

    fn run(&mut self) {
        for _ in 0..60 {
            if self.step() == 0 {
                return;
            }
        }
        panic!("the line never went quiet");
    }

    /// The `ServiceDiscovered` events node `i` emitted, as (provider, service).
    fn discovered(&self, i: usize) -> Vec<(String, String)> {
        self.events[i]
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::ServiceDiscovered {
                    provider_peer_id,
                    service_id,
                    ..
                } => Some((provider_peer_id.clone(), service_id.clone())),
                _ => None,
            })
            .collect()
    }

    fn discovery_frames(&self) -> Vec<&(String, String, Message)> {
        self.frames
            .iter()
            .filter(|(_, _, m)| m.content.starts_with("__SVC_DISC"))
            .collect()
    }

    /// Discovery responses written by `node` (its own answers, not frames it
    /// relayed for someone else).
    fn answers_by(&self, i: usize) -> usize {
        let address = &self.nodes[i].address;
        let ids: std::collections::HashSet<String> = self
            .frames
            .iter()
            .filter(|(_, _, m)| {
                m.content.starts_with(SVC_DISCOVER_RESPONSE) && m.sender.as_str() == address
            })
            .map(|(_, _, m)| m.id.as_str())
            .collect();
        ids.len()
    }
}

/// alice and bob are members three hops apart. Between them stand a member
/// of another application and an unconfigured node, both offering the same
/// service id. The query and the answer cross both; neither matches,
/// answers or reports, and neither ever sees the service id or the provider.
#[test]
fn two_members_three_hops_apart_discover_each_others_service() {
    let key = publisher(1);
    let alice = relay_node("alice", Some(&key), true);
    let mut xavier = relay_node("xavier", Some(&publisher(2)), true);
    let mut yara = relay_node("yara", None, false);
    let mut bob = relay_node("bob", Some(&key), true);
    provide(&mut xavier);
    provide(&mut yara);
    provide(&mut bob);
    let bob_address = bob.address.clone();
    let mut line = Line::new(vec![alice, xavier, yara, bob]);

    line.nodes[0]
        .protocol
        .discover_services(Some(SERVICE))
        .expect("a query");
    line.run();

    assert_eq!(
        line.discovered(0),
        vec![(bob_address.clone(), SERVICE.to_string())]
    );
    for i in 1..4 {
        assert!(
            line.discovered(i).is_empty(),
            "node {i} reported a discovery"
        );
    }
    assert_eq!(
        line.answers_by(1),
        0,
        "another application's member answered"
    );
    assert_eq!(
        line.answers_by(2),
        0,
        "an unconfigured node answered the sentinel"
    );
    assert_eq!(line.answers_by(3), 1);

    let crossed = line.discovery_frames();
    assert!(
        crossed
            .iter()
            .any(|(from, to, m)| m.content.starts_with(SVC_DISCOVER_QUERY)
                && from == &line.nodes[2].address
                && to == &bob_address),
        "the query crossed both strangers to reach bob"
    );
    for (from, to, frame) in crossed {
        assert!(
            !frame.content.contains(SERVICE),
            "{from} -> {to} carried the service id in the clear"
        );
        assert!(
            frame.content.contains(DISCOVERY_SEALED_SERVICE_ID),
            "{from} -> {to} carried no sentinel"
        );
        if frame.content.starts_with(SVC_DISCOVER_RESPONSE) {
            assert_eq!(frame.sender.as_str(), bob_address);
            assert_eq!(frame.recipient.as_str(), line.nodes[0].address);
        }
    }

    // Acknowledgements are carriage here too. The other application's
    // member acknowledges alice's query although it admits nobody on the
    // line, and alice acknowledges bob's proven answer although bob is
    // admitted by nothing else, so neither is resent to its expiry.
    let acked = |by: &str, to: &str, prefix: &str| {
        let ids: Vec<String> = line
            .frames
            .iter()
            .filter(|(_, _, m)| m.content.starts_with(prefix) && m.recipient.as_str() == by)
            .map(|(_, _, m)| m.id.as_str())
            .collect();
        line.frames.iter().any(|(_, _, m)| {
            m.sender.as_str() == by
                && m.recipient.as_str() == to
                && m.metadata
                    .get(ACK_FOR_KEY)
                    .is_some_and(|id| ids.contains(id))
        })
    };
    let (alice_address, xavier_address) =
        (line.nodes[0].address.clone(), line.nodes[1].address.clone());
    assert!(acked(&xavier_address, &alice_address, SVC_DISCOVER_QUERY));
    assert!(acked(&alice_address, &bob_address, SVC_DISCOVER_RESPONSE));
}

/// A node holding the application's key but no certificate asks: the member
/// next to it carries the query on and does not answer, and nothing is
/// reported anywhere.
#[test]
fn a_strangers_query_is_carried_but_never_answered() {
    let key = publisher(1);
    let stranger = relay_node("sam", Some(&key), false);
    let mut member = relay_node("mia", Some(&key), true);
    let mut far = relay_node("nora", Some(&key), true);
    provide(&mut member);
    provide(&mut far);
    let mut line = Line::new(vec![stranger, member, far]);

    line.nodes[0]
        .protocol
        .discover_services(Some(SERVICE))
        .expect("a query");
    line.run();

    let (mia, nora) = (line.nodes[1].address.clone(), line.nodes[2].address.clone());
    assert!(
        line.discovery_frames().iter().any(|(from, to, m)| m
            .content
            .starts_with(SVC_DISCOVER_QUERY)
            && from == &mia
            && to == &nora),
        "the member forwarded the stranger's query"
    );
    assert_eq!(line.answers_by(1), 0);
    assert_eq!(line.answers_by(2), 0);
    for i in 0..3 {
        assert!(line.discovered(i).is_empty());
    }
}

/// A forged discovery answer addressed to a member, from a sender it has not
/// admitted: no event, and not even an acknowledgement goes back, since an
/// answer ending at this node is end to end.
#[test]
fn a_forged_answer_to_a_member_is_not_acknowledged() {
    let key = publisher(1);
    let alice = relay_node("alice", Some(&key), true);
    let eve = relay_node("eve", None, false);
    let mut line = Line::new(vec![alice, eve]);
    let (alice_address, eve_address) =
        (line.nodes[0].address.clone(), line.nodes[1].address.clone());

    let forged = format!(
        "{SVC_DISCOVER_RESPONSE}{}",
        serde_json::json!({
            "query_id": "q",
            "service_id": SERVICE,
            "version": "1",
            "provider_peer_id": eve_address,
            "capabilities": {},
            "hop_count": 0,
            "originator": alice_address,
        })
    );
    line.nodes[1]
        .protocol
        .send_internal_message(&alice_address, forged, MessagePriority::Medium)
        .expect("sent");
    line.run();

    assert!(line.discovered(0).is_empty());
    assert!(
        !line
            .frames
            .iter()
            .any(|(from, _, m)| from == &alice_address && m.metadata.contains_key(ACK_FOR_KEY)),
        "the member acknowledged a stranger's forged answer"
    );
}

/// The same forged answer to an unconfigured node is reported and
/// acknowledged, as before this release: invariant 5.
#[test]
fn an_unconfigured_originator_reports_an_answer_as_before() {
    let alice = relay_node("alice", None, false);
    let eve = relay_node("eve", None, false);
    let mut line = Line::new(vec![alice, eve]);
    let (alice_address, eve_address) =
        (line.nodes[0].address.clone(), line.nodes[1].address.clone());

    let answer = format!(
        "{SVC_DISCOVER_RESPONSE}{}",
        serde_json::json!({
            "query_id": "q",
            "service_id": SERVICE,
            "version": "1",
            "provider_peer_id": eve_address,
            "capabilities": {},
            "hop_count": 0,
            "originator": alice_address,
        })
    );
    line.nodes[1]
        .protocol
        .send_internal_message(&alice_address, answer, MessagePriority::Medium)
        .expect("sent");
    line.run();

    assert_eq!(line.discovered(0), vec![(eve_address, SERVICE.to_string())]);
    assert!(line
        .frames
        .iter()
        .any(|(from, _, m)| from == &alice_address && m.metadata.contains_key(ACK_FOR_KEY)));
}
