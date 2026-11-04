//! The application membership exchange over the real send and receive paths
//! (ADR 0037, step 5 of the shared network plan).
//!
//! Every frame goes through signing, the mock BLE link, the control gate and
//! the prefix dispatch. What is pinned is what each node concludes and what
//! a neighbour can observe on the link: that every node sends the same
//! frames at the same sizes whatever it is, that only two members of one
//! application see each other, and that nothing leaves the link.

use std::sync::Arc;

use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use offline_protocol_core::{Address, HopCount, Message};
use offline_protocol_sealed::membership::{
    MemberCert, MemberFrame, MEMBER_HELLO_LEN, MEMBER_SEALED_LEN,
};
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};
use crate::ProtocolConfig;

const APP: &str = "com.example.chat";

/// A publisher key pair, seeded so each test's keys are reproducible.
fn publisher(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// A certificate `publisher` signs over `address`, current for a day.
fn certificate(publisher: &SigningKey, address: &str) -> MemberCert {
    let address: Address = address.parse().expect("an address");
    let not_after = u64::try_from(Utc::now().timestamp()).unwrap() + 86_400;
    let signature = publisher
        .sign(&MemberCert::signing_payload(&address, not_after))
        .to_bytes();
    MemberCert {
        address,
        not_after,
        signature,
    }
}

struct Node {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
}

impl Node {
    fn launch(label: &str, publisher_key: Option<[u8; 32]>) -> Self {
        Self::launch_with(label, publisher_key, false)
    }

    /// `auto_key_exchange` off unless a test is about the key-package push
    /// on discovery: with it on, every link also carries a handshake.
    fn launch_with(label: &str, publisher_key: Option<[u8; 32]>, auto_key_exchange: bool) -> Self {
        Self::launch_configured(label, publisher_key, auto_key_exchange, |_| {})
    }

    fn launch_configured(
        label: &str,
        publisher_key: Option<[u8; 32]>,
        auto_key_exchange: bool,
        tweak: impl FnOnce(&mut ProtocolConfig),
    ) -> Self {
        let mut config: ProtocolConfig = create_test_config_for_user(label);
        config.app_id = APP.to_string();
        config.encryption.enabled = true;
        config.encryption.auto_key_exchange = auto_key_exchange;
        config.app_publisher_key = publisher_key;
        tweak(&mut config);
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let state = Arc::new(InMemoryStorage::new());
        protocol
            .initialize_mls(
                crate::test_identity::seeded_storage(label),
                Arc::new(TestProtocolStateStorage { storage: state }),
            )
            .expect("initialize_mls");
        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        mock.set_reject_unknown_recipients(true);
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));
        protocol.start().expect("start");
        Self {
            protocol,
            transport,
            address: id(label),
        }
    }

    /// A member: configured with `publisher`'s key and holding a certificate
    /// it signed.
    fn member(label: &str, publisher: &SigningKey) -> Self {
        let mut node = Self::launch(label, Some(publisher.verifying_key().to_bytes()));
        let cert = certificate(publisher, &node.address);
        node.protocol
            .set_app_membership(&cert.to_bytes())
            .expect("the node's own certificate");
        node
    }

    /// Puts `peer` in range, as discovery would.
    fn link(&mut self, peer: &Node) {
        self.transport.add_connected_peer(peer.address.clone(), -55);
        self.protocol.on_neighbor_discovered(&peer.address);
    }

    /// The membership frames handed to a neighbour since the last take.
    fn take_member_frames(&mut self) -> Vec<(String, Message)> {
        let sends = self.transport.peer_sends();
        self.transport.clear_peer_sends();
        sends
            .into_iter()
            .filter(|(_, m)| m.content.starts_with(internal_prefixes::MEMBER))
            .collect()
    }

    fn receive_from(&mut self, message: Message, from: &str) {
        self.transport.queue_message_from(message, from.to_string());
        while self.protocol.receive_message().is_some() {}
    }

    fn sees(&self, peer: &Node) -> bool {
        self.protocol.is_member_visible(&peer.address)
    }
}

/// The body of a membership frame, parsed.
fn frame(message: &Message) -> MemberFrame {
    let body = message
        .content
        .strip_prefix(internal_prefixes::MEMBER)
        .expect("a membership frame");
    MemberFrame::parse(body).expect("a well-formed membership frame")
}

/// Every membership frame each node sent, delivered to its neighbour, until
/// neither has anything more to say. Returns what crossed the link, in order.
fn run(a: &mut Node, b: &mut Node) -> Vec<(String, Message)> {
    let mut crossed = Vec::new();
    for _ in 0..8 {
        let from_a = a.take_member_frames();
        let from_b = b.take_member_frames();
        if from_a.is_empty() && from_b.is_empty() {
            return crossed;
        }
        for (to, message) in from_a {
            assert_eq!(
                to, b.address,
                "a membership frame goes to the neighbour only"
            );
            crossed.push((a.address.clone(), message.clone()));
            b.receive_from(message, &a.address.clone());
        }
        for (to, message) in from_b {
            assert_eq!(
                to, a.address,
                "a membership frame goes to the neighbour only"
            );
            crossed.push((b.address.clone(), message.clone()));
            a.receive_from(message, &b.address.clone());
        }
    }
    panic!("the exchange did not settle");
}

fn meet(a: &mut Node, b: &mut Node) -> Vec<(String, Message)> {
    a.link(b);
    b.link(a);
    run(a, b)
}

/// What a neighbour sees on the link: per sender, the sequence of
/// (step, payload size). This is everything the exchange lets a stranger
/// observe, and it must not vary with membership.
fn shape(crossed: &[(String, Message)], sender: &str) -> Vec<(u8, usize)> {
    crossed
        .iter()
        .filter(|(from, _)| from == sender)
        .map(|(_, m)| match frame(m) {
            MemberFrame::Hello(_) => (1, MEMBER_HELLO_LEN),
            MemberFrame::Response(sealed) => (2, sealed.len()),
        })
        .collect()
}

#[test]
fn two_members_of_one_application_see_each_other() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);

    meet(&mut alice, &mut bob);

    assert!(alice.sees(&bob));
    assert!(bob.sees(&alice));
}

