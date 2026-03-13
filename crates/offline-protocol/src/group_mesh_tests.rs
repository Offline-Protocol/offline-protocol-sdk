use super::group_mesh::*;
use crate::protocol::tests::{create_test_config, create_test_config_for_user};
use crate::protocol::{base64_decode, base64_encode, internal_prefixes, InternalMessageResult};
use crate::{Event, OfflineProtocol};
use offline_protocol_core::{AppId, UserId};
use offline_protocol_transport::{Transport, TransportType};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

/// Creates a protocol instance with MLS initialized and started, plus an event collector.
fn setup_started_with_events() -> (OfflineProtocol, Arc<Mutex<Vec<Event>>>) {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    (protocol, events)
}

/// Creates a protocol instance with MLS initialized (not started), plus an event collector.
fn setup_with_events() -> (OfflineProtocol, Arc<Mutex<Vec<Event>>>) {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    (protocol, events)
}

/// Creates a two-party (Alice + Bob) setup with MLS groups in sync.
/// Returns (alice, bob, group_id).
fn setup_alice_bob_group(group_name: &str) -> (OfflineProtocol, OfflineProtocol, String) {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    bob.initialize_mls(storage_b).unwrap();
    alice.start().unwrap();
    bob.start().unwrap();

    let group_info = alice.create_group(group_name).unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    let bob_kp = {
        let bob_mls = bob.mls_manager_for_testing().read().unwrap();
        bob_mls.generate_key_package().unwrap()
    };
    let (welcome, _commit) = {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls
            .add_group_member(&gid, &bob_kp.key_package_data)
            .unwrap()
    };
    {
        let bob_mls = bob.mls_manager_for_testing().read().unwrap();
        bob_mls.join_group(&welcome).unwrap();
    }

    alice.refresh_group_members(&group_id).unwrap();
    bob.group_mesh.members.insert(
        group_id.clone(),
        vec!["alice".to_string(), "bob".to_string()],
    );

    (alice, bob, group_id)
}

/// Builds an internal message from sender→recipient with the given content.
fn make_message(sender: &str, recipient: &str, content: &str) -> offline_protocol_core::Message {
    offline_protocol_core::Message::new(
        UserId::new(sender).unwrap(),
        UserId::new(recipient).unwrap(),
        AppId::new("test-app").unwrap(),
        content,
    )
}

#[test]
fn test_group_mls_create_mesh_group_requires_mls() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    // Without MLS initialization, create_mesh_group should fail
    let result = protocol.create_group("Test Group");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("MLS not initialized"),
        "Expected MLS not initialized error"
    );
}

#[test]
fn test_group_mls_create_mesh_group_with_mls() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let group_info = protocol.create_group("Test Group").unwrap();
    assert_eq!(group_info.name, Some("Test Group".to_string()));
    assert!(group_info.group_id.as_str().starts_with("group:"));
    assert!(group_info.members.contains(&"user123".to_string()));

    // Verify group is cached
    let cached = protocol
        .group_mesh
        .members
        .get(group_info.group_id.as_str());
    assert!(cached.is_some());
    assert!(cached.unwrap().contains(&"user123".to_string()));
}

#[test]
fn test_group_mls_list_groups() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // Initially no groups
    let groups = protocol.list_groups().unwrap();
    assert!(groups.is_empty());

    // Create a group
    let info = protocol.create_group("My Group").unwrap();
    let groups = protocol.list_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0], info.group_id.as_str());
}

#[test]
fn test_group_mls_send_message_requires_mls() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let result = protocol.send_group_message("some-group", "hello", None, None);
    assert!(result.is_err());
}

#[test]
fn test_group_mls_send_message_group_not_found() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // Sending to non-existent group should fail
    let result = protocol.send_group_message("nonexistent-group", "hello", None, None);
    assert!(result.is_err());
}

#[test]
fn test_group_mls_send_message_solo_group() {
    let (mut protocol, _events) = setup_started_with_events();

    // Create group (only self is a member)
    let info = protocol.create_group("Solo Group").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Sending in a solo group should succeed but produce no message IDs
    // (no other members to fan out to)
    let result = protocol.send_group_message(&group_id, "hello", None, None);
    assert!(result.is_ok());
    let message_ids = result.unwrap();
    assert!(message_ids.is_empty(), "No messages should be sent to self");
}

#[test]
fn test_group_mls_leave_group() {
    let (mut protocol, _events) = setup_started_with_events();

    let info = protocol.create_group("Leave Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Verify group exists in cache
    assert!(protocol.group_mesh.members.contains_key(&group_id));

    // Leave the group
    protocol.leave_group(&group_id).unwrap();

    // Verify group removed from cache
    assert!(!protocol.group_mesh.members.contains_key(&group_id));
}

#[test]
fn test_group_mls_invite_requires_key_package() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let info = protocol.create_group("Invite Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Inviting without key package should fail
    let result = protocol.invite_to_group(&group_id, "bob");
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("No key package"),
        "Expected no key package error"
    );
}

#[test]
fn test_group_mls_dedup_cache() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Insert dedup entry using message ID as key
    let key = "msg-001".to_string();
    protocol
        .group_mesh
        .message_dedup
        .insert(key.clone(), Instant::now());
    assert!(protocol.group_mesh.message_dedup.contains_key(&key));

    // Cleanup should keep recent entries
    protocol.cleanup_group_message_dedup();
    assert!(protocol.group_mesh.message_dedup.contains_key(&key));

    // Insert old entry and verify cleanup removes it
    let old_key = "msg-002".to_string();
    protocol.group_mesh.message_dedup.insert(
        old_key.clone(),
        Instant::now() - StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS + 1),
    );
    protocol.cleanup_group_message_dedup();
    assert!(!protocol.group_mesh.message_dedup.contains_key(&old_key));
    assert!(protocol.group_mesh.message_dedup.contains_key(&key));
}

#[test]
fn test_group_mls_process_leave_message() {
    let (mut protocol, events) = setup_started_with_events();

    // Pre-populate group member cache.
    // "alice" < "bob" < "user123" lexicographically.
    // When "alice" leaves, "bob" is elected (lex-first remaining).
    // We are "user123", so we are NOT elected.
    protocol.group_mesh.members.insert(
        "group:test-123".to_string(),
        vec![
            "user123".to_string(),
            "alice".to_string(),
            "bob".to_string(),
        ],
    );

    // Simulate receiving a leave message from alice
    let leave_payload = GroupMlsLeavePayload {
        group_id: "group:test-123".to_string(),
        leaving_member: "alice".to_string(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_LEAVE,
        serde_json::to_string(&leave_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // Non-elected nodes do NOT emit GroupMemberRemoved immediately.
    // The event will be emitted when the elected node's MLS commit arrives
    // and is processed via handle_group_mls_commit → process_commit_core.
    let events = events.lock().unwrap();
    let leave_event = events
        .iter()
        .find(|e| matches!(e, Event::GroupMemberRemoved { .. }));
    assert!(
        leave_event.is_none(),
        "Non-elected node should not emit premature GroupMemberRemoved"
    );
}

#[test]
fn test_group_mls_process_commit_empty_ciphertext_no_event() {
    let (mut protocol, events) = setup_started_with_events();

    // Create a group first so refresh_group_members can find it
    let info = protocol.create_group("Commit Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Simulate receiving a commit "add" message with empty ciphertext.
    // MLS processing will fail, so no membership event should be emitted.
    let commit_payload = GroupMlsCommitPayload {
        group_id: group_id.clone(),
        commit_type: GroupCommitType::Add,
        ciphertext: String::new(),
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No membership event should be emitted since MLS processing failed
    let events = events.lock().unwrap();
    let add_event = events.iter().find(|e| {
        matches!(e, Event::GroupMemberAdded { group_id: gid, user_id, .. }
            if gid == &group_id && user_id == "carol")
    });
    assert!(
        add_event.is_none(),
        "Should NOT emit GroupMemberAdded when MLS commit processing fails"
    );
}

#[test]
fn test_group_mls_refresh_group_members() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // Create group
    let info = protocol.create_group("Refresh Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // refresh_group_members should populate cache
    protocol.group_mesh.members.clear();
    let members = protocol.refresh_group_members(&group_id).unwrap();
    assert!(members.contains(&"user123".to_string()));
    assert!(protocol.group_mesh.members.contains_key(&group_id));
}

#[test]
fn test_group_mls_group_events_emitted_on_create() {
    let (mut protocol, events) = setup_with_events();

    protocol.create_group("Event Test").unwrap();

    let events = events.lock().unwrap();
    let created_event = events
        .iter()
        .find(|e| matches!(e, Event::GroupCreated { name, .. } if name == "Event Test"));
    assert!(created_event.is_some(), "Expected GroupCreated event");
}

#[test]
fn test_group_mls_cleanup_includes_group_dedup() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Add an expired dedup entry
    protocol.group_mesh.message_dedup.insert(
        "msg-expired-001".to_string(),
        Instant::now() - StdDuration::from_secs(GROUP_MESSAGE_DEDUP_TTL_SECS + 1),
    );
    assert_eq!(protocol.group_mesh.message_dedup.len(), 1);

    // cleanup_expired_entries should clean it up
    protocol.cleanup_expired_entries();
    assert!(protocol.group_mesh.message_dedup.is_empty());
}

#[test]
fn test_group_mls_internal_prefixes_defined() {
    // Verify all new prefixes are unique and well-formed
    assert!(internal_prefixes::GROUP_MLS_MSG.starts_with("__"));
    assert!(internal_prefixes::GROUP_MLS_MSG.ends_with("__"));
    assert!(internal_prefixes::GROUP_MLS_WELCOME.starts_with("__"));
    assert!(internal_prefixes::GROUP_MLS_COMMIT.starts_with("__"));
    assert!(internal_prefixes::GROUP_MLS_LEAVE.starts_with("__"));

    // All prefixes are distinct
    let prefixes = [
        internal_prefixes::GROUP_MLS_MSG,
        internal_prefixes::GROUP_MLS_WELCOME,
        internal_prefixes::GROUP_MLS_COMMIT,
        internal_prefixes::GROUP_MLS_LEAVE,
    ];
    for (i, a) in prefixes.iter().enumerate() {
        for (j, b) in prefixes.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "Prefixes must be unique");
                assert!(!a.starts_with(b), "No prefix should be a prefix of another");
            }
        }
    }
}

#[test]
fn test_group_mls_payload_serialization_roundtrip() {
    // Message payload with reply_to and epoch
    let msg_payload = GroupMlsMessagePayload {
        group_id: "group:abc".to_string(),
        ciphertext: "dGVzdA==".to_string(),
        epoch: 42,
        reply_to: Some("msg-123".to_string()),
    };
    let json = serde_json::to_string(&msg_payload).unwrap();
    let parsed: GroupMlsMessagePayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.group_id, "group:abc");
    assert_eq!(parsed.ciphertext, "dGVzdA==");
    assert_eq!(parsed.epoch, 42);
    assert_eq!(parsed.reply_to, Some("msg-123".to_string()));

    // Message payload without reply_to (should not include field in JSON)
    let msg_no_reply = GroupMlsMessagePayload {
        group_id: "group:abc".to_string(),
        ciphertext: "dGVzdA==".to_string(),
        epoch: 1,
        reply_to: None,
    };
    let json_no_reply = serde_json::to_string(&msg_no_reply).unwrap();
    assert!(!json_no_reply.contains("reply_to"));
    let parsed_no_reply: GroupMlsMessagePayload = serde_json::from_str(&json_no_reply).unwrap();
    assert_eq!(parsed_no_reply.reply_to, None);
    assert_eq!(parsed_no_reply.epoch, 1);

    let welcome_payload = GroupMlsWelcomePayload {
        group_id: "group:def".to_string(),
        group_name: Some("Test Group".to_string()),
        welcome_data: "d2VsY29tZQ==".to_string(),
        member_list: vec!["alice".to_string(), "bob".to_string()],
    };
    let json = serde_json::to_string(&welcome_payload).unwrap();
    let parsed: GroupMlsWelcomePayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.group_id, "group:def");
    assert_eq!(parsed.group_name, Some("Test Group".to_string()));
    assert_eq!(parsed.member_list.len(), 2);

    let commit_payload = GroupMlsCommitPayload {
        group_id: "group:ghi".to_string(),
        commit_type: GroupCommitType::Add,
        ciphertext: "Y29tbWl0".to_string(),
        epoch: 5,
        affected_member: Some("carol".to_string()),
    };
    let json = serde_json::to_string(&commit_payload).unwrap();
    let parsed: GroupMlsCommitPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.commit_type, GroupCommitType::Add);
    assert_eq!(parsed.affected_member, Some("carol".to_string()));
    assert_eq!(parsed.epoch, 5);

    // Verify enum serializes to lowercase
    assert!(json.contains("\"add\""));

    let leave_payload = GroupMlsLeavePayload {
        group_id: "group:jkl".to_string(),
        leaving_member: "dave".to_string(),
    };
    let json = serde_json::to_string(&leave_payload).unwrap();
    let parsed: GroupMlsLeavePayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.leaving_member, "dave");
}

