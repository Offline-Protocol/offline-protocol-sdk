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
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::protocol::data_sync::{SyncChannel, MAX_GROUP_BLOB_CHUNKS, MAX_SYNC_BLOB_BYTES};
use crate::protocol::prefixes::internal_prefixes;
use crate::protocol::tests::{create_test_config_for_user, id};
use crate::protocol::types::{
    DATA_GROUP_BLOB_V1, DATA_GROUP_V1, DATA_INTEREST_V1, DATA_MEDIA_V1, DATA_SYNC_V1,
    DATA_TOMBSTONE_V1,
};
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
            // Both entries, because one advertisement carries both: a build
            // that intercepts group frames at all is a build that carries
            // bytes inside one. Seeding entry 2 alone would model a peer no
            // release produces, and would quietly hold the blob gate open
            // by making every member look like one never heard from.
            member.protocol.peer_data_group.insert(other.clone());
            member.protocol.peer_data_group_blob.insert(other.clone());
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
        alice.protocol.peer_data_group_blob.insert(other.clone());
    }
    for other in [&alice.address, &bob.address, &carol.address] {
        dave.protocol.peer_data_group.insert(other.clone());
        dave.protocol.peer_data_group_blob.insert(other.clone());
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
        vec![
            DATA_SYNC_V1,
            DATA_GROUP_V1,
            DATA_MEDIA_V1,
            DATA_TOMBSTONE_V1,
            DATA_INTEREST_V1,
            DATA_GROUP_BLOB_V1
        ],
        "a build that intercepts group frames has to say so, or no peer \
         will ever send it one. The media entry rides the same list and is \
         read independently: it says nothing about groups, because v1 \
         carries blobs on 1:1 sessions only. The removal entry is read \
         independently too, and a group carries removals without consulting \
         it: one ciphertext reaches the whole roster, so there is no \
         per-member choice to make"
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
fn a_closed_gate_probes_the_member_holding_it_shut() {
    // Rediscovery is the only trigger a group space has that does not
    // require somebody to be editing, so on a device that only reads it is
    // the only moment anything notices the gate is shut. Probing from the
    // local-commit path alone would leave that device waiting for another
    // member to do it.
    let (mut alice, bob, _carol, group) = trio();
    alice.protocol.peer_data_group.remove(&bob.address);
    alice.protocol.key_package_sent_to.clear();

    alice
        .protocol
        .kick_group_data_sync(&group, &bob.address, "rediscovered");

    assert!(
        alice.protocol.key_package_sent_to.contains(&bob.address),
        "a member of unknown capability has to be asked what it supports; \
         our key package is the question, and its automatic reply is the \
         answer"
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

#[test]
fn a_member_that_narrows_its_interest_refuses_what_the_group_broadcasts() {
    // A group declares its interest unconditionally: one ciphertext reaches
    // the whole roster, so there is no per-member choice to make and no
    // member's capability to gate on. What carries the narrowing here is the
    // other half, the local refusal, and this is the channel that proves it
    // runs on a blob nobody addressed to this device in particular.
    let (mut alice, mut bob, mut carol, group) = trio();

    carol
        .protocol
        .data_set_interest(&group, vec!["wanted*".to_string()])
        .expect("interest");

    write(&mut alice, &group, "wanted.notes", "k", "yes");
    write(&mut alice, &group, "other.notes", "k", "no");
    let rounds = settle(&mut alice, &mut bob, &mut carol);

    let carol_docs = carol.protocol.data_list_docs(&group).expect("list");
    assert!(
        carol_docs.iter().any(|doc| doc == "wanted.notes"),
        "the document the narrowing asked for never arrived: {carol_docs:?}"
    );
    assert!(
        !carol_docs.iter().any(|doc| doc == "other.notes"),
        "a document outside the declared interest was stored from a group broadcast: \
         {carol_docs:?}"
    );
    // Bob narrowed nothing and the same ciphertext reached him, so he holds
    // both. Interest is this device's policy, never the space's.
    assert_eq!(
        read(&mut bob, &group, "other.notes", "k"),
        Some(DataValue::text("no")),
        "one member's interest narrowed what another member stores"
    );
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the group exchange did not terminate: {rounds:?}"
    );
}

#[test]
fn a_group_fetch_has_to_name_the_member_it_asks() {
    // A reference replicates to everybody and says nothing about who holds
    // the bytes, so a group fetch takes the member. The unaddressed call is
    // refused rather than quietly asking somebody, and the refusal says
    // which call to use instead.
    let (mut alice, _bob, _carol, group) = trio();
    let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let err = alice
        .protocol
        .data_fetch_attachment(&group, hash)
        .expect_err("an unaddressed group fetch must refuse");
    assert!(
        format!("{err}").contains("data_fetch_attachment_from"),
        "the refusal must name the call that works: {err}"
    );

    // And a member who is not in the group is refused too, rather than
    // being sent a frame under a key they do not hold.
    let err = alice
        .protocol
        .data_fetch_attachment_from(&group, &id("stranger"), hash)
        .expect_err("a fetch from a non-member must refuse");
    assert!(
        format!("{err}").contains("not a member"),
        "the refusal must name the reason: {err}"
    );
}

#[test]
fn a_group_document_too_large_to_frame_is_reported_not_carried() {
    // The group arm of the media escalation, driven through the real ladder
    // rather than by calling the rung directly. A 1:1 space answers a
    // document this size by carrying it over the media path; a group space
    // has no such path, because a blob transfer needs a confirmed 1:1
    // session and two members need not have one. So it says so.
    let (mut alice, mut bob, mut carol, group) = trio();

    // Past the 32 KiB frame budget, in one document, so every rung below the
    // media path is exhausted by the time the ladder reaches the top.
    //
    // The filler varies per key on purpose: repeated text compresses inside
    // the engine's encoding, and a document that compresses back under the
    // budget never reaches the rung this test is about.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for round in 0..24 {
        let filler: String = (0..4096)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                char::from(b'0' + (seed % 64) as u8)
            })
            .collect();
        alice
            .protocol
            .data_map_set(
                &group,
                "notes",
                "m",
                &format!("k{round}"),
                DataValue::text(filler),
            )
            .expect("set");
    }
    // One flush for everything, so the catch-up a peer asks for is the whole
    // document rather than a run of small deltas it already has.
    alice.protocol.data_flush(&group, "notes").expect("flush");
    assert!(
        alice.protocol.data_doc_size(&group, "notes").expect("size")
            > crate::protocol::data_sync::MAX_SYNC_BLOB_BYTES as u64,
        "the fixture must build a document no frame can carry"
    );
    settle(&mut alice, &mut bob, &mut carol);

    let reported: Vec<_> = alice
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .filter(|value| value.get("type").and_then(|t| t.as_str()) == Some("data_doc_unsyncable"))
        .collect();
    assert!(
        !reported.is_empty(),
        "the document must actually have outgrown every frame, or this test \
         asserts nothing"
    );
    assert!(
        reported
            .iter()
            .all(|event| event["reason"].as_str() == Some("group_space_has_no_media_path")),
        "a group dead end must name itself: {reported:?}"
    );
}