#[test]
fn members_of_two_applications_see_nothing_and_look_alike() {
    let mut alice = Node::member("alice", &publisher(1));
    let mut bob = Node::member("bob", &publisher(2));
    let crossed = meet(&mut alice, &mut bob);

    assert!(!alice.sees(&bob));
    assert!(!bob.sees(&alice));

    // The same pair under one application: the link must look identical.
    let key = publisher(1);
    let mut carol = Node::member("carol", &key);
    let mut dave = Node::member("dave", &key);
    let crossed_members = meet(&mut carol, &mut dave);
    assert!(carol.sees(&dave));

    let expected = vec![(1, MEMBER_HELLO_LEN), (2, MEMBER_SEALED_LEN)];
    assert_eq!(shape(&crossed, &alice.address), expected);
    assert_eq!(shape(&crossed, &bob.address), expected);
    assert_eq!(shape(&crossed_members, &carol.address), expected);
    assert_eq!(shape(&crossed_members, &dave.address), expected);
}

#[test]
fn an_unconfigured_node_runs_the_exchange_at_the_same_sizes() {
    let mut alice = Node::member("alice", &publisher(1));
    let mut bob = Node::launch("bob", None);
    let crossed = meet(&mut alice, &mut bob);

    assert!(!alice.sees(&bob));
    assert!(!bob.sees(&alice));
    let expected = vec![(1, MEMBER_HELLO_LEN), (2, MEMBER_SEALED_LEN)];
    assert_eq!(shape(&crossed, &bob.address), expected);
    assert_eq!(shape(&crossed, &alice.address), expected);
}

/// Party 3 of the attacker table: it holds the application's binary, so the
/// application key, but no certificate. It recognises a member, which the
/// ADR accepts, and is never recognised as one.
#[test]
fn a_node_with_the_key_and_no_certificate_is_never_visible() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut mallory = Node::launch("mallory", Some(key.verifying_key().to_bytes()));
    meet(&mut alice, &mut mallory);

    assert!(!alice.sees(&mallory));
    assert!(
        mallory.sees(&alice),
        "recognising a member is party 3's residual"
    );
}

/// A member's certificate presented by another node: it opens, since the
/// presenter holds the application key, and still fails the address check.
#[test]
fn a_certificate_for_another_address_does_not_make_its_presenter_visible() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut mallory = Node::launch("mallory", Some(key.verifying_key().to_bytes()));
    mallory
        .protocol
        .set_app_membership_unchecked(certificate(&key, &id("carol")));
    meet(&mut alice, &mut mallory);

    assert!(!alice.sees(&mallory));
}

#[test]
fn every_membership_frame_stays_on_the_link() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    let crossed = meet(&mut alice, &mut bob);

    assert_eq!(crossed.len(), 4, "one hello and one response each way");
    for (_, message) in &crossed {
        assert!(
            !message.requires_ack,
            "an acknowledgement would be an answer"
        );
        assert_eq!(message.ttl.value(), 1, "no forwarder may pass it on");
        assert_eq!(message.hop_count.value(), 0);
    }
}

/// A hello that reaches this node any way but straight over a mesh link is
/// consumed and never answered: a relay mailbox would hold it for days, and
/// a forwarded one is not from a neighbour.
#[test]
fn a_hello_that_did_not_come_over_the_link_is_not_answered() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    alice.link(&bob);
    let (_, hello) = alice.take_member_frames().remove(0);
    bob.transport.add_connected_peer(alice.address.clone(), -55);

    let mut forwarded = hello.clone();
    forwarded.hop_count = HopCount::from_value(1);
    bob.receive_from(forwarded, &alice.address);
    assert!(
        bob.take_member_frames().is_empty(),
        "a forwarded hello is not answered"
    );

    assert!(bob
        .protocol
        .process_internal_message_via(&hello, Some(TransportType::Internet))
        .is_some());
    assert!(
        bob.take_member_frames().is_empty(),
        "a hello over the internet is not answered"
    );

    assert!(bob
        .protocol
        .process_internal_message_via(&hello, None)
        .is_some());
    assert!(
        bob.take_member_frames().is_empty(),
        "a hello of unknown arrival is not answered"
    );
}

/// A recorded hello replayed: answering it again would seal a second
/// plaintext under a key already used.
#[test]
fn a_replayed_hello_is_answered_once() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    alice.link(&bob);
    bob.transport.add_connected_peer(alice.address.clone(), -55);
    let (_, hello) = alice.take_member_frames().remove(0);

    bob.receive_from(hello.clone(), &alice.address);
    let first = bob.take_member_frames();
    assert_eq!(first.len(), 2, "bob's own hello, then the answer");

    // The same body in a fresh, validly signed frame: what the deduplicator
    // does not catch, since the id is new.
    let mut replay = hello;
    replay.id = offline_protocol_core::MessageId::new();
    replay.metadata.clear();
    {
        let manager = alice.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        OfflineProtocol::sign_control_message_with(&mut replay, &manager, false).unwrap();
    }
    bob.receive_from(replay, &alice.address);
    assert!(bob.take_member_frames().is_empty());
}

/// The link delivers the peer's response ahead of its hello: it is held, and
/// the exchange still completes once the hello arrives.
#[test]
fn a_response_ahead_of_its_hello_still_completes() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    alice.link(&bob);
    bob.transport.add_connected_peer(alice.address.clone(), -55);

    // Bob hears alice's hello and answers with his own hello and response.
    let (_, alice_hello) = alice.take_member_frames().remove(0);
    bob.receive_from(alice_hello, &alice.address);
    let mut from_bob = bob.take_member_frames();
    assert_eq!(from_bob.len(), 2);
    let (_, bob_response) = from_bob.pop().unwrap();
    let (_, bob_hello) = from_bob.pop().unwrap();
    assert!(matches!(frame(&bob_hello), MemberFrame::Hello(_)));

    alice.receive_from(bob_response, &bob.address);
    assert!(!alice.sees(&bob), "nothing to open it with yet");
    alice.receive_from(bob_hello, &bob.address);
    assert!(alice.sees(&bob));
}

#[test]
fn a_peer_that_restarts_is_recognised_again() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    meet(&mut alice, &mut bob);
    assert!(alice.sees(&bob));

    // Bob restarts: same identity, no memory of the exchange.
    let mut bob = Node::member("bob", &key);
    bob.link(&alice);
    alice.transport.add_connected_peer(bob.address.clone(), -55);
    run(&mut alice, &mut bob);
    assert!(bob.sees(&alice));
}

#[test]
fn a_rediscovery_within_the_interval_sends_nothing() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    meet(&mut alice, &mut bob);

    alice.protocol.on_neighbor_discovered(&bob.address);
    assert!(alice.take_member_frames().is_empty());
}

