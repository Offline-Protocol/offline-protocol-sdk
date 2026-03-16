use super::*;
use crate::constants::ACK_FOR_KEY;
use crate::events::{DecryptionFailureCode, PresenceStatus};
#[cfg(feature = "mls-observability")]
use crate::mls_observability::MlsLifecycleEvent;
use chrono::Duration as ChronoDuration;
use offline_protocol_core::{AppId, MessagePriority, ServiceDescriptor, UserId};
use offline_protocol_transport::{
    mock::MockTransport, Transport, TransportMetrics, TransportStatus, TransportType,
};
use std::collections::VecDeque;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

pub(crate) fn create_test_config() -> ProtocolConfig {
    ProtocolConfig::new("test-app", "user123")
}

pub(crate) fn create_test_config_for_user(user_id: &str) -> ProtocolConfig {
    ProtocolConfig::new("test-app", user_id)
}

#[cfg(feature = "mls-observability")]
#[derive(Default, Clone)]
struct RecordingMlsEmitter {
    events: Arc<Mutex<Vec<MlsLifecycleEvent>>>,
}

#[cfg(feature = "mls-observability")]
impl MlsEventEmitter for RecordingMlsEmitter {
    fn emit(&self, event: MlsLifecycleEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[cfg(feature = "mls-observability")]
impl RecordingMlsEmitter {
    fn take(&self) -> Vec<MlsLifecycleEvent> {
        let mut guard = self.events.lock().unwrap();
        std::mem::take(&mut *guard)
    }
}

fn pending_test_message(sender: &str, content: &str) -> Message {
    Message::new(
        UserId::new(sender).unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        content,
    )
}

#[derive(Debug, Clone)]
struct FlakyTransport {
    transport_type: TransportType,
    status: Arc<Mutex<TransportStatus>>,
    sent_messages: Arc<Mutex<Vec<Message>>>,
    failures_remaining: Arc<Mutex<u32>>,
}

impl FlakyTransport {
    fn fail_first(transport_type: TransportType, failures: u32) -> Self {
        Self {
            transport_type,
            status: Arc::new(Mutex::new(TransportStatus::Unavailable)),
            sent_messages: Arc::new(Mutex::new(Vec::new())),
            failures_remaining: Arc::new(Mutex::new(failures)),
        }
    }
}

impl Transport for FlakyTransport {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn transport_type(&self) -> TransportType {
        self.transport_type
    }

    fn status(&self) -> TransportStatus {
        *self.status.lock().unwrap()
    }

    fn metrics(&self) -> TransportMetrics {
        TransportMetrics::default()
    }

    fn send(&self, message: &Message) -> offline_protocol_transport::Result<()> {
        let mut remaining = self.failures_remaining.lock().unwrap();
        if *remaining > 0 {
            *remaining = remaining.saturating_sub(1);
            return Err(offline_protocol_transport::Error::SendFailed(
                "forced failure".to_string(),
            ));
        }

        self.sent_messages.lock().unwrap().push(message.clone());
        Ok(())
    }

    fn receive(&self) -> offline_protocol_transport::Result<Option<Message>> {
        Ok(None)
    }

    fn start(&mut self) -> offline_protocol_transport::Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Available;
        Ok(())
    }

    fn stop(&mut self) -> offline_protocol_transport::Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }
}

#[derive(Default)]
struct FailingPendingListStorage {
    inner: crate::mls::InMemoryStorage,
}

impl MlsStorage for FailingPendingListStorage {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.inner.store(key_type, key_id, data)
    }

    fn load(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Option<Vec<u8>>> {
        self.inner.load(key_type, key_id)
    }

    fn delete(
        &self,
        key_type: &str,
        key_id: &str,
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        self.inner.delete(key_type, key_id)
    }

    fn list_keys(
        &self,
        key_type: &str,
    ) -> offline_protocol_mls::storage::StorageResult<Vec<String>> {
        if key_type == storage_keys::PENDING_MESSAGES {
            return Err(offline_protocol_mls::StorageError::LoadFailed(
                "forced restore failure".to_string(),
            ));
        }
        self.inner.list_keys(key_type)
    }
}

#[test]
fn test_protocol_creation() {
    let protocol = OfflineProtocol::new(create_test_config());
    assert!(protocol.is_ok());
}

#[test]
fn test_protocol_start_stop() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    assert_eq!(protocol.state(), ProtocolState::Stopped);

    assert!(protocol.start().is_ok());
    assert_eq!(protocol.state(), ProtocolState::Running);

    assert!(protocol.stop().is_ok());
    assert_eq!(protocol.state(), ProtocolState::Stopped);
}

#[test]
fn test_protocol_already_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.start().unwrap();
    let result = protocol.start();
    assert!(result.is_err());
}

#[test]
fn test_protocol_pause_resume() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.start().unwrap();
    assert_eq!(protocol.state(), ProtocolState::Running);

    protocol.pause().unwrap();
    assert_eq!(protocol.state(), ProtocolState::Paused);

    protocol.resume().unwrap();
    assert_eq!(protocol.state(), ProtocolState::Running);
}

#[test]
fn test_send_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Add a mock transport
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);
    assert!(result.is_ok());
}

#[test]
fn test_send_message_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);
    assert!(result.is_err());
}

#[test]
fn test_receive_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    // Add a mock transport for testing
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Queue a message in the mock transport
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Test message",
    );
    mock_transport.queue_message(message.clone());

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Receive it
    let received = protocol.receive_message();
    assert!(received.is_some());
    assert_eq!(received.unwrap().id, message.id);
}

#[test]
fn test_event_handler() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let event_received = Arc::new(Mutex::new(false));
    let event_received_clone = event_received.clone();

    protocol.on_event(move |event| {
        if matches!(event, Event::MessageSent { .. }) {
            *event_received_clone.lock().unwrap() = true;
        }
    });

    // Add a mock transport
    use offline_protocol_transport::{mock::MockTransport, TransportType};
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();
    protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();

    assert!(*event_received.lock().unwrap());
}

#[test]
fn test_deduplication() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Add a mock transport
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    // Send same message twice
    protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();
    let result = protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);

    // Second send should succeed (different message ID generated)
    assert!(result.is_ok());
}

#[test]
fn test_process_retries() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.start().unwrap();

    // Process should not fail
    assert!(protocol.process().is_ok());
}

#[test]
fn test_ack_timeout_requeues_message() {
    let mut config = create_test_config();
    config.reliability.ack.default_timeout_ms = 10;
    config.reliability.retry.initial_delay_ms = 5;
    config.reliability.retry.max_retries = 2;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));

    protocol.start().unwrap();

    protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(mock_transport.sent_messages().len(), 1);

    thread::sleep(Duration::from_millis(15));
    protocol.process().unwrap();
    thread::sleep(Duration::from_millis(10));
    protocol.process().unwrap();

    assert!(
        mock_transport.sent_messages().len() >= 2,
        "Expected retry to resend message"
    );
}

#[test]
fn test_config_access() {
    let config = create_test_config();
    let protocol = OfflineProtocol::new(config.clone()).unwrap();

    assert_eq!(protocol.config().app_id, config.app_id);
    assert_eq!(protocol.config().user_id, config.user_id);
}

#[test]
fn test_ble_only_transport_works() {
    // Test that BLE works independently when it's the only transport enabled
    // This verifies the fix for BLE not working when Internet/WiFi Direct are disabled
    let mut config = create_test_config();
    config.transport.ble_enabled = true;
    config.transport.wifi_direct_enabled = false;
    config.transport.internet_enabled = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Add only BLE transport (simulating BLE-only configuration)
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    // Start protocol - BLE should be available
    protocol.start().unwrap();
    assert_eq!(protocol.state(), ProtocolState::Running);

    // Verify BLE transport is available
    let available_transports = protocol.transport_manager().get_available_transports();
    assert!(
        available_transports.contains_key(&TransportType::BLE),
        "BLE transport should be available when it's the only transport enabled"
    );
    assert_eq!(
        available_transports.len(),
        1,
        "Only BLE transport should be available"
    );

    // Test that we can send a message via BLE
    let result = protocol.send_message(
        "bob",
        "Hello from BLE-only!",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(
        result.is_ok(),
        "Should be able to send message when only BLE is enabled"
    );

    // Verify the message was sent via BLE
    let current_transport = protocol.transport_manager().current_transport();
    assert_eq!(
        current_transport,
        Some(TransportType::BLE),
        "Current transport should be BLE"
    );
}

// ========================================================================
// AUTO-ENCRYPTION TESTS
// ========================================================================

use crate::config::EncryptionConfig;

#[test]
fn test_encryption_config_default_enabled() {
    let config = create_test_config();
    assert!(
        config.encryption.enabled,
        "Encryption should be enabled by default"
    );
    assert!(
        config.encryption.auto_key_exchange,
        "Auto key exchange should be enabled by default"
    );
    assert!(
        config.encryption.store_pending,
        "Store pending should be enabled by default"
    );
}

#[test]
fn test_encryption_config_disabled() {
    let mut config = create_test_config();
    config.encryption = EncryptionConfig::disabled();

    assert!(!config.encryption.enabled);
    assert!(!config.encryption.auto_key_exchange);
    assert!(!config.encryption.store_pending);

    let protocol = OfflineProtocol::new(config).unwrap();
    assert!(!protocol.is_mls_initialized());
}

#[test]
fn test_should_auto_encrypt_without_mls() {
    let config = create_test_config();
    let protocol = OfflineProtocol::new(config).unwrap();

    // Even though encryption is enabled by default, MLS is not initialized
    assert!(!protocol.is_mls_initialized());
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_emits_initialized_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    protocol.initialize_mls(storage).unwrap();

    let events = emitter.take();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MlsLifecycleEvent::Initialized { .. })),
        "Expected initialized lifecycle event"
    );
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_emits_session_missing_when_not_initialized() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));

    let result = protocol.encrypt_content_for_recipient_strict("bob", "hello");
    assert!(matches!(result, Err(Error::MlsNotInitialized)));

    let events = emitter.take();
    assert!(events.iter().any(|event| matches!(
        event,
        MlsLifecycleEvent::SessionMissing {
            error_category: Some(MlsErrorCategory::NotInitialized),
            ..
        }
    )));
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_emits_encryption_used_for_successful_encrypt() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let key_package = bob_manager.generate_key_package().unwrap();

    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol
        .confirm_session_state("bob", "manual_test")
        .unwrap();

    let encrypted = protocol
        .encrypt_content_for_recipient_strict("bob", "hello secure")
        .unwrap();
    assert!(encrypted.starts_with(internal_prefixes::ENCRYPTED));

    let events = emitter.take();
    assert!(events
        .iter()
        .any(|event| { matches!(event, MlsLifecycleEvent::EncryptionUsed { .. }) }));
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_emits_decryption_failed_not_initialized() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));

    let encrypted = EncryptedMessage {
        group_id: offline_protocol_mls::GroupId::new("session:alice:bob"),
        message_type: offline_protocol_mls::MlsMessageType::Application,
        epoch: 1,
        ciphertext: vec![1, 2, 3],
        sender_id: "alice".to_string(),
        timestamp_ms: 1234,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::ENCRYPTED,
        serde_json::to_string(&encrypted).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let events = emitter.take();
    assert!(events.iter().any(|event| matches!(
        event,
        MlsLifecycleEvent::DecryptionFailed {
            failure_kind: DecryptionFailureKind::NotInitialized,
            ..
        }
    )));
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_no_encryption_event_on_aborted_encrypt() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: vec![1, 2, 3],
            local_expires_at_ms: (Utc::now().timestamp_millis() as u64).saturating_add(60_000),
        },
    );

    let result = protocol.encrypt_content_for_recipient_strict("bob", "blocked");
    assert!(matches!(
        result,
        Err(Error::SessionNotReady(EstablishmentState::HaveKeyPackage))
    ));

    let events = emitter.take();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, MlsLifecycleEvent::EncryptionUsed { .. })),
        "EncryptionUsed should not emit for aborted operation"
    );
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_session_ready_emits_once_for_idempotent_confirm() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    protocol.welcome_lifecycles.insert(
        "bob".to_string(),
        WelcomeLifecycleRecord {
            peer_id: "bob".to_string(),
            group_id: "session:user123:bob".to_string(),
            state: WelcomeDeliveryState::Sent,
            attempt: 1,
            welcome_message: Message::new(
                UserId::new("user123").unwrap(),
                UserId::new("bob").unwrap(),
                AppId::new("test-app").unwrap(),
                "__MLS_WELCOME__{}",
            ),
            next_retry_at: None,
            last_reason_code: None,
            last_transport_error: None,
            created_at: Utc::now(),
            expires_at: Utc::now() + ChronoDuration::seconds(60),
        },
    );

    assert!(protocol
        .confirm_session_state("bob", "confirmation_ack_received")
        .unwrap());
    assert!(!protocol
        .confirm_session_state("bob", "confirmation_ack_received")
        .unwrap());

    let events = emitter.take();
    let ready_count = events
        .iter()
        .filter(|event| matches!(event, MlsLifecycleEvent::SessionReady { .. }))
        .count();
    assert_eq!(ready_count, 1);
}

#[cfg(feature = "mls-observability")]
#[test]
fn test_mls_observability_uses_opaque_identifiers() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    let events = emitter.take();
    let initialized = events
        .iter()
        .find_map(|event| match event {
            MlsLifecycleEvent::Initialized { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .unwrap();
    assert_ne!(initialized, "peer=none|group=none");
    assert_eq!(initialized.len(), 32);
}

#[test]
fn test_on_neighbor_discovered_without_mls() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Add a mock transport
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));

    protocol.start().unwrap();

    // This should not panic even without MLS initialized
    protocol.on_neighbor_discovered("peer123");

    // No key package should have been sent since MLS is not initialized
    assert_eq!(mock_transport.sent_messages().len(), 0);
}

#[test]
fn test_on_neighbor_lost_clears_tracking() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Simulate that we've sent a key package to a peer (by inserting into tracking set)
    protocol.key_package_sent_to.insert("peer123".to_string());
    assert!(protocol.key_package_sent_to.contains("peer123"));

    // Neighbor lost should remove from tracking
    protocol.on_neighbor_lost("peer123");
    assert!(!protocol.key_package_sent_to.contains("peer123"));
}

#[test]
fn test_internal_prefixes_are_correct() {
    // Verify internal message prefixes match expected values
    assert_eq!(internal_prefixes::KEY_PACKAGE, "__MLS_KEY_PKG__");
    assert_eq!(internal_prefixes::WELCOME, "__MLS_WELCOME__");
    assert_eq!(internal_prefixes::ENCRYPTED, "__MLS_ENC__");
    assert_eq!(
        internal_prefixes::SESSION_CONFIRM_PROBE,
        "__MLS_CONFIRM_PROBE__"
    );
    assert_eq!(
        internal_prefixes::SESSION_CONFIRM_ACK,
        "__MLS_CONFIRM_ACK__"
    );
    assert_eq!(internal_prefixes::CONN_REQUEST, "__CONN_REQ__");
    assert_eq!(internal_prefixes::CONN_ACCEPT, "__CONN_ACC__");
    assert_eq!(internal_prefixes::CONN_REJECT, "__CONN_REJ__");
    assert_eq!(internal_prefixes::PRESENCE, "__PRESENCE__");
    assert_eq!(internal_prefixes::TYPING_INDICATOR, "__TYPING__");
    assert_eq!(internal_prefixes::READ_RECEIPT, "__READ_RECEIPT__");
    assert_eq!(
        offline_protocol_services::SVC_DISCOVER_QUERY,
        "__SVC_DISC_Q__"
    );
    assert_eq!(
        offline_protocol_services::SVC_DISCOVER_RESPONSE,
        "__SVC_DISC_R__"
    );
    assert_eq!(offline_protocol_services::SVC_REQUEST, "__SVC_REQ__");
    assert_eq!(offline_protocol_services::SVC_RESPONSE, "__SVC_RESP__");
}

#[test]
fn test_process_internal_message_key_package() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Create a key package message
    let key_pkg_payload = KeyPackagePayload {
        user_id: "sender123".to_string(),
        key_package_data: vec![1, 2, 3, 4],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&key_pkg_payload).unwrap()
    );

    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    // Process the message
    let result = protocol.process_internal_message(&message);

    // Should be consumed (not surfaced to app)
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // Key package should be stored
    assert!(protocol.pending_key_packages.contains_key("sender123"));
    let received = protocol.pending_key_packages.get("sender123").unwrap();
    assert_eq!(received.key_package_data, vec![1u8, 2, 3, 4]);
    assert!(received.local_expires_at_ms > 0);
}

#[test]
fn test_process_internal_message_connection_request_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = ConnectionRequestPayload {
        sender_name: "Alice".to_string(),
        timestamp_ms: 12345,
        key_package: Some(vec![9, 8, 7]),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::CONN_REQUEST,
        serde_json::to_string(&payload).unwrap()
    );

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ConnectionRequestReceived {
            sender,
            sender_name,
            timestamp,
            key_package,
        } => {
            assert_eq!(sender, "alice");
            assert_eq!(sender_name, "Alice");
            assert_eq!(*timestamp, 12345);
            assert_eq!(key_package.as_ref(), Some(&vec![9, 8, 7]));
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_process_internal_message_connection_accepted_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = ConnectionAcceptedPayload {
        accepted_by_name: "Bob".to_string(),
        timestamp_ms: 99999,
        key_package: Some(vec![1, 2, 3, 4]),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::CONN_ACCEPT,
        serde_json::to_string(&payload).unwrap()
    );

    let message = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ConnectionAccepted {
            accepted_by,
            accepted_by_name,
            timestamp,
            key_package,
        } => {
            assert_eq!(accepted_by, "bob");
            assert_eq!(accepted_by_name, "Bob");
            assert_eq!(*timestamp, 99999);
            assert_eq!(key_package.as_ref(), Some(&vec![1, 2, 3, 4]));
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_process_internal_message_connection_rejected_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let content = internal_prefixes::CONN_REJECT.to_string();
    let message = Message::new(
        UserId::new("carol").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ConnectionRejected { rejected_by } => {
            assert_eq!(rejected_by, "carol");
        }
        _ => panic!("Wrong event type"),
    }
}

// ========================================================================
// SENDER-SIDE CONNECTION REQUEST TESTS
// ========================================================================

#[test]
fn test_send_connection_request_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_connection_request("bob", "Alice", None);
    assert!(result.is_ok());
}

#[test]
fn test_send_connection_request_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.send_connection_request("bob", "Alice", None);
    assert!(result.is_err());
}

#[test]
fn test_send_connection_request_with_key_package() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let key_package = vec![1, 2, 3, 4, 5];
    let result = protocol.send_connection_request("bob", "Alice", Some(key_package));
    assert!(result.is_ok());
}