// ---- removals in a group ------------------------------------------------

fn holds(member: &mut Member, space: &str, doc: &str) -> bool {
    member
        .protocol
        .data_list_docs(space)
        .expect("list")
        .iter()
        .any(|name| name == doc)
}

/// The ciphertext of the frame a removal produces, as the roster receives it.
fn group_removal_frame(from: &mut Member, group: &str, doc: &str) -> String {
    from.transport.clear_sent_messages();
    from.protocol.data_remove_doc(group, doc).expect("remove");
    let frame = from
        .transport
        .sent_messages()
        .iter()
        .find_map(|m| {
            let payload = m.content.strip_prefix(internal_prefixes::GROUP_MLS_MSG)?;
            let value: serde_json::Value = serde_json::from_str(payload).ok()?;
            Some(value.get("ciphertext")?.as_str()?.to_string())
        })
        .expect("the removal produced a group frame");
    from.transport.clear_sent_messages();
    frame
}

#[test]
fn a_removal_reaches_every_member_of_a_group() {
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    assert!(
        holds(&mut bob, &group, "notes") && holds(&mut carol, &group, "notes"),
        "precondition: the document has to reach the roster before its removal can"
    );

    alice
        .protocol
        .data_remove_doc(&group, "notes")
        .expect("remove");
    let rounds = settle(&mut alice, &mut bob, &mut carol);

    for (member, label) in [
        (&mut alice, "alice"),
        (&mut bob, "bob"),
        (&mut carol, "carol"),
    ] {
        assert!(
            !holds(member, &group, "notes"),
            "{label} still holds a document the group removed"
        );
    }
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange did not terminate: {rounds:?}"
    );
}