/// Past the interval a rediscovery runs the exchange again, for a member
/// exactly as for a stranger. Skipping it for a peer already visible would
/// make the hello an answer to "is that address a member here", readable by
/// anyone who presents a recorded address on a link of its own.
#[test]
fn a_member_rediscovered_after_the_interval_is_greeted_like_anyone() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    let mut carol = Node::member("carol", &publisher(2));
    meet(&mut alice, &mut bob);
    meet(&mut alice, &mut carol);
    assert!(alice.sees(&bob));
    assert!(!alice.sees(&carol));

    alice.protocol.expire_member_exchanges();
    alice.protocol.on_neighbor_discovered(&bob.address);
    alice.protocol.on_neighbor_discovered(&carol.address);
    let sent = alice.take_member_frames();
    let hello_to = |peer: &Node| {
        sent.iter()
            .filter(|(to, m)| *to == peer.address && matches!(frame(m), MemberFrame::Hello(_)))
            .count()
    };
    assert_eq!(hello_to(&bob), 1, "a visible member is greeted again");
    assert_eq!(hello_to(&carol), 1, "as a stranger is");

    // And the member stays recognised through the new exchange.
    for (to, message) in sent {
        if to == bob.address {
            bob.receive_from(message, &alice.address);
        }
    }
    run(&mut alice, &mut bob);
    assert!(alice.sees(&bob));
    assert!(bob.sees(&alice));
}

/// A certificate the publisher signed for the presenter's own address, but
/// expired: it opens and verifies, and still does not make its presenter
/// visible.
///
/// Two checks hold this: the exchange refuses an expired certificate, and
/// `is_member_visible` compares the cached `not_after` with the clock. Either
/// alone keeps the test green, and removing both fails it; the second is what
/// ages out a certificate that was current when it verified.
#[test]
fn an_expired_certificate_in_the_exchange_is_not_visible() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut mallory = Node::launch("mallory", Some(key.verifying_key().to_bytes()));
    let address: Address = mallory.address.parse().unwrap();
    mallory.protocol.set_app_membership_unchecked(MemberCert {
        address: address.clone(),
        not_after: 1,
        signature: key
            .sign(&MemberCert::signing_payload(&address, 1))
            .to_bytes(),
    });
    meet(&mut alice, &mut mallory);

    assert!(!alice.sees(&mallory));
    assert!(
        mallory.sees(&alice),
        "the positive control: the pair did run"
    );
}

/// The application's backend issues the certificate after the node has met
/// its neighbours. Supplying it starts a fresh exchange with each of them, so
/// they learn now rather than on some later discovery.
#[test]
fn a_certificate_supplied_after_meeting_is_presented_at_once() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::launch("bob", Some(key.verifying_key().to_bytes()));
    meet(&mut alice, &mut bob);
    assert!(!alice.sees(&bob), "no certificate yet");

    let cert = certificate(&key, &bob.address);
    bob.protocol
        .set_app_membership(&cert.to_bytes())
        .expect("bob's own certificate");
    run(&mut alice, &mut bob);

    assert!(alice.sees(&bob));
    assert!(bob.sees(&alice));
}

#[test]
fn set_app_membership_refuses_what_would_never_verify() {
    let key = publisher(1);
    let mut alice = Node::launch("alice", Some(key.verifying_key().to_bytes()));

    let other = certificate(&key, &id("bob"));
    assert!(alice
        .protocol
        .set_app_membership(&other.to_bytes())
        .is_err());

    let wrong_publisher = certificate(&publisher(2), &alice.address);
    assert!(alice
        .protocol
        .set_app_membership(&wrong_publisher.to_bytes())
        .is_err());

    let address: Address = alice.address.parse().unwrap();
    let expired = MemberCert {
        address: address.clone(),
        not_after: 1,
        signature: key
            .sign(&MemberCert::signing_payload(&address, 1))
            .to_bytes(),
    };
    assert!(alice
        .protocol
        .set_app_membership(&expired.to_bytes())
        .is_err());

    assert!(alice.protocol.set_app_membership(&[0u8; 10]).is_err());

    let own = certificate(&key, &alice.address);
    assert!(alice.protocol.set_app_membership(&own.to_bytes()).is_ok());

    let mut unconfigured = Node::launch("carol", None);
    let carol_cert = certificate(&key, &unconfigured.address);
    assert!(matches!(
        unconfigured
            .protocol
            .set_app_membership(&carol_cert.to_bytes()),
        Err(crate::Error::InvalidConfiguration(_))
    ));
}

// --- Frames built by hand ----------------------------------------------------
//
// The tests below build the peer's frames from the chapter's construction with
// the primitives called directly, rather than by running a second engine. That
// does two things: it lets a test send what an honest engine never would, and
// it pins the engine's HKDF inputs and its AEAD (zero nonce, no associated
// data) against an independent reading of the spec, which the vector file
// cannot do because it does not pin the AEAD output.

mod by_hand {
    use super::*;
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
    use hmac::Mac;
    use offline_protocol_sealed::membership::{
        app_key_info, exchange_key_info, exchange_salt, hello_tag_input,
        transcript_signing_payload, MemberHello, MEMBER_NONCE_LEN,
    };
    use sha2::Sha256;

    fn hkdf(ikm: &[u8], salt: Option<&[u8]>, info: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        hkdf::Hkdf::<Sha256>::new(salt, ikm)
            .expand(info, &mut out)
            .unwrap();
        out
    }

    fn k_app(publisher: &SigningKey) -> [u8; 32] {
        hkdf(
            &publisher.verifying_key().to_bytes(),
            None,
            &app_key_info(APP),
        )
    }

    fn addr(node: &Node) -> Address {
        node.address.parse().unwrap()
    }

    fn tag(k_app: &[u8; 32], nonce: &[u8; 16], from: &Node, to: &Node) -> [u8; 16] {
        let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(k_app).unwrap();
        mac.update(&hello_tag_input(nonce, &addr(from), &addr(to)));
        mac.finalize().into_bytes()[..16].try_into().unwrap()
    }

    /// A membership frame from `from` to `to`, signed by `from`'s identity as
    /// its own engine would sign it.
    fn signed_frame(from: &mut Node, to: &Node, frame: &MemberFrame) -> Message {
        let content = format!("{}{}", internal_prefixes::MEMBER, frame.to_body());
        let mut message = from
            .protocol
            .create_message(to.address.clone(), content, None, None)
            .unwrap();
        message.requires_ack = false;
        message.ttl = offline_protocol_core::TTL::from_value(1);
        let manager = from.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        OfflineProtocol::sign_control_message_with(&mut message, &manager, false).unwrap();
        message
    }

    fn identity_sign(node: &Node, payload: &[u8]) -> Vec<u8> {
        let manager = node.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.sign_data(payload).unwrap()
    }

    fn hello_nonce(message: &Message) -> [u8; MEMBER_NONCE_LEN] {
        match frame(message) {
            MemberFrame::Hello(hello) => hello.nonce,
            MemberFrame::Response(_) => panic!("expected a hello"),
        }
    }

