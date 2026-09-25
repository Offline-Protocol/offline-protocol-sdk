//! The per-send application id, end to end.
//!
//! One instance can serve several local applications behind one identity.
//! What makes that work is two things this module pins: a rich send can
//! stamp an app id other than the configured one on its own frames, and the
//! receiver reports the stamped id on `MessageReceived` and `FileReceived`.
//!
//! The failure each test names is a send that silently reaches the wire, or
//! the receiving application, under the configured id instead. That happens
//! wherever a frame is rebuilt from a record that dropped the value: the
//! pending queue's flush, the media pump's later batches, a restart.

use super::*;

const OTHER_APP: &str = "other-app";

fn other_app() -> Option<String> {
    Some(OTHER_APP.to_string())
}

/// A started, encrypting protocol with no session to `bob`, so a valid send
/// parks in the pending queue.
fn parking_protocol() -> (OfflineProtocol, MockTransport) {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();
    (protocol, handle)
}

/// Builds and confirms a session from `protocol` to `bob`.
fn confirm_session_to_bob(protocol: &mut OfflineProtocol) {
    let bob_manager =
        crate::test_identity::manager_for("bob", Arc::new(crate::mls::InMemoryStorage::new()));
    let bob_key_package = bob_manager.generate_key_package().unwrap();
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package(&id("bob"), &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session(&id("bob")).unwrap();
    }
    protocol
        .confirm_session_state(&id("bob"), "test_setup")
        .unwrap();
}

fn capture_events(protocol: &mut OfflineProtocol) -> Arc<Mutex<Vec<Event>>> {
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let handle = Arc::clone(&events);
    protocol.on_event(move |event| handle.lock().unwrap().push(event));
    events
}

#[test]
fn a_rich_send_stamps_its_own_app_id_and_a_plain_one_the_configured_id() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let named = alice
        .send_message_with(
            &id("bob"),
            "for the other app",
            SendMessageOptions {
                app_id: other_app(),
                ..Default::default()
            },
        )
        .unwrap();
    let plain = alice
        .send_message_with(&id("bob"), "for this app", SendMessageOptions::default())
        .unwrap();

    let sent = alice_handle.sent_messages();
    let wire = |message_id: &MessageId| {
        sent.iter()
            .find(|m| &m.id == message_id)
            .unwrap_or_else(|| panic!("{message_id} must reach the wire"))
            .app_id
            .as_str()
            .to_string()
    };
    assert_eq!(wire(&named), OTHER_APP);
    assert_eq!(wire(&plain), "test-app");

    // The outbox keeps the stamped frame, so every retry resends it as is.
    assert_eq!(alice.outbox[&named].message.app_id.as_str(), OTHER_APP);
}