#[test]
fn a_removal_delivered_by_the_relay_is_applied() {
    // The second of the three inbound group routes. A hardening fix that
    // lands on one and not its siblings is a shape this repository has
    // already paid for twice, so each route gets its own test.
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    assert!(holds(&mut bob, &group, "notes"), "precondition");

    let ciphertext = group_removal_frame(&mut alice, &group, "notes");
    bob.protocol.handle_relay_group_message_with_mls(
        &group,
        &alice.address,
        &ciphertext,
        "2026-09-22T00:00:00Z",
        "relay-removal-1",
        None,
        None,
    );

    assert!(
        !holds(&mut bob, &group, "notes"),
        "a removal delivered by the relay was not applied"
    );
    assert!(
        !bob.saw_group_message(),
        "the removal was surfaced to the application as a chat message"
    );
}

#[test]
fn a_removal_buffered_before_the_group_was_ready_is_applied_on_drain() {
    // The third route: a frame that arrived before local group state could
    // open it waits in the pending buffer and is re-judged on drain.
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);
    assert!(holds(&mut bob, &group, "notes"), "precondition");

    let ciphertext = group_removal_frame(&mut alice, &group, "notes");
    bob.protocol.buffer_pending_group_message(
        &group,
        crate::group_mesh::PendingGroupMessage {
            sender: alice.address.clone(),
            message_id: "buffered-removal-1".to_string(),
            logical_id: None,
            ciphertext_b64: ciphertext,
            timestamp: Some("2026-09-22T00:00:00Z".to_string()),
            reply_to: None,
            forward_info: None,
            buffered_at: std::time::Instant::now(),
            received_via: None,
        },
    );
    bob.protocol.drain_pending_group_messages(&group);

    assert!(
        !holds(&mut bob, &group, "notes"),
        "a removal drained from the pending buffer was not applied"
    );
    assert!(
        !bob.saw_group_message(),
        "the drained removal was surfaced to the application as a chat message"
    );
}

#[test]
fn a_member_that_edited_while_a_removal_crossed_keeps_the_document_for_everyone() {
    // Edit-wins inside a group, where the third member is the interesting
    // one: Carol neither removed nor edited, so whatever she ends up with
    // is what the rule actually resolved to rather than what either actor
    // decided locally.
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "notes", "title", "hello");
    settle(&mut alice, &mut bob, &mut carol);

    alice.transport.clear_sent_messages();
    bob.transport.clear_sent_messages();
    carol.transport.clear_sent_messages();
    alice
        .protocol
        .data_remove_doc(&group, "notes")
        .expect("remove");
    write(&mut bob, &group, "notes", "added", "B");
    let rounds = settle(&mut alice, &mut bob, &mut carol);

    for (member, label) in [
        (&mut alice, "alice"),
        (&mut bob, "bob"),
        (&mut carol, "carol"),
    ] {
        assert!(
            holds(member, &group, "notes"),
            "{label} lost a document an edit had kept alive"
        );
        assert_eq!(
            read(member, &group, "notes", "added"),
            Some(DataValue::text("B")),
            "{label} did not converge on the edit that won"
        );
        assert_eq!(
            read(member, &group, "notes", "title"),
            Some(DataValue::text("hello")),
            "{label} kept the document without the content the edit was made on"
        );
    }
    assert_eq!(
        rounds.last(),
        Some(&0),
        "the exchange did not terminate: {rounds:?}"
    );
}

#[test]
fn a_long_run_of_directed_frames_all_arrive_and_decrypt() {
    // Carrying a blob inside a group means a run of directed frames, and
    // this is what that rests on: every one of them decrypting, in one
    // epoch, at a length a blob actually produces. A blob at the cap is 32
    // frames; this sends twice that.
    let (mut alice, mut bob, mut carol, group) = trio();

    // A real version token, so each offer is a frame Bob genuinely acts on
    // rather than one he merely fails to parse. Acting on it is the positive
    // signal: a frame that did not decrypt creates no document.
    write(&mut alice, &group, "seed", "k", "v");
    settle(&mut alice, &mut bob, &mut carol);
    let version = alice
        .protocol
        .data_doc_version(&group, "seed")
        .expect("version");
    alice.transport.clear_sent_messages();

    let total = 64usize;
    for index in 0..total {
        alice
            .protocol
            .send_group_internal_frame(
                &group,
                Some(&bob.address),
                &format!(
                    "{}{{\"v\":1,\"k\":\"vv\",\"reply\":true,\"partial\":true,\"docs\":{{\"spike{index}\":\"{version}\"}}}}",
                    internal_prefixes::DATA_V1
                ),
            )
            .expect("send a directed frame");
    }
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    let arrived = bob
        .protocol
        .data_list_docs(&group)
        .expect("list")
        .iter()
        .filter(|name| name.starts_with("spike"))
        .count();
    let carol_saw = carol
        .protocol
        .data_list_docs(&group)
        .expect("list")
        .iter()
        .filter(|name| name.starts_with("spike"))
        .count();
    assert_eq!(
        carol_saw, 0,
        "a directed frame reached a member it was not addressed to"
    );
    assert!(
        !bob.saw_group_message(),
        "a replication frame was surfaced to the application as a chat message"
    );
    assert_eq!(
        arrived, total,
        "directed frames were lost or did not decrypt"
    );
}

