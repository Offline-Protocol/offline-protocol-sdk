//! Replication across a group space, over the real group send and receive
//! paths.
//!
//! The sibling of `data_sync.rs`, and the same standard applies: every frame
//! here is encrypted for a real MLS group, carried by a transport, and
//! dispatched by the group message handler. What is new in this file is that
//! a space now has more than one other replica in it, so the questions are
//! different — one ciphertext has to serve the roster, an answer has to
//! reach one member rather than all of them, and a member that cannot
//! intercept these frames must never be sent one.

use std::sync::{Arc, Mutex};

use offline_protocol_data::DataValue;
use offline_protocol_transport::{MockTransport, Transport, TransportType};

use crate::group_mesh::{RosterRatchetGap, MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS};
use crate::mls::InMemoryStorage;
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::types::{DATA_GROUP_V1, DATA_SYNC_V1};
use crate::protocol::{OfflineProtocol, TestProtocolStateStorage};

/// One member of the group, with the transport its frames go through.
struct Member {
    protocol: OfflineProtocol,
    transport: MockTransport,
    address: String,
    events: Arc<Mutex<Vec<crate::Event>>>,
}

impl Member {
    fn new(label: &str) -> Self {
        let mut config = create_test_config_for_user(label);
        config.encryption.enabled = true;
        config.data.enabled = true;

        let mut protocol = OfflineProtocol::new(config).expect("protocol");
        let secure = crate::test_identity::seeded_storage(label);
        let state = Arc::new(InMemoryStorage::new());
        protocol
            .initialize_mls(
                secure,
                Arc::new(TestProtocolStateStorage { storage: state }),
            )
            .expect("initialize_mls");

        let mock = MockTransport::new(TransportType::BLE);
        mock.start().expect("transport start");
        let transport = mock.clone();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock));

        let events: Arc<Mutex<Vec<crate::Event>>> = Arc::new(Mutex::new(Vec::new()));
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

    /// Whether the application was handed a group message.
    ///
    /// The thing a missed interception looks like: the frame decrypts, and
    /// because nothing recognised it the group handler emits it as a chat
    /// message whose body is literal `__DATA_V1__` JSON.
    fn saw_group_message(&self) -> bool {
        self.events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, crate::Event::GroupMessageReceived { .. }))
    }
}

/// A real three-member MLS group, with every member known group-capable.
///
/// Built through the MLS manager rather than through `invite_to_group` so
/// the fixture is the group itself and not the invite path: the triggers
/// that ride an invite have their own tests, and a fixture that depended on
/// them could not tell a broken trigger from a broken group.
fn trio() -> (Member, Member, Member, String) {
    let mut alice = Member::new("alice");
    let mut bob = Member::new("bob");
    let mut carol = Member::new("carol");

    let group_id = {
        let info = alice.protocol.create_group("notes").expect("create");
        info.group_id.as_str().to_string()
    };
    let gid = offline_protocol_mls::GroupId::new(&group_id).unwrap();

    // Bob first, then Carol. Adding Carol advances the group epoch, and Bob
    // has to merge that commit or Alice's later ciphertext is one epoch
    // ahead of him — which is a real condition the protocol handles by
    // buffering, and which would make this fixture silently test the
    // buffering path instead of replication.
    add_member(&alice, &bob, &gid, &[]);
    add_member(&alice, &carol, &gid, &[&bob]);

    // Every member has to agree on the roster, and Bob has to learn Carol
    // (and vice versa) the way a real join does: from the group state, not
    // from this fixture's knowledge of it.
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

    // What an inviter's attestation leaves behind on each member. The
    // attestation plumbing that produces it has its own tests; these are
    // about what happens once every member is known capable.
    let (a, b, c) = (
        alice.address.clone(),
        bob.address.clone(),
        carol.address.clone(),
    );
    for (member, others) in [
        (&mut alice, [&b, &c]),
        (&mut bob, [&a, &c]),
        (&mut carol, [&a, &b]),
    ] {
        for other in others {
            member.protocol.peer_data_group.insert(other.clone());
        }
    }

    (alice, bob, carol, group_id)
}

/// Add `joiner` to the group, merging the resulting commit into every
/// member that is already in it.
///
/// `existing` are the members other than the adder, who merges its own
/// commit as part of producing it.
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
        // A commit returns `Ok(None)`: MLS consumed it and advanced the
        // epoch rather than producing application data.
        mls.decrypt_from_group(&encrypted, &adder.address)
            .expect("merge the add commit");
    }
}