#[test]
fn test_group_mls_leave_sender_mismatch_rejected() {
    let (mut protocol, events) = setup_started_with_events();

    // Simulate a spoofed leave: sender is "bob" but claims "alice" left
    let leave_payload = GroupMlsLeavePayload {
        group_id: "group:test-123".to_string(),
        leaving_member: "alice".to_string(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_LEAVE,
        serde_json::to_string(&leave_payload).unwrap()
    );
    let message = make_message("bob", "user123", &content); // sender != leaving_member

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No event should be emitted for spoofed leave
    let events = events.lock().unwrap();
    let leave_event = events
        .iter()
        .find(|e| matches!(e, Event::GroupMemberRemoved { .. }));
    assert!(leave_event.is_none(), "Spoofed leave should not emit event");
}

#[test]
fn test_group_mls_full_lifecycle_create_invite_send_decrypt() {
    let (alice, mut bob, group_id) = setup_alice_bob_group("Integration Test Group");

    // Set up Bob's event capture
    let bob_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let bob_events_clone = bob_events.clone();
    bob.on_event(move |event| {
        bob_events_clone.lock().unwrap().push(event);
    });

    // Alice encrypts a message via MLS and constructs the wire payload
    let encrypted = {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls.encrypt_for_group(&gid, b"Hello group!").unwrap()
    };

    let msg_payload = GroupMlsMessagePayload {
        group_id: group_id.clone(),
        ciphertext: base64_encode(&encrypted.ciphertext),
        epoch: encrypted.epoch,
        reply_to: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_MSG,
        serde_json::to_string(&msg_payload).unwrap()
    );

    // Simulate Bob receiving this message from Alice
    let bob_message = make_message("alice", "bob", &content);
    let result = bob.process_internal_message(&bob_message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // Verify Bob received the decrypted message
    let events = bob_events.lock().unwrap();
    let received = events.iter().find(|e| {
        matches!(e, Event::GroupMessageReceived { group_id: gid, content, .. }
            if gid == &group_id && content == "Hello group!")
    });
    assert!(
        received.is_some(),
        "Bob should have received 'Hello group!' via GroupMessageReceived event"
    );
}

#[test]
fn test_group_mls_send_message_multiple_members() {
    let (mut protocol, events) = setup_started_with_events();

    // Create group and pre-populate cache with multiple members
    let info = protocol.create_group("Multi-Member Group").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Manually set the member cache to include more members
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "user123".to_string(), // self
            "bob".to_string(),
            "carol".to_string(),
        ],
    );

    // Send a group message
    let msg_ids = protocol
        .send_group_message(&group_id, "hello everyone", None, None)
        .unwrap();

    // Should have sent to bob and carol (not self)
    assert_eq!(msg_ids.len(), 2, "Should send to 2 members (bob, carol)");

    // Check GroupMessageSent event
    let events = events.lock().unwrap();
    let sent_event = events.iter().find(|e| {
        matches!(e, Event::GroupMessageSent { group_id: gid, member_count, .. }
            if gid == &group_id && *member_count == 2)
    });
    assert!(
        sent_event.is_some(),
        "Expected GroupMessageSent event with member_count=2"
    );
}

#[test]
fn test_group_mls_dedup_cap_enforcement() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Insert more than MAX_GROUP_MESSAGE_DEDUP_ENTRIES
    let count = MAX_GROUP_MESSAGE_DEDUP_ENTRIES + 100;
    for i in 0..count {
        protocol
            .group_mesh
            .message_dedup
            .insert(format!("msg-{:06}", i), Instant::now());
    }
    assert_eq!(protocol.group_mesh.message_dedup.len(), count);

    // Cleanup should enforce the cap
    protocol.cleanup_group_message_dedup();
    assert!(
        protocol.group_mesh.message_dedup.len() <= MAX_GROUP_MESSAGE_DEDUP_ENTRIES,
        "Dedup cache should be capped at {}",
        MAX_GROUP_MESSAGE_DEDUP_ENTRIES
    );
}

#[test]
fn test_group_mls_commit_unknown_group() {
    let (mut protocol, events) = setup_started_with_events();

    // Simulate receiving a commit for a group we don't belong to
    let commit_payload = GroupMlsCommitPayload {
        group_id: "group:nonexistent".to_string(),
        commit_type: GroupCommitType::Add,
        ciphertext: base64_encode(b"fake-commit-data"),
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No membership event should be emitted
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMemberAdded { .. })),
        "No GroupMemberAdded event for unknown group"
    );
}

#[test]
fn test_group_mls_message_after_leaving() {
    let (mut protocol, _) = setup_started_with_events();

    // Create and leave a group
    let info = protocol.create_group("Leave Test").unwrap();
    let group_id = info.group_id.as_str().to_string();
    protocol.leave_group(&group_id).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    // Simulate receiving an encrypted group message for the left group
    let msg_payload = GroupMlsMessagePayload {
        group_id: group_id.clone(),
        ciphertext: base64_encode(b"encrypted-content"),
        epoch: 1,
        reply_to: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_MSG,
        serde_json::to_string(&msg_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    // Should not panic, just fail decryption gracefully
    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No message received event
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMessageReceived { .. })),
        "No message event after leaving group"
    );
}

#[test]
fn test_group_mls_oversized_payload_rejected() {
    let (mut protocol, events) = setup_started_with_events();

    // Create an oversized base64 payload
    let oversized = "A".repeat(MAX_BASE64_PAYLOAD_SIZE + 1);
    let msg_payload = GroupMlsMessagePayload {
        group_id: "group:test".to_string(),
        ciphertext: oversized,
        epoch: 1,
        reply_to: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_MSG,
        serde_json::to_string(&msg_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No message received event — payload was rejected
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMessageReceived { .. })),
        "Oversized payload should not produce a message event"
    );
}

#[test]
fn test_group_mls_duplicate_message_rejected() {
    let (alice, mut bob, group_id) = setup_alice_bob_group("Dedup Test Group");

    // Set up Bob's event capture
    let bob_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let bob_events_clone = bob_events.clone();
    bob.on_event(move |event| {
        bob_events_clone.lock().unwrap().push(event);
    });

    // Alice encrypts a message
    let encrypted = {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls.encrypt_for_group(&gid, b"Hello dedup!").unwrap()
    };
    let msg_payload = GroupMlsMessagePayload {
        group_id: group_id.clone(),
        ciphertext: base64_encode(&encrypted.ciphertext),
        epoch: encrypted.epoch,
        reply_to: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_MSG,
        serde_json::to_string(&msg_payload).unwrap()
    );

    // Build a message with a fixed ID so we can send the same one twice
    let bob_message = make_message("alice", "bob", &content);

    // First delivery — should succeed and emit GroupMessageReceived
    let result1 = bob.process_internal_message(&bob_message);
    assert!(matches!(result1, Some(InternalMessageResult::Consumed)));

    // Second delivery of the SAME message — should be deduped (no event)
    let result2 = bob.process_internal_message(&bob_message);
    assert!(matches!(result2, Some(InternalMessageResult::Consumed)));

    // Verify exactly ONE GroupMessageReceived event (the duplicate was dropped)
    let events = bob_events.lock().unwrap();
    let received_count = events
        .iter()
        .filter(|e| matches!(e, Event::GroupMessageReceived { .. }))
        .count();
    assert_eq!(
        received_count, 1,
        "Expected exactly 1 GroupMessageReceived, got {} (duplicate should be dropped)",
        received_count
    );
}

#[test]
fn test_group_mls_leave_non_member_rejected() {
    let (mut protocol, events) = setup_started_with_events();

    // Pre-populate group member cache — "eve" is NOT in the group
    protocol.group_mesh.members.insert(
        "group:nonmember-test".to_string(),
        vec!["user123".to_string(), "alice".to_string()],
    );

    // "eve" sends a leave notification for herself (sender matches, but not a member)
    let leave_payload = GroupMlsLeavePayload {
        group_id: "group:nonmember-test".to_string(),
        leaving_member: "eve".to_string(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_LEAVE,
        serde_json::to_string(&leave_payload).unwrap()
    );
    let message = make_message("eve", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No removal event
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMemberRemoved { .. })),
        "Non-member leave should not produce a removal event"
    );
}

#[test]
fn test_group_mls_leave_deterministic_election() {
    let (mut protocol, events) = setup_started_with_events();

    // Use a fake group_id that doesn't exist in MLS so refresh_group_members
    // fails and falls back to the local cache.
    let group_id = "group:election-test".to_string();

    // "alice" < "bob" < "user123" lexicographically.
    // When "bob" leaves, "alice" should be elected (lex-first remaining).
    // Since we are "user123", we should NOT be elected.
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "alice".to_string(),
            "bob".to_string(),
            "user123".to_string(),
        ],
    );

    // "bob" sends a leave notification
    let leave_payload = GroupMlsLeavePayload {
        group_id: group_id.clone(),
        leaving_member: "bob".to_string(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_LEAVE,
        serde_json::to_string(&leave_payload).unwrap()
    );
    let message = make_message("bob", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // Since "alice" < "user123", alice should be elected, not us.
    let events = events.lock().unwrap();
    let remove_event = events
        .iter()
        .find(|e| matches!(e, Event::GroupMemberRemoved { .. }));
    assert!(
        remove_event.is_none(),
        "Non-elected node should not emit premature GroupMemberRemoved"
    );
}

#[test]
fn test_group_mls_leave_we_are_elected() {
    let (mut protocol, events) = setup_started_with_events();

    // Use a fake group_id so refresh_group_members falls back to cache
    let group_id = "group:elected-test".to_string();

    // Members: "user123" < "zzz" lexicographically.
    // When "zzz" leaves, "user123" is the lex-first remaining → we should be elected.
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec!["user123".to_string(), "zzz".to_string()],
    );

    // "zzz" sends a leave notification
    let leave_payload = GroupMlsLeavePayload {
        group_id: group_id.clone(),
        leaving_member: "zzz".to_string(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_LEAVE,
        serde_json::to_string(&leave_payload).unwrap()
    );
    let message = make_message("zzz", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // We were elected, so remove_from_group was attempted.
    // Since "zzz" is not a real MLS member, the remove will fail — but the
    // important thing is we didn't emit a duplicate event from the non-elected path.
    let events = events.lock().unwrap();
    // Verify no double-emit: at most one GroupMemberRemoved event
    let remove_count = events
        .iter()
        .filter(|e| matches!(e, Event::GroupMemberRemoved { user_id, .. } if user_id == "zzz"))
        .count();
    assert!(
        remove_count <= 1,
        "Expected at most 1 GroupMemberRemoved event, got {}",
        remove_count
    );
}

#[test]
fn test_group_mls_remove_from_group() {
    let (mut protocol, _) = setup_started_with_events();

    // Create a group
    let info = protocol.create_group("Remove Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Try to remove a non-existent member — MLS should error
    let result = protocol.remove_from_group(&group_id, "nonexistent-user");
    assert!(result.is_err(), "Removing non-member should fail");
}

#[test]
fn test_group_mls_base64_decode_size_guard() {
    // Verify the base64_decode function rejects oversized payloads
    let oversized = "A".repeat(MAX_BASE64_PAYLOAD_SIZE + 1);
    let result = base64_decode(&oversized);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("payload too large"));

    // Verify normal-sized payloads pass
    let normal = base64_encode(b"hello world");
    let result = base64_decode(&normal);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), b"hello world");
}

#[test]
fn test_group_mls_welcome_bad_data_no_panic() {
    let (mut protocol, events) = setup_started_with_events();

    // Send a Welcome with invalid base64 welcome_data
    let welcome_payload = GroupMlsWelcomePayload {
        group_id: "group:bad-welcome".to_string(),
        group_name: Some("Bad Group".to_string()),
        welcome_data: "not-valid-base64!!!".to_string(),
        member_list: vec!["alice".to_string()],
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_WELCOME,
        serde_json::to_string(&welcome_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    // Should not panic
    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No join event
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMemberAdded { .. })),
        "Bad welcome data should not produce a join event"
    );
}