    /// Carol, a member, has said hello to alice; alice's side is built by
    /// hand. `peer_nonce_signed` is the nonce of carol's that alice's
    /// transcript signature names. Returns whether carol sees alice.
    fn carol_sees_a_hand_built_alice(peer_nonce_signed: Option<[u8; 16]>) -> bool {
        let key = publisher(1);
        let k_app = k_app(&key);
        let mut carol = Node::member("carol", &key);
        let mut alice = Node::member("alice", &key);
        carol.link(&alice);
        let (_, carol_hello) = carol.take_member_frames().remove(0);
        let nonce_c = hello_nonce(&carol_hello);

        let nonce_a = [9u8; 16];
        let hello = MemberFrame::Hello(MemberHello {
            nonce: nonce_a,
            tag: tag(&k_app, &nonce_a, &alice, &carol),
        });

        let salt = exchange_salt(&addr(&alice), &nonce_a, &addr(&carol), &nonce_c).unwrap();
        let sealing = hkdf(&k_app, Some(&salt), &exchange_key_info(&addr(&alice)));
        let transcript = transcript_signing_payload(
            &addr(&alice),
            &nonce_a,
            &addr(&carol),
            &peer_nonce_signed.unwrap_or(nonce_c),
        );
        let mut plaintext = certificate(&key, &alice.address).to_bytes().to_vec();
        plaintext.extend_from_slice(&identity_sign(&alice, &transcript));
        let sealed = ChaCha20Poly1305::new(Key::from_slice(&sealing))
            .encrypt(Nonce::from_slice(&[0u8; 12]), plaintext.as_slice())
            .unwrap();
        let response = MemberFrame::Response(sealed.try_into().unwrap());

        let hello = signed_frame(&mut alice, &carol, &hello);
        let response = signed_frame(&mut alice, &carol, &response);
        carol.receive_from(hello, &alice.address);
        carol.receive_from(response, &alice.address);
        carol.sees(&alice)
    }

    /// The positive control: a response built from the chapter alone, with
    /// no associated data, is one the engine accepts.
    #[test]
    fn a_response_built_from_the_chapter_is_accepted() {
        assert!(carol_sees_a_hand_built_alice(None));
    }

    /// The transcript signature is what ties a certificate to this exchange:
    /// a credential whose signature names another nonce of the peer's, as one
    /// recorded from an earlier exchange would, is refused.
    #[test]
    fn a_credential_signed_for_another_exchange_is_refused() {
        assert!(!carol_sees_a_hand_built_alice(Some([0u8; 16])));
    }

    /// Carol's answer to a hello from alice: whether it opens under the key
    /// a holder of `K_app` derives for it. `tag_ok` is whether alice's hello
    /// carried the right tag.
    fn carol_answer_opens(tag_ok: bool) -> bool {
        let key = publisher(1);
        let k_app = k_app(&key);
        let mut carol = Node::member("carol", &key);
        let mut alice = Node::member("alice", &key);
        carol
            .transport
            .add_connected_peer(alice.address.clone(), -55);

        let nonce_a = [7u8; 16];
        let hello = MemberFrame::Hello(MemberHello {
            nonce: nonce_a,
            tag: if tag_ok {
                tag(&k_app, &nonce_a, &alice, &carol)
            } else {
                [0u8; 16]
            },
        });
        let hello = signed_frame(&mut alice, &carol, &hello);
        carol.receive_from(hello, &alice.address);
        let mut sent = carol.take_member_frames();
        assert_eq!(sent.len(), 2, "carol's own hello, then the answer");
        let (_, answer) = sent.pop().unwrap();
        let (_, carol_hello) = sent.pop().unwrap();
        let nonce_c = hello_nonce(&carol_hello);
        let MemberFrame::Response(sealed) = frame(&answer) else {
            panic!("expected a response");
        };
        let salt = exchange_salt(&addr(&carol), &nonce_c, &addr(&alice), &nonce_a).unwrap();
        let opening = hkdf(&k_app, Some(&salt), &exchange_key_info(&addr(&carol)));
        ChaCha20Poly1305::new(Key::from_slice(&opening))
            .decrypt(Nonce::from_slice(&[0u8; 12]), sealed.as_slice())
            .is_ok()
    }

    /// A peer that keeps sending fresh nonces is answered under one nonce of
    /// this node's for eight of them, and the ninth starts this node over
    /// with a fresh nonce, so the answered set stays bounded and no key
    /// repeats.
    #[test]
    fn the_ninth_fresh_hello_restarts_with_a_fresh_nonce() {
        let key = publisher(1);
        let k_app = k_app(&key);
        let mut carol = Node::member("carol", &key);
        let mut alice = Node::member("alice", &key);
        carol
            .transport
            .add_connected_peer(alice.address.clone(), -55);

        let mut carol_nonces = Vec::new();
        for i in 0..9u8 {
            let nonce = [i + 1; 16];
            let hello = MemberFrame::Hello(MemberHello {
                nonce,
                tag: tag(&k_app, &nonce, &alice, &carol),
            });
            let hello = signed_frame(&mut alice, &carol, &hello);
            carol.receive_from(hello, &alice.address);
            let sent = carol.take_member_frames();
            assert_eq!(sent.len(), 2, "hello {i}: carol's hello, then the answer");
            assert!(matches!(frame(&sent[1].1), MemberFrame::Response(_)));
            carol_nonces.push(hello_nonce(&sent[0].1));
        }
        assert!(
            carol_nonces[..8].iter().all(|n| *n == carol_nonces[0]),
            "the same nonce while the peer restarts, so the two settle"
        );
        assert_ne!(carol_nonces[8], carol_nonces[0], "a fresh one past eight");
    }

    /// A member seals its credential only for a hello whose tag proves the
    /// application key on this link. Anything else gets bytes no key opens.
    #[test]
    fn a_member_reveals_its_credential_only_to_a_hello_whose_tag_verifies() {
        assert!(carol_answer_opens(true), "the positive control");
        assert!(!carol_answer_opens(false));
    }
}

/// An unsigned hello carries no identity key, so its response could never be
/// checked and its sender is only a claim: it is not answered.
///
/// The control gate refuses it before the handler runs, so this pins the
/// observable outcome. The handler's own refusal of an unsigned frame is a
/// second line behind the gate, and removing it alone changes nothing here.
#[test]
fn an_unsigned_hello_is_not_answered() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    alice.link(&bob);
    bob.transport.add_connected_peer(alice.address.clone(), -55);
    let (_, mut hello) = alice.take_member_frames().remove(0);
    let signed = hello.clone();
    hello.metadata.clear();

    bob.receive_from(hello, &alice.address);
    assert!(bob.take_member_frames().is_empty());

    // The positive control: the same hello, signed, is answered.
    bob.receive_from(signed, &alice.address);
    assert_eq!(bob.take_member_frames().len(), 2);
}

