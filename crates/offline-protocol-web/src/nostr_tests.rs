//! Two engines that share nothing but a relay, on one host.
//!
//! The relay here is the smallest one that behaves like a real relay for
//! everything the transport relies on: it stores what is not ephemeral,
//! replaces an addressable event by `(kind, pubkey, d)`, filters on `kinds`,
//! `since`, `limit` and any `#` tag, answers a query with what it holds, and
//! forwards a new event to every live subscription it matches. The worker's
//! part (sockets, `OK` correlation, end-of-stored-events per relay) is the
//! browser page's to test; this is the engine's.

use std::time::Duration;

use serde_json::Value;

use crate::node::{Node, NodeOptions};
use crate::nostr_host::NostrOptions;

#[derive(Default)]
struct Relay {
    stored: Vec<Value>,
}

impl Relay {
    fn publish(&mut self, event: &Value) {
        let kind = event["kind"].as_u64().unwrap_or(0);
        if (20_000..30_000).contains(&kind) {
            return;
        }
        if (30_000..40_000).contains(&kind) {
            let d = tag(event, "d");
            self.stored.retain(|held| {
                !(held["kind"] == event["kind"]
                    && held["pubkey"] == event["pubkey"]
                    && tag(held, "d") == d)
            });
        }
        self.stored.push(event.clone());
    }

    fn matching(&self, filter: &Value) -> Vec<Value> {
        let mut found: Vec<Value> = self
            .stored
            .iter()
            .filter(|event| matches(filter, event))
            .cloned()
            .collect();
        found.sort_by_key(|event| std::cmp::Reverse(event["created_at"].as_i64()));
        if let Some(limit) = filter["limit"].as_u64() {
            found.truncate(limit as usize);
        }
        found
    }
}

fn tag(event: &Value, name: &str) -> Option<String> {
    event["tags"]
        .as_array()?
        .iter()
        .find_map(|t| (t[0] == name).then(|| t[1].as_str().map(str::to_string))?)
}

