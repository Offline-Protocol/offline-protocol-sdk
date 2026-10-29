//! Service subscriptions between engines over a link (ADR 0035).
//!
//! What is pinned: a subscription made before any session waits for it and
//! then opens; no subscription frame ever crosses the link readable; the
//! three admission policies; an update in plaintext is dropped whoever signed
//! it; a `latest` value for a subscriber out of range waits in the outbox as
//! one superseding entry per key, so the subscriber comes back to the newest
//! value only; a resync hands the state back; a peer whose key package omits
//! entry 2 is refused at the call; unregistering a service ends what was
//! subscribed to it; a key filter and a topic pattern hold across the link,
//! with nothing they carry readable on it; and a queued subscription frame is
//! never written to the outbox's store.

use std::sync::{Arc, Mutex};

use offline_protocol_core::{Message, ServiceDescriptor, ServiceId};
use offline_protocol_services::{
    RulesFallback, SubscribeOptions, SubscriptionMode, SubscriptionPolicy, SubscriptionState,
    SVC_MESSAGE_PREFIX,
};
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use super::{create_test_config_for_user, signed_frame};
use crate::mls::{InMemoryStorage, MlsStorage};
use crate::protocol::types::storage_keys;
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::test_identity::id;
use crate::{Event, ProtocolConfig};

const SVC: &str = "fleet.v1";

struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    events: Arc<Mutex<Vec<Event>>>,
    /// Where the protocol state, the outbox included, is persisted.
    state: Arc<InMemoryStorage>,
}