/// Carry everything `from` sent to whichever of `to` it was addressed to.
fn pump(from: &mut Member, to: &mut [&mut Member]) -> usize {
    let messages = from.transport.sent_messages();
    from.transport.clear_sent_messages();
    let mut moved = 0;
    for message in messages {
        for peer in to.iter_mut() {
            if message.recipient.as_str() == peer.address {
                peer.transport.queue_message(message.clone());
                moved += 1;
            }
        }
    }
    for peer in to.iter_mut() {
        while peer.protocol.receive_message().is_some() {}
    }
    moved
}

/// Run the group to quiescence, returning the frames carried each round.
fn settle(alice: &mut Member, bob: &mut Member, carol: &mut Member) -> Vec<usize> {
    let mut rounds = Vec::new();
    for _ in 0..8 {
        let mut carried = 0;
        // Split borrows: each pump needs the sender mutably and the others
        // mutably, which cannot overlap.
        carried += {
            let (b, c) = (&mut *bob, &mut *carol);
            pump(alice, &mut [b, c])
        };
        carried += {
            let (a, c) = (&mut *alice, &mut *carol);
            pump(bob, &mut [a, c])
        };
        carried += {
            let (a, b) = (&mut *alice, &mut *bob);
            pump(carol, &mut [a, b])
        };
        rounds.push(carried);
        if carried == 0 {
            break;
        }
    }
    rounds
}

fn write(member: &mut Member, space: &str, doc: &str, key: &str, value: &str) {
    member
        .protocol
        .data_map_set(space, doc, "m", key, DataValue::text(value))
        .expect("set");
    member.protocol.data_flush(space, doc).expect("flush");
}

fn read(member: &mut Member, space: &str, doc: &str, key: &str) -> Option<DataValue> {
    member
        .protocol
        .data_map_get(space, doc, "m", key)
        .expect("get")
}

/// How many of the frames a member sent are replication frames.
///
/// Counted by decrypting them as the recipient would, because a group frame
/// is opaque on the wire: `__GRP_MLS_MSG__` is all an observer sees whether
/// it carries a chat message or a document change.
fn data_frames_sent(from: &Member, to: &mut Member, group_id: &str) -> usize {
    from.transport
        .sent_messages()
        .iter()
        .filter(|message| message.recipient.as_str() == to.address)
        .filter(|message| {
            let Some(payload) = message
                .content
                .strip_prefix(internal_prefixes::GROUP_MLS_MSG)
            else {
                return false;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
                return false;
            };
            let Some(ciphertext) = value.get("ciphertext").and_then(|c| c.as_str()) else {
                return false;
            };
            let Ok(bytes) = crate::protocol::base64_decode(ciphertext) else {
                return false;
            };
            let mls = to.protocol.mls_manager_for_testing().read().unwrap();
            let gid = offline_protocol_mls::GroupId::new(group_id).unwrap();
            let encrypted = offline_protocol_mls::EncryptedMessage {
                group_id: gid,
                message_type: offline_protocol_mls::MlsMessageType::Application,
                epoch: 0,
                ciphertext: bytes,
                sender_id: from.address.clone(),
                timestamp_ms: 0,
            };
            matches!(
                mls.decrypt_from_group(&encrypted, &from.address),
                Ok(Some(plaintext))
                    if String::from_utf8_lossy(&plaintext)
                        .starts_with(internal_prefixes::DATA_V1)
            )
        })
        .count()
}

#[test]
fn a_change_written_in_a_group_reaches_every_member() {
    let (mut alice, mut bob, mut carol, group) = trio();

    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);

    for (label, member) in [("bob", &mut bob), ("carol", &mut carol)] {
        assert_eq!(
            read(member, &group, "notes", "title"),
            Some(DataValue::text("hello")),
            "{label} never received the change"
        );
    }
}

#[test]
fn a_group_change_is_encrypted_once_for_the_whole_roster() {
    // The headline property of a group space, and the reason it is not N
    // 1:1 spaces: one MLS encryption serves everyone. A regression here
    // would not fail any convergence test — every member would still
    // receive the change — so it is asserted directly.
    let (mut alice, mut bob, mut carol, group) = trio();

    write(&mut alice, &group, "notes", "title", "hello");

    let to_bob = data_frames_sent(&alice, &mut bob, &group);
    let to_carol = data_frames_sent(&alice, &mut carol, &group);
    assert_eq!(
        (to_bob, to_carol),
        (1, 1),
        "one commit must produce exactly one frame per member"
    );

    let ciphertexts: Vec<String> = alice
        .transport
        .sent_messages()
        .iter()
        .filter_map(|m| {
            let payload = m.content.strip_prefix(internal_prefixes::GROUP_MLS_MSG)?;
            let value: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(value.get("ciphertext")?.as_str()?.to_string())
        })
        .collect();
    assert_eq!(ciphertexts.len(), 2, "one copy addressed to each member");
    assert_eq!(
        ciphertexts[0], ciphertexts[1],
        "both members must be handed the SAME ciphertext: encrypting per \
         member would make a group of N cost N encryptions and N times the \
         key material, which is the whole reason a group space exists"
    );
}