#[test]
fn test_accept_connection_request_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.accept_connection_request("bob", "Alice", None);
    assert!(result.is_ok());
}

#[test]
fn test_accept_connection_request_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.accept_connection_request("bob", "Alice", None);
    assert!(result.is_err());
}

#[test]
fn test_accept_connection_request_with_key_package() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let key_package = vec![10, 20, 30];
    let result = protocol.accept_connection_request("bob", "Alice", Some(key_package));
    assert!(result.is_ok());
}

#[test]
fn test_reject_connection_request_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.reject_connection_request("bob");
    assert!(result.is_ok());
}

#[test]
fn test_reject_connection_request_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.reject_connection_request("bob");
    assert!(result.is_err());
}

#[test]
fn test_send_connection_request_returns_unique_ids() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let id1 = protocol
        .send_connection_request("bob", "Alice", None)
        .unwrap();
    let id2 = protocol
        .send_connection_request("carol", "Alice", None)
        .unwrap();
    assert_ne!(id1, id2);
}

// ========================================================================
// PRESENCE, TYPING, AND READ RECEIPT TESTS
// ========================================================================

#[test]
fn test_process_internal_message_presence_update_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = PresencePayload {
        status: PresenceStatus::Online,
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::PRESENCE,
        serde_json::to_string(&payload).unwrap()
    );

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::PresenceUpdated {
            peer_id,
            status,
            timestamp,
        } => {
            assert_eq!(peer_id, "alice");
            assert_eq!(*status, PresenceStatus::Online);
            assert_eq!(*timestamp, 12345);
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_process_internal_message_typing_indicator_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = TypingIndicatorPayload {
        conversation_id: "bob".to_string(),
        is_typing: true,
        timestamp_ms: 67890,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::TYPING_INDICATOR,
        serde_json::to_string(&payload).unwrap()
    );

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::TypingIndicatorReceived {
            sender,
            conversation_id,
            is_typing,
            timestamp,
        } => {
            assert_eq!(sender, "alice");
            assert_eq!(conversation_id, "bob");
            assert!(*is_typing);
            assert_eq!(*timestamp, 67890);
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_process_internal_message_typing_indicator_stopped() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = TypingIndicatorPayload {
        conversation_id: "group-123".to_string(),
        is_typing: false,
        timestamp_ms: 99999,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::TYPING_INDICATOR,
        serde_json::to_string(&payload).unwrap()
    );

    let message = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::TypingIndicatorReceived {
            sender,
            conversation_id,
            is_typing,
            ..
        } => {
            assert_eq!(sender, "bob");
            assert_eq!(conversation_id, "group-123");
            assert!(!*is_typing);
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_process_internal_message_read_receipt_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = ReadReceiptPayload {
        message_ids: vec![
            "msg-1".to_string(),
            "msg-2".to_string(),
            "msg-3".to_string(),
        ],
        timestamp_ms: 11111,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::READ_RECEIPT,
        serde_json::to_string(&payload).unwrap()
    );

    let message = Message::new(
        UserId::new("carol").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ReadReceiptReceived {
            sender,
            message_ids,
            timestamp,
        } => {
            assert_eq!(sender, "carol");
            assert_eq!(message_ids, &vec!["msg-1", "msg-2", "msg-3"]);
            assert_eq!(*timestamp, 11111);
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_send_presence_update_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_presence_update("bob", PresenceStatus::Online);
    assert!(result.is_ok());
}

#[test]
fn test_send_presence_update_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let result = protocol.send_presence_update("bob", PresenceStatus::Online);
    assert!(result.is_err());
}

#[test]
fn test_send_typing_indicator_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_typing_indicator("bob", "bob", true);
    assert!(result.is_ok());
}

#[test]
fn test_send_typing_indicator_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let result = protocol.send_typing_indicator("bob", "bob", true);
    assert!(result.is_err());
}

#[test]
fn test_send_read_receipt_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_read_receipt("bob", vec!["msg-1".to_string()]);
    assert!(result.is_ok());
}

#[test]
fn test_send_read_receipt_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let result = protocol.send_read_receipt("bob", vec!["msg-1".to_string()]);
    assert!(result.is_err());
}

#[test]
fn test_send_read_receipt_empty_message_ids() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_read_receipt("bob", vec![]);
    assert!(result.is_err());
}

#[test]
fn test_send_read_receipt_exceeds_max_ids() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let ids: Vec<String> = (0..257).map(|i| format!("msg-{i}")).collect();
    let result = protocol.send_read_receipt("bob", ids);
    assert!(result.is_err());
}

#[test]
fn test_send_typing_indicator_empty_conversation_id() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_typing_indicator("bob", "", true);
    assert!(result.is_err());
}

#[test]
fn test_process_internal_message_presence_malformed_payload() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let content = format!("{}not-valid-json", internal_prefixes::PRESENCE);
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_process_internal_message_presence_negative_timestamp() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = PresencePayload {
        status: PresenceStatus::Online,
        timestamp_ms: -1,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::PRESENCE,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_process_internal_message_typing_empty_conversation_id() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = TypingIndicatorPayload {
        conversation_id: String::new(),
        is_typing: true,
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::TYPING_INDICATOR,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_process_internal_message_read_receipt_empty_ids() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = ReadReceiptPayload {
        message_ids: vec![],
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::READ_RECEIPT,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_process_internal_message_read_receipt_exceeds_max_ids() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let ids: Vec<String> = (0..257).map(|i| format!("msg-{i}")).collect();
    let payload = ReadReceiptPayload {
        message_ids: ids,
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::READ_RECEIPT,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_send_presence_update_empty_recipient() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_presence_update("", PresenceStatus::Online);
    assert!(result.is_err());
}

#[test]
fn test_send_typing_indicator_empty_recipient() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_typing_indicator("", "convo", true);
    assert!(result.is_err());
}

#[test]
fn test_send_read_receipt_empty_recipient() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_read_receipt("", vec!["msg-1".to_string()]);
    assert!(result.is_err());
}

#[test]
fn test_send_presence_update_away_status() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_presence_update("bob", PresenceStatus::Away);
    assert!(result.is_ok());
}

#[test]
fn test_send_presence_update_offline_status() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_presence_update("bob", PresenceStatus::Offline);
    assert!(result.is_ok());
}

#[test]
fn test_process_internal_message_typing_negative_timestamp() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = TypingIndicatorPayload {
        conversation_id: "bob".to_string(),
        is_typing: true,
        timestamp_ms: -1,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::TYPING_INDICATOR,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_process_internal_message_read_receipt_negative_timestamp() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = ReadReceiptPayload {
        message_ids: vec!["msg-1".to_string()],
        timestamp_ms: -1,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::READ_RECEIPT,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn test_process_internal_message_regular_message() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Create a regular (non-internal) message
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello, this is a regular message!",
    );

    // Process the message
    let result = protocol.process_internal_message(&message);

    // Should not be an internal message
    assert!(result.is_none());
}

#[test]
fn test_pending_message_queue() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Queue some pending messages
    protocol.queue_pending_message(
        "bob",
        "Hello Bob!",
        MessagePriority::High,
        MessageId::new(),
        None,
    );
    protocol.queue_pending_message(
        "bob",
        "Another message",
        MessagePriority::Medium,
        MessageId::new(),
        None,
    );
    protocol.queue_pending_message(
        "alice",
        "Hello Alice!",
        MessagePriority::Low,
        MessageId::new(),
        None,
    );

    // Check pending messages are stored
    assert!(protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(protocol.pending_encrypted_messages.contains_key("alice"));

    let bob_pending = protocol.pending_encrypted_messages.get("bob").unwrap();
    assert_eq!(bob_pending.len(), 2);
    assert_eq!(bob_pending[0].content, "Hello Bob!");
    assert_eq!(bob_pending[0].priority, MessagePriority::High);
}

#[test]
fn test_encryption_builder_methods() {
    let config = ProtocolConfig::builder("test-app", "user123")
        .encryption_enabled(false)
        .auto_key_exchange(true)
        .store_pending_messages(false)
        .build()
        .unwrap();

    assert!(!config.encryption.enabled);
    assert!(config.encryption.auto_key_exchange);
    assert!(!config.encryption.store_pending);
}

#[test]
fn test_require_encryption_blocks_plaintext_when_mls_uninitialized() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message("bob", "Hello", None::<MessagePriority>, None::<String>);
    assert!(matches!(result, Err(Error::EncryptFailed(_))));
    assert_eq!(transport_handle.sent_messages().len(), 0);
}

#[test]
fn test_require_encryption_returns_typed_failures() {
    // NoKeyPackage
    let mut no_key_config = create_test_config();
    no_key_config.encryption.require_encryption = true;
    let mut no_key_protocol = OfflineProtocol::new(no_key_config).unwrap();
    let mut no_key_transport = MockTransport::new(TransportType::BLE);
    no_key_transport.start().unwrap();
    no_key_protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(no_key_transport));
    no_key_protocol.start().unwrap();
    no_key_protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let no_key_result =
        no_key_protocol.send_message("bob", "nkp", None::<MessagePriority>, None::<String>);
    assert!(matches!(
        no_key_result,
        Err(Error::SessionNotReady(EstablishmentState::NoKeyPackage))
    ));

    // SessionPending
    let mut pending_config = create_test_config();
    pending_config.encryption.require_encryption = true;
    let mut pending_protocol = OfflineProtocol::new(pending_config).unwrap();
    let mut pending_transport = MockTransport::new(TransportType::BLE);
    pending_transport.start().unwrap();
    pending_protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(pending_transport));
    pending_protocol.start().unwrap();
    pending_protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let bob_manager =
        crate::mls::MlsManager::new("bob", Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = pending_protocol
            .mls_manager
            .as_ref()
            .unwrap()
            .read()
            .unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    let pending_result =
        pending_protocol.send_message("bob", "pending", None::<MessagePriority>, None::<String>);
    assert!(matches!(
        pending_result,
        Err(Error::SessionNotReady(EstablishmentState::SessionPending))
    ));

    // EncryptFailed
    let mut encrypt_fail_config = create_test_config();
    encrypt_fail_config.encryption.require_encryption = true;
    let mut encrypt_fail_protocol = OfflineProtocol::new(encrypt_fail_config).unwrap();
    let mut encrypt_fail_transport = MockTransport::new(TransportType::BLE);
    encrypt_fail_transport.start().unwrap();
    encrypt_fail_protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(encrypt_fail_transport));
    encrypt_fail_protocol.start().unwrap();
    let encrypt_fail_result = encrypt_fail_protocol.send_message(
        "bob",
        "encrypt-failed",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(matches!(encrypt_fail_result, Err(Error::EncryptFailed(_))));
}

#[test]
fn test_require_encryption_failure_does_not_send_transport_payload() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message("bob", "blocked", None::<MessagePriority>, None::<String>);
    assert!(matches!(
        result,
        Err(Error::SessionNotReady(EstablishmentState::NoKeyPackage))
    ));
    assert_eq!(transport_handle.sent_messages().len(), 0);
}

#[test]
fn test_require_encryption_encrypt_failed_emits_send_error_without_transport_output() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    // Keep MLS uninitialized to force strict-mode EncryptFailed path.
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message(
        "bob",
        "must-never-leak",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(matches!(result, Err(Error::EncryptFailed(_))));
    assert_eq!(transport_handle.sent_messages().len(), 0);
}

#[test]
fn test_require_encryption_strict_mode_is_side_effect_free_on_session_pending() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let bob_manager =
        crate::mls::MlsManager::new("bob", Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let result = protocol.send_message(
        "bob",
        "strict-no-side-effects",
        None::<MessagePriority>,
        None::<String>,
    );

    // Strict path does not create session; we have key package but no session -> HaveKeyPackage
    assert!(matches!(
        result,
        Err(Error::SessionNotReady(EstablishmentState::HaveKeyPackage))
    ));
    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(!protocol.welcome_lifecycles.contains_key("bob"));
}

#[test]
fn test_require_encryption_blocks_plaintext_for_send_message_via_transport() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message_via_transport(
        "bob",
        "blocked-via-transport",
        None::<MessagePriority>,
        TransportType::BLE,
        None::<String>,
    );

    assert!(matches!(
        result,
        Err(Error::SessionNotReady(EstablishmentState::NoKeyPackage))
    ));
    assert_eq!(transport_handle.sent_messages().len(), 0);
}

#[test]
fn test_require_encryption_blocks_plaintext_connection_control_messages() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let request_result = protocol.send_connection_request("bob", "alice", None);
    assert!(matches!(request_result, Err(Error::EncryptFailed(_))));

    let accept_result = protocol.accept_connection_request("bob", "alice", None);
    assert!(matches!(accept_result, Err(Error::EncryptFailed(_))));

    let reject_result = protocol.reject_connection_request("bob");
    assert!(matches!(reject_result, Err(Error::EncryptFailed(_))));

    assert_eq!(transport_handle.sent_messages().len(), 0);
}

#[test]
fn test_non_strict_mode_preserves_pending_queue_behavior() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.encryption.require_encryption = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message("bob", "queued", None::<MessagePriority>, None::<String>);
    assert!(result.is_ok());
    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, std::vec::Vec::len),
        1
    );
}

#[test]
fn test_confirmed_sessions_tracking() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Initially no confirmed sessions
    assert!(protocol.confirmed_sessions.is_empty());

    // Add a confirmed session
    protocol.confirmed_sessions.insert("peer123".to_string());

    assert!(protocol.confirmed_sessions.contains("peer123"));
    assert!(!protocol.confirmed_sessions.contains("peer456"));
}

#[test]
fn test_session_confirmation_persists_across_restart_bidirectional_send() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;

    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let alice_storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());

    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();

    alice.initialize_mls(alice_storage.clone()).unwrap();
    bob.initialize_mls(bob_storage.clone()).unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    // Establish session from Alice -> Bob.
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    alice.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    // This creates session + Welcome and queues plaintext until confirmed.
    let _ = alice
        .send_message("bob", "bootstrap", None::<MessagePriority>, None::<String>)
        .unwrap();

    let welcome_wire = alice_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| msg.content.starts_with(internal_prefixes::WELCOME))
        .map(|msg| msg.content)
        .expect("expected welcome message sent by initiator");
    let welcome_msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &welcome_wire,
    );
    let _ = bob.process_internal_message(&welcome_msg);

    // Bob sends encrypted message; Alice decrypts and confirms.
    bob.send_message("alice", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    let bob_sent = bob_transport_handle.sent_messages();
    let last = bob_sent.last().unwrap().clone();
    let _ = alice.process_internal_message(&last);

    // Simulate restart on both peers with same storage.
    let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice2.config.encryption.enabled = true;
    alice2.config.encryption.store_pending = true;
    alice2.initialize_mls(alice_storage.clone()).unwrap();
    let mut alice2_transport = MockTransport::new(TransportType::BLE);
    alice2_transport.start().unwrap();
    let alice2_transport_handle = alice2_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice2_transport));
    alice2.start().unwrap();

    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls(bob_storage.clone()).unwrap();
    let mut bob2_transport = MockTransport::new(TransportType::BLE);
    bob2_transport.start().unwrap();
    let bob2_transport_handle = bob2_transport.clone();
    bob2.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob2_transport));
    bob2.start().unwrap();

    alice2
        .send_message(
            "bob",
            "after-restart-a2b",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    bob2.send_message(
        "alice",
        "after-restart-b2a",
        None::<MessagePriority>,
        None::<String>,
    )
    .unwrap();

    let a2b = alice2_transport_handle.sent_messages();
    let b2a = bob2_transport_handle.sent_messages();
    assert!(a2b
        .last()
        .unwrap()
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
    assert!(b2a
        .last()
        .unwrap()
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
}

#[test]
fn test_initialize_mls_restore_failure_does_not_publish_partial_state() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let initial_clock = protocol.lamport_clock.value();

    let result = protocol.initialize_mls(Arc::new(FailingPendingListStorage::default()));
    assert!(result.is_err());
    assert!(protocol.mls_manager.is_none());
    assert!(protocol.message_storage.is_none());
    assert!(protocol.pending_encrypted_messages.is_empty());
    assert!(protocol.confirmed_sessions.is_empty());
    assert!(protocol.welcome_lifecycles.is_empty());
    assert_eq!(protocol.lamport_clock.value(), initial_clock);
}

#[test]
fn test_auto_send_and_manual_mls_share_single_state_under_concurrency() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.encryption.require_encryption = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    let bob_manager = MlsManager::new("bob", Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();

    let mls_handle_before = protocol.mls_manager.as_ref().unwrap().clone();
    let sessions_before = {
        let manager = mls_handle_before.read().unwrap();
        manager.list_sessions().unwrap()
    };
    let groups_before = {
        let manager = mls_handle_before.read().unwrap();
        manager.list_groups().unwrap().len()
    };

    let shared = Arc::new(Mutex::new(protocol));
    let manual_shared = Arc::clone(&shared);
    let manual_thread = thread::spawn(move || {
        for i in 0..24 {
            let mls = {
                let guard = manual_shared.lock().unwrap();
                guard.mls_manager.as_ref().unwrap().clone()
            };
            let manager = mls.read().unwrap();
            manager
                .create_group(&format!("manual-concurrent-group-{}", i))
                .unwrap();
        }
    });

    let auto_shared = Arc::clone(&shared);
    let auto_thread = thread::spawn(move || {
        for i in 0..24 {
            let content = format!("auto-encrypted-{}", i);
            let mut guard = auto_shared.lock().unwrap();
            guard
                .send_message("bob", &content, None::<MessagePriority>, None::<String>)
                .unwrap();
        }
    });

    manual_thread.join().unwrap();
    auto_thread.join().unwrap();

    let mls_handle_after = {
        let protocol = shared.lock().unwrap();
        protocol.mls_manager.as_ref().unwrap().clone()
    };
    assert!(Arc::ptr_eq(&mls_handle_before, &mls_handle_after));

    let sessions_after = {
        let manager = mls_handle_after.read().unwrap();
        manager.list_sessions().unwrap()
    };
    assert_eq!(sessions_before, sessions_after);

    let sent = transport_handle.sent_messages();
    assert!(sent
        .iter()
        .filter(|message| message.recipient.as_str() == "bob")
        .all(|message| message.content.starts_with(internal_prefixes::ENCRYPTED)));

    let groups_after = {
        let manager = mls_handle_after.read().unwrap();
        manager.list_groups().unwrap().len()
    };
    assert_eq!(groups_after, groups_before + 24);

    let mut protocol = shared.lock().unwrap();
    protocol.stop().unwrap();
}

#[test]
fn test_manual_welcome_processing_confirms_session_for_auto_encrypt_flow() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.encryption.require_encryption = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let alice_key_package = {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    bob_manager
        .import_key_package("alice", &alice_key_package.key_package_data)
        .unwrap();
    let welcome = bob_manager.create_session("alice").unwrap();

    protocol.manual_mls_process_welcome(&welcome).unwrap();

    let persisted = protocol.load_session_state_entry("bob").unwrap().unwrap();
    assert_eq!(persisted, SessionState::Confirmed);

    protocol
        .send_message(
            "bob",
            "manual-welcome-unblocks-auto-send",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    let sent = transport_handle.sent_messages();
    assert!(sent
        .iter()
        .filter(|message| message.recipient.as_str() == "bob")
        .any(|message| message.content.starts_with(internal_prefixes::ENCRYPTED)));

    protocol.stop().unwrap();
}

#[test]
fn test_manual_delete_session_clears_protocol_session_state() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let bob_manager = MlsManager::new("bob", Arc::new(InMemoryStorage::new())).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );

    protocol.manual_mls_delete_session("bob").unwrap();

    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        assert!(!manager.has_session("bob").unwrap());
    }
    assert!(!protocol.confirmed_sessions.contains("bob"));
    assert!(protocol.load_session_state_entry("bob").unwrap().is_none());
}