impl Node {
    fn new(label: &str) -> Self {
        let mut config: ProtocolConfig = create_test_config_for_user(label);
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = true;
        config.encryption.store_pending = true;
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let state = Arc::new(InMemoryStorage::new());
        protocol
            .initialize_mls(
                crate::test_identity::seeded_storage(label),
                Arc::new(TestProtocolStateStorage {
                    storage: state.clone(),
                }),
            )
            .expect("initialize_mls");
        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
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
            state,
        }
    }

    fn register(&mut self, policy: SubscriptionPolicy) {
        self.protocol
            .register_service(ServiceDescriptor {
                service_id: ServiceId::new(SVC).unwrap(),
                version: "1.0".to_string(),
                capabilities: Default::default(),
            })
            .unwrap();
        self.protocol
            .set_service_subscription_policy(SVC, policy)
            .unwrap();
    }

    /// `(key, body, snapshot)` of every update received, then forgotten.
    fn take_updates(&self) -> Vec<(Option<String>, String, bool)> {
        let mut updates = Vec::new();
        self.events.lock().unwrap().retain(|event| match event {
            Event::ServiceUpdateReceived {
                key,
                body,
                snapshot,
                ..
            } => {
                updates.push((key.clone(), body.clone(), *snapshot));
                false
            }
            _ => true,
        });
        updates
    }

    /// `(role, state, reason)` of every state change, then forgotten.
    fn take_changes(&self) -> Vec<(String, String, String)> {
        let mut changes = Vec::new();
        self.events.lock().unwrap().retain(|event| match event {
            Event::ServiceSubscriptionChanged {
                role,
                state,
                reason,
                ..
            } => {
                changes.push((role.clone(), state.clone(), reason.clone()));
                false
            }
            _ => true,
        });
        changes
    }

    fn asked(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::ServiceSubscribeReceived { sub_id, .. } => Some(sub_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn state_of(&self, sub_id: &str) -> Option<SubscriptionState> {
        self.protocol
            .service_subscriptions()
            .into_iter()
            .find(|info| info.sub_id == sub_id)
            .map(|info| info.state)
    }

    fn outbox_to(&self, peer: &str) -> usize {
        self.protocol
            .outbox
            .values()
            .filter(|entry| entry.message.recipient.as_str() == peer)
            .count()
    }
}

/// Alice subscribes, Bob provides. Every frame that crosses is recorded.
struct Pair {
    alice: Node,
    bob: Node,
    carried: Vec<Message>,
    linked: bool,
}

impl Pair {
    fn new() -> Self {
        let mut pair = Self {
            alice: Node::new("alice"),
            bob: Node::new("bob"),
            carried: Vec::new(),
            linked: false,
        };
        pair.link();
        pair
    }

    fn link(&mut self) {
        let (a, b) = (self.alice.address.clone(), self.bob.address.clone());
        self.alice.transport.add_connected_peer(b.clone(), -55);
        self.bob.transport.add_connected_peer(a.clone(), -55);
        self.alice.protocol.on_neighbor_discovered(&b);
        self.bob.protocol.on_neighbor_discovered(&a);
        self.linked = true;
    }

    fn unlink(&mut self) {
        let (a, b) = (self.alice.address.clone(), self.bob.address.clone());
        self.alice.transport.remove_connected_peer(&b);
        self.bob.transport.remove_connected_peer(&a);
        self.linked = false;
    }

    fn step(&mut self) {
        for node in [&mut self.alice, &mut self.bob] {
            while node.protocol.receive_message().is_some() {}
            node.protocol.last_reconciliation_at = None;
            node.protocol.process().expect("process");
        }
        let from_alice = self.alice.transport.sent_messages();
        self.alice.transport.clear_sent_messages();
        let from_bob = self.bob.transport.sent_messages();
        self.bob.transport.clear_sent_messages();
        if !self.linked {
            return;
        }
        for frame in from_alice {
            self.carried.push(frame.clone());
            self.bob
                .transport
                .queue_message_from(frame, self.alice.address.clone());
        }
        for frame in from_bob {
            self.carried.push(frame.clone());
            self.alice
                .transport
                .queue_message_from(frame, self.bob.address.clone());
        }
    }

    fn run(&mut self, rounds: usize) {
        for _ in 0..rounds {
            self.step();
        }
    }

    fn sessions_confirmed(&self) -> bool {
        self.alice
            .protocol
            .confirmed_sessions
            .contains(&self.bob.address)
            && self
                .bob
                .protocol
                .confirmed_sessions
                .contains(&self.alice.address)
    }

    /// Alice subscribes to Bob's `topic` and the pair runs until it settles.
    fn subscribe(&mut self, topic: &str, mode: SubscriptionMode) -> String {
        let bob = self.bob.address.clone();
        let sub_id = self
            .alice
            .protocol
            .subscribe_service(&bob, SVC, topic, mode, "")
            .expect("subscribe");
        self.run(12);
        sub_id
    }
}

/// Nothing a carrier saw names the subscription family or carries a body.
fn assert_nothing_readable(carried: &[Message], secrets: &[&str]) {
    for frame in carried {
        assert!(
            !frame.content.starts_with(SVC_MESSAGE_PREFIX),
            "a service frame crossed in the clear: {}",
            &frame.content[..frame.content.len().min(40)]
        );
        for secret in secrets {
            assert!(
                !frame.content.contains(secret),
                "`{secret}` crossed in the clear"
            );
        }
    }
}

#[test]
fn a_subscription_made_before_any_session_opens_once_it_confirms() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    assert!(!pair.sessions_confirmed());
    let bob = pair.bob.address.clone();
    let sub_id = pair
        .alice
        .protocol
        .subscribe_service(&bob, SVC, "battery", SubscriptionMode::Latest, "")
        .unwrap();
    assert_eq!(
        pair.alice.state_of(&sub_id),
        Some(SubscriptionState::Pending)
    );
    pair.run(12);
    assert!(pair.sessions_confirmed());
    assert_eq!(
        pair.alice.state_of(&sub_id),
        Some(SubscriptionState::Active)
    );
    assert!(pair.alice.take_changes().contains(&(
        "consumer".into(),
        "active".into(),
        "accepted".into()
    )));

    let report = pair
        .bob
        .protocol
        .publish_service_update(SVC, "battery", Some("pct"), "battery-81")
        .unwrap();
    assert_eq!((report.subscribers, report.sent_live), (1, 1));
    pair.run(2);
    assert_eq!(
        pair.alice.take_updates(),
        vec![(Some("pct".to_string()), "battery-81".to_string(), false)]
    );
    assert_nothing_readable(&pair.carried, &["battery-81", "fleet.v1"]);
}

#[test]
fn restricted_asks_the_providers_application() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::Restricted);
    let sub_id = pair.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(pair.bob.asked(), vec![sub_id.clone()]);
    assert_eq!(
        pair.alice.state_of(&sub_id),
        Some(SubscriptionState::Pending)
    );
    pair.bob
        .protocol
        .accept_service_subscription(&sub_id)
        .unwrap();
    pair.run(3);
    assert_eq!(
        pair.alice.state_of(&sub_id),
        Some(SubscriptionState::Active)
    );
}