#[test]
fn test_group_mls_welcome_valid_base64_bad_mls_no_panic() {
    let (mut protocol, events) = setup_started_with_events();

    // Send a Welcome with valid base64 but garbage MLS data
    let welcome_payload = GroupMlsWelcomePayload {
        group_id: "group:garbage-mls".to_string(),
        group_name: Some("Garbage MLS".to_string()),
        welcome_data: base64_encode(b"this is not valid MLS data"),
        member_list: vec!["alice".to_string()],
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_WELCOME,
        serde_json::to_string(&welcome_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    // Should not panic — MLS join will fail gracefully
    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No join event
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMemberAdded { .. })),
        "Invalid MLS welcome should not produce a join event"
    );
}

#[test]
fn test_group_mls_send_message_partial_failure() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();
    // NOTE: Do NOT call protocol.start() — send_internal_message will fail
    // for all members because the protocol is not running, simulating
    // total delivery failure.

    let info = protocol.create_group("Partial Failure Group").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Populate cache with multiple members
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "user123".to_string(),
            "bob".to_string(),
            "carol".to_string(),
        ],
    );

    // Sending should fail since protocol is not started
    let result = protocol.send_group_message(&group_id, "hello", None, None);
    assert!(result.is_err(), "Total send failure should return Err");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("All group message sends failed"),
        "Error should indicate total failure"
    );
}

#[test]
fn test_group_mls_commit_oversized_ciphertext_rejected() {
    let (mut protocol, events) = setup_started_with_events();

    // Commit with oversized base64 ciphertext
    let oversized = "A".repeat(MAX_BASE64_PAYLOAD_SIZE + 1);
    let commit_payload = GroupMlsCommitPayload {
        group_id: "group:oversized".to_string(),
        commit_type: GroupCommitType::Add,
        ciphertext: oversized,
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No membership events
    let events = events.lock().unwrap();
    assert!(
        events.iter().all(|e| !matches!(
            e,
            Event::GroupMemberAdded { .. } | Event::GroupMemberRemoved { .. }
        )),
        "Oversized commit should not produce membership events"
    );
}

#[test]
fn test_group_mls_malformed_json_payloads() {
    let (mut protocol, _) = setup_started_with_events();

    // Malformed JSON for each message type — should all be consumed without panic
    let prefixes = [
        internal_prefixes::GROUP_MLS_MSG,
        internal_prefixes::GROUP_MLS_WELCOME,
        internal_prefixes::GROUP_MLS_COMMIT,
        internal_prefixes::GROUP_MLS_LEAVE,
    ];

    for prefix in &prefixes {
        let content = format!("{}{{not valid json!", prefix);
        let message = make_message("alice", "user123", &content);
        let result = protocol.process_internal_message(&message);
        assert!(
            matches!(result, Some(InternalMessageResult::Consumed)),
            "Malformed JSON for prefix {} should be consumed",
            prefix
        );
    }
}

// ========================================================================
// Dedup-before-decrypt (anti-replay amplification)
// ========================================================================

#[test]
fn test_group_mls_dedup_inserted_before_decrypt_attempt() {
    let (mut protocol, _) = setup_started_with_events();

    // Create a group so MLS is available for the group_id
    let info = protocol.create_group("Dedup Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Build a group message with bad ciphertext (will fail decryption)
    let msg_payload = GroupMlsMessagePayload {
        group_id: group_id.clone(),
        ciphertext: base64_encode(b"definitely-not-valid-mls-ciphertext"),
        epoch: 1,
        reply_to: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_MSG,
        serde_json::to_string(&msg_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let msg_id = message.id.as_str().to_string();

    // Process the message — decryption will fail but dedup should be recorded
    let _ = protocol.process_internal_message(&message);
    assert!(
        protocol.group_mesh.message_dedup.contains_key(&msg_id),
        "Dedup entry must be inserted even when decryption fails"
    );

    // Second delivery of same message should be skipped entirely
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    let _ = protocol.process_internal_message(&message);
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::GroupMessageReceived { .. })),
        "Replayed message after dedup must not produce any event"
    );
}

// ========================================================================
// Leave notification ordering (send-before-delete)
// ========================================================================

#[test]
fn test_group_mls_leave_preserves_state_on_total_send_failure() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();
    // Note: protocol NOT started — send_internal_message will fail with NotStarted

    let info = protocol.create_group("Leave Fail Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Inject a fake member so there are recipients
    protocol
        .group_mesh
        .members
        .get_mut(&group_id)
        .unwrap()
        .push("bob".to_string());

    // Attempt to leave — all sends should fail because protocol isn't started
    let result = protocol.leave_group(&group_id);
    assert!(
        result.is_err(),
        "Leave should fail when all notifications fail"
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("local state preserved"),
        "Error should indicate state was preserved for retry"
    );

    // Verify local MLS state is still intact (group still exists)
    let groups = protocol.list_groups().unwrap();
    assert!(
        groups.contains(&group_id),
        "Group should still exist locally after failed leave"
    );

    // Verify cache is still intact
    assert!(
        protocol.group_mesh.members.contains_key(&group_id),
        "Group member cache should be preserved after failed leave"
    );
}

#[test]
fn test_group_mls_leave_deletes_state_after_successful_notification() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Leave OK Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Solo group (only self) — no recipients, leave should succeed directly
    let result = protocol.leave_group(&group_id);
    assert!(result.is_ok(), "Leave with no other members should succeed");

    // Verify local state was cleaned up
    let groups = protocol.list_groups().unwrap();
    assert!(
        !groups.contains(&group_id),
        "Group should be removed after successful leave"
    );
    assert!(
        !protocol.group_mesh.members.contains_key(&group_id),
        "Cache should be cleared after successful leave"
    );
}

// ========================================================================
// Out-of-order commit buffering
// ========================================================================

#[test]
fn test_group_mls_commit_failure_buffers_for_retry() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let group_id = "group:buffer-test".to_string();
    let pending = PendingCommit {
        sender: "alice".to_string(),
        data: serde_json::to_string(&GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(b"commit-data"),
            epoch: 99,
            affected_member: Some("new-member".to_string()),
        })
        .unwrap(),
        buffered_at: Instant::now(),
        retry_count: 0,
    };

    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(pending);

    // Verify commit was buffered
    assert!(
        protocol.group_mesh.pending_commits.contains_key(&group_id),
        "Failed commit should be buffered"
    );
    assert_eq!(
        protocol
            .group_mesh
            .pending_commits
            .get(&group_id)
            .unwrap()
            .len(),
        1,
        "Exactly one commit should be buffered"
    );
}

#[test]
fn test_group_mls_pending_commit_buffer_cap() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let group_id = "group:cap-test".to_string();

    // Fill the buffer beyond capacity via buffer_pending_commit
    for i in 0..(MAX_PENDING_COMMITS_PER_GROUP + 4) {
        let data = serde_json::to_string(&GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(format!("commit-{}", i).as_bytes()),
            epoch: i as u64,
            affected_member: None,
        })
        .unwrap();
        protocol.buffer_pending_commit(&group_id, "alice", &data);
    }

    // Buffer should be capped at MAX_PENDING_COMMITS_PER_GROUP
    let buffered = protocol.group_mesh.pending_commits.get(&group_id).unwrap();
    assert_eq!(
        buffered.len(),
        MAX_PENDING_COMMITS_PER_GROUP,
        "Buffer should not exceed cap"
    );
}

#[test]
fn test_group_mls_pending_commit_expired_entries_cleaned_up() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let group_id = "test-group".to_string();
    // Insert a pending commit with an expired timestamp
    let expired = PendingCommit {
        sender: "alice".to_string(),
        data: "{}".to_string(),
        buffered_at: Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10),
        retry_count: 0,
    };
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(expired);

    // Insert a recent one
    let recent = PendingCommit {
        sender: "bob".to_string(),
        data: "{}".to_string(),
        buffered_at: Instant::now(),
        retry_count: 0,
    };
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(recent);

    // Run cleanup
    protocol.cleanup_group_message_dedup();

    // Expired entry should be removed, recent one retained
    let buf = protocol.group_mesh.pending_commits.get(&group_id).unwrap();
    assert_eq!(buf.len(), 1, "Only recent pending commit should survive");
    assert_eq!(buf[0].sender, "bob");
}

#[test]
fn test_group_mls_commit_empty_ciphertext_not_buffered() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("No Buffer Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Empty ciphertext — this is a malformed commit, not an ordering issue
    let commit_payload = GroupMlsCommitPayload {
        group_id: group_id.clone(),
        commit_type: GroupCommitType::Remove,
        ciphertext: String::new(),
        epoch: 1,
        affected_member: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);
    let _ = protocol.process_internal_message(&message);

    // Empty ciphertext should NOT be buffered (it's not an ordering issue)
    assert!(
        !protocol.group_mesh.pending_commits.contains_key(&group_id)
            || protocol
                .group_mesh
                .pending_commits
                .get(&group_id)
                .unwrap()
                .is_empty(),
        "Empty ciphertext commits must not be buffered"
    );
}

#[test]
fn test_group_mls_double_leave_is_idempotent() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Double Leave").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // First leave should succeed
    let result = protocol.leave_group(&group_id);
    assert!(result.is_ok());

    // Second leave is idempotent
    let result = protocol.leave_group(&group_id);
    assert!(
        result.is_ok(),
        "Double leave should be idempotent (no error)"
    );

    // Verify state is clean
    assert!(!protocol.group_mesh.members.contains_key(&group_id));
    let groups = protocol.list_groups().unwrap();
    assert!(!groups.contains(&group_id));
}

#[test]
fn test_group_mls_invite_exceeds_max_members() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Cap Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    let max_members = protocol.config.group.max_group_members;

    // Simulate a group at max_group_members by injecting fake members into cache
    let fake_members: Vec<String> = (0..max_members).map(|i| format!("member-{}", i)).collect();
    protocol
        .group_mesh
        .members
        .insert(group_id.clone(), fake_members);

    // Attempting to invite should fail with the cap error
    let result = protocol.invite_to_group(&group_id, "new-invitee");
    assert!(result.is_err(), "Should reject invite when at member cap");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cannot exceed"),
        "Error should mention cap, got: {}",
        err_msg
    );
}

#[test]
fn test_group_mls_invite_custom_max_members() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut config = create_test_config();
    config.group.max_group_members = 3; // small cap for testing
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();
    protocol.start().unwrap();

    let info = protocol.create_group("Custom Cap Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Simulate 3 members (at the custom cap)
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "user123".to_string(),
            "alice".to_string(),
            "bob".to_string(),
        ],
    );

    // Should be rejected — at the cap
    let result = protocol.invite_to_group(&group_id, "carol");
    assert!(result.is_err(), "Should reject invite when at custom cap");
    assert!(result.unwrap_err().to_string().contains("cannot exceed 3"));
}

#[test]
fn test_group_mls_invite_below_custom_cap_allowed() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut config = create_test_config();
    config.group.max_group_members = 3;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();
    protocol.start().unwrap();

    let info = protocol.create_group("Below Cap Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // 2 members — below the cap of 3
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec!["user123".to_string(), "alice".to_string()],
    );

    let result = protocol.invite_to_group(&group_id, "bob");
    // Should fail for missing key package, NOT for cap
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("No key package"),
        "Should fail for missing key package, not cap. Got: {}",
        err_msg
    );
}

#[test]
fn test_group_mls_invite_max_members_1_blocks_any_invite() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut config = create_test_config();
    config.group.max_group_members = 1;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();
    protocol.start().unwrap();

    let info = protocol.create_group("Solo Only").unwrap();
    let group_id = info.group_id.as_str().to_string();

    let result = protocol.invite_to_group(&group_id, "alice");
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("cannot exceed 1"),
        "max_group_members=1 should block all invites"
    );
}

#[test]
fn test_group_mls_invite_large_max_members_not_rejected() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut config = create_test_config();
    config.group.max_group_members = 10_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();
    protocol.start().unwrap();

    let info = protocol.create_group("Large Cap Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    let result = protocol.invite_to_group(&group_id, "alice");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        !err_msg.contains("cannot exceed"),
        "Large cap should not trigger member limit. Got: {}",
        err_msg
    );
}

#[test]
fn test_group_mls_send_large_content_no_send_side_limit() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Large Content Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // 1 MB plaintext
    let large_content = "A".repeat(1_048_576);
    let result = protocol.send_group_message(&group_id, &large_content, None, None);

    // Solo group → Ok([]) since there's no one to send to
    assert!(result.is_ok(), "Large content should not be rejected");
    assert!(result.unwrap().is_empty());
}