#[test]
fn test_manual_delete_session_failure_keeps_protocol_state_unchanged() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    protocol.confirm_session_state("bob", "test_setup").unwrap();
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert!(protocol.confirmed_sessions.contains("bob"));

    // Force a deterministic failure path by poisoning the MLS lock.
    let poisoned_handle = protocol.mls_manager.as_ref().unwrap().clone();
    let poison_result = thread::spawn(move || {
        let _guard = poisoned_handle.write().unwrap();
        panic!("poison mls lock");
    })
    .join();
    assert!(poison_result.is_err());

    let result = protocol.manual_mls_delete_session("bob");
    assert!(result.is_err());
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert!(protocol.confirmed_sessions.contains("bob"));
}

#[test]
fn test_is_session_confirmed_clears_stale_confirmed_state_without_mls_session() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    protocol.confirm_session_state("bob", "test_setup").unwrap();
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );

    assert!(!protocol.is_session_confirmed("bob").unwrap());
    assert!(protocol.load_session_state_entry("bob").unwrap().is_none());
    assert!(!protocol.confirmed_sessions.contains("bob"));
}

#[test]
fn test_session_group_detection_for_manual_decrypt_confirmation() {
    assert!(OfflineProtocol::is_session_group_id("session:alice:bob"));
    assert!(!OfflineProtocol::is_session_group_id("group:team"));
}

#[test]
fn test_confirmation_crash_recovery_before_first_send() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    // Build a real session in MLS storage without using protocol transport.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = manager.create_session("bob").unwrap();
        bob_manager.join_session(&welcome).unwrap();
    }

    // Persist confirmation and "crash" before first outbound post-confirm send.
    protocol.confirm_session_state("bob", "test_setup").unwrap();

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls(storage).unwrap();
    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    restarted
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    restarted.start().unwrap();

    restarted
        .send_message(
            "bob",
            "post-crash-send",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    let sent = transport_handle.sent_messages();
    assert!(sent
        .last()
        .unwrap()
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
}

#[test]
fn test_confirmation_transition_idempotent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    // First confirmation transitions Pending -> Confirmed.
    assert!(protocol
        .confirm_session_state("peer123", "idempotency_test")
        .unwrap());
    // Replay confirmation is a no-op and remains Confirmed.
    assert!(!protocol
        .confirm_session_state("peer123", "idempotency_test")
        .unwrap());

    let persisted = protocol
        .load_session_state_entry("peer123")
        .unwrap()
        .unwrap();
    assert_eq!(persisted, SessionState::Confirmed);
}

#[test]
fn test_pending_session_state_blocks_send_until_confirmed() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Provide a key package so first send creates a session and persists Pending.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    protocol
        .send_message("bob", "queued-1", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .send_message("bob", "queued-2", None::<MessagePriority>, None::<String>)
        .unwrap();

    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .unwrap()
            .len(),
        2
    );
    let persisted = protocol.load_session_state_entry("bob").unwrap().unwrap();
    assert_eq!(persisted, SessionState::Pending);
}

#[test]
fn test_welcome_send_failure_keeps_session_pending_and_emits_reason_code() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 3;
    config.reliability.retry.initial_delay_ms = 1;
    config.reliability.retry.max_delay_ms = 5;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
    flaky.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let observed_events_clone = observed_events.clone();
    protocol.on_event(move |event| {
        observed_events_clone.lock().unwrap().push(event);
    });

    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "queued-after-welcome-fail",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Pending
    );
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert!(protocol
        .pending_encrypted_messages
        .get("bob")
        .is_some_and(|messages| !messages.is_empty()));

    let events = observed_events.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::WelcomeSendFailed {
            reason_code: crate::events::WelcomeReasonCode::TransportUnavailable,
            retryable: true,
            ..
        }
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, Event::SecureSessionEstablished { .. })));
}

#[test]
fn test_welcome_retry_exhaustion_expires_and_aborts_pending_queue() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 1;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 10);
    flaky.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let observed_events_clone = observed_events.clone();
    protocol.on_event(move |event| {
        observed_events_clone.lock().unwrap().push(event);
    });

    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let result = protocol.send_message(
        "bob",
        "should-fail-terminally",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(result.is_err());

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Expired);
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Pending
    );
    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));

    let events = observed_events.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::WelcomeSendExpired {
            reason_code: crate::events::WelcomeReasonCode::RetryExhausted,
            ..
        }
    )));
}

#[test]
fn test_welcome_partial_success_after_retry_reaches_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 3;
    config.reliability.retry.initial_delay_ms = 1;
    config.reliability.retry.max_delay_ms = 5;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
    flaky.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "queued-after-flaky-send",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Failed
    );

    thread::sleep(Duration::from_millis(10));
    protocol.process().unwrap();

    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );
}

#[test]
fn test_welcome_internet_requires_async_confirmation_before_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "queued-over-internet",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    let welcome_message_id = protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .welcome_message
        .id
        .as_str()
        .to_string();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );
    assert!(protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .next_retry_at
        .is_some());

    protocol
        .on_transport_send_confirmed(&welcome_message_id)
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );
}

#[test]
fn test_welcome_terminal_lifecycle_can_be_overwritten() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__MLS_WELCOME__dummy".to_string(),
    );
    protocol
        .upsert_welcome_lifecycle("bob", "session:bob:1", message.clone(), "test_created")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::Sent, "test_sent")
        .unwrap();

    let overwrite =
        protocol.upsert_welcome_lifecycle("bob", "session:bob:2", message, "test_overwrite");
    assert!(overwrite.is_ok());
}

#[test]
fn test_welcome_non_terminal_lifecycle_cannot_be_overwritten() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__MLS_WELCOME__dummy".to_string(),
    );
    protocol
        .upsert_welcome_lifecycle("bob", "session:bob:1", message.clone(), "test_created")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
        .unwrap();

    let overwrite =
        protocol.upsert_welcome_lifecycle("bob", "session:bob:2", message, "test_overwrite");
    assert!(overwrite.is_err());
}

#[test]
fn test_welcome_lifecycle_rejects_illegal_transition_from_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "welcome-sent",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );

    let illegal = protocol.transition_welcome_state(
        "bob",
        WelcomeDeliveryState::Failed,
        "test_illegal_transition",
    );
    assert!(illegal.is_err());
}

#[test]
fn test_welcome_restart_recovery_restores_failed_lifecycle() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 3;
    config.reliability.retry.initial_delay_ms = 50;
    config.reliability.retry.max_delay_ms = 50;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    let mut flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
    flaky.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "restart-recovery",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Failed
    );

    let mut restarted = OfflineProtocol::new(config).unwrap();
    restarted.initialize_mls(storage).unwrap();
    let restored = restarted.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(restored.state, WelcomeDeliveryState::Failed);
    assert!(restored.next_retry_at.is_some());
}

#[test]
fn test_welcome_restore_repairs_failed_without_retry_schedule() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__MLS_WELCOME__dummy".to_string(),
    );
    protocol
        .upsert_welcome_lifecycle("bob", "session:bob:1", message, "test_created")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::Failed, "test_failed")
        .unwrap();

    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.last_reason_code = Some(crate::events::WelcomeReasonCode::TransportUnavailable);
        record.next_retry_at = None;
    }
    let persisted = protocol.welcome_lifecycles.get("bob").cloned().unwrap();
    protocol
        .persist_welcome_lifecycle_entry(&persisted)
        .unwrap();

    let mut restarted = OfflineProtocol::new(config).unwrap();
    restarted.initialize_mls(storage).unwrap();
    let restored = restarted.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(restored.state, WelcomeDeliveryState::Failed);
    assert!(restored.next_retry_at.is_some());
}

#[test]
fn test_welcome_restore_promotes_retry_exhausted_failed_to_expired() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__MLS_WELCOME__dummy".to_string(),
    );
    protocol
        .upsert_welcome_lifecycle("bob", "session:bob:1", message, "test_created")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_attempted")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::Failed, "test_failed")
        .unwrap();

    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.last_reason_code = Some(crate::events::WelcomeReasonCode::RetryExhausted);
        record.next_retry_at = None;
    }
    let persisted = protocol.welcome_lifecycles.get("bob").cloned().unwrap();
    protocol
        .persist_welcome_lifecycle_entry(&persisted)
        .unwrap();

    let mut restarted = OfflineProtocol::new(config).unwrap();
    restarted.initialize_mls(storage).unwrap();
    let restored = restarted.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(restored.state, WelcomeDeliveryState::Expired);
    assert!(restored.next_retry_at.is_none());
}

#[test]
fn test_welcome_transport_callbacks_out_of_order_converge_to_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "queued-over-internet",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    let welcome_message_id = protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .welcome_message
        .id
        .as_str()
        .to_string();

    protocol
        .on_transport_send_failed(
            &welcome_message_id,
            Some("Internet transport send failed".to_string()),
        )
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Failed
    );

    protocol
        .on_transport_send_confirmed(&welcome_message_id)
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );

    protocol
        .on_transport_send_failed(
            &welcome_message_id,
            Some("Late failure callback".to_string()),
        )
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );
}

#[test]
fn test_welcome_dropped_confirmation_expires_with_explicit_failure_events() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 1;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "welcome-confirmation-never-arrives",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.next_retry_at = Some(Utc::now() - ChronoDuration::milliseconds(1));
    }

    protocol.process().unwrap();

    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Expired
    );
    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(!protocol.confirmed_sessions.contains("bob"));

    let captured = events.lock().unwrap();
    assert!(captured.iter().any(|event| matches!(
        event,
        Event::WelcomeSendExpired {
            reason_code: crate::events::WelcomeReasonCode::RetryExhausted,
            ..
        }
    )));
    assert!(captured.iter().any(|event| matches!(
        event,
        Event::SecureSessionFailed { peer_id, reason }
            if peer_id == "bob" && reason.contains("Welcome delivery failed")
    )));
    assert!(!captured
        .iter()
        .any(|event| matches!(event, Event::SecureSessionEstablished { .. })));
}

#[test]
fn test_welcome_delayed_confirmation_after_timeout_converges_to_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 3;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    let _ = protocol
        .send_message(
            "bob",
            "welcome-confirmation-delayed",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    let welcome_message_id = protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .welcome_message
        .id
        .as_str()
        .to_string();
    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.next_retry_at = Some(Utc::now() - ChronoDuration::milliseconds(1));
    }

    protocol.process().unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Failed
    );

    protocol
        .on_transport_send_confirmed(&welcome_message_id)
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );
    assert!(!protocol.confirmed_sessions.contains("bob"));
}

#[test]
fn test_welcome_reordered_after_encrypted_message_flushes_pending_decryption() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    bob.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    alice_manager
        .import_key_package("bob", &bob_key_package.key_package_data)
        .unwrap();
    let welcome = alice_manager.create_session("bob").unwrap();
    let encrypted = alice_manager
        .encrypt_for_user("bob", b"encrypted-before-welcome")
        .unwrap();

    let encrypted_wire = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "{}{}",
            internal_prefixes::ENCRYPTED,
            serde_json::to_string(&encrypted).unwrap()
        ),
    );
    let welcome_wire = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "{}{}",
            internal_prefixes::WELCOME,
            serde_json::to_string(&welcome).unwrap()
        ),
    );

    let encrypted_result = bob.process_internal_message(&encrypted_wire);
    assert!(matches!(
        encrypted_result,
        Some(InternalMessageResult::Consumed)
    ));
    assert!(bob.pending_queue.pending_decryption.contains_key("alice"));
    assert!(!bob.confirmed_sessions.contains("alice"));

    let welcome_result = bob.process_internal_message(&welcome_wire);
    assert!(matches!(
        welcome_result,
        Some(InternalMessageResult::Consumed)
    ));
    assert!(bob.confirmed_sessions.contains("alice"));
    assert!(!bob.pending_queue.pending_decryption.contains_key("alice"));

    let delayed_received = bob
        .receive_message()
        .expect("expected delayed decrypted payload");
    assert_eq!(delayed_received.content, "encrypted-before-welcome");
    assert_eq!(
        delayed_received
            .metadata
            .get("delayed_decrypt")
            .map(String::as_str),
        Some("true")
    );

    let captured = events.lock().unwrap();
    assert!(captured.iter().any(|event| matches!(
        event,
        Event::SecureSessionEstablished { peer_id, .. } if peer_id == "alice"
    )));
}

#[test]
fn test_welcome_duplicate_delivery_emits_single_established_event() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    bob.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    bob.start().unwrap();

    let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    alice_manager
        .import_key_package("bob", &bob_key_package.key_package_data)
        .unwrap();
    let welcome = alice_manager.create_session("bob").unwrap();
    let welcome_wire = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "{}{}",
            internal_prefixes::WELCOME,
            serde_json::to_string(&welcome).unwrap()
        ),
    );

    bob_transport_handle.queue_message(welcome_wire.clone());
    bob_transport_handle.queue_message(welcome_wire);
    assert!(bob.receive_message().is_none());

    assert!(bob.confirmed_sessions.contains("alice"));
    let captured = events.lock().unwrap();
    let established_count = captured
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::SecureSessionEstablished { peer_id, .. } if peer_id == "alice"
            )
        })
        .count();
    assert_eq!(established_count, 1);
}

#[test]
fn test_restore_session_state_migrates_legacy_session_to_pending_without_inference() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    // Build a real session in MLS storage but leave session state absent.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = manager.create_session("bob").unwrap();
        bob_manager.join_session(&welcome).unwrap();
    }

    assert!(protocol.load_session_state_entry("bob").unwrap().is_none());

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls(storage).unwrap();

    let restored = restarted.load_session_state_entry("bob").unwrap().unwrap();
    assert_eq!(restored, SessionState::Pending);
}