#[test]
fn a_run_of_directed_frames_survives_the_ratchet_promotion() {
    let (mut alice, mut bob, mut carol, group) = trio();
    write(&mut alice, &group, "seed", "k", "v");
    settle(&mut alice, &mut bob, &mut carol);
    let version = alice
        .protocol
        .data_doc_version(&group, "seed")
        .expect("version");

    // Put the sender one frame short of the bound, so a run crosses it.
    spend_the_budget(&mut alice, &group);
    alice.transport.clear_sent_messages();

    let total = 8usize;
    for index in 0..total {
        alice
            .protocol
            .send_group_internal_frame(
                &group,
                Some(&bob.address),
                &format!(
                    "{}{{\"v\":1,\"k\":\"vv\",\"reply\":true,\"partial\":true,\"docs\":{{\"promo{index}\":\"{version}\"}}}}",
                    internal_prefixes::DATA_V1
                ),
            )
            .expect("send a directed frame");
    }
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    let bob_saw = bob
        .protocol
        .data_list_docs(&group)
        .expect("list")
        .iter()
        .filter(|name| name.starts_with("promo"))
        .count();
    let carol_saw = carol
        .protocol
        .data_list_docs(&group)
        .expect("list")
        .iter()
        .filter(|name| name.starts_with("promo"))
        .count();
    assert_eq!(
        bob_saw, total,
        "a run of directed frames lost one at the promotion"
    );
    // The promoted frame is a roster-wide delivery, so the members it was
    // not addressed to receive it and act on it. That is why a chunk frame
    // has to be droppable by a member with no fetch outstanding: promotion
    // puts one in front of every member eventually.
    assert_eq!(
        carol_saw, 1,
        "the promotion did not reach the rest of the roster, or reached it more than once"
    );
}

// ---- attachment bytes inside a group ------------------------------------

/// Events of one kind, as the application sees them.
///
/// Serialized rather than matched, because that is the shape an application
/// actually receives across the FFI and it keeps the assertions about the
/// fields rather than about the variant.
fn attachment_events(member: &Member, name: &str) -> Vec<serde_json::Value> {
    member
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .filter(|value| value.get("type").and_then(|t| t.as_str()) == Some(name))
        .collect()
}

/// One string field of an event.
fn field<'a>(event: &'a serde_json::Value, key: &str) -> &'a str {
    event
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("event has no {key}: {event}"))
}

#[test]
fn a_member_fetches_a_blob_from_another_member() {
    // Nothing roster-wide has gone out yet, so this device has no entry
    // against the ratchet-gap budget and its first directed frame is
    // promoted to the whole roster. That is not incidental to this test: it
    // is why Carol receives the request at all, and so it is the `to` field
    // rather than the addressing that keeps her application out of it.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![7u8; 40 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    // Bob asks Alice, by name: the reference says what the bytes are, never
    // who has them.
    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    let asked = attachment_events(&alice, "data_attachment_requested");
    assert_eq!(asked.len(), 1, "the holder was not asked");
    assert!(
        attachment_events(&carol, "data_attachment_requested").is_empty(),
        "a directed request reached a member it was not addressed to"
    );

    // Only the application has the bytes, so only it can answer.
    alice
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes.clone())
        .expect("provide");
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    let received = attachment_events(&bob, "data_attachment_received");
    assert_eq!(received.len(), 1, "the bytes never arrived");
    assert_eq!(field(&received[0], "hash"), hash);
    assert_eq!(
        BASE64.decode(field(&received[0], "data")).expect("base64"),
        bytes,
        "the blob came back changed"
    );
    assert!(
        attachment_events(&carol, "data_attachment_received").is_empty(),
        "a member who asked for nothing was handed a blob"
    );
    assert!(
        !carol.saw_group_message() && !bob.saw_group_message(),
        "a chunk frame was surfaced to the application as a chat message"
    );
}