#[test]
fn a_change_received_from_the_group_is_not_pushed_on() {
    // Without this rule every member re-broadcasts every change to every
    // other member: one edit becomes N^2 frames, and the group gets slower
    // the more people are in it. The change already reached everyone —
    // that is what one group ciphertext does.
    let (mut alice, mut bob, mut carol, group) = trio();

    write(&mut alice, &group, "notes", "title", "hello");

    // Carry Alice's frame in, then look at what Bob does with it — before
    // any further settling, because a settle would let the traffic this
    // test is looking for be absorbed and disappear.
    {
        let (b, c) = (&mut bob, &mut carol);
        pump(&mut alice, &mut [b, c]);
    }
    assert_eq!(
        read(&mut bob, &group, "notes", "title"),
        Some(DataValue::text("hello")),
        "the change has to have arrived for its echo to be meaningful"
    );

    let echoed_to_carol = data_frames_sent(&bob, &mut carol, &group);
    assert_eq!(
        echoed_to_carol, 0,
        "Bob re-broadcast a change he received from the group; Carol \
         already had it from Alice's own frame"
    );
}

#[test]
fn a_member_that_cannot_intercept_group_frames_is_never_sent_one() {
    // The compatibility trap `DATA_GROUP_V1` exists for. An install that
    // speaks only the 1:1 version holds group sessions and has no group
    // interception at all, so a replication frame sent into its group
    // surfaces to its user as literal `__DATA_V1__` text. One ciphertext
    // serves the roster, so one such member has to close the gate for
    // everyone.
    let (mut alice, mut bob, mut carol, group) = trio();
    alice.protocol.peer_data_group.remove(&carol.address);

    write(&mut alice, &group, "notes", "title", "hello");

    assert_eq!(
        data_frames_sent(&alice, &mut bob, &group),
        0,
        "the gate is all-members: a frame Bob can read is delivered to \
         Carol too, and she would render it as text"
    );

    settle(&mut alice, &mut bob, &mut carol);
    assert_eq!(
        read(&mut bob, &group, "notes", "title"),
        None,
        "nothing may replicate while a member of unknown capability is in \
         the roster"
    );
}

#[test]
fn the_group_capability_is_independent_of_the_one_to_one_one() {
    // A peer from before group spaces advertises `[1]` and replicates 1:1
    // perfectly well. Collapsing the two entries into one flag would either
    // send that peer a group frame it renders as text, or stop replicating
    // with it entirely.
    let mut protocol = OfflineProtocol::new({
        let mut config = create_test_config_for_user("alice");
        config.encryption.enabled = true;
        config.data.enabled = true;
        config
    })
    .unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    protocol.peer_data_sync.insert(id("legacy"));
    assert!(
        protocol.data_sync_active(&id("legacy")),
        "1:1 replication must work with a peer that predates group spaces"
    );
    assert!(
        !protocol.group_data_sync_active(&[id("alice"), id("legacy")]),
        "the same peer must hold the group gate closed"
    );

    protocol.peer_data_group.insert(id("current"));
    protocol.peer_data_sync.insert(id("current"));
    assert!(protocol.group_data_sync_active(&[id("alice"), id("current")]));
}

#[test]
fn a_group_space_is_named_by_its_group_id() {
    // The group id carries a colon, which the document charset forbids and
    // the space charset allows for exactly this reason. If a space could
    // not be named by its scope, a group space would need a second
    // translated name and something would have to keep the two consistent.
    let (mut alice, _bob, _carol, group) = trio();
    assert!(group.starts_with("group:"), "group ids carry a colon");

    write(&mut alice, &group, "notes", "title", "hello");
    assert_eq!(
        read(&mut alice, &group, "notes", "title"),
        Some(DataValue::text("hello"))
    );
    assert!(
        alice.protocol.data_list_spaces().unwrap().contains(&group),
        "the space has to be listed under the name it was written with"
    );
}