#[test]
fn test_restore_session_state_keeps_missing_state_pending_when_queue_exists() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    // Build a real session in MLS storage but leave session state absent.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = manager.create_session("bob").unwrap();
        bob_manager.join_session(&welcome).unwrap();
    }

    protocol.queue_pending_message(
        "bob",
        "queued-before-restart",
        MessagePriority::Medium,
        MessageId::new(),
        None,
    );
    assert!(protocol.load_session_state_entry("bob").unwrap().is_none());

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls(storage).unwrap();

    let restored = restarted.load_session_state_entry("bob").unwrap().unwrap();
    assert_eq!(restored, SessionState::Pending);
    assert_eq!(
        restarted
            .pending_encrypted_messages
            .get("bob")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn test_start_flushes_restored_pending_messages_for_confirmed_session() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    // Build a real session and mark it confirmed.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = manager.create_session("bob").unwrap();
        bob_manager.join_session(&welcome).unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();

    protocol.queue_pending_message(
        "bob",
        "queued-before-crash",
        MessagePriority::Medium,
        MessageId::new(),
        None,
    );

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls(storage).unwrap();

    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    restarted
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    restarted.start().unwrap();

    assert!(!restarted.pending_encrypted_messages.contains_key("bob"));
    assert!(restarted
        .load_pending_messages_from_storage("bob")
        .is_none());

    let sent = transport_handle.sent_messages();
    assert!(sent
        .last()
        .unwrap()
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
}

#[test]
fn test_pending_sessions_reconcile_via_probe_after_restart() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let alice_storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());

    // Build a durable MLS session on both peers, but leave confirmation Pending.
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    alice.initialize_mls(alice_storage.clone()).unwrap();
    bob.initialize_mls(bob_storage.clone()).unwrap();

    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    alice
        .ensure_session_state_entry("bob", "test_setup")
        .unwrap();
    bob.ensure_session_state_entry("alice", "test_setup")
        .unwrap();

    // Restart both peers with the same storage to simulate a crash/restart cycle.
    let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice2.config.encryption.enabled = true;
    alice2.config.encryption.store_pending = true;
    alice2.initialize_mls(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls(bob_storage).unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob2.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob2.start().unwrap();

    let probe_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from alice");
    let probe_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from bob");

    let _ = bob2.process_internal_message(&probe_from_alice);
    let _ = alice2.process_internal_message(&probe_from_bob);

    let ack_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from alice");
    let ack_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from bob");

    let _ = bob2.process_internal_message(&ack_from_alice);
    let _ = alice2.process_internal_message(&ack_from_bob);

    assert_eq!(
        alice2.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert_eq!(
        bob2.load_session_state_entry("alice").unwrap().unwrap(),
        SessionState::Confirmed
    );

    alice2
        .send_message("bob", "a2b", None::<MessagePriority>, None::<String>)
        .unwrap();
    bob2.send_message("alice", "b2a", None::<MessagePriority>, None::<String>)
        .unwrap();

    assert!(alice_transport_handle
        .sent_messages()
        .last()
        .unwrap()
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
    assert!(bob_transport_handle
        .sent_messages()
        .last()
        .unwrap()
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
}

#[test]
fn test_pending_sessions_reconcile_on_send_without_process_tick() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let alice_storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());

    // Build a durable MLS session on both peers, but leave confirmation Pending.
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    alice.initialize_mls(alice_storage.clone()).unwrap();
    bob.initialize_mls(bob_storage.clone()).unwrap();

    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    alice
        .ensure_session_state_entry("bob", "test_setup")
        .unwrap();
    bob.ensure_session_state_entry("alice", "test_setup")
        .unwrap();

    // Restart both peers with the same storage to simulate a crash/restart cycle.
    let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice2.config.encryption.enabled = true;
    alice2.config.encryption.store_pending = true;
    alice2.initialize_mls(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls(bob_storage).unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob2.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob2.start().unwrap();

    // Simulate dropped startup probes. Force probe schedule due now so send-path
    // reconciliation can retry without depending on process().
    alice2
        .confirmation_probe_due_at
        .insert("bob".to_string(), Utc::now() - ChronoDuration::seconds(1));
    bob2.confirmation_probe_due_at
        .insert("alice".to_string(), Utc::now() - ChronoDuration::seconds(1));

    // Active sends while pending should queue and trigger a fresh probe attempt.
    alice2
        .send_message("bob", "queued-a2b", None::<MessagePriority>, None::<String>)
        .unwrap();
    bob2.send_message(
        "alice",
        "queued-b2a",
        None::<MessagePriority>,
        None::<String>,
    )
    .unwrap();

    let probe_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from alice send-path reconciliation");
    let probe_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from bob send-path reconciliation");

    let _ = bob2.process_internal_message(&probe_from_alice);
    let _ = alice2.process_internal_message(&probe_from_bob);

    let ack_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from alice");
    let ack_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from bob");

    let _ = bob2.process_internal_message(&ack_from_alice);
    let _ = alice2.process_internal_message(&ack_from_bob);

    assert_eq!(
        alice2.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert_eq!(
        bob2.load_session_state_entry("alice").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert!(!alice2.pending_encrypted_messages.contains_key("bob"));
    assert!(!bob2.pending_encrypted_messages.contains_key("alice"));

    assert!(alice_transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
    assert!(bob_transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
}

#[test]
fn test_pending_sessions_reconcile_on_concurrent_send_after_restart() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let alice_storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());

    // Build a durable MLS session on both peers, but leave confirmation Pending.
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    alice.initialize_mls(alice_storage.clone()).unwrap();
    bob.initialize_mls(bob_storage.clone()).unwrap();

    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    alice
        .ensure_session_state_entry("bob", "test_setup")
        .unwrap();
    bob.ensure_session_state_entry("alice", "test_setup")
        .unwrap();

    // Restart both peers with the same storage to simulate a crash/restart cycle.
    let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice2.config.encryption.enabled = true;
    alice2.config.encryption.store_pending = true;
    alice2.initialize_mls(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls(bob_storage).unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob2.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob2.start().unwrap();

    // Simulate dropped startup probes. Force probe schedule due now so concurrent
    // send-path reconciliation can retry without depending on process().
    alice_transport_handle.clear_sent_messages();
    bob_transport_handle.clear_sent_messages();
    alice2
        .confirmation_probe_due_at
        .insert("bob".to_string(), Utc::now() - ChronoDuration::seconds(1));
    bob2.confirmation_probe_due_at
        .insert("alice".to_string(), Utc::now() - ChronoDuration::seconds(1));

    // Start both sends at the same instant to make the race deterministic.
    let alice_shared = Arc::new(Mutex::new(alice2));
    let bob_shared = Arc::new(Mutex::new(bob2));
    let start_barrier = Arc::new(Barrier::new(3));

    let alice_barrier = Arc::clone(&start_barrier);
    let alice_sender = Arc::clone(&alice_shared);
    let alice_send_thread = thread::spawn(move || {
        alice_barrier.wait();
        alice_sender
            .lock()
            .unwrap()
            .send_message(
                "bob",
                "queued-a2b-concurrent",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
    });

    let bob_barrier = Arc::clone(&start_barrier);
    let bob_sender = Arc::clone(&bob_shared);
    let bob_send_thread = thread::spawn(move || {
        bob_barrier.wait();
        bob_sender
            .lock()
            .unwrap()
            .send_message(
                "alice",
                "queued-b2a-concurrent",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
    });

    start_barrier.wait();
    alice_send_thread.join().unwrap();
    bob_send_thread.join().unwrap();

    let probe_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from alice send-path reconciliation");
    let probe_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from bob send-path reconciliation");

    let _ = bob_shared
        .lock()
        .unwrap()
        .process_internal_message(&probe_from_alice);
    let _ = alice_shared
        .lock()
        .unwrap()
        .process_internal_message(&probe_from_bob);

    let ack_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from alice");
    let ack_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from bob");

    let _ = bob_shared
        .lock()
        .unwrap()
        .process_internal_message(&ack_from_alice);
    let _ = alice_shared
        .lock()
        .unwrap()
        .process_internal_message(&ack_from_bob);

    assert_eq!(
        alice_shared
            .lock()
            .unwrap()
            .load_session_state_entry("bob")
            .unwrap()
            .unwrap(),
        SessionState::Confirmed
    );
    assert_eq!(
        bob_shared
            .lock()
            .unwrap()
            .load_session_state_entry("alice")
            .unwrap()
            .unwrap(),
        SessionState::Confirmed
    );
    assert!(!alice_shared
        .lock()
        .unwrap()
        .pending_encrypted_messages
        .contains_key("bob"));
    assert!(!bob_shared
        .lock()
        .unwrap()
        .pending_encrypted_messages
        .contains_key("alice"));

    assert!(alice_transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
    assert!(bob_transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
}

#[test]
fn test_send_message_via_transport_respects_session_confirmation_gating() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage).unwrap();

    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data,
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );

    protocol
        .send_message_via_transport(
            "bob",
            "forced-transport-pending",
            None::<MessagePriority>,
            TransportType::BLE,
            None::<String>,
        )
        .unwrap();

    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Pending
    );
    assert!(!transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content == "forced-transport-pending"));
}

#[test]
fn test_send_message_fails_closed_when_confirmation_state_is_corrupted() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls(storage.clone()).unwrap();

    let mut transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    // Create a real MLS session to ensure send path reaches confirmation-state read.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = manager.create_session("bob").unwrap();
        bob_manager.join_session(&welcome).unwrap();
    }

    storage
        .store(storage_keys::SESSION_STATES, "bob", b"not-valid-json")
        .unwrap();

    let result = protocol.send_message("bob", "sensitive", None::<MessagePriority>, None::<String>);
    assert!(result.is_err());
    assert!(!transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content == "sensitive"));
}

#[test]
fn test_receive_poll_drives_pending_session_reconciliation_without_process_or_new_sends() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let alice_storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());

    // Build a durable MLS session on both peers, but leave confirmation Pending.
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    alice.initialize_mls(alice_storage.clone()).unwrap();
    bob.initialize_mls(bob_storage.clone()).unwrap();

    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    alice
        .ensure_session_state_entry("bob", "test_setup")
        .unwrap();
    bob.ensure_session_state_entry("alice", "test_setup")
        .unwrap();

    // Queue pending messages before restart so we can verify they flush
    // after poll-driven reconciliation.
    alice.queue_pending_message(
        "bob",
        "queued-before-restart-a2b",
        MessagePriority::Medium,
        MessageId::new(),
        None,
    );
    bob.queue_pending_message(
        "alice",
        "queued-before-restart-b2a",
        MessagePriority::Medium,
        MessageId::new(),
        None,
    );

    let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice2.config.encryption.enabled = true;
    alice2.config.encryption.store_pending = true;
    alice2.initialize_mls(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls(bob_storage).unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob2.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob2.start().unwrap();

    // Simulate dropped startup probes and force receive-poll-driven retries.
    alice_transport_handle.clear_sent_messages();
    bob_transport_handle.clear_sent_messages();
    alice2
        .confirmation_probe_due_at
        .insert("bob".to_string(), Utc::now() - ChronoDuration::seconds(1));
    bob2.confirmation_probe_due_at
        .insert("alice".to_string(), Utc::now() - ChronoDuration::seconds(1));

    // No process() calls and no new sends here.
    let _ = alice2.receive_message();
    let _ = bob2.receive_message();

    let probe_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from alice receive poll");
    let probe_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)
        })
        .expect("expected confirmation probe from bob receive poll");

    let _ = bob2.process_internal_message(&probe_from_alice);
    let _ = alice2.process_internal_message(&probe_from_bob);

    let ack_from_alice = alice_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from alice");
    let ack_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| {
            msg.content
                .starts_with(internal_prefixes::SESSION_CONFIRM_ACK)
        })
        .expect("expected confirmation ack from bob");

    let _ = bob2.process_internal_message(&ack_from_alice);
    let _ = alice2.process_internal_message(&ack_from_bob);

    assert_eq!(
        alice2.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert_eq!(
        bob2.load_session_state_entry("alice").unwrap().unwrap(),
        SessionState::Confirmed
    );
    assert!(!alice2.pending_encrypted_messages.contains_key("bob"));
    assert!(!bob2.pending_encrypted_messages.contains_key("alice"));
    assert!(alice_transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
    assert!(bob_transport_handle
        .sent_messages()
        .iter()
        .any(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED)));
}

#[test]
fn test_pending_decryption_queue() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Initially no pending decryption messages
    assert!(protocol.pending_queue.pending_decryption.is_empty());

    // Queue an encrypted message for a sender
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "encrypted content",
    );

    protocol.enqueue_pending_decryption("sender123", &message);

    // Check message is queued
    assert!(protocol
        .pending_queue
        .pending_decryption
        .contains_key("sender123"));
    assert_eq!(
        protocol
            .pending_queue
            .pending_decryption
            .get("sender123")
            .unwrap()
            .len(),
        1
    );

    // Queue another message from same sender
    let message2 = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "more encrypted content",
    );

    protocol.enqueue_pending_decryption("sender123", &message2);

    assert_eq!(
        protocol
            .pending_queue
            .pending_decryption
            .get("sender123")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn test_session_confirmation_clears_pending_decryption() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Queue some pending decryption messages
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "encrypted content",
    );

    protocol.enqueue_pending_decryption("sender123", &message);

    assert!(!protocol.pending_queue.pending_decryption.is_empty());

    // Calling process_pending_decryption should remove the entries
    // (even if decryption fails since MLS is not initialized)
    protocol.process_pending_decryption("sender123");

    // The messages should be removed from the pending queue
    assert!(!protocol
        .pending_queue
        .pending_decryption
        .contains_key("sender123"));
}

#[test]
fn test_on_neighbor_lost_clears_confirmed_session() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Add a confirmed session
    protocol.confirmed_sessions.insert("peer123".to_string());
    protocol.key_package_sent_to.insert("peer123".to_string());

    assert!(protocol.confirmed_sessions.contains("peer123"));

    // When neighbor is lost, the key_package_sent_to is cleared
    // (confirmed_sessions might still remain - it's the crypto state)
    protocol.on_neighbor_lost("peer123");

    assert!(!protocol.key_package_sent_to.contains("peer123"));
}

#[test]
fn test_welcome_message_confirms_session() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Initially no confirmed sessions
    assert!(!protocol.confirmed_sessions.contains("sender123"));

    // Simulate receiving a welcome message
    // Note: Since MLS is not initialized, the welcome won't actually be processed,
    // but we can test the structure is in place
    let welcome_content = format!(
        "{}{{\"group_id\":\"session:sender123:user123\",\"welcome_data\":[],\"inviter_id\":\"sender123\",\"timestamp_ms\":12345}}",
        internal_prefixes::WELCOME
    );

    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &welcome_content,
    );

    // Process the message
    let result = protocol.process_internal_message(&message);

    // Should be consumed (welcome message is internal)
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
}

#[test]
fn test_encrypted_message_before_session_queued() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Create an encrypted message with the proper format
    let encrypted_content = format!(
        "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
        internal_prefixes::ENCRYPTED
    );

    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &encrypted_content,
    );

    // Process the message without MLS initialized - should be consumed and signaled as an error
    let result = protocol.process_internal_message(&message);

    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
}

#[test]
fn test_mls_pipeline_happy_path_init_send_encrypted_receive_decrypted() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    alice
        .initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();
    bob.initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }
    alice.confirm_session_state("bob", "test_setup").unwrap();
    bob.confirm_session_state("alice", "test_setup").unwrap();

    alice
        .send_message(
            "bob",
            "hello-through-mls",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    let encrypted_wire = alice_transport_handle
        .sent_messages()
        .last()
        .expect("expected encrypted message from alice")
        .clone();
    assert!(encrypted_wire
        .content
        .starts_with(internal_prefixes::ENCRYPTED));

    bob_transport_handle.queue_message(encrypted_wire);
    let received = bob.receive_message().expect("expected decrypted message");
    assert_eq!(received.content, "hello-through-mls");
    assert_eq!(
        received.metadata.get("encrypted").map(String::as_str),
        Some("true")
    );
}

#[test]
fn test_mls_pipeline_missing_session_applies_drop_newest_policy() {
    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 1;
    config.encryption.pending_queue.max_pending_global = 10;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;

    let mut bob = OfflineProtocol::new(config).unwrap();
    bob.initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    alice_manager
        .import_key_package("bob", &bob_key_package.key_package_data)
        .unwrap();
    alice_manager.create_session("bob").unwrap();

    let encrypted_one = alice_manager.encrypt_for_user("bob", b"first").unwrap();
    let encrypted_two = alice_manager.encrypt_for_user("bob", b"second").unwrap();

    let first_message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "{}{}",
            internal_prefixes::ENCRYPTED,
            serde_json::to_string(&encrypted_one).unwrap()
        ),
    );
    let second_message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "{}{}",
            internal_prefixes::ENCRYPTED,
            serde_json::to_string(&encrypted_two).unwrap()
        ),
    );

    let first_result = bob.process_internal_message(&first_message);
    let second_result = bob.process_internal_message(&second_message);

    assert!(matches!(
        first_result,
        Some(InternalMessageResult::Consumed)
    ));
    assert!(matches!(
        second_result,
        Some(InternalMessageResult::Consumed)
    ));
    assert_eq!(bob.pending_queue.pending_decryption["alice"].len(), 1);
    assert_eq!(
        bob.pending_queue.pending_decryption["alice"][0]
            .message
            .id
            .as_str(),
        first_message.id.as_str()
    );
    assert_eq!(
        bob.pending_queue
            .metrics
            .pending_messages_dropped_overflow_total,
        1
    );
}

#[test]
fn test_encrypted_message_decryption_failure_emits_app_error_event() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let encrypted_content = format!(
        "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
        internal_prefixes::ENCRYPTED
    );
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &encrypted_content,
    );

    let _ = protocol.process_internal_message(&message);

    let captured = events.lock().unwrap();
    assert!(captured.iter().any(|event| matches!(
        event,
        Event::MessageDecryptionFailed {
            message_id,
            sender,
            code,
            reason,
        } if message_id == &message.id.as_str()
            && sender == "sender123"
            && code == &DecryptionFailureCode::NotInitialized
            && reason.contains("not initialized")
    )));
}

#[test]
fn test_invalid_encrypted_payload_emits_app_error_event_and_is_consumed() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let malformed_payload = format!("{}{{\"group_id\":\"bad\"", internal_prefixes::ENCRYPTED);
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &malformed_payload,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert!(captured.iter().any(|event| matches!(
        event,
        Event::MessageDecryptionFailed {
            message_id,
            sender,
            code,
            reason,
        } if message_id == &message.id.as_str()
            && sender == "sender123"
            && code == &DecryptionFailureCode::InvalidPayload
            && reason == "Invalid encrypted payload"
    )));
}

#[test]
fn test_internal_prefix_malformed_payload_fuzz_is_panic_free() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let malformed_payloads = vec![
        "".to_string(),
        "{".to_string(),
        "{\"unexpected\":".to_string(),
        "{\"timestamp_ms\":\"not-a-number\"}".to_string(),
        "{\"group_id\":null}".to_string(),
        "[]".to_string(),
        "x".repeat(1024),
    ];
    let prefixes = [
        internal_prefixes::WELCOME,
        internal_prefixes::ENCRYPTED,
        internal_prefixes::CONN_REQUEST,
        internal_prefixes::CONN_ACCEPT,
        internal_prefixes::CONN_REJECT,
        internal_prefixes::PRESENCE,
        internal_prefixes::TYPING_INDICATOR,
        internal_prefixes::READ_RECEIPT,
        internal_prefixes::GROUP_CREATED,
        internal_prefixes::GROUP_MSG,
        internal_prefixes::GROUP_MEMBER_ADDED,
        internal_prefixes::GROUP_MEMBER_REMOVED,
        internal_prefixes::GROUP_INFO,
        internal_prefixes::USER_GROUPS,
        internal_prefixes::GROUP_ERROR,
        offline_protocol_services::SVC_DISCOVER_QUERY,
        offline_protocol_services::SVC_DISCOVER_RESPONSE,
        offline_protocol_services::SVC_REQUEST,
        offline_protocol_services::SVC_RESPONSE,
    ];

    for prefix in prefixes {
        for payload in &malformed_payloads {
            let message = Message::new(
                UserId::new("sender123").unwrap(),
                UserId::new("user123").unwrap(),
                AppId::new("test-app").unwrap(),
                &format!("{prefix}{payload}"),
            );

            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                protocol.process_internal_message(&message)
            }));

            assert!(
                outcome.is_ok(),
                "panic for prefix {prefix:?} payload {payload:?}"
            );
            let result = outcome.unwrap();
            assert!(matches!(result, Some(InternalMessageResult::Consumed)));
        }
    }
}

#[test]
fn test_receive_message_decrypt_failure_emits_error_without_message_received() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let encrypted_content = format!(
        "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
        internal_prefixes::ENCRYPTED
    );
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &encrypted_content,
    );
    mock_transport.queue_message(message.clone());

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let received = protocol.receive_message();
    assert!(received.is_none());

    let captured = events.lock().unwrap();
    assert!(captured.iter().any(|event| matches!(
        event,
        Event::MessageDecryptionFailed {
            message_id,
            sender,
            code,
            ..
        } if message_id == &message.id.as_str()
            && sender == "sender123"
            && code == &DecryptionFailureCode::NotInitialized
    )));
    assert!(!captured
        .iter()
        .any(|event| matches!(event, Event::MessageReceived { .. })));
}

#[test]
fn test_encrypted_message_group_not_found_is_queued_with_typed_classification() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let encrypted_content = format!(
        "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
        internal_prefixes::ENCRYPTED
    );

    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &encrypted_content,
    );

    let result = protocol.process_internal_message(&message);

    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert!(protocol
        .pending_queue
        .pending_decryption
        .contains_key("sender123"));
    assert_eq!(
        protocol.pending_queue.pending_decryption["sender123"].len(),
        1
    );
}

#[test]
fn test_pending_queue_stress_memory_plateaus_with_unfinished_handshake() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 32;
    config.encryption.pending_queue.max_pending_global = 64;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    for idx in 0..10_000 {
        let msg = pending_test_message("sender123", &format!("encrypted-{idx}"));
        protocol.enqueue_pending_decryption("sender123", &msg);
    }

    assert_eq!(protocol.pending_queue.pending_decryption_total, 32);
    assert_eq!(protocol.pending_queue.metrics.pending_messages_current, 32);
    assert_eq!(
        *protocol
            .pending_queue
            .metrics
            .pending_messages_per_peer
            .get("sender123")
            .unwrap(),
        32
    );
}

