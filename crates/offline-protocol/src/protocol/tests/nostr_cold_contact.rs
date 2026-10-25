//! Cold first contact over Nostr, between two devices that share nothing but
//! a relay (issue #633).
//!
//! alice holds bob's address and nothing else: no link, no prior key-package
//! exchange, no session. Encryption is required, so nothing she sends can
//! leave until a session exists, and the only way one can form is for her to
//! fetch the key package bob published. What is pinned is that the engine
//! asks for it by itself: the host half here does only what every binding's
//! Nostr driver does (move events, answer queries, report the relay live),
//! and asks for nothing.
//!
//! The relay is the smallest one that behaves like a real relay for what the
//! transport relies on: it stores what is not ephemeral, replaces an
//! addressable event by `(kind, pubkey, d)`, filters on `kinds`, `since`,
//! `limit` and any `#` tag, answers a query with what it holds, and forwards
//! a new event to every live subscription it matches.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::Value;

use offline_protocol_transport::nostr::{NostrTransport, ResolvedRecord};
use offline_protocol_transport::{Transport, TransportStatus, TransportType};

use crate::mls::InMemoryStorage;
use crate::protocol::tests::create_test_config_for_user;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::test_identity::id;
use crate::{Event, ProtocolConfig};

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
    let found = event["tags"].as_array()?.iter().find(|t| t[0] == name)?;
    found[1].as_str().map(str::to_string)
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

struct Node {
    protocol: OfflineProtocol,
    address: String,
    events: Arc<Mutex<Vec<Event>>>,
    inbox: Vec<String>,
    /// The standing inbox subscription's filter.
    subscription: Value,
    /// Resolution queries this node issued.
    queries: usize,
}

fn strict_config(label: &str) -> ProtocolConfig {
    let mut config = create_test_config_for_user(label);
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.auto_key_exchange = true;
    config.encryption.store_pending = true;
    config.transport.nostr_enabled = true;
    config.transport.nostr_cold_contact_enabled = true;
    config
}

impl Node {
    fn new(label: &str) -> Self {
        let mut protocol = OfflineProtocol::new(strict_config(label)).expect("protocol");
        protocol
            .initialize_mls(
                crate::test_identity::seeded_storage(label),
                Arc::new(TestProtocolStateStorage {
                    storage: Arc::new(InMemoryStorage::new()),
                }),
            )
            .expect("initialize_mls");
        // Built on the derived address, as every binding builds it.
        let nostr = NostrTransport::new(id(label)).expect("nostr transport");
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::Nostr, Box::new(nostr));
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        protocol.on_event(move |event| sink.lock().unwrap().push(event));
        protocol.start().expect("start");
        let mut node = Self {
            protocol,
            address: id(label),
            events,
            inbox: Vec::new(),
            subscription: Value::Null,
            queries: 0,
        };
        // What a driver does when its first relay comes up.
        node.transport()
            .on_status_changed(TransportStatus::Available);
        node.protocol.flush_outbox_all();
        let req: Value = serde_json::from_str(
            &node
                .nostr(|nostr| nostr.create_subscription("inbox"))
                .expect("subscription"),
        )
        .expect("req json");
        node.subscription = req[2].clone();
        node
    }

    fn transport(&self) -> Arc<dyn Transport> {
        self.protocol
            .transport_manager()
            .get_transport(TransportType::Nostr)
            .expect("nostr slot")
    }

    fn nostr<T>(&self, f: impl FnOnce(&NostrTransport) -> T) -> T {
        let transport = self.transport();
        f(transport
            .as_any()
            .downcast_ref::<NostrTransport>()
            .expect("nostr transport"))
    }

    /// The receive path every binding runs on an inbox event: unseal, hand
    /// the bytes to the transport, drain, then the reachability notice under
    /// the protocol sender.
    fn receive(&mut self, event: &Value) {
        let sender = event["pubkey"].as_str().expect("pubkey");
        let content = event["content"].as_str().expect("content");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(content)
            .unwrap_or_else(|_| content.as_bytes().to_vec());
        let data = self.nostr(|nostr| nostr.unseal_event_payload(sender, &raw).into_owned());
        let transport = self.transport();
        let real_sender = transport
            .deserialize_message(&data)
            .ok()
            .map(|message| message.sender.as_str().to_string());
        transport.on_data_received(data).expect("on_data_received");
        self.drain();
        if let Some(peer) = real_sender {
            self.protocol.on_neighbor_discovered(&peer);
        }
    }

    fn drain(&mut self) {
        while let Some(message) = self.protocol.receive_message() {
            self.inbox.push(message.content);
        }
    }

    fn delivered(&self, message_id: &str) -> bool {
        self.events.lock().unwrap().iter().any(
            |event| matches!(event, Event::MessageDelivered { message_id: id, .. } if id == message_id),
        )
    }
}