#[test]
fn a_joining_member_catches_up_on_documents_written_before_it_arrived() {
    // The roster-change half of the item: a member that joins a group with
    // history has to receive it, and the only thing that knows the history
    // exists is the member holding it.
    let (mut alice, mut bob, mut carol, group) = trio();

    // Carol has not yet heard of this document.
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);

    // A fresh member with no knowledge of the space at all, standing in for
    // one that has just been welcomed.
    let mut dave = Member::new("dave");
    let gid = offline_protocol_mls::GroupId::new(&group).unwrap();
    let kp = {
        let mls = dave.protocol.mls_manager_for_testing().read().unwrap();
        mls.generate_key_package().unwrap()
    };
    let (welcome, _commit) = {
        let mls = alice.protocol.mls_manager_for_testing().read().unwrap();
        mls.add_group_member(&gid, &dave.address, &kp.key_package_data)
            .unwrap()
    };
    {
        let mls = dave.protocol.mls_manager_for_testing().read().unwrap();
        mls.join_group(&welcome).unwrap();
    }
    alice.protocol.refresh_group_members(&group).unwrap();
    dave.protocol.group_mesh.members.insert(
        group.clone(),
        vec![
            alice.address.clone(),
            bob.address.clone(),
            carol.address.clone(),
            dave.address.clone(),
        ],
    );
    for other in [&bob.address, &carol.address, &dave.address] {
        alice.protocol.peer_data_group.insert(other.clone());
    }
    for other in [&alice.address, &bob.address, &carol.address] {
        dave.protocol.peer_data_group.insert(other.clone());
    }

    // What the inviter does on a real invite: offer the newcomer what this
    // device holds.
    alice
        .protocol
        .kick_group_data_sync(&group, &dave.address, "member_added");

    for _ in 0..6 {
        let mut carried = 0;
        carried += {
            let (d, b) = (&mut dave, &mut bob);
            pump(&mut alice, &mut [d, b])
        };
        carried += {
            let (a, b) = (&mut alice, &mut bob);
            pump(&mut dave, &mut [a, b])
        };
        if carried == 0 {
            break;
        }
    }

    assert_eq!(
        read(&mut dave, &group, "notes", "title"),
        Some(DataValue::text("hello")),
        "a member that joined after the writing never received the document"
    );
}

#[test]
fn an_answer_goes_to_the_member_that_asked_and_to_nobody_else() {
    // Anti-entropy between two members is still a conversation between two
    // devices. Broadcasting the answer would have every other member
    // decrypt and import a change they already had, and on a mesh that is
    // the traffic the whole design is trying not to spend.
    let (mut alice, mut bob, mut carol, group) = trio();

    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);

    // Bob asks Alice for her versions. Only Alice should answer, and only
    // to Bob.
    bob.protocol
        .kick_group_data_sync(&group, &alice.address, "test_offer");
    {
        let (a, c) = (&mut alice, &mut carol);
        pump(&mut bob, &mut [a, c])
    };

    assert_eq!(
        data_frames_sent(&alice, &mut carol, &group),
        0,
        "Carol was sent an answer to a question Bob asked"
    );
}

#[test]
fn edits_made_apart_converge_across_the_group_and_the_exchange_terminates() {
    let (mut alice, mut bob, mut carol, group) = trio();

    // Give every replica the document, so all three can edit it while
    // apart.
    write(&mut alice, &group, "notes", "a", "1");
    settle(&mut alice, &mut bob, &mut carol);

    // Partitioned: each writes a different key and nothing moves.
    write(&mut alice, &group, "notes", "from_alice", "x");
    write(&mut bob, &group, "notes", "from_bob", "y");
    write(&mut carol, &group, "notes", "from_carol", "z");
    alice.transport.clear_sent_messages();
    bob.transport.clear_sent_messages();
    carol.transport.clear_sent_messages();

    // Reconnect: one member sweeps the others.
    alice
        .protocol
        .kick_group_data_sync(&group, &bob.address, "reconnect");
    alice
        .protocol
        .kick_group_data_sync(&group, &carol.address, "reconnect");
    bob.protocol
        .kick_group_data_sync(&group, &carol.address, "reconnect");

    let rounds = settle(&mut alice, &mut bob, &mut carol);

    for (label, member) in [
        ("alice", &mut alice),
        ("bob", &mut bob),
        ("carol", &mut carol),
    ] {
        for key in ["from_alice", "from_bob", "from_carol"] {
            assert!(
                read(member, &group, "notes", key).is_some(),
                "{label} is missing {key} after reconnecting"
            );
        }
    }

    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange has to stop: replicas that answer each other's \
         answers converge and then keep talking forever (rounds: {rounds:?})"
    );
}