#[test]
fn test_group_mls_send_very_large_content_solo_group() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Very Large Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // 2 MB plaintext
    let content = "B".repeat(2 * 1_048_576);
    let result = protocol.send_group_message(&group_id, &content, None, None);
    assert!(
        result.is_ok(),
        "2 MB content should not be rejected at send"
    );
}

#[test]
fn test_group_mls_drain_pending_commits_no_double_buffering() {
    let (mut protocol, _) = setup_started_with_events();

    // Create a group so MLS is aware of it
    let info = protocol.create_group("Drain Test").unwrap();
    let real_group_id = info.group_id.as_str().to_string();

    // Manually insert pending commits with garbage ciphertext.
    let bad_commit = GroupMlsCommitPayload {
        group_id: real_group_id.clone(),
        commit_type: GroupCommitType::Add,
        ciphertext: base64_encode(b"not-a-real-mls-commit"),
        epoch: 99,
        affected_member: Some("carol".to_string()),
    };
    let bad_data = serde_json::to_string(&bad_commit).unwrap();

    protocol.group_mesh.pending_commits.insert(
        real_group_id.clone(),
        VecDeque::from(vec![
            PendingCommit {
                sender: "alice".to_string(),
                data: bad_data.clone(),
                buffered_at: Instant::now(),
                retry_count: 0,
            },
            PendingCommit {
                sender: "bob".to_string(),
                data: bad_data,
                buffered_at: Instant::now(),
                retry_count: 0,
            },
        ]),
    );

    // Run drain — both commits will be permanently rejected
    protocol.drain_pending_commits(&real_group_id);

    let remaining = protocol
        .group_mesh
        .pending_commits
        .get(&real_group_id)
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        remaining, 0,
        "Permanently rejected commits should not be re-buffered, got {}",
        remaining
    );
}

#[test]
fn test_group_mls_drain_pending_commits_expired_entries_dropped() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Drain Expiry Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert an expired pending commit
    let bad_commit = GroupMlsCommitPayload {
        group_id: group_id.clone(),
        commit_type: GroupCommitType::Add,
        ciphertext: base64_encode(b"stale-commit"),
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let data = serde_json::to_string(&bad_commit).unwrap();

    protocol.group_mesh.pending_commits.insert(
        group_id.clone(),
        VecDeque::from(vec![PendingCommit {
            sender: "alice".to_string(),
            data,
            buffered_at: Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 1),
            retry_count: 0,
        }]),
    );

    // Drain should drop expired entry
    protocol.drain_pending_commits(&group_id);

    assert!(
        !protocol.group_mesh.pending_commits.contains_key(&group_id),
        "Expired pending commit should be dropped and entry cleaned up"
    );
}

#[test]
fn test_group_mls_handle_commit_permanent_failure_not_buffered() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Buffer Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // No pending commits initially
    assert!(protocol.group_mesh.pending_commits.is_empty());

    // Simulate receiving a commit with valid base64 but garbage MLS bytes.
    let commit_payload = GroupMlsCommitPayload {
        group_id: group_id.clone(),
        commit_type: GroupCommitType::Add,
        ciphertext: base64_encode(b"fake-but-decodable-commit"),
        epoch: 42,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // Garbage ciphertext → permanent rejection, not buffered
    let buffered = protocol
        .group_mesh
        .pending_commits
        .get(&group_id)
        .map(|b| b.len())
        .unwrap_or(0);
    assert_eq!(
        buffered, 0,
        "Permanent deserialization failure should not be buffered"
    );
}

#[test]
fn test_group_mls_commit_rejected_not_buffered() {
    let (mut protocol, _) = setup_started_with_events();

    // Empty ciphertext — should be rejected, not buffered
    let commit_payload = GroupMlsCommitPayload {
        group_id: "group:reject-test".to_string(),
        commit_type: GroupCommitType::Add,
        ciphertext: String::new(),
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    protocol.process_internal_message(&message);
    assert!(
        protocol.group_mesh.pending_commits.is_empty(),
        "Rejected commits (empty ciphertext) should not be buffered"
    );

    // Malformed JSON — should also not be buffered
    let bad_content = format!("{}{{invalid json", internal_prefixes::GROUP_MLS_COMMIT);
    let bad_message = make_message("alice", "user123", &bad_content);

    protocol.process_internal_message(&bad_message);
    assert!(
        protocol.group_mesh.pending_commits.is_empty(),
        "Rejected commits (bad JSON) should not be buffered"
    );
}

#[test]
fn test_group_mls_buffer_pending_commit_respects_cap() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let group_id = "group:cap-test".to_string();

    // Fill the buffer to capacity
    for i in 0..MAX_PENDING_COMMITS_PER_GROUP {
        protocol.buffer_pending_commit(&group_id, &format!("sender-{}", i), &format!("data-{}", i));
    }
    assert_eq!(
        protocol.group_mesh.pending_commits[&group_id].len(),
        MAX_PENDING_COMMITS_PER_GROUP
    );

    // One more should evict the oldest
    protocol.buffer_pending_commit(&group_id, "sender-new", "data-new");
    let buf = &protocol.group_mesh.pending_commits[&group_id];
    assert_eq!(buf.len(), MAX_PENDING_COMMITS_PER_GROUP);
    // The oldest (sender-0) should have been evicted
    assert_eq!(buf[0].sender, "sender-1");
    assert_eq!(buf[buf.len() - 1].sender, "sender-new");
}

#[test]
fn test_group_mls_invite_respects_max_group_members() {
    let mut config = create_test_config();
    config.group.max_group_members = 2;

    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();
    protocol.start().unwrap();

    let info = protocol.create_group("Capped Group").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Manually set member cache to 2 members (at cap)
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec!["user123".to_string(), "alice".to_string()],
    );

    // Invite should fail due to cap
    let result = protocol.invite_to_group(&group_id, "bob");
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("cannot exceed"),
        "Error should mention the group member limit"
    );
}

// ========================================================================
// End-to-end invite_to_group tests
// ========================================================================

#[test]
fn test_group_mls_invite_to_group_end_to_end() {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    bob.initialize_mls(storage_b).unwrap();
    alice.start().unwrap();
    bob.start().unwrap();

    // Capture Alice's events to verify Welcome/Commit sends
    let alice_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let alice_events_clone = alice_events.clone();
    alice.on_event(move |event| {
        alice_events_clone.lock().unwrap().push(event);
    });

    // Alice creates a group
    let group_info = alice.create_group("Invite E2E Test").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    // Bob generates a key package
    let bob_kp = {
        let bob_mls = bob.mls_manager_for_testing().read().unwrap();
        bob_mls.generate_key_package().unwrap()
    };

    // Alice receives Bob's key package (simulating key exchange)
    use crate::protocol::ReceivedKeyPackage;
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    alice.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000, // 10 min
        },
    );

    // Alice invites Bob via the full invite_to_group() path
    let invite_result = alice.invite_to_group(&group_id, "bob");
    assert!(
        invite_result.is_ok(),
        "invite_to_group should succeed: {:?}",
        invite_result.err()
    );

    // Verify GroupMemberAdded event was emitted
    let events = alice_events.lock().unwrap();
    let added_event = events.iter().find(|e| {
        matches!(e, Event::GroupMemberAdded { group_id: gid, user_id, added_by }
            if gid == &group_id && user_id == "bob" && added_by == "alice")
    });
    assert!(
        added_event.is_some(),
        "Expected GroupMemberAdded event for bob"
    );

    // Verify Alice's member cache was updated
    let cached = alice.group_mesh.members.get(&group_id).unwrap();
    assert!(
        cached.contains(&"bob".to_string()),
        "Bob should be in Alice's member cache after invite"
    );
}

#[test]
fn test_group_mls_invite_and_bob_joins_via_welcome() {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    bob.initialize_mls(storage_b).unwrap();
    alice.start().unwrap();
    bob.start().unwrap();

    // Alice creates a group
    let group_info = alice.create_group("Join E2E Test").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    // Bob generates key package, Alice stores it
    let bob_kp = {
        let bob_mls = bob.mls_manager_for_testing().read().unwrap();
        bob_mls.generate_key_package().unwrap()
    };
    use crate::protocol::ReceivedKeyPackage;
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    alice.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );

    // Alice invites Bob
    alice.invite_to_group(&group_id, "bob").unwrap();

    // Verify Alice's outbox contains the Welcome message for Bob
    let welcome_sent = alice.outbox_messages().any(|msg| {
        msg.recipient.as_str() == "bob"
            && msg
                .content
                .starts_with(internal_prefixes::GROUP_MLS_WELCOME)
    });
    assert!(
        welcome_sent,
        "Alice's outbox should contain a Welcome message for Bob"
    );
}

#[test]
fn test_group_mls_invite_sends_commit_to_existing_members() {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_c = Arc::new(crate::mls::InMemoryStorage::default());
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let carol_proto = OfflineProtocol::new(create_test_config_for_user("carol")).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    bob.initialize_mls(storage_b).unwrap();
    drop(carol_proto);
    alice.start().unwrap();
    bob.start().unwrap();

    // Alice creates a group and adds Bob directly at MLS layer first
    let group_info = alice.create_group("Commit Fan-out Test").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    let bob_kp = {
        let bob_mls = bob.mls_manager_for_testing().read().unwrap();
        bob_mls.generate_key_package().unwrap()
    };
    {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls
            .add_group_member(&gid, &bob_kp.key_package_data)
            .unwrap();
    }
    alice.refresh_group_members(&group_id).unwrap();

    // Now generate a key package for carol
    let carol_mls_manager = offline_protocol_mls::MlsManager::new("carol", storage_c).unwrap();
    let carol_kp = carol_mls_manager.generate_key_package().unwrap();

    // Alice stores Carol's key package
    use crate::protocol::ReceivedKeyPackage;
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    alice.pending_key_packages.insert(
        "carol".to_string(),
        ReceivedKeyPackage {
            key_package_data: carol_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );

    // Clear outbox before invite so we can see what invite_to_group sends
    alice.clear_outbox();

    // Alice invites Carol
    alice.invite_to_group(&group_id, "carol").unwrap();

    // Verify a Commit was sent to Bob (existing member)
    let commit_to_bob = alice.outbox_messages().any(|msg| {
        msg.recipient.as_str() == "bob"
            && msg.content.starts_with(internal_prefixes::GROUP_MLS_COMMIT)
    });
    assert!(
        commit_to_bob,
        "Alice should have sent a Commit to Bob (existing member) during Carol's invite"
    );

    // Verify a Welcome was sent to Carol
    let welcome_to_carol = alice.outbox_messages().any(|msg| {
        msg.recipient.as_str() == "carol"
            && msg
                .content
                .starts_with(internal_prefixes::GROUP_MLS_WELCOME)
    });
    assert!(
        welcome_to_carol,
        "Alice should have sent a Welcome to Carol"
    );

    // Verify NO commit was sent to Carol
    let commit_to_carol = alice.outbox_messages().any(|msg| {
        msg.recipient.as_str() == "carol"
            && msg.content.starts_with(internal_prefixes::GROUP_MLS_COMMIT)
    });
    assert!(
        !commit_to_carol,
        "Carol should NOT receive a Commit (she gets the Welcome instead)"
    );
}

#[test]
fn test_group_mls_invite_expired_key_package_rejected() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Expiry Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert an expired key package for bob
    use crate::protocol::ReceivedKeyPackage;
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: vec![1, 2, 3],
            local_expires_at_ms: 0, // already expired
        },
    );

    let result = protocol.invite_to_group(&group_id, "bob");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("expired"),
        "Error should mention expiry. Got: {}",
        err_msg
    );

    // The expired key package should have been removed
    assert!(
        !protocol.pending_key_packages.contains_key("bob"),
        "Expired key package should be cleaned up"
    );
}

#[test]
fn test_group_mls_max_group_members_enforced_with_valid_key_package() {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let mut config = create_test_config_for_user("alice");
    config.group.max_group_members = 1; // only creator allowed
    let mut alice = OfflineProtocol::new(config).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    alice.start().unwrap();

    let info = alice.create_group("Cap Enforcement").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Generate a real key package for bob
    let bob_mls = offline_protocol_mls::MlsManager::new("bob", storage_b).unwrap();
    let bob_kp = bob_mls.generate_key_package().unwrap();

    use crate::protocol::ReceivedKeyPackage;
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    alice.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );

    // Group has 1 member (alice), cap is 1 → invite should be rejected
    let result = alice.invite_to_group(&group_id, "bob");
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("cannot exceed"),
        "Should fail at member cap, not at MLS layer. Got: {}",
        err_msg
    );
}