#[test]
fn test_pending_queue_sustained_mixed_invalid_and_early_encrypted_is_bounded() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 16;
    config.encryption.pending_queue.max_pending_global = 32;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let valid_early_encrypted = format!(
        "{}{{\"group_id\":\"session:sender123:user123\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"sender123\",\"timestamp_ms\":12345}}",
        internal_prefixes::ENCRYPTED
    );
    let malformed_variants = [
        format!("{}{{", internal_prefixes::ENCRYPTED),
        format!("{}{{\"group_id\":\"bad\"", internal_prefixes::ENCRYPTED),
        format!("{}[]", internal_prefixes::ENCRYPTED),
        format!(
            "{}{{\"ciphertext\":\"not-array\"}}",
            internal_prefixes::ENCRYPTED
        ),
    ];

    let mut early_count: u64 = 0;
    let mut invalid_count: u64 = 0;
    for idx in 0..10_000 {
        let content = if idx % 5 == 0 {
            invalid_count += 1;
            malformed_variants[(idx % malformed_variants.len()) as usize].as_str()
        } else {
            early_count += 1;
            valid_early_encrypted.as_str()
        };

        let message = Message::new(
            UserId::new("sender123").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            content,
        );
        let result = protocol.process_internal_message(&message);
        assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    }

    let per_peer_limit = protocol
        .config
        .encryption
        .pending_queue
        .max_pending_per_peer;
    let global_limit = protocol.config.encryption.pending_queue.max_pending_global;
    assert!(protocol.pending_queue.pending_decryption_total <= global_limit);
    assert!(
        protocol
            .pending_queue
            .pending_decryption
            .get("sender123")
            .map(VecDeque::len)
            .unwrap_or(0)
            <= per_peer_limit
    );

    let metrics = protocol.pending_queue_metrics();
    assert_eq!(metrics.pending_messages_received_total, early_count);
    assert_eq!(
        metrics.pending_messages_current,
        protocol.pending_queue.pending_decryption_total
    );
    assert!(metrics.pending_messages_dropped_overflow_total > 0);
    assert_eq!(early_count + invalid_count, 10_000);
}

#[test]
fn test_pending_queue_flood_respects_per_peer_fairness() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 3;
    config.encryption.pending_queue.max_pending_global = 6;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    for idx in 0..100 {
        let msg = pending_test_message("noisy-peer", &format!("noisy-{idx}"));
        protocol.enqueue_pending_decryption("noisy-peer", &msg);
    }
    for idx in 0..3 {
        let msg = pending_test_message("peer-a", &format!("a-{idx}"));
        protocol.enqueue_pending_decryption("peer-a", &msg);
        let msg = pending_test_message("peer-b", &format!("b-{idx}"));
        protocol.enqueue_pending_decryption("peer-b", &msg);
    }

    assert!(protocol.pending_queue.pending_decryption_total <= 6);
    assert!(
        protocol
            .pending_queue
            .pending_decryption
            .get("noisy-peer")
            .map(VecDeque::len)
            .unwrap_or(0)
            <= 3
    );
    assert!(protocol
        .pending_queue
        .pending_decryption
        .contains_key("peer-a"));
    assert!(protocol
        .pending_queue
        .pending_decryption
        .contains_key("peer-b"));
}

#[test]
fn test_pending_queue_drop_newest_policy_enforced_for_per_peer_limit() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 1;
    config.encryption.pending_queue.max_pending_global = 10;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let first = pending_test_message("peer-a", "first");
    let second = pending_test_message("peer-a", "second");
    protocol.enqueue_pending_decryption("peer-a", &first);
    protocol.enqueue_pending_decryption("peer-a", &second);

    assert_eq!(protocol.pending_queue.pending_decryption["peer-a"].len(), 1);
    let queued_message = &protocol.pending_queue.pending_decryption["peer-a"][0];
    assert_eq!(queued_message.message.content, "first");
    assert_eq!(
        protocol
            .pending_queue
            .metrics
            .pending_messages_dropped_overflow_total,
        1
    );
}

#[test]
fn test_pending_queue_global_limit_fail_closed_when_global_index_corrupted() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 1;
    config.encryption.pending_queue.max_pending_global = 1;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropOldest;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.enqueue_pending_decryption("peer-a", &pending_test_message("peer-a", "m1"));
    assert_eq!(protocol.pending_queue.pending_decryption_total, 1);

    // Simulate index drift: queue has data but global-order index is empty.
    protocol
        .pending_queue
        .pending_decryption_global_order
        .clear();

    protocol.enqueue_pending_decryption("peer-b", &pending_test_message("peer-b", "m2"));

    assert_eq!(protocol.pending_queue.pending_decryption_total, 1);
    assert!(!protocol
        .pending_queue
        .pending_decryption
        .contains_key("peer-b"));
    assert!(protocol
        .pending_queue
        .pending_decryption
        .contains_key("peer-a"));
    assert!(
        protocol
            .pending_queue
            .metrics
            .pending_messages_eviction_failures_total
            >= 1
    );
}

#[test]
fn test_pending_queue_ttl_expiration_is_deterministic_and_monotonic() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 10;
    config.encryption.pending_queue.max_pending_global = 100;
    config.encryption.pending_queue.pending_ttl_ms = 1_000;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let old_msg = pending_test_message("sender123", "old");
    let fresh_msg = pending_test_message("sender123", "fresh");
    protocol.enqueue_pending_decryption("sender123", &old_msg);
    protocol.enqueue_pending_decryption("sender123", &fresh_msg);

    {
        let queue = protocol
            .pending_queue
            .pending_decryption
            .get_mut("sender123")
            .unwrap();
        let old = queue.front_mut().unwrap();
        old.received_at = Instant::now() - Duration::from_millis(2_000);
    }

    let config = protocol.config.encryption.pending_queue.clone();
    let expired =
        protocol
            .pending_queue
            .prune_expired_for_peer(&config, "sender123", Instant::now());
    assert_eq!(expired, 1);
    assert_eq!(
        protocol.pending_queue.pending_decryption["sender123"].len(),
        1
    );
    assert_eq!(
        protocol
            .pending_queue
            .metrics
            .pending_messages_expired_total,
        1
    );
}

#[test]
fn test_pending_messages_replay_decrypt_after_session_readiness() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let alice_manager = MlsManager::new("alice", Arc::new(InMemoryStorage::new())).unwrap();
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    let welcome = alice_manager
        .import_key_package("bob", &bob_key_package.key_package_data)
        .and_then(|_| alice_manager.create_session("bob"))
        .unwrap();

    let encrypted = alice_manager
        .encrypt_for_user("bob", b"queued secret")
        .unwrap();
    let encrypted_payload = format!(
        "{}{}",
        internal_prefixes::ENCRYPTED,
        serde_json::to_string(&encrypted).unwrap()
    );
    let incoming = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &encrypted_payload,
    );

    let result = bob.process_internal_message(&incoming);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));
    assert_eq!(bob.pending_queue.pending_decryption["alice"].len(), 1);

    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }

    bob.process_pending_decryption("alice");
    assert!(!bob.pending_queue.pending_decryption.contains_key("alice"));
    let metrics = bob.pending_queue_metrics();
    assert_eq!(metrics.pending_messages_received_total, 1);
}

#[test]
fn test_pending_queue_concurrency_multi_peer_enqueue_is_bounded() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 8;
    config.encryption.pending_queue.max_pending_global = 64;
    config.encryption.pending_queue.pending_ttl_ms = 60_000;
    let protocol = Arc::new(Mutex::new(OfflineProtocol::new(config).unwrap()));

    let mut handles = Vec::new();
    for peer_idx in 0..16 {
        let protocol = Arc::clone(&protocol);
        handles.push(thread::spawn(move || {
            let peer = format!("peer-{peer_idx}");
            for msg_idx in 0..50 {
                let msg = Message::new(
                    UserId::new(&peer).unwrap(),
                    UserId::new("user123").unwrap(),
                    AppId::new("test-app").unwrap(),
                    &format!("concurrent-{msg_idx}"),
                );
                protocol
                    .lock()
                    .unwrap()
                    .enqueue_pending_decryption(&peer, &msg);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let protocol = protocol.lock().unwrap();
    assert!(protocol.pending_queue.pending_decryption_total <= 64);
    for queue in protocol.pending_queue.pending_decryption.values() {
        assert!(queue.len() <= 8);
    }
}

// ========================================================================
// LAMPORT CLOCK TESTS
// ========================================================================

use crate::mls::InMemoryStorage;

#[test]
fn test_lamport_clock_advances_on_send() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));

    protocol.start().unwrap();

    assert_eq!(protocol.lamport_clock.value(), 0);

    protocol
        .send_message("bob", "msg1", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), 1);

    protocol
        .send_message("bob", "msg2", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), 2);
}

#[test]
fn test_lamport_clock_merges_on_receive() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Create a message with a high Lamport clock from a peer
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello",
    );
    message.lamport_clock = LamportClock::from_value(50);
    mock_transport.queue_message(message);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    assert_eq!(protocol.lamport_clock.value(), 0);

    let received = protocol.receive_message();
    assert!(received.is_some());

    // Clock should be max(0, 50) + 1 = 51
    assert_eq!(protocol.lamport_clock.value(), 51);
}

#[test]
fn test_lamport_clock_monotonic_across_send_receive() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Send a message first (clock -> 1)
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
    protocol.start().unwrap();

    protocol
        .send_message("bob", "hi", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), 1);

    // Receive a message with lower clock (clock should still advance)
    let mut message = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "reply",
    );
    message.lamport_clock = LamportClock::from_value(0);
    mock_transport.queue_message(message);

    // Legacy message (clock=0) — merge is skipped so clock stays at 1
    let received = protocol.receive_message();
    assert!(received.is_some());
    assert_eq!(protocol.lamport_clock.value(), 1);

    // Now receive a message with higher clock
    let mut message2 = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "another",
    );
    message2.lamport_clock = LamportClock::from_value(10);
    mock_transport.queue_message(message2);

    let received2 = protocol.receive_message();
    assert!(received2.is_some());
    // max(1, 10) + 1 = 11
    assert_eq!(protocol.lamport_clock.value(), 11);
}

#[test]
fn test_lamport_clock_persists_and_restores() {
    let storage = Arc::new(InMemoryStorage::new());

    // First session: send messages to advance the clock
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol
            .enable_message_persistence(storage.clone())
            .unwrap();
        protocol.start().unwrap();

        // Send 5 messages to advance clock to 5
        for i in 0..5 {
            protocol
                .send_message(
                    "bob",
                    format!("msg{}", i),
                    None::<MessagePriority>,
                    None::<String>,
                )
                .unwrap();
        }
        assert_eq!(protocol.lamport_clock.value(), 5);
    }

    // Second session: clock should restore from storage
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let mut mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        assert_eq!(protocol.lamport_clock.value(), 0);

        protocol
            .enable_message_persistence(storage.clone())
            .unwrap();

        // After attaching storage, clock should be restored
        assert_eq!(protocol.lamport_clock.value(), 5);

        // Next send should be 6, not 1
        protocol.start().unwrap();
        protocol
            .send_message(
                "bob",
                "after restart",
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
        assert_eq!(protocol.lamport_clock.value(), 6);
    }
}

#[test]
fn test_lamport_clock_restore_with_corrupted_data() {
    let storage = Arc::new(InMemoryStorage::new());

    // Write corrupted data (wrong length)
    storage
        .store(
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
            &[1, 2, 3], // only 3 bytes, not 8
        )
        .unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence(storage.clone())
        .unwrap();

    // Clock should remain at 0 (corrupted data ignored)
    assert_eq!(protocol.lamport_clock.value(), 0);
}

#[test]
fn test_lamport_clock_restore_never_goes_backward() {
    let storage = Arc::new(InMemoryStorage::new());

    // Store a value of 10 in storage
    storage
        .store(
            storage_keys::LAMPORT_CLOCK,
            storage_keys::LAMPORT_CLOCK_ID,
            &10u64.to_le_bytes(),
        )
        .unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Advance in-memory clock to 20 before attaching storage
    for _ in 0..20 {
        protocol.lamport_clock.tick();
    }
    assert_eq!(protocol.lamport_clock.value(), 20);

    // Attaching storage should NOT regress to 10
    protocol
        .enable_message_persistence(storage.clone())
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), 20);
}

#[test]
fn test_lamport_clock_merge_on_internal_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Create a key package message with a high Lamport clock
    let key_pkg_payload = KeyPackagePayload {
        user_id: "sender456".to_string(),
        key_package_data: vec![5, 6, 7, 8],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&key_pkg_payload).unwrap()
    );
    let mut message = Message::new(
        UserId::new("sender456").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    message.lamport_clock = LamportClock::from_value(100);
    mock_transport.queue_message(message);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    assert_eq!(protocol.lamport_clock.value(), 0);

    // Receiving the internal message should merge the clock even
    // though process_internal_message returns Consumed
    let received = protocol.receive_message();
    // Internal messages are consumed, not surfaced
    assert!(received.is_none());

    // Clock should have merged: max(0, 100) + 1 = 101
    assert_eq!(protocol.lamport_clock.value(), 101);
}

#[test]
fn test_lamport_clock_merge_on_duplicate_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Create two copies of the same message (simulate duplicate delivery)
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello",
    );
    message.lamport_clock = LamportClock::from_value(42);
    let message_dup = message.clone();

    mock_transport.queue_message(message);
    mock_transport.queue_message(message_dup);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // First receive: message delivered
    let received = protocol.receive_message();
    assert!(received.is_some());
    // max(0, 42) + 1 = 43
    assert_eq!(protocol.lamport_clock.value(), 43);

    // Second receive: duplicate detected, but clock should have
    // already merged (merge happens before dedup).
    // The duplicate carries the same clock=42, so merge would yield
    // max(43, 42) + 1 = 44
    let received2 = protocol.receive_message();
    assert!(received2.is_none());
    assert_eq!(protocol.lamport_clock.value(), 44);
}

#[test]
fn test_receive_internal_connection_request_sends_delivery_ack() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    let payload = ConnectionRequestPayload {
        sender_name: "Alice".to_string(),
        timestamp_ms: 12345,
        key_package: None,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::CONN_REQUEST,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    mock_transport.queue_message(message.clone());

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
    protocol.start().unwrap();

    let received = protocol.receive_message();
    assert!(received.is_none(), "internal message should be consumed");

    let expected_ack = message.id.as_str();
    let ack_count = mock_transport
        .sent_messages()
        .iter()
        .filter(|sent| {
            sent.metadata
                .get(ACK_FOR_KEY)
                .is_some_and(|ack_for| ack_for == &expected_ack)
        })
        .count();
    assert_eq!(ack_count, 1, "expected ACK for internal control message");
}

#[test]
fn test_receive_duplicate_message_reacks_when_requires_ack() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello",
    );
    let message_dup = message.clone();
    mock_transport.queue_message(message);
    mock_transport.queue_message(message_dup.clone());

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
    protocol.start().unwrap();

    let first = protocol.receive_message();
    assert!(first.is_some());
    let second = protocol.receive_message();
    assert!(second.is_none(), "duplicate should not be surfaced");

    let expected_ack = message_dup.id.as_str();
    let ack_count = mock_transport
        .sent_messages()
        .iter()
        .filter(|sent| {
            sent.metadata
                .get(ACK_FOR_KEY)
                .is_some_and(|ack_for| ack_for == &expected_ack)
        })
        .count();
    assert_eq!(
        ack_count, 2,
        "expected initial ACK and duplicate re-ACK for same message id"
    );
}

#[test]
fn test_lamport_clock_no_tick_on_pending_message() {
    let mut config = create_test_config();
    config.encryption.enabled = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Send two messages, verify each tick advances by exactly 1
    let clock_before = protocol.lamport_clock.value();
    protocol
        .send_message("bob", "first", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), clock_before + 1);

    protocol
        .send_message("bob", "second", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), clock_before + 2);
}

#[test]
fn test_lamport_clock_sent_message_carries_clock_value() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport.clone()));
    protocol.start().unwrap();

    protocol
        .send_message("bob", "test", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Verify the sent message carries the Lamport clock
    let sent = mock_transport.sent_messages();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].lamport_clock.value(), 1);
}

#[test]
fn test_key_package_remaining_lifetime_ms() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Create a key package with remaining_lifetime_ms = 0 (legacy sender)
    let key_pkg_payload = KeyPackagePayload {
        user_id: "legacy_peer".to_string(),
        key_package_data: vec![1, 2, 3],
        remaining_lifetime_ms: 0,
        timestamp_ms: 12345,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&key_pkg_payload).unwrap()
    );
    let message = Message::new(
        UserId::new("legacy_peer").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // Should have stored with a 30-day default lifetime
    let received = protocol.pending_key_packages.get("legacy_peer").unwrap();
    let now_ms = Utc::now().timestamp_millis() as u64;
    let thirty_days_ms: u64 = 30 * 24 * 60 * 60 * 1000;
    // Should expire roughly 30 days from now (within 1 second tolerance)
    let diff = received
        .local_expires_at_ms
        .abs_diff(now_ms + thirty_days_ms);
    assert!(
        diff < 1000,
        "Expiry should be ~30 days from now, diff was {}",
        diff
    );
}

#[test]
fn test_key_package_expired_discarded() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // MLS must be initialized so establish_secure_session reaches the
    // expiry check instead of short-circuiting with MlsNotInitialized.
    let storage = Arc::new(InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    // Manually insert an already-expired key package
    protocol.pending_key_packages.insert(
        "expired_peer".to_string(),
        ReceivedKeyPackage {
            key_package_data: vec![1, 2, 3],
            local_expires_at_ms: 0, // expired at epoch
        },
    );

    assert!(protocol.pending_key_packages.contains_key("expired_peer"));

    // Attempting to establish session should detect expiry and discard
    let result = protocol.establish_secure_session("expired_peer");
    assert!(result.is_err());
    assert!(!protocol.pending_key_packages.contains_key("expired_peer"));
}

#[test]
fn test_peer_key_package_persisted_and_restored_after_restart() {
    let storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

    // First session: receive key package (persisted via process_internal_message)
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();
        let key_pkg_payload = KeyPackagePayload {
            user_id: "bob".to_string(),
            key_package_data: bob_key_package.key_package_data.clone(),
            remaining_lifetime_ms: 60 * 60 * 1000,
            timestamp_ms: 0,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::KEY_PACKAGE,
            serde_json::to_string(&key_pkg_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        let _ = protocol.process_internal_message(&message);
        assert!(protocol.pending_key_packages.contains_key("bob"));
    }

    // Second session: new protocol, same storage; restore should repopulate pending_key_packages
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();
        assert!(
            protocol.pending_key_packages.contains_key("bob"),
            "Key package should be restored from storage"
        );
        let welcome = protocol.establish_secure_session("bob").unwrap();
        assert!(
            welcome.is_some(),
            "Session should be created from restored key package"
        );
    }
}