#[test]
fn group_replication_stops_when_the_layer_is_switched_off() {
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    alice.transport.clear_sent_messages();

    alice.protocol.config.data.enabled = false;

    // The switch refuses the edit itself, which is the layer's shipped
    // behaviour and not what this test is about.
    assert!(
        alice
            .protocol
            .data_map_set(&group, "notes", "m", "title", DataValue::text("again"))
            .is_err(),
        "a switched-off layer refuses writes"
    );

    // What it is about: nothing goes out on the group path either. A sweep
    // is the one thing that could still produce a frame without a local
    // edit behind it.
    alice
        .protocol
        .kick_group_data_sync(&group, &bob.address, "kill_switch");
    assert_eq!(
        data_frames_sent(&alice, &mut bob, &group),
        0,
        "the kill switch has to gate the group path too"
    );
}

/// Encrypt one replication frame for the group, as a member would send it,
/// and hand back the base64 ciphertext.
///
/// The two tests below need the frame without the transport that normally
/// carries it, because they are about the two inbound paths a frame can
/// take *other* than the live mesh one.
fn group_data_frame(from: &mut Member, group: &str, doc: &str, key: &str, value: &str) -> String {
    // Produce a real delta by writing, then reading back what the push sent.
    write(from, group, doc, key, value);
    let frame = from
        .transport
        .sent_messages()
        .iter()
        .find_map(|m| {
            let payload = m.content.strip_prefix(internal_prefixes::GROUP_MLS_MSG)?;
            let value: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(value.get("ciphertext")?.as_str()?.to_string())
        })
        .expect("the write produced a group replication frame");
    from.transport.clear_sent_messages();
    frame
}

#[test]
fn a_replication_frame_delivered_by_the_relay_is_intercepted() {
    // The relay path is a second inbound route into the same group, and it
    // does not go through the mesh handler at all. A frame arriving this
    // way must be consumed as replication rather than surfaced as a
    // message: this repository has already paid once for a hardening fix
    // that landed on one inbound path while its sibling stayed broken.
    let (mut alice, mut bob, _carol, group) = trio();
    let ciphertext = group_data_frame(&mut alice, &group, "notes", "title", "hello");

    bob.protocol.handle_relay_group_message_with_mls(
        &group,
        &alice.address,
        &ciphertext,
        "2026-08-20T00:00:00Z",
        "relay-msg-1",
        None,
        None,
    );

    assert_eq!(
        read(&mut bob, &group, "notes", "title"),
        Some(DataValue::text("hello")),
        "a replication frame delivered by the relay was not applied"
    );
    assert!(
        !bob.saw_group_message(),
        "the frame was surfaced to the application as a chat message"
    );
}

#[test]
fn a_replication_frame_buffered_before_the_group_was_ready_is_intercepted_on_drain() {
    // The third inbound path: a frame that arrived before local group state
    // could open it waits in the pending buffer and is re-judged on drain.
    // Its ACK is still owed there — the sender is retransmitting until it
    // lands — so the drain has to both apply the frame and settle it.
    let (mut alice, mut bob, _carol, group) = trio();
    let ciphertext = group_data_frame(&mut alice, &group, "notes", "title", "hello");

    bob.protocol.buffer_pending_group_message(
        &group,
        crate::group_mesh::PendingGroupMessage {
            sender: alice.address.clone(),
            message_id: "buffered-1".to_string(),
            logical_id: None,
            ciphertext_b64: ciphertext,
            timestamp: Some("2026-08-20T00:00:00Z".to_string()),
            reply_to: None,
            forward_info: None,
            buffered_at: std::time::Instant::now(),
            received_via: None,
        },
    );

    bob.protocol.drain_pending_group_messages(&group);

    assert_eq!(
        read(&mut bob, &group, "notes", "title"),
        Some(DataValue::text("hello")),
        "a replication frame drained from the pending buffer was not applied"
    );
    assert!(
        !bob.saw_group_message(),
        "the drained frame was surfaced to the application as a chat message"
    );
}

#[test]
fn the_group_capability_is_advertised_and_recorded() {
    let (alice, _bob, _carol, _group) = trio();
    assert_eq!(
        alice.protocol.advertised_data_versions(),
        vec![DATA_SYNC_V1, DATA_GROUP_V1],
        "a build that intercepts group frames has to say so, or no peer \
         will ever send it one"
    );
}

