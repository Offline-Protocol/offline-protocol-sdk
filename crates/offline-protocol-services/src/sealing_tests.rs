//! The configured node's discovery rules over a stand-in for the engine's
//! cryptography: an "application key" is a tag byte, a proof names its
//! signer, and a node without a certificate proves nothing. The real cipher
//! and signatures are exercised by the engine's tests.

use super::*;
use crate::events::ServiceEvent;
use crate::services::{MeshServices, OutboundMessage, ServiceAction};
use crate::{SVC_DISCOVER_QUERY, SVC_DISCOVER_RESPONSE};
use offline_protocol_core::{ServiceDescriptor, ServiceId};
use std::cell::Cell;

fn addr(byte: u8) -> String {
    Address::from_hash_bytes([byte; Address::HASH_LEN]).to_string()
}

struct Fake {
    me: String,
    app: u8,
    certified: bool,
    now: Cell<u64>,
}

impl Fake {
    fn member(me: &str, app: u8) -> Self {
        Self {
            me: me.to_string(),
            app,
            certified: true,
            now: Cell::new(1_800_000_000),
        }
    }

    fn uncertified(me: &str, app: u8) -> Self {
        Self {
            certified: false,
            ..Self::member(me, app)
        }
    }

    fn proof_of(signer: &str, recipient: Option<&str>, kind: ProofKind, bound: &[u8]) -> Vec<u8> {
        format!("{signer}|{recipient:?}|{}|{bound:?}", kind as u8).into_bytes()
    }
}

impl DiscoverySealer for Fake {
    fn now_unix(&self) -> u64 {
        self.now.get()
    }
    fn identity_key(&self) -> Option<Vec<u8>> {
        Some(format!("pk-{}", self.me).into_bytes())
    }
    fn seal(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        let mut out = vec![self.app];
        out.extend_from_slice(plaintext);
        Some(out)
    }
    fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        let (tag, rest) = sealed.split_first()?;
        (*tag == self.app).then(|| rest.to_vec())
    }
    fn prove(&self, recipient: Option<&str>, kind: ProofKind, bound: &[u8]) -> Vec<u8> {
        if self.certified {
            let mut proof = vec![self.app];
            proof.extend(Self::proof_of(&self.me, recipient, kind, bound));
            proof
        } else {
            vec![0xEE; 189]
        }
    }
    fn verify(
        &self,
        proof: &[u8],
        signer: &str,
        identity_key: &[u8],
        recipient: Option<&str>,
        kind: ProofKind,
        bound: &[u8],
    ) -> bool {
        let Some((tag, rest)) = proof.split_first() else {
            return false;
        };
        *tag == self.app
            && identity_key == format!("pk-{signer}").as_bytes()
            && rest == Self::proof_of(signer, recipient, kind, bound).as_slice()
    }
}

fn provider(service: &str) -> MeshServices {
    let mut svc = MeshServices::new();
    svc.register_service(ServiceDescriptor {
        service_id: ServiceId::new(service).expect("a service id"),
        version: "2.0".to_string(),
        capabilities: HashMap::from([("format".to_string(), "json".to_string())]),
    })
    .expect("registered");
    svc
}

fn consumed(action: ServiceAction) -> (Vec<OutboundMessage>, Vec<ServiceEvent>) {
    match action {
        ServiceAction::Consumed {
            messages_to_send,
            events_to_emit,
        } => (messages_to_send, events_to_emit),
        ServiceAction::NotHandled => panic!("a discovery frame is always consumed"),
    }
}

fn query_payload(content: &str) -> ServiceDiscoveryQueryPayload {
    serde_json::from_str(content.strip_prefix(SVC_DISCOVER_QUERY).expect("a query"))
        .expect("a query payload")
}

/// `alice` asks as a configured node, toward one neighbour.
fn ask(alice: &Fake, service: Option<&str>) -> String {
    let mut svc = MeshServices::new();
    let result = svc
        .discover_services_sealed(&alice.me, &[addr(9)], service, alice)
        .expect("a sealed query");
    result.messages[0].content.clone()
}