#[test]
fn rules_admit_a_listed_peer_and_refuse_the_rest() {
    let mut pair = Pair::new();
    let alice = pair.alice.address.clone();
    pair.bob.register(SubscriptionPolicy::Rules {
        allow_peers: vec![alice],
        allow_groups: Vec::new(),
        fallback: RulesFallback::Reject,
    });
    let sub_id = pair.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(
        pair.alice.state_of(&sub_id),
        Some(SubscriptionState::Active)
    );

    pair.bob
        .protocol
        .set_service_subscription_policy(
            SVC,
            SubscriptionPolicy::Rules {
                allow_peers: Vec::new(),
                allow_groups: Vec::new(),
                fallback: RulesFallback::Reject,
            },
        )
        .unwrap();
    let refused = pair.subscribe("position", SubscriptionMode::Latest);
    assert_eq!(pair.alice.state_of(&refused), None);
    assert!(pair.alice.take_changes().contains(&(
        "consumer".into(),
        "ended".into(),
        "policy".into()
    )));

    // The subscription admitted before the change goes through the policy
    // again on its next renewal (a resync carries the same frame), and ends.
    assert_eq!(
        pair.alice.state_of(&sub_id),
        Some(SubscriptionState::Active)
    );
    pair.alice
        .protocol
        .resync_service_subscription(&sub_id)
        .unwrap();
    pair.run(12);
    assert_eq!(pair.alice.state_of(&sub_id), None);
    assert!(pair.alice.take_changes().contains(&(
        "consumer".into(),
        "ended".into(),
        "policy".into()
    )));
    let report = pair
        .bob
        .protocol
        .publish_service_update(SVC, "battery", Some("pct"), "81")
        .unwrap();
    assert_eq!(
        report.subscribers, 0,
        "a refused peer is no longer streamed to"
    );
}

/// Invariant 1: a subscription frame that arrives as plaintext is dropped,
/// even signed by the provider itself.
#[test]
fn a_plaintext_update_is_dropped() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    let sub_id = pair.subscribe("battery", SubscriptionMode::Latest);
    pair.alice.take_updates();
    let forged = format!(
        "__SVC_UPD__{{\"sub_id\":\"{sub_id}\",\"stream\":\"x\",\"seq\":99,\"key\":\"pct\",\"body\":\"forged\"}}"
    );
    let frame = signed_frame(&pair.bob.address, &pair.alice.address, forged);
    pair.alice
        .transport
        .queue_message_from(frame, pair.bob.address.clone());
    while pair.alice.protocol.receive_message().is_some() {}
    assert!(pair.alice.take_updates().is_empty());
}

/// Invariant 5, and multi-hop `latest`: with the subscriber out of range,
/// each value replaces the one queued before it, so the subscriber comes
/// back to the newest value and only that.
#[test]
fn a_latest_value_for_an_absent_subscriber_supersedes_the_one_queued() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.subscribe("position", SubscriptionMode::Latest);
    pair.alice.take_updates();
    let alice = pair.alice.address.clone();

    pair.unlink();
    pair.run(2);
    let before = pair.bob.outbox_to(&alice);
    for (i, body) in ["pos-1", "pos-2", "pos-3"].iter().enumerate() {
        // Past the update interval, so none of them coalesces.
        std::thread::sleep(std::time::Duration::from_millis(60));
        let report = pair
            .bob
            .protocol
            .publish_service_update(SVC, "position", Some("xy"), body)
            .unwrap();
        assert_eq!(report.queued, 1, "publish {i} was not queued");
    }
    assert_eq!(
        pair.bob.outbox_to(&alice),
        before + 1,
        "more than one value per key waits for an absent subscriber"
    );

    pair.link();
    pair.run(6);
    let received = pair.alice.take_updates();
    assert_eq!(
        received.last(),
        Some(&(Some("xy".to_string()), "pos-3".to_string(), false))
    );
    assert!(!received
        .iter()
        .any(|(_, body, _)| body == "pos-1" || body == "pos-2"));
}