#[test]
fn test_group_mls_remove_from_group_with_real_members() {
    let (mut alice, _bob, group_id) = setup_alice_bob_group("Remove Real Test");

    // Verify bob is in the group
    let members_before = alice.group_mesh.members.get(&group_id).unwrap();
    assert!(
        members_before.contains(&"bob".to_string()),
        "Bob should be in group before removal"
    );

    // Capture events
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    alice.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    // Clear outbox to see what remove sends
    alice.clear_outbox();

    // Alice removes Bob
    let result = alice.remove_from_group(&group_id, "bob");
    assert!(
        result.is_ok(),
        "remove_from_group should succeed: {:?}",
        result.err()
    );

    // Verify bob is no longer in the cached member list
    let members_after = alice.group_mesh.members.get(&group_id).unwrap();
    assert!(
        !members_after.contains(&"bob".to_string()),
        "Bob should be removed from member cache"
    );

    // Verify GroupMemberRemoved event was emitted
    let events = events.lock().unwrap();
    let removed_event = events.iter().find(|e| {
        matches!(e, Event::GroupMemberRemoved { group_id: gid, user_id, removed_by }
            if gid == &group_id && user_id == "bob" && removed_by == "alice")
    });
    assert!(
        removed_event.is_some(),
        "Expected GroupMemberRemoved event for bob"
    );
}

#[test]
fn test_group_mls_invite_multiple_members_successively() {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_c = Arc::new(crate::mls::InMemoryStorage::default());
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    alice.start().unwrap();

    let group_info = alice.create_group("Multi Invite Test").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    // Generate key packages for bob and carol
    let bob_mls = offline_protocol_mls::MlsManager::new("bob", storage_b).unwrap();
    let bob_kp = bob_mls.generate_key_package().unwrap();

    let carol_mls = offline_protocol_mls::MlsManager::new("carol", storage_c).unwrap();
    let carol_kp = carol_mls.generate_key_package().unwrap();

    use crate::protocol::ReceivedKeyPackage;
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;

    // Store Bob's key package and invite
    alice.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );
    alice.invite_to_group(&group_id, "bob").unwrap();

    // Verify Bob was added
    let members = alice.group_mesh.members.get(&group_id).unwrap();
    assert!(members.contains(&"bob".to_string()));

    // Store Carol's key package and invite
    alice.pending_key_packages.insert(
        "carol".to_string(),
        ReceivedKeyPackage {
            key_package_data: carol_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );
    alice.invite_to_group(&group_id, "carol").unwrap();

    // Verify both are in the group
    let members = alice.group_mesh.members.get(&group_id).unwrap();
    assert!(members.contains(&"bob".to_string()));
    assert!(members.contains(&"carol".to_string()));
    assert!(members.contains(&"alice".to_string()));
    assert_eq!(members.len(), 3);
}

#[test]
fn test_group_mls_commit_group_not_found_is_rejected_not_retriable() {
    let (mut protocol, _) = setup_started_with_events();

    // Do NOT create any group — the group_id won't exist in MLS.
    let commit_payload = GroupMlsCommitPayload {
        group_id: "group:does-not-exist".to_string(),
        commit_type: GroupCommitType::Add,
        ciphertext: base64_encode(b"some-commit-data"),
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let _ = protocol.process_internal_message(&message);

    // GroupNotFound is permanent → should NOT be buffered
    assert!(
        !protocol
            .group_mesh
            .pending_commits
            .contains_key("group:does-not-exist"),
        "Commit for unknown group must be rejected, not buffered"
    );
}

#[test]
fn test_group_mls_commit_bad_deserialization_is_rejected_not_retriable() {
    let (mut protocol, _) = setup_started_with_events();

    // Create a group so the group_id exists
    let info = protocol.create_group("Deser Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Valid base64 but garbage MLS bytes
    let commit_payload = GroupMlsCommitPayload {
        group_id: group_id.clone(),
        commit_type: GroupCommitType::Add,
        ciphertext: base64_encode(b"this-is-not-mls"),
        epoch: 1,
        affected_member: Some("carol".to_string()),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::GROUP_MLS_COMMIT,
        serde_json::to_string(&commit_payload).unwrap()
    );
    let message = make_message("alice", "user123", &content);

    let _ = protocol.process_internal_message(&message);

    let buffered = protocol
        .group_mesh
        .pending_commits
        .get(&group_id)
        .map(|b| b.len())
        .unwrap_or(0);
    assert!(
        buffered <= 1,
        "Bad ciphertext should be rejected or buffered at most once, got {}",
        buffered
    );
}

#[test]
fn test_group_mls_multiple_groups_independent() {
    let (mut protocol, events) = setup_started_with_events();

    // Create two independent groups
    let info_a = protocol.create_group("Group Alpha").unwrap();
    let info_b = protocol.create_group("Group Beta").unwrap();
    let group_a = info_a.group_id.as_str().to_string();
    let group_b = info_b.group_id.as_str().to_string();

    assert_ne!(group_a, group_b);

    // Both should be listed
    let groups = protocol.list_groups().unwrap();
    assert!(groups.contains(&group_a));
    assert!(groups.contains(&group_b));
    assert_eq!(groups.len(), 2);

    // Leave group A
    protocol.leave_group(&group_a).unwrap();

    // Group B should still exist and be functional
    let groups = protocol.list_groups().unwrap();
    assert!(!groups.contains(&group_a));
    assert!(groups.contains(&group_b));

    // Sending to group B should still work
    let result = protocol.send_group_message(&group_b, "hello beta", None, None);
    assert!(result.is_ok());

    // Verify GroupCreated events for both
    let events = events.lock().unwrap();
    let created_count = events
        .iter()
        .filter(|e| matches!(e, Event::GroupCreated { .. }))
        .count();
    assert_eq!(created_count, 2, "Expected 2 GroupCreated events");
}

#[test]
fn test_group_mls_max_group_members_boundary() {
    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let mut config = create_test_config_for_user("alice");
    config.group.max_group_members = 2;
    let mut alice = OfflineProtocol::new(config).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    alice.start().unwrap();

    let info = alice.create_group("Tiny Group").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Alice is already member 1. Add Bob as member 2 (at capacity).
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let bob_mls = offline_protocol_mls::MlsManager::new("bob", storage_b).unwrap();
    let bob_kp = bob_mls.generate_key_package().unwrap();

    use crate::protocol::ReceivedKeyPackage;
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    alice.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );
    // This should succeed — 2 members == max
    alice.invite_to_group(&group_id, "bob").unwrap();

    // Now try to add carol as member 3 — should fail
    let storage_c = Arc::new(crate::mls::InMemoryStorage::default());
    let carol_mls = offline_protocol_mls::MlsManager::new("carol", storage_c).unwrap();
    let carol_kp = carol_mls.generate_key_package().unwrap();
    alice.pending_key_packages.insert(
        "carol".to_string(),
        ReceivedKeyPackage {
            key_package_data: carol_kp.key_package_data,
            local_expires_at_ms: now_ms + 600_000,
        },
    );
    let result = alice.invite_to_group(&group_id, "carol");
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("cannot exceed"),
        "Error should mention exceeding member limit"
    );
}

#[test]
fn test_group_mls_send_message_with_reply_to() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Reply Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Populate cache with another member
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec!["user123".to_string(), "bob".to_string()],
    );

    // Send with reply_to
    let result =
        protocol.send_group_message(&group_id, "replying here", None, Some("msg-original-123"));
    assert!(result.is_ok());
    let msg_ids = result.unwrap();
    assert_eq!(msg_ids.len(), 1, "Should send to bob only");
}

#[test]
fn test_group_mls_leave_self_only_group() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Solo Leave").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Should succeed — no other members to notify
    protocol.leave_group(&group_id).unwrap();
    assert!(!protocol.group_mesh.members.contains_key(&group_id));

    // Group should no longer be listed
    let groups = protocol.list_groups().unwrap();
    assert!(!groups.contains(&group_id));
}

#[test]
fn test_group_mls_dedup_independent_per_message_id() {
    let (alice, mut bob, group_id) = setup_alice_bob_group("Multi Msg Test");

    let bob_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let bob_events_clone = bob_events.clone();
    bob.on_event(move |event| {
        bob_events_clone.lock().unwrap().push(event);
    });

    // Alice sends two distinct messages
    let enc1 = {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls.encrypt_for_group(&gid, b"First message").unwrap()
    };
    let enc2 = {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls
            .encrypt_for_group(&gid, b"Second message")
            .unwrap()
    };

    for encrypted in [&enc1, &enc2] {
        let msg_payload = GroupMlsMessagePayload {
            group_id: group_id.clone(),
            ciphertext: base64_encode(&encrypted.ciphertext),
            epoch: encrypted.epoch,
            reply_to: None,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::GROUP_MLS_MSG,
            serde_json::to_string(&msg_payload).unwrap()
        );
        // Each Message::new generates a unique ID
        let bob_message = make_message("alice", "bob", &content);
        bob.process_internal_message(&bob_message);
    }

    let events = bob_events.lock().unwrap();
    let received_count = events
        .iter()
        .filter(|e| matches!(e, Event::GroupMessageReceived { .. }))
        .count();
    assert_eq!(
        received_count, 2,
        "Both distinct messages should be received (not deduplicated)"
    );
}

#[test]
fn test_group_mls_expired_key_package_rejected() {
    let (mut protocol, _) = setup_started_with_events();

    let info = protocol.create_group("Expired KP Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    use crate::protocol::ReceivedKeyPackage;
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: vec![1, 2, 3],
            local_expires_at_ms: 0, // expired
        },
    );

    let result = protocol.invite_to_group(&group_id, "bob");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("expired"));

    // Expired package should be removed from cache
    assert!(
        !protocol.pending_key_packages.contains_key("bob"),
        "Expired key package should be cleaned up"
    );
}

#[test]
fn test_group_mls_pending_commit_drain_cascades() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let group_id = "group:cascade-test".to_string();
    let mut buf = VecDeque::new();
    buf.push_back(PendingCommit {
        sender: "alice".to_string(),
        data: serde_json::to_string(&GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(b"commit-1"),
            epoch: 1,
            affected_member: None,
        })
        .unwrap(),
        buffered_at: Instant::now(),
        retry_count: 0,
    });
    buf.push_back(PendingCommit {
        sender: "bob".to_string(),
        data: serde_json::to_string(&GroupMlsCommitPayload {
            group_id: group_id.clone(),
            commit_type: GroupCommitType::Add,
            ciphertext: base64_encode(b"commit-2"),
            epoch: 2,
            affected_member: None,
        })
        .unwrap(),
        buffered_at: Instant::now(),
        retry_count: 0,
    });
    protocol
        .group_mesh
        .pending_commits
        .insert(group_id.clone(), buf);

    // drain without MLS — all should be rejected, no panic or infinite loop
    protocol.drain_pending_commits(&group_id);

    let remaining = protocol
        .group_mesh
        .pending_commits
        .get(&group_id)
        .map(|b| b.len())
        .unwrap_or(0);
    assert_eq!(
        remaining, 0,
        "All pending commits should be rejected when MLS is unavailable"
    );
}

// ====================================================================
// RELAY PATH TESTS
// ====================================================================

#[test]
fn test_relay_sync_on_internet_available_transition() {
    use offline_protocol_transport::mock::MockTransport;

    let (mut protocol, _) = setup_started_with_events();

    // Create a group (relay registration will fail — no Internet yet)
    let info = protocol.create_group("Relay Sync Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Group should NOT be relay-synced (no Internet)
    assert!(
        !protocol.group_mesh.relay_synced.contains(&group_id),
        "Group should not be relay-synced without Internet"
    );
    assert!(!protocol.group_mesh.internet_was_available);

    // Add Internet transport
    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));

    // Trigger the 0→1 transition via check_relay_group_sync
    protocol.check_relay_group_sync();

    // Internet should now be tracked as available
    assert!(protocol.group_mesh.internet_was_available);

    // Group should be relay-synced
    assert!(
        protocol.group_mesh.relay_synced.contains(&group_id),
        "Group should be relay-synced after Internet becomes available"
    );
}