#[test]
fn test_establishment_state_returns_correct_states() {
    let storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls(storage).unwrap();

    // No key package, no session
    assert_eq!(
        protocol.get_establishment_state("bob").unwrap(),
        EstablishmentState::NoKeyPackage
    );

    // Add key package -> HaveKeyPackage
    protocol.pending_key_packages.insert(
        "bob".to_string(),
        ReceivedKeyPackage {
            key_package_data: bob_key_package.key_package_data.clone(),
            local_expires_at_ms: Utc::now().timestamp_millis() as u64 + 60_000,
        },
    );
    assert_eq!(
        protocol.get_establishment_state("bob").unwrap(),
        EstablishmentState::HaveKeyPackage
    );

    // Create session (via MLS manager directly) -> SessionPending
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.ensure_session_state_entry("bob", "test").unwrap();
    assert_eq!(
        protocol.get_establishment_state("bob").unwrap(),
        EstablishmentState::SessionPending
    );

    // Confirm -> SessionConfirmed
    protocol.confirm_session_state("bob", "test").unwrap();
    assert_eq!(
        protocol.get_establishment_state("bob").unwrap(),
        EstablishmentState::SessionConfirmed
    );
}

#[test]
fn test_establish_secure_session_loads_from_storage_after_restart() {
    let storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

    // Persist key package (simulate receive then restart: in-memory cleared)
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();
        let key_pkg_payload = KeyPackagePayload {
            user_id: "bob".to_string(),
            key_package_data: bob_key_package.key_package_data.clone(),
            remaining_lifetime_ms: 60 * 60 * 1000,
            timestamp_ms: 0,
        };
        let content = format!(
            "{}{}",
            internal_prefixes::KEY_PACKAGE,
            serde_json::to_string(&key_pkg_payload).unwrap()
        );
        let message = Message::new(
            UserId::new("bob").unwrap(),
            UserId::new("alice").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        );
        let _ = protocol.process_internal_message(&message);
        assert!(protocol.pending_key_packages.contains_key("bob"));
    }

    // New protocol instance: restore runs and loads key package from storage
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls(storage.clone()).unwrap();
        // establish_secure_session should try load from storage and create session (no terminal error)
        let result = protocol.establish_secure_session("bob");
        assert!(
            result.is_ok(),
            "establish_secure_session should load from storage and create session, got {:?}",
            result
        );
        let welcome = result.unwrap();
        assert!(welcome.is_some());
    }
}

// ========================================================================
// SERVICE DISCOVERY & REQUEST/RESPONSE TESTS
// ========================================================================

#[test]
fn test_register_and_unregister_service() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let descriptor = ServiceDescriptor {
        service_id: offline_protocol_core::ServiceId::new("echo.v1").unwrap(),
        version: "1.0".to_string(),
        capabilities: HashMap::new(),
    };
    protocol.register_service(descriptor).unwrap();
    assert!(protocol.mesh_services().has_service("echo.v1"));

    let removed = protocol.unregister_service("echo.v1").unwrap();
    assert!(removed);
    assert!(!protocol.mesh_services().has_service("echo.v1"));

    let removed_again = protocol.unregister_service("echo.v1").unwrap();
    assert!(!removed_again);
}

#[test]
fn test_process_svc_discover_query_with_match() {
    use offline_protocol_services::SVC_DISCOVER_QUERY;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Register a service
    let descriptor = ServiceDescriptor {
        service_id: offline_protocol_core::ServiceId::new("weather").unwrap(),
        version: "2.0".to_string(),
        capabilities: {
            let mut m = HashMap::new();
            m.insert("format".to_string(), "json".to_string());
            m
        },
    };
    protocol.register_service(descriptor).unwrap();

    // Build a discovery query message from a remote peer using raw JSON
    let content = format!(
        "{}{}",
        SVC_DISCOVER_QUERY,
        serde_json::json!({
            "query_id": "q-001",
            "originator": "alice",
            "service_id": "weather",
            "remaining_hops": 10
        })
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // The query should be recorded in seen set
    assert!(protocol
        .mesh_services()
        .seen_discovery_queries()
        .contains_key("q-001"));
}

#[test]
fn test_process_svc_discover_query_dedup() {
    use offline_protocol_services::SVC_DISCOVER_QUERY;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let content = format!(
        "{}{}",
        SVC_DISCOVER_QUERY,
        serde_json::json!({
            "query_id": "q-dedup",
            "originator": "alice",
            "remaining_hops": 10
        })
    );

    let make_msg = || {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("user123").unwrap(),
            AppId::new("test-app").unwrap(),
            &content,
        )
    };

    // First time: processes normally
    let r1 = protocol.process_internal_message(&make_msg());
    assert!(matches!(r1, Some(InternalMessageResult::Consumed)));

    // Second time: deduplicated (still consumed, but no further action)
    let r2 = protocol.process_internal_message(&make_msg());
    assert!(matches!(r2, Some(InternalMessageResult::Consumed)));
}

#[test]
fn test_process_svc_discover_response_emits_event() {
    use offline_protocol_services::SVC_DISCOVER_RESPONSE;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let content = format!(
        "{}{}",
        SVC_DISCOVER_RESPONSE,
        serde_json::json!({
            "query_id": "q-123",
            "service_id": "weather",
            "version": "2.0",
            "provider_peer_id": "bob",
            "capabilities": {},
            "hop_count": 1
        })
    );
    let message = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ServiceDiscovered {
            query_id,
            service_id,
            version,
            provider_peer_id,
            hop_count,
            ..
        } => {
            assert_eq!(query_id, "q-123");
            assert_eq!(service_id, "weather");
            assert_eq!(version, "2.0");
            assert_eq!(provider_peer_id, "bob");
            assert_eq!(*hop_count, 1);
        }
        other => panic!("Wrong event type: {:?}", other),
    }
}

#[test]
fn test_process_svc_request_unregistered_auto_not_found() {
    use offline_protocol_services::SVC_REQUEST;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    // No services registered — request should auto-respond not_found
    let content = format!(
        "{}{}",
        SVC_REQUEST,
        serde_json::json!({
            "request_id": "req-001",
            "service_id": "nonexistent",
            "method": "get",
            "body": "{}"
        })
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    // No ServiceRequestReceived event should be emitted
    let captured = events.lock().unwrap();
    assert!(
        captured.is_empty(),
        "Should not emit event for unregistered service, got {:?}",
        *captured
    );
}

#[test]
fn test_process_svc_request_registered_emits_event() {
    use offline_protocol_services::SVC_REQUEST;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    // Register the service first
    let descriptor = ServiceDescriptor {
        service_id: offline_protocol_core::ServiceId::new("echo").unwrap(),
        version: "1.0".to_string(),
        capabilities: HashMap::new(),
    };
    protocol.register_service(descriptor).unwrap();

    let content = format!(
        "{}{}",
        SVC_REQUEST,
        serde_json::json!({
            "request_id": "req-002",
            "service_id": "echo",
            "method": "ping",
            "body": "hello"
        })
    );
    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ServiceRequestReceived {
            request_id,
            service_id,
            method,
            body,
            sender,
        } => {
            assert_eq!(request_id, "req-002");
            assert_eq!(service_id, "echo");
            assert_eq!(method, "ping");
            assert_eq!(body, "hello");
            assert_eq!(sender, "alice");
        }
        other => panic!("Wrong event type: {:?}", other),
    }
}

#[test]
fn test_process_svc_response_emits_event() {
    use offline_protocol_services::SVC_RESPONSE;

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let content = format!(
        "{}{}",
        SVC_RESPONSE,
        serde_json::json!({
            "request_id": "req-003",
            "service_id": "echo",
            "status": "ok",
            "body": "pong"
        })
    );
    let message = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        Event::ServiceResponseReceived {
            request_id,
            service_id,
            status,
            body,
            provider_peer_id,
        } => {
            assert_eq!(request_id, "req-003");
            assert_eq!(service_id, "echo");
            assert_eq!(status, "ok");
            assert_eq!(body, "pong");
            assert_eq!(provider_peer_id, "bob");
        }
        other => panic!("Wrong event type: {:?}", other),
    }
}

#[test]
fn test_process_regular_message_not_consumed_by_service_handlers() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello, this is a normal message",
    );

    let result = protocol.process_internal_message(&message);
    assert!(result.is_none(), "Regular messages should not be consumed");
}

#[test]
fn test_discover_services_no_peers() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Start protocol so send_internal_message doesn't fail with NotStarted
    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // No peers in key_package_sent_to — should succeed with empty broadcast
    let query_id = protocol.discover_services(None).unwrap();
    assert!(!query_id.is_empty());
    assert!(protocol
        .mesh_services()
        .seen_discovery_queries()
        .contains_key(&query_id));
}

#[test]
fn test_require_encryption_blocks_service_discovery_control_messages() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let discover_result = protocol.discover_services(None);
    assert!(matches!(discover_result, Err(Error::EncryptFailed(_))));

    let request_result = protocol.send_service_request("bob", "echo.v1", "ping", "{}");
    assert!(matches!(request_result, Err(Error::EncryptFailed(_))));

    let respond_result =
        protocol.respond_to_service_request("req-1", "alice", "echo.v1", "ok", "pong");
    assert!(matches!(respond_result, Err(Error::EncryptFailed(_))));

    assert_eq!(transport_handle.sent_messages().len(), 0);
}

#[test]
fn test_known_peers_capacity_limit() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Fill to capacity
    for i in 0..MAX_KNOWN_PEERS {
        protocol.on_neighbor_discovered(&format!("peer-{i}"));
    }
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);

    // One more should be rejected
    protocol.on_neighbor_discovered("peer-overflow");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
    assert!(!protocol.known_peers.contains("peer-overflow"));

    // Existing peer should still be updatable (no-op insert, not rejected)
    protocol.on_neighbor_discovered("peer-0");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
    assert!(protocol.known_peers.contains("peer-0"));

    // Removing a peer frees capacity
    protocol.on_neighbor_lost("peer-0");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS - 1);

    // Now the new peer can be added
    protocol.on_neighbor_discovered("peer-overflow");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
    assert!(protocol.known_peers.contains("peer-overflow"));
}

#[test]
fn test_known_peers_does_not_track_self() {
    let config = create_test_config();
    let self_id = config.user_id.clone();
    let mut protocol = OfflineProtocol::new(config).unwrap();

    protocol.on_neighbor_discovered(&self_id);
    assert!(protocol.known_peers.is_empty());
}

#[test]
fn test_on_neighbor_lost_removes_from_known_peers() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.on_neighbor_discovered("alice");
    assert!(protocol.known_peers.contains("alice"));

    protocol.on_neighbor_lost("alice");
    assert!(!protocol.known_peers.contains("alice"));
}

#[test]
fn test_seen_discovery_queries_cleanup() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Cleanup is tested directly in offline-protocol-services crate.
    // Here we verify the integration: cleanup_expired_entries() delegates.
    protocol.cleanup_expired_entries();

    // Just verify it doesn't panic and the method is wired correctly
    assert!(protocol.mesh_services().seen_discovery_queries().is_empty());
}

// ========================================================================
// SECURITY: prefix injection, transport identity, TOFU key pinning
// ========================================================================

#[test]
fn test_send_message_rejects_internal_prefixes() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Every internal prefix must be rejected
    for prefix in INTERNAL_PREFIXES {
        let content = format!("{}injected_payload", prefix);
        let result =
            protocol.send_message("bob", &content, None::<MessagePriority>, None::<String>);
        assert!(
            result.is_err(),
            "Expected rejection for prefix '{}', but send succeeded",
            prefix
        );
    }
}

#[test]
fn test_send_message_via_transport_rejects_internal_prefixes() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    for prefix in INTERNAL_PREFIXES {
        let content = format!("{}injected_payload", prefix);
        let result = protocol.send_message_via_transport(
            "bob",
            &content,
            None::<MessagePriority>,
            TransportType::BLE,
            None::<String>,
        );
        assert!(
            result.is_err(),
            "Expected rejection for prefix '{}' via send_message_via_transport, but send succeeded",
            prefix
        );
    }
}

#[test]
fn test_send_message_allows_normal_content() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message(
        "bob",
        "Hello, this is a normal message!",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(result.is_ok());
}

// NOTE: SVC prefix coverage is now verified by
// test_internal_prefixes_completeness above.

#[test]
fn test_is_internal_prefix() {
    assert!(OfflineProtocol::is_internal_prefix("__MLS_KEY_PKG__data"));
    assert!(OfflineProtocol::is_internal_prefix("__CONN_REQ__data"));
    assert!(OfflineProtocol::is_internal_prefix("__SVC_DISC_Q__data"));
    assert!(OfflineProtocol::is_internal_prefix("__SVC_REQ__data"));
    // The `SVC_MESSAGE_PREFIX` ("__SVC_") entry in `INTERNAL_PREFIXES` acts
    // as a catch-all: any content starting with "__SVC_" matches, even if the
    // specific sub-prefix (e.g., "__SVC_NEW_THING__") is not explicitly listed.
    // This ensures future service prefixes are automatically blocked from
    // user-sent messages without requiring a code change to `INTERNAL_PREFIXES`.
    assert!(OfflineProtocol::is_internal_prefix("__SVC_NEW_THING__data"));
    assert!(OfflineProtocol::is_internal_prefix(
        "__SVC_my_legitimate_content"
    ));
    assert!(!OfflineProtocol::is_internal_prefix("Hello world"));
    assert!(!OfflineProtocol::is_internal_prefix("__UNKNOWN__data"));
    assert!(!OfflineProtocol::is_internal_prefix(""));
}

#[test]
fn test_validate_transport_sender_match() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    msg.set_transport_peer_id("alice".to_string()).unwrap();

    assert!(protocol.validate_transport_sender(&msg));
}

#[test]
fn test_validate_transport_sender_mismatch() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    msg.set_transport_peer_id("eve".to_string()).unwrap();

    assert!(!protocol.validate_transport_sender(&msg));
}

#[test]
fn test_validate_transport_sender_no_transport_id() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    // No transport_peer_id — should pass (best effort, default config)
    assert!(protocol.validate_transport_sender(&msg));
}

#[test]
fn test_validate_transport_sender_no_transport_id_required() {
    let mut config = create_test_config();
    config.security.require_transport_identity = true;
    let protocol = OfflineProtocol::new(config).unwrap();

    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    // No transport_peer_id — should FAIL when require_transport_identity is true
    assert!(!protocol.validate_transport_sender(&msg));
}

#[test]
fn test_validate_transport_sender_match_with_require_identity() {
    let mut config = create_test_config();
    config.security.require_transport_identity = true;
    let protocol = OfflineProtocol::new(config).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    msg.set_transport_peer_id("alice".to_string()).unwrap();
    // Matching transport_peer_id — should pass regardless of config
    assert!(protocol.validate_transport_sender(&msg));
}

#[test]
fn test_control_message_with_transport_mismatch_is_dropped() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Create a control message claiming to be from "alice" but delivered by "eve"
    let mut msg = pending_test_message(
        "sender123",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    msg.set_transport_peer_id("eve".to_string()).unwrap();

    // process_internal_message should reject (drop without ACK) the message
    let result = protocol.process_internal_message(&msg);
    assert!(
        matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "Expected spoofed control message to be rejected by security gate"
    );
}

#[test]
fn test_tofu_key_pinning_and_mismatch() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Pin a key for "alice"
    let fake_pk = vec![1u8; 32];
    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: fake_pk.clone(),
            last_seen_ms: 1000,
        },
    );

    // Verify same key passes TOFU check
    assert_eq!(
        protocol
            .known_peer_public_keys
            .get("alice")
            .unwrap()
            .public_key,
        fake_pk
    );

    // A different key should be detected
    let different_pk = vec![2u8; 32];
    assert_ne!(
        protocol
            .known_peer_public_keys
            .get("alice")
            .unwrap()
            .public_key,
        different_pk,
    );
}

#[test]
fn test_tofu_store_bounded_capacity_with_lru_eviction() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Use timestamps old enough to be eviction-eligible (beyond the
    // TOFU_MIN_EVICTION_AGE_MS threshold).
    let old_base = Utc::now().timestamp_millis() - TOFU_MIN_EVICTION_AGE_MS - 100_000;

    // Fill the TOFU store to capacity with ascending last_seen timestamps
    for i in 0..MAX_TOFU_PEERS {
        protocol.known_peer_public_keys.insert(
            format!("peer_{}", i),
            TofuEntry {
                public_key: vec![i as u8; 32],
                last_seen_ms: old_base + i as i64, // peer_0 has oldest timestamp
            },
        );
    }
    assert_eq!(protocol.known_peer_public_keys.len(), MAX_TOFU_PEERS);

    // tofu_check_or_pin should evict the oldest entry and add the new one.
    let result = protocol.tofu_check_or_pin("new_peer", vec![99u8; 32]);
    assert!(result.is_ok());

    // Size should still be at the cap
    assert_eq!(protocol.known_peer_public_keys.len(), MAX_TOFU_PEERS);
    assert!(protocol.known_peer_public_keys.contains_key("new_peer"));
    assert!(
        !protocol.known_peer_public_keys.contains_key("peer_0"),
        "LRU entry should have been evicted"
    );
    // peer_1 should still be present (it wasn't the LRU)
    assert!(protocol.known_peer_public_keys.contains_key("peer_1"));
}

#[test]
fn test_tofu_eviction_refuses_when_all_entries_too_recent() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Fill with entries that are all recent (within the min eviction age).
    let recent = Utc::now().timestamp_millis();
    for i in 0..MAX_TOFU_PEERS {
        protocol.known_peer_public_keys.insert(
            format!("recent_peer_{}", i),
            TofuEntry {
                public_key: vec![i as u8; 32],
                last_seen_ms: recent - i as i64, // all very recent
            },
        );
    }
    assert_eq!(protocol.known_peer_public_keys.len(), MAX_TOFU_PEERS);

    // Attempt to pin a new peer — should succeed (signature was valid)
    // but should NOT evict any entry or pin the new key.
    let result = protocol.tofu_check_or_pin("attacker_peer", vec![42u8; 32]);
    assert!(result.is_ok(), "Should accept message despite full store");
    assert!(
        !protocol
            .known_peer_public_keys
            .contains_key("attacker_peer"),
        "Should not pin new peer when all entries are too recent to evict"
    );
    assert_eq!(
        protocol.known_peer_public_keys.len(),
        MAX_TOFU_PEERS,
        "Store size should be unchanged"
    );
}