/// Hands `content` to `node` from `sender`, configured with `sealer`.
fn deliver(
    node: &mut MeshServices,
    sealer: &Fake,
    content: &str,
    sender: &str,
    peers: &[String],
) -> (Vec<OutboundMessage>, Vec<ServiceEvent>) {
    consumed(node.handle_incoming_message_sealed(
        content,
        sender,
        0,
        &sealer.me,
        peers,
        Some(sealer),
    ))
}

fn answers(messages: &[OutboundMessage]) -> Vec<&OutboundMessage> {
    messages
        .iter()
        .filter(|m| m.content.starts_with(SVC_DISCOVER_RESPONSE))
        .collect()
}

#[test]
fn a_configured_query_shows_only_its_routing_fields() {
    let alice = Fake::member(&addr(1), 7);
    let content = ask(&alice, Some("weather"));
    let payload = query_payload(&content);
    assert_eq!(payload.originator, alice.me);
    assert_eq!(
        payload.service_id.as_deref(),
        Some(DISCOVERY_SEALED_SERVICE_ID)
    );
    assert!(!content.contains("weather"));
    assert!(payload.sealed.is_some() && payload.member_proof.is_some());
}

#[test]
fn a_member_answers_a_member_toward_the_originator_and_it_is_reported() {
    let (alice, bob) = (Fake::member(&addr(1), 7), Fake::member(&addr(2), 7));
    let query = ask(&alice, Some("weather"));

    let mut bob_svc = provider("weather");
    let relay = addr(5);
    let (sent, events) = deliver(&mut bob_svc, &bob, &query, &relay, &[addr(6)]);
    assert!(events.is_empty());
    let replies = answers(&sent);
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].recipient, alice.me,
        "a configured provider answers the originator, never the direct sender"
    );
    assert!(!replies[0].content.contains("weather"));
    assert!(sent
        .iter()
        .any(|m| m.recipient == addr(6) && m.content.starts_with(SVC_DISCOVER_QUERY)));

    let mut alice_svc = MeshServices::new();
    let (sent, events) = deliver(&mut alice_svc, &alice, &replies[0].content, &bob.me, &[]);
    assert!(sent.is_empty());
    match events.as_slice() {
        [ServiceEvent::ServiceDiscovered {
            service_id,
            provider_peer_id,
            version,
            capabilities,
            ..
        }] => {
            assert_eq!(service_id, "weather");
            assert_eq!(provider_peer_id, &bob.me);
            assert_eq!(version, "2.0");
            assert_eq!(capabilities.get("format").map(String::as_str), Some("json"));
        }
        other => panic!("expected one discovery, got {other:?}"),
    }
}

/// The forwarder belongs to another application: it cannot open the body,
/// and still carries the query on, unchanged but for the hop budget, and
/// answers nothing.
#[test]
fn a_node_of_another_application_forwards_unchanged_and_answers_nothing() {
    let alice = Fake::member(&addr(1), 7);
    let other = Fake::member(&addr(3), 8);
    let query = ask(&alice, Some("weather"));
    let original = query_payload(&query);

    let mut other_svc = provider("weather");
    let (sent, events) = deliver(&mut other_svc, &other, &query, &alice.me, &[addr(6)]);
    assert!(events.is_empty());
    assert!(answers(&sent).is_empty());
    let forwarded: Vec<_> = sent
        .iter()
        .filter(|m| m.content.starts_with(SVC_DISCOVER_QUERY))
        .collect();
    assert_eq!(forwarded.len(), 1);
    let carried = query_payload(&forwarded[0].content);
    assert_eq!(carried.query_id, original.query_id);
    assert_eq!(carried.service_id, original.service_id);
    assert_eq!(carried.sealed, original.sealed);
    assert_eq!(carried.member_proof, original.member_proof);
    assert_eq!(carried.remaining_hops, original.remaining_hops - 1);
}

/// A stranger holding the key but no certificate: its query is carried and
/// never matched.
#[test]
fn a_query_without_a_valid_proof_is_forwarded_not_answered() {
    let stranger = Fake::uncertified(&addr(4), 7);
    let bob = Fake::member(&addr(2), 7);
    let query = ask(&stranger, None);
    let mut bob_svc = provider("weather");
    let (sent, events) = deliver(&mut bob_svc, &bob, &query, &stranger.me, &[addr(6)]);
    assert!(events.is_empty());
    assert!(answers(&sent).is_empty());
    assert_eq!(sent.len(), 1, "the query is still forwarded");
}