fn matches(filter: &Value, event: &Value) -> bool {
    if let Some(kinds) = filter["kinds"].as_array() {
        if !kinds.contains(&event["kind"]) {
            return false;
        }
    }
    if let (Some(since), Some(at)) = (filter["since"].as_i64(), event["created_at"].as_i64()) {
        if at < since {
            return false;
        }
    }
    for (key, wanted) in filter.as_object().into_iter().flatten() {
        let Some(name) = key.strip_prefix('#') else {
            continue;
        };
        let held: Vec<&Value> = event["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t[0] == name)
            .map(|t| &t[1])
            .collect();
        let wanted = wanted.as_array().cloned().unwrap_or_default();
        if !wanted.iter().any(|w| held.contains(&w)) {
            return false;
        }
    }
    true
}

struct Peer {
    node: Node,
    address: String,
    events: Vec<Value>,
    /// The standing subscription's filter, once the peer is "connected".
    subscription: Option<Value>,
}

const SUBSCRIPTION: &str = "inbox";

fn peer(profile: &str, username_discovery: bool) -> Peer {
    let options = NodeOptions {
        nostr: Some(NostrOptions {
            cold_contact: true,
            username_discovery,
        }),
    };
    let mut node = Node::with_options("web-nostr-test", profile, options).expect("node");
    let address = node.initialize().expect("initialize");
    node.start().expect("start");
    Peer {
        node,
        address,
        events: Vec::new(),
        subscription: None,
    }
}

/// What a worker does when its first relay comes up: report it live, then
/// subscribe, receiving what the relay already holds for this device.
fn connect(relay: &Relay, p: &mut Peer) {
    p.node.nostr_status_changed(true).expect("status");
    let req: Value =
        serde_json::from_str(&p.node.nostr_subscription(SUBSCRIPTION).expect("req")).expect("json");
    assert_eq!(req[0], "REQ");
    assert_eq!(req[1], SUBSCRIPTION);
    let filter = req[2].clone();
    for event in relay.matching(&filter) {
        deliver(p, SUBSCRIPTION, &event);
    }
    p.subscription = Some(filter);
}

fn deliver(p: &mut Peer, subscription: &str, event: &Value) {
    let message = serde_json::json!(["EVENT", subscription, event]).to_string();
    p.node.nostr_relay_event(&message).expect("relay event");
}

/// One pass of what the worker and the relay do between them, then a tick.
fn step(relay: &mut Relay, peers: &mut [&mut Peer]) {
    for i in 0..peers.len() {
        while let Some(out) = peers[i].node.nostr_next_event() {
            let sent: Value = serde_json::from_str(&out.event_json).expect("event json");
            assert_eq!(sent[0], "EVENT");
            let event = sent[1].clone();
            assert_eq!(event["id"], out.event_id.as_str(), "OK names this id");
            relay.publish(&event);
            peers[i].node.nostr_confirm_sent(&out.message_id);
            for other in peers.iter_mut() {
                let live = other
                    .subscription
                    .as_ref()
                    .is_some_and(|filter| matches(filter, &event));
                if live {
                    deliver(other, SUBSCRIPTION, &event);
                }
            }
        }
        while let Some(query) = peers[i].node.nostr_next_query() {
            let req: Value = serde_json::from_str(&query.req_json).expect("req json");
            assert_eq!(req[1], query.query_id.as_str());
            for event in relay.matching(&req[2]) {
                deliver(peers[i], &query.query_id, &event);
            }
            peers[i].node.nostr_query_completed(&query.query_id);
        }
    }
    for p in peers.iter_mut() {
        p.node.process().expect("process");
        while let Some(json) = p.node.poll_event() {
            p.events
                .push(serde_json::from_str(&json).expect("event json"));
        }
    }
}

fn run_until(
    relay: &mut Relay,
    a: &mut Peer,
    b: &mut Peer,
    mut done: impl FnMut(&Peer, &Peer) -> bool,
) -> bool {
    for _ in 0..400 {
        step(relay, &mut [&mut *a, &mut *b]);
        if done(a, b) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

/// A few passes, so both peers' publications reach the relay before a test
/// looks anything up: a query is answered with what the relay holds when it
/// arrives, as a real relay answers it.
fn settle(relay: &mut Relay, a: &mut Peer, b: &mut Peer) {
    for _ in 0..4 {
        step(relay, &mut [&mut *a, &mut *b]);
    }
}

#[test]
fn nothing_is_published_until_a_relay_is_live() {
    let mut a = peer("alice", false);
    a.node.process().expect("process");
    assert!(
        a.node.nostr_next_event().is_none(),
        "a key package went out with no relay to carry it"
    );
    a.node.nostr_status_changed(true).expect("status");
    assert!(
        a.node.nostr_next_event().is_some(),
        "a live relay receives the key-package publications"
    );
}

#[test]
fn a_device_known_only_by_its_address_is_reached_over_nostr() {
    let mut relay = Relay::default();
    let (mut a, mut b) = (peer("alice", false), peer("bob", false));
    connect(&relay, &mut a);
    connect(&relay, &mut b);
    // Bob's key packages reach the relay before Alice knows he exists.
    settle(&mut relay, &mut a, &mut b);
    let b_addr = b.address.clone();
    a.node
        .send_message(&b_addr, "found you through a relay", false)
        .expect("send");
    let a_addr = a.address.clone();
    assert!(
        run_until(&mut relay, &mut a, &mut b, |_, b| {
            b.events.iter().any(|e| {
                e["type"] == "message_received"
                    && e["content"] == "found you through a relay"
                    && e["encrypted"] == true
                    && e["sender"] == a_addr.as_str()
            })
        }),
        "the message never arrived; Alice saw {:?}",
        a.events
            .iter()
            .map(|e| e["type"].clone())
            .collect::<Vec<_>>()
    );
    assert!(
        b.events.iter().any(|e| e["type"] == "neighbor_discovered"
            && e["peer_id"] == a_addr.as_str()
            && e["transport"] == "Nostr"),
        "the sender is announced under the address its message proved"
    );
}

#[test]
fn a_username_resolves_to_every_device_claiming_it() {
    let mut relay = Relay::default();
    let (mut a, mut b) = (peer("alice", true), peer("bobby", true));
    connect(&relay, &mut a);
    connect(&relay, &mut b);
    settle(&mut relay, &mut a, &mut b);
    assert!(a.node.resolve_username("bobby").expect("resolve"));
    let b_addr = b.address.clone();
    assert!(run_until(&mut relay, &mut a, &mut b, |a, _| {
        a.events.iter().any(|e| {
            e["type"] == "username_resolved"
                && e["username"] == "bobby"
                && e["claims"]
                    .as_array()
                    .is_some_and(|claims| claims.iter().any(|c| c["address"] == b_addr.as_str()))
        })
    }));
}

#[test]
fn a_name_nobody_claims_still_answers() {
    let mut relay = Relay::default();
    let (mut a, mut b) = (peer("alice", true), peer("bob", true));
    connect(&relay, &mut a);
    connect(&relay, &mut b);
    assert!(a.node.resolve_username("nobody-here").expect("resolve"));
    assert!(run_until(&mut relay, &mut a, &mut b, |a, _| {
        a.events.iter().any(|e| {
            e["type"] == "username_resolved"
                && e["username"] == "nobody-here"
                && e["claims"].as_array().is_some_and(Vec::is_empty)
        })
    }));
}

#[test]
fn an_invite_opens_to_the_address_that_made_it() {
    let a = peer("alice", false);
    let blob = a.node.create_invite(Some("Alice"), true).expect("invite");
    let invite = offline_protocol_mls::invite::parse_invite(&blob).expect("parse");
    assert_eq!(invite.address.to_string(), a.address);
    assert_eq!(invite.petname.as_deref(), Some("Alice"));
    assert!(invite.signed);
}

#[test]
fn a_session_with_a_device_met_only_by_address_starts_pending() {
    use crate::node::SessionStart;
    let mut relay = Relay::default();
    let (mut a, mut b) = (peer("alice", false), peer("bob", false));
    connect(&relay, &mut a);
    connect(&relay, &mut b);
    settle(&mut relay, &mut a, &mut b);
    let b_addr = b.address.clone();
    assert_eq!(
        a.node.establish_secure_session(&b_addr).expect("establish"),
        SessionStart::Pending
    );
    assert!(run_until(&mut relay, &mut a, &mut b, |a, _| {
        a.events
            .iter()
            .any(|e| e["type"] == "secure_session_established" && e["peer_id"] == b_addr.as_str())
    }));
    assert_eq!(
        a.node.establish_secure_session(&b_addr).expect("establish"),
        SessionStart::Existing
    );
}

#[test]
fn a_node_without_relays_has_no_nostr_slot() {
    let mut node = Node::new("web-nostr-test", "plain").expect("node");
    node.initialize().expect("initialize");
    assert!(node.nostr_status_changed(true).is_err());
    assert!(node.nostr_subscription(SUBSCRIPTION).is_err());
    assert!(node.nostr_next_event().is_none());
}