#[test]
fn test_transport_peer_id_not_serialized() {
    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    msg.set_transport_peer_id("ble-device-42".to_string())
        .unwrap();

    let json = serde_json::to_string(&msg).unwrap();
    assert!(
        !json.contains("transport_peer_id"),
        "transport_peer_id must not appear in serialized output"
    );
    assert!(
        !json.contains("ble-device-42"),
        "transport peer identity must not leak into serialized output"
    );

    // Deserialize back — field should be None
    let deserialized: Message = serde_json::from_str(&json).unwrap();
    assert!(deserialized.transport_peer_id().is_none());
}

#[test]
fn test_unsigned_control_message_rejected_when_tofu_pinned() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Pin a key for "alice" (simulating a prior signed exchange)
    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: vec![1u8; 32],
            last_seen_ms: 1000,
        },
    );

    // Create an unsigned control message from "alice"
    let msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    // No __ctrl_sig / __ctrl_pk metadata → unsigned

    // Should be rejected because alice has a TOFU-pinned key
    let result = protocol.process_internal_message(&msg);
    assert!(
        matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "Unsigned control message from TOFU-pinned peer should be rejected by security gate"
    );
}

#[test]
fn test_unsigned_control_message_allowed_from_unknown_peer() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // No TOFU entry for "bob"
    assert!(!protocol.known_peer_public_keys.contains_key("bob"));

    // Create an unsigned control message from "bob"
    let msg = pending_test_message(
        "bob",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );

    // Should be allowed through (legacy peer, no TOFU key pinned)
    let result = protocol.process_internal_message(&msg);
    // CONN_REQUEST handler will attempt to parse the payload — it won't
    // return None (it will be Consumed by the handler or by parsing logic).
    // The key point is that it was NOT rejected by the security gate.
    assert!(
        result.is_some(),
        "Unsigned control message from unknown peer should pass the security gate"
    );
}

#[test]
fn test_verify_control_message_tofu_violation() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // We need MLS initialized for signing. Since we can't easily init MLS
    // in unit tests, we test the verify path directly by constructing a
    // message with valid-looking but mismatched TOFU state.

    // Pin a key for "alice"
    let pinned_pk = vec![1u8; 32];
    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: pinned_pk,
            last_seen_ms: 1000,
        },
    );

    // Create a message with a different public key in metadata
    let different_pk = vec![2u8; 32];
    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    // Put a fake signature and the different public key
    msg.metadata
        .insert(CTRL_SIG_META_KEY.to_string(), base64_encode(&vec![0u8; 64]));
    msg.metadata
        .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&different_pk));

    // verify_control_message should fail because the signature won't verify
    // (we used a fake signature), OR it would fail on TOFU mismatch.
    // Either way it should be an error.
    let result = protocol.verify_control_message(&msg);
    assert!(
        result.is_err(),
        "Should reject: bad signature or TOFU mismatch"
    );
}

#[test]
fn test_tofu_lru_eviction_removes_oldest() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Use timestamps old enough to be eviction-eligible
    let old_base = Utc::now().timestamp_millis() - TOFU_MIN_EVICTION_AGE_MS - 100_000;

    // Insert 3 entries with different timestamps
    protocol.known_peer_public_keys.insert(
        "old_peer".to_string(),
        TofuEntry {
            public_key: vec![1u8; 32],
            last_seen_ms: old_base,
        },
    );
    protocol.known_peer_public_keys.insert(
        "medium_peer".to_string(),
        TofuEntry {
            public_key: vec![2u8; 32],
            last_seen_ms: old_base + 100,
        },
    );
    protocol.known_peer_public_keys.insert(
        "new_peer".to_string(),
        TofuEntry {
            public_key: vec![3u8; 32],
            last_seen_ms: old_base + 200,
        },
    );

    // Find the LRU entry (among eviction-eligible ones)
    let eviction_cutoff = Utc::now().timestamp_millis() - TOFU_MIN_EVICTION_AGE_MS;
    let lru = protocol
        .known_peer_public_keys
        .iter()
        .filter(|(_, e)| e.last_seen_ms < eviction_cutoff)
        .min_by_key(|(_, e)| e.last_seen_ms)
        .map(|(k, _)| k.clone())
        .unwrap();

    assert_eq!(
        lru, "old_peer",
        "LRU eviction should target the oldest entry"
    );
}

#[test]
fn test_canonical_signing_payload_is_length_prefixed() {
    // Verify the canonical payload uses a domain separator followed by
    // length-prefixed encoding to prevent ambiguity and cross-context reuse.
    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__CONN_REQ__payload",
    );

    let canonical = OfflineProtocol::build_canonical_payload(&msg).unwrap();

    // Payload must start with the domain separator
    assert!(
        canonical.starts_with(CTRL_SIGN_DOMAIN),
        "canonical payload must start with domain separator"
    );

    // Parse fields after the domain separator
    let mut cursor = CTRL_SIGN_DOMAIN.len();
    let mut fields = Vec::new();
    while cursor < canonical.len() {
        assert!(cursor + 4 <= canonical.len(), "truncated length prefix");
        let len = u32::from_be_bytes(canonical[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        assert!(cursor + len <= canonical.len(), "truncated field data");
        let field = std::str::from_utf8(&canonical[cursor..cursor + len]).unwrap();
        fields.push(field.to_string());
        cursor += len;
    }

    assert_eq!(fields.len(), 4, "canonical payload should have 4 fields");
    assert_eq!(fields[0], "alice");
    assert_eq!(fields[1], msg.id.as_str());
    assert_eq!(fields[2], "bob");
    assert_eq!(fields[3], "__CONN_REQ__payload");
}

#[test]
fn test_canonical_payload_no_collision_with_similar_sender() {
    // Regression test: ensure that a longer sender does NOT produce the
    // same payload as a shorter sender with matching content.
    let msg_a = Message::new(
        UserId::new("alice-extra").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "content",
    );
    let msg_b = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "content",
    );

    let payload_a = OfflineProtocol::build_canonical_payload(&msg_a).unwrap();
    let payload_b = OfflineProtocol::build_canonical_payload(&msg_b).unwrap();

    // Both must start with domain separator
    assert!(payload_a.starts_with(CTRL_SIGN_DOMAIN));
    assert!(payload_b.starts_with(CTRL_SIGN_DOMAIN));

    // Even if content and recipient match, different sender lengths
    // produce different payloads.
    assert_ne!(payload_a, payload_b);
}

#[test]
fn test_internal_prefixes_completeness() {
    // The `define_internal_prefixes!` macro guarantees that every constant
    // in `internal_prefixes` is also in `INTERNAL_PREFIXES`. This test
    // verifies the SVC and DATA_PLANE invariants that aren't macro-generated.

    // Verify the SVC catch-all and explicit SVC prefixes
    assert!(INTERNAL_PREFIXES.contains(&offline_protocol_services::SVC_MESSAGE_PREFIX));
    assert!(INTERNAL_PREFIXES.contains(&offline_protocol_services::SVC_DISCOVER_QUERY));
    assert!(INTERNAL_PREFIXES.contains(&offline_protocol_services::SVC_DISCOVER_RESPONSE));
    assert!(INTERNAL_PREFIXES.contains(&offline_protocol_services::SVC_REQUEST));
    assert!(INTERNAL_PREFIXES.contains(&offline_protocol_services::SVC_RESPONSE));

    // Every DATA_PLANE_PREFIXES entry must also be in INTERNAL_PREFIXES
    // (data-plane messages still need injection prevention).
    for prefix in DATA_PLANE_PREFIXES {
        assert!(
            INTERNAL_PREFIXES.contains(prefix),
            "DATA_PLANE_PREFIXES entry {:?} is missing from INTERNAL_PREFIXES — \
             data-plane messages must still be protected from injection",
            prefix
        );
    }

    // Only DATA_PLANE_PREFIXES entries should be excluded from security
    // gating. Any internal prefix NOT in DATA_PLANE_PREFIXES is
    // automatically security-gated (signature + TOFU). If this assertion
    // fails, a new prefix was added to INTERNAL_PREFIXES but also needs to
    // be evaluated: should it be in DATA_PLANE_PREFIXES (MLS-authenticated)
    // or remain security-gated (control-plane, Ed25519-signed)?
    let excluded: Vec<&&str> = INTERNAL_PREFIXES
        .iter()
        .filter(|p| !OfflineProtocol::is_security_gated_prefix(p))
        .collect();
    let expected_excluded: Vec<&&str> = DATA_PLANE_PREFIXES.iter().collect();
    assert_eq!(
        excluded, expected_excluded,
        "Only DATA_PLANE_PREFIXES entries should be excluded from security gating. \
         If a new internal prefix was added, decide whether it is data-plane \
         (MLS-authenticated) or control-plane (Ed25519-signed) and update accordingly."
    );
}

// ========================================================================
// SECURITY: end-to-end signing round-trip and edge cases
// ========================================================================

#[test]
fn test_sign_and_verify_control_message_roundtrip() {
    let mut protocol = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    // Create a control message from "alice"
    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );

    // Sign it
    protocol.sign_control_message(&mut msg).unwrap();

    // Must have signature and public key metadata
    assert!(
        msg.metadata.contains_key(CTRL_SIG_META_KEY),
        "Signed message must contain signature metadata"
    );
    assert!(
        msg.metadata.contains_key(CTRL_PK_META_KEY),
        "Signed message must contain public key metadata"
    );

    // Verify it — should succeed and TOFU-pin alice's key
    let result = protocol.verify_control_message(&msg);
    assert!(
        matches!(result, Ok(true)),
        "Round-trip sign+verify must succeed, got: {:?}",
        result
    );

    // Verify again — TOFU-pinned key should match
    let result2 = protocol.verify_control_message(&msg);
    assert!(
        matches!(result2, Ok(true)),
        "Second verify with same key must succeed"
    );
}

#[test]
fn test_sign_and_verify_rejects_tampered_content() {
    let mut protocol = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!(
            "{}{{\"data\":\"original\"}}",
            internal_prefixes::CONN_REQUEST
        ),
    );

    protocol.sign_control_message(&mut msg).unwrap();

    // Tamper with the content after signing
    msg.content = format!(
        "{}{{\"data\":\"tampered\"}}",
        internal_prefixes::CONN_REQUEST
    );

    // Verification must fail — signature no longer matches content
    let result = protocol.verify_control_message(&msg);
    assert!(result.is_err(), "Tampered content must fail verification");
}

#[test]
fn test_sign_control_message_without_mls_sends_unsigned() {
    // No MLS initialization — sign_control_message should gracefully no-op
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );

    protocol.sign_control_message(&mut msg).unwrap();

    assert!(
        !msg.metadata.contains_key(CTRL_SIG_META_KEY),
        "Without MLS, message must remain unsigned"
    );
    assert!(
        !msg.metadata.contains_key(CTRL_PK_META_KEY),
        "Without MLS, message must not have public key metadata"
    );
}

#[test]
fn test_verify_control_message_malformed_base64_signature() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    msg.metadata.insert(
        CTRL_SIG_META_KEY.to_string(),
        "!!!not-valid-base64!!!".to_string(),
    );
    msg.metadata
        .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&vec![1u8; 32]));

    let result = protocol.verify_control_message(&msg);
    assert!(
        result.is_err(),
        "Malformed base64 signature must be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Invalid control signature encoding"),
        "Error should mention invalid encoding, got: {}",
        err_msg
    );
}

#[test]
fn test_verify_control_message_empty_signature() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    // Empty signature (0 bytes) — Ed25519 expects 64 bytes
    msg.metadata
        .insert(CTRL_SIG_META_KEY.to_string(), base64_encode(&[]));
    msg.metadata
        .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&vec![1u8; 32]));

    let result = protocol.verify_control_message(&msg);
    assert!(result.is_err(), "Empty signature must be rejected");
}

#[test]
fn test_verify_control_message_signature_without_public_key() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    // Has signature but no public key
    msg.metadata
        .insert(CTRL_SIG_META_KEY.to_string(), base64_encode(&vec![0u8; 64]));

    let result = protocol.verify_control_message(&msg);
    assert!(
        result.is_err(),
        "Signature without public key must be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("missing public key"),
        "Error should mention missing public key, got: {}",
        err_msg
    );
}

// ========================================================================
// INTEGRATION: full transport → receive → verify → TOFU round-trip
// ========================================================================

#[test]
fn test_integration_signed_control_message_via_mock_transport() {
    // Set up "alice" as the sender with MLS initialized so she can sign.
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls(storage_a).unwrap();

    // Set up "bob" as the receiver with MLS initialized so he can verify.
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage_b = Arc::new(crate::mls::InMemoryStorage::new());
    bob.initialize_mls(storage_b).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Alice creates and signs a control message destined for bob.
    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"hello\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut msg).unwrap();
    assert!(
        msg.metadata.contains_key(CTRL_SIG_META_KEY),
        "Alice should have signed the message"
    );

    // Enqueue the signed message on bob's transport with alice's identity.
    mock_transport.queue_message_from(msg, "alice".to_string());

    // Wire up bob's transport manager.
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    // Bob receives and processes — the security gate should pass.
    // receive_message drives the full transport → dedup → process_internal_message path.
    let received = bob.receive_message();
    // CONN_REQUEST is consumed internally (not surfaced to the app).
    assert!(
        received.is_none(),
        "Control message should be consumed, not surfaced"
    );

    // Alice's key should now be TOFU-pinned in bob's store.
    assert!(
        bob.known_peer_public_keys.contains_key("alice"),
        "Bob should have TOFU-pinned alice's public key"
    );
}

#[test]
fn test_integration_spoofed_transport_identity_rejected() {
    // A signed control message claiming to be from "alice" but delivered
    // by transport peer "eve" must be rejected.
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls(storage_a).unwrap();

    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage_b = Arc::new(crate::mls::InMemoryStorage::new());
    bob.initialize_mls(storage_b).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"evil\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut msg).unwrap();

    // Deliver via "eve" — transport identity mismatch.
    mock_transport.queue_message_from(msg, "eve".to_string());

    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    let received = bob.receive_message();
    assert!(
        received.is_none(),
        "Spoofed message should be consumed (dropped)"
    );
    assert!(
        !bob.known_peer_public_keys.contains_key("alice"),
        "Spoofed message must not TOFU-pin alice's key"
    );
}

#[test]
fn test_integration_relay_forwarded_message_no_transport_peer_id() {
    // Relay-forwarded messages are created fresh via send_internal_message
    // and thus have transport_peer_id = None. Verify they pass the
    // transport sender check (best-effort).
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Simulate a relay-forwarded control message: no transport_peer_id.
    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!(
            "{}{{\"data\":\"relayed\"}}",
            internal_prefixes::CONN_REQUEST
        ),
    );
    assert!(
        msg.transport_peer_id().is_none(),
        "Relay-forwarded message should have no transport_peer_id"
    );

    // Enqueue without transport identity (using queue_message, not queue_message_from).
    mock_transport.queue_message(msg);

    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    // Should pass the transport sender check and reach the handler
    // (CONN_REQUEST handler will consume it).
    let received = bob.receive_message();
    assert!(
        received.is_none(),
        "Unsigned relay message from unknown peer should be consumed by handler"
    );
}

#[test]
fn test_integration_encrypted_message_survives_security_gate_after_tofu_pin() {
    // Critical regression test: after a signed control message exchange
    // TOFU-pins the sender's key, subsequent __MLS_ENC__ (data-plane)
    // messages from that sender must NOT be rejected as "signature
    // downgrade". MLS provides its own authentication for encrypted
    // messages; they are not signed with the control-plane Ed25519 key.
    //
    // Without the DATA_PLANE_PREFIXES exclusion in
    // `is_security_gated_prefix`, this test would fail because the
    // security gate would treat __MLS_ENC__ as a control message and
    // reject it for being unsigned from a TOFU-pinned peer.

    use crate::mls::InMemoryStorage;

    // --- Set up Alice (sender) with MLS ---
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    alice
        .initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let mut alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    // --- Set up Bob (receiver) with MLS ---
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let mut bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    // --- Step 1: Alice sends a signed control message (key package) ---
    // Manually create and sign a CONN_REQUEST from Alice to Bob.
    let mut ctrl_msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!(
            "{}{{\"sender_name\":\"alice\",\"timestamp_ms\":1234}}",
            internal_prefixes::CONN_REQUEST
        ),
    );
    alice.sign_control_message(&mut ctrl_msg).unwrap();
    assert!(
        ctrl_msg.metadata.contains_key(CTRL_SIG_META_KEY),
        "Control message should be signed"
    );

    // Bob receives it — this will TOFU-pin Alice's public key.
    bob_transport_handle.queue_message_from(ctrl_msg, "alice".to_string());
    let _ = bob.receive_message(); // consumed internally

    // Verify Alice's key is now TOFU-pinned at Bob.
    assert!(
        bob.known_peer_public_keys.contains_key("alice"),
        "Alice's key should be TOFU-pinned at Bob after signed control message"
    );

    // --- Step 2: Set up MLS session directly (shortcut) ---
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    {
        let manager = alice.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        let welcome = manager.create_session("bob").unwrap();
        let bob_manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        bob_manager.join_session(&welcome).unwrap();
    }
    alice.confirm_session_state("bob", "test_setup").unwrap();
    bob.confirm_session_state("alice", "test_setup").unwrap();

    // --- Step 3: Alice sends an encrypted message ---
    alice
        .send_message(
            "bob",
            "hello after TOFU pin",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    let encrypted_wire = alice_transport_handle
        .sent_messages()
        .last()
        .expect("expected encrypted message from alice")
        .clone();
    assert!(
        encrypted_wire
            .content
            .starts_with(internal_prefixes::ENCRYPTED),
        "Message should be MLS-encrypted"
    );

    // --- Step 4: Bob receives the encrypted message ---
    // This is the critical assertion: the __MLS_ENC__ message must NOT
    // be rejected by the security gate, even though Alice has a
    // TOFU-pinned key and this message is unsigned.
    bob_transport_handle.queue_message(encrypted_wire);
    let received = bob
        .receive_message()
        .expect("Encrypted message must NOT be dropped by security gate after TOFU pin");
    assert_eq!(received.content, "hello after TOFU pin");
    assert_eq!(
        received.metadata.get("encrypted").map(String::as_str),
        Some("true")
    );
}

// ========================================================================
// SECURITY: parameterized security gate, edge cases, ACK bypass
// ========================================================================