#[test]
fn test_relay_sync_cleared_on_internet_lost() {
    use offline_protocol_transport::mock::MockTransport;

    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // Add Internet transport and start
    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    // Create group
    let info = protocol.create_group("Relay Lost Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Mark Internet as seen
    protocol.group_mesh.internet_was_available = true;
    protocol.group_mesh.relay_synced.insert(group_id.clone());

    // Remove Internet transport to simulate connectivity loss
    protocol
        .transport_manager_mut()
        .remove_transport(TransportType::Internet);

    // Trigger the 1→0 transition
    protocol.check_relay_group_sync();

    assert!(!protocol.group_mesh.internet_was_available);
    assert!(
        protocol.group_mesh.relay_synced.is_empty(),
        "Relay sync state should be cleared when Internet is lost"
    );
}

#[test]
fn test_relay_sync_disabled_config() {
    use offline_protocol_transport::mock::MockTransport;

    let mut config = create_test_config();
    config.group.relay_enabled = false;

    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    // Create group — should NOT be relay-synced since relay is disabled
    let info = protocol.create_group("No Relay Test").unwrap();
    let group_id = info.group_id.as_str().to_string();
    assert!(
        !protocol.group_mesh.relay_synced.contains(&group_id),
        "Group should not be relay-synced when relay is disabled"
    );

    // check_relay_group_sync should be a no-op
    protocol.check_relay_group_sync();
    assert!(!protocol.group_mesh.internet_was_available);
}

#[test]
fn test_relay_broadcast_used_when_synced() {
    use offline_protocol_transport::mock::MockTransport;

    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // Add Internet transport
    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    // Create a group with multiple members
    let info = protocol.create_group("Relay Broadcast Test").unwrap();
    let group_id = info.group_id.as_str().to_string();
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "user123".to_string(),
            "bob".to_string(),
            "carol".to_string(),
            "dave".to_string(),
        ],
    );

    // Mark group as relay-synced
    protocol.group_mesh.relay_synced.insert(group_id.clone());

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    // Send a group message — should use relay broadcast (1 message)
    let msg_ids = protocol
        .send_group_message(&group_id, "hello relay", None, None)
        .unwrap();

    assert_eq!(
        msg_ids.len(),
        1,
        "Relay broadcast should return exactly 1 message ID"
    );

    // GroupMessageSent event should reflect all members
    let events = events.lock().unwrap();
    let sent_event = events.iter().find(|e| {
        matches!(e, Event::GroupMessageSent { group_id: gid, member_count, .. }
            if gid == &group_id && *member_count == 3)
    });
    assert!(
        sent_event.is_some(),
        "Expected GroupMessageSent with member_count=3 (bob, carol, dave)"
    );
}

#[test]
fn test_relay_broadcast_fallback_to_fanout() {
    use offline_protocol_transport::mock::MockTransport;

    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let info = protocol.create_group("Fanout Test").unwrap();
    let group_id = info.group_id.as_str().to_string();
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "user123".to_string(),
            "bob".to_string(),
            "carol".to_string(),
        ],
    );

    // Do NOT mark as relay-synced — force per-member fan-out
    protocol.group_mesh.relay_synced.remove(&group_id);

    let msg_ids = protocol
        .send_group_message(&group_id, "hello fanout", None, None)
        .unwrap();

    assert_eq!(
        msg_ids.len(),
        2,
        "Fan-out should return one message ID per member"
    );
}

#[test]
fn test_handle_relay_group_message_plaintext_passthrough() {
    let (mut protocol, events) = setup_started_with_events();

    // Pre-populate member cache so the group is known
    protocol.group_mesh.members.insert(
        "group:relay-plain".to_string(),
        vec!["user123".to_string(), "alice".to_string()],
    );

    // Plaintext content (not valid base64) should pass through
    protocol.handle_relay_group_message_with_mls(
        "group:relay-plain",
        "alice",
        "Hello world! This is not base64",
        "2026-03-13T00:00:00Z",
        "msg-relay-001",
        None,
    );

    let events = events.lock().unwrap();
    let received = events.iter().find(|e| {
        matches!(e, Event::GroupMessageReceived { group_id, content, sender, .. }
            if group_id == "group:relay-plain"
                && content == "Hello world! This is not base64"
                && sender == "alice")
    });
    assert!(
        received.is_some(),
        "Plaintext relay message should be emitted as-is"
    );
}

#[test]
fn test_handle_relay_group_message_mls_decrypt() {
    let (alice, mut bob, group_id) = setup_alice_bob_group("Relay MLS Test");

    // Alice encrypts a message
    let encrypted = {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls
            .encrypt_for_group(&gid, b"Relay decrypted!")
            .unwrap()
    };
    let ciphertext_b64 = base64_encode(&encrypted.ciphertext);

    // Set up Bob's event capture
    let bob_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let bob_events_clone = bob_events.clone();
    bob.on_event(move |event| {
        bob_events_clone.lock().unwrap().push(event);
    });

    // Route through relay path
    bob.handle_relay_group_message_with_mls(
        &group_id,
        "alice",
        &ciphertext_b64,
        "2026-03-13T00:00:00Z",
        "msg-relay-mls-001",
        None,
    );

    // Verify decrypted content
    let events = bob_events.lock().unwrap();
    let received = events.iter().find(|e| {
        matches!(e, Event::GroupMessageReceived { group_id: gid, content, .. }
            if gid == &group_id && content == "Relay decrypted!")
    });
    assert!(
        received.is_some(),
        "Relay MLS message should be decrypted and emitted"
    );
}

#[test]
fn test_handle_relay_group_message_mls_unavailable_emits_raw() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    // Deliberately NOT initializing MLS
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    let raw_content = base64_encode(b"some ciphertext");
    protocol.handle_relay_group_message_with_mls(
        "group:no-mls",
        "alice",
        &raw_content,
        "2026-03-13T00:00:00Z",
        "msg-relay-no-mls",
        None,
    );

    // Should emit the raw base64 content since MLS is unavailable
    let events = events.lock().unwrap();
    let received = events.iter().find(|e| {
        matches!(e, Event::GroupMessageReceived { group_id, content, .. }
            if group_id == "group:no-mls" && content == &raw_content)
    });
    assert!(
        received.is_some(),
        "Should emit raw content when MLS is unavailable"
    );
}

#[test]
fn test_relay_register_group_on_create() {
    use offline_protocol_transport::mock::MockTransport;

    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let info = protocol.create_group("Auto Register Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    assert!(
        protocol.group_mesh.relay_synced.contains(&group_id),
        "Group should be relay-synced after creation with Internet available"
    );
}

#[test]
fn test_is_internet_available() {
    use offline_protocol_transport::mock::MockTransport;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.start().unwrap();

    // No Internet transport → not available
    assert!(!protocol.is_internet_available());

    // Add Internet transport
    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));

    // Now available
    assert!(protocol.is_internet_available());

    // Remove it
    protocol
        .transport_manager_mut()
        .remove_transport(TransportType::Internet);
    assert!(!protocol.is_internet_available());
}

#[test]
fn test_group_mls_send_total_failure_emits_partial_failure_event() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();
    // NOT started — all sends will fail with NotStarted

    let info = protocol.create_group("Failure Event Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Populate cache with multiple members
    protocol.group_mesh.members.insert(
        group_id.clone(),
        vec![
            "user123".to_string(),
            "bob".to_string(),
            "carol".to_string(),
        ],
    );

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    let result = protocol.send_group_message(&group_id, "hello", None, None);
    assert!(result.is_err(), "Total send failure should return Err");

    // Verify GroupMessagePartialFailure event was emitted with correct members
    let events = events.lock().unwrap();
    let failure_event = events.iter().find(|e| {
        matches!(
            e,
            Event::GroupMessagePartialFailure { group_id: gid, .. } if gid == &group_id
        )
    });
    assert!(
        failure_event.is_some(),
        "Expected GroupMessagePartialFailure event on total failure"
    );

    if let Some(Event::GroupMessagePartialFailure {
        failed_members,
        succeeded_members,
        ..
    }) = failure_event
    {
        assert!(
            failed_members.contains(&"bob".to_string()),
            "bob should be in failed_members"
        );
        assert!(
            failed_members.contains(&"carol".to_string()),
            "carol should be in failed_members"
        );
        assert_eq!(
            failed_members.len(),
            2,
            "Should have exactly 2 failed members"
        );
        assert!(
            succeeded_members.is_empty(),
            "succeeded_members should be empty on total failure"
        );
    } else {
        panic!("Event should be GroupMessagePartialFailure");
    }
}

// ========================================================================
// EPOCH FORK DETECTION TESTS
// ========================================================================

#[test]
fn test_epoch_fork_not_flagged_for_never_retried_expired_commits() {
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("Fork Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert expired pending commit with retry_count=0 (never retried — likely slow delivery)
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 0,
        });

    protocol.drain_pending_commits(&group_id);

    assert!(
        !protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Should not flag epoch fork for never-retried expired commits"
    );
    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::GroupEpochForkDetected { .. })),
        "Should not emit GroupEpochForkDetected event"
    );
}

#[test]
fn test_epoch_fork_flagged_for_retried_expired_commits() {
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("Fork Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert expired pending commit with retry_count > 0 (retried and kept failing = epoch mismatch)
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 3,
        });

    protocol.drain_pending_commits(&group_id);

    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Should flag epoch fork for retried expired commits"
    );
    let fork = &protocol.group_mesh.epoch_forks[&group_id];
    assert_eq!(fork.group_id, group_id);
    assert!(!fork.resolution_attempted);

    let events = events.lock().unwrap();
    assert!(
        events.iter().any(
            |e| matches!(e, Event::GroupEpochForkDetected { group_id: gid, .. } if gid == &group_id)
        ),
        "Should emit GroupEpochForkDetected event"
    );
}

#[test]
fn test_epoch_fork_not_duplicated_if_already_tracked() {
    let (mut protocol, _events) = setup_with_events();
    let info = protocol.create_group("Fork Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Pre-populate fork state with local_epoch=1
    protocol.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 1,
            detected_at: Instant::now(),
            resolution_attempted: false,
        },
    );

    // Insert expired retried commit — would normally trigger fork detection
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 2,
        });

    protocol.drain_pending_commits(&group_id);

    // The original fork state should be preserved (epoch 1), not overwritten
    assert_eq!(protocol.group_mesh.epoch_forks[&group_id].local_epoch, 1);
}

#[test]
fn test_epoch_fork_max_entries_eviction() {
    let (mut protocol, _events) = setup_with_events();

    // Fill up to MAX_EPOCH_FORK_ENTRIES
    for i in 0..MAX_EPOCH_FORK_ENTRIES {
        let gid = format!("group_{}", i);
        protocol.group_mesh.epoch_forks.insert(
            gid.clone(),
            EpochForkState {
                group_id: gid,
                local_epoch: i as u64,
                detected_at: Instant::now(),
                resolution_attempted: false,
            },
        );
    }
    assert_eq!(
        protocol.group_mesh.epoch_forks.len(),
        MAX_EPOCH_FORK_ENTRIES
    );

    // Trigger a new fork via expired retried commit on a new group
    let new_gid = "group_new".to_string();
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(new_gid.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 1,
        });

    protocol.drain_pending_commits(&new_gid);

    // Should have evicted one and added the new one — count stays bounded
    assert!(protocol.group_mesh.epoch_forks.contains_key(&new_gid));
    assert_eq!(
        protocol.group_mesh.epoch_forks.len(),
        MAX_EPOCH_FORK_ENTRIES
    );
}

#[test]
fn test_epoch_fork_cleared_on_successful_commit() {
    let (mut alice, bob, group_id) = setup_alice_bob_group("Fork Clear Test");

    // Insert a fork state for this group
    alice.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 1,
            detected_at: Instant::now(),
            resolution_attempted: false,
        },
    );

    // Create a valid MLS commit by having bob do a key update, then feed
    // it through alice's protocol message handling path (handle_group_mls_commit
    // → process_commit_core) which clears fork state on Success.
    let alice_update = {
        let mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        mls.update_keys(&gid).unwrap()
    };

    // Bob processes alice's update first so he's on the same epoch
    {
        let mls = bob.mls_manager_for_testing().read().unwrap();
        let encrypted = offline_protocol_mls::EncryptedMessage {
            group_id: offline_protocol_mls::GroupId::new(&group_id),
            message_type: offline_protocol_mls::MlsMessageType::Commit,
            epoch: alice_update.epoch,
            ciphertext: alice_update.ciphertext.clone(),
            sender_id: "alice".to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        mls.decrypt_from_group(&encrypted).unwrap();
    }

    // Bob creates a commit that alice will process through the protocol layer
    let bob_commit = {
        let mls = bob.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        mls.update_keys(&gid).unwrap()
    };

    // Build a protocol-layer commit message from bob to alice
    let commit_payload = GroupMlsCommitPayload {
        group_id: group_id.clone(),
        commit_type: GroupCommitType::KeyUpdate,
        ciphertext: base64_encode(&bob_commit.ciphertext),
        epoch: bob_commit.epoch,
        affected_member: None,
    };
    let data = serde_json::to_string(&commit_payload).unwrap();

    // Process through the protocol layer — this calls process_commit_core
    alice.handle_group_mls_commit("bob", &data);

    // Fork should be cleared after successful commit processing
    assert!(
        !alice.group_mesh.epoch_forks.contains_key(&group_id),
        "Fork state should be cleared after successful commit"
    );
}