/// An unconfigured node's cleartext query: a configured provider carries it
/// and does not answer, since that would be its service list in the clear
/// to a stranger.
#[test]
fn a_configured_provider_does_not_answer_a_cleartext_query() {
    let bob = Fake::member(&addr(2), 7);
    let mut plain = MeshServices::new();
    let query = plain
        .discover_services(&addr(1), &[addr(2)], None)
        .expect("a query")
        .messages[0]
        .content
        .clone();
    let mut bob_svc = provider("weather");
    let (sent, _) = deliver(&mut bob_svc, &bob, &query, &addr(1), &[addr(6)]);
    assert!(answers(&sent).is_empty());
    assert_eq!(sent.len(), 1);
}

#[test]
fn a_query_is_answered_once_per_query_id() {
    let (alice, bob) = (Fake::member(&addr(1), 7), Fake::member(&addr(2), 7));
    let query = ask(&alice, None);
    let mut bob_svc = provider("weather");
    let (first, _) = deliver(&mut bob_svc, &bob, &query, &alice.me, &[]);
    assert_eq!(answers(&first).len(), 1);

    // The dedup window passed, or another path brought it: the answered set
    // still holds it while it is fresh.
    bob_svc.forget_seen_queries();
    bob.now.set(bob.now.get() + 30);
    let (second, _) = deliver(&mut bob_svc, &bob, &query, &addr(5), &[]);
    assert!(answers(&second).is_empty());
}

#[test]
fn a_replayed_query_past_its_lifetime_is_not_answered() {
    let (alice, bob) = (Fake::member(&addr(1), 7), Fake::member(&addr(2), 7));
    let query = ask(&alice, None);

    let mut late = provider("weather");
    bob.now
        .set(alice.now.get() + DISCOVERY_QUERY_LIFETIME_SECS + 1);
    let (sent, _) = deliver(&mut late, &bob, &query, &addr(5), &[addr(6)]);
    assert!(answers(&sent).is_empty());
    assert_eq!(sent.len(), 1, "a stale query is still carried");

    // Within the lifetime on either side of the originator's clock it is
    // answered: a provider a little behind is not refused.
    let mut behind = provider("weather");
    bob.now.set(alice.now.get() - DISCOVERY_QUERY_LIFETIME_SECS);
    let (sent, _) = deliver(&mut behind, &bob, &query, &addr(5), &[]);
    assert_eq!(answers(&sent).len(), 1);
}

/// The answered set refuses a replay for as long as the query could be
/// judged fresh, which is the bound the ADR states for party 2.
#[test]
fn the_answered_set_outlives_freshness() {
    let (alice, bob) = (Fake::member(&addr(1), 7), Fake::member(&addr(2), 7));
    let query = ask(&alice, None);
    let mut bob_svc = provider("weather");
    let start = alice.now.get() - DISCOVERY_QUERY_LIFETIME_SECS;
    bob.now.set(start);
    let (first, _) = deliver(&mut bob_svc, &bob, &query, &addr(5), &[]);
    assert_eq!(answers(&first).len(), 1);
    for later in [
        1,
        DISCOVERY_QUERY_LIFETIME_SECS,
        2 * DISCOVERY_QUERY_LIFETIME_SECS,
    ] {
        bob_svc.forget_seen_queries();
        bob.now.set(start + later);
        let (again, _) = deliver(&mut bob_svc, &bob, &query, &addr(5), &[]);
        assert!(answers(&again).is_empty(), "answered twice {later} s on");
    }
}