/// Nothing of a subscription outlives the process (ADR 0035, *Restart*). A
/// restored frame would name a stream or a `sub_id` that no longer exists,
/// and stored sealed it cannot be found at start-up, so it is never written:
/// the update waiting for an absent subscriber is in the outbox and not in
/// its store, while a direct message queued beside it is in both.
#[test]
fn a_queued_subscription_frame_is_never_written_to_the_outbox_store() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.subscribe("position", SubscriptionMode::Latest);
    let alice = pair.alice.address.clone();

    pair.unlink();
    pair.run(2);
    let report = pair
        .bob
        .protocol
        .publish_service_update(SVC, "position", Some("xy"), "pos")
        .unwrap();
    assert_eq!(report.queued, 1);
    let message = pair
        .bob
        .protocol
        .send_message(alice.clone(), "hello", None, None::<String>)
        .unwrap();

    let stored = pair.bob.state.list_keys(storage_keys::OUTBOX).unwrap();
    assert!(
        stored.contains(&message.as_str().to_string()),
        "the outbox is persisted, so its absence below is the rule"
    );
    let queued: Vec<String> = pair
        .bob
        .protocol
        .outbox
        .values()
        .filter(|entry| OfflineProtocol::is_subscription_outbox_entry(entry))
        .map(|entry| entry.message.id.as_str().to_string())
        .collect();
    assert!(!queued.is_empty(), "the update waits in the outbox");
    for id in &queued {
        assert!(!stored.contains(id), "a subscription frame was persisted");
    }

    // The mark is the entry's own, not read back from its re-seal
    // provenance: an entry that lost its provenance still stays unwritten.
    let marked: Vec<_> = pair
        .bob
        .protocol
        .outbox
        .values_mut()
        .filter(|entry| entry.subscription_frame)
        .map(|entry| {
            entry.reseal = None;
            entry.clone()
        })
        .collect();
    assert_eq!(marked.len(), queued.len());
    for entry in &marked {
        pair.bob.protocol.persist_outbox_entry(entry);
    }
    let stored = pair.bob.state.list_keys(storage_keys::OUTBOX).unwrap();
    for id in &queued {
        assert!(!stored.contains(id), "a subscription frame was persisted");
    }
}

#[test]
fn a_resync_hands_the_state_back() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.bob
        .protocol
        .publish_service_update(SVC, "battery", Some("pct"), "81")
        .unwrap();
    let sub_id = pair.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(pair.alice.take_updates().len(), 1);
    pair.alice
        .protocol
        .resync_service_subscription(&sub_id)
        .unwrap();
    pair.run(4);
    assert_eq!(
        pair.alice.take_updates(),
        vec![(Some("pct".to_string()), "81".to_string(), true)]
    );
}

#[test]
fn a_log_subscriber_is_sent_every_event_in_order() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.subscribe("doors", SubscriptionMode::Log);
    for event in ["opened", "closed", "opened again"] {
        pair.bob
            .protocol
            .publish_service_update(SVC, "doors", None, event)
            .unwrap();
    }
    pair.run(4);
    let bodies: Vec<String> = pair
        .alice
        .take_updates()
        .into_iter()
        .map(|(_, body, _)| body)
        .collect();
    assert_eq!(bodies, vec!["opened", "closed", "opened again"]);
}

#[test]
fn a_provider_that_does_not_advertise_subscriptions_is_refused_at_the_call() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    // A session from a request, so that no subscription frame has proved
    // anything about Bob on this device.
    let bob = pair.bob.address.clone();
    pair.alice
        .protocol
        .send_service_request(&bob, SVC, "ping", "{}")
        .unwrap();
    pair.run(12);
    assert!(pair.sessions_confirmed());
    pair.alice
        .protocol
        .record_svc_sub_advertisement(&bob, false);
    let refused =
        pair.alice
            .protocol
            .subscribe_service(&bob, SVC, "battery", SubscriptionMode::Latest, "");
    assert!(
        matches!(
            refused,
            Err(crate::Error::Service(
                offline_protocol_services::ServiceError::Unsupported(_)
            ))
        ),
        "{refused:?}"
    );
    assert!(pair.alice.protocol.service_subscriptions().is_empty());
}

#[test]
fn unregistering_the_service_ends_the_subscription_on_both_sides() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    let sub_id = pair.subscribe("battery", SubscriptionMode::Latest);
    pair.alice.take_changes();
    assert!(pair.bob.protocol.unregister_service(SVC).unwrap());
    pair.run(3);
    assert_eq!(pair.alice.state_of(&sub_id), None);
    assert!(pair.alice.take_changes().contains(&(
        "consumer".into(),
        "ended".into(),
        "service_unregistered".into()
    )));
    assert!(pair.bob.protocol.service_subscribers(Some(SVC)).is_empty());
}

#[test]
fn unsubscribing_ends_it_on_the_provider() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    let sub_id = pair.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(pair.bob.protocol.service_subscribers(Some(SVC)).len(), 1);
    assert!(pair.alice.protocol.unsubscribe_service(&sub_id).unwrap());
    pair.run(3);
    assert!(pair.bob.protocol.service_subscribers(Some(SVC)).is_empty());
}

