//! Ephemeral sends (ADR 0032): once over a live direct link, never retried,
//! and never allowed to strand a group member's ratchet.
//!
//! Every group frame here is encrypted for a real three-member MLS group
//! and decrypted by the real group handler, because the property that makes
//! an ephemeral group send safe (a member who missed some can still read
//! the next reliable one) is a property of the ratchet, not of the code
//! that counts.

use std::sync::{Arc, Mutex};

use offline_protocol_core::Message;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::group_mesh::{
    GroupSendOptions, RosterRatchetGap, MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS,
};
use crate::mls::InMemoryStorage;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::{OfflineProtocol, SendMessageOptions, TestProtocolStateStorage};
use crate::{Error, Event};

struct Member {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Member {
    fn new(label: &str, encrypted: bool) -> Self {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = encrypted;
        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        if encrypted {
            let secure = crate::test_identity::seeded_storage(label);
            let state = Arc::new(InMemoryStorage::new());
            protocol
                .initialize_mls(
                    secure,
                    Arc::new(TestProtocolStateStorage { storage: state }),
                )
                .expect("initialize_mls");
        }
        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
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
        }
    }

    fn link(&self, other: &Member) {
        self.transport
            .add_connected_peer(other.address.clone(), -40);
    }

    fn unlink(&self, other: &Member) {
        self.transport.remove_connected_peer(&other.address);
    }

    /// Takes everything this member put on its transport.
    fn take_sent(&self) -> Vec<Message> {
        let sent = self.transport.sent_messages();
        self.transport.clear_sent_messages();
        sent
    }

    fn received_group_message(&self, content: &str) -> bool {
        self.events.lock().unwrap().iter().any(
            |event| matches!(event, Event::GroupMessageReceived { content: c, .. } if c == content),
        )
    }

    fn deliver(&mut self, frames: &[Message]) {
        for frame in frames {
            if frame.recipient.as_str() == self.address {
                self.transport.queue_message(frame.clone());
            }
        }
        while self.protocol.receive_message().is_some() {}
    }
}

/// A real three-member MLS group, built through the MLS manager.
fn trio() -> (Member, Member, Member, String) {
    let mut alice = Member::new("alice", true);
    let mut bob = Member::new("bob", true);
    let mut carol = Member::new("carol", true);
    let group_id = alice
        .protocol
        .create_group("table")
        .expect("create")
        .group_id
        .as_str()
        .to_string();
    let gid = offline_protocol_mls::GroupId::new(&group_id).unwrap();
    add_member(&alice, &bob, &gid, &[]);
    add_member(&alice, &carol, &gid, &[&bob]);
    alice.protocol.refresh_group_members(&group_id).unwrap();
    let roster = vec![
        alice.address.clone(),
        bob.address.clone(),
        carol.address.clone(),
    ];
    for member in [&mut bob, &mut carol] {
        member
            .protocol
            .group_mesh
            .members
            .insert(group_id.clone(), roster.clone());
    }
    (alice, bob, carol, group_id)
}

fn add_member(
    adder: &Member,
    joiner: &Member,
    gid: &offline_protocol_mls::GroupId,
    existing: &[&Member],
) {
    let kp = {
        let mls = joiner.protocol.mls_manager_for_testing().read().unwrap();
        mls.generate_key_package().unwrap()
    };
    let (welcome, commit) = {
        let mls = adder.protocol.mls_manager_for_testing().read().unwrap();
        mls.add_group_member(gid, &joiner.address, &kp.key_package_data)
            .unwrap()
    };
    {
        let mls = joiner.protocol.mls_manager_for_testing().read().unwrap();
        mls.join_group(&welcome).unwrap();
    }
    for member in existing {
        let mls = member.protocol.mls_manager_for_testing().read().unwrap();
        let encrypted = offline_protocol_mls::EncryptedMessage {
            group_id: gid.clone(),
            message_type: offline_protocol_mls::MlsMessageType::Commit,
            epoch: commit.epoch,
            ciphertext: commit.ciphertext.clone(),
            sender_id: adder.address.clone(),
            timestamp_ms: 0,
        };
        mls.decrypt_from_group(&encrypted, &adder.address)
            .expect("merge the add commit");
    }
}

fn ephemeral() -> GroupSendOptions {
    GroupSendOptions {
        ephemeral: true,
        ..Default::default()
    }
}

fn gap(member: &Member, group: &str) -> Option<RosterRatchetGap> {
    member
        .protocol
        .group_mesh
        .roster_invisible_generations
        .get(group)
        .copied()
}

/// What the sender keeps for a frame after the call returns.
fn kept_state(member: &Member) -> (usize, usize) {
    (
        member.protocol.outbox.len(),
        member.protocol.retry_queue.len(),
    )
}