// --- The application's calls (step 6) ----------------------------------------

#[test]
fn supplying_the_same_certificate_again_sends_nothing() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    meet(&mut alice, &mut bob);
    assert!(alice.sees(&bob));

    // An application that hands the certificate over on every foreground.
    let cert = certificate(&key, &alice.address);
    let first = alice.protocol.app_membership_status().certificate_not_after;
    let same = MemberCert {
        not_after: first.expect("alice holds a certificate"),
        ..cert
    };
    let same = MemberCert {
        signature: key
            .sign(&MemberCert::signing_payload(&same.address, same.not_after))
            .to_bytes(),
        ..same
    };
    alice
        .protocol
        .set_app_membership(&same.to_bytes())
        .expect("the same certificate");
    assert!(
        alice.take_member_frames().is_empty(),
        "the same certificate again restarts nothing"
    );

    // A different certificate (a renewal) does restart, so every neighbour
    // learns the new one.
    let renewed = MemberCert {
        not_after: same.not_after + 1,
        signature: key
            .sign(&MemberCert::signing_payload(
                &same.address,
                same.not_after + 1,
            ))
            .to_bytes(),
        ..same
    };
    alice
        .protocol
        .set_app_membership(&renewed.to_bytes())
        .expect("a renewed certificate");
    assert_eq!(alice.take_member_frames().len(), 1, "one hello to bob");
}

#[test]
fn a_peer_with_no_mesh_link_gets_no_hello() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);

    // Discovery through the relay or the LAN: no mesh link to bob.
    alice.protocol.on_neighbor_discovered(&bob.address);
    assert!(alice.take_member_frames().is_empty());
    assert_eq!(alice.protocol.member_exchange_count(), 0);

    // The mesh link that follows is greeted at once, not held off.
    meet(&mut alice, &mut bob);
    assert!(alice.sees(&bob));
}

#[test]
fn app_membership_status_reports_this_node_only() {
    let key = publisher(1);
    let unconfigured = Node::launch("carol", None);
    assert_eq!(
        unconfigured.protocol.app_membership_status(),
        crate::AppMembershipStatus {
            configured: false,
            certificate_not_after: None,
            visible_members: 0,
        }
    );

    let mut alice = Node::launch("alice", Some(key.verifying_key().to_bytes()));
    let status = alice.protocol.app_membership_status();
    assert!(status.configured);
    assert_eq!(status.certificate_not_after, None);

    let cert = certificate(&key, &alice.address);
    alice
        .protocol
        .set_app_membership(&cert.to_bytes())
        .expect("alice's certificate");
    let mut bob = Node::member("bob", &key);
    meet(&mut alice, &mut bob);
    let status = alice.protocol.app_membership_status();
    assert_eq!(status.certificate_not_after, Some(cert.not_after));
    assert_eq!(status.visible_members, 1);
}

#[test]
fn a_certificate_from_sign_app_membership_is_accepted() {
    let key = publisher(7);
    let mut alice = Node::launch("alice", Some(key.verifying_key().to_bytes()));
    let not_after = u64::try_from(Utc::now().timestamp()).unwrap() + 3_600;
    let cert = crate::sign_app_membership(&key.to_bytes(), &alice.address, not_after)
        .expect("a certificate");
    alice
        .protocol
        .set_app_membership(&cert)
        .expect("signed by the configured publisher");

    assert!(crate::sign_app_membership(&[0u8; 31], &alice.address, not_after).is_err());
    assert!(crate::sign_app_membership(&key.to_bytes(), "not-an-address", not_after).is_err());
}

#[test]
fn the_exchange_table_is_bounded() {
    use crate::protocol::membership::MAX_MEMBER_EXCHANGES;

    let mut alice = Node::launch("alice", None);
    for n in 0..=MAX_MEMBER_EXCHANGES {
        let mut seed = [0xEE; 32];
        seed[..8].copy_from_slice(&(n as u64).to_be_bytes());
        let key = SigningKey::from_bytes(&seed);
        let peer = offline_protocol_sealed::derive_address(&key.verifying_key().to_bytes())
            .expect("an address")
            .to_string();
        alice.transport.add_connected_peer(peer.clone(), -60);
        alice.protocol.on_neighbor_discovered(&peer);
    }
    assert_eq!(alice.protocol.member_exchange_count(), MAX_MEMBER_EXCHANGES);
}

#[test]
fn the_visible_set_is_bounded_and_drops_the_soonest_to_end() {
    use crate::protocol::membership::MAX_VISIBLE_MEMBERS;

    let mut alice = Node::launch("alice", None);
    let far = u64::try_from(Utc::now().timestamp()).unwrap() + 86_400;
    for n in 0..MAX_VISIBLE_MEMBERS {
        alice
            .protocol
            .mark_member_visible(&format!("peer-{n}"), far + n as u64);
    }
    // One more: the entry ending soonest ("peer-0") makes room.
    alice.protocol.mark_member_visible("late", far + 1_000_000);
    assert!(!alice.protocol.is_member_visible("peer-0"));
    assert!(alice.protocol.is_member_visible("peer-1"));
    assert!(alice.protocol.is_member_visible("late"));
    assert_eq!(
        alice.protocol.app_membership_status().visible_members as usize,
        MAX_VISIBLE_MEMBERS
    );
}

/// `sign_app_membership` holds the publisher's private key for the length of
/// one call. Nothing in its body may log, emit, store or send, so the key can
/// reach none of those whatever is later added around it.
#[test]
fn sign_app_membership_never_lets_the_key_out() {
    let source = include_str!("../membership.rs");
    let start = source
        .find("pub fn sign_app_membership(")
        .expect("the function is in membership.rs");
    let body_start = start + source[start..].find('{').expect("a body");
    let mut depth = 0usize;
    let mut end = body_start;
    for (offset, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &source[body_start..end];
    assert!(body.contains("SigningKey::from_bytes"), "found the body");
    for refused in [
        "trace!",
        "debug!",
        "info!",
        "warn!",
        "error!",
        "tracing",
        "println",
        "eprintln",
        "dbg!",
        "self",
        "emit",
        "storage",
        "store",
        "persist",
        "send",
        "seed:?",
        "{seed",
        "publisher_private_key:?",
        "{publisher_private_key",
    ] {
        assert!(
            !body.contains(refused),
            "sign_app_membership's body names `{refused}`: the private key must not reach a log, \
             an event, storage or a link"
        );
    }
}

// --- The visibility gate (step 7) ---------------------------------------------

use crate::events::{Event, PresenceStatus};
use std::sync::Mutex;

/// Every event the core emits, for a test to read.
fn record_events(node: &mut Node) -> Arc<Mutex<Vec<Event>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    node.protocol
        .on_event(move |event| sink.lock().unwrap().push(event));
    seen
}