#[test]
fn a_fetch_toward_a_member_known_not_to_carry_bytes_is_refused_at_the_call() {
    // What entry 6 actually buys. The member receives the question, knows
    // its shape, and refuses; the entry is what lets the asking device say
    // so at the call instead of leaving an application waiting out the
    // silence timeout for an answer that was never coming.
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes alice cannot carry");

    // Alice speaks group replication and not the blob entry: every build
    // between the two releases.
    bob.protocol.peer_data_group_blob.remove(&alice.address);
    bob.protocol
        .peer_data_group_blob_attested
        .remove(&alice.address);

    let err = bob
        .protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect_err("a member known not to carry bytes must be refused");
    assert!(
        format!("{err}").contains("does not carry attachment bytes"),
        "the refusal must name the reason: {err}"
    );
    assert!(
        bob.protocol.pending_attachment_fetches.is_empty(),
        "a refused fetch still spent a slot"
    );
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert!(
        attachment_events(&alice, "data_attachment_requested").is_empty(),
        "a refused fetch still asked"
    );
}

#[test]
fn a_fetch_toward_a_member_nothing_is_known_about_is_refused_at_the_call() {
    // The one directed frame this layer sends to a member who asked for
    // nothing, so it answers to the same rule a broadcast does: a member
    // not known to intercept `__DATA_V1__` renders it as chat text, and
    // neither road may put one in front of them.
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes from a stranger");

    bob.protocol.forget_data_sync_peer(&alice.address);

    let err = bob
        .protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect_err("a member nothing is known about must be refused");
    assert!(
        format!("{err}").contains("shown to them as text"),
        "the refusal must name the failure it prevents: {err}"
    );
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert!(
        attachment_events(&alice, "data_attachment_requested").is_empty(),
        "a refused fetch still asked"
    );
}

#[test]
fn an_inviters_attestation_is_enough_to_ask_a_member() {
    // The whole reason entry 6 has an attested sibling. Members of a group
    // never exchange key packages with each other, so on a group nobody
    // built out of existing 1:1 contacts the direct knowledge is empty and
    // the inviter is the only source there is. Read independently of entry
    // 2, which is what makes the fetch gate and the frame gate separable.
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes only an inviter vouched for");

    bob.protocol.forget_data_sync_peer(&alice.address);
    bob.protocol
        .record_attested_data(&alice.address, &[DATA_GROUP_V1, DATA_GROUP_BLOB_V1]);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("an attested member can be asked");
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert_eq!(
        attachment_events(&alice, "data_attachment_requested").len(),
        1,
        "the attested member was never asked"
    );
}

#[test]
fn an_attestation_without_the_blob_entry_still_lets_the_question_go() {
    // An inviter on a build that predates entry 6 attests entry 2 alone,
    // which says nothing either way about the bytes. "We have not heard"
    // must not read as "they cannot": asking and hearing nothing is the
    // honest outcome, and the silence timeout reports it.
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes an old inviter said nothing about");

    bob.protocol.forget_data_sync_peer(&alice.address);
    bob.protocol
        .record_attested_data(&alice.address, &[DATA_GROUP_V1]);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("silence about the entry is not knowledge that it is absent");
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert_eq!(
        attachment_events(&alice, "data_attachment_requested").len(),
        1,
        "a member an inviter said nothing about was never asked"
    );
}

#[test]
fn chunks_nobody_asked_for_are_dropped() {
    // The rule the promotion makes load-bearing: a roster-wide frame puts a
    // chunk in front of members who asked for nothing, and they must not
    // assemble it.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![3u8; 8 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    // Nobody fetched, and Alice answers anyway.
    alice
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect("provide");
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    for (member, label) in [(&bob, "bob"), (&carol, "carol")] {
        assert!(
            attachment_events(member, "data_attachment_received").is_empty(),
            "{label} assembled a blob nothing asked for"
        );
        assert!(
            !member.saw_group_message(),
            "{label} was shown a chunk frame as a chat message"
        );
    }
}

#[test]
fn a_blob_that_does_not_hash_to_what_was_asked_for_is_refused() {
    let (mut alice, mut bob, mut carol, group) = trio();
    let wanted = vec![1u8; 4 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&wanted);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    // A holder that sends different bytes under the asked-for address. The
    // send seam is reached directly, because `provide` checks the hash on
    // the way out and this is about the check on the way in.
    let lie = vec![2u8; 4 * 1024];
    alice.protocol.send_chunk_for_test(
        &group,
        &SyncChannel::GroupDirected(bob.address.clone()),
        &hash,
        0,
        1,
        &BASE64.encode(&lie),
    );
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    assert!(
        attachment_events(&bob, "data_attachment_received").is_empty(),
        "bytes that did not hash to the request were handed to the application"
    );
    let ended = attachment_events(&bob, "data_attachment_unavailable");
    assert_eq!(ended.len(), 1, "the fetch ended without saying so");
    assert_eq!(field(&ended[0], "reason"), "hash_mismatch");
    assert_eq!(
        field(&ended[0], "peer_id"),
        alice.address,
        "the report names the group rather than the member that was asked"
    );
}