#[test]
fn an_invalid_app_id_fails_before_anything_is_queued_or_ticked() {
    let (mut protocol, handle) = parking_protocol();
    let clock_before = protocol.lamport_clock.value();

    for bad in ["bad/app", "", ".."]
        .into_iter()
        .map(str::to_string)
        .chain(std::iter::once(
            "a".repeat(offline_protocol_core::MAX_ID_LEN + 1),
        ))
    {
        let result = protocol.send_message_with(
            &id("bob"),
            "hello",
            SendMessageOptions {
                app_id: Some(bad.clone()),
                ..Default::default()
            },
        );
        assert!(
            matches!(result, Err(Error::InvalidArgument(ref m)) if m.contains("app_id")),
            "{bad:?} must fail as InvalidArgument, got {result:?}"
        );
    }
    assert_eq!(protocol.lamport_clock.value(), clock_before);
    assert!(!protocol.pending_encrypted_messages.contains_key(&id("bob")));
    assert!(handle.sent_messages().is_empty());

    // The same send with a valid id parks, so the checks above were made on
    // a path that would otherwise have left a durable entry behind.
    protocol
        .send_message_with(
            &id("bob"),
            "hello",
            SendMessageOptions {
                app_id: other_app(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(protocol.pending_encrypted_messages[&id("bob")].len(), 1);
}

#[test]
fn a_parked_rich_send_flushes_under_its_own_app_id() {
    let (mut protocol, handle) = parking_protocol();
    let queued = protocol
        .send_message_with(
            &id("bob"),
            "parked",
            SendMessageOptions {
                app_id: other_app(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        handle.sent_messages().is_empty(),
        "no session yet: it parks"
    );
    assert_eq!(
        protocol.pending_encrypted_messages[&id("bob")][0]
            .app_id
            .as_deref(),
        Some(OTHER_APP)
    );

    confirm_session_to_bob(&mut protocol);
    protocol.flush_pending_messages(&id("bob")).unwrap();

    let sent = handle.sent_messages();
    let flushed = sent.iter().find(|m| m.id == queued).expect("flushed");
    assert_eq!(
        flushed.app_id.as_str(),
        OTHER_APP,
        "the flush rebuilds the frame and must restore the per-send id"
    );
}

#[test]
fn a_parked_app_id_survives_a_restart() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage.clone()).unwrap();
    confirm_session_to_bob(&mut protocol);

    let message_id = MessageId::new();
    protocol.queue_pending_message_at(
        &id("bob"),
        "queued-before-crash",
        MessagePriority::Medium,
        message_id.clone(),
        None,
        None,
        ContentType::default(),
        None,
        None,
        other_app(),
        Utc::now(),
    );

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls_for_test(storage).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let handle = transport.clone();
    restarted
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    restarted.start().unwrap();

    let sent = handle.sent_messages();
    let flushed = sent
        .iter()
        .find(|m| m.id == message_id)
        .expect("the restored entry flushes on start");
    assert_eq!(flushed.app_id.as_str(), OTHER_APP);
}

#[test]
fn a_pending_record_written_before_the_field_existed_restores_as_the_configured_id() {
    let pending = PendingMessage {
        content: "legacy".to_string(),
        priority: MessagePriority::Medium,
        message_id: MessageId::new(),
        reply_to_msg: None,
        forwarded_from: None,
        content_type: ContentType::Text,
        media_metadata: None,
        rich: None,
        app_id: None,
        queued_at: Utc::now(),
        serialized_bytes: 0,
    };
    let json = serde_json::to_value(&pending).unwrap();
    assert!(
        json.get("app_id").is_none(),
        "an entry on the configured id writes the same record it always did"
    );
    let restored: PendingMessage = serde_json::from_value(json).unwrap();
    assert_eq!(restored.app_id, None);
}

#[test]
fn every_media_chunk_carries_the_per_send_app_id_and_the_receiver_reports_it() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    let bob_events = capture_events(&mut bob);

    // 10 KB over 4 KB BLE chunks = 3 chunks at window 2: the third chunk is
    // built by the pump from the transfer record, not by the send call.
    let file_data: Vec<u8> = (0..10_240u32).map(|i| (i % 251) as u8).collect();
    alice
        .send_media_with(
            &id("bob"),
            file_data.clone(),
            "photo.jpg",
            ContentType::Image,
            MediaSendOptions {
                app_id: other_app(),
                ..Default::default()
            },
        )
        .unwrap();

    let received = |events: &Arc<Mutex<Vec<Event>>>| {
        events.lock().unwrap().iter().find_map(|e| match e {
            Event::FileReceived { app_id, .. } => Some(app_id.clone()),
            _ => None,
        })
    };
    let mut chunks = Vec::new();
    let mut rounds = 0;
    while received(&bob_events).is_none() {
        rounds += 1;
        assert!(rounds < 32, "media transfer did not complete");
        let outbound = alice_handle.sent_messages();
        alice_handle.clear_sent_messages();
        for msg in outbound {
            if msg.content_type == ContentType::FileChunk {
                chunks.push(msg.app_id.as_str().to_string());
            }
            bob_handle.queue_message(msg);
        }
        while bob.receive_message().is_some() {}
        let acks = bob_handle.sent_messages();
        bob_handle.clear_sent_messages();
        for msg in acks {
            alice_handle.queue_message(msg);
        }
        while alice.receive_message().is_some() {}
        alice.pump_media_transfers();
    }

    assert!(chunks.len() >= 3, "expected a pumped batch, got {chunks:?}");
    assert!(
        chunks.iter().all(|a| a == OTHER_APP),
        "every chunk, the pumped ones included, carries the per-send id: {chunks:?}"
    );
    assert_eq!(received(&bob_events), Some(other_app()));
}

#[test]
fn an_interrupted_transfer_names_its_app_id_after_a_restart() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let (named, plain) = {
        let (mut alice, _handle, _events) =
            plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));
        let named = alice
            .send_media_with(
                "bob",
                vec![7u8; 256],
                "named.bin",
                ContentType::File,
                MediaSendOptions {
                    app_id: other_app(),
                    ..Default::default()
                },
            )
            .unwrap();
        let plain = alice
            .send_media_with(
                "bob",
                vec![9u8; 256],
                "plain.bin",
                ContentType::File,
                MediaSendOptions::default(),
            )
            .unwrap();
        (named, plain)
    };

    let (_alice, _handle, events) =
        plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));
    let resend_app_id = |file: &str| {
        events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                Event::MediaResendRequired {
                    file_id, app_id, ..
                } if file_id == file => Some(app_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{file} must be surfaced for a resend"))
    };
    assert_eq!(resend_app_id(&named), other_app());
    assert_eq!(resend_app_id(&plain), None);
}