fn discovered(events: &Mutex<Vec<Event>>, peer: &str) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::NeighborDiscovered {
                peer_id, transport, ..
            } if peer_id == peer => Some(transport.clone()),
            _ => None,
        })
        .collect()
}

/// A carrier reporting `peer` the way the bindings do: link first, then
/// the core's decision.
fn reach(node: &mut Node, peer: &Node) -> bool {
    node.transport.add_connected_peer(peer.address.clone(), -55);
    node.protocol.neighbor_reachable(&peer.address, "BLE")
}

#[test]
fn a_member_is_reported_once_it_proves_membership_and_a_stranger_never() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    let mut carol = Node::member("carol", &publisher(2));
    let events = record_events(&mut alice);

    assert!(!reach(&mut alice, &bob), "not visible yet: held");
    bob.link(&alice);
    run(&mut alice, &mut bob);
    assert_eq!(
        discovered(&events, &bob.address),
        vec!["BLE".to_string()],
        "the core reports bob once he is visible, naming the carrier"
    );

    assert!(!reach(&mut alice, &carol));
    carol.link(&alice);
    run(&mut alice, &mut carol);
    assert!(discovered(&events, &carol.address).is_empty());
    assert!(!alice.protocol.app_sees_peer(&carol.address));

    // Rediscovered while visible: reported by the caller at once.
    assert!(reach(&mut alice, &bob));
}

#[test]
fn an_unconfigured_node_reports_every_neighbour_at_once() {
    let mut alice = Node::launch("alice", None);
    let bob = Node::member("bob", &publisher(1));
    let events = record_events(&mut alice);
    assert!(reach(&mut alice, &bob));
    assert!(alice.protocol.app_sees_peer(&bob.address));
    assert!(alice.protocol.neighbor_unreachable(&bob.address));
    assert!(
        discovered(&events, &bob.address).is_empty(),
        "the caller reports it; the core adds nothing"
    );
}

#[test]
fn a_loss_is_reported_only_for_a_neighbour_that_was_reported() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    let carol = Node::member("carol", &publisher(2));

    reach(&mut alice, &carol);
    alice.take_member_frames();
    assert!(
        !alice.protocol.neighbor_unreachable(&carol.address),
        "never reported, so never reported lost"
    );

    reach(&mut alice, &bob);
    bob.link(&alice);
    run(&mut alice, &mut bob);
    assert!(alice.protocol.neighbor_unreachable(&bob.address));
    assert!(
        !alice.protocol.neighbor_unreachable(&bob.address),
        "reported lost once"
    );
}

#[test]
fn a_neighbour_lost_before_it_proves_membership_is_never_reported() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    let events = record_events(&mut alice);

    reach(&mut alice, &bob);
    assert!(!alice.protocol.neighbor_unreachable(&bob.address));
    // The exchange completes after the loss was seen.
    bob.link(&alice);
    run(&mut alice, &mut bob);
    assert!(discovered(&events, &bob.address).is_empty());
}

/// Key packages handed to the link for `peer`, by either send path.
fn key_packages_to(node: &Node, peer: &Node) -> usize {
    let is_kp = |m: &Message| {
        m.recipient.as_str() == peer.address
            && m.content.starts_with(internal_prefixes::KEY_PACKAGE)
    };
    node.transport
        .peer_sends()
        .iter()
        .filter(|(_, m)| is_kp(m))
        .count()
        + node
            .transport
            .sent_messages()
            .iter()
            .filter(|m| is_kp(m))
            .count()
}

#[test]
fn the_key_package_push_waits_until_the_peer_is_visible() {
    let key = publisher(1);
    let mut alice = Node::launch_with("alice", Some(key.verifying_key().to_bytes()), true);
    let cert = certificate(&key, &alice.address);
    alice
        .protocol
        .set_app_membership(&cert.to_bytes())
        .expect("alice's certificate");
    let mut bob = Node::member("bob", &key);
    let carol = Node::member("carol", &publisher(2));

    reach(&mut alice, &carol);
    reach(&mut alice, &bob);
    assert_eq!(key_packages_to(&alice, &carol), 0);
    assert_eq!(
        key_packages_to(&alice, &bob),
        0,
        "not before bob is visible"
    );

    // The exchange, keeping alice's other sends on the record.
    bob.link(&alice);
    for _ in 0..4 {
        let from_alice: Vec<_> = alice
            .transport
            .peer_sends()
            .into_iter()
            .filter(|(to, m)| {
                to == &bob.address && m.content.starts_with(internal_prefixes::MEMBER)
            })
            .collect();
        for (_, m) in from_alice {
            bob.receive_from(m, &alice.address.clone());
        }
        for (_, m) in bob.take_member_frames() {
            alice.receive_from(m, &bob.address.clone());
        }
        if alice.sees(&bob) {
            break;
        }
    }
    assert!(alice.sees(&bob));
    assert_eq!(
        key_packages_to(&alice, &bob),
        1,
        "pushed once bob is visible"
    );
    assert_eq!(key_packages_to(&alice, &carol), 0, "never to a stranger");
}