#[test]
fn the_shared_group_sweep_finds_a_group_on_a_cold_roster_cache() {
    // Rediscovery is the only trigger a group space has for closing a gap
    // that no live frame carried, and the launch it matters on is the one
    // where nothing has touched the group yet. The roster cache is filled on
    // demand, so a sweep that read only it would find nothing here and the
    // two replicas would sit apart until somebody happened to edit.
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    alice.transport.clear_sent_messages();

    // What a relaunch leaves behind: MLS still holds the group, and nothing
    // has refilled the caches that live only in memory.
    alice.protocol.group_mesh.members.clear();
    alice.protocol.last_data_sync_offer.clear();
    alice.protocol.group_spaces_enumerated = false;

    alice
        .protocol
        .kick_shared_group_data_sync(&bob.address, "peer_rediscovered");

    assert!(
        data_frames_sent(&alice, &mut bob, &group) > 0,
        "a rediscovered member of a group this device is in has to be \
         offered its documents, and which groups those are comes from MLS, \
         not from a cache that a restart emptied"
    );
}

#[test]
fn a_space_named_after_a_one_to_one_session_slot_is_not_a_group_space() {
    // MLS stores every 1:1 session as a group, and the group-info read that
    // classifies a space answers for one. Left unguarded, a space named
    // after a session slot is treated as a group space and then filed in
    // `group_mesh.members` — the cache that leaving a group, admin
    // auto-promotion and the shared-group sweep all read as the set of real
    // groups. A space name is never wire-supplied, so this is a local
    // footgun rather than a reachable attack, and it costs one prefix test.
    let alice = Member::new("alice");
    crate::protocol::tests::create_local_session_with(&alice.protocol, "bob");
    let slot = crate::test_identity::session_slot("alice", "bob");

    // Non-vacuity: without this the assertions below would pass on a name
    // MLS simply does not know, which is the wrong reason.
    {
        let mls = alice.protocol.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&slot).unwrap();
        assert!(
            mls.has_group(&gid).unwrap(),
            "precondition: MLS really does hold a group at the session slot"
        );
    }

    let mut alice = alice;
    assert!(
        alice.protocol.group_space_roster(&slot).is_none(),
        "a session slot must never classify as a group space"
    );
    assert!(
        !alice.protocol.group_mesh.members.contains_key(&slot),
        "and classifying it must not have filed it in the roster cache"
    );
}

#[test]
fn directed_frames_do_not_strand_the_member_they_skip() {
    // A group has ONE sender ratchet per epoch. A frame addressed to one
    // member still advances the generation every *other* member must reach,
    // and OpenMLS refuses a generation more than 1000 ahead of the highest a
    // receiver has seen. Directed replication answers are the first traffic
    // in this SDK that advances the ratchet without every member observing
    // it, and rediscovery produces them without the user doing anything, so
    // left unbounded they cross 1000 on their own.
    //
    // What that costs is not the replication: it is every later frame from
    // this sender, chat included, permanently undecryptable for the skipped
    // member until a commit rotates the epoch — and a stable group produces
    // no commits. The frame lands as `Retriable`, is buffered, and expires.
    let (mut alice, mut bob, mut carol, group) = trio();

    // Alice and Bob reconcile over and over while Carol is simply not
    // around, which is the steady state of a rediscovery loop between two
    // co-located members of a three-member group.
    let hidden = MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS as usize * 5;
    for _ in 0..hidden {
        // The production trigger, with only its rate limit stepped over.
        alice.protocol.last_data_sync_offer.clear();
        alice
            .protocol
            .kick_group_data_sync(&group, &bob.address, "rediscovered");
    }
    assert!(
        hidden > 1000,
        "the run has to exceed the forward-distance limit or this test \
         passes for the wrong reason (hidden: {hidden})"
    );

    // Deliver everything, including the frames the budget promoted to the
    // whole roster. Those are the rungs Carol climbs.
    settle(&mut alice, &mut bob, &mut carol);

    // Now something every member needs.
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);

    assert_eq!(
        read(&mut carol, &group, "notes", "title"),
        Some(DataValue::text("hello")),
        "Carol could not decrypt a frame addressed to the whole roster: the \
         directed traffic she never saw ran the group's sender ratchet out of \
         her reach, which costs her every later message from Alice and not \
         just this one"
    );
}

/// The gap this device believes it has opened in `group`.
fn ratchet_gap(member: &Member, group: &str) -> Option<RosterRatchetGap> {
    member
        .protocol
        .group_mesh
        .roster_invisible_generations
        .get(group)
        .copied()
}