#[test]
fn a_response_whose_proof_names_another_provider_is_not_reported() {
    let (alice, bob) = (Fake::member(&addr(1), 7), Fake::member(&addr(2), 7));
    let mallory = addr(3);
    let query = ask(&alice, Some("weather"));
    let qid = query_payload(&query).query_id;

    // Bob's genuine response, handed on by mallory as if it were hers.
    let mut bob_svc = provider("weather");
    let (sent, _) = deliver(&mut bob_svc, &bob, &query, &addr(5), &[]);
    let genuine = answers(&sent)[0].content.clone();
    let mut alice_svc = MeshServices::new();
    let (_, events) = deliver(&mut alice_svc, &alice, &genuine, &mallory, &[]);
    assert!(
        events.is_empty(),
        "a response sent by someone other than its provider"
    );
    assert!(!discovery_response_is_proven(
        &genuine, &mallory, &alice.me, &alice
    ));

    // A body naming mallory as the provider, under bob's proof.
    let forged = seal_response(
        &bob,
        &qid,
        &alice.me,
        &mallory,
        "weather",
        "9.9",
        &HashMap::new(),
        0,
    )
    .expect("a response");
    let forged = format!(
        "{SVC_DISCOVER_RESPONSE}{}",
        serde_json::to_string(&forged).expect("json")
    );
    let (_, events) = deliver(&mut alice_svc, &alice, &forged, &mallory, &[]);
    assert!(events.is_empty(), "a proof made by bob over mallory's body");

    // And the genuine one, from bob, is reported.
    assert!(discovery_response_is_proven(
        &genuine, &bob.me, &alice.me, &alice
    ));
    let (_, events) = deliver(&mut alice_svc, &alice, &genuine, &bob.me, &[]);
    assert_eq!(events.len(), 1);
}

/// A configured originator never asks in the clear, so a cleartext response
/// addressed to it is a stranger's and is not reported. One on its way to
/// someone else is carried toward its originator, proof or not.
#[test]
fn cleartext_responses_are_carried_but_never_reported() {
    let alice = Fake::member(&addr(1), 7);
    let mut svc = MeshServices::new();
    let plain = |originator: &str| {
        format!(
            "{SVC_DISCOVER_RESPONSE}{}",
            serde_json::to_string(&ServiceDiscoveryResponsePayload {
                query_id: "q".to_string(),
                service_id: "weather".to_string(),
                version: "1".to_string(),
                provider_peer_id: addr(2),
                capabilities: HashMap::new(),
                hop_count: 0,
                originator: originator.to_string(),
                sealed: None,
                member_proof: None,
            })
            .expect("json")
        )
    };
    let (sent, events) = deliver(&mut svc, &alice, &plain(&alice.me), &addr(2), &[]);
    assert!(sent.is_empty() && events.is_empty());
    assert!(discovery_response_ends_at(&plain(&alice.me), &alice.me));
    assert!(discovery_response_ends_at(&plain(""), &alice.me));
    assert!(!discovery_response_is_proven(
        &plain(&alice.me),
        &addr(2),
        &alice.me,
        &alice
    ));

    let elsewhere = plain(&addr(4));
    assert!(!discovery_response_ends_at(&elsewhere, &alice.me));
    let (sent, events) = deliver(&mut svc, &alice, &elsewhere, &addr(2), &[]);
    assert!(events.is_empty());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].recipient, addr(4));
}

/// No service id can be the sentinel, so `register_service` can never hold
/// it: the type refuses it before a descriptor exists, from code and from
/// the wire alike.
#[test]
fn register_service_refuses_the_sentinel() {
    assert!(ServiceId::new(DISCOVERY_SEALED_SERVICE_ID).is_err());
    let descriptor = format!(
        r#"{{"service_id":"{DISCOVERY_SEALED_SERVICE_ID}","version":"1","capabilities":{{}}}}"#
    );
    assert!(serde_json::from_str::<ServiceDescriptor>(&descriptor).is_err());
}

/// An unconfigured node's query is what it was: no sealed field, no proof,
/// the requested service in the clear.
#[test]
fn an_unconfigured_query_is_unchanged_on_the_wire() {
    let mut svc = MeshServices::new();
    let content = svc
        .discover_services("user1", &["bob".to_string()], Some("weather"))
        .expect("a query")
        .messages[0]
        .content
        .clone();
    let json: serde_json::Value =
        serde_json::from_str(content.strip_prefix(SVC_DISCOVER_QUERY).expect("a query"))
            .expect("json");
    let mut keys: Vec<_> = json
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["originator", "query_id", "remaining_hops", "service_id"]
    );
    assert_eq!(json["service_id"], "weather");
}