#[test]
fn presence_from_a_peer_the_application_does_not_see_is_not_reported() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    let carol = Node::member("carol", &publisher(2));
    let events = record_events(&mut alice);
    meet(&mut alice, &mut bob);

    let body = r#"{"status":"online","timestamp_ms":1}"#;
    alice.protocol.handle_presence_message(&carol.address, body);
    alice.protocol.handle_presence_message(&bob.address, body);
    let reported: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::PresenceUpdated {
                peer_id, status, ..
            } => {
                assert_eq!(*status, PresenceStatus::Online);
                Some(peer_id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(reported, vec![bob.address.clone()]);
}

// --- The inbound gate (step 7b) -----------------------------------------------

/// Every frame `node` handed to its transport for `peer`, by either send
/// path, then clears both records.
fn take_frames_to(node: &Node, peer: &Node) -> Vec<Message> {
    let mut out: Vec<Message> = node
        .transport
        .peer_sends()
        .into_iter()
        .map(|(_, m)| m)
        .chain(node.transport.sent_messages())
        .filter(|m| m.recipient.as_str() == peer.address)
        .collect();
    out.dedup_by(|a, b| a.id == b.id);
    node.transport.clear_peer_sends();
    node.transport.clear_sent_messages();
    out
}

/// Answers other than the membership exchange itself.
fn answers(frames: &[Message]) -> Vec<&Message> {
    frames
        .iter()
        .filter(|m| !m.content.starts_with(internal_prefixes::MEMBER))
        .collect()
}

/// `from` pushes its key package to `to`, which receives it.
fn push_key_package(from: &mut Node, to: &mut Node) -> Message {
    from.transport.add_connected_peer(to.address.clone(), -55);
    to.transport.add_connected_peer(from.address.clone(), -55);
    take_frames_to(from, to);
    from.protocol
        .push_key_package(&to.address, false, true)
        .expect("a key package goes out");
    let frame = take_frames_to(from, to)
        .into_iter()
        .find(|m| m.content.starts_with(internal_prefixes::KEY_PACKAGE))
        .expect("the key package frame");
    take_frames_to(to, from);
    to.receive_from(frame.clone(), &from.address.clone());
    frame
}

fn member_with_auto_kx(label: &str, key: &SigningKey) -> Node {
    let mut node = Node::launch_with(label, Some(key.verifying_key().to_bytes()), true);
    let cert = certificate(key, &node.address);
    node.protocol
        .set_app_membership(&cert.to_bytes())
        .expect("own certificate");
    node
}

#[test]
fn a_strangers_key_package_to_a_member_gets_nothing_back() {
    let mut alice = member_with_auto_kx("alice", &publisher(1));
    let mut carol = member_with_auto_kx("carol", &publisher(2));
    push_key_package(&mut carol, &mut alice);
    assert!(
        answers(&take_frames_to(&alice, &carol)).is_empty(),
        "no key package, Welcome, acknowledgement or reset to a stranger"
    );
}

#[test]
fn a_stranger_the_application_named_is_answered() {
    let mut alice = member_with_auto_kx("alice", &publisher(1));
    let mut carol = member_with_auto_kx("carol", &publisher(2));
    alice.protocol.name_peer(&carol.address);
    push_key_package(&mut carol, &mut alice);
    assert!(
        !answers(&take_frames_to(&alice, &carol)).is_empty(),
        "a named peer's key package is answered"
    );
}

#[test]
fn an_unconfigured_node_answers_as_before() {
    let mut alice = Node::launch_with("alice", None, true);
    let mut carol = member_with_auto_kx("carol", &publisher(2));
    push_key_package(&mut carol, &mut alice);
    assert!(!answers(&take_frames_to(&alice, &carol)).is_empty());
}

#[test]
fn two_members_who_never_met_converge_on_the_proof() {
    let key = publisher(1);
    let mut alice = member_with_auto_kx("alice", &key);
    let mut bob = member_with_auto_kx("bob", &key);
    let frame = push_key_package(&mut bob, &mut alice);
    assert!(
        frame
            .metadata
            .contains_key(offline_protocol_sealed::membership::MEMBER_PROOF_META_KEY),
        "a member's key package carries its proof"
    );
    assert!(!alice.sees(&bob), "never neighbours: no exchange ran");
    assert!(
        !answers(&take_frames_to(&alice, &bob)).is_empty(),
        "the proof admits bob, so alice answers"
    );
}

#[test]
fn a_proof_made_for_one_member_does_not_admit_its_sender_at_another() {
    let key = publisher(1);
    let mut alice = member_with_auto_kx("alice", &key);
    let mut bob = member_with_auto_kx("bob", &key);
    let mut dave = member_with_auto_kx("dave", &key);
    // An outsider holding no certificate: its key packages carry no proof.
    let mut mallory = Node::launch_with("mallory", Some(key.verifying_key().to_bytes()), true);

    // bob's proof, recorded on its way to alice.
    let to_alice = push_key_package(&mut bob, &mut alice);
    let stolen = to_alice
        .metadata
        .get(offline_protocol_sealed::membership::MEMBER_PROOF_META_KEY)
        .cloned()
        .expect("a proof");

    // bob's own frame to dave, carrying the proof made for alice instead of
    // its own. The metadata is outside the control signature, so the frame
    // passes the gate; the proof binds alice and alice's package.
    bob.transport.add_connected_peer(dave.address.clone(), -55);
    dave.transport.add_connected_peer(bob.address.clone(), -55);
    take_frames_to(&bob, &dave);
    bob.protocol
        .push_key_package(&dave.address, false, true)
        .expect("a key package");
    let mut to_dave = take_frames_to(&bob, &dave)
        .into_iter()
        .find(|m| m.content.starts_with(internal_prefixes::KEY_PACKAGE))
        .expect("the frame");
    to_dave.metadata.insert(
        offline_protocol_sealed::membership::MEMBER_PROOF_META_KEY.to_string(),
        stolen,
    );
    dave.receive_from(to_dave, &bob.address.clone());
    assert!(answers(&take_frames_to(&dave, &bob)).is_empty());

    // And a node with the key but no certificate proves nothing.
    push_key_package(&mut mallory, &mut dave);
    assert!(answers(&take_frames_to(&dave, &mallory)).is_empty());
}

#[test]
fn the_named_set_survives_a_restart_and_an_explicit_send_writes_it() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let carol = Node::member("carol", &publisher(2));
    assert!(!alice.protocol.app_sees_peer(&carol.address));

    alice
        .transport
        .add_connected_peer(carol.address.clone(), -55);
    let _ = alice
        .protocol
        .send_typing_indicator(&carol.address, "c", true);
    assert!(
        alice.protocol.app_sees_peer(&carol.address),
        "named by the app's own call"
    );

    alice.protocol.forget_named_peers_in_memory();
    assert!(!alice.protocol.app_sees_peer(&carol.address));
    alice.protocol.restore_named_peers();
    assert!(
        alice.protocol.app_sees_peer(&carol.address),
        "restored from storage"
    );
}

// --- Nothing names the application in the clear (step 9) ---------------------