/// One round: each device publishes what it holds and issues its queries,
/// the relay answers and forwards, then every device runs its tick.
fn step(relay: &mut Relay, nodes: &mut [Node]) {
    for i in 0..nodes.len() {
        while let Some(signed) = nodes[i]
            .nostr(|nostr| nostr.get_next_signed_event())
            .expect("sign")
        {
            let sent: Value = serde_json::from_str(&signed.event_json).expect("event json");
            let event = sent[1].clone();
            relay.publish(&event);
            nodes[i]
                .protocol
                .on_transport_send_confirmed(&signed.message_id)
                .expect("confirm");
            nodes[i].transport().confirm_sent(&signed.message_id);
            for (j, other) in nodes.iter_mut().enumerate() {
                if j != i && matches(&other.subscription, &event) {
                    other.receive(&event);
                }
            }
        }
        while let Some(query) = nodes[i].nostr(|nostr| nostr.next_query()).expect("query") {
            nodes[i].queries += 1;
            let req: Value = serde_json::from_str(&query.req_json).expect("req json");
            for event in relay.matching(&req[2]) {
                let opened = nodes[i]
                    .nostr(|nostr| nostr.open_query_event(&query.query_id, &event.to_string()))
                    .expect("open");
                if let Some(ResolvedRecord::KeyPackage { plaintext, .. }) = opened {
                    nodes[i]
                        .protocol
                        .handle_resolved_key_package(&plaintext)
                        .expect("resolved key package");
                    nodes[i].drain();
                }
            }
            nodes[i].nostr(|nostr| nostr.complete_query(&query.query_id));
        }
    }
    for node in nodes.iter_mut() {
        node.drain();
        // The reconciliation scan is throttled to once every two seconds;
        // the test steps far faster than that.
        node.protocol.last_reconciliation_at = None;
        node.protocol.process().expect("process");
    }
}

#[test]
fn a_device_known_only_by_its_address_is_reached_over_nostr() {
    let mut relay = Relay::default();
    let mut nodes = vec![Node::new("alice"), Node::new("bob")];
    // bob's key packages reach the relay before alice knows he exists.
    for _ in 0..3 {
        step(&mut relay, &mut nodes);
    }
    assert!(
        relay.stored.iter().any(|event| event["kind"] == 30443),
        "precondition: bob's key packages stand on the relay"
    );
    assert_eq!(
        nodes[0].queries, 0,
        "precondition: alice has asked about no one"
    );

    let bob = nodes[1].address.clone();
    let message_id = nodes[0]
        .protocol
        .send_message(&bob, "found you through a relay", None, None::<String>)
        .expect("queued until a session exists")
        .as_str()
        .to_string();

    for _ in 0..40 {
        step(&mut relay, &mut nodes);
        if nodes[0].delivered(&message_id) {
            break;
        }
    }
    assert_eq!(
        nodes[1]
            .inbox
            .iter()
            .filter(|content| content.as_str() == "found you through a relay")
            .count(),
        1,
        "bob receives the message from a device whose only path to him is a relay, once"
    );
    assert!(
        nodes[0].delivered(&message_id),
        "and alice holds bob's acknowledgement"
    );
    assert!(
        nodes[0].queries >= 1,
        "the session formed from the key package alice fetched"
    );
}