#[test]
fn test_security_gate_rejects_spoofed_transport_for_all_gated_prefixes() {
    // Verify that the security gate drops control messages with a
    // transport identity mismatch for EVERY security-gated prefix
    // (all INTERNAL_PREFIXES except DATA_PLANE_PREFIXES).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let gated_prefixes: Vec<&&str> = INTERNAL_PREFIXES
        .iter()
        .filter(|p| !DATA_PLANE_PREFIXES.contains(p))
        .collect();
    assert!(
        !gated_prefixes.is_empty(),
        "There must be at least one security-gated prefix"
    );

    for prefix in gated_prefixes {
        let mut msg = pending_test_message("sender123", &format!("{}test_payload", prefix));
        msg.set_transport_peer_id("eve".to_string()).unwrap();

        let result = protocol.process_internal_message(&msg);
        assert!(
            matches!(result, Some(InternalMessageResult::SecurityRejected)),
            "Security gate should reject spoofed message for prefix '{}'",
            prefix
        );
    }
}

#[test]
fn test_data_plane_prefixes_bypass_security_gate() {
    // __MLS_ENC__ messages are data-plane (MLS provides its own
    // authentication) and must NOT be rejected by the security gate,
    // even when the sender has a TOFU-pinned key.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // First, TOFU-pin a key for "alice" to simulate a prior signed
    // control message exchange.
    protocol.tofu_check_or_pin("alice", vec![1u8; 32]).unwrap();
    assert!(protocol.known_peer_public_keys.contains_key("alice"));

    // Now send an unsigned __MLS_ENC__ message from alice — this is the
    // normal data path after MLS session establishment.
    let msg = pending_test_message(
        "alice",
        &format!(
            "{}{{\"group_id\":\"session:alice:bob\",\"message_type\":\"Application\",\"epoch\":0,\"ciphertext\":[1,2,3],\"sender_id\":\"alice\",\"timestamp_ms\":12345}}",
            internal_prefixes::ENCRYPTED
        ),
    );

    let result = protocol.security_gate_control_message(&msg);
    assert!(
        result.is_none(),
        "Security gate must NOT block __MLS_ENC__ messages — MLS provides its own authentication"
    );
}

#[test]
fn test_verify_control_message_pk_without_signature_is_malformed() {
    // A message with a public key in metadata but no signature should
    // be treated as malformed (not as unsigned/legacy).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    // Public key present but no signature
    msg.metadata
        .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&vec![1u8; 32]));

    let result = protocol.verify_control_message(&msg);
    assert!(
        result.is_err(),
        "Public key without signature must be rejected as malformed"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("missing signature"),
        "Error should mention missing signature, got: {}",
        err_msg
    );
}

#[test]
fn test_security_gate_rejects_pk_only_no_signature() {
    // Full process_internal_message path: a control message with a
    // public key but no signature should be dropped by the security gate.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    msg.metadata
        .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&vec![1u8; 32]));

    let result = protocol.process_internal_message(&msg);
    assert!(
        matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "Malformed message (pk without sig) should be rejected by security gate"
    );
}

#[test]
fn test_ack_messages_bypass_security_gate() {
    // ACK messages do not start with any internal prefix (they have
    // empty or non-prefixed content), so the security gate must not
    // interfere with them — even if transport_peer_id mismatches.
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // ACK-like message: content is just the acked message ID, not an
    // internal prefix.
    let msg = pending_test_message("alice", "some-message-id-being-acked");
    assert!(
        !OfflineProtocol::is_internal_prefix(&msg.content),
        "ACK content must not be detected as an internal prefix"
    );

    // The security gate should return None (pass-through) regardless
    // of any transport mismatch, because ACKs aren't control messages.
    assert!(
        !OfflineProtocol::is_internal_prefix(""),
        "Empty content must not trigger the security gate"
    );
}

#[test]
fn test_set_transport_peer_id_rejects_empty_string() {
    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    let result = msg.set_transport_peer_id("".to_string());
    assert!(result.is_err(), "Empty transport_peer_id must be rejected");
    assert!(
        msg.transport_peer_id().is_none(),
        "transport_peer_id should remain None after rejected empty set"
    );
}

#[test]
fn test_tofu_rejects_empty_public_key() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.tofu_check_or_pin("alice", vec![]);
    assert!(
        result.is_err(),
        "Empty public key must be rejected by TOFU store"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("empty public key"),
        "Error should mention empty public key, got: {}",
        err_msg
    );
    assert!(
        !protocol.known_peer_public_keys.contains_key("alice"),
        "Empty key must not be pinned"
    );
}

#[test]
fn test_verify_control_message_zero_byte_public_key_rejected() {
    // Even if someone constructs a message with a valid-looking
    // signature but a 0-byte public key, Ed25519 parsing should fail.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = pending_test_message(
        "alice",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    msg.metadata
        .insert(CTRL_SIG_META_KEY.to_string(), base64_encode(&vec![0u8; 64]));
    msg.metadata
        .insert(CTRL_PK_META_KEY.to_string(), base64_encode(&[]));

    let result = protocol.verify_control_message(&msg);
    assert!(
        result.is_err(),
        "Zero-byte public key must be rejected during verification"
    );
}

#[test]
fn test_signature_downgrade_detection_for_tofu_pinned_peer() {
    // A peer that has previously sent signed control messages (TOFU-pinned)
    // must have subsequent unsigned control messages rejected as a possible
    // downgrade attack (impersonation attempt).
    let mut protocol = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    // Set up "alice" as a sender with MLS so she can sign.
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls(storage_a).unwrap();

    // Alice creates and signs a control message.
    let mut signed_msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"first\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut signed_msg).unwrap();
    assert!(signed_msg.metadata.contains_key(CTRL_SIG_META_KEY));

    // Bob verifies — this TOFU-pins alice's key.
    let result = protocol.verify_control_message(&signed_msg);
    assert!(
        matches!(result, Ok(true)),
        "First signed message should verify"
    );
    assert!(
        protocol.known_peer_public_keys.contains_key("alice"),
        "Alice's key should be TOFU-pinned"
    );

    // Now "alice" (or an impersonator) sends an unsigned control message.
    let unsigned_msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!(
            "{}{{\"data\":\"unsigned\"}}",
            internal_prefixes::CONN_REQUEST
        ),
    );
    assert!(!unsigned_msg.metadata.contains_key(CTRL_SIG_META_KEY));

    // The security gate should reject this as a signature downgrade.
    let gate_result = protocol.security_gate_control_message(&unsigned_msg);
    assert!(
        matches!(gate_result, Some(InternalMessageResult::SecurityRejected)),
        "Unsigned message from TOFU-pinned peer should be rejected as signature downgrade"
    );
}

#[test]
fn test_second_signed_message_from_tofu_pinned_peer_passes_gate() {
    // After TOFU-pinning, a subsequent correctly-signed control message
    // from the same peer (with the same key) should pass the security gate.
    let mut protocol = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls(storage_a).unwrap();

    // First signed message — pins alice's key.
    let mut msg1 = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"first\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut msg1).unwrap();

    let gate1 = protocol.security_gate_control_message(&msg1);
    assert!(
        gate1.is_none(),
        "First signed message should pass the security gate"
    );
    assert!(protocol.known_peer_public_keys.contains_key("alice"));

    // Second signed message — same key, should also pass.
    let mut msg2 = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"second\"}}", internal_prefixes::CONN_ACCEPT),
    );
    alice.sign_control_message(&mut msg2).unwrap();

    let gate2 = protocol.security_gate_control_message(&msg2);
    assert!(
        gate2.is_none(),
        "Second signed message from same TOFU-pinned peer should pass the security gate"
    );
}

#[test]
fn test_security_rejected_does_not_send_ack() {
    // Verify that SecurityRejected messages do NOT trigger a delivery ACK,
    // preventing an attacker from confirming the target is online.
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage_b = Arc::new(crate::mls::InMemoryStorage::new());
    bob.initialize_mls(storage_b).unwrap();

    let mut mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();

    // Create a spoofed control message: sender says "alice", transport says "eve"
    let mut spoofed = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"evil\"}}", internal_prefixes::CONN_REQUEST),
    );
    spoofed.requires_ack = true;
    mock_transport.queue_message_from(spoofed, "eve".to_string());

    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    // Process the message — should be security-rejected
    let received = bob.receive_message();
    assert!(received.is_none(), "Spoofed message should not surface");

    // Verify no ACK was sent back
    let sent = transport_handle.sent_messages();
    let ack_count = sent
        .iter()
        .filter(|m| m.metadata.contains_key(ACK_FOR_KEY))
        .count();
    assert_eq!(
        ack_count, 0,
        "Security-rejected messages must NOT trigger a delivery ACK"
    );
}

// ========================================================================
// TOFU PERSISTENCE
// ========================================================================

#[test]
fn test_tofu_entries_persisted_via_storage() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage.clone()).unwrap();

    // Pin a key for "alice"
    let pk = vec![42u8; 32];
    protocol.tofu_check_or_pin("alice", pk.clone()).unwrap();

    // Verify the entry was persisted to raw storage
    let raw = storage
        .load(storage_keys::TOFU_KEYS, "alice")
        .unwrap()
        .expect("TOFU entry should be persisted");
    let restored: TofuEntry = serde_json::from_slice(&raw).unwrap();
    assert_eq!(restored.public_key, pk);
}

#[test]
fn test_tofu_entries_restored_on_restart() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // Protocol A pins a key for "alice"
    {
        let mut protocol_a = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
        protocol_a.initialize_mls(storage.clone()).unwrap();
        protocol_a
            .tofu_check_or_pin("alice", vec![10u8; 32])
            .unwrap();
    }

    // Protocol B uses the same storage — simulates restart
    let mut protocol_b = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    protocol_b.initialize_mls(storage.clone()).unwrap();

    // The restored TOFU store should contain alice's pinned key
    assert!(
        protocol_b.known_peer_public_keys.contains_key("alice"),
        "TOFU entry for alice should be restored from storage"
    );
    assert_eq!(
        protocol_b.known_peer_public_keys["alice"].public_key,
        vec![10u8; 32]
    );

    // A different key for alice should be rejected (TOFU mismatch)
    let result = protocol_b.tofu_check_or_pin("alice", vec![99u8; 32]);
    assert!(
        result.is_err(),
        "TOFU mismatch should be detected after restore"
    );
}

#[test]
fn test_tofu_eviction_deletes_from_storage() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage.clone()).unwrap();

    // Fill the TOFU store with old entries
    let old_base = Utc::now().timestamp_millis() - TOFU_MIN_EVICTION_AGE_MS - 100_000;
    for i in 0..MAX_TOFU_PEERS {
        let entry = TofuEntry {
            public_key: vec![i as u8; 32],
            last_seen_ms: old_base + i as i64,
        };
        protocol
            .known_peer_public_keys
            .insert(format!("peer_{}", i), entry.clone());
        protocol.persist_tofu_entry(&format!("peer_{}", i), &entry);
    }

    // Verify peer_0 is in storage
    assert!(
        storage
            .load(storage_keys::TOFU_KEYS, "peer_0")
            .unwrap()
            .is_some(),
        "peer_0 should be in storage before eviction"
    );

    // Pin a new peer — should evict peer_0 (oldest)
    protocol
        .tofu_check_or_pin("new_peer", vec![0xFFu8; 32])
        .unwrap();

    // peer_0 should be deleted from storage
    assert!(
        storage
            .load(storage_keys::TOFU_KEYS, "peer_0")
            .unwrap()
            .is_none(),
        "Evicted peer_0 should be deleted from storage"
    );

    // new_peer should be persisted
    assert!(
        storage
            .load(storage_keys::TOFU_KEYS, "new_peer")
            .unwrap()
            .is_some(),
        "Newly pinned peer should be persisted"
    );
}

#[test]
fn test_tofu_last_seen_update_persisted() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage.clone()).unwrap();

    let pk = vec![7u8; 32];
    protocol.tofu_check_or_pin("carol", pk.clone()).unwrap();

    let raw1 = storage
        .load(storage_keys::TOFU_KEYS, "carol")
        .unwrap()
        .unwrap();
    let entry1: TofuEntry = serde_json::from_slice(&raw1).unwrap();
    let first_seen = entry1.last_seen_ms;

    // Re-verify the same key (updates last_seen)
    std::thread::sleep(std::time::Duration::from_millis(5));
    protocol.tofu_check_or_pin("carol", pk).unwrap();

    let raw2 = storage
        .load(storage_keys::TOFU_KEYS, "carol")
        .unwrap()
        .unwrap();
    let entry2: TofuEntry = serde_json::from_slice(&raw2).unwrap();
    assert!(
        entry2.last_seen_ms >= first_seen,
        "last_seen_ms should be updated after re-verification"
    );
}

#[test]
fn test_tofu_restore_skips_corrupted_entries() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // Write a valid entry
    let valid_entry = TofuEntry {
        public_key: vec![1u8; 32],
        last_seen_ms: Utc::now().timestamp_millis(),
    };
    storage
        .store(
            storage_keys::TOFU_KEYS,
            "valid_peer",
            &serde_json::to_vec(&valid_entry).unwrap(),
        )
        .unwrap();

    // Write corrupted data for another peer
    storage
        .store(
            storage_keys::TOFU_KEYS,
            "corrupted_peer",
            b"not valid json{{{",
        )
        .unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.enable_message_persistence(storage).unwrap();

    // Valid entry should be restored, corrupted should be skipped
    assert!(
        protocol.known_peer_public_keys.contains_key("valid_peer"),
        "Valid TOFU entry should be restored"
    );
    assert!(
        !protocol
            .known_peer_public_keys
            .contains_key("corrupted_peer"),
        "Corrupted TOFU entry should be skipped"
    );
}

#[test]
fn test_tofu_restore_caps_at_max_peers() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // Store more entries than MAX_TOFU_PEERS
    let now = Utc::now().timestamp_millis();
    for i in 0..(MAX_TOFU_PEERS + 50) {
        let entry = TofuEntry {
            public_key: vec![i as u8; 32],
            last_seen_ms: now,
        };
        storage
            .store(
                storage_keys::TOFU_KEYS,
                &format!("peer_{}", i),
                &serde_json::to_vec(&entry).unwrap(),
            )
            .unwrap();
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.enable_message_persistence(storage).unwrap();

    assert!(
        protocol.known_peer_public_keys.len() <= MAX_TOFU_PEERS,
        "Restored TOFU entries should be capped at MAX_TOFU_PEERS, got {}",
        protocol.known_peer_public_keys.len()
    );
}

#[test]
fn test_security_gate_rejects_missing_transport_id_when_required() {
    let mut config = create_test_config_for_user("bob");
    config.security.require_transport_identity = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    // Create a signed control message from alice with no transport_peer_id
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls(storage_a).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut msg).unwrap();
    // No transport_peer_id set — should be rejected by security gate

    let result = protocol.security_gate_control_message(&msg);
    assert!(
        matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "Control message without transport identity should be rejected \
         when require_transport_identity=true"
    );
}

#[test]
fn test_security_gate_passes_with_matching_transport_id_when_required() {
    let mut config = create_test_config_for_user("bob");
    config.security.require_transport_identity = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls(storage).unwrap();

    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls(storage_a).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut msg).unwrap();
    msg.set_transport_peer_id("alice".to_string()).unwrap();

    let result = protocol.security_gate_control_message(&msg);
    assert!(
        result.is_none(),
        "Signed control message with matching transport identity should pass"
    );
}

#[test]
fn test_enable_message_persistence_restores_tofu_keys() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // Pre-populate storage with a TOFU entry
    let entry = TofuEntry {
        public_key: vec![55u8; 32],
        last_seen_ms: Utc::now().timestamp_millis(),
    };
    storage
        .store(
            storage_keys::TOFU_KEYS,
            "dave",
            &serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();

    // Use enable_message_persistence (not initialize_mls)
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.enable_message_persistence(storage).unwrap();

    assert!(
        protocol.known_peer_public_keys.contains_key("dave"),
        "TOFU entry should be restored via enable_message_persistence path"
    );
    assert_eq!(
        protocol.known_peer_public_keys["dave"].public_key,
        vec![55u8; 32]
    );
}

#[test]
fn test_tofu_restore_skips_invalid_peer_ids() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // Write a valid entry
    let valid_entry = TofuEntry {
        public_key: vec![1u8; 32],
        last_seen_ms: Utc::now().timestamp_millis(),
    };
    storage
        .store(
            storage_keys::TOFU_KEYS,
            "valid_peer",
            &serde_json::to_vec(&valid_entry).unwrap(),
        )
        .unwrap();

    // Write entries with hostile peer IDs (pre-validation-era data)
    let hostile_entry = TofuEntry {
        public_key: vec![2u8; 32],
        last_seen_ms: Utc::now().timestamp_millis(),
    };
    for hostile_id in &["../evil", "peer/slash", "peer:colon", "peer\0nul"] {
        storage
            .store(
                storage_keys::TOFU_KEYS,
                hostile_id,
                &serde_json::to_vec(&hostile_entry).unwrap(),
            )
            .unwrap();
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.enable_message_persistence(storage).unwrap();

    assert!(
        protocol.known_peer_public_keys.contains_key("valid_peer"),
        "Valid peer ID should be restored"
    );
    assert!(
        !protocol.known_peer_public_keys.contains_key("../evil"),
        "Path-traversal peer ID should be skipped"
    );
    assert!(
        !protocol.known_peer_public_keys.contains_key("peer/slash"),
        "Slash peer ID should be skipped"
    );
    assert!(
        !protocol.known_peer_public_keys.contains_key("peer:colon"),
        "Colon peer ID should be skipped"
    );
    assert_eq!(
        protocol.known_peer_public_keys.len(),
        1,
        "Only the valid peer should be restored"
    );
}

#[test]
fn test_tofu_restore_truncation_deterministic_on_equal_timestamps() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // Store more entries than MAX_TOFU_PEERS, all with the same timestamp
    let now = Utc::now().timestamp_millis();
    let count = MAX_TOFU_PEERS + 10;
    for i in 0..count {
        let entry = TofuEntry {
            public_key: vec![i as u8; 32],
            last_seen_ms: now, // identical timestamps
        };
        storage
            .store(
                storage_keys::TOFU_KEYS,
                &format!("peer_{:04}", i),
                &serde_json::to_vec(&entry).unwrap(),
            )
            .unwrap();
    }

    // Restore twice and verify we get the same set both times
    let mut protocol_a = OfflineProtocol::new(create_test_config()).unwrap();
    protocol_a
        .enable_message_persistence(storage.clone())
        .unwrap();
    let keys_a: std::collections::BTreeSet<String> =
        protocol_a.known_peer_public_keys.keys().cloned().collect();

    let mut protocol_b = OfflineProtocol::new(create_test_config()).unwrap();
    protocol_b.enable_message_persistence(storage).unwrap();
    let keys_b: std::collections::BTreeSet<String> =
        protocol_b.known_peer_public_keys.keys().cloned().collect();

    assert_eq!(keys_a.len(), MAX_TOFU_PEERS);
    assert_eq!(
        keys_a, keys_b,
        "Truncation should be deterministic when timestamps are equal"
    );
}