#[test]
fn message_received_reports_the_stamped_app_id_and_the_ack_keeps_the_configured_one() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    let bob_events = capture_events(&mut bob);

    alice
        .send_message_with(
            &id("bob"),
            "hello other app",
            SendMessageOptions {
                app_id: other_app(),
                ..Default::default()
            },
        )
        .unwrap();
    for msg in alice_handle.sent_messages() {
        bob_handle.queue_message(msg);
    }
    while bob.receive_message().is_some() {}

    let events = bob_events.lock().unwrap();
    let event = events
        .iter()
        .find(
            |e| matches!(e, Event::MessageReceived { content, .. } if content == "hello other app"),
        )
        .expect("delivered");
    let Event::MessageReceived { app_id, .. } = event else {
        unreachable!()
    };
    assert_eq!(app_id, OTHER_APP);
    let json = serde_json::to_value(event).unwrap();
    assert_eq!(
        json["app_id"], OTHER_APP,
        "the field crosses to the bindings"
    );

    // The acknowledgement is the engine's, not an application's.
    let acks: Vec<_> = bob_handle
        .sent_messages()
        .into_iter()
        .filter(|m| m.metadata.contains_key(crate::constants::ACK_FOR_KEY))
        .collect();
    assert!(!acks.is_empty(), "the delivery is acknowledged");
    assert!(acks.iter().all(|m| m.app_id.as_str() == "test-app"));
}

#[test]
fn a_delayed_decrypt_reports_the_app_id_of_the_frame_it_held() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let events = capture_events(&mut bob);

    let alice_manager =
        crate::test_identity::manager_for("alice", Arc::new(crate::mls::InMemoryStorage::new()));
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    alice_manager
        .import_key_package(&id("bob"), &bob_key_package.key_package_data)
        .unwrap();
    let welcome = alice_manager.create_session(&id("bob")).unwrap();
    let encrypted = alice_manager
        .encrypt_for_user(&id("bob"), b"held until the welcome")
        .unwrap();

    let mut encrypted_wire = signed_frame(
        &id("alice"),
        &id("bob"),
        format!(
            "{}{}",
            internal_prefixes::ENCRYPTED,
            serde_json::to_string(&encrypted).unwrap()
        ),
    );
    encrypted_wire.app_id = AppId::new(OTHER_APP).unwrap();
    let welcome_wire = signed_frame(
        &id("alice"),
        &id("bob"),
        format!(
            "{}{}",
            internal_prefixes::WELCOME,
            serde_json::to_string(&welcome).unwrap()
        ),
    );

    assert!(matches!(
        bob.process_internal_message(&encrypted_wire),
        Some(InternalMessageResult::Deferred)
    ));
    bob.process_internal_message(&welcome_wire);

    let app_id = events
        .lock()
        .unwrap()
        .iter()
        .find_map(|e| match e {
            Event::MessageReceived {
                content,
                transport,
                app_id,
                ..
            } if content == "held until the welcome" => {
                assert_eq!(transport, "delayed");
                Some(app_id.clone())
            }
            _ => None,
        })
        .expect("the held message surfaces once the welcome lands");
    assert_eq!(app_id, OTHER_APP);
}