/// `(topic, key, body)` of every update received, then forgotten.
fn take_topic_updates(node: &Node) -> Vec<(String, Option<String>, String)> {
    let mut updates = Vec::new();
    node.events.lock().unwrap().retain(|event| match event {
        Event::ServiceUpdateReceived {
            topic, key, body, ..
        } => {
            updates.push((topic.clone(), key.clone(), body.clone()));
            false
        }
        _ => true,
    });
    updates
}

#[test]
fn a_key_filter_keeps_other_keys_on_the_provider() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.bob
        .protocol
        .publish_service_update(SVC, "car", Some("battery"), "battery-80")
        .unwrap();
    pair.bob
        .protocol
        .publish_service_update(SVC, "car", Some("location"), "secret-location")
        .unwrap();
    let bob = pair.bob.address.clone();
    pair.alice
        .protocol
        .subscribe_service_with(
            &bob,
            SVC,
            "car",
            SubscribeOptions {
                keys: vec!["battery".into()],
                ..SubscribeOptions::new(SubscriptionMode::Latest)
            },
        )
        .unwrap();
    pair.run(12);
    assert_eq!(
        take_topic_updates(&pair.alice),
        vec![("car".into(), Some("battery".into()), "battery-80".into())]
    );

    let report = pair
        .bob
        .protocol
        .publish_service_update(SVC, "car", Some("location"), "secret-location-2")
        .unwrap();
    assert_eq!(report.subscribers, 0);
    pair.run(2);
    assert!(take_topic_updates(&pair.alice).is_empty());
    assert_nothing_readable(&pair.carried, &["battery-80", "fleet.v1"]);
}

#[test]
fn a_topic_pattern_follows_topics_made_after_it() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.bob
        .protocol
        .publish_service_update(SVC, "rooms/kitchen", Some("temp"), "kitchen-21")
        .unwrap();
    pair.subscribe("rooms/*", SubscriptionMode::Latest);
    assert_eq!(
        take_topic_updates(&pair.alice),
        vec![(
            "rooms/kitchen".into(),
            Some("temp".into()),
            "kitchen-21".into()
        )]
    );

    // Past the update interval the snapshot started.
    std::thread::sleep(std::time::Duration::from_millis(60));
    pair.bob
        .protocol
        .publish_service_update(SVC, "rooms/hall", Some("temp"), "hall-19")
        .unwrap();
    pair.bob
        .protocol
        .publish_service_update(SVC, "garden", Some("temp"), "garden-12")
        .unwrap();
    pair.run(2);
    assert_eq!(
        take_topic_updates(&pair.alice),
        vec![("rooms/hall".into(), Some("temp".into()), "hall-19".into())]
    );
    let info = pair.alice.protocol.service_subscriptions();
    assert_eq!(info[0].topic, "rooms/*");
    assert_eq!(info[0].topics, vec!["rooms/hall", "rooms/kitchen"]);
    assert_nothing_readable(&pair.carried, &["kitchen-21", "hall-19", "rooms/"]);
}

/// The proved set is a subset of the encryption-capable set, so it is bounded
/// by that set's cap: a sender outside it is not recorded.
#[test]
fn only_an_encryption_capable_sender_is_recorded_as_proved() {
    let mut pair = Pair::new();
    pair.subscribe("battery", SubscriptionMode::Latest);
    let alice = pair.alice.address.clone();
    assert_eq!(
        pair.bob.protocol.peer_routes_subscriptions(&alice),
        Some(true)
    );

    pair.bob.protocol.handle_subscription_frame(
        "stranger",
        "__SVC_UNSUB__{\"v\":2,\"sub_id\":\"x\",\"reason\":\"\"}",
    );
    assert_eq!(
        pair.bob.protocol.peer_routes_subscriptions("stranger"),
        None
    );
}

/// Bob makes a group and adds Alice to it as a plain member, with no role:
/// a role is recorded for the creator and for anyone given one, so a member
/// is in the roster and not in the roles. Returns the group id.
fn group_with_alice(pair: &Pair, name: &str) -> String {
    let bob = pair.bob.protocol.mls_manager_for_testing().read().unwrap();
    let group_id = bob.create_group(name).unwrap().group_id;
    let key_package = {
        let alice = pair
            .alice
            .protocol
            .mls_manager_for_testing()
            .read()
            .unwrap();
        alice.generate_key_package().unwrap()
    };
    let (welcome, _) = bob
        .add_group_member(
            &group_id,
            &pair.alice.address,
            &key_package.key_package_data,
        )
        .unwrap();
    {
        let alice = pair
            .alice
            .protocol
            .mls_manager_for_testing()
            .read()
            .unwrap();
        alice.join_group(&welcome).unwrap();
    }
    group_id.as_str().to_string()
}