#[test]
fn the_first_ephemeral_group_send_of_a_process_goes_out_reliable() {
    // The ratchet-gap count is not persisted, so a process cannot see the
    // gap it inherited. Same rule as the directed replication frames.
    let (mut alice, bob, carol, group) = trio();
    alice.link(&bob);
    alice.link(&carol);
    alice
        .protocol
        .send_group_message_with(&group, "first", ephemeral())
        .expect("send");
    let sent = alice.take_sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        sent.iter().all(|frame| frame.requires_ack),
        "the first send has to be a rung every member can count on"
    );
    assert_eq!(gap(&alice, &group).map(|g| g.invisible), Some(0));
}

#[test]
fn an_ephemeral_group_send_reaches_linked_members_and_keeps_nothing() {
    let (mut alice, mut bob, mut carol, group) = trio();
    alice.link(&bob);
    alice.link(&carol);
    alice
        .protocol
        .send_group_message_with(&group, "warm up", ephemeral())
        .expect("send");
    let warm = alice.take_sent();
    bob.deliver(&warm);
    carol.deliver(&warm);
    let before = kept_state(&alice);

    alice
        .protocol
        .send_group_message_with(&group, "ball at 3,4", ephemeral())
        .expect("send");
    let sent = alice.take_sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent.iter().all(|frame| !frame.requires_ack), "{sent:?}");
    assert_eq!(
        kept_state(&alice),
        before,
        "an ephemeral frame has no outbox entry and no retry"
    );
    // Acknowledgements Bob still owes for the reliable warm-up.
    bob.protocol.process().expect("process");
    bob.take_sent();
    bob.deliver(&sent);
    carol.deliver(&sent);
    assert!(bob.received_group_message("ball at 3,4"));
    assert!(carol.received_group_message("ball at 3,4"));
    bob.protocol.process().expect("process");
    let ephemeral_ids: Vec<String> = sent.iter().map(|f| f.id.to_string()).collect();
    let acks: Vec<Message> = bob
        .take_sent()
        .into_iter()
        .filter(|f| {
            f.metadata
                .get("ack_for")
                .is_some_and(|id| ephemeral_ids.contains(id))
        })
        .collect();
    assert!(
        acks.is_empty(),
        "the receiver acknowledges nothing it was not asked to: {acks:?}"
    );
    assert_eq!(gap(&alice, &group).map(|g| g.invisible), Some(0));
}

#[test]
fn an_ephemeral_group_send_with_no_linked_member_is_refused_before_encrypting() {
    let (mut alice, _bob, _carol, group) = trio();
    let result = alice
        .protocol
        .send_group_message_with(&group, "nobody hears", ephemeral());
    assert!(
        matches!(
            result,
            Err(Error::Transport(
                offline_protocol_transport::Error::PeerNotReachable(_)
            ))
        ),
        "{result:?}"
    );
    assert!(alice.take_sent().is_empty());
    assert!(
        gap(&alice, &group).is_none(),
        "nothing was encrypted, so no generation was spent"
    );
}

#[test]
fn a_member_who_misses_ephemeral_sends_gets_a_reliable_rung_when_the_budget_is_spent() {
    let (mut alice, mut bob, mut carol, group) = trio();
    alice.link(&bob);
    alice.link(&carol);
    alice
        .protocol
        .send_group_message_with(&group, "warm up", ephemeral())
        .expect("send");
    let warm = alice.take_sent();
    bob.deliver(&warm);
    carol.deliver(&warm);

    // Carol walks out of range. Bob keeps playing.
    alice.unlink(&carol);
    for n in 0..5 {
        alice
            .protocol
            .send_group_message_with(&group, &format!("tick {n}"), ephemeral())
            .expect("bob is linked");
    }
    let ticks = alice.take_sent();
    assert_eq!(ticks.len(), 5);
    assert!(ticks.iter().all(|f| f.recipient.as_str() == bob.address));
    bob.deliver(&ticks);
    assert_eq!(
        gap(&alice, &group).map(|g| g.invisible),
        Some(5),
        "each frame Carol missed still moved the ratchet she will need"
    );

    // What 256 missed frames would have done.
    let epoch = gap(&alice, &group).unwrap().epoch;
    alice
        .protocol
        .group_mesh
        .roster_invisible_generations
        .insert(
            group.clone(),
            RosterRatchetGap {
                epoch,
                invisible: MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS,
            },
        );
    alice
        .protocol
        .send_group_message_with(&group, "rung", ephemeral())
        .expect("send");
    let rung = alice.take_sent();
    let to_carol: Vec<&Message> = rung
        .iter()
        .filter(|f| f.recipient.as_str() == carol.address)
        .collect();
    assert!(
        !to_carol.is_empty() && to_carol.iter().all(|f| f.requires_ack),
        "with the budget spent the send is reliable, so Carol's copy waits \
         for her: {rung:?}"
    );
    assert_eq!(gap(&alice, &group).map(|g| g.invisible), Some(0));

    // Carol comes back and the rung decrypts across the frames she missed.
    carol.deliver(&rung);
    assert!(
        carol.received_group_message("rung"),
        "{:?}",
        carol.events.lock().unwrap()
    );
}