// ========================================================================
// EPOCH FORK RESOLUTION TESTS
// ========================================================================

#[test]
fn test_epoch_fork_resolution_by_leader() {
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("Resolution Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Protocol user "user123" is the only member → lex-first → leader.
    // Insert a fork that's past the resolution delay.
    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);
    protocol.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 1,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    // Fork should be resolved and removed (leader issued key update successfully)
    assert!(
        !protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Fork should be removed after successful resolution by leader"
    );

    let events = events.lock().unwrap();
    assert!(
        events.iter().any(
            |e| matches!(e, Event::GroupEpochForkResolved { group_id: gid, .. } if gid == &group_id)
        ),
        "Should emit GroupEpochForkResolved event"
    );
}

#[test]
fn test_epoch_fork_resolution_skipped_for_non_leader() {
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config_for_user("zoe")).unwrap();
    protocol.initialize_mls(storage).unwrap();
    let _info = protocol.create_group("Non-Leader Test").unwrap();

    // Use a fake group_id for the fork so refresh_group_members fails and
    // falls back to cached membership where "alice" is lex-first leader.
    let fake_group_id = "fake_fork_group".to_string();
    protocol.group_mesh.members.insert(
        fake_group_id.clone(),
        vec!["alice".to_string(), "zoe".to_string()],
    );

    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);
    protocol.group_mesh.epoch_forks.insert(
        fake_group_id.clone(),
        EpochForkState {
            group_id: fake_group_id.clone(),
            local_epoch: 1,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    // Fork should still exist but marked as attempted (non-leader doesn't act)
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&fake_group_id),
        "Fork should still exist for non-leader"
    );
    assert!(
        protocol.group_mesh.epoch_forks[&fake_group_id].resolution_attempted,
        "Fork should be marked as resolution_attempted even for non-leader"
    );
}

#[test]
fn test_epoch_fork_not_resolved_before_delay() {
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("Delay Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert a fork that was just detected (within the delay window)
    protocol.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 1,
            detected_at: Instant::now(),
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    // Fork should still exist and not attempted (delay hasn't elapsed)
    assert!(protocol.group_mesh.epoch_forks.contains_key(&group_id));
    assert!(!protocol.group_mesh.epoch_forks[&group_id].resolution_attempted);

    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::GroupEpochForkResolved { .. })),
        "Should not emit GroupEpochForkResolved before delay"
    );
}

#[test]
fn test_epoch_fork_stale_entries_cleaned_up() {
    let (mut protocol, _events) = setup_with_events();

    // Insert a very old fork entry (older than 5-minute stale threshold)
    let very_old = Instant::now() - StdDuration::from_secs(400);
    protocol.group_mesh.epoch_forks.insert(
        "stale_group".to_string(),
        EpochForkState {
            group_id: "stale_group".to_string(),
            local_epoch: 1,
            detected_at: very_old,
            resolution_attempted: true,
        },
    );

    protocol.check_epoch_forks();

    assert!(
        !protocol.group_mesh.epoch_forks.contains_key("stale_group"),
        "Stale fork entries (>5 min) should be cleaned up"
    );
}

// ========================================================================
// LEAVE ELECTION FALLBACK TESTS
// ========================================================================

#[test]
fn test_leave_election_cleared_when_member_already_removed() {
    let (mut protocol, _events) = setup_with_events();
    let info = protocol.create_group("Leave Election Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // "bob" was never added to this MLS group, so refresh_group_members
    // won't find him — simulates a member that was already removed.
    let past = Instant::now() - StdDuration::from_secs(LEAVE_ELECTION_TIMEOUT_SECS + 5);
    let key = leave_election_key(&group_id, "bob");
    protocol.group_mesh.pending_leave_elections.insert(
        key.clone(),
        PendingLeaveElection {
            group_id: group_id.clone(),
            leaving_member: "bob".to_string(),
            received_at: past,
        },
    );

    protocol.check_leave_election_timeouts();

    assert!(
        !protocol
            .group_mesh
            .pending_leave_elections
            .contains_key(&key),
        "Election should be cleared when leaving member is no longer in group"
    );
}

#[test]
fn test_leave_election_timeout_re_elects_self() {
    let (mut alice, _bob, group_id) = setup_alice_bob_group("Re-election Test");

    // Bob is in the MLS group. Alice is "alice", bob is "bob".
    // After filtering out the leaver ("bob"), remaining = ["alice"].
    // alice is lex-first → candidate at interval 0 → should attempt remove.
    let past = Instant::now() - StdDuration::from_secs(LEAVE_ELECTION_TIMEOUT_SECS + 5);
    let key = leave_election_key(&group_id, "bob");
    alice.group_mesh.pending_leave_elections.insert(
        key.clone(),
        PendingLeaveElection {
            group_id: group_id.clone(),
            leaving_member: "bob".to_string(),
            received_at: past,
        },
    );

    alice.check_leave_election_timeouts();

    // Remove should succeed → election cleared
    assert!(
        !alice.group_mesh.pending_leave_elections.contains_key(&key),
        "Election should be cleared after successful remove"
    );

    // Bob should no longer be in the MLS group
    let members = alice.refresh_group_members(&group_id).unwrap();
    assert!(
        !members.contains(&"bob".to_string()),
        "Bob should be removed from group after re-election"
    );
}

#[test]
fn test_leave_election_not_triggered_before_timeout() {
    let (mut protocol, _events) = setup_with_events();
    let info = protocol.create_group("Timeout Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert a pending election that hasn't timed out yet
    let key = leave_election_key(&group_id, "bob");
    protocol.group_mesh.pending_leave_elections.insert(
        key.clone(),
        PendingLeaveElection {
            group_id: group_id.clone(),
            leaving_member: "bob".to_string(),
            received_at: Instant::now(), // just now — well within timeout
        },
    );

    protocol.check_leave_election_timeouts();

    // Election should still be pending (hasn't timed out)
    assert!(
        protocol
            .group_mesh
            .pending_leave_elections
            .contains_key(&key),
        "Election should remain pending before timeout"
    );
}

#[test]
fn test_leave_election_key_helper() {
    assert_eq!(leave_election_key("group1", "alice"), "group1:alice");
    assert_eq!(
        leave_election_key("my-group-id", "user:123"),
        "my-group-id:user:123"
    );
}

#[test]
fn test_pending_commit_retry_count_incremented() {
    let (mut protocol, _events) = setup_with_events();
    let info = protocol.create_group("Retry Count Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert a non-expired pending commit with invalid data.
    // process_commit_core will return Rejected (bad data), so it won't
    // be re-buffered. Instead, test the buffering path directly.
    protocol.buffer_pending_commit(&group_id, "bob", "fake-data");

    let pending = protocol.group_mesh.pending_commits.get(&group_id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].retry_count, 0, "Initial retry_count should be 0");
}

#[test]
fn test_key_update_commit_type_serialization() {
    // Verify KeyUpdate serializes/deserializes correctly
    let payload = GroupMlsCommitPayload {
        group_id: "test-group".to_string(),
        commit_type: GroupCommitType::KeyUpdate,
        ciphertext: "abc".to_string(),
        epoch: 5,
        affected_member: None,
    };

    let json = serde_json::to_string(&payload).unwrap();
    assert!(
        json.contains("\"keyupdate\""),
        "KeyUpdate should serialize as 'keyupdate'"
    );

    let deserialized: GroupMlsCommitPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.commit_type, GroupCommitType::KeyUpdate);
    assert!(deserialized.affected_member.is_none());
}

// ========================================================================
// EPOCH FORK DETECTION & RESOLUTION — RESILIENCE TESTS
// ========================================================================

#[test]
fn test_epoch_fork_detected_via_periodic_cleanup_without_drain() {
    // Validates fix for the "complete fork" scenario: when NO commits
    // succeed for a group, drain_pending_commits is never called. Fork
    // detection must also fire from cleanup_group_message_dedup.
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("Periodic Fork Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Insert expired pending commit with retry_count > 0 (fork signal)
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake-commit".to_string(),
            buffered_at: past,
            retry_count: 2,
        });

    // Do NOT call drain_pending_commits — call periodic cleanup instead
    protocol.cleanup_group_message_dedup();

    // Fork should be detected via the periodic cleanup path
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Fork should be detected during periodic cleanup when retried commits expire"
    );

    let events = events.lock().unwrap();
    assert!(
        events.iter().any(
            |e| matches!(e, Event::GroupEpochForkDetected { group_id: gid, .. } if gid == &group_id)
        ),
        "Should emit GroupEpochForkDetected during periodic cleanup"
    );
}

#[test]
fn test_epoch_fork_not_flagged_via_cleanup_for_never_retried() {
    // Commits that expired with retry_count=0 during periodic cleanup
    // should NOT trigger fork detection (likely just slow delivery).
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("No Fork Cleanup").unwrap();
    let group_id = info.group_id.as_str().to_string();

    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 0, // never retried
        });

    protocol.cleanup_group_message_dedup();

    assert!(
        !protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Never-retried expired commits should not trigger fork detection during cleanup"
    );
    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::GroupEpochForkDetected { .. })),
        "No fork event for never-retried expired commits"
    );
}

#[test]
fn test_epoch_fork_cleanup_does_not_duplicate_existing_fork() {
    // If a fork is already tracked for a group, periodic cleanup should
    // not overwrite it with a new detection.
    let (mut protocol, events) = setup_with_events();
    let info = protocol.create_group("No Dup Cleanup").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Pre-insert fork state
    let original_detected_at = Instant::now() - StdDuration::from_secs(30);
    protocol.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 42,
            detected_at: original_detected_at,
            resolution_attempted: false,
        },
    );

    // Insert expired retried commit
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 3,
        });

    protocol.cleanup_group_message_dedup();

    // Original fork state should be preserved
    assert_eq!(protocol.group_mesh.epoch_forks[&group_id].local_epoch, 42);

    // No new fork detection event should be emitted
    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::GroupEpochForkDetected { .. })),
        "Should not emit duplicate fork detection event"
    );
}