/// `allow_groups` admits a member of a listed group by the group's roster on
/// the provider. Read from the roles instead, it admitted the creator and
/// anyone given a role, and refused every plain member.
#[test]
fn rules_admit_a_member_of_a_listed_group_and_refuse_a_non_member() {
    let mut pair = Pair::new();
    let with_alice = group_with_alice(&pair, "fleet");
    let without_alice = pair
        .bob
        .protocol
        .mls_manager_for_testing()
        .read()
        .unwrap()
        .create_group("office")
        .unwrap()
        .group_id
        .as_str()
        .to_string();
    assert!(!pair
        .bob
        .protocol
        .get_group_roles(&with_alice)
        .unwrap()
        .contains_key(&pair.alice.address));

    pair.bob.register(SubscriptionPolicy::Rules {
        allow_peers: Vec::new(),
        allow_groups: vec![without_alice.clone()],
        fallback: RulesFallback::Reject,
    });
    let refused = pair.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(pair.alice.state_of(&refused), None);
    assert!(pair.alice.take_changes().contains(&(
        "consumer".into(),
        "ended".into(),
        "policy".into()
    )));

    pair.bob
        .protocol
        .set_service_subscription_policy(
            SVC,
            SubscriptionPolicy::Rules {
                allow_peers: Vec::new(),
                allow_groups: vec![without_alice, with_alice],
                fallback: RulesFallback::Reject,
            },
        )
        .unwrap();
    let admitted = pair.subscribe("battery", SubscriptionMode::Latest);
    assert_eq!(
        pair.alice.state_of(&admitted),
        Some(SubscriptionState::Active)
    );
}

/// Blocking is bidirectional, and a subscription is no exception. The
/// provider that blocks a subscriber stops streaming to it at once, not at
/// the end of a lease, and queues nothing for it; the consumer that blocks a
/// provider holds nothing on it and cannot subscribe to it again.
#[test]
fn blocking_a_peer_ends_its_subscriptions_on_both_sides() {
    let mut pair = Pair::new();
    pair.bob.register(SubscriptionPolicy::AcceptAll);
    pair.alice.register(SubscriptionPolicy::AcceptAll);
    let alice = pair.alice.address.clone();
    pair.subscribe("battery", SubscriptionMode::Latest);
    let bob_on_alice = pair
        .bob
        .protocol
        .subscribe_service(&alice, SVC, "battery", SubscriptionMode::Latest, "")
        .unwrap();
    pair.run(12);
    assert_eq!(pair.bob.protocol.service_subscribers(Some(SVC)).len(), 1);
    assert_eq!(
        pair.bob.state_of(&bob_on_alice),
        Some(SubscriptionState::Active)
    );
    pair.bob.take_changes();

    // Out of range, so that a queued update would show in the outbox.
    pair.unlink();
    pair.run(2);
    let queued_before = pair.bob.outbox_to(&alice);
    pair.bob.protocol.block_user(&alice).unwrap();

    assert!(pair.bob.protocol.service_subscribers(Some(SVC)).is_empty());
    assert!(pair.bob.protocol.service_subscriptions().is_empty());
    let changes = pair.bob.take_changes();
    assert!(changes.contains(&("provider".into(), "ended".into(), "revoked".into())));
    assert!(changes.contains(&("consumer".into(), "ended".into(), "unsubscribed".into())));
    let report = pair
        .bob
        .protocol
        .publish_service_update(SVC, "battery", Some("pct"), "81")
        .unwrap();
    assert_eq!(report.subscribers, 0);
    assert!(
        pair.bob.outbox_to(&alice) <= queued_before,
        "a frame was queued for a blocked peer"
    );
    assert!(pair
        .bob
        .protocol
        .outbox
        .values()
        .all(|entry| entry.message.recipient.as_str() != alice || !entry.subscription_frame));
    let again =
        pair.bob
            .protocol
            .subscribe_service(&alice, SVC, "battery", SubscriptionMode::Latest, "");
    assert!(
        matches!(again, Err(crate::Error::UserBlocked(_))),
        "{again:?}"
    );
}