#[test]
fn a_group_fetch_that_nobody_answers_names_the_member_it_was_put_to() {
    // Every road that ends a fetch owes the same event, and in a group the
    // space is not the peer: an application told the group id cannot stop
    // asking that member, or ask the next one. The refusal road already
    // named the member because it reads the sender; these roads read the
    // record, which is the only place the member survives.
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes nobody sends");

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    let stale = std::time::Instant::now()
        - crate::protocol::data_sync::ATTACHMENT_FETCH_TIMEOUT
        - std::time::Duration::from_secs(1);
    for pending in bob.protocol.pending_attachment_fetches.values_mut() {
        pending.last_seen = stale;
    }
    bob.protocol.expire_attachment_fetches();

    let ended = attachment_events(&bob, "data_attachment_unavailable");
    assert_eq!(ended.len(), 1, "the silence was never reported");
    assert_eq!(field(&ended[0], "reason"), "timeout");
    assert_eq!(
        field(&ended[0], "space_id"),
        group,
        "the report names the wrong space"
    );
    assert_eq!(
        field(&ended[0], "peer_id"),
        alice.address,
        "the report names the group rather than the member that was asked"
    );
}

#[test]
fn forgetting_a_member_ends_the_group_fetch_put_to_them() {
    // A fetch keyed by a group is not keyed by the member, so the road that
    // forgets a peer has to read the record to find it. Left behind, the
    // question holds a slot against the fetch bound until it times out, for
    // a member this device has stopped replicating with entirely.
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes alice was asked for");

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert!(
        !bob.protocol.pending_attachment_fetches.is_empty(),
        "the question was never recorded"
    );

    bob.protocol.forget_data_sync_peer(&alice.address);

    assert!(
        bob.protocol.pending_attachment_fetches.is_empty(),
        "a question put to a forgotten member outlived them"
    );
    let ended = attachment_events(&bob, "data_attachment_unavailable");
    assert_eq!(ended.len(), 1, "the fetch was dropped without saying so");
    assert_eq!(field(&ended[0], "reason"), "peer_gone");
    assert_eq!(
        field(&ended[0], "peer_id"),
        alice.address,
        "the report names the group rather than the member that was asked"
    );
}

#[test]
fn asking_a_second_member_is_a_new_question_rather_than_a_repeat() {
    // The SDK does not walk the roster, so falling back to another member
    // is the application's move to make, and it has to work the moment the
    // first member goes quiet. The record is keyed by the space and the
    // blob, neither of which changes when the member does.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![8u8; 4 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert_eq!(
        attachment_events(&alice, "data_attachment_requested").len(),
        1,
        "the first member was never asked"
    );

    // Alice says nothing. Well inside the repeat window, the application
    // asks Carol instead.
    bob.protocol
        .data_fetch_attachment_from(&group, &carol.address, &hash)
        .expect("fallback");
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    assert_eq!(
        attachment_events(&carol, "data_attachment_requested").len(),
        1,
        "the fallback was swallowed as a repeat and asked nobody"
    );

    // The question put to Alice is closed, and names her.
    let ended = attachment_events(&bob, "data_attachment_unavailable");
    assert_eq!(ended.len(), 1, "the displaced question never ended");
    assert_eq!(field(&ended[0], "peer_id"), alice.address);

    // And the member who was actually asked can still answer.
    carol
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect("provide");
    pump(&mut carol, &mut [&mut alice, &mut bob]);
    let received = attachment_events(&bob, "data_attachment_received");
    assert_eq!(
        received.len(),
        1,
        "the fallback member's answer was refused"
    );
    assert_eq!(field(&received[0], "peer_id"), carol.address);
}