/// Declare the budget spent at the epoch the group is actually on, which is
/// what 256 directed frames would have done, without sending them.
fn spend_the_budget(member: &mut Member, group: &str) {
    let epoch = ratchet_gap(member, group)
        .expect("the group has to have been sent something before its budget can be spent")
        .epoch;
    member
        .protocol
        .group_mesh
        .roster_invisible_generations
        .insert(
            group.to_string(),
            RosterRatchetGap {
                epoch,
                invisible: MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS,
            },
        );
}

#[test]
fn a_roster_wide_frame_clears_the_ratchet_gap_budget() {
    // The budget is spent by frames the roster does not see and cleared by
    // one it does. Group chat is the common clearer, so a talkative group
    // never promotes anything; a group that only replicates documents is
    // the one that relies on the promotion.
    let (mut alice, bob, _carol, group) = trio();

    for _ in 0..10 {
        alice.protocol.last_data_sync_offer.clear();
        alice
            .protocol
            .kick_group_data_sync(&group, &bob.address, "rediscovered");
    }
    assert_eq!(
        ratchet_gap(&alice, &group).map(|gap| gap.invisible),
        Some(9),
        "each directed frame has to be counted, or the budget never trips. \
         Nine and not ten because the first frame of the process is promoted \
         on its own account: this device cannot see the generation it \
         inherited from earlier sessions"
    );

    alice
        .protocol
        .send_group_message(&group, "an ordinary message", None, None)
        .expect("send a group message");

    assert_eq!(
        ratchet_gap(&alice, &group).map(|gap| gap.invisible),
        Some(0),
        "a message every member is handed puts the whole roster back within \
         reach, so the gap it closed must not keep counting against the next \
         promotion"
    );
    assert!(
        ratchet_gap(&alice, &group).is_some(),
        "and the entry has to stay: an absent one means \"this process has \
         never given the roster a frame\", which would promote every frame \
         from here on"
    );
}

#[test]
fn the_first_directed_frame_of_a_process_is_promoted() {
    // The MLS sender ratchet is persisted with the group state and this
    // counter is not, so a relaunch inherits a generation it cannot read.
    // Counting from zero there would let a device that restarts often
    // accumulate the gap across sessions and strand a member without the
    // budget ever tripping: four quiet launches of 250 directed frames each
    // cross 1000 with the counter never leaving 250.
    let (mut alice, mut bob, mut carol, group) = trio();

    // Give the group a history so the offer below has something to carry,
    // then take the process back to what a relaunch leaves: the MLS state
    // (and its generation) intact, this map empty.
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    alice
        .protocol
        .group_mesh
        .roster_invisible_generations
        .clear();
    alice.transport.clear_sent_messages();

    alice.protocol.last_data_sync_offer.clear();
    alice
        .protocol
        .kick_group_data_sync(&group, &bob.address, "rediscovered");

    assert!(
        data_frames_sent(&alice, &mut carol, &group) > 0,
        "the first directed frame after a relaunch has to reach the whole \
         roster: it is the only rung this process can be sure every member \
         can still decrypt"
    );
    assert_eq!(
        ratchet_gap(&alice, &group).map(|gap| gap.invisible),
        Some(0),
        "and having promoted it, the count starts from a gap of nothing"
    );
}

#[test]
fn an_epoch_rotation_rebases_the_ratchet_gap() {
    // The gap is a property of one epoch: a commit rotates the group and
    // every member restarts the ratchet at zero, so a count carried across
    // the rotation describes a gap that no longer exists. Keyed by epoch
    // rather than cleared at each rotation site, because a rotation site
    // added later cannot forget to call something it never knew about.
    let (mut alice, mut bob, mut carol, group) = trio();

    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    let epoch = ratchet_gap(&alice, &group)
        .expect("the write recorded an epoch")
        .epoch;

    // A spent budget belonging to an epoch the group has since left.
    alice
        .protocol
        .group_mesh
        .roster_invisible_generations
        .insert(
            group.clone(),
            RosterRatchetGap {
                epoch: epoch.saturating_sub(1),
                invisible: MAX_ROSTER_INVISIBLE_GROUP_GENERATIONS,
            },
        );
    alice.transport.clear_sent_messages();

    alice.protocol.last_data_sync_offer.clear();
    alice
        .protocol
        .kick_group_data_sync(&group, &bob.address, "rediscovered");

    assert_eq!(
        data_frames_sent(&alice, &mut carol, &group),
        0,
        "a gap recorded in an epoch the group has left must not spend a \
         roster-wide frame to close a ratchet every member already restarted"
    );
    let gap = ratchet_gap(&alice, &group).expect("the directed frame was counted");
    assert_eq!(
        gap.epoch, epoch,
        "the count has to move to the epoch it now describes"
    );
    assert_eq!(
        gap.invisible, 1,
        "and start from that epoch's first invisible frame"
    );
}