/// A real 1:1 session between `a` and `b`, both advertising the sealed rich
/// payload, as a completed key package exchange would leave them.
fn session(a: &mut Node, b: &mut Node) {
    use crate::protocol::types::SessionState;
    let b_kp = {
        let manager = b.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = a.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package(&b.address, &b_kp.key_package_data)
            .unwrap();
        manager.create_session(&b.address).unwrap()
    };
    {
        let manager = b.protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    let (a_addr, b_addr) = (a.address.clone(), b.address.clone());
    for (node, peer) in [(&mut *a, b_addr), (&mut *b, a_addr)] {
        node.protocol.record_encryption_capable(&peer);
        node.protocol
            .persist_session_state(&peer, SessionState::Confirmed, "test")
            .unwrap();
        node.protocol.confirmed_sessions.insert(peer.clone());
        node.protocol.peer_rich_payload.insert(peer.clone());
        node.transport.add_connected_peer(peer, -55);
    }
}

fn received_app_ids(events: &Mutex<Vec<Event>>) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::MessageReceived { app_id, .. } => Some(app_id.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_configured_node_stamps_the_constant_and_seals_the_real_application_id() {
    let key = publisher(1);
    let mut alice = Node::member("alice", &key);
    let mut bob = Node::member("bob", &key);
    session(&mut alice, &mut bob);
    let events = record_events(&mut bob);
    take_frames_to(&alice, &bob);

    alice
        .protocol
        .send_message(&bob.address, "hello", None, None::<String>)
        .expect("sent");
    let frames = take_frames_to(&alice, &bob);
    let dm = frames
        .iter()
        .find(|m| !m.content.starts_with(internal_prefixes::MEMBER))
        .expect("the message went out")
        .clone();
    assert_eq!(
        dm.app_id.as_str(),
        offline_protocol_sealed::membership::WIRE_APP_ID,
        "the wire names no application"
    );
    assert!(
        !dm.content.contains(APP),
        "nor does the ciphertext's outside"
    );

    bob.receive_from(dm, &alice.address.clone());
    assert_eq!(received_app_ids(&events), vec![APP.to_string()]);
}

#[test]
fn an_unconfigured_node_stamps_its_own_application_id_as_before() {
    let mut alice = Node::launch("alice", None);
    let mut bob = Node::launch("bob", None);
    session(&mut alice, &mut bob);
    take_frames_to(&alice, &bob);
    alice
        .protocol
        .send_message(&bob.address, "hello", None, None::<String>)
        .expect("sent");
    let frames = take_frames_to(&alice, &bob);
    assert!(frames
        .iter()
        .filter(|m| !m.content.starts_with(internal_prefixes::MEMBER))
        .all(|m| m.app_id.as_str() == APP));
}

// --- Every neighbour report is the core's decision (step 8) -------------------

/// The source of `file` without its inline test modules.
fn shipped_source(file: &std::path::Path) -> String {
    let source = std::fs::read_to_string(file).unwrap_or_default();
    let mut cut = source.len();
    let mut from = 0;
    while let Some(at) = source[from..].find("#[cfg(test)]\nmod ") {
        let at = from + at;
        let line_end = source[at + 13..]
            .find('\n')
            .map_or(source.len(), |n| at + 13 + n);
        if source[at..line_end].trim_end().ends_with('{') {
            cut = at;
            break;
        }
        from = line_end;
    }
    source[..cut].to_string()
}

/// A report of a neighbour reaches the application only through the core's
/// gate: `neighbor_reachable`, `neighbor_unreachable` or `app_sees_peer`, or
/// from the core's own release once a held peer turns visible. A host that
/// emits one on its own would show an application a peer it never admitted,
/// and every test above would still pass, because they drive the core.
///
/// The check is textual: each construction of `NeighborDiscovered`,
/// `NeighborLost` or `PeerRssi` outside the event definitions and telemetry
/// must be handed straight to an emitter, with a gate call in the lines
/// before it. A new site either takes the gate or fails here.
#[test]
fn every_neighbour_report_passes_the_core_gate() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if !crates.join("offline-protocol-uniffi").is_dir() {
        return;
    }
    const REPORTS: [&str; 3] = [
        "Event::NeighborDiscovered {",
        "Event::NeighborLost {",
        "Event::PeerRssi {",
    ];
    const GATES: [&str; 4] = [
        "neighbor_reachable(",
        "neighbor_unreachable(",
        "app_sees_peer(",
        "fn release_held_discovery(",
    ];
    const WINDOW: usize = 14;

    let mut sites = 0;
    let mut offenders = Vec::new();
    let mut stack: Vec<_> = [
        "offline-protocol",
        "offline-protocol-uniffi",
        "offline-protocol-web",
    ]
    .iter()
    .map(|name| crates.join(name).join("src"))
    .collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" && name != "telemetry" {
                    stack.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "events.rs" {
                continue;
            }
            let source = shipped_source(&path);
            let lines: Vec<&str> = source.lines().collect();
            for (n, line) in lines.iter().enumerate() {
                if !REPORTS.iter().any(|report| line.contains(report)) {
                    continue;
                }
                sites += 1;
                let before = &lines[n.saturating_sub(WINDOW)..n];
                let emitted = lines[n.saturating_sub(2)..=n]
                    .iter()
                    .any(|l| l.contains("emit_event(") || l.contains("push_event("));
                let gated = before.iter().any(|l| GATES.iter().any(|g| l.contains(g)));
                if !emitted || !gated {
                    offenders.push(format!("{}:{}", path.display(), n + 1));
                }
            }
        }
    }
    // The FFI crate's six, the browser's five and the core's release: a
    // count that drops means the scan stopped seeing them, not that they went.
    assert!(sites >= 12, "found only {sites} neighbour reports");
    assert!(
        offenders.is_empty(),
        "a neighbour report the core did not decide (ADR 0037): {offenders:#?}"
    );
}

/// Every frame addressed to this node enters through one loop, and the
/// membership gate sits in it ahead of the first side effect. Data sync,
/// presence, key packages and acknowledgements are then all answered under
/// the same predicate, with no list of their own to keep in step. A second
/// reader of the transport would be a second entry with no gate.
#[test]
fn frames_for_this_node_have_one_entry_and_it_is_gated() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut readers = Vec::new();
    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    stack.push(path);
                }
                continue;
            }
            if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
                let count = shipped_source(&path)
                    .matches("transport_manager.receive()")
                    .count();
                readers.extend(std::iter::repeat_n(path.clone(), count));
            }
        }
    }
    assert_eq!(
        readers,
        vec![src.join("protocol").join("receive.rs")],
        "frames for this node must have exactly one entry"
    );

    let receive = shipped_source(&src.join("protocol").join("receive.rs"));
    let entry = receive.find("transport_manager.receive()").unwrap();
    let gate = receive[entry..]
        .find("inbound_admitted(")
        .expect("the entry loop runs the membership gate");
    let first_effect = receive[entry..]
        .find("lamport_clock.value()")
        .expect("the entry loop merges the sender's clock");
    assert!(
        gate < first_effect,
        "the gate runs before the first side effect"
    );
}

#[path = "membership_discovery.rs"]
mod discovery;