#[test]
fn an_ephemeral_group_send_refuses_rich_options() {
    let (mut alice, bob, _carol, group) = trio();
    alice.link(&bob);
    let result = alice.protocol.send_group_message_with(
        &group,
        "x",
        GroupSendOptions {
            ephemeral: true,
            reply_to_msg: Some("a".into()),
            ..Default::default()
        },
    );
    assert!(
        matches!(result, Err(Error::InvalidArgument(_))),
        "{result:?}"
    );
    assert!(alice.take_sent().is_empty());
}

fn ephemeral_dm() -> SendMessageOptions {
    SendMessageOptions {
        ephemeral: true,
        ..Default::default()
    }
}

#[test]
fn an_ephemeral_direct_message_goes_once_over_the_link_and_keeps_nothing() {
    let mut alice = Member::new("alice", false);
    let bob = Member::new("bob", false);
    alice.link(&bob);
    alice
        .protocol
        .send_message_with(bob.address.clone(), "paddle 12", ephemeral_dm())
        .expect("send");
    let sent = alice.take_sent();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].requires_ack);
    assert_eq!(kept_state(&alice), (0, 0));
    assert!(alice.events.lock().unwrap().iter().any(|e| matches!(
        e,
        Event::MessageSent {
            requires_ack: false,
            ..
        }
    )));

    // A failed attempt is the caller's to repeat or drop, never a retry.
    alice.transport.set_fail_next_sends(1);
    let failed = alice
        .protocol
        .send_message_with(bob.address.clone(), "paddle 13", ephemeral_dm());
    assert!(failed.is_err());
    alice.protocol.process().expect("process");
    assert!(alice.take_sent().is_empty());
    assert_eq!(kept_state(&alice), (0, 0));
}

#[test]
fn an_ephemeral_direct_message_needs_a_live_link() {
    let mut alice = Member::new("alice", false);
    let bob = Member::new("bob", false);
    let result = alice
        .protocol
        .send_message_with(bob.address.clone(), "anyone?", ephemeral_dm());
    assert!(
        matches!(
            result,
            Err(Error::Transport(
                offline_protocol_transport::Error::PeerNotReachable(_)
            ))
        ),
        "{result:?}"
    );
    assert!(alice.take_sent().is_empty());
    assert_eq!(kept_state(&alice), (0, 0));
}

#[test]
fn an_ephemeral_direct_message_without_a_session_is_refused_not_queued() {
    let mut alice = Member::new("alice", true);
    let bob = Member::new("bob", true);
    alice.link(&bob);
    let result = alice
        .protocol
        .send_message_with(bob.address.clone(), "too soon", ephemeral_dm());
    assert!(
        matches!(result, Err(Error::SessionNotReady(_))),
        "{result:?}"
    );
    assert!(alice.protocol.pending_encrypted_messages.is_empty());
    assert!(alice.take_sent().is_empty());
}

#[test]
fn an_ephemeral_direct_message_never_leaves_as_plaintext_by_default() {
    // SEC-M3: with MLS uninitialized the default refuses rather than sending
    // plaintext, and an ephemeral send is held to the same rule.
    // Encryption on, as the default requires, and MLS never initialized.
    let config = crate::ProtocolConfig::new("test-app", "alice");
    let mut protocol = OfflineProtocol::new(config).expect("protocol");
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().expect("transport start");
    let bob = id("bob");
    mock.add_connected_peer(bob.clone(), -40);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock.clone()));
    protocol.start().expect("start");
    let result = protocol.send_message_with(bob, "in the clear?", ephemeral_dm());
    assert!(matches!(result, Err(Error::EncryptFailed(_))), "{result:?}");
    assert!(mock.sent_messages().is_empty());
}

#[test]
fn an_ephemeral_direct_message_refuses_rich_options() {
    let mut alice = Member::new("alice", false);
    let bob = Member::new("bob", false);
    alice.link(&bob);
    let result = alice.protocol.send_message_with(
        bob.address.clone(),
        "x",
        SendMessageOptions {
            ephemeral: true,
            reply_to_msg: Some("m".into()),
            ..Default::default()
        },
    );
    assert!(
        matches!(result, Err(Error::InvalidArgument(_))),
        "{result:?}"
    );
    assert!(alice.take_sent().is_empty());
}