#[test]
fn a_promotion_never_crosses_a_closed_replication_gate() {
    // A directed frame needs no capability check: the member it answers
    // asked for it. A promotion is a roster-wide delivery, and handing that
    // same frame to a member who does not intercept `__DATA_V1__` shows it
    // to their user as literal text, which is the send the all-members gate
    // exists to refuse. Reachable through knowledge asymmetry: Alice can be
    // answering Bob perfectly legitimately at the moment she learns Carol is
    // on a build that cannot read these.
    //
    // With the promotion unavailable the frame is withheld rather than sent
    // addressed, because every directed encryption spends a generation only
    // its target observes. Past the forward-distance limit that costs the
    // others this device's group *chat*, so continuing would trade a
    // permanent messaging failure for a document convergence that is
    // already stalled group-wide.
    let (mut alice, mut bob, mut carol, group) = trio();

    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    spend_the_budget(&mut alice, &group);
    let spent = ratchet_gap(&alice, &group).expect("the budget was just spent");
    alice.protocol.peer_data_group.remove(&carol.address);
    alice.transport.clear_sent_messages();

    // Bob asks. Alice would answer him directly, and the spent budget is
    // what would otherwise promote that answer to the whole roster.
    bob.protocol.last_data_sync_offer.clear();
    bob.protocol
        .kick_group_data_sync(&group, &alice.address, "rediscovered");
    {
        let (a, c) = (&mut alice, &mut carol);
        pump(&mut bob, &mut [a, c]);
    }

    assert_eq!(
        data_frames_sent(&alice, &mut carol, &group),
        0,
        "the ratchet budget promoted a replication frame into a roster whose \
         gate is closed; Carol renders it to her user as literal __DATA_V1__ \
         text, which is the whole reason the gate exists"
    );
    assert_eq!(
        data_frames_sent(&alice, &mut bob, &group),
        0,
        "with the promotion unavailable the answer has to be withheld, not \
         sent addressed: another generation only Bob observes is one more \
         step toward Carol losing this device's chat as well"
    );
    assert_eq!(
        ratchet_gap(&alice, &group).map(|gap| gap.invisible),
        Some(spent.invisible),
        "a withheld frame must not be counted, or the refusal spends the \
         very budget it exists to stop spending"
    );

    // The positive control, and the reason this test is about the gate
    // rather than about nothing ever happening: the same question, with
    // Carol known capable again, is answered by the promotion.
    alice.protocol.peer_data_group.insert(carol.address.clone());
    alice.transport.clear_sent_messages();
    bob.protocol.last_data_sync_offer.clear();
    bob.protocol
        .kick_group_data_sync(&group, &alice.address, "rediscovered");
    {
        let (a, c) = (&mut alice, &mut carol);
        pump(&mut bob, &mut [a, c]);
    }
    assert!(
        data_frames_sent(&alice, &mut carol, &group) > 0,
        "once every member can read one, the spent budget has to promote the \
         answer to the whole roster: that is the rung Carol climbs"
    );
}

#[test]
fn forgetting_a_peer_drops_its_group_offer_windows_too() {
    // The offer window and the capability are two halves of one fact. A
    // group window is keyed by (member, group), so dropping the bare peer
    // key leaves one behind per shared group, and each of those suppresses
    // the first offer to that peer after the capability is relearned —
    // which is a document that silently does not sync, one space at a time.
    let (mut alice, bob, _carol, group) = trio();

    alice
        .protocol
        .kick_group_data_sync(&group, &bob.address, "rediscovered");
    alice.protocol.kick_data_sync(&bob.address, "rediscovered");
    assert!(
        alice
            .protocol
            .last_data_sync_offer
            .keys()
            .any(|k| k.contains(&group)),
        "precondition: the group offer has to have stamped a window"
    );

    alice.protocol.forget_data_sync_peer(&bob.address);

    assert!(
        !alice
            .protocol
            .last_data_sync_offer
            .keys()
            .any(|k| k.starts_with(bob.address.as_str())),
        "every window belonging to the forgotten peer has to go, the group \
         ones included"
    );
}