#[test]
fn a_group_fetch_cannot_be_put_to_this_device() {
    // A device is in the roster it just read. An application walking that
    // roster to choose a member reaches this, and a question put to
    // nobody would be answered by the silence timeout fifteen minutes
    // later.
    let (mut alice, _bob, _carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes this device would already have");

    let err = alice
        .protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect_err("a device must not ask itself");
    assert!(
        format!("{err}").contains("not a peer of itself"),
        "the refusal must name the reason: {err}"
    );
    assert!(
        alice.protocol.pending_attachment_fetches.is_empty(),
        "a refused fetch still spent a slot"
    );
}

#[test]
fn a_blob_too_large_for_a_group_is_refused_at_the_call() {
    // The cap is what keeps a request a single-hop question: the whole
    // answer leaves at once, so nothing has to ask for the next window.
    // The holder hears about it while it still has the file in hand.
    let (mut alice, mut bob, _carol, group) = trio();
    let bytes = vec![9u8; (MAX_GROUP_BLOB_CHUNKS as usize + 1) * 32 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    let err = alice
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect_err("an oversized group attachment must refuse");
    assert!(
        format!("{err}").contains("1:1"),
        "the refusal must say where a blob this size goes: {err}"
    );
}

#[test]
fn a_member_that_has_none_says_so_rather_than_going_quiet() {
    let (mut alice, mut bob, mut carol, group) = trio();
    let hash = OfflineProtocol::data_attachment_hash(b"bytes alice does not have");

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);
    alice
        .protocol
        .data_decline_attachment(&group, &bob.address, &hash)
        .expect("decline");
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    let ended = attachment_events(&bob, "data_attachment_unavailable");
    assert_eq!(ended.len(), 1, "a refusal left the asker waiting");
    assert_eq!(field(&ended[0], "reason"), "declined");
    assert_eq!(
        field(&ended[0], "peer_id"),
        alice.address,
        "the report names the wrong member"
    );
}

#[test]
fn an_answer_from_a_member_the_question_was_not_put_to_is_refused() {
    // Two members can both hold the bytes, and only one was asked. A blob
    // from the other is not an answer: admitting it would let any member
    // end a question that was put to somebody else.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![5u8; 4 * 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    carol
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect("provide");
    pump(&mut carol, &mut [&mut alice, &mut bob]);

    assert!(
        attachment_events(&bob, "data_attachment_received").is_empty(),
        "a member who was not asked answered the question anyway"
    );
}

#[test]
fn a_chunk_declaring_a_shape_this_layer_does_not_carry_is_dropped() {
    // The receiver's own bound, not a restatement of the sender's. A holder
    // that ignores the cap would otherwise spend this device's memory on
    // whatever count it felt like declaring, and an index outside the count
    // would sit in the assembly forever, keeping it one piece short.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![4u8; 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);
    let blob = BASE64.encode(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    for (index, total, shape) in [
        (0u32, 0u32, "a count of nothing"),
        (0, MAX_GROUP_BLOB_CHUNKS + 1, "a count past the cap"),
        (5, 1, "an index outside its own count"),
    ] {
        alice.protocol.send_chunk_for_test(
            &group,
            &SyncChannel::GroupDirected(bob.address.clone()),
            &hash,
            index,
            total,
            &blob,
        );
        pump(&mut alice, &mut [&mut bob, &mut carol]);
        assert!(
            attachment_events(&bob, "data_attachment_received").is_empty(),
            "a chunk declaring {shape} was assembled anyway"
        );
    }

    // And the fetch is still open, so the well-formed answer still lands:
    // a malformed piece must not cancel a carriage that is otherwise on its
    // way.
    alice
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect("provide");
    pump(&mut alice, &mut [&mut bob, &mut carol]);
    assert_eq!(
        attachment_events(&bob, "data_attachment_received").len(),
        1,
        "a malformed chunk cancelled the fetch it arrived against"
    );
}

#[test]
fn a_chunk_over_the_frame_budget_is_dropped() {
    // The per-piece bound, which is this device's and not a restatement of
    // the sender's. A holder that ignored it would otherwise decide on its
    // own how much of another member's memory one piece may cost.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![2u8; 1024];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    let oversized = BASE64.encode(vec![0u8; MAX_SYNC_BLOB_BYTES + 1]);
    alice.protocol.send_chunk_for_test(
        &group,
        &SyncChannel::GroupDirected(bob.address.clone()),
        &hash,
        0,
        1,
        &oversized,
    );
    pump(&mut alice, &mut [&mut bob, &mut carol]);
    assert!(
        attachment_events(&bob, "data_attachment_received").is_empty(),
        "a chunk over the frame budget was assembled anyway"
    );

    // And the carriage survives it, like every other malformed piece.
    alice
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect("provide");
    pump(&mut alice, &mut [&mut bob, &mut carol]);
    assert_eq!(
        attachment_events(&bob, "data_attachment_received").len(),
        1,
        "an oversized chunk cancelled the fetch it arrived against"
    );
}

#[test]
fn a_carriage_that_stops_half_way_still_ends() {
    // Arriving bytes refresh the clock, which is what keeps a carriage in
    // progress from expiring underneath itself. The other side of that is
    // this: a member that sends one piece and stops has refreshed the clock
    // once, and the question must still end rather than hold its slot and
    // its assembled bytes for good.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes: Vec<u8> = (0..(MAX_SYNC_BLOB_BYTES + 512)).map(|i| i as u8).collect();
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    // One piece of two, then silence.
    let first = BASE64.encode(&bytes[..MAX_SYNC_BLOB_BYTES]);
    alice.protocol.send_chunk_for_test(
        &group,
        &SyncChannel::GroupDirected(bob.address.clone()),
        &hash,
        0,
        2,
        &first,
    );
    pump(&mut alice, &mut [&mut bob, &mut carol]);
    assert!(
        attachment_events(&bob, "data_attachment_received").is_empty(),
        "an incomplete answer was handed over"
    );
    assert_eq!(
        bob.protocol.pending_attachment_fetches.len(),
        1,
        "the half-arrived answer left no question behind"
    );

    let stale = std::time::Instant::now()
        - crate::protocol::data_sync::ATTACHMENT_FETCH_TIMEOUT
        - std::time::Duration::from_secs(1);
    for pending in bob.protocol.pending_attachment_fetches.values_mut() {
        pending.last_seen = stale;
    }
    bob.protocol.expire_attachment_fetches();

    assert!(
        bob.protocol.pending_attachment_fetches.is_empty(),
        "a half-arrived answer held its slot and its bytes for good"
    );
    let ended = attachment_events(&bob, "data_attachment_unavailable");
    assert_eq!(ended.len(), 1, "the stalled carriage was never reported");
    assert_eq!(field(&ended[0], "reason"), "timeout");
    assert_eq!(field(&ended[0], "peer_id"), alice.address);
}

#[test]
fn a_duplicate_chunk_replaces_rather_than_appends() {
    // What carrying the count on every piece buys: the pieces are
    // independent, so one arriving twice is one piece and not two, and the
    // answer still completes at the count it declared. Appending would
    // leave the blob longer than it was and fail the hash that asked for
    // it.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes: Vec<u8> = (0..(MAX_SYNC_BLOB_BYTES + 512)).map(|i| i as u8).collect();
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    let pieces: Vec<String> = bytes
        .chunks(MAX_SYNC_BLOB_BYTES)
        .map(|chunk| BASE64.encode(chunk))
        .collect();
    assert_eq!(pieces.len(), 2, "this test needs a blob of two pieces");
    let total = pieces.len() as u32;
    for index in [0usize, 0, 1] {
        alice.protocol.send_chunk_for_test(
            &group,
            &SyncChannel::GroupDirected(bob.address.clone()),
            &hash,
            index as u32,
            total,
            &pieces[index],
        );
    }
    pump(&mut alice, &mut [&mut bob, &mut carol]);

    let received = attachment_events(&bob, "data_attachment_received");
    assert_eq!(received.len(), 1, "the repeated piece broke the count");
    assert_eq!(
        BASE64.decode(field(&received[0], "data")).expect("base64"),
        bytes,
        "a duplicate piece was appended rather than replacing"
    );
}

#[test]
fn a_refusal_from_a_member_the_question_was_not_put_to_is_ignored() {
    // Any member can send a refusal, and a promoted frame puts one in front
    // of everybody. Only the member the question was put to can end it,
    // or one member could close another member's fetch and the asker would
    // stop waiting for an answer that was still coming.
    let (mut alice, mut bob, mut carol, group) = trio();
    let bytes = vec![6u8; 2048];
    let hash = OfflineProtocol::data_attachment_hash(&bytes);

    bob.protocol
        .data_fetch_attachment_from(&group, &alice.address, &hash)
        .expect("fetch");
    pump(&mut bob, &mut [&mut alice, &mut carol]);

    carol
        .protocol
        .data_decline_attachment(&group, &bob.address, &hash)
        .expect("decline");
    pump(&mut carol, &mut [&mut alice, &mut bob]);
    assert!(
        attachment_events(&bob, "data_attachment_unavailable").is_empty(),
        "a member who was not asked ended somebody else's fetch"
    );

    // The member who *was* asked still gets through.
    alice
        .protocol
        .data_provide_attachment(&group, &bob.address, &hash, bytes)
        .expect("provide");
    pump(&mut alice, &mut [&mut bob, &mut carol]);
    assert_eq!(
        attachment_events(&bob, "data_attachment_received").len(),
        1,
        "the fetch did not survive a refusal from the wrong member"
    );
}