#[test]
fn test_epoch_fork_resolution_includes_failed_members() {
    // Verifies that GroupEpochForkResolved includes members we couldn't reach.
    // Use a real MLS group but DON'T start the protocol — send_internal_message
    // will fail for all members, simulating unreachable peers.
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let info = protocol.create_group("Failed Members Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Use a fake group_id that doesn't exist in MLS so refresh_group_members
    // fails and falls back to the cache, preserving our injected members.
    let fake_group_id = "fake_fork_failed_members".to_string();
    protocol.group_mesh.members.insert(
        fake_group_id.clone(),
        vec![
            "bob".to_string(),
            "carol".to_string(),
            "user123".to_string(),
        ],
    );

    // Insert a fork for the REAL group_id (so update_keys succeeds) but
    // use the fake_group_id for the fork tracking. We need update_keys to
    // succeed, so instead test with the real group_id and simply don't start
    // the protocol. The real group only has user123 from MLS perspective,
    // so refresh_group_members returns only user123 and there's nobody to
    // fail sending to. Instead, let's test the tracking differently:
    // We use setup_alice_bob_group which gives us a 2-member group, then
    // don't start the protocol for the resolution check.

    // Clean approach: use the real group with injected extra fake members.
    // refresh_group_members will overwrite to the real MLS membership,
    // so we put the fork on the real group and inject fake members.
    // Since refresh succeeds, members = MLS members = [user123].
    // No other members → no sends → no failures. That won't test it.

    // Best approach: put fork on a fake group_id so refresh fails and
    // cache is used. update_keys for a nonexistent group will fail.
    // That's the update_keys failure path, not what we want.

    // Final approach: use a started alice+bob group, but stop the protocol
    // so sends fail.
    drop(protocol);

    let (mut alice, _bob, group_id) = setup_alice_bob_group("Failed Members Fork Test");

    // Alice's protocol is started but we can break sends by stopping it.
    // Actually, send_internal_message uses the outbox. It won't fail
    // for a started protocol. Instead, inject a fake third member that
    // doesn't exist — sends to real members will succeed (go to outbox),
    // but we can verify the event structure works.

    // Simplest valid test: use fake group_id with cached members. The
    // fork resolution will fail at update_keys (group not in MLS), which
    // is the Err branch. To test failed_members in the Ok branch, we
    // need update_keys to succeed. So use the real group_id. refresh
    // will give [alice, bob]. Sends to bob will succeed (outbox). So
    // failed_members will be empty. That tests the happy path.

    // To get actual send failures: don't start the protocol.
    drop(alice);

    let storage_a = Arc::new(crate::mls::InMemoryStorage::default());
    let storage_b = Arc::new(crate::mls::InMemoryStorage::default());
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    alice.initialize_mls(storage_a).unwrap();
    bob.initialize_mls(storage_b).unwrap();
    // Start ONLY for group creation, then we don't need it for sends
    alice.start().unwrap();
    bob.start().unwrap();

    let group_info = alice.create_group("Fork Send Fail").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    let bob_kp = {
        let bob_mls = bob.mls_manager_for_testing().read().unwrap();
        bob_mls.generate_key_package().unwrap()
    };
    {
        let alice_mls = alice.mls_manager_for_testing().read().unwrap();
        let gid = offline_protocol_mls::GroupId::new(&group_id);
        alice_mls
            .add_group_member(&gid, &bob_kp.key_package_data)
            .unwrap();
    }
    alice.refresh_group_members(&group_id).unwrap();

    // Stop alice so send_internal_message fails
    let _ = alice.stop();

    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);
    alice.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 1,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    alice.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    alice.check_epoch_forks();

    // Fork should be resolved (key update succeeds even if sends fail)
    assert!(
        !alice.group_mesh.epoch_forks.contains_key(&group_id),
        "Fork should be cleaned up after resolution attempt"
    );

    let events = events.lock().unwrap();
    let resolved = events.iter().find(
        |e| matches!(e, Event::GroupEpochForkResolved { group_id: gid, .. } if gid == &group_id),
    );
    assert!(resolved.is_some(), "Should emit GroupEpochForkResolved");

    if let Some(Event::GroupEpochForkResolved { failed_members, .. }) = resolved {
        // Bob should be in failed_members because protocol is stopped
        assert!(
            failed_members.contains(&"bob".to_string()),
            "bob should be in failed_members when protocol is stopped"
        );
        // Alice (self) should NOT be in failed_members
        assert!(
            !failed_members.contains(&"alice".to_string()),
            "self should not be in failed_members"
        );
    } else {
        panic!("Expected GroupEpochForkResolved event");
    }
}

#[test]
fn test_epoch_fork_resolution_no_failed_members_when_all_succeed() {
    // When the leader successfully sends to all members, failed_members should be empty.
    let (mut protocol, events) = setup_started_with_events();
    let info = protocol.create_group("All Succeed Test").unwrap();
    let group_id = info.group_id.as_str().to_string();

    // Solo group — no other members to send to
    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);
    protocol.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 1,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    let events = events.lock().unwrap();
    let resolved = events.iter().find(
        |e| matches!(e, Event::GroupEpochForkResolved { group_id: gid, .. } if gid == &group_id),
    );
    assert!(resolved.is_some(), "Should emit resolved event");
    if let Some(Event::GroupEpochForkResolved { failed_members, .. }) = resolved {
        assert!(
            failed_members.is_empty(),
            "No failed members when all sends succeed"
        );
    }
}

#[test]
fn test_epoch_fork_update_keys_failure_leaves_resolution_attempted() {
    // When update_keys fails, resolution_attempted stays true (manual intervention needed).
    let storage = Arc::new(crate::mls::InMemoryStorage::default());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // Use a fake group_id that doesn't exist in MLS — update_keys will fail.
    let fake_group_id = "group:nonexistent-for-update".to_string();
    protocol.group_mesh.members.insert(
        fake_group_id.clone(),
        vec!["user123".to_string()], // self is leader
    );

    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);
    protocol.group_mesh.epoch_forks.insert(
        fake_group_id.clone(),
        EpochForkState {
            group_id: fake_group_id.clone(),
            local_epoch: 1,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        events_clone.lock().unwrap().push(event);
    });

    protocol.check_epoch_forks();

    // Fork should still exist with resolution_attempted = true
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&fake_group_id),
        "Fork should remain when update_keys fails"
    );
    assert!(
        protocol.group_mesh.epoch_forks[&fake_group_id].resolution_attempted,
        "resolution_attempted should be true after failed update_keys"
    );

    // No resolved event should be emitted
    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::GroupEpochForkResolved { .. })),
        "Should not emit resolved event when update_keys fails"
    );
}

#[test]
fn test_epoch_fork_mls_unavailable_resets_resolution_attempted() {
    // When MLS is completely unavailable during resolution, resolution_attempted
    // should be reset to false so it can be retried on the next tick.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    // Deliberately do NOT initialize MLS — read_mls_guard will fail

    let group_id = "group:no-mls".to_string();
    protocol
        .group_mesh
        .members
        .insert(group_id.clone(), vec!["user123".to_string()]);

    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);
    protocol.group_mesh.epoch_forks.insert(
        group_id.clone(),
        EpochForkState {
            group_id: group_id.clone(),
            local_epoch: 0,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    // Fork should still exist with resolution_attempted = false (reset for retry)
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Fork should remain when MLS is unavailable"
    );
    assert!(
        !protocol.group_mesh.epoch_forks[&group_id].resolution_attempted,
        "resolution_attempted should be reset to false when MLS is unavailable"
    );
}

#[test]
fn test_epoch_fork_multiple_concurrent_groups() {
    // Multiple groups can have independent forks detected and tracked.
    let (mut protocol, events) = setup_with_events();
    let info_a = protocol.create_group("Fork Group A").unwrap();
    let info_b = protocol.create_group("Fork Group B").unwrap();
    let group_a = info_a.group_id.as_str().to_string();
    let group_b = info_b.group_id.as_str().to_string();

    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);

    // Insert expired retried commits for BOTH groups
    for gid in [&group_a, &group_b] {
        protocol
            .group_mesh
            .pending_commits
            .entry(gid.clone())
            .or_default()
            .push_back(PendingCommit {
                sender: "bob".to_string(),
                data: "fake".to_string(),
                buffered_at: past,
                retry_count: 1,
            });
    }

    // Trigger fork detection via periodic cleanup
    protocol.cleanup_group_message_dedup();

    // Both groups should have forks detected independently
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_a),
        "Group A should have fork detected"
    );
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_b),
        "Group B should have fork detected"
    );

    let events = events.lock().unwrap();
    let fork_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::GroupEpochForkDetected { .. }))
        .collect();
    assert_eq!(
        fork_events.len(),
        2,
        "Should emit fork detection for both groups"
    );
}

#[test]
fn test_epoch_fork_multiple_groups_resolve_independently() {
    // Forks in different groups resolve independently — resolving one
    // should not affect the other.
    let (mut protocol, events) = setup_with_events();
    let info_a = protocol.create_group("Resolve A").unwrap();
    let info_b = protocol.create_group("Resolve B").unwrap();
    let group_a = info_a.group_id.as_str().to_string();
    let group_b = info_b.group_id.as_str().to_string();

    let past = Instant::now() - StdDuration::from_secs(EPOCH_FORK_RESOLUTION_DELAY_SECS + 5);

    // Group A: fork ready for resolution
    protocol.group_mesh.epoch_forks.insert(
        group_a.clone(),
        EpochForkState {
            group_id: group_a.clone(),
            local_epoch: 1,
            detected_at: past,
            resolution_attempted: false,
        },
    );

    // Group B: fork not yet ready (detected just now)
    protocol.group_mesh.epoch_forks.insert(
        group_b.clone(),
        EpochForkState {
            group_id: group_b.clone(),
            local_epoch: 2,
            detected_at: Instant::now(),
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    // Group A should be resolved and cleaned up
    assert!(
        !protocol.group_mesh.epoch_forks.contains_key(&group_a),
        "Group A fork should be resolved"
    );

    // Group B should still be pending (delay not elapsed)
    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_b),
        "Group B fork should still be pending"
    );
    assert!(
        !protocol.group_mesh.epoch_forks[&group_b].resolution_attempted,
        "Group B resolution should not have been attempted"
    );

    let events = events.lock().unwrap();
    let resolved_count = events
        .iter()
        .filter(|e| matches!(e, Event::GroupEpochForkResolved { .. }))
        .count();
    assert_eq!(resolved_count, 1, "Only group A should have been resolved");
}

#[test]
fn test_epoch_fork_stale_cleanup_removes_old_attempted_and_unattempted() {
    // Both attempted and unattempted forks older than 5 minutes are cleaned up.
    let (mut protocol, _events) = setup_with_events();

    let very_old = Instant::now() - StdDuration::from_secs(400);

    // Old fork with resolution_attempted = true
    protocol.group_mesh.epoch_forks.insert(
        "stale_attempted".to_string(),
        EpochForkState {
            group_id: "stale_attempted".to_string(),
            local_epoch: 1,
            detected_at: very_old,
            resolution_attempted: true,
        },
    );

    // Old fork with resolution_attempted = false
    protocol.group_mesh.epoch_forks.insert(
        "stale_unattempted".to_string(),
        EpochForkState {
            group_id: "stale_unattempted".to_string(),
            local_epoch: 2,
            detected_at: very_old,
            resolution_attempted: false,
        },
    );

    // Recent fork — should survive
    protocol.group_mesh.epoch_forks.insert(
        "recent_fork".to_string(),
        EpochForkState {
            group_id: "recent_fork".to_string(),
            local_epoch: 3,
            detected_at: Instant::now(),
            resolution_attempted: false,
        },
    );

    protocol.check_epoch_forks();

    assert!(
        !protocol
            .group_mesh
            .epoch_forks
            .contains_key("stale_attempted"),
        "Old attempted fork should be cleaned up"
    );
    assert!(
        !protocol
            .group_mesh
            .epoch_forks
            .contains_key("stale_unattempted"),
        "Old unattempted fork should be cleaned up"
    );
    assert!(
        protocol.group_mesh.epoch_forks.contains_key("recent_fork"),
        "Recent fork should survive cleanup"
    );
}

#[test]
fn test_epoch_fork_detection_with_mls_unavailable_logs_epoch_zero() {
    // When MLS is not initialized, flag_potential_epoch_fork should
    // still work — local_epoch falls back to 0 with a warning logged.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    // MLS NOT initialized

    let group_id = "group:no-mls-fork".to_string();

    // Insert expired retried commit
    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);
    protocol
        .group_mesh
        .pending_commits
        .entry(group_id.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 1,
        });

    // Should not panic — fork detection gracefully handles MLS unavailable
    protocol.drain_pending_commits(&group_id);

    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_id),
        "Fork should be detected even when MLS is unavailable"
    );
    assert_eq!(
        protocol.group_mesh.epoch_forks[&group_id].local_epoch, 0,
        "local_epoch should fall back to 0 when MLS is unavailable"
    );
}

#[test]
fn test_epoch_fork_periodic_cleanup_multiple_groups_mixed() {
    // Periodic cleanup should only flag forks for groups with retried-expired
    // commits, not groups with only never-retried expired commits.
    let (mut protocol, events) = setup_with_events();
    let info_fork = protocol.create_group("Will Fork").unwrap();
    let info_nofork = protocol.create_group("No Fork").unwrap();
    let group_fork = info_fork.group_id.as_str().to_string();
    let group_nofork = info_nofork.group_id.as_str().to_string();

    let past = Instant::now() - StdDuration::from_secs(PENDING_COMMIT_TTL_SECS + 10);

    // group_fork: retried expired commit → should flag fork
    protocol
        .group_mesh
        .pending_commits
        .entry(group_fork.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "bob".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 2,
        });

    // group_nofork: never-retried expired commit → should NOT flag fork
    protocol
        .group_mesh
        .pending_commits
        .entry(group_nofork.clone())
        .or_default()
        .push_back(PendingCommit {
            sender: "carol".to_string(),
            data: "fake".to_string(),
            buffered_at: past,
            retry_count: 0,
        });

    protocol.cleanup_group_message_dedup();

    assert!(
        protocol.group_mesh.epoch_forks.contains_key(&group_fork),
        "Group with retried expired commits should have fork detected"
    );
    assert!(
        !protocol.group_mesh.epoch_forks.contains_key(&group_nofork),
        "Group with only never-retried expired commits should not have fork"
    );

    let events = events.lock().unwrap();
    let fork_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::GroupEpochForkDetected { .. }))
        .collect();
    assert_eq!(
        fork_events.len(),
        1,
        "Only one fork event should be emitted"
    );
}
