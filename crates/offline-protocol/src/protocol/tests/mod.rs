use super::*;
use crate::constants::ACK_FOR_KEY;
use crate::events::{DecryptionFailureCode, PresenceSource, PresenceStatus, SecurityWarningCode};
use crate::mls_observability::{
    DecryptionFailureKind, MlsErrorCategory, MlsLifecycleEvent, MlsOperationContext,
};
use crate::telemetry::{
    MlsVerbosity, NoopTelemetrySink, TelemetryConfig, TelemetryRecord, TelemetrySink,
};
use chrono::Duration as ChronoDuration;
use offline_protocol_core::{AppId, ContentType, MessagePriority, ServiceDescriptor, UserId};
use offline_protocol_transport::{
    mock::MockTransport, Transport, TransportMetrics, TransportStatus, TransportType,
};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

pub(crate) fn create_test_config() -> ProtocolConfig {
    create_test_config_for_user("user123")
}

pub(crate) fn create_test_config_for_user(user_id: &str) -> ProtocolConfig {
    let mut config = ProtocolConfig::new("test-app", user_id);
    // Most tests exercise transport/reliability/routing machinery without
    // initializing MLS; opt out of the fail-closed default (SEC-M3) so their
    // sends take the legacy plaintext path. Tests asserting the strict
    // default behavior construct `ProtocolConfig::new` directly.
    config.encryption.require_encryption = false;
    config
}

#[derive(Default, Clone)]
struct RecordingMlsEmitter {
    events: Arc<Mutex<Vec<MlsLifecycleEvent>>>,
}

impl MlsEventEmitter for RecordingMlsEmitter {
    fn emit(&self, event: MlsLifecycleEvent) {
        self.events.lock().unwrap().push(event);
    }
}

impl RecordingMlsEmitter {
    fn take(&self) -> Vec<MlsLifecycleEvent> {
        let mut guard = self.events.lock().unwrap();
        std::mem::take(&mut *guard)
    }
}

/// Minimal [`TelemetrySink`] implementation that records every record it
/// receives. Cloning the sink clones the shared backing vector, so a caller
/// keeps a handle to the records after the sink is moved into `Arc`.
#[derive(Default, Clone)]
struct RecordingTelemetrySink {
    records: Arc<Mutex<Vec<TelemetryRecord>>>,
}

impl TelemetrySink for RecordingTelemetrySink {
    fn emit(&self, record: &TelemetryRecord) {
        self.records.lock().unwrap().push(record.clone());
    }
}

impl RecordingTelemetrySink {
    fn take(&self) -> Vec<TelemetryRecord> {
        let mut guard = self.records.lock().unwrap();
        std::mem::take(&mut *guard)
    }
}

/// Reads (or creates) the per-install protocol-state record key held in
/// `storage`, exactly as `restore_or_init_state_record_key` does, so a fixture
/// can pre-seed a sealed record before the protocol is constructed.
fn state_record_cipher_for(storage: &InMemoryStorage) -> state_crypto::StateRecordCipher {
    use state_crypto::STATE_RECORD_KEY_BYTES;

    let existing = storage
        .load(
            storage_keys::STATE_RECORD_KEY,
            storage_keys::STATE_RECORD_KEY_ID,
        )
        .unwrap();
    let key = match existing {
        Some(bytes) if bytes.len() == STATE_RECORD_KEY_BYTES => bytes,
        _ => {
            let fresh = state_crypto::StateRecordCipher::generate_key();
            storage
                .store(
                    storage_keys::STATE_RECORD_KEY,
                    storage_keys::STATE_RECORD_KEY_ID,
                    &*fresh,
                )
                .unwrap();
            fresh.to_vec()
        }
    };
    let mut key_bytes = [0u8; STATE_RECORD_KEY_BYTES];
    key_bytes.copy_from_slice(&key);
    state_crypto::StateRecordCipher::new(&key_bytes)
}

/// Seeds a record into a sealed protocol-state category the way the SDK writes
/// it, so restore paths can read it back.
fn seed_sealed_state_record(
    storage: &InMemoryStorage,
    key_type: &str,
    key_id: &str,
    plaintext: &[u8],
) {
    let sealed = state_record_cipher_for(storage)
        .seal(key_type, key_id, plaintext)
        .unwrap();
    storage.store(key_type, key_id, &sealed).unwrap();
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

    fn start(&self) -> offline_protocol_transport::Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Available;
        Ok(())
    }

    fn stop(&self) -> offline_protocol_transport::Result<()> {
        *self.status.lock().unwrap() = TransportStatus::Disconnected;
        Ok(())
    }

    fn on_status_changed(&self, status: TransportStatus) {
        *self.status.lock().unwrap() = status;
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

#[derive(Default)]
struct FailingScrubSecretStorage {
    inner: crate::mls::InMemoryStorage,
}

impl MlsStorage for FailingScrubSecretStorage {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        if key_type == storage_keys::SCRUB_SECRET {
            return Err(offline_protocol_mls::StorageError::StoreFailed(
                "forced scrub-secret persist failure".to_string(),
            ));
        }
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
        self.inner.list_keys(key_type)
    }
}

/// Fails `store` for the Nostr signing secret while `fail_store` is set,
/// delegating everything else to the in-memory storage.
#[derive(Default)]
struct FailingNostrSecretStorage {
    inner: crate::mls::InMemoryStorage,
    fail_store: std::sync::atomic::AtomicBool,
}

impl MlsStorage for FailingNostrSecretStorage {
    fn store(
        &self,
        key_type: &str,
        key_id: &str,
        data: &[u8],
    ) -> offline_protocol_mls::storage::StorageResult<()> {
        if key_type == storage_keys::NOSTR_SIGNING_SECRET
            && self.fail_store.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(offline_protocol_mls::StorageError::StoreFailed(
                "forced nostr-secret persist failure".to_string(),
            ));
        }
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
        self.inner.list_keys(key_type)
    }
}

#[test]
fn test_protocol_creation() {
    let protocol = OfflineProtocol::new(create_test_config());
    assert!(protocol.is_ok());
}

#[test]
fn test_secure_and_protocol_state_storage_are_routed_separately() {
    let mut config = ProtocolConfig::new("test-app", "alice");
    config.transport.internet_enabled = true;
    let secure = Arc::new(crate::mls::InMemoryStorage::new());
    let state = Arc::new(crate::mls::InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();

    protocol
        .initialize_mls(
            secure.clone(),
            Arc::new(TestProtocolStateStorage {
                storage: state.clone(),
            }),
        )
        .unwrap();
    protocol.start().unwrap();
    protocol
        .send_message("bob", "queued", None, None::<String>)
        .unwrap();
    let outbox_id = protocol
        .send_connection_request("carol", "Alice", None, None)
        .unwrap();

    assert!(secure
        .load(storage_keys::SCRUB_SECRET, storage_keys::SCRUB_SECRET_ID)
        .unwrap()
        .is_some());
    assert!(state
        .load(storage_keys::SCRUB_SECRET, storage_keys::SCRUB_SECRET_ID)
        .unwrap()
        .is_none());
    assert!(state
        .load(storage_keys::PENDING_MESSAGES, "bob")
        .unwrap()
        .is_some());
    assert!(secure
        .load(storage_keys::PENDING_MESSAGES, "bob")
        .unwrap()
        .is_none());
    assert!(state
        .load(storage_keys::OUTBOX, &outbox_id.as_str())
        .unwrap()
        .is_some());
    assert!(secure
        .load(storage_keys::OUTBOX, &outbox_id.as_str())
        .unwrap()
        .is_none());
}

#[test]
fn test_fresh_protocol_state_does_not_restore_pre_reinstall_pending_messages() {
    let mut config = ProtocolConfig::new("test-app", "alice");
    config.transport.internet_enabled = true;
    let secure = Arc::new(crate::mls::InMemoryStorage::new());
    let old_state = Arc::new(crate::mls::InMemoryStorage::new());

    {
        let mut before_reinstall = OfflineProtocol::new(config.clone()).unwrap();
        before_reinstall
            .initialize_mls(
                secure.clone(),
                Arc::new(TestProtocolStateStorage {
                    storage: old_state.clone(),
                }),
            )
            .unwrap();
        before_reinstall.start().unwrap();
        before_reinstall
            .send_message("bob", "stale", None, None::<String>)
            .unwrap();
        before_reinstall
            .send_connection_request("carol", "Alice", None, None)
            .unwrap();
        assert!(old_state
            .load(storage_keys::PENDING_MESSAGES, "bob")
            .unwrap()
            .is_some());
        assert!(!old_state
            .list_keys(storage_keys::OUTBOX)
            .unwrap()
            .is_empty());
    }

    let fresh_state = Arc::new(crate::mls::InMemoryStorage::new());
    let mut after_reinstall = OfflineProtocol::new(config).unwrap();
    after_reinstall
        .initialize_mls(
            secure,
            Arc::new(TestProtocolStateStorage {
                storage: fresh_state.clone(),
            }),
        )
        .unwrap();

    assert!(after_reinstall.pending_encrypted_messages.is_empty());
    assert!(after_reinstall.outbox.is_empty());
    assert!(fresh_state
        .list_keys(storage_keys::PENDING_MESSAGES)
        .unwrap()
        .is_empty());
    assert!(fresh_state
        .list_keys(storage_keys::OUTBOX)
        .unwrap()
        .is_empty());
}

#[test]
fn test_invalid_recipient_is_rejected_before_queue_or_clock_side_effects() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.start().unwrap();
    let clock_before = protocol.lamport_clock.value();

    let error = protocol
        .send_message("unresolved:token", "hello", None, None::<String>)
        .unwrap_err();

    assert!(matches!(error, Error::InvalidArgument(_)));
    assert!(protocol.pending_encrypted_messages.is_empty());
    assert!(protocol.outbox.is_empty());
    assert_eq!(protocol.lamport_clock.value(), clock_before);
}

#[test]
fn test_pending_session_messages_expire_and_are_deleted_from_state_storage() {
    let mut config = ProtocolConfig::new("test-app", "alice");
    config.transport.internet_enabled = true;
    config.reliability.retry.pending_message_max_lifetime_ms = 1;
    let secure = Arc::new(crate::mls::InMemoryStorage::new());
    let state = Arc::new(crate::mls::InMemoryStorage::new());
    let events = Arc::new(Mutex::new(Vec::new()));
    let event_sink = events.clone();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.on_event(move |event| event_sink.lock().unwrap().push(event));
    protocol
        .initialize_mls(
            secure,
            Arc::new(TestProtocolStateStorage {
                storage: state.clone(),
            }),
        )
        .unwrap();
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "queued", None, None::<String>)
        .unwrap();
    protocol.pending_encrypted_messages.get_mut("bob").unwrap()[0].queued_at =
        Utc::now() - ChronoDuration::milliseconds(2);

    protocol.cleanup_expired_pending_messages();

    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(state
        .load(storage_keys::PENDING_MESSAGES, "bob")
        .unwrap()
        .is_none());
    assert!(events.lock().unwrap().iter().any(|event| matches!(
        event,
        Event::MessageFailed {
            message_id: failed_id,
            reason,
            retry_count: 0,
        } if failed_id == &message_id.as_str()
            && reason == "Pending session lifetime exceeded"
    )));
}

#[test]
fn test_lifetime_expiry_handles_extreme_configured_bounds() {
    let now = Utc::now();

    assert!(!lifetime_expired(now, now, i64::MAX as u64));
    assert!(!lifetime_expired(
        now,
        DateTime::<Utc>::MIN_UTC,
        i64::MAX as u64
    ));
    assert!(!lifetime_expired(now, now, u64::MAX));
    assert!(lifetime_expired(
        now,
        now - ChronoDuration::milliseconds(2),
        1
    ));
}

#[test]
fn test_runtime_retry_and_ack_updates_reject_zero_delays() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let original_retry = protocol.config.reliability.retry.clone();
    let original_ack = protocol.config.reliability.ack.clone();

    let mut retry = original_retry.clone();
    retry.initial_delay_ms = 0;
    assert!(matches!(
        protocol.update_retry_config(retry),
        Err(Error::InvalidConfiguration(_))
    ));
    assert_eq!(
        protocol.config.reliability.retry.initial_delay_ms,
        original_retry.initial_delay_ms
    );

    let mut retry = original_retry.clone();
    retry.outbox_max_lifetime_ms = 0;
    assert!(matches!(
        protocol.update_retry_config(retry),
        Err(Error::InvalidConfiguration(_))
    ));
    assert_eq!(
        protocol.config.reliability.retry.outbox_max_lifetime_ms,
        original_retry.outbox_max_lifetime_ms
    );

    let mut ack = original_ack.clone();
    ack.default_timeout_ms = 0;
    assert!(matches!(
        protocol.update_ack_config(ack),
        Err(Error::InvalidConfiguration(_))
    ));
    assert_eq!(
        protocol.config.reliability.ack.default_timeout_ms,
        original_ack.default_timeout_ms
    );
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
    let mock_transport = MockTransport::new(TransportType::BLE);
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
    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_receive_message_surfaces_reply_context_on_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let captured = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    protocol.on_event(move |event| {
        if let Event::MessageReceived { reply_context, .. } = event {
            *captured_clone.lock().unwrap() = Some(reply_context.clone());
        }
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    let message = Message::builder(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content("see quoted")
    .reply_context(offline_protocol_core::ReplyContext {
        sender: UserId::new("carol").unwrap(),
        text: "the original".to_string(),
        timestamp: None,
        reply_media_label: None,
        reply_content_type: Some("text".to_string()),
    })
    .build();
    mock_transport.queue_message(message);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    assert!(protocol.receive_message().is_some());
    let rc = captured
        .lock()
        .unwrap()
        .clone()
        .flatten()
        .expect("reply_context surfaced on MessageReceived");
    assert_eq!(rc.sender, "carol");
    assert_eq!(rc.text, "the original");
    assert_eq!(rc.reply_content_type.as_deref(), Some("text"));
}

#[test]
fn test_encrypted_receive_drops_relay_injected_reply_context() {
    // The outer `reply_context` field is hop-visible cleartext outside the
    // MLS AEAD boundary: a relay can inject or rewrite it in transit. On a
    // decrypted (encrypted: true) message it must NOT surface — otherwise an
    // attacker-controlled quote preview would render as if it were part of
    // the authenticated conversation.
    use crate::mls::InMemoryStorage;

    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    alice
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    let captured = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    bob.on_event(move |event| {
        if let Event::MessageReceived {
            reply_context,
            encrypted,
            ..
        } = event
        {
            *captured_clone.lock().unwrap() = Some((reply_context.clone(), encrypted));
        }
    });

    // Establish the MLS session directly (shortcut, same as the TOFU test).
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

    alice
        .send_message(
            "bob",
            "sealed hello",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    let mut wire_msg = alice_transport_handle
        .sent_messages()
        .last()
        .expect("an __MLS_ENC__ message must reach the wire")
        .clone();
    assert!(wire_msg.content.starts_with(internal_prefixes::ENCRYPTED));

    // Hostile relay: splice a forged quote preview onto the sealed message.
    wire_msg.reply_context = Some(offline_protocol_core::ReplyContext {
        sender: UserId::new("mallory").unwrap(),
        text: "forged quote".to_string(),
        timestamp: None,
        reply_media_label: None,
        reply_content_type: None,
    });

    bob_transport_handle.queue_message(wire_msg);
    let received = bob.receive_message().expect("message delivered");
    assert_eq!(received.content, "sealed hello");
    assert!(
        received.reply_context.is_none(),
        "outer reply_context on a sealed message must be dropped"
    );

    let (event_rc, event_encrypted) = captured
        .lock()
        .unwrap()
        .clone()
        .expect("MessageReceived emitted");
    assert!(event_encrypted, "message must surface as encrypted");
    assert!(
        event_rc.is_none(),
        "relay-injected reply_context must not surface on an encrypted event"
    );
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
    let mock_transport = MockTransport::new(TransportType::BLE);
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
    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
    let mock_transport = MockTransport::new(TransportType::BLE);
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

#[test]
fn test_mls_observability_emits_initialized_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    protocol.initialize_mls_for_test(storage).unwrap();

    let events = emitter.take();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MlsLifecycleEvent::Initialized { .. })),
        "Expected initialized lifecycle event"
    );
}

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

#[test]
fn test_mls_observability_emits_encryption_used_for_successful_encrypt() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

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

#[test]
fn test_mls_observability_emits_decryption_failed_not_initialized() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));

    let encrypted = EncryptedMessage {
        group_id: offline_protocol_mls::GroupId::new("session:alice:bob").unwrap(),
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

#[test]
fn test_mls_observability_no_encryption_event_on_aborted_encrypt() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

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

#[test]
fn test_mls_observability_session_ready_emits_once_for_idempotent_confirm() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    protocol.welcome_lifecycles.insert(
        "bob".to_string(),
        WelcomeLifecycleRecord {
            peer_id: "bob".to_string(),
            group_id: "session:user123:bob".to_string(),
            state: WelcomeDeliveryState::Sent,
            attempt: 1,
            unreachable_parks: 0,
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

#[test]
fn test_mls_observability_uses_opaque_identifiers() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

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

// ===========================================================================
// TelemetrySink integration (unified telemetry surface)
// ===========================================================================

#[test]
fn test_telemetry_sink_receives_protocol_events() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));

    let records = sink.take();
    assert_eq!(records.len(), 1, "sink should have received one record");
    match &records[0] {
        TelemetryRecord::Protocol(event) => {
            assert_eq!(event.telemetry_name(), "protocol.neighbor.lost");
            match event.as_ref() {
                crate::events::Event::NeighborLost { peer_id } => {
                    // scrub_ids defaults to true — expect a 32-hex opaque ID.
                    assert_ne!(peer_id, "alice", "peer_id must be scrubbed by default");
                    assert_eq!(peer_id.len(), 32);
                    assert!(peer_id.chars().all(|c| c.is_ascii_hexdigit()));
                }
                _ => panic!("unexpected inner variant"),
            }
        }
        other => panic!("expected Protocol variant, got {other:?}"),
    }
}

#[test]
fn test_telemetry_sink_receives_mls_events() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let records = sink.take();
    assert!(
        records.iter().any(|r| matches!(
            r,
            TelemetryRecord::Mls(MlsLifecycleEvent::Initialized { .. })
        )),
        "sink should observe mls.initialized, got {records:?}",
    );
}

#[test]
fn test_mls_verbosity_off_suppresses_both_sink_and_legacy_emitter() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(
            Arc::new(sink.clone()),
            TelemetryConfig::default().with_mls_verbosity(MlsVerbosity::Off),
        )
        .unwrap();

    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let legacy = emitter.take();
    let sink_records = sink.take();
    assert!(
        legacy.is_empty(),
        "MlsVerbosity::Off must suppress the legacy emitter, got {legacy:?}",
    );
    assert!(
        sink_records
            .iter()
            .all(|r| !matches!(r, TelemetryRecord::Mls(_))),
        "MlsVerbosity::Off must suppress sink Mls records, got {sink_records:?}",
    );
}

/// Counts the `mls.decryption_failed` records observed by the sink.
fn count_decryption_failed(records: &[TelemetryRecord]) -> usize {
    records
        .iter()
        .filter(|r| {
            matches!(
                r,
                TelemetryRecord::Mls(MlsLifecycleEvent::DecryptionFailed { .. })
            )
        })
        .count()
}

#[test]
fn test_mls_sampling_default_caps_decryption_failed_flood() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    // 25 failures for the same sender+kind within one window. The default
    // fixed-window limiter caps emission at 10 per peer+kind.
    for _ in 0..25 {
        protocol.emit_mls_decryption_failed(
            "sender-a",
            None,
            DecryptionFailureKind::InvalidCiphertext,
            MlsOperationContext::Receive,
        );
    }

    let count = count_decryption_failed(&sink.take());
    assert_eq!(
        count, 10,
        "default config must cap decryption_failed at the per-window ceiling, got {count}",
    );
}

#[test]
fn test_mls_sampling_bypass_delivers_unsampled_decryption_failed() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(
            Arc::new(sink.clone()),
            TelemetryConfig::default().with_mls_sampling_bypass(true),
        )
        .unwrap();

    for _ in 0..25 {
        protocol.emit_mls_decryption_failed(
            "sender-a",
            None,
            DecryptionFailureKind::InvalidCiphertext,
            MlsOperationContext::Receive,
        );
    }

    let count = count_decryption_failed(&sink.take());
    assert_eq!(
        count, 25,
        "mls_sampling_bypass must deliver every decryption_failed event un-sampled, got {count}",
    );
}

#[test]
fn test_scrub_ids_false_passes_raw_peer_id_to_sink() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(
            Arc::new(sink.clone()),
            TelemetryConfig::default().with_scrub_ids(false),
        )
        .unwrap();

    protocol.emit_event(crate::events::Event::neighbor_lost("alice-raw".into()));

    let records = sink.take();
    assert_eq!(records.len(), 1);
    match &records[0] {
        TelemetryRecord::Protocol(event) => match event.as_ref() {
            crate::events::Event::NeighborLost { peer_id } => {
                assert_eq!(
                    peer_id, "alice-raw",
                    "scrub_ids(false) must pass raw identifier through",
                );
            }
            _ => panic!("unexpected inner variant"),
        },
        other => panic!("expected Protocol variant, got {other:?}"),
    }
}

#[test]
fn test_legacy_event_callback_still_fires_with_sink_installed() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let captured: Arc<Mutex<Vec<crate::events::Event>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_handler = captured.clone();
    protocol.on_event(move |event| {
        captured_handler.lock().unwrap().push(event);
    });

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));

    // Legacy callback receives the raw event.
    let legacy_events = captured.lock().unwrap().clone();
    assert_eq!(legacy_events.len(), 1);
    match &legacy_events[0] {
        crate::events::Event::NeighborLost { peer_id } => assert_eq!(peer_id, "alice"),
        _ => panic!("unexpected variant"),
    }

    // Sink receives the scrubbed event.
    let sink_records = sink.take();
    assert_eq!(sink_records.len(), 1);
    match &sink_records[0] {
        TelemetryRecord::Protocol(event) => match event.as_ref() {
            crate::events::Event::NeighborLost { peer_id } => {
                assert_ne!(peer_id, "alice", "sink should see scrubbed ID");
                assert_eq!(peer_id.len(), 32);
            }
            _ => panic!("unexpected inner variant"),
        },
        other => panic!("expected Protocol variant, got {other:?}"),
    }
}

#[test]
fn test_noop_telemetry_sink_installs_cleanly() {
    // Guard against regressions where the install path depends on the sink
    // actually receiving records. `NoopTelemetrySink` is the zero-cost
    // default apps fall back to when they want install-path wiring without
    // paying for emission.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .install_telemetry_sink(Arc::new(NoopTelemetrySink), TelemetryConfig::default())
        .unwrap();
    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));
    // If we got here, the install + emit path did not panic.
}

#[test]
fn test_install_telemetry_sink_replaces_previous_sink() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let first = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(first.clone()), TelemetryConfig::default())
        .unwrap();

    // First sink should observe the pre-swap event.
    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));

    let second = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(second.clone()), TelemetryConfig::default())
        .unwrap();

    // After re-install, only the second sink observes the post-swap event.
    protocol.emit_event(crate::events::Event::neighbor_lost("bob".into()));

    let first_records = first.take();
    let second_records = second.take();
    assert_eq!(
        first_records.len(),
        1,
        "first sink should see exactly the pre-swap event, got {first_records:?}",
    );
    assert_eq!(
        second_records.len(),
        1,
        "second sink should see exactly the post-swap event, got {second_records:?}",
    );
}

#[test]
fn test_mls_verbosity_diagnostic_matches_lifecycle_today() {
    // Locks the documented contract that `Diagnostic` currently emits the
    // same stream as `Lifecycle`. If the two ever diverge, update the
    // `MlsVerbosity::Diagnostic` rustdoc at the same time.
    fn records_for(verbosity: MlsVerbosity) -> Vec<TelemetryRecord> {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        let sink = RecordingTelemetrySink::default();
        protocol
            .install_telemetry_sink(
                Arc::new(sink.clone()),
                TelemetryConfig::default().with_mls_verbosity(verbosity),
            )
            .unwrap();
        let storage = Arc::new(crate::mls::InMemoryStorage::new());
        protocol.initialize_mls_for_test(storage).unwrap();
        sink.take()
    }

    let lifecycle_names: Vec<&'static str> = records_for(MlsVerbosity::Lifecycle)
        .iter()
        .map(|r| r.name())
        .collect();
    let diagnostic_names: Vec<&'static str> = records_for(MlsVerbosity::Diagnostic)
        .iter()
        .map(|r| r.name())
        .collect();

    assert_eq!(
        lifecycle_names, diagnostic_names,
        "Diagnostic must emit the same record names as Lifecycle until the richer stream lands",
    );
}

#[test]
fn test_session_id_stays_consistent_across_install_boundary() {
    // The `telemetry_fallback_secret` is per-instance and is reused when
    // `install_telemetry_sink` supplies a config without an explicit
    // `scrub_secret`. This means an opaque `session_id` that the legacy
    // emitter sees before install must match what the sink sees after
    // install for the same (peer, group) pair.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(emitter.clone()));

    // Pre-install: drive an MLS session-missing emission. The legacy
    // emitter records the opaque session_id derived from the pre-install
    // scrubber.
    protocol.emit_mls_session_missing(
        Some("peer-a"),
        Some("group-b"),
        MlsOperationContext::SessionLookup,
        MlsErrorCategory::NotInitialized,
    );
    let pre_install = emitter.take();
    assert_eq!(pre_install.len(), 1);
    let pre_session_id = match &pre_install[0] {
        MlsLifecycleEvent::SessionMissing { session_id, .. } => session_id.clone(),
        other => panic!("unexpected MLS event before install: {other:?}"),
    };

    // Install the sink with a default config (no scrub_secret) — the
    // fallback secret is reused, so derived correlation tokens must stay
    // stable.
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    // Post-install: drive another session-missing emission with the same
    // inputs. Both the legacy emitter and the sink now observe it.
    protocol.emit_mls_session_missing(
        Some("peer-a"),
        Some("group-b"),
        MlsOperationContext::SessionLookup,
        MlsErrorCategory::NotInitialized,
    );

    let post_legacy = emitter.take();
    let post_sink: Vec<TelemetryRecord> = sink
        .take()
        .into_iter()
        .filter(|r| matches!(r, TelemetryRecord::Mls(_)))
        .collect();
    assert_eq!(post_legacy.len(), 1);
    assert_eq!(post_sink.len(), 1);

    let post_legacy_id = match &post_legacy[0] {
        MlsLifecycleEvent::SessionMissing { session_id, .. } => session_id.clone(),
        other => panic!("unexpected MLS event after install: {other:?}"),
    };
    let post_sink_id = match &post_sink[0] {
        TelemetryRecord::Mls(MlsLifecycleEvent::SessionMissing { session_id, .. }) => {
            session_id.clone()
        }
        other => panic!("unexpected sink record after install: {other:?}"),
    };

    assert_eq!(
        pre_session_id, post_legacy_id,
        "legacy emitter must see a consistent session_id across the install boundary",
    );
    assert_eq!(
        post_legacy_id, post_sink_id,
        "legacy emitter and sink must see the same session_id post-install",
    );
}

#[test]
fn test_sink_installed_after_initialize_mls_does_not_replay() {
    // `install_telemetry_sink` is not a replay API — events that fired
    // before the sink existed stay on the floor. Regression guard for the
    // documented contract.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    // Sink installed *after* MLS init.
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    let records = sink.take();
    assert!(
        records.iter().all(|r| !matches!(
            r,
            TelemetryRecord::Mls(MlsLifecycleEvent::Initialized { .. })
        )),
        "initialize_mls firing before install must not replay to the sink, got {records:?}",
    );
}

#[test]
fn test_on_neighbor_discovered_without_mls() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Add a mock transport
    let mock_transport = MockTransport::new(TransportType::BLE);
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
    assert_eq!(internal_prefixes::RICH_V1, "__RICH_V1__");
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

/// Helper: feed `protocol` a signed-shape key package from `sender` advertising
/// `wire_versions`, exercising the negotiation path in `handle_key_package_message`.
#[cfg(test)]
fn feed_key_package(protocol: &mut OfflineProtocol, sender: &str, wire_versions: Vec<u8>) {
    let payload = KeyPackagePayload {
        user_id: sender.to_string(),
        key_package_data: vec![1, 2, 3, 4],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions,
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    protocol.process_internal_message(&message);
}

#[test]
fn binary_wire_negotiation_marks_capable_peer() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    // binary_wire_enabled defaults to true.
    let mut protocol = OfflineProtocol::new(config).unwrap();

    feed_key_package(
        &mut protocol,
        "peer-bin",
        vec![offline_protocol_core::WIRE_VERSION_V1],
    );

    assert!(protocol
        .transport_manager
        .peer_supports_binary_wire("peer-bin"));
}

#[test]
fn binary_wire_negotiation_ignores_legacy_peer() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Legacy peer omits wire_versions (empty) -> stays on JSON.
    feed_key_package(&mut protocol, "peer-legacy", Vec::new());

    assert!(!protocol
        .transport_manager
        .peer_supports_binary_wire("peer-legacy"));
}

#[test]
fn binary_wire_kill_switch_prevents_recording() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    config.transport.binary_wire_enabled = false; // kill switch off
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Peer advertises binary, but with the codec disabled we do not record it.
    feed_key_package(
        &mut protocol,
        "peer-bin",
        vec![offline_protocol_core::WIRE_VERSION_V1],
    );

    assert!(!protocol
        .transport_manager
        .peer_supports_binary_wire("peer-bin"));
}

/// Helper: like [`feed_key_package`] but advertising `env_versions`
/// (compact-MLS-envelope capability) instead of wire versions.
#[cfg(test)]
fn feed_key_package_with_env(protocol: &mut OfflineProtocol, sender: &str, env_versions: Vec<u8>) {
    let payload = KeyPackagePayload {
        user_id: sender.to_string(),
        key_package_data: vec![1, 2, 3, 4],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions,
        rich_versions: Vec::new(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    protocol.process_internal_message(&message);
}

/// Fixture `EncryptedMessage` with a realistic ciphertext size.
#[cfg(test)]
fn sample_encrypted_message() -> offline_protocol_mls::EncryptedMessage {
    offline_protocol_mls::EncryptedMessage {
        group_id: offline_protocol_mls::GroupId::new("session:alice:bob").unwrap(),
        message_type: offline_protocol_mls::MlsMessageType::Application,
        epoch: 3,
        ciphertext: (0u8..=255).collect(),
        sender_id: "alice".to_string(),
        timestamp_ms: 1_700_000_000_000,
    }
}

#[test]
fn compact_envelope_negotiation_marks_capable_peer() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    // compact_envelope_enabled defaults to true.
    let mut protocol = OfflineProtocol::new(config).unwrap();

    feed_key_package_with_env(&mut protocol, "peer-compact", vec![MLS_ENVELOPE_COMPACT_V1]);

    assert!(protocol.peer_compact_envelope.contains("peer-compact"));
}

#[test]
fn compact_envelope_negotiation_ignores_legacy_peer() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Legacy peer omits env_versions (empty) -> stays on the JSON envelope.
    feed_key_package_with_env(&mut protocol, "peer-legacy", Vec::new());

    assert!(!protocol.peer_compact_envelope.contains("peer-legacy"));
}

#[test]
fn compact_envelope_kill_switch_prevents_recording_and_sealing() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    config.encryption.compact_envelope_enabled = false; // kill switch off
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Peer advertises the capability, but with the flag off we do not record it.
    feed_key_package_with_env(&mut protocol, "peer-compact", vec![MLS_ENVELOPE_COMPACT_V1]);
    assert!(!protocol.peer_compact_envelope.contains("peer-compact"));

    // Defense in depth: even a (stale) recorded capability must not produce a
    // compact envelope while the flag is off.
    protocol
        .peer_compact_envelope
        .insert("peer-compact".to_string());
    let sealed = protocol
        .seal_encrypted_content("peer-compact", &sample_encrypted_message())
        .unwrap();
    let body = sealed.strip_prefix(internal_prefixes::ENCRYPTED).unwrap();
    assert!(
        body.starts_with('{'),
        "kill switch must force the JSON envelope"
    );
}

#[test]
fn compact_envelope_downgrade_removes_capability() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    feed_key_package_with_env(&mut protocol, "peer", vec![MLS_ENVELOPE_COMPACT_V1]);
    assert!(protocol.peer_compact_envelope.contains("peer"));

    // A fresh key package without the capability (peer downgraded or flipped
    // its own kill switch) must remove it.
    feed_key_package_with_env(&mut protocol, "peer", Vec::new());
    assert!(!protocol.peer_compact_envelope.contains("peer"));
}

#[test]
fn seal_encrypted_content_picks_envelope_per_recipient_capability() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let encrypted = sample_encrypted_message();

    // Unknown capability -> legacy JSON envelope (the permanent floor).
    let sealed = protocol.seal_encrypted_content("peer", &encrypted).unwrap();
    let body = sealed.strip_prefix(internal_prefixes::ENCRYPTED).unwrap();
    assert!(body.starts_with('{'));

    // Advertised capability -> compact envelope: base64, never '{', and far
    // smaller than the JSON form (whose ciphertext is an integer array).
    feed_key_package_with_env(&mut protocol, "peer", vec![MLS_ENVELOPE_COMPACT_V1]);
    let sealed = protocol.seal_encrypted_content("peer", &encrypted).unwrap();
    let body = sealed.strip_prefix(internal_prefixes::ENCRYPTED).unwrap();
    assert!(!body.starts_with('{'));
    let json_len = serde_json::to_string(&encrypted).unwrap().len();
    assert!(
        body.len() * 2 < json_len,
        "compact envelope ({}) should be well under half the JSON form ({})",
        body.len(),
        json_len
    );

    // And the receive path reads it back, field for field.
    let parsed = OfflineProtocol::parse_encrypted_payload(body).unwrap();
    assert_eq!(parsed.ciphertext, encrypted.ciphertext);
    assert_eq!(parsed.group_id.as_str(), encrypted.group_id.as_str());
    assert_eq!(parsed.epoch, encrypted.epoch);
    assert_eq!(parsed.sender_id, encrypted.sender_id);
    assert_eq!(parsed.timestamp_ms, encrypted.timestamp_ms);
}

#[test]
fn parse_encrypted_payload_reads_legacy_compact_and_wrapped_json_forms() {
    let encrypted = sample_encrypted_message();
    let json = serde_json::to_string(&encrypted).unwrap();
    let compact = base64_encode(&encrypted.to_bytes());
    // Historical layout: base64-wrapped JSON (`EncryptedMessage::from_base64`).
    let wrapped_json = encrypted.to_base64().unwrap();

    for body in [json.as_str(), compact.as_str(), wrapped_json.as_str()] {
        let parsed = OfflineProtocol::parse_encrypted_payload(body)
            .unwrap_or_else(|| panic!("form must parse: {}", &body[..20.min(body.len())]));
        assert_eq!(parsed.ciphertext, encrypted.ciphertext);
        assert_eq!(parsed.group_id.as_str(), encrypted.group_id.as_str());
    }

    // Hostile shapes are rejected, never panicked on.
    assert!(OfflineProtocol::parse_encrypted_payload("").is_none());
    assert!(OfflineProtocol::parse_encrypted_payload("not base64 !!!").is_none());
    assert!(OfflineProtocol::parse_encrypted_payload(&base64_encode(&[0xFF; 8])).is_none());
    let mut truncated = encrypted.to_bytes();
    truncated.truncate(truncated.len() - 5);
    assert!(OfflineProtocol::parse_encrypted_payload(&base64_encode(&truncated)).is_none());
}

/// Helper: like [`feed_key_package`] but advertising `rich_versions`
/// (sealed rich payload capability). Also used by `group_mesh_tests`.
#[cfg(test)]
pub(crate) fn feed_key_package_with_rich(
    protocol: &mut OfflineProtocol,
    sender: &str,
    rich_versions: Vec<u8>,
) {
    let payload = KeyPackagePayload {
        user_id: sender.to_string(),
        key_package_data: vec![1, 2, 3, 4],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    protocol.process_internal_message(&message);
}

/// Fixture rich extras with every field populated, including media secrets.
#[cfg(test)]
fn sample_rich_extras() -> RichSendExtras {
    let mut media = sample_media_metadata(42);
    media.download_url = Some("https://cdn.example/blob/1".to_string());
    media.encryption_key = Some("a2V5LWJ5dGVz".to_string());
    media.iv = Some("aXYtYnl0ZXM=".to_string());
    RichSendExtras {
        reply_context: Some(offline_protocol_core::ReplyContext {
            sender: UserId::new("carol").unwrap(),
            text: "the quoted message".to_string(),
            timestamp: None,
            reply_media_label: None,
            reply_content_type: Some("text".to_string()),
        }),
        media_metadata: Some(media),
        forward_info: Some(offline_protocol_core::ForwardInfo {
            original_sender: UserId::new("dave").unwrap(),
            original_message_id: MessageId::new(),
            original_timestamp: offline_protocol_core::Timestamp::now(),
            forward_count: 1,
        }),
    }
}

#[test]
fn rich_payload_negotiation_marks_capable_peer() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    // rich_payload_enabled defaults to true.
    let mut protocol = OfflineProtocol::new(config).unwrap();

    feed_key_package_with_rich(&mut protocol, "peer-rich", vec![RICH_PAYLOAD_V1]);

    assert!(protocol.peer_rich_payload.contains("peer-rich"));
    assert!(protocol.rich_seal_active("peer-rich"));
}

#[test]
fn rich_payload_negotiation_ignores_legacy_peer() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Legacy peer omits rich_versions (empty) -> plain text only.
    feed_key_package_with_rich(&mut protocol, "peer-legacy", Vec::new());

    assert!(!protocol.peer_rich_payload.contains("peer-legacy"));
    assert!(!protocol.rich_seal_active("peer-legacy"));
}

#[test]
fn rich_payload_kill_switch_prevents_recording_and_sealing() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    config.encryption.rich_payload_enabled = false; // kill switch off
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Peer advertises the capability, but with the flag off we do not record it.
    feed_key_package_with_rich(&mut protocol, "peer-rich", vec![RICH_PAYLOAD_V1]);
    assert!(!protocol.peer_rich_payload.contains("peer-rich"));

    // Defense in depth: even a (stale) recorded capability must not seal
    // while the flag is off.
    protocol.peer_rich_payload.insert("peer-rich".to_string());
    assert!(!protocol.rich_seal_active("peer-rich"));
}

#[test]
fn rich_payload_downgrade_removes_capability() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.auto_key_exchange = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    feed_key_package_with_rich(&mut protocol, "peer", vec![RICH_PAYLOAD_V1]);
    assert!(protocol.peer_rich_payload.contains("peer"));

    // A fresh key package without the capability (peer downgraded or flipped
    // its own kill switch) must remove it.
    feed_key_package_with_rich(&mut protocol, "peer", Vec::new());
    assert!(!protocol.peer_rich_payload.contains("peer"));
}

/// Feeds a key package advertising both end-to-end capabilities (compact
/// envelope + sealed rich payload) — the persistence tests below exercise
/// both sets at once.
#[cfg(test)]
fn feed_key_package_with_caps(
    protocol: &mut OfflineProtocol,
    sender: &str,
    env_versions: Vec<u8>,
    rich_versions: Vec<u8>,
) {
    let payload = KeyPackagePayload {
        user_id: sender.to_string(),
        key_package_data: vec![1, 2, 3, 4],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions,
        rich_versions,
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&payload).unwrap()
    );
    let message = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    protocol.process_internal_message(&message);
}

/// Builds a protocol instance with MLS initialized on the given storage —
/// the restart half of the capability persistence tests.
#[cfg(test)]
fn protocol_with_mls_storage(storage: Arc<InMemoryStorage>) -> OfflineProtocol {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();
    protocol
}

#[test]
fn peer_capabilities_persist_across_restart() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    feed_key_package_with_caps(
        &mut protocol,
        "peer",
        vec![MLS_ENVELOPE_COMPACT_V1],
        vec![RICH_PAYLOAD_V1],
    );
    assert!(protocol.peer_compact_envelope.contains("peer"));
    assert!(protocol.peer_rich_payload.contains("peer"));

    // Restart: a fresh instance on the same storage must re-learn both
    // capabilities from the durable record, without any live exchange.
    let restarted = protocol_with_mls_storage(storage);
    assert!(
        restarted.peer_compact_envelope.contains("peer"),
        "compact envelope capability must survive a restart"
    );
    assert!(
        restarted.peer_rich_payload.contains("peer"),
        "rich payload capability must survive a restart"
    );
    assert!(restarted.peer_supports_rich_payload("peer"));
}

#[test]
fn peer_capability_downgrade_survives_restart() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    feed_key_package_with_caps(&mut protocol, "peer", Vec::new(), vec![RICH_PAYLOAD_V1]);
    // A fresh key package without the capability removes it — the durable
    // record must follow, or a restart would resurrect the stale grant.
    feed_key_package_with_caps(&mut protocol, "peer", Vec::new(), Vec::new());
    assert!(!protocol.peer_rich_payload.contains("peer"));

    let restarted = protocol_with_mls_storage(storage);
    assert!(
        !restarted.peer_rich_payload.contains("peer"),
        "a downgraded capability must not come back after restart"
    );
}

#[test]
fn peer_capability_restore_respects_kill_switch_but_keeps_record() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    feed_key_package_with_caps(&mut protocol, "peer", Vec::new(), vec![RICH_PAYLOAD_V1]);

    // Restart with the kill switch off: the capability must not be applied…
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.rich_payload_enabled = false;
    let mut gated = OfflineProtocol::new(config).unwrap();
    gated.initialize_mls_for_test(storage.clone()).unwrap();
    assert!(!gated.peer_rich_payload.contains("peer"));
    assert!(!gated.peer_supports_rich_payload("peer"));

    // …but the record survives, so flipping the switch back on restores it
    // on the next run — same semantics as toggling the switch live.
    let restored = protocol_with_mls_storage(storage);
    assert!(restored.peer_rich_payload.contains("peer"));
}

#[test]
fn peer_capability_unblock_clears_record() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    feed_key_package_with_caps(&mut protocol, "peer", Vec::new(), vec![RICH_PAYLOAD_V1]);

    // Unblock runs the clean-slate cleanup: capability forgotten in memory
    // and on disk, to be re-learned from the fresh exchange.
    protocol.block_user("peer").unwrap();
    protocol.unblock_user("peer").unwrap();
    assert!(!protocol.peer_rich_payload.contains("peer"));

    let restarted = protocol_with_mls_storage(storage);
    assert!(
        !restarted.peer_rich_payload.contains("peer"),
        "unblock's clean slate must extend to the durable capability record"
    );
}

#[test]
fn peer_capability_restore_prunes_overflow() {
    // A durable store inflated past the cap (e.g. by a forged-sender flood
    // on an older build) must shrink to the cap in a single boot, exactly
    // like restore_peer_key_packages.
    let storage = Arc::new(InMemoryStorage::new());
    let caps = PeerCapabilities {
        attested_rich_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: vec![RICH_PAYLOAD_V1],
    };
    let encoded = serde_json::to_vec(&caps).unwrap();
    for i in 0..MAX_KEY_PACKAGE_SENT_TO + 7 {
        storage
            .store(
                storage_keys::PEER_CAPABILITIES,
                &format!("peer-{i}"),
                &encoded,
            )
            .unwrap();
    }

    let protocol = protocol_with_mls_storage(storage.clone());
    assert_eq!(protocol.peer_rich_payload.len(), MAX_KEY_PACKAGE_SENT_TO);
    assert_eq!(
        storage
            .list_keys(storage_keys::PEER_CAPABILITIES)
            .unwrap()
            .len(),
        MAX_KEY_PACKAGE_SENT_TO,
        "overflow records must be pruned from durable storage"
    );
}

/// Establishes a local MLS session from `protocol` toward `peer` at the
/// manager level (import + create), without needing the peer to process the
/// Welcome — `has_session`/`list_sessions` are local.
#[cfg(test)]
fn create_local_session_with(protocol: &OfflineProtocol, peer: &str) {
    let peer_mgr =
        crate::mls::MlsManager::new(peer, Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
    let peer_kp = peer_mgr.generate_key_package().unwrap();
    let mls = protocol.mls_manager.as_ref().unwrap().clone();
    let manager = mls.read().unwrap();
    manager
        .import_key_package(peer, &peer_kp.key_package_data)
        .unwrap();
    manager.create_session(peer).unwrap();
}

#[test]
fn peer_capability_eviction_spares_session_peers() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut alice = protocol_with_mls_storage(storage.clone());
    create_local_session_with(&alice, "bob");

    // Bob re-advertises after his own restart: the package is inserted into
    // pending_key_packages but never consumed (the session already exists),
    // so bob IS in the eviction candidate pool despite the session.
    feed_key_package_with_caps(&mut alice, "bob", Vec::new(), vec![RICH_PAYLOAD_V1]);
    assert!(alice.pending_key_packages.contains_key("bob"));
    // Make bob the deterministic soonest-to-expire victim.
    alice
        .pending_key_packages
        .get_mut("bob")
        .unwrap()
        .local_expires_at_ms = 1;

    // A forged-sender flood fills the map to capacity…
    while alice.pending_key_packages.len() < MAX_PENDING_KEY_PACKAGES {
        let i = alice.pending_key_packages.len();
        alice.pending_key_packages.insert(
            format!("forged-{i}"),
            ReceivedKeyPackage {
                key_package_data: vec![0],
                local_expires_at_ms: u64::MAX,
            },
        );
    }
    // …and the next forged package evicts bob's key package. His durable
    // capability record must survive, or the flood would silently degrade
    // rich sends to him after the next relaunch (the #200 window).
    feed_key_package_with_caps(&mut alice, "trigger-1", Vec::new(), vec![RICH_PAYLOAD_V1]);
    assert!(!alice.pending_key_packages.contains_key("bob"));
    assert!(
        storage
            .load(storage_keys::PEER_CAPABILITIES, "bob")
            .unwrap()
            .is_some(),
        "a session peer's durable capability record must survive eviction"
    );
    assert!(alice.peer_rich_payload.contains("bob"));

    // A non-session victim's record goes with its key package — both are
    // recoverable via re-exchange, and this ties the durable capability
    // count to the flood bound.
    feed_key_package_with_caps(&mut alice, "carol", Vec::new(), vec![RICH_PAYLOAD_V1]);
    alice
        .pending_key_packages
        .get_mut("carol")
        .unwrap()
        .local_expires_at_ms = 1;
    feed_key_package_with_caps(&mut alice, "trigger-2", Vec::new(), vec![RICH_PAYLOAD_V1]);
    assert!(!alice.pending_key_packages.contains_key("carol"));
    assert!(
        storage
            .load(storage_keys::PEER_CAPABILITIES, "carol")
            .unwrap()
            .is_none(),
        "a non-session victim's capability record is evicted with its key package"
    );
}

#[test]
fn peer_capability_restore_prefers_session_peers() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut alice = protocol_with_mls_storage(storage.clone());
    create_local_session_with(&alice, "bob");
    feed_key_package_with_caps(
        &mut alice,
        "bob",
        vec![MLS_ENVELOPE_COMPACT_V1],
        vec![RICH_PAYLOAD_V1],
    );

    // Inflate the store past the cap with forged non-session records.
    // `list_keys` order is backend-defined, so without session-first
    // admission the over-cap prune could evict bob's record while keeping
    // forged leftovers.
    let caps = PeerCapabilities {
        attested_rich_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: vec![RICH_PAYLOAD_V1],
    };
    let encoded = serde_json::to_vec(&caps).unwrap();
    for i in 0..MAX_KEY_PACKAGE_SENT_TO + 40 {
        storage
            .store(
                storage_keys::PEER_CAPABILITIES,
                &format!("forged-{i}"),
                &encoded,
            )
            .unwrap();
    }

    let restarted = protocol_with_mls_storage(storage.clone());
    assert!(
        restarted.peer_rich_payload.contains("bob"),
        "the session peer's record must always survive an over-cap prune"
    );
    assert!(storage
        .load(storage_keys::PEER_CAPABILITIES, "bob")
        .unwrap()
        .is_some());
    assert_eq!(
        storage
            .list_keys(storage_keys::PEER_CAPABILITIES)
            .unwrap()
            .len(),
        MAX_KEY_PACKAGE_SENT_TO
    );
}

#[test]
fn peer_capability_restore_deletes_corrupt_record() {
    let storage = Arc::new(InMemoryStorage::new());
    storage
        .store(storage_keys::PEER_CAPABILITIES, "mangled", b"not-json")
        .unwrap();
    let protocol = protocol_with_mls_storage(storage.clone());
    assert!(!protocol.peer_rich_payload.contains("mangled"));
    assert!(!protocol.peer_compact_envelope.contains("mangled"));
    assert!(
        storage
            .load(storage_keys::PEER_CAPABILITIES, "mangled")
            .unwrap()
            .is_none(),
        "a corrupt record must be deleted, not re-parsed on every boot"
    );
}

#[test]
fn peer_capability_persist_truncates_version_lists() {
    // The advertised lists are unauthenticated wire input persisted raw; a
    // hostile advertiser padding them must not bloat the durable record.
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    let mut rich_versions = vec![RICH_PAYLOAD_V1];
    rich_versions.extend(2..=200u8);
    feed_key_package_with_caps(&mut protocol, "peer", Vec::new(), rich_versions);

    let data = storage
        .load(storage_keys::PEER_CAPABILITIES, "peer")
        .unwrap()
        .expect("record must persist");
    let caps: PeerCapabilities = serde_json::from_slice(&data).unwrap();
    assert_eq!(caps.rich_versions.len(), MAX_PERSISTED_CAPABILITY_VERSIONS);
    assert!(caps.rich_versions.contains(&RICH_PAYLOAD_V1));
}

#[test]
fn attested_rich_gates_group_seal_only_and_persists() {
    // An inviter-attested capability must open the *group* seal gate for a
    // peer we never directly exchanged with, survive a restart, and never
    // leak into DM sealing (that path always has direct knowledge).
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    let members = vec!["user123".to_string(), "peer".to_string()];
    assert!(!protocol.group_rich_seal_active(&members));

    protocol.record_attested_rich("peer", &[RICH_PAYLOAD_V1]);
    assert!(protocol.peer_rich_attested.contains("peer"));
    assert!(protocol.group_rich_seal_active(&members));
    assert!(
        !protocol.rich_seal_active("peer") && !protocol.peer_supports_rich_payload("peer"),
        "attestation must never open the DM seal gate"
    );

    let restarted = protocol_with_mls_storage(storage);
    assert!(
        restarted.peer_rich_attested.contains("peer"),
        "attested capability must survive a restart"
    );
    assert!(restarted.group_rich_seal_active(&members));
}

#[test]
fn attested_rich_ignores_non_v1_and_self() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage);
    protocol.record_attested_rich("peer", &[99]);
    assert!(!protocol.peer_rich_attested.contains("peer"));
    protocol.record_attested_rich("user123", &[RICH_PAYLOAD_V1]);
    assert!(!protocol.peer_rich_attested.contains("user123"));
}

#[test]
fn direct_key_package_overrides_attested_rich() {
    // Direct knowledge is authoritative in both directions: a package
    // without the capability evicts a stale attestation (memory + disk),
    // and one with it moves the peer to the direct set.
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    let members = vec!["user123".to_string(), "peer".to_string()];

    protocol.record_attested_rich("peer", &[RICH_PAYLOAD_V1]);
    feed_key_package_with_rich(&mut protocol, "peer", Vec::new());
    assert!(!protocol.peer_rich_attested.contains("peer"));
    assert!(!protocol.group_rich_seal_active(&members));
    let restarted = protocol_with_mls_storage(storage.clone());
    assert!(
        !restarted.peer_rich_attested.contains("peer"),
        "a direct downgrade must also kill the durable attested record"
    );

    protocol.record_attested_rich("peer", &[RICH_PAYLOAD_V1]);
    feed_key_package_with_rich(&mut protocol, "peer", vec![RICH_PAYLOAD_V1]);
    assert!(protocol.peer_rich_payload.contains("peer"));
    assert!(!protocol.peer_rich_attested.contains("peer"));
    assert!(protocol.group_rich_seal_active(&members));

    // Attesting a directly-known peer is a no-op — the direct entry already
    // covers it, and a stale attested duplicate would outlive a downgrade.
    protocol.record_attested_rich("peer", &[RICH_PAYLOAD_V1]);
    assert!(!protocol.peer_rich_attested.contains("peer"));
}

#[test]
fn attested_rich_kill_switch_gates_memory_but_keeps_record() {
    // Same semantics as the direct sets: the kill switch gates recording
    // and use, not knowledge — flipping it back on restores the attestation
    // on the next run.
    let storage = Arc::new(InMemoryStorage::new());
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.rich_payload_enabled = false;
    let mut gated = OfflineProtocol::new(config).unwrap();
    gated.initialize_mls_for_test(storage.clone()).unwrap();
    gated.record_attested_rich("peer", &[RICH_PAYLOAD_V1]);
    assert!(!gated.peer_rich_attested.contains("peer"));

    let restored = protocol_with_mls_storage(storage);
    assert!(restored.peer_rich_attested.contains("peer"));
}

#[test]
fn attested_rich_unblock_clears_record() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    protocol.record_attested_rich("peer", &[RICH_PAYLOAD_V1]);

    protocol.block_user("peer").unwrap();
    protocol.unblock_user("peer").unwrap();
    assert!(!protocol.peer_rich_attested.contains("peer"));

    let restarted = protocol_with_mls_storage(storage);
    assert!(
        !restarted.peer_rich_attested.contains("peer"),
        "unblock's clean slate must extend to the attested record"
    );
}

#[test]
fn attested_rich_persist_truncates_and_merges_with_direct_record() {
    // Attestation persists raw but truncated, and merges into an existing
    // record without clobbering directly-advertised capabilities.
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage.clone());
    feed_key_package_with_caps(
        &mut protocol,
        "peer",
        vec![MLS_ENVELOPE_COMPACT_V1],
        Vec::new(),
    );

    let mut versions = vec![RICH_PAYLOAD_V1];
    versions.extend(2..=200u8);
    protocol.record_attested_rich("peer", &versions);

    let data = storage
        .load(storage_keys::PEER_CAPABILITIES, "peer")
        .unwrap()
        .expect("record must persist");
    let caps: PeerCapabilities = serde_json::from_slice(&data).unwrap();
    assert_eq!(
        caps.attested_rich_versions.len(),
        MAX_PERSISTED_CAPABILITY_VERSIONS
    );
    assert!(caps.attested_rich_versions.contains(&RICH_PAYLOAD_V1));
    assert!(
        caps.env_versions.contains(&MLS_ENVELOPE_COMPACT_V1),
        "attestation must merge into the record, not clobber direct capabilities"
    );
}

#[test]
fn attested_rich_flood_eviction_clears_at_cap() {
    // Bounded like the direct sets: keyed by ids from (signed but
    // TOFU-trusted) group control frames, so reset at capacity rather than
    // growing without bound. Forgetting costs one re-attestation/backfill.
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = protocol_with_mls_storage(storage);
    for i in 0..MAX_KEY_PACKAGE_SENT_TO {
        protocol.record_attested_rich(&format!("peer-{i}"), &[RICH_PAYLOAD_V1]);
    }
    assert_eq!(protocol.peer_rich_attested.len(), MAX_KEY_PACKAGE_SENT_TO);
    protocol.record_attested_rich("one-more", &[RICH_PAYLOAD_V1]);
    assert_eq!(
        protocol.peer_rich_attested.len(),
        1,
        "at capacity the set resets before inserting"
    );
    assert!(protocol.peer_rich_attested.contains("one-more"));
}

#[test]
fn seal_rich_payload_round_trips_through_apply_decrypted_content() {
    let extras = sample_rich_extras();
    let sealed =
        OfflineProtocol::seal_rich_payload("hello rich", &extras, ContentType::Image).unwrap();
    assert!(sealed.starts_with(internal_prefixes::RICH_V1));

    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "cipher-placeholder",
    );
    // Relay-injected outer quote: must never survive into the restored fields.
    message.reply_context = Some(offline_protocol_core::ReplyContext {
        sender: UserId::new("mallory").unwrap(),
        text: "injected quote".to_string(),
        timestamp: None,
        reply_media_label: None,
        reply_content_type: None,
    });
    OfflineProtocol::apply_decrypted_content(&mut message, sealed);

    assert_eq!(message.content, "hello rich");
    let restored = message
        .reply_context
        .expect("sealed reply context restored");
    assert_eq!(restored.sender.as_str(), "carol");
    assert_eq!(restored.text, "the quoted message");
    let media = message
        .media_metadata
        .expect("sealed media metadata restored");
    assert_eq!(media.encryption_key.as_deref(), Some("a2V5LWJ5dGVz"));
    assert_eq!(media.iv.as_deref(), Some("aXYtYnl0ZXM="));
    assert_eq!(
        media.download_url.as_deref(),
        Some("https://cdn.example/blob/1")
    );
    let forward = message
        .forwarded_from
        .expect("sealed forward info restored");
    assert_eq!(forward.original_sender.as_str(), "dave");
    assert_eq!(
        message.content_type,
        ContentType::Image,
        "sealed content_type must overwrite the relay-writable outer hint"
    );
    assert_eq!(
        message.metadata.get("encrypted").map(String::as_str),
        Some("true")
    );
}

#[test]
fn apply_decrypted_content_rich_body_is_authoritative_over_outer_fields() {
    // Sealed body carrying ONLY a reply context: the relay-writable outer
    // media/forward copies must be wiped, not trusted.
    let extras = RichSendExtras {
        reply_context: sample_rich_extras().reply_context,
        media_metadata: None,
        forward_info: None,
    };
    let sealed =
        OfflineProtocol::seal_rich_payload("bare rich", &extras, ContentType::default()).unwrap();

    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "cipher-placeholder",
    );
    message.media_metadata = Some(sample_media_metadata(7));
    message.forwarded_from = Some(offline_protocol_core::ForwardInfo {
        original_sender: UserId::new("mallory").unwrap(),
        original_message_id: MessageId::new(),
        original_timestamp: offline_protocol_core::Timestamp::now(),
        forward_count: 3,
    });
    OfflineProtocol::apply_decrypted_content(&mut message, sealed);

    assert_eq!(message.content, "bare rich");
    assert!(message.reply_context.is_some());
    assert!(
        message.media_metadata.is_none(),
        "outer media metadata must not survive a sealed rich message"
    );
    assert!(
        message.forwarded_from.is_none(),
        "outer forward info must not survive a sealed rich message"
    );
}

#[test]
fn apply_decrypted_content_rich_parse_failure_surfaces_raw_text() {
    // Malformed JSON after the prefix -> raw text fallback, nothing restored.
    let raw = format!("{}not-json", internal_prefixes::RICH_V1);
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "cipher-placeholder",
    );
    OfflineProtocol::apply_decrypted_content(&mut message, raw.clone());
    assert_eq!(message.content, raw);
    assert!(message.reply_context.is_none());
    assert_eq!(
        message.metadata.get("encrypted").map(String::as_str),
        Some("true")
    );

    // A hostile reply sender inside the sealed body fails UserId validation
    // -> same fallback: the whole body surfaces as text, never a partial
    // parse with an attacker-shaped identifier.
    let hostile = format!(
        "{}{}",
        internal_prefixes::RICH_V1,
        r#"{"text":"x","reply_context":{"sender":"evil/path","text":"q"}}"#
    );
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "cipher-placeholder",
    );
    OfflineProtocol::apply_decrypted_content(&mut message, hostile.clone());
    assert_eq!(message.content, hostile);
    assert!(message.reply_context.is_none());
}

#[test]
fn apply_decrypted_content_legacy_rich_body_keeps_outer_content_type() {
    // A body sealed by a sender predating the sealed content_type copy has
    // no such field — the outer hint must stand (backward compatibility),
    // not be reset to Text.
    let legacy = format!(
        "{}{}",
        internal_prefixes::RICH_V1,
        r#"{"text":"old sender"}"#
    );
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "cipher-placeholder",
    );
    message.content_type = ContentType::VoiceNote;
    OfflineProtocol::apply_decrypted_content(&mut message, legacy);
    assert_eq!(message.content, "old sender");
    assert_eq!(message.content_type, ContentType::VoiceNote);
}

#[test]
fn apply_decrypted_content_refuses_sealed_file_chunk_content_type() {
    // FileChunk is an internal transport type: the receive loop routes on
    // content_type AFTER decryption, so honoring a sealed FileChunk claim
    // would hand an ordinary message to the file-transfer manager, which
    // drops it. Refuse it (like the send boundary does) and keep the outer
    // value.
    let hostile = format!(
        "{}{}",
        internal_prefixes::RICH_V1,
        r#"{"text":"smuggled","content_type":"file_chunk"}"#
    );
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "cipher-placeholder",
    );
    OfflineProtocol::apply_decrypted_content(&mut message, hostile);
    assert_eq!(message.content, "smuggled");
    assert_eq!(
        message.content_type,
        ContentType::Text,
        "sealed FileChunk claim must not survive into routing"
    );
}

#[test]
fn send_message_rejects_rich_prefix_content() {
    // `__RICH_V1__` joined INTERNAL_PREFIXES: user content can never
    // impersonate a sealed rich body through the public send APIs.
    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.start().unwrap();

    let result = protocol.send_message(
        "bob",
        format!("{}fake-body", internal_prefixes::RICH_V1),
        None,
        None::<String>,
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "rich prefix injection must be rejected, got: {:?}",
        err
    );
    assert!(err.to_string().contains("reserved internal prefix"));
}

#[test]
fn send_message_with_rejects_file_chunk_content_type() {
    // FileChunk is an internal transport content type: the receiver routes
    // it into the file-transfer manager and drops it (after ACK) instead of
    // surfacing — the boundary must reject it, not silently lose the message.
    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.start().unwrap();

    let err = protocol
        .send_message_with(
            "bob",
            "hello",
            SendMessageOptions {
                content_type: Some(ContentType::FileChunk),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "FileChunk content type must be rejected, got: {:?}",
        err
    );
    assert!(err.to_string().contains("internal content type"));
}

#[test]
fn send_message_with_rejects_oversized_rich_extras() {
    // The extras cap is enforced at the boundary (before capability or
    // session state matters), so an oversized quote can never enter the
    // pending queue where a flush-time seal failure would re-queue forever.
    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.start().unwrap();

    let mut extras = sample_rich_extras();
    extras.reply_context.as_mut().unwrap().text = "q".repeat(MAX_RICH_EXTRAS_BYTES + 1);
    let err = protocol
        .send_message_with(
            "bob",
            "hello",
            SendMessageOptions {
                reply_context: extras.reply_context,
                media_metadata: extras.media_metadata,
                forward_info: extras.forward_info,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "oversized rich extras must be rejected, got: {:?}",
        err
    );
    assert!(err.to_string().contains("Rich extras too large"));
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
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
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
        initial_message: Some("hey, it's Alice from the trail".to_string()),
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
            initial_message,
        } => {
            assert_eq!(sender, "alice");
            assert_eq!(sender_name, "Alice");
            assert_eq!(*timestamp, 12345);
            assert_eq!(key_package.as_ref(), Some(&vec![9, 8, 7]));
            assert_eq!(
                initial_message.as_deref(),
                Some("hey, it's Alice from the trail")
            );
        }
        _ => panic!("Wrong event type"),
    }
}

/// A payload from a pre-initial-message sender (no `initial_message` key)
/// must still parse and surface `None` — the field is additive.
#[test]
fn test_connection_request_payload_without_initial_message_parses() {
    let legacy = r#"{"sender_name":"Alice","timestamp_ms":12345,"key_package":[9,8,7]}"#;
    let payload: ConnectionRequestPayload = serde_json::from_str(legacy).unwrap();
    assert_eq!(payload.sender_name, "Alice");
    assert_eq!(payload.initial_message, None);

    // And None round-trips without emitting the key at all, so new senders
    // stay byte-compatible with old receivers unless the feature is used.
    let serialized = serde_json::to_string(&payload).unwrap();
    assert!(!serialized.contains("initial_message"));
}

#[test]
fn test_tofu_key_mismatch_emits_security_warning_with_stable_code() {
    // Consumers gate a peer re-handshake on this exact signal, so the
    // reason_code contract must not drift. A pinned peer presenting a
    // different key (reinstall / new device / impersonation) must be rejected
    // AND emit SecurityWarning carrying a stable TOFU_KEY_MISMATCH code — not
    // merely a human-readable `reason` string that could be reworded.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    // First contact pins the key; a different key for the same peer mismatches.
    assert!(protocol.tofu_check_or_pin("alice", vec![1, 2, 3]).is_ok());
    assert!(protocol.tofu_check_or_pin("alice", vec![4, 5, 6]).is_err());

    let captured = events.lock().unwrap();
    let (peer_id, reason_code) = captured
        .iter()
        .find_map(|e| match e {
            Event::SecurityWarning {
                peer_id,
                reason_code,
                ..
            } => Some((peer_id.clone(), *reason_code)),
            _ => None,
        })
        .expect("a SecurityWarning event should have been emitted on key mismatch");
    assert_eq!(peer_id, "alice");
    assert_eq!(reason_code, SecurityWarningCode::TofuKeyMismatch);
    // The serialized code is the stable string JS consumers match on.
    assert_eq!(reason_code.as_str(), "TOFU_KEY_MISMATCH");
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

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.send_connection_request("bob", "Alice", None, None);
    assert!(result.is_ok());
}

#[test]
fn test_send_connection_request_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.send_connection_request("bob", "Alice", None, None);
    assert!(result.is_err());
}

#[test]
fn test_send_connection_request_with_key_package() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let key_package = vec![1, 2, 3, 4, 5];
    let result = protocol.send_connection_request("bob", "Alice", Some(key_package), None);
    assert!(result.is_ok());
}

/// The relay's "recipient offline" DeliveryError (bridged as a
/// `recipient_unreachable` transport failure) must surface as a typed
/// ConnectionRequestUndeliverable event — before this path existed the
/// signal was welcome-only and a connection request to an offline peer
/// produced no feedback until generic retry exhaustion.
#[test]
fn test_connection_request_recipient_unreachable_emits_undeliverable() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    protocol
        .on_transport_send_failed(
            &sent_id.as_str(),
            Some("recipient_unreachable: User is offline".to_string()),
        )
        .unwrap();

    let captured = events.lock().unwrap();
    let undeliverable = captured
        .iter()
        .find_map(|e| match e {
            Event::ConnectionRequestUndeliverable {
                recipient,
                message_id,
                reason,
            } => Some((recipient.clone(), message_id.clone(), reason.clone())),
            _ => None,
        })
        .expect("recipient_unreachable must emit ConnectionRequestUndeliverable");
    assert_eq!(undeliverable.0, "bob");
    assert_eq!(undeliverable.1, sent_id.as_str());
    assert!(undeliverable.2.starts_with("recipient_unreachable"));
    drop(captured);

    // The entry is consumed on emission: a duplicate DeliveryError for the
    // same id must not double-emit.
    protocol
        .on_transport_send_failed(
            &sent_id.as_str(),
            Some("recipient_unreachable: User is offline".to_string()),
        )
        .unwrap();
    let captured = events.lock().unwrap();
    assert_eq!(
        captured
            .iter()
            .filter(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. }))
            .count(),
        1
    );
}

/// A non-unreachable transport failure stays with the generic retry
/// machinery — the request may still deliver, so no undeliverable event.
#[test]
fn test_connection_request_generic_send_failure_does_not_emit_undeliverable() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    protocol
        .on_transport_send_failed(&sent_id.as_str(), Some("socket closed".to_string()))
        .unwrap();

    let captured = events.lock().unwrap();
    assert!(
        !captured
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "generic transport failures must not emit ConnectionRequestUndeliverable"
    );
}

/// `initial_message` is a plaintext control-frame payload, so oversized
/// input must fail loudly at the API instead of fragmenting over BLE or
/// dying at a relay frame limit after a message id was already returned.
#[test]
fn test_send_connection_request_rejects_oversized_initial_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let oversized = "x".repeat(MAX_INITIAL_MESSAGE_BYTES + 1);
    let result = protocol.send_connection_request("bob", "Alice", None, Some(oversized));
    assert!(
        matches!(result, Err(crate::Error::InvalidArgument(_))),
        "oversized initial_message must be rejected with InvalidArgument, got {:?}",
        result
    );

    // Exactly at the cap is accepted.
    let at_cap = "x".repeat(MAX_INITIAL_MESSAGE_BYTES);
    assert!(protocol
        .send_connection_request("bob", "Alice", None, Some(at_cap))
        .is_ok());
}

/// The pending-request map is bounded: past the cap the oldest entry is
/// evicted first, so a burst of requests can neither grow the map
/// unboundedly nor displace the newest (most likely still relevant) entry.
#[test]
fn test_pending_connection_request_cap_evicts_oldest_first() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let mut ids = Vec::new();
    for i in 0..=MAX_PENDING_CONNECTION_REQUESTS {
        let id = protocol
            .send_connection_request(&format!("peer{}", i), "Alice", None, None)
            .unwrap();
        ids.push(id.as_str());
    }

    assert_eq!(
        protocol.pending_connection_requests.len(),
        MAX_PENDING_CONNECTION_REQUESTS,
        "map must stay at the cap after cap+1 sends"
    );
    assert!(
        !protocol.pending_connection_requests.contains_key(&ids[0]),
        "the oldest entry must be the one evicted"
    );
    assert!(
        protocol
            .pending_connection_requests
            .contains_key(ids.last().unwrap()),
        "the newest entry must survive eviction"
    );
}

/// Proof of delivery settles the pending entry: after the request's
/// delivery ack, a late recipient_unreachable for the same id must not
/// emit ConnectionRequestUndeliverable — the recipient already has the
/// request.
#[test]
fn test_delivery_ack_settles_pending_connection_request() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    let ack = Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .metadata(ACK_FOR_KEY, sent_id.as_str())
    .build();
    protocol.handle_ack_message(&ack);
    assert!(
        !protocol
            .pending_connection_requests
            .contains_key(&sent_id.as_str()),
        "delivery ack must settle the pending connection request"
    );

    protocol
        .on_transport_send_failed(
            &sent_id.as_str(),
            Some("recipient_unreachable: User is offline".to_string()),
        )
        .unwrap();
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "a delivered request must never emit ConnectionRequestUndeliverable"
    );
}

/// An authenticated answer from the peer settles every request tracked
/// toward them (the accept/reject frame carries no request id, so
/// settlement is recipient-keyed): accept clears bob's entry while
/// carol's survives, reject clears carol's — and a late unreachable for
/// a settled id stays quiet.
#[test]
fn test_connection_answer_settles_pending_requests() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let id_bob = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();
    let id_carol = protocol
        .send_connection_request("carol", "Alice", None, None)
        .unwrap();

    protocol.handle_connection_accepted("bob", r#"{"accepted_by_name":"Bob","timestamp_ms":1}"#);
    assert!(
        !protocol
            .pending_connection_requests
            .contains_key(&id_bob.as_str()),
        "accept must settle the request tracked toward the accepter"
    );
    assert!(
        protocol
            .pending_connection_requests
            .contains_key(&id_carol.as_str()),
        "requests toward other peers must survive"
    );

    protocol.handle_connection_rejected("carol");
    assert!(
        protocol.pending_connection_requests.is_empty(),
        "reject must settle the request tracked toward the rejecter"
    );

    protocol
        .on_transport_send_failed(
            &id_bob.as_str(),
            Some("recipient_unreachable: User is offline".to_string()),
        )
        .unwrap();
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "an answered request must never emit ConnectionRequestUndeliverable"
    );
}

/// The TTL holds at read time too: the insert-path prune only runs on the
/// next send, so an idle map can hold an entry past the correlation
/// window — a recipient_unreachable that stale must drop the entry
/// without emitting.
#[test]
fn test_stale_pending_connection_request_does_not_emit_undeliverable() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    // Age the entry past the TTL. On a host whose monotonic clock started
    // more recently than the TTL no such Instant exists — skip, the
    // scenario cannot occur there either.
    let Some(stale) = std::time::Instant::now()
        .checked_sub(PENDING_CONNECTION_REQUEST_TTL + Duration::from_secs(1))
    else {
        return;
    };
    protocol
        .pending_connection_requests
        .get_mut(&sent_id.as_str())
        .unwrap()
        .sent_at = stale;

    protocol
        .on_transport_send_failed(
            &sent_id.as_str(),
            Some("recipient_unreachable: User is offline".to_string()),
        )
        .unwrap();

    assert!(
        protocol.pending_connection_requests.is_empty(),
        "a stale entry must be dropped at read time"
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "a stale entry must not emit ConnectionRequestUndeliverable"
    );
}

/// Retry exhaustion is the terminal outcome for a connection request that
/// never got an authoritative DeliveryError: the typed undeliverable event
/// must fire alongside the generic message_failed, so fast offline feedback
/// and slow retry exhaustion surface through the same typed channel.
#[test]
fn test_max_retries_settles_pending_connection_request_with_undeliverable() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    protocol.handle_max_retries_exceeded(&sent_id, 10).unwrap();

    let captured = events.lock().unwrap();
    let undeliverable = captured
        .iter()
        .find_map(|e| match e {
            Event::ConnectionRequestUndeliverable {
                recipient,
                message_id,
                reason,
            } => Some((recipient.clone(), message_id.clone(), reason.clone())),
            _ => None,
        })
        .expect("retry exhaustion on a pending connection request must emit undeliverable");
    assert_eq!(undeliverable.0, "bob");
    assert_eq!(undeliverable.1, sent_id.as_str());
    assert_eq!(undeliverable.2, "max_retries_exceeded");
    assert!(
        captured
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "the generic message_failed must still fire alongside the typed event"
    );
    drop(captured);

    assert!(
        protocol.pending_connection_requests.is_empty(),
        "retry exhaustion must settle the pending entry"
    );
}

/// An ordinary message exhausting its retries is not a connection request
/// and must produce only the generic message_failed.
#[test]
fn test_max_retries_on_ordinary_message_does_not_emit_undeliverable() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();

    protocol.handle_max_retries_exceeded(&sent_id, 10).unwrap();

    let captured = events.lock().unwrap();
    assert!(
        captured
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "retry exhaustion must emit the generic message_failed"
    );
    assert!(
        !captured
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "an ordinary message must not emit ConnectionRequestUndeliverable"
    );
}

/// A pending entry past the TTL is dropped at retry exhaustion without a
/// typed event — a signal that stale belongs to a request the app has long
/// stopped waiting on (mirrors the on_transport_send_failed behavior).
#[test]
fn test_max_retries_on_stale_pending_connection_request_does_not_emit_undeliverable() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    // Age the entry past the TTL. On a host whose monotonic clock started
    // more recently than the TTL no such Instant exists — skip, the
    // scenario cannot occur there either.
    let Some(stale) = std::time::Instant::now()
        .checked_sub(PENDING_CONNECTION_REQUEST_TTL + Duration::from_secs(1))
    else {
        return;
    };
    protocol
        .pending_connection_requests
        .get_mut(&sent_id.as_str())
        .unwrap()
        .sent_at = stale;

    protocol.handle_max_retries_exceeded(&sent_id, 10).unwrap();

    assert!(
        protocol.pending_connection_requests.is_empty(),
        "a stale entry must be dropped at retry exhaustion"
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "a stale entry must not emit ConnectionRequestUndeliverable at retry exhaustion"
    );
}

#[test]
fn test_accept_connection_request_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_cancel_connection_request_success() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let result = protocol.cancel_connection_request("bob");
    assert!(result.is_ok());
}

#[test]
fn test_cancel_connection_request_not_started() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let result = protocol.cancel_connection_request("bob");
    assert!(result.is_err());
}

#[test]
fn test_process_internal_message_connection_cancelled_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);

    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let content = internal_prefixes::CONN_CANCEL.to_string();
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
        Event::ConnectionRequestCancelled { cancelled_by } => {
            assert_eq!(cancelled_by, "carol");
        }
        _ => panic!("Wrong event type"),
    }
}

#[test]
fn test_send_connection_request_returns_unique_ids() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol.start().unwrap();

    let id1 = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();
    let id2 = protocol
        .send_connection_request("carol", "Alice", None, None)
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
            last_seen_ms,
            source,
        } => {
            assert_eq!(peer_id, "alice");
            assert_eq!(*status, PresenceStatus::Online);
            assert_eq!(*timestamp, 12345);
            assert_eq!(*last_seen_ms, None);
            assert_eq!(*source, PresenceSource::Peer);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
        None,
        ContentType::default(),
        None,
        None,
    );
    protocol.queue_pending_message(
        "bob",
        "Another message",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
        None,
    );
    protocol.queue_pending_message(
        "alice",
        "Hello Alice!",
        MessagePriority::Low,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
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
fn test_outbound_pending_queue_evicts_oldest_at_per_peer_limit() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let event_sink = events.clone();
    protocol.on_event(move |event| event_sink.lock().unwrap().push(event));
    let first_id = MessageId::new();

    for index in 0..=MAX_PENDING_MESSAGES_PER_PEER {
        protocol.queue_pending_message(
            "bob",
            &format!("message-{index}"),
            MessagePriority::Medium,
            if index == 0 {
                first_id.clone()
            } else {
                MessageId::new()
            },
            None,
            None,
            ContentType::default(),
            None,
            None,
        );
    }

    let queued = protocol.pending_encrypted_messages.get("bob").unwrap();
    assert_eq!(queued.len(), MAX_PENDING_MESSAGES_PER_PEER);
    assert!(queued.iter().all(|message| message.message_id != first_id));
    assert!(events.lock().unwrap().iter().any(|event| matches!(
        event,
        Event::MessageFailed {
            message_id,
            reason,
            retry_count: 0,
        } if message_id == &first_id.as_str()
            && reason == "Pending session queue capacity exceeded"
    )));
}

#[test]
fn test_outbound_pending_queue_enforces_global_limit() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mut original_ids = std::collections::HashSet::new();

    for index in 0..MAX_PENDING_MESSAGES_GLOBAL {
        let message_id = MessageId::new();
        original_ids.insert(message_id.as_str().to_string());
        protocol.queue_pending_message(
            &format!("peer-{}", index / MAX_PENDING_MESSAGES_PER_PEER),
            "queued",
            MessagePriority::Medium,
            message_id,
            None,
            None,
            ContentType::default(),
            None,
            None,
        );
    }
    let newest_id = MessageId::new();
    protocol.queue_pending_message(
        "overflow-peer",
        "newest",
        MessagePriority::Medium,
        newest_id.clone(),
        None,
        None,
        ContentType::default(),
        None,
        None,
    );

    assert_eq!(
        protocol.total_pending_message_count(),
        MAX_PENDING_MESSAGES_GLOBAL
    );
    let queued_ids: std::collections::HashSet<_> = protocol
        .pending_encrypted_messages
        .values()
        .flatten()
        .map(|message| message.message_id.as_str().to_string())
        .collect();
    assert!(queued_ids.contains(&newest_id.as_str()));
    assert_eq!(
        queued_ids.intersection(&original_ids).count(),
        MAX_PENDING_MESSAGES_GLOBAL - 1
    );
}

#[test]
fn test_periodic_pending_cleanup_waits_for_earliest_deadline() {
    let mut config = create_test_config();
    config.reliability.retry.pending_message_max_lifetime_ms = 10_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.queue_pending_message(
        "bob",
        "queued",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
        None,
    );

    assert!(protocol
        .next_pending_message_expiry
        .is_some_and(|expiry| expiry > Utc::now()));
    protocol.cleanup_expired_entries();
    assert!(protocol.pending_encrypted_messages.contains_key("bob"));

    protocol.pending_encrypted_messages.get_mut("bob").unwrap()[0].queued_at =
        Utc::now() - ChronoDuration::milliseconds(10_001);
    protocol.next_pending_message_expiry = Some(Utc::now() - ChronoDuration::milliseconds(1));
    protocol.cleanup_expired_entries();

    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(protocol.next_pending_message_expiry.is_none());
}

#[test]
fn test_encryption_builder_methods() {
    // Disabling encryption without also opting out of require_encryption
    // (true by default since SEC-M3) must fail validation — plaintext
    // operation requires an explicit double opt-out.
    let result = ProtocolConfig::builder("test-app", "user123")
        .encryption_enabled(false)
        .build();
    assert!(result.is_err());

    let config = ProtocolConfig::builder("test-app", "user123")
        .encryption_enabled(false)
        .require_encryption(false)
        .auto_key_exchange(true)
        .store_pending_messages(false)
        .build()
        .unwrap();

    assert!(!config.encryption.enabled);
    assert!(!config.encryption.require_encryption);
    assert!(config.encryption.auto_key_exchange);
    assert!(!config.encryption.store_pending);
}

#[test]
fn test_require_encryption_blocks_plaintext_when_mls_uninitialized() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_default_config_fails_closed_when_mls_uninitialized() {
    // Stock config, no explicit require_encryption: SEC-M3 flipped the
    // default to fail closed, so a node that never initialized MLS must
    // error instead of silently sending plaintext.
    let config = ProtocolConfig::new("test-app", "user123");
    assert!(config.encryption.require_encryption);

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_plaintext_opt_out_emits_security_warning_once_per_peer() {
    // Explicit opt-out (require_encryption=false via the test helper) with
    // MLS uninitialized: sends still leave as plaintext, and each peer gets
    // exactly one PLAINTEXT_SEND security warning regardless of message count.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let warnings: Arc<Mutex<Vec<(String, SecurityWarningCode)>>> = Arc::new(Mutex::new(Vec::new()));
    let warnings_clone = warnings.clone();
    protocol.on_event(move |event| {
        if let Event::SecurityWarning {
            peer_id,
            reason_code,
            ..
        } = event
        {
            warnings_clone.lock().unwrap().push((peer_id, reason_code));
        }
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    protocol
        .send_message("bob", "one", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .send_message("bob", "two", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .send_message("carol", "three", None::<MessagePriority>, None::<String>)
        .unwrap();

    // The opt-out path still delivers — all three reached the transport.
    assert_eq!(transport_handle.sent_messages().len(), 3);
    let warnings = warnings.lock().unwrap();
    let count_for = |peer: &str| {
        warnings
            .iter()
            .filter(|(p, c)| p == peer && *c == SecurityWarningCode::PlaintextSend)
            .count()
    };
    assert_eq!(count_for("bob"), 1);
    assert_eq!(count_for("carol"), 1);
}

#[test]
fn test_require_encryption_returns_typed_failures() {
    // With store_pending disabled, require_encryption returns typed errors
    // NoKeyPackage
    let mut no_key_config = create_test_config();
    no_key_config.encryption.require_encryption = true;
    no_key_config.encryption.store_pending = false;
    let mut no_key_protocol = OfflineProtocol::new(no_key_config).unwrap();
    let no_key_transport = MockTransport::new(TransportType::BLE);
    no_key_transport.start().unwrap();
    no_key_protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(no_key_transport));
    no_key_protocol.start().unwrap();
    no_key_protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
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
    pending_config.encryption.store_pending = false;
    let mut pending_protocol = OfflineProtocol::new(pending_config).unwrap();
    let pending_transport = MockTransport::new(TransportType::BLE);
    pending_transport.start().unwrap();
    pending_protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(pending_transport));
    pending_protocol.start().unwrap();
    pending_protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
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
    let encrypt_fail_transport = MockTransport::new(TransportType::BLE);
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
fn test_require_encryption_queues_when_store_pending_enabled() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // With store_pending=true, messages should be queued (not error) when
    // session isn't ready, even with require_encryption=true.
    let result = protocol.send_message("bob", "queued", None::<MessagePriority>, None::<String>);
    assert!(result.is_ok());
    // Message is queued, NOT sent via transport
    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, Vec::len),
        1
    );
}

#[test]
fn test_require_encryption_encrypt_failed_emits_send_error_without_transport_output() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    // Keep MLS uninitialized to force strict-mode EncryptFailed path.
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_require_encryption_queues_message_when_session_pending_with_key_package() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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
        "pending-with-key-pkg",
        None::<MessagePriority>,
        None::<String>,
    );

    // With store_pending=true, the message should be queued for later
    // delivery rather than returning an error.
    assert!(result.is_ok());
    // Queuing kicks establishment: the session is created from the pending
    // key package and the Welcome goes out immediately, so confirmation does
    // not depend on the peer contacting us first.
    assert!(protocol
        .mls_manager
        .as_ref()
        .unwrap()
        .read()
        .unwrap()
        .has_session("bob")
        .unwrap());
    // Two control messages hit the wire: the Welcome from the establishment
    // kick, then the confirmation probe from the post-queue reconciliation.
    let sent = transport_handle.sent_messages();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].content.starts_with(internal_prefixes::WELCOME));
    assert!(sent[1]
        .content
        .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE));
    // The user message itself is queued, never on the wire.
    assert!(protocol.pending_encrypted_messages.contains_key("bob"));
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, Vec::len),
        1
    );
}

#[test]
fn test_require_encryption_queues_for_send_message_via_transport() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_message_via_transport(
        "bob",
        "queued-via-transport",
        None::<MessagePriority>,
        TransportType::BLE,
        None::<String>,
    );

    // With store_pending=true, message should be queued, not sent plaintext
    assert!(result.is_ok());
    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, Vec::len),
        1
    );
}

#[test]
fn test_require_encryption_pending_flush_encrypts_and_delivers() {
    // End-to-end: message queued during session establishment with
    // require_encryption + store_pending, then flushed after session
    // confirmation — the flushed message must be encrypted.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Step 1: Send message before session exists — should be queued.
    let result = protocol.send_message(
        "bob",
        "hello encrypted",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(result.is_ok());
    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, Vec::len),
        1
    );

    // Step 2: Create MLS session (import key package + create session).
    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = crate::mls::MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.generate_key_package().unwrap();
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }

    // Step 3: Confirm the session.
    protocol.confirm_session_state("bob", "test_setup").unwrap();

    // Step 4: Flush pending messages — they should now encrypt and send.
    protocol.flush_pending_messages("bob").unwrap();

    // Pending queue should be drained.
    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));

    // The message should have been sent via transport (encrypted).
    let sent = transport_handle.sent_messages();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].content.starts_with(internal_prefixes::ENCRYPTED),
        "Flushed message must be encrypted, got: {}",
        &sent[0].content[..sent[0].content.len().min(60)]
    );
}

#[test]
fn test_flush_pending_message_keeps_queued_id() {
    // The id returned to the caller at queue time (SessionNotReady) must be
    // the id the message eventually dispatches under — MessageDeferred and
    // the later MessageSent/Delivered/Failed correlate through it.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let queued_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(transport_handle.sent_messages().len(), 0);

    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = crate::mls::MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.generate_key_package().unwrap();
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();

    protocol.flush_pending_messages("bob").unwrap();

    let sent = transport_handle.sent_messages();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].id, queued_id,
        "Flushed message must keep the id returned at queue time"
    );
    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
}

#[test]
fn test_flush_requeue_keeps_queued_id_and_storage() {
    // A flush attempted while the session is still not ready re-queues the
    // message — under the SAME id, and without clearing the persisted copy.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let queued_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    let second_id = protocol
        .send_message("bob", "world", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Session never established: flush re-queues rather than sending.
    protocol.flush_pending_messages("bob").unwrap();

    assert_eq!(transport_handle.sent_messages().len(), 0);
    let pending = protocol.pending_encrypted_messages.get("bob").unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(
        pending[0].message_id, queued_id,
        "Re-queued pending message must keep its original id"
    );
    assert_eq!(
        pending[1].message_id, second_id,
        "Re-queue must preserve the original send order"
    );
    let persisted = protocol
        .load_pending_messages_from_storage("bob")
        .expect("pending messages must remain persisted after a re-queue");
    assert_eq!(persisted.len(), 2);
    assert_eq!(
        persisted[0].message_id, queued_id,
        "Persisted snapshot must keep the original id across a re-queue"
    );
    assert_eq!(
        persisted[1].message_id, second_id,
        "Persisted snapshot must preserve the original send order"
    );
}

/// Builds a protocol whose sends always land in the pending-session queue
/// (encryption required, `store_pending` on, no session ever established), with
/// its MLS + protocol-state storage wired up.
fn pending_queue_protocol() -> OfflineProtocol {
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
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();
    protocol
}

#[test]
fn test_protocol_state_backing_bytes_hide_plaintext_and_media_keys() {
    // The install-scoped container gets the delivery state's LIFECYCLE, not a
    // free pass on its confidentiality: pending entries carry message
    // plaintext and cloud-media key material, which used to sit behind the
    // credential store. Whatever a provider is handed must be opaque.
    const SENTINEL_TEXT: &str = "sentinel-plaintext-payload";
    const SENTINEL_KEY: &str = "c2VudGluZWwtbWVkaWEta2V5";
    const SENTINEL_IV: &str = "c2VudGluZWwtbWVkaWEtaXY=";

    let mut protocol = pending_queue_protocol();
    protocol
        .send_message_with(
            "bob",
            SENTINEL_TEXT,
            SendMessageOptions {
                media_metadata: Some(offline_protocol_core::MediaMetadata {
                    encryption_key: Some(SENTINEL_KEY.to_string()),
                    iv: Some(SENTINEL_IV.to_string()),
                    ..sample_media_metadata(1024)
                }),
                ..Default::default()
            },
        )
        .unwrap();

    let state_storage = protocol.protocol_state_storage.clone().unwrap();
    let stored = state_storage
        .load(storage_keys::PENDING_MESSAGES, "bob")
        .unwrap()
        .expect("the pending message must have been persisted");

    for sentinel in [SENTINEL_TEXT, SENTINEL_KEY, SENTINEL_IV] {
        assert!(
            !stored
                .windows(sentinel.len())
                .any(|window| window == sentinel.as_bytes()),
            "backing bytes must not expose {sentinel:?}"
        );
    }

    // Opaque, but not lost: the SDK still reads its own record back.
    let restored = protocol
        .load_pending_messages_from_storage("bob")
        .expect("the sealed record must open for the SDK");
    assert_eq!(restored[0].content, SENTINEL_TEXT);
    assert_eq!(
        restored[0]
            .rich
            .as_ref()
            .and_then(|rich| rich.media_metadata.as_ref())
            .and_then(|media| media.encryption_key.as_deref()),
        Some(SENTINEL_KEY)
    );
}

#[test]
fn test_sensitive_state_is_not_persisted_in_the_clear_without_the_record_key() {
    // Fail closed. If the per-install record key is unavailable, sensitive
    // categories are simply not persisted — losing crash recovery is
    // recoverable, losing at-rest confidentiality is not.
    let mut protocol = pending_queue_protocol();
    protocol.state_record_cipher = None;

    let state_storage = protocol.protocol_state_storage.clone().unwrap();
    let result = protocol.write_state_record(
        state_storage.as_ref(),
        storage_keys::PENDING_MESSAGES,
        "bob",
        b"sentinel-plaintext-payload",
    );
    assert!(
        matches!(result, Err(crate::ProtocolStateError::StoreFailed(_))),
        "a sealed category must refuse to write without the key, got {result:?}"
    );
    assert!(
        state_storage
            .load(storage_keys::PENDING_MESSAGES, "bob")
            .unwrap()
            .is_none(),
        "nothing may reach the store in the clear"
    );

    // A non-sensitive category is unaffected — the key gates confidentiality,
    // not persistence in general.
    protocol
        .write_state_record(
            state_storage.as_ref(),
            storage_keys::SESSION_STATES,
            "bob",
            b"\"Confirmed\"",
        )
        .expect("non-sealed categories must still persist");
}

#[test]
fn test_sealed_records_do_not_open_under_a_rotated_record_key() {
    // A record key that could not be reused (corrupt blob, restored backup)
    // must not leave undecryptable records behind forever: they fail to open
    // and are dropped on read.
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    seed_sealed_state_record(&storage, storage_keys::PENDING_MESSAGES, "bob", b"[]");

    // Rotate the key out from under the record.
    storage
        .store(
            storage_keys::STATE_RECORD_KEY,
            storage_keys::STATE_RECORD_KEY_ID,
            &*state_crypto::StateRecordCipher::generate_key(),
        )
        .unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(
        storage
            .load(storage_keys::PENDING_MESSAGES, "bob")
            .unwrap()
            .is_none(),
        "an unopenable record must be dropped rather than retried every boot"
    );
}

#[test]
fn test_flush_terminal_welcome_failure_drops_pending_without_resurrection() {
    // A terminal Welcome failure surfacing mid-flush runs
    // abort_pending_session_for_peer (via the prepare path), which clears
    // the queue and its storage and settles the peer with
    // secure_session_failed. The flush's failure merge must not re-insert
    // (resurrect) what the abort just settled.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    protocol
        .send_message("bob", "first", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .send_message("bob", "second", None::<MessagePriority>, None::<String>)
        .unwrap();

    // The Welcome for bob goes terminal before the flush runs.
    protocol.welcome_lifecycles.insert(
        "bob".to_string(),
        WelcomeLifecycleRecord {
            peer_id: "bob".to_string(),
            group_id: "session:user123:bob".to_string(),
            state: WelcomeDeliveryState::Expired,
            attempt: 1,
            unreachable_parks: 0,
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
            expires_at: Utc::now() - ChronoDuration::seconds(1),
        },
    );

    protocol.flush_pending_messages("bob").unwrap();

    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert!(
        !protocol.pending_encrypted_messages.contains_key("bob"),
        "Flush must not resurrect messages the session abort settled"
    );
    assert!(
        protocol.load_pending_messages_from_storage("bob").is_none(),
        "The aborted queue must stay cleared in storage"
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::SecureSessionFailed { .. })),
        "The abort settles the peer with secure_session_failed"
    );
}

#[test]
fn test_flush_drops_pending_for_blocked_recipient() {
    // A recipient blocked after messages were queued: flush drops the queue
    // (memory + storage) instead of retrying into UserBlocked forever, and
    // settles each held id with a terminal MessageFailed.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let queued_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, Vec::len),
        1
    );

    protocol.block_user("bob").unwrap();
    protocol.flush_pending_messages("bob").unwrap();

    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert!(!protocol.pending_encrypted_messages.contains_key("bob"));
    assert!(
        protocol.load_pending_messages_from_storage("bob").is_none(),
        "Persisted pending messages must be cleared for a blocked recipient"
    );
    let captured = events.lock().unwrap();
    assert!(
        captured.iter().any(|event| matches!(
            event,
            Event::MessageFailed { message_id, reason, .. }
                if *message_id == queued_id.as_str() && reason == "Recipient blocked"
        )),
        "Dropped pending message must settle with a terminal MessageFailed"
    );
}

#[test]
fn test_flush_dedup_hit_drops_already_dispatched_message() {
    // With stable ids, a message whose id is already in the deduplicator
    // (e.g. stale storage snapshot restored after the send went out) is
    // dropped at flush rather than dispatched again or retried forever.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let queued_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol.deduplicator.mark_seen(queued_id.clone());

    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = crate::mls::MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.generate_key_package().unwrap();
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();

    protocol.flush_pending_messages("bob").unwrap();

    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert!(
        !protocol.pending_encrypted_messages.contains_key("bob"),
        "Dedup-hit message must be dropped, not re-queued"
    );
    // Exact-mode hit is authoritative: the id was already settled when it
    // first dispatched, so no contradictory terminal event is emitted.
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, Event::MessageFailed { .. })),
        "Exact-mode dedup drop must not re-settle an already-settled id"
    );
}

#[test]
fn test_flush_bloom_dedup_hit_settles_with_message_failed() {
    // In bloom mode a dedup hit may be a ~1% false positive for a message
    // that never went out. The flush still drops it (dispatching would
    // error-loop on the same filter) but must settle the id with a terminal
    // MessageFailed so the app can resend instead of losing it silently.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;
    config.reliability.dedup.use_bloom_filter = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let queued_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol.deduplicator.mark_seen(queued_id.clone());
    assert!(!protocol.deduplicator.is_exact());

    protocol.flush_pending_messages("bob").unwrap();

    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert!(
        !protocol.pending_encrypted_messages.contains_key("bob"),
        "Bloom dedup-hit message must be dropped, not re-queued"
    );
    let captured = events.lock().unwrap();
    assert!(
        captured.iter().any(|event| matches!(
            event,
            Event::MessageFailed { message_id, .. } if *message_id == queued_id.as_str()
        )),
        "Bloom-mode dedup drop must settle the id with a terminal MessageFailed"
    );
}

#[test]
fn test_message_deferred_includes_next_retry_at() {
    // The deferred event must carry the retry queue's actual schedule, not
    // a None the app has to guess around.
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    let before_ms = chrono::Utc::now().timestamp_millis();
    protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();

    let captured = events.lock().unwrap();
    let deferred = captured
        .iter()
        .find_map(|event| match event {
            Event::MessageDeferred { next_retry_at, .. } => Some(*next_retry_at),
            _ => None,
        })
        .expect("MessageDeferred must be emitted for a failed send");
    let next_retry_at = deferred.expect("next_retry_at must be populated from the retry queue");
    assert!(
        next_retry_at >= before_ms + 60_000,
        "next_retry_at {} must reflect the 60s backoff (now {})",
        next_retry_at,
        before_ms
    );
}

#[test]
fn test_retry_failure_emits_message_retrying() {
    // Every re-schedule after a failed retry attempt surfaces as a
    // MessageRetrying event with the new count and the queue's schedule.
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 1;
    config.reliability.retry.max_delay_ms = 1;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.retry_queue_size(), 1);

    // 1ms backoff: wait it out so the entry is ready; the retry fails
    // again and must re-schedule loudly.
    std::thread::sleep(std::time::Duration::from_millis(10));
    protocol.process_retry_queue().unwrap();

    let captured = events.lock().unwrap();
    let retrying = captured
        .iter()
        .find_map(|event| match event {
            Event::MessageRetrying {
                message_id: id,
                recipient,
                retry_count,
                next_retry_at,
            } => Some((id.clone(), recipient.clone(), *retry_count, *next_retry_at)),
            _ => None,
        })
        .expect("MessageRetrying must be emitted when a retry attempt fails");
    assert_eq!(retrying.0, message_id.as_str());
    assert_eq!(retrying.1, "bob");
    assert_eq!(retrying.2, 1);
    assert!(retrying.3 > 0);
}

#[test]
fn test_unreachable_relay_verdict_emits_message_undeliverable_non_terminal() {
    // The relay's recipient_unreachable DeliveryError for a plain DM used
    // to die in a silent `return Ok(())`. It must surface as a
    // non-terminal MessageUndeliverable: event out, outbox entry intact.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert!(protocol.outbox.contains_key(&message_id));

    // Non-unreachable reasons stay with the generic retry machinery.
    protocol
        .on_transport_send_failed(&message_id.as_str(), Some("timeout".to_string()))
        .unwrap();
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageUndeliverable { .. })),
        "Non-unreachable failure must not emit MessageUndeliverable"
    );

    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    let captured = events.lock().unwrap();
    let undeliverable = captured
        .iter()
        .find_map(|event| match event {
            Event::MessageUndeliverable {
                message_id: id,
                recipient,
                reason,
                file_id,
            } => Some((
                id.clone(),
                recipient.clone(),
                reason.clone(),
                file_id.clone(),
            )),
            _ => None,
        })
        .expect("MessageUndeliverable must be emitted for an unreachable DM recipient");
    assert_eq!(undeliverable.0, message_id.as_str());
    assert_eq!(undeliverable.1, "bob");
    assert!(undeliverable.2.starts_with("recipient_unreachable"));
    assert_eq!(undeliverable.3, None);
    drop(captured);

    // Non-terminal: the outbox entry survives, but the message is parked —
    // no pending ACK left to burn max_retries against an offline peer.
    assert!(
        protocol.outbox.contains_key(&message_id),
        "MessageUndeliverable must not settle the outbox entry"
    );
    assert!(
        !protocol.ack_manager.is_waiting_for_ack(&message_id),
        "Unreachable verdict must drop the pending ACK (park)"
    );
    // A BLE mock transport is up, so the park keeps a timed mesh
    // reachability probe instead of going edge-only.
    assert!(
        protocol.retry_queue.contains(&message_id.as_str()),
        "Mesh carrier present: park must schedule a reachability probe"
    );
}

#[test]
fn test_unreachable_verdict_without_outbox_entry_is_dropped() {
    // No outbox entry means no recipient to attribute — the verdict is
    // logged and dropped, never emitted with a made-up recipient.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    protocol
        .on_transport_send_failed(
            &MessageId::new().as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageUndeliverable { .. })),
        "Unattributable unreachable verdict must not emit an event"
    );
}

#[test]
fn test_unreachable_media_chunk_resolves_file_id() {
    // A media chunk's unreachable verdict carries the owning transfer's
    // file_id so the app can correlate it to the media send.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Plant a media outbox entry + chunk mapping directly (a full MLS
    // media transfer setup is exercised elsewhere; this test targets the
    // lookup wiring).
    let chunk_message = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "chunk-bytes",
    );
    let chunk_id = chunk_message.id.clone();
    protocol.media_outbox.insert(
        chunk_id.clone(),
        OutboxEntry {
            message: chunk_message,
            attempt_count: 1,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: Some(TransportType::BLE),
            reseal: None,
        },
    );
    protocol
        .outbound_media_chunks
        .insert(chunk_id.clone(), ("file_abc".to_string(), 3));

    protocol
        .on_transport_send_failed(
            &chunk_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    let captured = events.lock().unwrap();
    let file_id = captured
        .iter()
        .find_map(|event| match event {
            Event::MessageUndeliverable { file_id, .. } => Some(file_id.clone()),
            _ => None,
        })
        .expect("MessageUndeliverable must fire for an unreachable media chunk");
    assert_eq!(file_id.as_deref(), Some("file_abc"));
}

#[test]
fn test_unreachable_dm_internet_only_parks_with_probe() {
    // Internet-only carrier (the default configuration for app users): the
    // relay's unreachable verdict parks the DM — pending ACK dropped so the
    // timeout loop can never burn max_retries into a terminal message_failed
    // against a provably offline peer — but the park still schedules an
    // escalating reachability probe. Without it the message would have NO
    // self-recovery at all, leaving delivery hostage to an external edge
    // (presence rotation / reconnect), which is what produced minutes-long
    // delivery delays in the field.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert!(protocol.ack_manager.is_waiting_for_ack(&message_id));

    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    assert!(
        protocol.outbox.contains_key(&message_id),
        "Parked DM must stay in the outbox"
    );
    assert!(
        !protocol.ack_manager.is_waiting_for_ack(&message_id),
        "Park must drop the pending ACK"
    );
    assert!(
        protocol.retry_queue.contains(&message_id.as_str()),
        "Internet-only park must still schedule a reachability probe"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&1),
        "The park counter must be armed on every carrier — it is what makes \
         probe exhaustion re-parkable instead of terminal"
    );

    // The probe is scheduled, not immediate: the tick loop has nothing to
    // settle terminally in the meantime.
    protocol.process().unwrap();
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "A parked DM must not settle as message_failed"
    );
    assert!(protocol.outbox.contains_key(&message_id));
}

#[test]
fn test_unreachable_dm_internet_only_probe_escalates_and_resets_on_edge() {
    // The internet-only probe shares the mesh ladder: each consecutive
    // verdict doubles the interval (15s -> 30s, 600s cap) so a peer that
    // stays offline cannot become an unbounded resend loop into the relay,
    // and any reachability edge resets the escalation.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();

    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));
    let first_delay = protocol
        .retry_queue
        .time_until_next_retry()
        .expect("first park must schedule a probe");
    assert!(first_delay > chrono::Duration::seconds(10));
    assert!(first_delay <= chrono::Duration::seconds(15));

    // Second consecutive verdict doubles the interval (15s -> 30s).
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&2));
    let second_delay = protocol
        .retry_queue
        .time_until_next_retry()
        .expect("second park must re-schedule the probe");
    assert!(second_delay > chrono::Duration::seconds(20));
    assert!(second_delay <= chrono::Duration::seconds(30));

    // A relay presence-online answer is the internet-only reachability edge
    // (the mesh path uses on_neighbor_discovered): it re-drives the parked DM
    // and resets the escalation counter.
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        None,
        "A presence-online edge must reset the escalation"
    );
}

#[test]
fn test_unreachable_dm_internet_only_reemits_undeliverable_per_verdict() {
    // Deliberate contract: unlike a mesh probe (which can never earn a relay
    // verdict), an internet probe re-earns a DeliveryError while the peer
    // stays offline, so MessageUndeliverable repeats — once per verdict. It
    // is a repeatable status signal, never a terminal one; the terminal
    // signal remains MessageFailed, which must NOT fire here.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();

    for _ in 0..2 {
        protocol
            .on_transport_send_failed(
                &message_id.as_str(),
                Some("recipient_unreachable: peer offline".to_string()),
            )
            .unwrap();
    }

    let events = events.lock().unwrap();
    let undeliverable = events
        .iter()
        .filter(|e| matches!(e, Event::MessageUndeliverable { .. }))
        .count();
    assert_eq!(
        undeliverable, 2,
        "Each verdict must re-emit the non-terminal undeliverable signal"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "Repeated verdicts must never settle the message terminally"
    );
}

#[test]
fn test_internet_only_probe_exhaustion_reparks_not_terminal() {
    // The hoisted counter is what arms try_repark_exhausted_dm on this
    // branch. A probe that succeeds locally but is never answered burns the
    // ACK budget; with a live counter that exhaustion must re-park (keeping
    // the outbox entry) rather than settle terminally — otherwise unifying
    // the branches would have traded a stalled message for a failed one.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));

    // Drive the exhaustion path directly: the probe's ACK budget ran out.
    protocol
        .handle_max_retries_exceeded(&message_id, 10)
        .unwrap();

    assert!(
        protocol.outbox.contains_key(&message_id),
        "Exhaustion with a live park counter must re-park, not evict"
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "Re-parked exhaustion must not surface a terminal message_failed"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&2),
        "The re-park escalates the interval like any other park"
    );
}

#[test]
fn test_delivery_ack_redrives_the_peers_other_parked_dms() {
    // The park counter is per-peer while the probes are per-message, so a
    // burst of DMs to an offline peer escalates the shared ladder once per
    // park: message 1 probes at 15s but message 3 is already scheduled at 60s,
    // and a burst of seven lands the last at the 600s cap. Clearing the
    // counter on delivery is therefore not enough on its own — delivery of any
    // one of them proves the rest can go NOW, and on a consumer that never
    // polls presence this ACK is the only edge that will ever say so.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    let internet_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let ids: Vec<_> = (0..3)
        .map(|i| {
            protocol
                .send_message(
                    "bob",
                    &format!("hello {}", i),
                    None::<MessagePriority>,
                    None::<String>,
                )
                .unwrap()
        })
        .collect();
    for id in &ids {
        protocol
            .on_transport_send_failed(
                &id.as_str(),
                Some("recipient_unreachable: peer offline".to_string()),
            )
            .unwrap();
    }
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&3),
        "each park escalates the shared per-peer ladder"
    );
    for id in &ids {
        assert!(
            !protocol.ack_manager.is_waiting_for_ack(id),
            "a parked DM holds no pending ACK"
        );
    }

    // Message 0's probe fires and registers its ACK (what the retry queue does
    // on a probe send); the peer answers it. The ACK deliberately carries no
    // ACK_TRANSPORT_KEY label: the re-drive override must come from the
    // sender's own record of the delivered send's transport
    // (`OutboxEntry::last_transport` — Internet here, and available), never
    // from the peer-supplied label, which decodes any absent or unknown value
    // to BLE and would let a peer steer the sibling sends.
    let probed = protocol.outbox.get(&ids[0]).unwrap().message.clone();
    protocol.ensure_ack_registration(&probed).unwrap();
    let sends_before = internet_handle.sent_messages().len();

    let ack = Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .metadata(ACK_FOR_KEY, ids[0].as_str())
    .build();
    protocol.handle_ack_message(&ack);

    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        None,
        "delivery proves reachability: the escalation starts over"
    );
    assert!(
        internet_handle.sent_messages().len() > sends_before,
        "the siblings must go out now, not wait out their escalated timers"
    );
    for id in &ids[1..] {
        assert!(
            protocol.ack_manager.is_waiting_for_ack(id),
            "each sibling must be re-driven with a fresh ACK budget"
        );
        assert!(
            protocol.outbox.contains_key(id),
            "re-driving must not settle the sibling"
        );
    }
}

#[test]
fn test_delivery_ack_redrive_ignores_peer_label_and_falls_back_to_dors() {
    // Two hostile-to-override conditions at once: the delivering transport
    // recorded on the outbox entry is gone by ACK time (last_transport = BLE,
    // never attached), and the ACK carries a garbage ACK_TRANSPORT_KEY label
    // (peer-supplied; decodes to BLE). The re-drive must ignore the label
    // entirely and, with its own last_transport unavailable, fall back to
    // DORS — a missing carrier costs routing freedom, never the re-drive.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    let internet_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let ids: Vec<_> = (0..2)
        .map(|i| {
            protocol
                .send_message(
                    "bob",
                    &format!("hello {}", i),
                    None::<MessagePriority>,
                    None::<String>,
                )
                .unwrap()
        })
        .collect();
    for id in &ids {
        protocol
            .on_transport_send_failed(
                &id.as_str(),
                Some("recipient_unreachable: peer offline".to_string()),
            )
            .unwrap();
    }
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&2));

    // The carrier that delivered message 0 has since dropped.
    protocol.outbox.get_mut(&ids[0]).unwrap().last_transport = Some(TransportType::BLE);

    let probed = protocol.outbox.get(&ids[0]).unwrap().message.clone();
    protocol.ensure_ack_registration(&probed).unwrap();
    let sends_before = internet_handle.sent_messages().len();

    let ack = Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .metadata(ACK_FOR_KEY, ids[0].as_str())
    .metadata(crate::constants::ACK_TRANSPORT_KEY, "garbage")
    .build();
    protocol.handle_ack_message(&ack);

    assert_eq!(protocol.dm_unreachable_parks.get("bob"), None);
    assert!(
        internet_handle.sent_messages().len() > sends_before,
        "the sibling must be re-driven over DORS's pick, not stranded on a \
         carrier this device does not have"
    );
    assert!(
        protocol.ack_manager.is_waiting_for_ack(&ids[1]),
        "the sibling must be re-driven with a fresh ACK budget"
    );
}

#[test]
fn test_unreachable_dm_mesh_probe_escalates_and_resets_on_edge() {
    // With a local mesh carrier up the peer may be a room away, so the park
    // keeps a timed reachability probe whose interval escalates per
    // consecutive unreachable park and resets on a reachability edge.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();

    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));
    let first_delay = protocol
        .retry_queue
        .time_until_next_retry()
        .expect("first park must schedule a probe");
    assert!(first_delay > chrono::Duration::seconds(10));
    assert!(first_delay <= chrono::Duration::seconds(15));

    // Second consecutive park doubles the interval (15s -> 30s).
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&2));
    let second_delay = protocol
        .retry_queue
        .time_until_next_retry()
        .expect("second park must re-schedule the probe");
    assert!(second_delay > chrono::Duration::seconds(20));
    assert!(second_delay <= chrono::Duration::seconds(30));

    // A per-peer reachability edge resets the escalation counter (the BLE
    // mock accepts the re-driven send, so the edge genuinely takes).
    protocol.on_neighbor_discovered("bob");
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), None);
}

#[test]
fn test_unreachable_media_chunk_is_not_parked() {
    // Media chunks keep the normal retry machinery: their offline story is
    // retry exhaustion -> transfer abort -> persisted descriptor ->
    // MediaResendRequired with app-resupplied bytes. Parking would strand
    // chunk bytes in memory for the outbox lifetime instead.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let chunk_message = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "chunk-bytes",
    );
    let chunk_id = chunk_message.id.clone();
    protocol.media_outbox.insert(
        chunk_id.clone(),
        OutboxEntry {
            message: chunk_message,
            attempt_count: 1,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: Some(TransportType::Internet),
            reseal: None,
        },
    );
    protocol
        .ack_manager
        .register_pending_ack(chunk_id.clone(), None)
        .unwrap();

    protocol
        .on_transport_send_failed(
            &chunk_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    assert!(
        protocol.ack_manager.is_waiting_for_ack(&chunk_id),
        "Media chunk must keep its pending ACK (not parked)"
    );
    assert!(protocol.media_outbox.contains_key(&chunk_id));
    assert!(protocol.dm_unreachable_parks.is_empty());
}

#[test]
fn test_parked_dm_redriven_with_fresh_ack_budget() {
    // Every re-drive registers a fresh pending ACK, so the retry budget
    // only ever burns against a peer believed reachable — attempts spent
    // before the peer went offline don't count against post-recovery
    // delivery.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Simulate a partly burned budget before the unreachable verdict.
    protocol.ack_manager.increment_retry_count(&message_id);
    protocol.ack_manager.increment_retry_count(&message_id);
    protocol.ack_manager.increment_retry_count(&message_id);
    assert_eq!(
        protocol
            .ack_manager
            .get_pending_ack(&message_id)
            .unwrap()
            .retry_count,
        3
    );

    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert!(!protocol.ack_manager.is_waiting_for_ack(&message_id));

    // Carrier-level reachability edge (e.g. internet reconnect).
    protocol.flush_outbox_all();

    let pending = protocol
        .ack_manager
        .get_pending_ack(&message_id)
        .expect("re-driven DM must be ACK-tracked again");
    assert_eq!(
        pending.retry_count, 0,
        "Re-drive must start with a fresh ACK retry budget"
    );
}

#[test]
fn test_parked_dm_redriven_on_peer_presence_online() {
    // The relay's presence-online answer is the un-park edge for peers
    // watched via CheckPresence: it must immediately re-drive the parked
    // DM.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert!(!protocol.ack_manager.is_waiting_for_ack(&message_id));

    protocol.on_peer_presence("bob", true, None);

    assert!(
        protocol.ack_manager.is_waiting_for_ack(&message_id),
        "Presence-online must re-drive the parked DM"
    );
}

#[test]
fn test_presence_watch_peers_includes_parked_dm_recipients() {
    // The SDK owns the "watch my DeliveryError recipients" duty: parked
    // (and generally non-in-flight) outbox recipients join the CheckPresence
    // watchlist, while a recipient whose message is in flight (awaiting
    // ACK) is not watched.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::Internet);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(mock_transport));
    protocol.start().unwrap();

    let parked_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .send_message("carol", "hi", None::<MessagePriority>, None::<String>)
        .unwrap();

    protocol
        .on_transport_send_failed(
            &parked_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    let watchlist = protocol.presence_watch_peers();
    assert!(
        watchlist.contains(&"bob".to_string()),
        "Parked DM recipient must be presence-watched"
    );
    assert!(
        !watchlist.contains(&"carol".to_string()),
        "In-flight (ACK-pending) recipient is not watched"
    );
}

/// Parks a DM to "bob" over a BLE+Internet dual-carrier setup, then puts it
/// in the probe-hostage state: the mesh reachability probe has fired (send
/// locally succeeded into the mesh, fresh pending ACK registered) and its
/// retry-queue entry is consumed. Returns the message id plus both mock
/// handles for send-capture assertions.
fn park_and_fire_probe(
    protocol: &mut OfflineProtocol,
) -> (MessageId, MockTransport, MockTransport) {
    let internet_mock = MockTransport::new(TransportType::Internet);
    internet_mock.start().unwrap();
    let ble_mock = MockTransport::new(TransportType::BLE);
    ble_mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet_mock.clone()));
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble_mock.clone()));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));

    // The probe fired: its retry entry is consumed and the mesh send
    // "succeeded" locally, re-registering a pending ACK no mesh peer will
    // ever answer.
    protocol.retry_queue.remove(&message_id.as_str());
    protocol
        .ack_manager
        .register_pending_ack(message_id.clone(), None)
        .unwrap();
    assert!(protocol.ack_manager.is_waiting_for_ack(&message_id));

    (message_id, internet_mock, ble_mock)
}

#[test]
fn test_probe_hostage_dm_redriven_on_peer_presence_online() {
    // The probe-hostage regression: a parked DM whose mesh probe is
    // awaiting an unanswerable ACK must still be re-driven by the relay's
    // presence-online answer — the awaiting-ACK flush filter must not hold
    // it hostage through the edge — and the re-drive must go out over the
    // internet transport (the reachability proof is relay-scoped; DORS
    // could route it back into the mesh void).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (message_id, internet_mock, ble_mock) = park_and_fire_probe(&mut protocol);

    // Burn some probe budget so the fresh-ACK assertion below is
    // distinguishable from the hostage ACK surviving.
    protocol.ack_manager.increment_retry_count(&message_id);
    protocol.ack_manager.increment_retry_count(&message_id);
    let internet_sent_before = internet_mock.sent_messages().len();
    let ble_sent_before = ble_mock.sent_messages().len();

    protocol.on_peer_presence("bob", true, None);

    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        None,
        "Presence-online is a reachability edge: the park counter resets"
    );
    let pending = protocol
        .ack_manager
        .get_pending_ack(&message_id)
        .expect("re-driven DM must be ACK-tracked again");
    assert_eq!(
        pending.retry_count, 0,
        "The hostage probe ACK must be canceled and replaced by a fresh one"
    );
    let internet_sent = internet_mock.sent_messages();
    assert_eq!(
        internet_sent.len(),
        internet_sent_before + 1,
        "The un-park re-drive must go out over the internet transport"
    );
    assert_eq!(internet_sent.last().unwrap().id, message_id);
    assert_eq!(
        ble_mock.sent_messages().len(),
        ble_sent_before,
        "The un-park re-drive must not be routed into the mesh"
    );
}

#[test]
fn test_probe_hostage_redrive_reparks_on_second_verdict() {
    // Loop continuity: if the presence-online re-drive earns another
    // unreachable verdict (stale presence answer), the DM re-parks with a
    // fresh escalation instead of settling terminally.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let (message_id, _internet_mock, _ble_mock) = park_and_fire_probe(&mut protocol);
    protocol.on_peer_presence("bob", true, None);
    assert!(protocol.ack_manager.is_waiting_for_ack(&message_id));

    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();

    assert!(
        !protocol.ack_manager.is_waiting_for_ack(&message_id),
        "Second verdict must re-park (drop the pending ACK)"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&1),
        "Escalation restarts after the presence edge reset the counter"
    );
    assert!(protocol.outbox.contains_key(&message_id));
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "The park loop must stay non-terminal"
    );
}

#[test]
fn test_unpark_cancel_spares_connection_request_ack() {
    // The un-park ACK cancel is scoped to parkable plain DMs: an in-flight
    // connection request to the same peer keeps its pending ACK (and its
    // typed terminal path) across the presence edge.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (dm_id, _internet_mock, _ble_mock) = park_and_fire_probe(&mut protocol);

    let conn_request = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__CONN_REQ__:{}",
    );
    let conn_id = conn_request.id.clone();
    protocol.outbox.insert(
        conn_id.clone(),
        OutboxEntry {
            message: conn_request,
            attempt_count: 1,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: Some(TransportType::Internet),
            reseal: None,
        },
    );
    protocol
        .ack_manager
        .register_pending_ack(conn_id.clone(), None)
        .unwrap();
    protocol.pending_connection_requests.insert(
        conn_id.as_str(),
        PendingConnectionRequest {
            recipient: "bob".to_string(),
            sent_at: std::time::Instant::now(),
        },
    );

    protocol.on_peer_presence("bob", true, None);

    assert!(
        protocol.ack_manager.is_waiting_for_ack(&conn_id),
        "Connection-request ACK must survive the un-park cancel"
    );
    assert!(
        protocol.ack_manager.is_waiting_for_ack(&dm_id),
        "The parked DM itself must be re-driven with a fresh ACK"
    );
}

#[test]
fn test_probe_hostage_dm_redriven_on_flush_outbox_all() {
    // The carrier-level edge (reconnect / start) has the same hostage
    // window: a probe-ACK-waiting DM must be canceled and re-driven, not
    // skipped by the awaiting-ACK guard. This edge is DORS-routed with no
    // per-peer reachability proof, and DORS ranks the mesh above the
    // internet here — so the re-drive locally succeeds into the mesh and
    // must NOT clear the park counter (only an internet-routed send, which
    // can earn a relay verdict, counts as re-driven for counter-clearing).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (message_id, internet_mock, ble_mock) = park_and_fire_probe(&mut protocol);
    let internet_sent_before = internet_mock.sent_messages().len();
    let ble_sent_before = ble_mock.sent_messages().len();

    protocol.ack_manager.increment_retry_count(&message_id);
    protocol.ack_manager.increment_retry_count(&message_id);

    protocol.flush_outbox_all();

    assert_eq!(
        ble_mock.sent_messages().len(),
        ble_sent_before + 1,
        "The DORS-routed re-drive goes over the mesh on this edge"
    );
    assert_eq!(internet_mock.sent_messages().len(), internet_sent_before);
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&1),
        "A mesh-routed local success is not proof of reachability: the \
         counter must be restored so exhaustion stays re-parkable"
    );
    let pending = protocol
        .ack_manager
        .get_pending_ack(&message_id)
        .expect("re-driven DM must be ACK-tracked again");
    assert_eq!(
        pending.retry_count, 0,
        "The hostage probe ACK must be canceled and replaced by a fresh one"
    );
}

#[test]
fn test_flush_outbox_all_internet_redrive_clears_counter() {
    // Counterpart of the mesh-routed case above: with the mesh carrier down,
    // the carrier-level edge's DORS re-drive goes over the internet — a send
    // that can earn a relay verdict — so the edge genuinely takes and the
    // park counter clears.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (message_id, internet_mock, ble_mock) = park_and_fire_probe(&mut protocol);
    let internet_sent_before = internet_mock.sent_messages().len();

    ble_mock.stop().unwrap();
    protocol.flush_outbox_all();

    assert_eq!(
        internet_mock.sent_messages().len(),
        internet_sent_before + 1,
        "With the mesh down the re-drive must go over the internet"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        None,
        "An internet-routed re-drive earns a relay verdict: the edge takes \
         and the counter clears"
    );
    assert!(protocol.ack_manager.is_waiting_for_ack(&message_id));
}

#[test]
fn test_flush_outbox_all_mesh_redrive_keeps_exhaustion_reparkable() {
    // The mesh-void hole on the carrier-level edge: DM parked under dual
    // carrier, internet drops, a mesh reconnect fires flush_outbox_all. The
    // DORS re-drive locally succeeds into the mesh void and registers an
    // unanswerable ACK. Because the mesh-routed "success" restored the park
    // counter, that ACK's exhaustion must re-park the DM (escalated
    // counter) instead of settling terminally with message_failed.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let (message_id, internet_mock, ble_mock) = park_and_fire_probe(&mut protocol);
    let ble_sent_before = ble_mock.sent_messages().len();

    internet_mock.stop().unwrap();
    protocol.flush_outbox_all();

    assert_eq!(
        ble_mock.sent_messages().len(),
        ble_sent_before + 1,
        "The BLE-only re-drive locally succeeds into the mesh"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&1),
        "The mesh-routed success must restore the counter"
    );

    // The re-driven send's ACK never arrives and its budget burns out
    // (0ms timeout stands in for the elapsed waits).
    let max_retries = protocol.config.reliability.retry.max_retries;
    protocol.ack_manager.remove_ack(&message_id);
    protocol
        .ack_manager
        .register_pending_ack(message_id.clone(), Some(0))
        .unwrap();
    for _ in 0..max_retries {
        protocol.ack_manager.increment_retry_count(&message_id);
    }
    protocol.process().unwrap();

    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "Mesh-void exhaustion after the carrier edge must not settle terminally"
    );
    assert!(
        protocol.outbox.contains_key(&message_id),
        "Re-parked DM must stay in the outbox"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&2),
        "Re-park must escalate the restored counter"
    );
}

#[test]
fn test_unpark_cancel_spares_welcome_ack() {
    // Sibling of the connection-request exemption above: an in-flight MLS
    // welcome to the same peer keeps its pending ACK (and its own delivery
    // lifecycle) across a reachability edge's un-park cancel.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (dm_id, _internet_mock, _ble_mock) = park_and_fire_probe(&mut protocol);

    let welcome = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "__MLS_WELCOME__{}",
    );
    let welcome_id = welcome.id.clone();
    protocol.outbox.insert(
        welcome_id.clone(),
        OutboxEntry {
            message: welcome.clone(),
            attempt_count: 1,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: Some(TransportType::Internet),
            reseal: None,
        },
    );
    protocol
        .ack_manager
        .register_pending_ack(welcome_id.clone(), None)
        .unwrap();
    // Sent: wire-confirmed, awaiting session confirmation — the one state
    // the discovery edge's welcome re-arm deliberately never touches.
    protocol.welcome_lifecycles.insert(
        "bob".to_string(),
        WelcomeLifecycleRecord {
            peer_id: "bob".to_string(),
            group_id: "session:user123:bob".to_string(),
            state: WelcomeDeliveryState::Sent,
            attempt: 1,
            unreachable_parks: 0,
            welcome_message: welcome,
            next_retry_at: None,
            last_reason_code: None,
            last_transport_error: None,
            created_at: Utc::now(),
            expires_at: Utc::now() + ChronoDuration::seconds(600),
        },
    );

    protocol.on_neighbor_discovered("bob");

    assert!(
        protocol.ack_manager.is_waiting_for_ack(&welcome_id),
        "Welcome ACK must survive the un-park cancel"
    );
    assert!(
        protocol.ack_manager.is_waiting_for_ack(&dm_id),
        "The parked DM itself must be re-driven with a fresh ACK"
    );
}

#[test]
fn test_probe_hostage_redrive_internet_failure_does_not_leak_into_mesh() {
    // The presence edge's rescue branch re-enters discovery, whose inner
    // flush picks up any parked DM whose forced-internet re-drive just
    // failed (no pending ACK, park counter already cleared). That inner
    // flush must keep the internet override: a DORS re-drive could locally
    // succeed into the mesh void, re-registering an unanswerable ACK that
    // exhausts into terminal message_failed with no live counter left to
    // re-park on. A fully failed edge instead resumes retry backoff.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (message_id, internet_mock, ble_mock) = park_and_fire_probe(&mut protocol);
    let ble_sent_before = ble_mock.sent_messages().len();

    internet_mock.set_fail_next_sends(usize::MAX);
    protocol.on_peer_presence("bob", true, None);

    assert_eq!(
        ble_mock.sent_messages().len(),
        ble_sent_before,
        "A failed forced-internet re-drive must not fall back into the mesh"
    );
    assert!(
        !protocol.ack_manager.is_waiting_for_ack(&message_id),
        "No ACK may be pending after every re-drive attempt failed"
    );
    assert!(
        protocol.retry_queue.contains(&message_id.as_str()),
        "The failed re-drive resumes backoff instead of stranding"
    );
    assert!(protocol.outbox.contains_key(&message_id));
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&1),
        "A totally failed edge must restore the park counter — the resumed \
         retries are DORS-routed and exhaustion must stay re-parkable"
    );
}

#[test]
fn test_failed_unpark_redrive_keeps_exhaustion_reparkable() {
    // Continuation of the failed-edge scenario above: the resumed
    // retry-queue send is DORS-routed and can locally succeed into the mesh
    // void, re-registering an unanswerable ACK. Because the failed edge
    // restored the park counter, that ACK's exhaustion must re-park the DM
    // (escalated counter, next probe scheduled) instead of settling
    // terminally with message_failed.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let (message_id, internet_mock, _ble_mock) = park_and_fire_probe(&mut protocol);

    internet_mock.set_fail_next_sends(usize::MAX);
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));

    // The resumed DORS retry locally succeeds into the mesh — the same
    // unanswerable shape park_and_fire_probe simulates: retry entry
    // consumed, fresh pending ACK registered — and its budget burns out
    // (0ms timeout stands in for the elapsed waits).
    protocol.retry_queue.remove(&message_id.as_str());
    let max_retries = protocol.config.reliability.retry.max_retries;
    protocol
        .ack_manager
        .register_pending_ack(message_id.clone(), Some(0))
        .unwrap();
    for _ in 0..max_retries {
        protocol.ack_manager.increment_retry_count(&message_id);
    }
    protocol.process().unwrap();

    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "Exhaustion after a failed re-drive edge must not settle terminally"
    );
    assert!(
        protocol.outbox.contains_key(&message_id),
        "Re-parked DM must stay in the outbox"
    );
    assert!(
        !protocol.ack_manager.is_waiting_for_ack(&message_id),
        "Re-park must drop the exhausted ACK"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&2),
        "Re-park must escalate the restored counter"
    );
}

#[test]
fn test_unpark_flush_overflow_reenqueues_instead_of_stranding() {
    // More in-flight DMs to a parked peer than FLUSH_BATCH_LIMIT: the probe
    // cancel strips the pending ACKs of all of them, but a flush re-drives
    // only one batch. The overflow must land back in the retry queue (a
    // fresh backoff slot) — not strand edge-only with no ACK, no retry
    // entry, and the park counter cleared. Exercised via the discovery edge,
    // which runs exactly one flush (the presence edge's rescue branch runs a
    // second inner flush, draining another batch on the same tick).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let (first_id, _internet_mock, _ble_mock) = park_and_fire_probe(&mut protocol);

    let extra = 5;
    let mut ids = vec![first_id];
    for _ in 0..(crate::constants::FLUSH_BATCH_LIMIT - 1 + extra) {
        // Each send succeeds over a mock and registers a pending ACK while
        // bob's park counter is live — the same unanswerable-probe shape.
        let id = protocol
            .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert!(protocol.ack_manager.is_waiting_for_ack(&id));
        ids.push(id);
    }

    protocol.on_neighbor_discovered("bob");

    let (redriven, overflow): (Vec<_>, Vec<_>) = ids
        .iter()
        .partition(|id| protocol.ack_manager.is_waiting_for_ack(id));
    assert_eq!(
        redriven.len(),
        crate::constants::FLUSH_BATCH_LIMIT,
        "Exactly one batch is re-driven with fresh ACKs on the edge"
    );
    assert_eq!(overflow.len(), extra);
    for id in overflow {
        assert!(
            protocol.retry_queue.contains(&id.as_str()),
            "Overflow past the batch limit must re-enqueue, not strand"
        );
    }
}

#[test]
fn test_cleanup_outbox_absolute_lifetime_cap_is_terminal_in_process() {
    // The carrier-relative window slides on every send, and a parked DM's
    // mesh reachability probe keeps sending — in a long-lived process only
    // the absolute cap from first_sent_at bounds the entry's total
    // lifetime (mirrors the restore path's cap).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let lifetime_ms = protocol.config.reliability.retry.outbox_max_lifetime_ms as i64;
    let over_cap = chrono::Utc::now()
        - chrono::Duration::milliseconds(
            lifetime_ms * (crate::constants::OUTBOX_ABSOLUTE_LIFETIME_FACTOR as i64) + 60_000,
        );

    let make_entry = |first_sent_at| {
        let message = Message::new(
            UserId::new("user123").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            "hello",
        );
        let id = message.id.clone();
        (
            id,
            OutboxEntry {
                message,
                attempt_count: 5,
                first_sent_at,
                last_sent_at: chrono::Utc::now(), // fresh: probe just sent
                last_transport: None,
                reseal: None,
            },
        )
    };

    let (expired_id, expired_entry) = make_entry(over_cap);
    let (fresh_id, fresh_entry) = make_entry(chrono::Utc::now());
    protocol.outbox.insert(expired_id.clone(), expired_entry);
    protocol.outbox.insert(fresh_id.clone(), fresh_entry);

    protocol.cleanup_outbox();

    assert!(
        !protocol.outbox.contains_key(&expired_id),
        "Entry past the absolute cap must be dropped despite a fresh last_sent_at"
    );
    assert!(
        protocol.outbox.contains_key(&fresh_id),
        "Entry inside both windows must survive"
    );
    let captured = events.lock().unwrap();
    assert!(
        captured.iter().any(|event| matches!(
            event,
            Event::MessageFailed { message_id, reason, .. }
                if *message_id == expired_id.as_str() && reason == "Outbox lifetime exceeded"
        )),
        "Absolute-cap expiry must settle terminally"
    );
}

#[test]
fn test_dm_unreachable_parks_counter_pruned_with_outbox() {
    // Only peers that still hold outbox entries may retain a park counter,
    // so the map stays bounded by the outbox cap.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));

    protocol.outbox.remove(&message_id);
    protocol.cleanup_outbox();

    assert!(
        protocol.dm_unreachable_parks.is_empty(),
        "Counter must be pruned once the peer holds no outbox entries"
    );
}

#[test]
fn test_exhausted_probe_ack_budget_reparks_instead_of_settling() {
    // A mesh reachability probe re-enters the ACK machinery when its send
    // locally succeeds into the mesh, but over BLE it can never earn the
    // relay verdict that would re-park it. When that fresh budget exhausts
    // while the recipient still holds a live unreachable-park counter, the
    // DM must re-park (counter escalated, next probe scheduled) instead of
    // settling terminally — settlement stays reserved for delivery or
    // outbox-lifetime expiry.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let message_id = protocol
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    protocol
        .on_transport_send_failed(
            &message_id.as_str(),
            Some("recipient_unreachable: peer offline".to_string()),
        )
        .unwrap();
    assert_eq!(protocol.dm_unreachable_parks.get("bob"), Some(&1));
    assert!(!protocol.ack_manager.is_waiting_for_ack(&message_id));

    // Make the scheduled probe due now and drive it through the real retry
    // machinery: the BLE send succeeds locally and re-registers an ACK.
    let probe_message = protocol.outbox.get(&message_id).unwrap().message.clone();
    protocol.retry_queue.remove(&message_id.as_str());
    protocol
        .retry_queue
        .enqueue_with_delay(probe_message, 1, 0)
        .unwrap();
    protocol.process().unwrap();
    assert!(
        protocol.ack_manager.is_waiting_for_ack(&message_id),
        "A locally-successful probe send must re-enter the ACK machinery"
    );

    // Burn the probe's fresh budget: no ACK will ever arrive over the mesh
    // (0ms timeout stands in for the elapsed waits).
    let max_retries = protocol.config.reliability.retry.max_retries;
    protocol.ack_manager.remove_ack(&message_id);
    protocol
        .ack_manager
        .register_pending_ack(message_id.clone(), Some(0))
        .unwrap();
    for _ in 0..max_retries {
        protocol.ack_manager.increment_retry_count(&message_id);
    }
    protocol.process().unwrap();

    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "Probe exhaustion with a live park counter must not settle terminally"
    );
    assert!(
        protocol.outbox.contains_key(&message_id),
        "Re-parked DM must stay in the outbox"
    );
    assert!(
        !protocol.ack_manager.is_waiting_for_ack(&message_id),
        "Re-park must drop the exhausted ACK"
    );
    assert_eq!(
        protocol.dm_unreachable_parks.get("bob"),
        Some(&2),
        "Re-park must escalate the counter"
    );
    let next_probe = protocol
        .retry_queue
        .time_until_next_retry()
        .expect("re-park must schedule the next probe");
    assert!(
        next_probe > chrono::Duration::seconds(15),
        "Next probe must use the escalated interval"
    );
    assert!(next_probe <= chrono::Duration::seconds(30));
}

#[test]
fn test_require_encryption_allows_connection_control_messages() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Connection control messages are internal protocol messages (not user
    // content), so they must work even with require_encryption=true.
    let request_result = protocol.send_connection_request("bob", "alice", None, None);
    assert!(request_result.is_ok());

    let accept_result = protocol.accept_connection_request("bob", "alice", None);
    assert!(accept_result.is_ok());

    let reject_result = protocol.reject_connection_request("bob");
    assert!(reject_result.is_ok());

    let cancel_result = protocol.cancel_connection_request("bob");
    assert!(cancel_result.is_ok());

    assert_eq!(transport_handle.sent_messages().len(), 4);
}

#[test]
fn test_non_strict_mode_preserves_pending_queue_behavior() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.encryption.require_encryption = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    alice
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();
    bob.initialize_mls_for_test(bob_storage.clone()).unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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
    alice2
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();
    let alice2_transport = MockTransport::new(TransportType::BLE);
    alice2_transport.start().unwrap();
    let alice2_transport_handle = alice2_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice2_transport));
    alice2.start().unwrap();

    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls_for_test(bob_storage.clone()).unwrap();
    let bob2_transport = MockTransport::new(TransportType::BLE);
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

    let result = protocol.initialize_mls_for_test(Arc::new(FailingPendingListStorage::default()));
    assert!(result.is_err());
    assert!(protocol.mls_manager.is_none());
    assert!(protocol.secure_storage.is_none());
    assert!(protocol.protocol_state_storage.is_none());
    assert!(protocol.pending_encrypted_messages.is_empty());
    assert!(protocol.confirmed_sessions.is_empty());
    assert!(protocol.welcome_lifecycles.is_empty());
    assert_eq!(protocol.lamport_clock.value(), initial_clock);
}

#[test]
fn test_initialize_mls_restore_failure_rolls_back_outbox() {
    // Storage that fails specifically on the outbox restore step. The failure
    // propagates out of the transactional initialize_mls closure, and the
    // rollback snapshot must restore the pre-existing in-memory outbox rather
    // than leave it half-merged or published under a failed init.
    #[derive(Default)]
    struct FailingOutboxListStorage {
        inner: crate::mls::InMemoryStorage,
    }
    impl MlsStorage for FailingOutboxListStorage {
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
            if key_type == storage_keys::OUTBOX {
                return Err(offline_protocol_mls::StorageError::LoadFailed(
                    "forced outbox restore failure".to_string(),
                ));
            }
            self.inner.list_keys(key_type)
        }
    }

    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Seed an in-memory outbox entry before persistence is wired up.
    protocol.ensure_outbox_entry(&test_message("bob", "pre-existing"));
    assert_eq!(protocol.outbox_entry_count(), 1);

    let result = protocol.initialize_mls_for_test(Arc::new(FailingOutboxListStorage::default()));
    assert!(result.is_err());
    assert!(protocol.mls_manager.is_none());
    assert!(protocol.secure_storage.is_none());
    assert!(protocol.protocol_state_storage.is_none());
    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "Outbox should be rolled back to its pre-initialize state on restore failure"
    );
}

#[test]
fn test_auto_send_and_manual_mls_share_single_state_under_concurrency() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.encryption.require_encryption = false;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let transport = MockTransport::new(TransportType::BLE);
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
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let transport = MockTransport::new(TransportType::BLE);
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
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
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
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
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
fn test_is_session_confirmed_trusts_cache_and_encrypt_evicts_stale() {
    // is_session_confirmed() trusts the in-memory cache without storage I/O.
    // Stale state (confirmed but no MLS session) is detected lazily when
    // encrypt_confirmed_session() fails, which evicts the cache entry.
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    protocol.confirm_session_state("bob", "test_setup").unwrap();
    assert_eq!(
        protocol.load_session_state_entry("bob").unwrap().unwrap(),
        SessionState::Confirmed
    );

    // Fast path trusts the cache — returns true even without an MLS session
    assert!(protocol.is_session_confirmed("bob").unwrap());
    assert!(protocol.confirmed_sessions.contains("bob"));

    // But encryption will fail because there's no actual MLS session,
    // which evicts the cache entry and returns SessionNotReady (queueable).
    let mls = protocol.mls_manager.as_ref().unwrap().clone();
    let result = protocol.encrypt_bytes_confirmed_session(&mls, "bob", b"test");
    assert!(matches!(result, Err(Error::SessionNotReady(_))));
    assert!(!protocol.confirmed_sessions.contains("bob"));

    // After cache eviction, is_session_confirmed falls through to storage.
    // The storage path detects no MLS session and cleans up stale state.
    assert!(!protocol.is_session_confirmed("bob").unwrap());
    assert!(protocol.load_session_state_entry("bob").unwrap().is_none());
}

#[test]
fn test_encrypt_confirmed_session_transient_error_preserves_cache() {
    // Transient errors (crypto, storage I/O) should NOT evict the cache.
    // Only SessionNotFound (session deleted externally) evicts.
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    // Create a real MLS session
    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = crate::mls::MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.generate_key_package().unwrap();
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();
    assert!(protocol.confirmed_sessions.contains("bob"));

    // Encrypt succeeds on the fast path
    let mls = protocol.mls_manager.as_ref().unwrap().clone();
    let result = protocol.encrypt_bytes_confirmed_session(&mls, "bob", b"hello");
    assert!(result.is_ok());
    // Cache still intact after successful encrypt
    assert!(protocol.confirmed_sessions.contains("bob"));

    // Now delete the session from MLS to simulate external wipe
    {
        let manager = mls.read().unwrap();
        manager.delete_session("bob").unwrap();
    }

    // Encrypt fails with SessionNotReady (queueable) — cache should be evicted
    let result2 = protocol.encrypt_bytes_confirmed_session(&mls, "bob", b"hello again");
    assert!(matches!(result2, Err(Error::SessionNotReady(_))));
    assert!(
        !protocol.confirmed_sessions.contains("bob"),
        "SessionNotFound must evict the cache"
    );
}

#[test]
fn test_externally_deleted_confirmed_session_queues_message() {
    // When a confirmed session is externally deleted (e.g., storage corruption),
    // send_message should queue the message (via SessionNotReady) rather than
    // dropping it with a terminal EncryptFailed error.
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    config.encryption.store_pending = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let transport_handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Create a real MLS session and confirm it
    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = crate::mls::MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.generate_key_package().unwrap();
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol.confirm_session_state("bob", "test_setup").unwrap();
    assert!(protocol.confirmed_sessions.contains("bob"));

    // Externally delete the MLS session (simulating storage corruption)
    {
        let mls = protocol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.delete_session("bob").unwrap();
    }

    // Send a message — should be queued (not dropped), because the
    // SessionNotReady error from encrypt_confirmed_session is queueable.
    let result = protocol.send_message(
        "bob",
        "should be queued",
        None::<MessagePriority>,
        None::<String>,
    );
    assert!(
        result.is_ok(),
        "Message should be queued, not error: {:?}",
        result
    );
    assert_eq!(transport_handle.sent_messages().len(), 0);
    assert_eq!(
        protocol
            .pending_encrypted_messages
            .get("bob")
            .map_or(0, Vec::len),
        1,
        "Message should be in the pending queue"
    );
    // Cache should be evicted so future sends go through the full path
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    restarted.initialize_mls_for_test(storage).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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
    protocol.initialize_mls_for_test(storage).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
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
    protocol.initialize_mls_for_test(storage).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, 10);
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
    protocol.initialize_mls_for_test(storage).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
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

    // A mesh welcome is now NON-TERMINAL until the peer proves the session: a
    // successful (retried) transport send leaves the lifecycle SendAttempted
    // with a confirm timeout, so a lost fragment keeps being re-sent instead of
    // being falsely marked delivered.
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::SendAttempted);
    assert!(lifecycle.next_retry_at.is_some());

    // The peer's session proof (here an ack) confirms the session and marks the
    // welcome Sent, ending the retries.
    protocol
        .confirm_session_state("bob", "confirmation_ack_received")
        .unwrap();
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
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
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
fn test_on_transport_send_confirmed_sends_immediate_probe() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    let transport_handle = internet.clone();
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
            "probe-after-confirm",
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

    // Count messages before transport confirmation
    let messages_before = transport_handle.sent_messages().len();

    protocol
        .on_transport_send_confirmed(&welcome_message_id)
        .unwrap();

    // After transport confirmation, a confirmation probe should be sent immediately
    let messages_after = transport_handle.sent_messages();
    assert!(
        messages_after.len() > messages_before,
        "Expected a confirmation probe to be sent after on_transport_send_confirmed"
    );
    let probe_msg = messages_after.last().unwrap();
    assert!(
        probe_msg
            .content
            .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE),
        "Expected last message to be a session confirmation probe, got: {}",
        &probe_msg.content[..probe_msg.content.len().min(40)]
    );
}

#[test]
fn test_mesh_welcome_sender_probes_for_confirmation() {
    // Regression: the Welcome SENDER on a mesh transport (BLE / WiFi-Direct) must
    // actively probe the peer for confirmation, not wait passively for the peer's
    // single proactive confirm. Mesh has no `on_transport_send_confirmed`, so if
    // the Welcome send does not seed the probe scheduler, the sender's
    // `run_throttled_reconciliation` `has_pending_work` gate stays false (it has
    // no pending encrypted messages) and it never emits a SESSION_CONFIRM_PROBE.
    // The Welcome then retransmits every confirm-timeout window until TTL — the
    // observed "Welcome send confirmation timed out, attempt 1,2,3,4..." loop.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    // Mesh transport only — no Internet path to confirm the send for us.
    let ble = MockTransport::new(TransportType::BLE);
    ble.start().unwrap();
    let transport_handle = ble.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));
    protocol.start().unwrap();

    // Peer key package available, as after accepting a connection request.
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

    // Accepter path: create the 1:1 group and send the Welcome. This does NOT
    // queue an app message, so it does not borrow the initiator's
    // queue_message_for_session_establishment -> kick-reconciliation path; the
    // probe must be seeded by the Welcome send itself.
    protocol.establish_secure_session("bob").unwrap();

    assert!(
        protocol.confirmation_probe_due_at.contains_key("bob"),
        "Welcome send should seed the confirmation-probe scheduler for the pending peer"
    );

    let messages_before = transport_handle.sent_messages().len();

    // A process tick now has pending work, so reconciliation is not
    // short-circuited and the sender emits a confirmation probe.
    protocol.process().unwrap();

    let messages_after = transport_handle.sent_messages();
    assert!(
        messages_after.len() > messages_before,
        "Expected the sender to emit a confirmation probe on the next process tick"
    );
    assert!(
        messages_after.iter().any(|m| m
            .content
            .starts_with(internal_prefixes::SESSION_CONFIRM_PROBE)),
        "Expected a SESSION_CONFIRM_PROBE to be emitted by the mesh Welcome sender"
    );
}

#[test]
fn test_adopter_resends_confirm_on_welcome_retransmit_without_owner_keep() {
    // Regression (Bug B): a simple adopter with a lexicographically SMALLER
    // user_id that already joined the owner's group must, on a Welcome
    // RETRANSMIT, re-send its encrypted confirm so a lost first confirm
    // self-heals — and must NOT misclassify the retransmit as a both-create race
    // (owner_keep). The buggy path entered owner_keep (because has_existing is
    // true and local < remote), which poisoned both_create_awaiting_decrypt and
    // suppressed the confirm, stranding the owner in Pending forever.
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    // Owner "zoe" (lexicographically greater than "bob") creates the group and
    // the Welcome. bob never creates its own group — it only joins zoe's, so bob
    // has no outbound welcome lifecycle for zoe.
    let zoe_manager = MlsManager::new("zoe", Arc::new(InMemoryStorage::new())).unwrap();
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    zoe_manager
        .import_key_package("bob", &bob_key_package.key_package_data)
        .unwrap();
    let welcome = zoe_manager.create_session("bob").unwrap();
    let welcome_content = format!(
        "{}{}",
        internal_prefixes::WELCOME,
        serde_json::to_string(&welcome).unwrap()
    );
    let make_welcome_msg = || {
        Message::new(
            UserId::new("zoe").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &welcome_content,
        )
    };

    // First Welcome: bob joins and proactively confirms its own side.
    bob.process_internal_message(&make_welcome_msg());
    assert!(
        bob.confirmed_sessions.contains("zoe"),
        "adopter should confirm its session on first Welcome receipt"
    );

    let encrypted_before = bob_transport_handle
        .sent_messages()
        .iter()
        .filter(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .count();

    // Welcome RETRANSMIT — the owner is still re-sending because it has not yet
    // observed our confirm.
    bob.process_internal_message(&make_welcome_msg());

    // Must NOT be misclassified as a both-create owner.
    assert!(
        !bob.both_create_awaiting_decrypt.contains("zoe"),
        "adopter retransmit must not enter owner_keep / poison both_create_awaiting_decrypt"
    );

    // Must re-send an encrypted confirm so a lost first confirm self-heals.
    let encrypted_after = bob_transport_handle
        .sent_messages()
        .iter()
        .filter(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .count();
    assert!(
        encrypted_after > encrypted_before,
        "adopter should re-send an encrypted confirm on Welcome retransmit"
    );
}

#[test]
fn test_welcome_terminal_lifecycle_can_be_overwritten() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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
    // A mesh welcome is non-terminal until the peer proves the session; the
    // peer's confirmation drives it to the terminal Sent state under test here.
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );
    protocol
        .confirm_session_state("bob", "confirmation_ack_received")
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );

    // Sent → SendAttempted must never happen directly: a re-send always
    // goes through a rebuild (Created) or a corrective Failed first.
    let illegal = protocol.transition_welcome_state(
        "bob",
        WelcomeDeliveryState::SendAttempted,
        "test_illegal_transition",
    );
    assert!(illegal.is_err());

    // Sent → Failed, by contrast, is the one legal way back out of Sent:
    // the relay's DeliveryError authoritatively proves a wire-confirmed
    // frame was dropped (see apply_recipient_unreachable_failure).
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::Failed, "recipient_unreachable")
        .unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Failed
    );
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, 1);
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
    restarted.initialize_mls_for_test(storage).unwrap();
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    restarted.initialize_mls_for_test(storage).unwrap();
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    restarted.initialize_mls_for_test(storage).unwrap();
    let restored = restarted.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(restored.state, WelcomeDeliveryState::Expired);
    assert!(restored.next_retry_at.is_none());
}

#[test]
fn test_welcome_no_carrier_ticks_keep_lifecycle_alive() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 2;
    config.reliability.retry.initial_delay_ms = 1;
    config.reliability.retry.max_delay_ms = 5;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    // No transport is added, so every send fails with TransportNotAvailable.
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

    let _ = protocol.send_message(
        "bob",
        "queued-offline",
        None::<MessagePriority>,
        None::<String>,
    );

    // Far more no-carrier attempts than max_retries — none may expire the
    // Welcome, and the retry budget must not accumulate.
    for _ in 0..10 {
        let _ = protocol.try_send_welcome("bob", "test_no_carrier_tick");
    }

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_ne!(
        lifecycle.state,
        WelcomeDeliveryState::Expired,
        "a no-carrier Welcome must never expire"
    );
    assert!(
        lifecycle.attempt <= 1,
        "no-carrier failures must not consume the retry budget, got attempt={}",
        lifecycle.attempt
    );

    let events = observed_events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::WelcomeSendExpired { .. })),
        "no-carrier ticks must not emit WelcomeSendExpired"
    );
}

#[test]
fn test_no_carrier_welcome_parks_without_churn() {
    // Regression: a Welcome with no transport carrier must be PARKED (cheap,
    // slow re-check), not re-attempted at the data-plane retry rate. Each wasted
    // attempt previously incremented-then-rolled-back the attempt counter, ran
    // two state transitions (two persists + two info logs), and emitted a
    // WelcomeSendAttempted event — ~1 Hz of storage/event churn while a device
    // is simply offline. A parked Welcome emits no attempt/failed events and
    // schedules its next re-check on the slow no-carrier interval.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    // Tiny data-plane backoff: if the no-carrier path WERE (incorrectly) using
    // it, next_retry_at would land ~milliseconds out, not the slow interval.
    config.reliability.retry.initial_delay_ms = 1;
    config.reliability.retry.max_delay_ms = 5;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

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

    let _ = protocol.send_message(
        "bob",
        "queued-offline",
        None::<MessagePriority>,
        None::<String>,
    );
    for _ in 0..10 {
        let _ = protocol.try_send_welcome("bob", "test_no_carrier_tick");
    }

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    // Parked, not aged: still non-terminal and no attempt consumed.
    assert_ne!(lifecycle.state, WelcomeDeliveryState::Expired);
    assert_eq!(
        lifecycle.attempt, 0,
        "a parked no-carrier Welcome must not consume an attempt, got {}",
        lifecycle.attempt
    );
    // Re-check is scheduled on the SLOW no-carrier interval, proving we did not
    // fall through to the ~1ms data-plane backoff.
    let next_retry = lifecycle
        .next_retry_at
        .expect("a parked Welcome must schedule a re-check");
    let secs_out = (next_retry - Utc::now()).num_seconds();
    assert!(
        secs_out >= WELCOME_NO_CARRIER_RETRY_SECS - 2,
        "parked Welcome must re-check on the slow no-carrier interval, got {secs_out}s"
    );

    // The churn we eliminated: no per-tick attempt/failed/expired events.
    let events = observed_events.lock().unwrap();
    let churn = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::WelcomeSendAttempted { .. }
                    | Event::WelcomeSendFailed { .. }
                    | Event::WelcomeSendExpired { .. }
            )
        })
        .count();
    assert_eq!(
        churn, 0,
        "no-carrier ticks must not emit attempt/failed/expired churn, got {churn}"
    );
}

#[test]
fn test_both_create_gate_survives_owner_restart() {
    // Regression: the both-create owner gate must persist. If an owner restarts
    // mid-convergence and loses the gate, a stale plaintext probe/ack could
    // confirm it, stop its Welcome retransmission, and strand the peer on a
    // divergent group. With the gate restored, only a group-aware decrypt may
    // confirm.
    let config = create_test_config_for_user("alice");
    let storage = Arc::new(InMemoryStorage::new());

    let mut owner = OfflineProtocol::new(config.clone()).unwrap();
    owner.initialize_mls_for_test(storage.clone()).unwrap();
    owner.mark_both_create_awaiting_decrypt("bob");
    assert!(owner.both_create_awaiting_decrypt.contains("bob"));
    // The gate blocks a plaintext-ack confirmation (only a decrypt is accepted).
    assert!(!owner.can_confirm_from_source("bob", "confirmation_ack_received"));

    // Restart with the same storage: the gate must be restored.
    let mut restarted = OfflineProtocol::new(config.clone()).unwrap();
    restarted.initialize_mls_for_test(storage.clone()).unwrap();
    assert!(
        restarted.both_create_awaiting_decrypt.contains("bob"),
        "both-create owner gate must survive a restart"
    );
    assert!(
        !restarted.can_confirm_from_source("bob", "confirmation_ack_received"),
        "restored gate must still block a plaintext-ack confirmation"
    );

    // Convergence clears it from storage too, so a later restart does not revive
    // it (which would wrongly keep blocking confirmation of a converged peer).
    restarted.clear_both_create_awaiting_decrypt("bob");
    let mut after_converge = OfflineProtocol::new(config).unwrap();
    after_converge.initialize_mls_for_test(storage).unwrap();
    assert!(
        !after_converge.both_create_awaiting_decrypt.contains("bob"),
        "a cleared gate must not be restored after convergence"
    );
}

#[test]
fn test_both_create_gate_cleared_on_session_delete() {
    // Regression: deleting a 1:1 session (or repairing stale state) must clear
    // the persisted both-create owner gate. A leaked gate entry would make
    // `can_confirm_from_source` reject every non-decrypt source — including
    // `welcome_received` — on the NEXT session with this peer, re-stranding it
    // in Pending, and it would survive restart via the storage restore.
    let config = create_test_config_for_user("alice");
    let storage = Arc::new(InMemoryStorage::new());

    let mut owner = OfflineProtocol::new(config.clone()).unwrap();
    owner.initialize_mls_for_test(storage.clone()).unwrap();
    owner.mark_both_create_awaiting_decrypt("bob");
    assert!(owner.both_create_awaiting_decrypt.contains("bob"));
    assert!(!owner.can_confirm_from_source("bob", "welcome_received"));

    // Tearing the session down clears the gate from memory AND storage.
    owner.manual_mls_delete_session("bob").unwrap();
    assert!(
        !owner.both_create_awaiting_decrypt.contains("bob"),
        "session delete must clear the both-create owner gate in memory"
    );
    // The peer is now confirmable via a fresh Welcome again.
    assert!(
        owner.can_confirm_from_source("bob", "welcome_received"),
        "a re-paired peer must not be blocked by a stale gate"
    );

    // A restart must not revive the cleared gate from storage.
    let mut restarted = OfflineProtocol::new(config).unwrap();
    restarted.initialize_mls_for_test(storage).unwrap();
    assert!(
        !restarted.both_create_awaiting_decrypt.contains("bob"),
        "a deleted gate must not be restored after restart"
    );
}

#[test]
fn test_welcome_sends_when_carrier_appears() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 2;
    config.reliability.retry.initial_delay_ms = 1;
    config.reliability.retry.max_delay_ms = 5;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();
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

    // Phase 1: no carrier — the Welcome stalls (non-terminal) without expiring.
    let _ = protocol.send_message(
        "bob",
        "queued-offline",
        None::<MessagePriority>,
        None::<String>,
    );
    assert_ne!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Expired
    );

    // Phase 2: a transport appears; the stalled Welcome is delivered.
    let mock = MockTransport::new(TransportType::BLE);
    mock.set_status(TransportStatus::Available);
    let mock_handle = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    protocol
        .try_send_welcome("bob", "test_carrier_appeared")
        .unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.state,
        WelcomeDeliveryState::SendAttempted,
        "the Welcome should be transmitted once a carrier exists"
    );
    assert!(
        lifecycle.last_reason_code.is_none(),
        "a successful send clears the failure reason"
    );
    assert!(
        !mock_handle.sent_messages().is_empty(),
        "the carrier should have received the Welcome"
    );
}

#[test]
fn test_welcome_restart_keeps_no_carrier_lifecycle_alive() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config.clone()).unwrap();
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

    // A Welcome that stalled with no carrier and then aged past its TTL while
    // the device was offline, with no retry scheduled.
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
        record.expires_at = Utc::now() - ChronoDuration::seconds(1_000);
    }
    let persisted = protocol.welcome_lifecycles.get("bob").cloned().unwrap();
    protocol
        .persist_welcome_lifecycle_entry(&persisted)
        .unwrap();

    // Restart: the no-carrier Welcome must survive, not be promoted to Expired.
    let mut restarted = OfflineProtocol::new(config).unwrap();
    restarted.initialize_mls_for_test(storage).unwrap();

    let restored = restarted.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        restored.state,
        WelcomeDeliveryState::Failed,
        "a stale-TTL no-carrier Welcome must NOT be expired on restart"
    );
    assert!(restored.next_retry_at.is_some());
    assert!(
        restored.expires_at > Utc::now(),
        "the TTL window should be restarted (carrier-relative) on restore"
    );
}

#[test]
fn test_welcome_expired_rearms_on_peer_rediscovery() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 1;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    // A present-but-failing carrier: the first send fails (SendFailed), which
    // with max_retries=1 exhausts the Welcome to Expired.
    let mock = MockTransport::new(TransportType::BLE);
    mock.set_status(TransportStatus::Available);
    mock.set_fail_next_sends(1);
    let mock_handle = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
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

    let _ = protocol.send_message(
        "bob",
        "should-expire-then-recover",
        None::<MessagePriority>,
        None::<String>,
    );
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Expired,
        "carrier-present exhaustion should expire the Welcome"
    );

    // Sends now succeed (the single forced failure is spent). Rediscovering the
    // peer must re-arm and re-send the expired Welcome.
    protocol.on_neighbor_discovered("bob");

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_ne!(
        lifecycle.state,
        WelcomeDeliveryState::Expired,
        "rediscovery must revive an expired Welcome"
    );
    assert_eq!(lifecycle.state, WelcomeDeliveryState::SendAttempted);
    assert!(
        !mock_handle.sent_messages().is_empty(),
        "the re-armed Welcome should have been transmitted"
    );
}

#[test]
fn test_welcome_transport_callbacks_out_of_order_converge_to_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
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
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
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
fn test_session_confirms_via_ack_when_welcome_is_send_attempted() {
    // Reproduces the BLE issue: welcome lifecycle stays SendAttempted
    // (e.g., DORS fell back to Internet), but the peer received the welcome
    // and sent a confirmation ack. Session should confirm despite the
    // welcome not being in Sent state.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    // Set up Bob's MLS session
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol
        .ensure_session_state_entry("bob", "test_setup")
        .unwrap();

    // Simulate: welcome lifecycle is stuck at SendAttempted (DORS fell back to
    // Internet, or BLE delivery not confirmed yet).
    let welcome_msg = protocol
        .create_message("bob", "placeholder", Some(MessagePriority::High), None)
        .unwrap();
    protocol
        .upsert_welcome_lifecycle("bob", "session:bob:user123", welcome_msg, "test_setup")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_setup")
        .unwrap();

    // Queue a message — it should be pending because session is not confirmed
    let _msg_id = protocol
        .send_message(
            "bob",
            "hello-over-ble",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert!(
        protocol.pending_encrypted_messages.contains_key("bob"),
        "Message should be queued pending session establishment"
    );

    // Simulate: Bob received our welcome and sends a confirmation ack
    let ack_msg = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        internal_prefixes::SESSION_CONFIRM_ACK,
    );
    transport_handle.queue_message(ack_msg);
    let _ = protocol.receive_message();

    // Session should now be confirmed and pending messages flushed
    assert!(
        protocol.confirmed_sessions.contains("bob"),
        "Session should be confirmed after receiving ack with SendAttempted welcome"
    );
    assert!(
        !protocol.pending_encrypted_messages.contains_key("bob"),
        "Pending messages should be flushed after session confirmation"
    );
}

/// Regression (Android↔iOS "receives but never sends"): a both-create owner
/// whose own Welcome timed out to the terminal `Expired` state on a lossy /
/// asymmetric BLE link must STILL confirm the session the moment it decrypts a
/// real group message from the peer. A successful group decrypt is definitive
/// proof the peer adopted our group; gating decrypt-confirmation on a still-active
/// local Welcome left the owner able to receive but never reply (every outbound
/// send → SessionNotReady). The both-create gate is preserved: a plaintext probe
/// must NOT confirm such an owner — only a decrypt may.
#[test]
fn test_both_create_owner_confirms_via_decrypt_after_welcome_expired() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    // We are the both-create owner: we created our own group for "bob" and sent a
    // Welcome that never reached `Sent` — retries exhausted it to `Expired`.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();
    {
        let manager = protocol.mls_manager.as_ref().unwrap().read().unwrap();
        manager
            .import_key_package("bob", &bob_key_package.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    protocol
        .ensure_session_state_entry("bob", "test_setup")
        .unwrap();

    let welcome_msg = protocol
        .create_message("bob", "placeholder", Some(MessagePriority::High), None)
        .unwrap();
    protocol
        .upsert_welcome_lifecycle("bob", "session:bob:user123", welcome_msg, "test_setup")
        .unwrap();
    // Drive the welcome through to the terminal Expired state along its legal
    // lifecycle path (SendAttempted -> Failed -> Expired) — what a Welcome that
    // times out after retry exhaustion actually does.
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::SendAttempted, "test_setup")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::Failed, "test_setup")
        .unwrap();
    protocol
        .transition_welcome_state("bob", WelcomeDeliveryState::Expired, "test_setup")
        .unwrap();
    // Both-create owner: confirmation is gated on a group-aware decrypt.
    protocol.mark_both_create_awaiting_decrypt("bob");

    // A plaintext probe is NOT proof of adoption — the gate must still reject it.
    assert!(
        !protocol.can_confirm_from_source("bob", "confirmation_probe_received"),
        "a plaintext probe must not confirm a both-create owner"
    );

    // A successful group decrypt IS definitive proof — it must confirm even though
    // our local Welcome is Expired. Pre-fix this returned false, leaving the owner
    // permanently able to receive but never send.
    assert!(
        protocol.can_confirm_from_source("bob", "decrypt_success"),
        "a group decrypt must confirm the owner regardless of an expired local Welcome"
    );
    assert!(
        matches!(
            protocol.confirm_session_state("bob", "decrypt_success"),
            Ok(true)
        ),
        "confirm_session_state must transition Pending->Confirmed on decrypt_success"
    );
    assert!(
        protocol.confirmed_sessions.contains("bob"),
        "session must be confirmed so outbound sends stop failing SessionNotReady"
    );
}

/// Regression (Android↔iOS group creation): the 1:1 session OWNER — the side that
/// created the session group and sent the Welcome — confirms only when it decrypts
/// the peer's first group-aware message (the `decrypt_success` path). That path must
/// surface the app-facing `SecureSessionEstablished` event, exactly as the adopter's
/// Welcome-receive path does. Before the fix the owner confirmed silently: 1:1 send
/// and receive both worked, but the app never learned the session existed, so any UI
/// gated on a known secure session — the demo's "contacts with secure sessions"
/// group-creation list — silently excluded the peer ("no contact available to create
/// a group"). A both-create owner is the acute case: `can_confirm_from_source` forces
/// it to converge ONLY through decrypt, so without this emission the peer could never
/// be added to a group from the owner's device.
#[test]
fn test_session_owner_emits_established_on_decrypt_confirmation() {
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

    alice.initialize_mls_for_test(alice_storage).unwrap();
    bob.initialize_mls_for_test(bob_storage).unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));

    // Capture the owner's events before any session work begins.
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    alice.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });
    alice.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_transport_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    // Alice (owner) creates the session group + Welcome from Bob's key package.
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
    alice
        .send_message("bob", "bootstrap", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Bob adopts Alice's group from the Welcome.
    let welcome_wire = alice_transport_handle
        .sent_messages()
        .into_iter()
        .find(|msg| msg.content.starts_with(internal_prefixes::WELCOME))
        .map(|msg| msg.content)
        .expect("expected welcome message sent by owner");
    let welcome_msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &welcome_wire,
    );
    let _ = bob.process_internal_message(&welcome_msg);

    // The owner has only SENT a Welcome — it must stay Pending (and silent) until a
    // group-aware decrypt proves the peer adopted the group.
    assert!(
        !alice.confirmed_sessions.contains("bob"),
        "owner must stay Pending until it decrypts a group-aware message from the peer"
    );

    // Bob sends an encrypted message; Alice (owner) decrypts it and confirms.
    bob.send_message("alice", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    let encrypted_from_bob = bob_transport_handle
        .sent_messages()
        .into_iter()
        .rev()
        .find(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("expected encrypted message sent by adopter");
    let _ = alice.process_internal_message(&encrypted_from_bob);

    assert!(
        alice.confirmed_sessions.contains("bob"),
        "owner must confirm the session on decrypt_success"
    );
    let captured = events.lock().unwrap();
    assert!(
        captured.iter().any(|event| matches!(
            event,
            Event::SecureSessionEstablished {
                peer_id,
                is_session,
                initiated_by_local,
                ..
            } if peer_id == "bob" && *is_session && *initiated_by_local
        )),
        "owner must emit SecureSessionEstablished so the app learns the 1:1 session exists"
    );
}

#[test]
fn test_welcome_delayed_confirmation_after_timeout_converges_to_sent() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 3;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
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
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
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
        Some(InternalMessageResult::Deferred)
    ));
    assert!(bob.pending_queue.contains_peer("alice"));
    assert!(!bob.confirmed_sessions.contains("alice"));

    let welcome_result = bob.process_internal_message(&welcome_wire);
    assert!(matches!(
        welcome_result,
        Some(InternalMessageResult::Consumed)
    ));
    assert!(bob.confirmed_sessions.contains("alice"));
    assert!(!bob.pending_queue.contains_peer("alice"));

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

/// Keystone of the deferred-ACK atom (test a + c): an encrypted DM that arrives
/// before the receiver's MLS session is ready is queued but NOT delivery-ACKed
/// and NOT left dedup-marked, so the sender's resend re-enters processing
/// instead of being swallowed by the duplicate re-ACK path. Repeated resends do
/// not stack queue entries (idempotent enqueue). Once the Welcome drains the
/// queue, the message surfaces exactly once and the id is re-marked, so a later
/// resend is deduped + re-ACKed rather than delivered twice.
#[test]
fn test_deferred_encrypted_dm_defers_ack_then_recovers_without_loss_or_dup() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_handle = Arc::clone(&received);
    bob.on_event(move |event| {
        if let Event::MessageReceived { content, .. } = event {
            received_handle.lock().unwrap().push(content);
        }
    });
    bob.start().unwrap();

    // Alice builds a real session to bob plus one encrypted message and the
    // matching Welcome, all at the MLS-manager level.
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
        .encrypt_for_user("bob", b"before-welcome")
        .unwrap();

    // One encrypted wire message, cloned for every resend so the id is stable.
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
    assert!(encrypted_wire.requires_ack, "delivery ACK must be in play");
    let msg_id = encrypted_wire.id.clone();
    let msg_id_str = msg_id.as_str().to_string();

    let acks_for_msg = |handle: &MockTransport| -> usize {
        handle
            .sent_messages()
            .into_iter()
            .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&msg_id_str))
            .count()
    };

    // 1) First delivery, session not ready: queued, unmarked, NOT ACKed, not surfaced.
    bob_handle.queue_message(encrypted_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);
    assert!(
        !bob.deduplicator.is_duplicate(&msg_id),
        "a deferred message must be unmarked so a resend can re-enter processing"
    );
    assert_eq!(
        acks_for_msg(&bob_handle),
        0,
        "a deferred message must not be ACKed"
    );
    assert!(
        received.lock().unwrap().is_empty(),
        "must not surface before decrypt"
    );

    // 2) Resend before the session is ready: reprocessed (not swallowed),
    //    still queued exactly once (idempotent enqueue), still not ACKed.
    bob_handle.queue_message(encrypted_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(
        bob.pending_queue.peer_queue_len("alice"),
        1,
        "resends of the same id must not stack queue entries"
    );
    assert_eq!(acks_for_msg(&bob_handle), 0);
    assert!(received.lock().unwrap().is_empty());

    // 3) Welcome arrives: session confirmed, queue drains, message surfaces once.
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
    bob_handle.queue_message(welcome_wire);
    while bob.receive_message().is_some() {}
    assert!(
        !bob.pending_queue.contains_peer("alice"),
        "queue must drain on Welcome"
    );
    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["before-welcome".to_string()],
        "the queued message must surface exactly once"
    );
    assert!(
        bob.deduplicator.is_duplicate(&msg_id),
        "the drained message must be re-marked so a later resend is deduped"
    );

    // 4) A resend arriving after the drain is deduped + re-ACKed, never delivered twice.
    bob_handle.clear_sent_messages();
    bob_handle.queue_message(encrypted_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(
        received.lock().unwrap().len(),
        1,
        "a post-drain resend must not double-deliver"
    );
    assert_eq!(
        acks_for_msg(&bob_handle),
        1,
        "a post-drain resend must be re-ACKed so the sender stops retrying"
    );
}

/// Kill-switch fall-through: with `crypto_recovery_enabled = false`, a desync DM
/// must take the *legacy* path — classified as a plain crypto failure that is
/// dropped **and delivery-ACKed** (the old lying-ACK behavior), with **no**
/// re-key triggered. This pins the disabled arm of the
/// `SessionDesync if crypto_recovery_enabled` guard so the kill switch remains a
/// true rollback lever.
#[test]
fn test_crypto_recovery_disabled_falls_back_to_drop_and_ack() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;
    // The rollback lever under test.
    bob_config.encryption.crypto_recovery_enabled = false;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_handle = Arc::clone(&received);
    bob.on_event(move |event| {
        if let Event::MessageReceived { content, .. } = event {
            received_handle.lock().unwrap().push(content);
        }
    });
    bob.start().unwrap();

    // Establish bob's session with alice via a real Welcome.
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
    bob_handle.queue_message(welcome_wire);
    while bob.receive_message().is_some() {}

    // Fork the session so alice's next message cannot decrypt (WrongEpoch).
    alice_manager
        .update_keys(&offline_protocol_mls::GroupId::new("session:alice:bob").unwrap())
        .unwrap();
    let encrypted = alice_manager
        .encrypt_for_user("bob", b"after-fork")
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
    let msg_id_str = encrypted_wire.id.as_str().to_string();

    bob_handle.clear_sent_messages();
    bob_handle.queue_message(encrypted_wire);
    while bob.receive_message().is_some() {}

    let sent = bob_handle.sent_messages();

    // Legacy behavior: the message IS delivery-ACKed (dropped-and-ACKed).
    let acks = sent
        .iter()
        .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&msg_id_str))
        .count();
    assert_eq!(
        acks, 1,
        "with recovery disabled, a desync DM falls back to drop-and-ACK"
    );

    // And NO re-key is triggered.
    let rekeys = sent
        .iter()
        .filter(|m| {
            m.content.starts_with(internal_prefixes::KEY_PACKAGE) && m.recipient.as_str() == "alice"
        })
        .count();
    assert_eq!(
        rekeys, 0,
        "with recovery disabled, a desync must NOT trigger a re-key"
    );

    assert!(
        received.lock().unwrap().is_empty(),
        "an undecryptable DM never surfaces to the app regardless of the kill switch"
    );
}

/// Tier 1 crypto-failure recovery: an established 1:1 session that has forked
/// (the peer advanced its epoch) yields an undecryptable DM. Instead of the old
/// silent drop-and-ACK, the receiver must withhold the delivery ACK (so the
/// sender is not told "delivered") and trigger a `session_reset` re-key to heal
/// the channel — without enqueueing the ciphertext (it is sealed to the dead
/// epoch and can never drain).
#[test]
fn test_desync_dm_withholds_ack_and_triggers_rekey() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_handle = Arc::clone(&received);
    bob.on_event(move |event| {
        if let Event::MessageReceived { content, .. } = event {
            received_handle.lock().unwrap().push(content);
        }
    });
    bob.start().unwrap();

    // Establish bob's session with alice via a real Welcome.
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
    bob_handle.queue_message(welcome_wire);
    while bob.receive_message().is_some() {}

    // Fork the session: alice advances her epoch (self-update) without bob
    // seeing the commit, then encrypts at the new epoch. Bob is now one epoch
    // behind → the message cannot decrypt (WrongEpoch → SessionDesync).
    alice_manager
        .update_keys(&offline_protocol_mls::GroupId::new("session:alice:bob").unwrap())
        .unwrap();
    let encrypted = alice_manager
        .encrypt_for_user("bob", b"after-fork")
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
    assert!(encrypted_wire.requires_ack, "delivery ACK must be in play");
    let msg_id_str = encrypted_wire.id.as_str().to_string();

    // Only look at side-effects caused by the forked message.
    bob_handle.clear_sent_messages();
    bob_handle.queue_message(encrypted_wire);
    while bob.receive_message().is_some() {}

    let sent = bob_handle.sent_messages();
    let acks = sent
        .iter()
        .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&msg_id_str))
        .count();
    assert_eq!(acks, 0, "a desync DM must NOT be delivery-ACKed");

    assert!(
        received.lock().unwrap().is_empty(),
        "an undecryptable desync DM must not surface to the app"
    );
    assert!(
        !bob.pending_queue.contains_peer("alice"),
        "a desync DM must NOT be enqueued (its ciphertext can never drain)"
    );

    let rekeys = sent
        .iter()
        .filter(|m| {
            m.content.starts_with(internal_prefixes::KEY_PACKAGE) && m.recipient.as_str() == "alice"
        })
        .count();
    assert_eq!(
        rekeys, 1,
        "a desync must trigger exactly one session_reset re-key to alice"
    );
}

/// The re-key DoS guard: a peer replaying stale-epoch ciphertext (or an injected
/// wrong-epoch frame) must not be able to drive a re-key storm. Repeated
/// same-peer re-key triggers within the rate-limit window collapse to a single
/// key-package send.
#[test]
fn test_desync_rekey_is_rate_limited() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    bob_handle.clear_sent_messages();

    // Five rapid triggers for the same peer, well inside REKEY_INTERVAL_SECS.
    for _ in 0..5 {
        bob.schedule_session_rekey("alice");
    }

    let rekeys = bob_handle
        .sent_messages()
        .into_iter()
        .filter(|m| {
            m.content.starts_with(internal_prefixes::KEY_PACKAGE) && m.recipient.as_str() == "alice"
        })
        .count();
    assert_eq!(
        rekeys, 1,
        "repeated desync triggers within the window must collapse to one re-key"
    );
}

/// Regression: the re-key floor must NOT be reset by a successful decrypt on the
/// healed session. A genuine re-fork and a replayed old-epoch frame are
/// indistinguishable at this layer, so if the successful-decrypt confirmation
/// path cleared the limiter (as an earlier revision did), an attacker landing
/// one legit decrypt between replays could defeat the rate limit and force
/// ~one session teardown per inbound message. This locks the floor across a heal.
#[test]
fn test_desync_rekey_floor_not_reset_by_successful_decrypt() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    bob_handle.clear_sent_messages();

    // First desync → one re-key, floor armed.
    bob.schedule_session_rekey("alice");
    assert!(
        bob.rekey_due_at.contains_key("alice"),
        "re-key must arm the rate-limit floor"
    );

    // Simulate the channel healing and a real message decrypting on the rebuilt
    // session — the successful-decrypt confirmation path. Historically this
    // cleared the floor; it must no longer, even for a now-confirmed peer.
    bob.confirm_session_from_successful_decrypt("alice", "session:alice:bob");
    assert!(
        bob.rekey_due_at.contains_key("alice"),
        "a successful decrypt on the healed session must NOT reset the re-key floor"
    );

    // A second desync trigger inside the window (the replay) must still be
    // suppressed — exactly one re-key total across the heal.
    bob.schedule_session_rekey("alice");
    let rekeys = bob_handle
        .sent_messages()
        .into_iter()
        .filter(|m| {
            m.content.starts_with(internal_prefixes::KEY_PACKAGE) && m.recipient.as_str() == "alice"
        })
        .count();
    assert_eq!(
        rekeys, 1,
        "a replay after a heal must not defeat the re-key floor"
    );
}

/// Tier 2 headline: after the recipient re-keys to a NEW epoch, the ciphertext
/// sealed at the original epoch is permanently dead — but a resend re-seals
/// against the peer's *current* session and decrypts on the new epoch, with the
/// message id preserved. This is the whole point of keeping re-seal provenance:
/// replaying the old bytes would be silent loss forever; re-sealing recovers.
#[test]
fn test_reseal_on_resend_recovers_after_recipient_rekeys_to_new_epoch() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    // Force the JSON envelope for a deterministic decode below.
    alice_config.encryption.compact_envelope_enabled = false;

    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    alice
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    // Bob as a bare manager; establish a real 1:1 session alice -> bob.
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_kp = bob_manager.get_or_create_key_package().unwrap();
    {
        let mgr = alice.mls_manager.as_ref().unwrap().read().unwrap();
        mgr.import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
    }
    let welcome = {
        let mgr = alice.mls_manager.as_ref().unwrap().read().unwrap();
        mgr.create_session("bob").unwrap()
    };
    bob_manager.join_session(&welcome).unwrap();
    // Force the session confirmed so the send encrypts now instead of queueing.
    alice.confirmed_sessions.insert("bob".to_string());

    let msg_id = alice
        .send_message("bob", "hello", None, None::<String>)
        .unwrap();

    let entry = alice
        .outbox
        .get(&msg_id)
        .expect("encrypted DM must enter the outbox")
        .clone();
    assert!(
        entry.reseal.is_some(),
        "an encrypted DM must store re-seal provenance"
    );
    assert!(entry
        .message
        .content
        .starts_with(internal_prefixes::ENCRYPTED));
    // The original ciphertext, sealed at the first epoch.
    let c1 = entry.message.content.clone();
    let c1_enc: offline_protocol_mls::EncryptedMessage =
        serde_json::from_str(c1.strip_prefix(internal_prefixes::ENCRYPTED).unwrap()).unwrap();

    // Re-key: tear down both sessions and rebuild a fresh one at a new epoch,
    // exactly as the desync heal does. The outbox entry (and its in-memory
    // re-seal provenance) survives the teardown.
    alice.manual_mls_delete_session("bob").unwrap();
    bob_manager.delete_session("alice").unwrap();
    let bob_kp2 = bob_manager.get_or_create_key_package().unwrap();
    {
        let mgr = alice.mls_manager.as_ref().unwrap().read().unwrap();
        mgr.import_key_package("bob", &bob_kp2.key_package_data)
            .unwrap();
    }
    let welcome2 = {
        let mgr = alice.mls_manager.as_ref().unwrap().read().unwrap();
        mgr.create_session("bob").unwrap()
    };
    bob_manager.join_session(&welcome2).unwrap();
    alice.confirmed_sessions.insert("bob".to_string());

    // The original bytes are now permanently undecryptable on the new session —
    // proving a verbatim replay would be silent loss.
    assert!(
        bob_manager.decrypt_from_user(&c1_enc, "alice").is_err(),
        "the pre-rekey ciphertext must be dead on the rebuilt session"
    );

    // Re-seal for a resend against the rebuilt session.
    let mut resend = entry.message.clone();
    alice.reseal_for_resend_in_place(&mut resend);
    assert_ne!(
        resend.content, c1,
        "re-seal must produce fresh ciphertext for the new epoch"
    );
    assert_eq!(
        resend.id, msg_id,
        "re-seal must preserve the message id for dedup/ACK correlation"
    );

    // The re-sealed ciphertext decrypts on the recipient's rebuilt session,
    // recovering the message that the dead bytes could never have delivered.
    let json = resend
        .content
        .strip_prefix(internal_prefixes::ENCRYPTED)
        .unwrap();
    let enc: offline_protocol_mls::EncryptedMessage = serde_json::from_str(json).unwrap();
    let pt = bob_manager.decrypt_from_user(&enc, "alice").unwrap();
    assert_eq!(
        pt.as_deref(),
        Some(&b"hello"[..]),
        "re-sealed ciphertext must decrypt to the original plaintext"
    );
}

/// A message with no re-seal provenance (plaintext, media, or a pre-upgrade
/// outbox entry) is replayed byte-for-byte — the safe verbatim fallback.
#[test]
fn test_reseal_is_noop_without_provenance() {
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let mut plain = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "not encrypted",
    );
    let before = plain.content.clone();
    alice.reseal_for_resend_in_place(&mut plain);
    assert_eq!(
        plain.content, before,
        "a non-encrypted message must be replayed verbatim"
    );
}

/// Creates an MLS-enabled, unstarted protocol paired with a handle to its mock
/// transport. Left unstarted so the caller can register event handlers first.
fn make_encrypted_protocol(user_id: &str) -> (OfflineProtocol, MockTransport) {
    let mut config = create_test_config_for_user(user_id);
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let handle = transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    (protocol, handle)
}

/// Wires a confirmed 1:1 MLS session between two full protocol instances by
/// exchanging a key package + Welcome directly through their managers, then
/// marking both sides confirmed. Mirrors what a real handshake converges to
/// without depending on handshake pump timing. `b` owns the created group.
fn establish_confirmed_session(
    a: &mut OfflineProtocol,
    a_id: &str,
    b: &mut OfflineProtocol,
    b_id: &str,
) {
    let a_kp = {
        let m = a.mls_manager.as_ref().unwrap().read().unwrap();
        m.get_or_create_key_package().unwrap()
    };
    let welcome = {
        let m = b.mls_manager.as_ref().unwrap().read().unwrap();
        m.import_key_package(a_id, &a_kp.key_package_data).unwrap();
        m.create_session(a_id).unwrap()
    };
    {
        let m = a.mls_manager.as_ref().unwrap().read().unwrap();
        m.join_session(&welcome).unwrap();
    }
    a.confirmed_sessions.insert(b_id.to_string());
    b.confirmed_sessions.insert(a_id.to_string());
}

/// Advances one side's 1:1 epoch without the peer merging the commit, forking
/// the shared `session:alice:bob` group so the peer can no longer decrypt.
fn fork_session_epoch(ahead: &mut OfflineProtocol) {
    let gid = offline_protocol_mls::GroupId::new("session:alice:bob").unwrap();
    let m = ahead.mls_manager.as_ref().unwrap().read().unwrap();
    m.update_keys(&gid).unwrap();
}

/// Delivers messages back and forth between two protocols over their mock
/// transports until nothing more moves (or `max_rounds` elapse). Each round
/// drains both inboxes, then hands every message each side *sent* to the other
/// side's inbox (tagged with the sender's id as the transport-verified peer).
fn pump_between(
    a: &mut OfflineProtocol,
    a_h: &MockTransport,
    b: &mut OfflineProtocol,
    b_h: &MockTransport,
    max_rounds: usize,
) {
    for _ in 0..max_rounds {
        while a.receive_message().is_some() {}
        while b.receive_message().is_some() {}
        let a_sent = a_h.sent_messages();
        a_h.clear_sent_messages();
        let b_sent = b_h.sent_messages();
        b_h.clear_sent_messages();
        if a_sent.is_empty() && b_sent.is_empty() {
            return;
        }
        for m in a_sent {
            let peer = m.sender.as_str().to_string();
            b_h.queue_message_from(m, peer);
        }
        for m in b_sent {
            let peer = m.sender.as_str().to_string();
            a_h.queue_message_from(m, peer);
        }
    }
}

/// End-to-end heal, detector id > peer id (single-round convergence). Alice
/// (the peer) forks ahead; Bob (the detector, "bob" > "alice") fails to decrypt,
/// re-keys, and — being the greater id — adopts Alice's returning Welcome in one
/// round. A fresh message each way then flows, proving the channel healed.
#[test]
fn test_desync_dm_heals_end_to_end_when_detector_id_is_greater() {
    let (mut alice, alice_h) = make_encrypted_protocol("alice");
    let (mut bob, bob_h) = make_encrypted_protocol("bob");

    let alice_rx: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bob_rx: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let h = Arc::clone(&alice_rx);
        alice.on_event(move |e| {
            if let Event::MessageReceived { content, .. } = e {
                h.lock().unwrap().push(content);
            }
        });
        let h = Arc::clone(&bob_rx);
        bob.on_event(move |e| {
            if let Event::MessageReceived { content, .. } = e {
                h.lock().unwrap().push(content);
            }
        });
    }
    alice.start().unwrap();
    bob.start().unwrap();

    establish_confirmed_session(&mut alice, "alice", &mut bob, "bob");

    // Fork: Alice advances her epoch; Bob is now behind and can't decrypt her.
    fork_session_epoch(&mut alice);
    alice
        .send_message("bob", "trigger-fork", None, None::<String>)
        .unwrap();

    // Pump the heal to convergence.
    pump_between(&mut alice, &alice_h, &mut bob, &bob_h, 40);

    // The channel is healed: a fresh message each way is delivered.
    alice
        .send_message("bob", "healed-a2b", None, None::<String>)
        .unwrap();
    bob.send_message("alice", "healed-b2a", None, None::<String>)
        .unwrap();
    pump_between(&mut alice, &alice_h, &mut bob, &bob_h, 40);

    assert!(
        bob_rx.lock().unwrap().iter().any(|c| c == "healed-a2b"),
        "bob must receive a fresh DM after the channel heals (got {:?})",
        bob_rx.lock().unwrap()
    );
    assert!(
        alice_rx.lock().unwrap().iter().any(|c| c == "healed-b2a"),
        "alice must receive a fresh DM after the channel heals (got {:?})",
        alice_rx.lock().unwrap()
    );
}

/// End-to-end heal, detector id < peer id (the fragile two-round path). Bob
/// (the peer) forks ahead; Alice (the detector, "alice" < "bob") fails to
/// decrypt and re-keys, but — being the smaller id — keeps her stale session on
/// Bob's returning Welcome. Her old-epoch traffic then makes Bob detect a desync
/// in turn and re-key back, forcing Alice to reset; the both-create tiebreaker
/// converges on the second round. A fresh message each way proves it heals.
#[test]
fn test_desync_dm_heals_end_to_end_when_detector_id_is_smaller() {
    let (mut alice, alice_h) = make_encrypted_protocol("alice");
    let (mut bob, bob_h) = make_encrypted_protocol("bob");

    let alice_rx: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let bob_rx: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let h = Arc::clone(&alice_rx);
        alice.on_event(move |e| {
            if let Event::MessageReceived { content, .. } = e {
                h.lock().unwrap().push(content);
            }
        });
        let h = Arc::clone(&bob_rx);
        bob.on_event(move |e| {
            if let Event::MessageReceived { content, .. } = e {
                h.lock().unwrap().push(content);
            }
        });
    }
    alice.start().unwrap();
    bob.start().unwrap();

    establish_confirmed_session(&mut alice, "alice", &mut bob, "bob");

    // Fork: Bob advances his epoch; Alice is now behind and can't decrypt him.
    fork_session_epoch(&mut bob);
    bob.send_message("alice", "trigger-fork", None, None::<String>)
        .unwrap();

    // Pump the heal to convergence (may take the extra round for alice < bob).
    pump_between(&mut alice, &alice_h, &mut bob, &bob_h, 60);

    // The channel is healed: a fresh message each way is delivered.
    alice
        .send_message("bob", "healed-a2b", None, None::<String>)
        .unwrap();
    bob.send_message("alice", "healed-b2a", None, None::<String>)
        .unwrap();
    pump_between(&mut alice, &alice_h, &mut bob, &bob_h, 60);

    assert!(
        bob_rx.lock().unwrap().iter().any(|c| c == "healed-a2b"),
        "bob must receive a fresh DM after the channel heals (got {:?})",
        bob_rx.lock().unwrap()
    );
    assert!(
        alice_rx.lock().unwrap().iter().any(|c| c == "healed-b2a"),
        "alice must receive a fresh DM after the channel heals (got {:?})",
        alice_rx.lock().unwrap()
    );
}

/// The deferred-ACK headline: an *evicted* pending entry (not merely a drained
/// one) still recovers on the sender's next resend, precisely because the
/// eviction never produced an ACK. With a 1-slot per-peer queue and `DropOldest`,
/// a second early message evicts the first; the evicted message is provably gone
/// (it does not surface when the queue later drains) and was never ACKed, so once
/// the session is ready the sender's resend delivers it. This is the recovery
/// path the queue-path silent-loss bug used to break.
#[test]
fn test_evicted_pending_message_recovers_on_resend_after_session_ready() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;
    // One slot: the evictor deterministically displaces the evicted entry under
    // the default `DropOldest` policy, no ACK-driven window pumping required.
    bob_config.encryption.pending_queue.max_pending_per_peer = 1;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_handle = Arc::clone(&received);
    bob.on_event(move |event| {
        if let Event::MessageReceived { content, .. } = event {
            received_handle.lock().unwrap().push(content);
        }
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

    // Encrypt the evictor FIRST (lower MLS generation) so that after it drains
    // it has been consumed in order, and the later-recovered evicted message
    // (higher generation) decrypts forward — no reliance on the out-of-order
    // ratchet window.
    let encrypted_evictor = alice_manager
        .encrypt_for_user("bob", b"evictor-msg")
        .unwrap();
    let encrypted_evicted = alice_manager
        .encrypt_for_user("bob", b"evicted-msg")
        .unwrap();

    let wire = |encrypted: &_| {
        Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test-app").unwrap(),
            &format!(
                "{}{}",
                internal_prefixes::ENCRYPTED,
                serde_json::to_string(encrypted).unwrap()
            ),
        )
    };
    // The evicted message is received FIRST (so it is the oldest queued entry),
    // then the evictor displaces it.
    let evicted_wire = wire(&encrypted_evicted);
    let evictor_wire = wire(&encrypted_evictor);
    let evicted_id = evicted_wire.id.clone();
    let evicted_id_str = evicted_id.as_str().to_string();
    let acks_for_evicted = |handle: &MockTransport| -> usize {
        handle
            .sent_messages()
            .into_iter()
            .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&evicted_id_str))
            .count()
    };

    // 1) Evicted message arrives, session not ready: queued, unmarked, not ACKed.
    bob_handle.queue_message(evicted_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);

    // 2) Evictor arrives: displaces the evicted entry under DropOldest.
    bob_handle.queue_message(evictor_wire);
    while bob.receive_message().is_some() {}
    assert_eq!(
        bob.pending_queue.peer_queue_len("alice"),
        1,
        "the 1-slot queue holds only the evictor now"
    );
    assert!(
        !bob.deduplicator.is_duplicate(&evicted_id),
        "the evicted message's id must stay unmarked so a resend re-enters processing"
    );
    assert_eq!(
        acks_for_evicted(&bob_handle),
        0,
        "the evicted message was never ACKed — that is what keeps recovery possible"
    );

    // 3) Welcome establishes the session and drains the queue. Only the evictor
    //    surfaces; the evicted message is provably gone (never delivered).
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
    bob_handle.queue_message(welcome_wire);
    while bob.receive_message().is_some() {}
    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["evictor-msg".to_string()],
        "only the surviving (evictor) message surfaces on drain; the evicted one is gone"
    );

    // 4) The un-ACKed sender resends the evicted message. The session is now
    //    ready, so it decrypts on the live path and is delivered + ACKed —
    //    recovery of an evicted entry with no silent loss.
    bob_handle.clear_sent_messages();
    bob_handle.queue_message(evicted_wire);
    while bob.receive_message().is_some() {}
    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["evictor-msg".to_string(), "evicted-msg".to_string()],
        "the evicted message is recovered exactly once by the sender's resend"
    );
    assert_eq!(
        acks_for_evicted(&bob_handle),
        1,
        "the recovered message is ACKed so the sender can finally stop retrying"
    );
}

/// change-6: `confirm_session_from_successful_decrypt` now drains the pending
/// decryption queue. A 1:1 session OWNER confirms ONLY by decrypting the peer's
/// first group-aware message (never a Welcome-receive), so before this change a
/// message queued on the owner side was stranded until TTL eviction. Here alice
/// (owner) never receives a Welcome — the sole confirmation, and therefore the
/// sole possible drain trigger, is the decrypt of bob's message.
#[test]
fn test_pending_queue_drains_on_decrypt_confirmation_owner_path() {
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    alice
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_handle = Arc::clone(&received);
    alice.on_event(move |event| {
        if let Event::MessageReceived { content, .. } = event {
            received_handle.lock().unwrap().push(content);
        }
    });
    alice.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
    bob.start().unwrap();

    // Alice (owner) creates the group + Welcome from bob's key package.
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
    alice
        .send_message("bob", "bootstrap", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Bob adopts alice's group from the Welcome.
    let welcome_wire = alice_handle
        .sent_messages()
        .into_iter()
        .find(|msg| msg.content.starts_with(internal_prefixes::WELCOME))
        .expect("owner should have sent a Welcome");
    let _ = bob.process_internal_message(&welcome_wire);
    assert!(
        !alice.confirmed_sessions.contains("bob"),
        "owner stays Pending until it decrypts a group-aware message"
    );

    // Bob sends two encrypted messages; capture their wires.
    bob.send_message(
        "alice",
        "queued-earlier",
        None::<MessagePriority>,
        None::<String>,
    )
    .unwrap();
    bob.send_message(
        "alice",
        "confirming",
        None::<MessagePriority>,
        None::<String>,
    )
    .unwrap();
    let mut encrypted: Vec<Message> = bob_handle
        .sent_messages()
        .into_iter()
        .filter(|msg| msg.content.starts_with(internal_prefixes::ENCRYPTED))
        .collect();
    assert!(
        encrypted.len() >= 2,
        "bob should have sent two encrypted messages"
    );
    let m2 = encrypted.pop().unwrap();
    let m1 = encrypted.pop().unwrap();

    // Simulate m1 having been queued on the owner before the group existed.
    alice.enqueue_pending_decryption("bob", &m1);
    assert!(alice.pending_queue.contains_peer("bob"));

    // Confirming decrypt of m2: this is the ONLY drain trigger available to the
    // owner (no Welcome/probe/ack is ever received here).
    let _ = alice.process_internal_message(&m2);
    assert!(
        alice.confirmed_sessions.contains("bob"),
        "owner must confirm via decrypt_success"
    );
    assert!(
        !alice.pending_queue.contains_peer("bob"),
        "the decrypt-confirmation must drain the pending queue (change-6)"
    );
    assert!(
        received
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == "queued-earlier"),
        "the queued message must surface once the session confirms via decrypt"
    );
}

/// Media-chunk parity for the deferred-ACK keystone (test d): an encrypted media
/// chunk that arrives before the session is ready is queued but NOT ACKed, its
/// id is unmarked so resends re-enter (and do not stack), and it drains and is
/// deduped once the Welcome establishes the session.
#[test]
fn test_deferred_media_chunk_defers_ack_then_recovers() {
    // Bob has no session; alice has an alice-side confirmed session.
    let (mut bob, bob_handle) = media_test_protocol("bob");

    let (mut alice, alice_handle) = media_test_protocol("alice");
    let bob_kp = {
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.generate_key_package().unwrap()
    };
    let welcome = {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    alice.confirm_session_state("bob", "test_setup").unwrap();

    let file_events: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let file_events_handle = Arc::clone(&file_events);
    bob.on_event(move |event| {
        if matches!(event, Event::FileReceived { .. }) {
            *file_events_handle.lock().unwrap() += 1;
        }
    });

    // Single small chunk so the transfer completes in one message.
    alice
        .send_media("bob", vec![7u8; 512], "s.bin", ContentType::File, None)
        .unwrap();
    let chunk_wire = alice_handle
        .sent_messages()
        .into_iter()
        .find(|m| m.content_type == ContentType::FileChunk)
        .expect("alice should have sent a media chunk");
    let chunk_id = chunk_wire.id.clone();
    let chunk_id_str = chunk_id.as_str().to_string();
    let acks_for_chunk = |handle: &MockTransport| -> usize {
        handle
            .sent_messages()
            .into_iter()
            .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&chunk_id_str))
            .count()
    };

    // 1) Chunk arrives before session ready: queued, unmarked, not ACKed.
    bob_handle.queue_message(chunk_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);
    assert!(!bob.deduplicator.is_duplicate(&chunk_id));
    assert_eq!(
        acks_for_chunk(&bob_handle),
        0,
        "a deferred chunk must not be ACKed"
    );
    assert_eq!(*file_events.lock().unwrap(), 0);

    // 2) Resend before ready: reprocessed, still one queue entry, still no ACK.
    bob_handle.queue_message(chunk_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);
    assert_eq!(acks_for_chunk(&bob_handle), 0);

    // 3) Welcome establishes the session and drains the chunk to completion.
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
    bob_handle.queue_message(welcome_wire);
    while bob.receive_message().is_some() {}
    assert!(
        !bob.pending_queue.contains_peer("alice"),
        "chunk queue must drain on Welcome"
    );
    assert_eq!(
        *file_events.lock().unwrap(),
        1,
        "the file must complete after drain"
    );
    assert!(
        bob.deduplicator.is_duplicate(&chunk_id),
        "a drained chunk must be re-marked so a resend is deduped"
    );

    // 4) A post-drain resend is deduped + re-ACKed, not reprocessed.
    bob_handle.clear_sent_messages();
    bob_handle.queue_message(chunk_wire.clone());
    while bob.receive_message().is_some() {}
    assert_eq!(
        *file_events.lock().unwrap(),
        1,
        "post-drain resend must not re-deliver"
    );
    assert_eq!(
        acks_for_chunk(&bob_handle),
        1,
        "post-drain resend must be re-ACKed"
    );
}

/// ACK-latency window closure: the drain now sends the deferred delivery ACK
/// directly on the message's recorded arrival transport, so the sender learns of
/// delivery WITHOUT having to resend. This pins the behavior that replaced the
/// old "drain surfaces locally but never ACKs" semantic — a sender that had
/// exhausted its retry budget before the session confirmed would otherwise mark
/// a locally-delivered message undeliverable. No resend happens in this test:
/// the single ACK is produced by the drain alone.
#[test]
fn test_deferred_dm_is_acked_on_drain_without_a_resend() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));
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
    let encrypted = alice_manager
        .encrypt_for_user("bob", b"before-welcome")
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
    let msg_id_str = encrypted_wire.id.as_str().to_string();
    let acks_for_msg = |handle: &MockTransport| -> usize {
        handle
            .sent_messages()
            .into_iter()
            .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&msg_id_str))
            .count()
    };

    // Deferred before the session is ready: queued, not ACKed.
    bob_handle.queue_message(encrypted_wire);
    while bob.receive_message().is_some() {}
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);
    assert_eq!(
        acks_for_msg(&bob_handle),
        0,
        "a deferred message must not be ACKed on receipt"
    );

    // Welcome drains the queue. The drain must ACK the surfaced message on its
    // recorded arrival transport — with NO resend from the sender.
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
    bob_handle.queue_message(welcome_wire);
    while bob.receive_message().is_some() {}
    assert!(!bob.pending_queue.contains_peer("alice"));
    assert_eq!(
        acks_for_msg(&bob_handle),
        1,
        "the drain must send the deferred ACK directly, without waiting for a resend"
    );
}

/// Fix parity for the plaintext-media gate: an unencrypted media chunk received
/// under `require_encryption` is rejected exactly like plaintext text — no
/// delivery ACK (don't confirm processing to an injector) and the id is left
/// unmarked (so a replay re-enters the gate instead of the duplicate re-ACK
/// path). Previously such a chunk was ACKed and left dedup-marked.
#[test]
fn test_plaintext_media_chunk_rejected_withholds_ack_and_unmarks() {
    use crate::file_transfer::FileChunk;

    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut bob = OfflineProtocol::new(config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
    bob_transport.start().unwrap();
    let bob_handle = bob_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(bob_transport));

    let file_events: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let file_events_handle = Arc::clone(&file_events);
    bob.on_event(move |event| {
        if matches!(event, Event::FileReceived { .. }) {
            *file_events_handle.lock().unwrap() += 1;
        }
    });
    bob.start().unwrap();

    let chunk = FileChunk {
        file_id: "plain-file".to_string(),
        file_name: "photo.jpg".to_string(),
        file_size: 6,
        total_chunks: 1,
        chunk_index: 0,
        chunk_data: vec![1u8; 6],
        file_checksum: "abc".to_string(),
    };
    let chunk_wire = Message::builder(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(chunk.to_json().unwrap())
    .content_type(ContentType::FileChunk)
    .build();
    let chunk_id = chunk_wire.id.clone();
    let chunk_id_str = chunk_id.as_str().to_string();

    bob_handle.queue_message(chunk_wire);
    while bob.receive_message().is_some() {}

    assert_eq!(
        *file_events.lock().unwrap(),
        0,
        "a policy-rejected plaintext chunk must not surface a file"
    );
    assert_eq!(
        bob_handle
            .sent_messages()
            .into_iter()
            .filter(|m| m.metadata.get(ACK_FOR_KEY) == Some(&chunk_id_str))
            .count(),
        0,
        "a policy-rejected plaintext chunk must not be ACKed"
    );
    assert!(
        !bob.deduplicator.is_duplicate(&chunk_id),
        "a policy-rejected plaintext chunk must be unmarked so a replay re-enters the gate"
    );
}

#[test]
fn test_welcome_duplicate_delivery_emits_single_established_event() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    bob_config.encryption.store_pending = true;

    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    restarted.initialize_mls_for_test(storage).unwrap();

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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
        None,
        ContentType::default(),
        None,
        None,
    );
    assert!(protocol.load_session_state_entry("bob").unwrap().is_none());

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls_for_test(storage).unwrap();

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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
        None,
        ContentType::default(),
        None,
        None,
    );

    let mut restarted = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    restarted.config.encryption.enabled = true;
    restarted.config.encryption.store_pending = true;
    restarted.initialize_mls_for_test(storage).unwrap();

    let transport = MockTransport::new(TransportType::BLE);
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
    alice
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();
    bob.initialize_mls_for_test(bob_storage.clone()).unwrap();

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
    alice2.initialize_mls_for_test(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls_for_test(bob_storage).unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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
    alice
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();
    bob.initialize_mls_for_test(bob_storage.clone()).unwrap();

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
    alice2.initialize_mls_for_test(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls_for_test(bob_storage).unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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
    alice
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();
    bob.initialize_mls_for_test(bob_storage.clone()).unwrap();

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
    alice2.initialize_mls_for_test(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls_for_test(bob_storage).unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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
    protocol.initialize_mls_for_test(storage).unwrap();

    let transport = MockTransport::new(TransportType::BLE);
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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

    let transport = MockTransport::new(TransportType::BLE);
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
    alice
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();
    bob.initialize_mls_for_test(bob_storage.clone()).unwrap();

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
        None,
        ContentType::default(),
        None,
        None,
    );
    bob.queue_pending_message(
        "alice",
        "queued-before-restart-b2a",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
        None,
    );

    let mut alice2 = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    alice2.config.encryption.enabled = true;
    alice2.config.encryption.store_pending = true;
    alice2.initialize_mls_for_test(alice_storage).unwrap();
    let mut bob2 = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob2.config.encryption.enabled = true;
    bob2.config.encryption.store_pending = true;
    bob2.initialize_mls_for_test(bob_storage).unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice2.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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
    assert!(protocol.pending_queue.is_empty());

    // Queue an encrypted message for a sender
    let message = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "encrypted content",
    );

    protocol.enqueue_pending_decryption("sender123", &message);

    // Check message is queued
    assert!(protocol.pending_queue.contains_peer("sender123"));
    assert_eq!(protocol.pending_queue.peer_queue_len("sender123"), 1);

    // Queue another message from same sender
    let message2 = Message::new(
        UserId::new("sender123").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "more encrypted content",
    );

    protocol.enqueue_pending_decryption("sender123", &message2);

    assert_eq!(protocol.pending_queue.peer_queue_len("sender123"), 2);
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

    assert!(!protocol.pending_queue.is_empty());

    // Calling process_pending_decryption should remove the entries
    // (even if decryption fails since MLS is not initialized)
    protocol.process_pending_decryption("sender123");

    // The messages should be removed from the pending queue
    assert!(!protocol.pending_queue.contains_peer("sender123"));
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
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
    alice_transport.start().unwrap();
    let alice_transport_handle = alice_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(alice_transport));
    alice.start().unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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

    let encrypted_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let flags_clone = encrypted_flags.clone();
    bob.on_event(move |event| {
        if let Event::MessageReceived { encrypted, .. } = event {
            flags_clone.lock().unwrap().push(encrypted);
        }
    });

    bob_transport_handle.queue_message(encrypted_wire);
    let received = bob.receive_message().expect("expected decrypted message");
    assert_eq!(received.content, "hello-through-mls");
    assert_eq!(
        received.metadata.get("encrypted").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        *encrypted_flags.lock().unwrap(),
        vec![true],
        "decrypted delivery must surface with encrypted=true on the event"
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
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
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
        Some(InternalMessageResult::Deferred)
    ));
    assert!(matches!(
        second_result,
        Some(InternalMessageResult::Deferred)
    ));
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);
    assert_eq!(
        bob.pending_queue
            .peek_entry("alice", 0)
            .unwrap()
            .message
            .id
            .as_str(),
        first_message.id.as_str()
    );
    assert_eq!(
        bob.pending_queue
            .metrics()
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
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

    assert!(matches!(result, Some(InternalMessageResult::Deferred)));
    assert!(protocol.pending_queue.contains_peer("sender123"));
    assert_eq!(protocol.pending_queue.peer_queue_len("sender123"), 1);
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

    assert_eq!(protocol.pending_queue.total(), 32);
    assert_eq!(
        protocol.pending_queue.metrics().pending_messages_current,
        32
    );
    assert_eq!(
        *protocol
            .pending_queue
            .metrics()
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
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
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
        let is_invalid = idx % 5 == 0;
        let content = if is_invalid {
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
        // Malformed payloads are consumed (nothing to queue or retry); a
        // valid-but-early encrypted message is deferred (queued, not ACKed).
        if is_invalid {
            assert!(matches!(result, Some(InternalMessageResult::Consumed)));
        } else {
            assert!(matches!(result, Some(InternalMessageResult::Deferred)));
        }
    }

    let per_peer_limit = protocol
        .config
        .encryption
        .pending_queue
        .max_pending_per_peer;
    let global_limit = protocol.config.encryption.pending_queue.max_pending_global;
    assert!(protocol.pending_queue.total() <= global_limit);
    assert!(protocol.pending_queue.peer_queue_len("sender123") <= per_peer_limit);

    let metrics = protocol.pending_queue_metrics();
    assert_eq!(metrics.pending_messages_received_total, early_count);
    assert_eq!(
        metrics.pending_messages_current,
        protocol.pending_queue.total()
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

    assert!(protocol.pending_queue.total() <= 6);
    assert!(protocol.pending_queue.peer_queue_len("noisy-peer") <= 3);
    assert!(protocol.pending_queue.contains_peer("peer-a"));
    assert!(protocol.pending_queue.contains_peer("peer-b"));
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

    assert_eq!(protocol.pending_queue.peer_queue_len("peer-a"), 1);
    assert_eq!(
        protocol
            .pending_queue
            .peek_entry("peer-a", 0)
            .unwrap()
            .message
            .content,
        "first"
    );
    assert_eq!(
        protocol
            .pending_queue
            .metrics()
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
    assert_eq!(protocol.pending_queue.total(), 1);

    // Simulate index drift: queue has data but global-order index is empty.
    protocol.pending_queue.corrupt_clear_global_order();

    protocol.enqueue_pending_decryption("peer-b", &pending_test_message("peer-b", "m2"));

    assert_eq!(protocol.pending_queue.total(), 1);
    assert!(!protocol.pending_queue.contains_peer("peer-b"));
    assert!(protocol.pending_queue.contains_peer("peer-a"));
    assert!(
        protocol
            .pending_queue
            .metrics()
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

    protocol
        .pending_queue
        .set_front_received_at("sender123", Instant::now() - Duration::from_millis(2_000));

    let config = protocol.config.encryption.pending_queue.clone();
    let expired =
        protocol
            .pending_queue
            .prune_expired_for_peer(&config, "sender123", Instant::now());
    assert_eq!(expired.len(), 1);
    assert_eq!(protocol.pending_queue.peer_queue_len("sender123"), 1);
    assert_eq!(
        protocol
            .pending_queue
            .metrics()
            .pending_messages_expired_total,
        1
    );
}

#[test]
fn test_pending_messages_replay_decrypt_after_session_readiness() {
    let mut bob_config = create_test_config_for_user("bob");
    bob_config.encryption.enabled = true;
    let mut bob = OfflineProtocol::new(bob_config).unwrap();
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
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
    assert!(matches!(result, Some(InternalMessageResult::Deferred)));
    assert_eq!(bob.pending_queue.peer_queue_len("alice"), 1);

    {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.join_session(&welcome).unwrap();
    }

    bob.process_pending_decryption("alice");
    assert!(!bob.pending_queue.contains_peer("alice"));
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
    assert!(protocol.pending_queue.total() <= 64);
    assert!(protocol.pending_queue.max_peer_queue_len() <= 8);
}

// ========================================================================
// LAMPORT CLOCK TESTS
// ========================================================================

use crate::mls::InMemoryStorage;

#[test]
fn test_lamport_clock_advances_on_send() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
        let mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        protocol
            .enable_message_persistence_for_test(storage.clone())
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
        let mock_transport = MockTransport::new(TransportType::BLE);
        mock_transport.start().unwrap();
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(mock_transport));

        assert_eq!(protocol.lamport_clock.value(), 0);

        protocol
            .enable_message_persistence_for_test(storage.clone())
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
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    // Clock should remain at 0 (corrupted data ignored)
    assert_eq!(protocol.lamport_clock.value(), 0);
}

#[test]
fn test_lamport_clock_debounce_threshold() {
    let storage = Arc::new(InMemoryStorage::new());

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));

    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    protocol.start().unwrap();

    // Send (LAMPORT_PERSIST_INTERVAL - 1) messages — storage should NOT be written yet.
    let below_threshold = (LAMPORT_PERSIST_INTERVAL - 1) as usize;
    for i in 0..below_threshold {
        protocol
            .send_message(
                "bob",
                format!("msg{}", i),
                None::<MessagePriority>,
                None::<String>,
            )
            .unwrap();
    }
    assert_eq!(protocol.lamport_clock.value(), below_threshold as u64);

    // Storage should still have the initial value (0) — debounce hasn't fired.
    let raw = storage
        .load(storage_keys::LAMPORT_CLOCK, storage_keys::LAMPORT_CLOCK_ID)
        .unwrap();
    assert!(
        raw.is_none() || raw.as_deref() == Some(&0u64.to_le_bytes()[..]),
        "Lamport clock should not be persisted below the debounce threshold"
    );

    // One more send crosses the threshold — storage should now be written.
    protocol
        .send_message("bob", "threshold", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), LAMPORT_PERSIST_INTERVAL);

    let raw = storage
        .load(storage_keys::LAMPORT_CLOCK, storage_keys::LAMPORT_CLOCK_ID)
        .unwrap()
        .expect("Lamport clock should be persisted at the threshold");
    let persisted = u64::from_le_bytes(raw.try_into().unwrap());
    assert_eq!(persisted, LAMPORT_PERSIST_INTERVAL);
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
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(protocol.lamport_clock.value(), 20);
}

#[test]
fn test_lamport_clock_merge_on_internal_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    // Create a key package message with a high Lamport clock
    let key_pkg_payload = KeyPackagePayload {
        user_id: "sender456".to_string(),
        key_package_data: vec![5, 6, 7, 8],
        remaining_lifetime_ms: 30 * 24 * 60 * 60 * 1000,
        timestamp_ms: 12345,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();

    let payload = ConnectionRequestPayload {
        sender_name: "Alice".to_string(),
        timestamp_ms: 12345,
        key_package: None,
        initial_message: None,
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
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
    protocol.initialize_mls_for_test(storage).unwrap();

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

    // First session: receive key package — auto_key_exchange causes
    // handle_key_package_message to auto-establish the session, consuming
    // the pending key package.
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls_for_test(storage.clone()).unwrap();
        let key_pkg_payload = KeyPackagePayload {
            user_id: "bob".to_string(),
            key_package_data: bob_key_package.key_package_data.clone(),
            remaining_lifetime_ms: 60 * 60 * 1000,
            timestamp_ms: 0,
            session_reset: false,
            wire_versions: Vec::new(),
            env_versions: Vec::new(),
            rich_versions: Vec::new(),
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
        // Session is auto-established, key package consumed
        assert!(
            !protocol.pending_key_packages.contains_key("bob"),
            "Key package should be consumed by auto-establish"
        );
        assert!(
            protocol.has_mls_session("bob").unwrap(),
            "Session should be auto-established after key package exchange"
        );
    }

    // Second session: new protocol, same storage; MLS session should be
    // restorable from the shared storage.
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls_for_test(storage.clone()).unwrap();
        // Session already exists from the first instance (shared storage)
        let welcome = protocol.establish_secure_session("bob").unwrap();
        assert!(
            welcome.is_none(),
            "Session already exists, no new welcome needed"
        );
    }
}

#[test]
fn test_pending_key_packages_capped_evicts_soonest_to_expire() {
    // Regression (H2): `pending_key_packages` is keyed by the wire-claimed
    // `sender`, and every insert also writes a durable Keychain/Keystore entry,
    // so an unpinned peer flooding distinct forged `__MLS_KEY_PKG__` senders
    // (accepted under the default config) would grow both memory and durable
    // storage without bound and re-inflate on every reboot. The map must be
    // capped like `known_peers`: at capacity a new peer evicts the
    // soonest-to-expire entry (and its persisted copy) rather than growing past
    // the cap.
    let mut config = create_test_config();
    // Keep the flooded entry resident: with auto-exchange off the handler just
    // inserts it, instead of auto-establishing and consuming it.
    config.encryption.auto_key_exchange = false;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    // Fill the map to capacity. One entry ("victim") is the unambiguous
    // soonest-to-expire; the rest expire far in the future.
    protocol.pending_key_packages.insert(
        "victim".to_string(),
        ReceivedKeyPackage {
            key_package_data: vec![0],
            local_expires_at_ms: 1, // smallest expiry -> the eviction target
        },
    );
    for i in 0..(MAX_PENDING_KEY_PACKAGES - 1) {
        protocol.pending_key_packages.insert(
            format!("filler_{i}"),
            ReceivedKeyPackage {
                key_package_data: vec![0],
                local_expires_at_ms: 1_000_000_000_000 + i as u64,
            },
        );
    }
    assert_eq!(
        protocol.pending_key_packages.len(),
        MAX_PENDING_KEY_PACKAGES
    );

    // A new forged sender arrives — this must evict, not grow past the cap.
    let key_pkg_payload = KeyPackagePayload {
        user_id: "attacker".to_string(),
        // Invalid bytes are fine: the insert precedes any MLS use of the data.
        key_package_data: vec![1, 2, 3],
        remaining_lifetime_ms: 60 * 60 * 1000,
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&key_pkg_payload).unwrap()
    );
    let message = Message::new(
        UserId::new("attacker").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    let _ = protocol.process_internal_message(&message);

    assert_eq!(
        protocol.pending_key_packages.len(),
        MAX_PENDING_KEY_PACKAGES,
        "map must stay bounded at the cap, not grow past it"
    );
    assert!(
        protocol.pending_key_packages.contains_key("attacker"),
        "the new key package should be inserted"
    );
    assert!(
        !protocol.pending_key_packages.contains_key("victim"),
        "the soonest-to-expire entry should have been evicted"
    );
}

#[test]
fn test_received_key_package_lifetime_is_clamped() {
    // Regression (H2 follow-up): `remaining_lifetime_ms` is an unauthenticated
    // wire field that becomes the eviction sort key for `pending_key_packages`
    // (soonest-to-expire is evicted first). A forged sender claiming a maximal
    // lifetime must not pin its entry as latest-to-expire and starve legitimate
    // peers, so the cached expiry is clamped to MAX_KEY_PACKAGE_LIFETIME_MS.
    let mut config = create_test_config();
    // Keep the entry resident (no auto-establish) so we can inspect its expiry.
    config.encryption.auto_key_exchange = false;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let before_ms = chrono::Utc::now().timestamp_millis() as u64;
    let key_pkg_payload = KeyPackagePayload {
        user_id: "attacker".to_string(),
        key_package_data: vec![1, 2, 3],
        remaining_lifetime_ms: u64::MAX, // attacker claims a maximal lifetime
        timestamp_ms: 0,
        session_reset: false,
        wire_versions: Vec::new(),
        env_versions: Vec::new(),
        rich_versions: Vec::new(),
    };
    let content = format!(
        "{}{}",
        internal_prefixes::KEY_PACKAGE,
        serde_json::to_string(&key_pkg_payload).unwrap()
    );
    let message = Message::new(
        UserId::new("attacker").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    let _ = protocol.process_internal_message(&message);
    let after_ms = chrono::Utc::now().timestamp_millis() as u64;

    let pkg = protocol
        .pending_key_packages
        .get("attacker")
        .expect("key package should be inserted");
    // Expiry is anchored to *our* clock and bounded by the clamp: it lands in
    // [before + MAX, after + MAX], never u64::MAX (which the unclamped
    // saturating_add would have produced).
    assert!(
        pkg.local_expires_at_ms >= before_ms.saturating_add(MAX_KEY_PACKAGE_LIFETIME_MS)
            && pkg.local_expires_at_ms <= after_ms.saturating_add(MAX_KEY_PACKAGE_LIFETIME_MS),
        "a maximal claimed lifetime must be clamped to MAX_KEY_PACKAGE_LIFETIME_MS (got {})",
        pkg.local_expires_at_ms
    );
}

#[test]
fn test_restore_peer_key_packages_prunes_overflow_from_storage() {
    // Regression (H2 follow-up): a pre-cap over-sized durable store (a flood
    // that landed before MAX_PENDING_KEY_PACKAGES existed) must not re-inflate
    // memory on boot or linger on disk forever. The restore loop bounds memory
    // to the cap AND prunes the on-disk overflow so the store shrinks to the
    // cap in a single boot.
    let storage = Arc::new(InMemoryStorage::new());
    let overflow = 5;
    let total = MAX_PENDING_KEY_PACKAGES + overflow;

    // Persist more than the cap of non-expired, non-session peer key packages,
    // writing straight to durable storage to bypass the live insert cap.
    {
        let mut writer = OfflineProtocol::new(create_test_config()).unwrap();
        writer.initialize_mls_for_test(storage.clone()).unwrap();
        let pkg = ReceivedKeyPackage {
            key_package_data: vec![0],
            local_expires_at_ms: u64::MAX, // never expired
        };
        for i in 0..total {
            writer.persist_peer_key_package(&format!("peer_{i}"), &pkg);
        }
    }
    assert_eq!(
        storage
            .list_keys(storage_keys::PEER_KEY_PACKAGES)
            .unwrap()
            .len(),
        total,
        "precondition: durable store holds more than the cap"
    );

    // Boot a fresh instance against the same storage; initialize_mls runs the
    // restore path.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

    assert_eq!(
        protocol.pending_key_packages.len(),
        MAX_PENDING_KEY_PACKAGES,
        "restore must bound memory to the cap"
    );
    assert_eq!(
        storage
            .list_keys(storage_keys::PEER_KEY_PACKAGES)
            .unwrap()
            .len(),
        MAX_PENDING_KEY_PACKAGES,
        "overflow must be pruned from durable storage, not left to linger"
    );
}

#[test]
fn test_establishment_state_returns_correct_states() {
    let storage = Arc::new(InMemoryStorage::new());
    let bob_storage = Arc::new(InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_key_package = bob_manager.get_or_create_key_package().unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

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

    // First session: receive key package — auto-establish creates the session
    // immediately, consuming the key package.
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls_for_test(storage.clone()).unwrap();
        let key_pkg_payload = KeyPackagePayload {
            user_id: "bob".to_string(),
            key_package_data: bob_key_package.key_package_data.clone(),
            remaining_lifetime_ms: 60 * 60 * 1000,
            timestamp_ms: 0,
            session_reset: false,
            wire_versions: Vec::new(),
            env_versions: Vec::new(),
            rich_versions: Vec::new(),
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
        // Auto-establish consumed the key package and created the session
        assert!(
            protocol.has_mls_session("bob").unwrap(),
            "Session should be auto-established after key package receipt"
        );
    }

    // New protocol instance: session should be restorable from shared storage
    {
        let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
        protocol.initialize_mls_for_test(storage.clone()).unwrap();
        // Session already exists from the first instance
        let result = protocol.establish_secure_session("bob");
        assert!(
            result.is_ok(),
            "establish_secure_session should succeed, got {:?}",
            result
        );
        let welcome = result.unwrap();
        assert!(
            welcome.is_none(),
            "Session already exists, no new welcome needed"
        );
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
    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_require_encryption_allows_service_control_messages() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    // Service messages are internal protocol messages (not user content),
    // so they must work even with require_encryption=true.
    // discover_services with no known peers returns Ok with empty broadcast.
    let discover_result = protocol.discover_services(None);
    assert!(discover_result.is_ok());

    // Add a known peer so service request has a target
    protocol.on_neighbor_discovered("bob");
    let request_result = protocol.send_service_request("bob", "echo.v1", "ping", "{}");
    assert!(request_result.is_ok());

    let respond_result =
        protocol.respond_to_service_request("req-1", "alice", "echo.v1", "ok", "pong");
    assert!(respond_result.is_ok());
}

#[test]
fn test_known_peers_capacity_evicts_least_recently_seen() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Fill to capacity
    for i in 0..MAX_KNOWN_PEERS {
        protocol.on_neighbor_discovered(&format!("peer-{i}"));
    }
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);

    // Re-discovering an existing peer at capacity refreshes it, no eviction
    protocol.on_neighbor_discovered("peer-0");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
    assert!(protocol.known_peers.contains_key("peer-0"));

    // Backdate one entry so the eviction victim is deterministic
    *protocol.known_peers.get_mut("peer-7").unwrap() -= Duration::from_secs(60);
    protocol.key_package_sent_to.insert("peer-7".to_string());

    // A new peer discovered at capacity is tracked; the least-recently-seen
    // entry is evicted (issue #140: a local BLE neighbor must never be
    // locked out by stale message-path senders).
    protocol.on_neighbor_discovered("peer-overflow");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS);
    assert!(protocol.known_peers.contains_key("peer-overflow"));
    assert!(!protocol.known_peers.contains_key("peer-7"));
    // Eviction mirrors on_neighbor_lost: the key-package marker is cleared
    // so the peer receives a fresh key package if it re-appears.
    assert!(!protocol.key_package_sent_to.contains("peer-7"));

    // Explicit loss still frees capacity
    protocol.on_neighbor_lost("peer-overflow");
    assert_eq!(protocol.known_peers.len(), MAX_KNOWN_PEERS - 1);
    assert!(!protocol.known_peers.contains_key("peer-overflow"));
}

#[test]
fn test_known_peers_ttl_eviction() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.on_neighbor_discovered("alice");
    protocol.on_neighbor_discovered("bob");
    protocol.key_package_sent_to.insert("alice".to_string());

    // Fresh entries survive a sweep at the current instant
    protocol.prune_stale_known_peers(std::time::Instant::now());
    assert_eq!(protocol.known_peers.len(), 2);

    // Beyond the TTL, unseen entries are evicted together with their
    // key-package markers
    let past_ttl = std::time::Instant::now() + Duration::from_secs(KNOWN_PEER_TTL_SECS + 1);
    protocol.prune_stale_known_peers(past_ttl);
    assert!(protocol.known_peers.is_empty());
    assert!(!protocol.is_known_peer("alice"));
    assert!(!protocol.key_package_sent_to.contains("alice"));
}

#[test]
fn test_known_peers_rediscovery_refreshes_last_seen() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.on_neighbor_discovered("alice");
    protocol.on_neighbor_discovered("bob");
    for seen in protocol.known_peers.values_mut() {
        *seen -= Duration::from_secs(60);
    }

    // Re-discovering alice refreshes her last-seen; bob stays backdated
    protocol.on_neighbor_discovered("alice");

    // At this sweep point bob's age exceeds the TTL but alice's does not
    let sweep_at = std::time::Instant::now() + Duration::from_secs(KNOWN_PEER_TTL_SECS - 30);
    protocol.prune_stale_known_peers(sweep_at);
    assert!(protocol.known_peers.contains_key("alice"));
    assert!(!protocol.known_peers.contains_key("bob"));
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
    assert!(protocol.known_peers.contains_key("alice"));

    protocol.on_neighbor_lost("alice");
    assert!(!protocol.known_peers.contains_key("alice"));
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
fn test_validate_transport_sender_relayed_hop_mismatch_passes() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    // A mesh-relayed frame: the transport identity names the carrier ("carol"),
    // not the origin ("alice"). hop_count >= 1 exempts it from the strict match.
    msg.increment_hop().unwrap();
    msg.set_transport_peer_id("carol".to_string()).unwrap();

    assert!(
        protocol.validate_transport_sender(&msg),
        "relayed frame (hop > 0) must not be rejected for carrier/origin mismatch"
    );
}

#[test]
fn test_validate_transport_sender_hop_zero_mismatch_still_rejected() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "hello",
    );
    // hop_count == 0 claims the carrier IS the origin — mismatch is spoofing.
    msg.set_transport_peer_id("eve".to_string()).unwrap();

    assert!(!protocol.validate_transport_sender(&msg));
}

#[test]
fn test_relayed_control_message_with_carrier_identity_not_security_rejected() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // A control message from "sender123" relayed by "carol" (hop 1): the
    // security gate must not drop it for the carrier/origin mismatch.
    let mut msg = pending_test_message(
        "sender123",
        &format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    msg.increment_hop().unwrap();
    msg.set_transport_peer_id("carol".to_string()).unwrap();

    let result = protocol.process_internal_message(&msg);
    assert!(
        !matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "relayed control message must pass the transport-identity gate"
    );
}

#[test]
fn test_control_message_with_transport_mismatch_is_dropped() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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
    protocol.initialize_mls_for_test(storage).unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

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
    alice.initialize_mls_for_test(storage_a).unwrap();

    // Set up "bob" as the receiver with MLS initialized so he can verify.
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage_b = Arc::new(crate::mls::InMemoryStorage::new());
    bob.initialize_mls_for_test(storage_b).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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
    alice.initialize_mls_for_test(storage_a).unwrap();

    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage_b = Arc::new(crate::mls::InMemoryStorage::new());
    bob.initialize_mls_for_test(storage_b).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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

    let mock_transport = MockTransport::new(TransportType::BLE);
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
        .initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let alice_transport = MockTransport::new(TransportType::BLE);
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
    bob.initialize_mls_for_test(Arc::new(InMemoryStorage::new()))
        .unwrap();

    let bob_transport = MockTransport::new(TransportType::BLE);
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

    // __GROUP_MSG__ is data-plane too: the relay's fan-out re-emits it per
    // member without the origin's Ed25519 metadata, and MLS authenticates
    // the payload after the gate instead.
    let msg = pending_test_message(
        "alice",
        &format!(
            "{}{{\"group_id\":\"g1\",\"sender\":\"alice\",\"content\":\"AAECAw==\"}}",
            internal_prefixes::GROUP_MSG
        ),
    );
    let result = protocol.security_gate_control_message(&msg);
    assert!(
        result.is_none(),
        "Security gate must NOT block __GROUP_MSG__ fan-out — MLS authenticates it after the gate"
    );
}

/// Regression for the relay group fan-out drop: a `GroupMessageReceived`
/// frame is rebuilt by the bridge as an unsigned `__GROUP_MSG__` (hop 0,
/// transport identity = the relay-authenticated sender). Before
/// `__GROUP_MSG__` was data-plane, a TOFU-pinned sender made the gate drop
/// the whole frame as a signature downgrade — group messages over the relay
/// silently vanished for exactly the peers you'd already talked to. The
/// frame must now reach dispatch (not SecurityRejected); sender
/// authentication is MLS's job at decrypt time.
#[test]
fn test_unsigned_group_msg_from_pinned_sender_reaches_dispatch() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.tofu_check_or_pin("alice", vec![1u8; 32]).unwrap();
    assert!(protocol.known_peer_public_keys.contains_key("alice"));

    let mut msg = pending_test_message(
        "alice",
        &format!(
            "{}{{\"group_id\":\"g1\",\"sender\":\"alice\",\"content\":\"AAECAw==\"}}",
            internal_prefixes::GROUP_MSG
        ),
    );
    msg.set_transport_peer_id("alice".to_string()).unwrap();

    let result = protocol.process_internal_message(&msg);
    assert!(
        !matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "unsigned __GROUP_MSG__ from a pinned sender must not be gate-dropped, got {:?}",
        result
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
    protocol.initialize_mls_for_test(storage).unwrap();

    // Set up "alice" as a sender with MLS so she can sign.
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage_a).unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage_a).unwrap();

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
    bob.initialize_mls_for_test(storage_b).unwrap();

    let mock_transport = MockTransport::new(TransportType::BLE);
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

#[test]
fn test_mls_enc_spoofed_sender_security_rejected() {
    // SEC-M1 on the __MLS_ENC__ path: a valid 1:1 ciphertext delivered under
    // a spoofed wire sender must map to SecurityRejected — the MLS credential
    // authenticates "bob", so an envelope claiming "mallory" must not be
    // surfaced. (SecurityRejected → no delivery ACK is covered by
    // test_security_rejected_does_not_send_ack.)
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage).unwrap();

    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_kp = bob_manager.generate_key_package().unwrap();

    // Establish a converged alice <-> bob session.
    let welcome = {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    bob_manager.join_session(&welcome).unwrap();

    // Bob encrypts a legitimate message for alice...
    let encrypted = bob_manager.encrypt_for_user("alice", b"hi alice").unwrap();
    let content = format!(
        "{}{}",
        internal_prefixes::ENCRYPTED,
        serde_json::to_string(&encrypted).unwrap()
    );

    // ...but the wire envelope claims it came from "mallory".
    let message = Message::new(
        UserId::new("mallory").unwrap(),
        UserId::new("alice").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = alice.process_internal_message(&message);
    assert!(
        matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "spoofed __MLS_ENC__ message must be SecurityRejected"
    );

    // The same ciphertext under the true sender still decrypts: the spoofed
    // attempt must not have destroyed alice's ability to attribute honestly
    // delivered messages.
    let encrypted2 = bob_manager.encrypt_for_user("alice", b"hi again").unwrap();
    let content2 = format!(
        "{}{}",
        internal_prefixes::ENCRYPTED,
        serde_json::to_string(&encrypted2).unwrap()
    );
    let honest = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("alice").unwrap(),
        AppId::new("test-app").unwrap(),
        &content2,
    );
    let result2 = alice.process_internal_message(&honest);
    assert!(
        matches!(result2, Some(InternalMessageResult::Decrypted(_))),
        "honest message after a spoofed attempt must still decrypt"
    );
}

#[test]
fn test_mls_enc_non_utf8_plaintext_rejected() {
    // SEC-L6: a decrypted payload that is not valid UTF-8 must be dropped
    // with a MessageDecryptionFailed event instead of surfacing a lossily
    // mangled string. Compliant senders always encrypt UTF-8 on the text
    // path (media chunks decrypt separately as bytes), so only a buggy or
    // malicious peer can produce this.
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    alice.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let bob_storage = Arc::new(crate::mls::InMemoryStorage::new());
    let bob_manager = MlsManager::new("bob", bob_storage).unwrap();
    let bob_kp = bob_manager.generate_key_package().unwrap();

    // Establish a converged alice <-> bob session.
    let welcome = {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    bob_manager.join_session(&welcome).unwrap();

    // Bob encrypts bytes that are not valid UTF-8.
    let encrypted = bob_manager
        .encrypt_for_user("alice", &[0xFF, 0xFE, 0xFD])
        .unwrap();
    let content = format!(
        "{}{}",
        internal_prefixes::ENCRYPTED,
        serde_json::to_string(&encrypted).unwrap()
    );
    let message = Message::new(
        UserId::new("bob").unwrap(),
        UserId::new("alice").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );

    let result = alice.process_internal_message(&message);
    assert!(
        matches!(result, Some(InternalMessageResult::Consumed)),
        "non-UTF-8 plaintext must be consumed, not surfaced as Decrypted"
    );

    let captured = events.lock().unwrap();
    assert!(
        captured.iter().any(|e| matches!(
            e,
            Event::MessageDecryptionFailed { code, .. }
                if *code == DecryptionFailureCode::InvalidPayload
        )),
        "non-UTF-8 plaintext must emit MessageDecryptionFailed with InvalidPayload"
    );
}

#[test]
fn test_welcome_with_mismatched_inviter_id_rejected() {
    // Honest peers set `inviter_id` to their own id (see
    // SessionManager::create_session). A Welcome whose payload inviter_id
    // disagrees with the transport sender is forged or tampered and must be
    // dropped before any session state changes — inviter_id is used
    // downstream as a raw storage key.
    let mut bob = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    bob.initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();

    let zoe_manager = MlsManager::new("zoe", Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
    let bob_key_package = {
        let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
        manager.get_or_create_key_package().unwrap()
    };
    zoe_manager
        .import_key_package("bob", &bob_key_package.key_package_data)
        .unwrap();
    let mut welcome = zoe_manager.create_session("bob").unwrap();
    // Tamper: the payload claims a different inviter than the wire sender.
    welcome.inviter_id = "mallory".to_string();

    let content = format!(
        "{}{}",
        internal_prefixes::WELCOME,
        serde_json::to_string(&welcome).unwrap()
    );
    let message = Message::new(
        UserId::new("zoe").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &content,
    );
    bob.process_internal_message(&message);

    let manager = bob.mls_manager.as_ref().unwrap().read().unwrap();
    assert!(
        !manager.has_session("zoe").unwrap(),
        "a Welcome with mismatched inviter_id must not create a session"
    );
}

// ========================================================================
// TOFU PERSISTENCE
// ========================================================================

#[test]
fn test_tofu_entries_persisted_via_storage() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
        protocol_a.initialize_mls_for_test(storage.clone()).unwrap();
        protocol_a
            .tofu_check_or_pin("alice", vec![10u8; 32])
            .unwrap();
    }

    // Protocol B uses the same storage — simulates restart
    let mut protocol_b = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    protocol_b.initialize_mls_for_test(storage.clone()).unwrap();

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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

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
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();

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
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

    // Create a signed control message from alice with no transport_peer_id
    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage_a).unwrap();

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
    protocol.initialize_mls_for_test(storage).unwrap();

    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage_a).unwrap();

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

/// With `require_transport_identity = true`, a frame claiming mesh relay
/// (hop > 0, carrier identity attached) skips the strict identity match —
/// but must not be able to skip the signature too: unsigned → rejected
/// (`UnsignedControlRejected`). Otherwise forging `hop_count` would grant
/// an unsigned frame more than the no-identity path, which the flag
/// rejects outright.
#[test]
fn test_relayed_unsigned_control_rejected_when_identity_required() {
    let mut config = create_test_config_for_user("bob");
    config.security.require_transport_identity = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    msg.increment_hop().unwrap();
    msg.set_transport_peer_id("carol".to_string()).unwrap();

    let result = protocol.security_gate_control_message(&msg);
    assert!(
        matches!(result, Some(InternalMessageResult::SecurityRejected)),
        "unsigned relayed control frame must be rejected under \
         require_transport_identity"
    );
}

/// The hop-count exemption still admits honest relayed traffic under the
/// strict flag: a SIGNED mesh-relayed frame with a carrier identity passes.
/// (The Ed25519 canonical payload deliberately excludes `hop_count`, so
/// relaying — increment + re-send — keeps the origin's signature valid.)
#[test]
fn test_relayed_signed_control_passes_when_identity_required() {
    let mut config = create_test_config_for_user("bob");
    config.security.require_transport_identity = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage_a).unwrap();

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"test\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut msg).unwrap();
    msg.increment_hop().unwrap();
    msg.set_transport_peer_id("carol".to_string()).unwrap();

    let result = protocol.security_gate_control_message(&msg);
    assert!(
        result.is_none(),
        "signed mesh-relayed control frame must pass the strict gate"
    );
}

/// End-to-end regression for the internet relay path: a SIGNED __CONN_REQ__
/// from a TOFU-pinned sender, in exactly the shape the bridge delivers relay
/// ingest (hop 0, transport_peer_id stamped with the relay-authenticated
/// uploader), must pass the gate and emit ConnectionRequestReceived. This is
/// the wire shape connection requests use now that they ship verbatim — the
/// former relay-native translation arrived unsigned and was dropped as a
/// signature downgrade precisely for pinned senders.
#[test]
fn test_signed_conn_request_from_pinned_sender_with_internet_identity() {
    let mut protocol = OfflineProtocol::new(create_test_config_for_user("bob")).unwrap();
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    protocol.initialize_mls_for_test(storage).unwrap();

    let mut alice = OfflineProtocol::new(create_test_config_for_user("alice")).unwrap();
    let storage_a = Arc::new(crate::mls::InMemoryStorage::new());
    alice.initialize_mls_for_test(storage_a).unwrap();

    // Pin alice's key first — any prior signed control contact does this,
    // which is what made the unsigned relay-native rebuild undeliverable.
    let mut pin_msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        format!("{}{{\"data\":\"first\"}}", internal_prefixes::CONN_REQUEST),
    );
    alice.sign_control_message(&mut pin_msg).unwrap();
    assert!(matches!(
        protocol.verify_control_message(&pin_msg),
        Ok(true)
    ));
    assert!(protocol.known_peer_public_keys.contains_key("alice"));

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let payload = ConnectionRequestPayload {
        sender_name: "Alice".to_string(),
        timestamp_ms: 12345,
        key_package: None,
        initial_message: None,
    };
    let mut message = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "{}{}",
            internal_prefixes::CONN_REQUEST,
            serde_json::to_string(&payload).unwrap()
        ),
    );
    alice.sign_control_message(&mut message).unwrap();
    message.set_transport_peer_id("alice".to_string()).unwrap();

    let result = protocol.process_internal_message(&message);
    assert!(matches!(result, Some(InternalMessageResult::Consumed)));

    let captured = events.lock().unwrap();
    assert!(
        captured.iter().any(|e| matches!(
            e,
            Event::ConnectionRequestReceived { sender, sender_name, .. }
                if sender == "alice" && sender_name == "Alice"
        )),
        "signed conn request from pinned sender must emit ConnectionRequestReceived"
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
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();

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
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();

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
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    let keys_a: std::collections::BTreeSet<String> =
        protocol_a.known_peer_public_keys.keys().cloned().collect();

    let mut protocol_b = OfflineProtocol::new(create_test_config()).unwrap();
    protocol_b
        .enable_message_persistence_for_test(storage)
        .unwrap();
    let keys_b: std::collections::BTreeSet<String> =
        protocol_b.known_peer_public_keys.keys().cloned().collect();

    assert_eq!(keys_a.len(), MAX_TOFU_PEERS);
    assert_eq!(
        keys_a, keys_b,
        "Truncation should be deterministic when timestamps are equal"
    );
}

#[test]
fn test_reset_tofu_for_peer_removes_pinned_key() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Pin a key for "alice"
    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: vec![1u8; 32],
            last_seen_ms: 1000,
        },
    );
    assert!(protocol.known_peer_public_keys.contains_key("alice"));

    // Reset should succeed and return true
    let removed = protocol.reset_tofu_for_peer("alice");
    assert!(removed);
    assert!(!protocol.known_peer_public_keys.contains_key("alice"));
}

#[test]
fn test_reset_tofu_for_peer_unknown_peer_is_idempotent() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Resetting a peer that was never pinned should return false, not error
    let removed = protocol.reset_tofu_for_peer("nonexistent");
    assert!(!removed);
}

#[test]
fn test_reset_tofu_for_peer_double_reset_is_idempotent() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: vec![1u8; 32],
            last_seen_ms: 1000,
        },
    );

    assert!(protocol.reset_tofu_for_peer("alice"));
    // Second reset on same peer should return false (already removed)
    assert!(!protocol.reset_tofu_for_peer("alice"));
}

#[test]
fn test_reset_tofu_for_peer_allows_repinning() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let old_key = vec![1u8; 32];
    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: old_key.clone(),
            last_seen_ms: 1000,
        },
    );

    // Reset the key
    assert!(protocol.reset_tofu_for_peer("alice"));

    // Simulate re-pinning with a new key
    let new_key = vec![2u8; 32];
    protocol.known_peer_public_keys.insert(
        "alice".to_string(),
        TofuEntry {
            public_key: new_key.clone(),
            last_seen_ms: 2000,
        },
    );

    assert_eq!(protocol.known_peer_public_keys["alice"].public_key, new_key);
}

// ========================================================================
// RELAY FORWARDING TESTS
// ========================================================================

#[test]
fn test_relay_forwards_third_party_message() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let relay_events_clone = relay_events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::MessageRelayed { .. }) {
            relay_events_clone.lock().unwrap().push(event);
        }
    });

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();

    // Create a message from alice to bob (not for us = user123)
    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello bob via relay",
    );
    let original_ttl = msg.ttl.value();
    let original_hops = msg.hop_count.value();
    mock.queue_message(msg);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    // Receiving should relay the message (not return it to us)
    let received = protocol.receive_message();
    assert!(
        received.is_none(),
        "Third-party message should be relayed, not returned"
    );

    // Verify MessageRelayed event was emitted with correct fields
    let events = relay_events.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "Should emit exactly one MessageRelayed event"
    );
    match &events[0] {
        Event::MessageRelayed {
            sender,
            recipient,
            hop_count,
            remaining_ttl,
            ..
        } => {
            assert_eq!(sender, "alice");
            assert_eq!(recipient, "bob");
            assert_eq!(*hop_count, original_hops + 1);
            assert_eq!(*remaining_ttl, original_ttl - 1);
        }
        _ => panic!("Expected MessageRelayed event"),
    }
}

#[test]
fn relay_restamps_binary_for_capable_third_party() {
    // A relayed message must be (re-)stamped from the relay's own per-peer
    // negotiation for the next hop — not left on whatever codec it arrived with.
    // Here we relay a third-party message to bob, who advertised binary support,
    // and assert the frame that leaves is a binary frame.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let transport_handle = mock.clone();

    // From alice to bob — a third party, since we are user123 — so it is relayed
    // rather than delivered locally.
    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "relay me",
    );
    mock.queue_message(msg);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    // Bob (the physical next hop in this single-hop relay) advertised binary.
    protocol
        .transport_manager_mut()
        .mark_peer_binary_wire("bob", true);
    protocol.start().unwrap();

    assert!(
        protocol.receive_message().is_none(),
        "third-party message should be relayed, not returned"
    );

    // Isolate the frame relayed to bob (any incidental control sends go to other
    // recipients) and assert it was re-stamped binary.
    let sent = transport_handle.sent_messages();
    let to_bob: Vec<_> = sent
        .iter()
        .filter(|m| m.recipient.as_str() == "bob")
        .collect();
    assert_eq!(to_bob.len(), 1, "exactly one frame relayed to bob");
    assert_eq!(to_bob[0].content, "relay me");
    assert_eq!(
        to_bob[0].wire_codec(),
        offline_protocol_core::WireCodec::BinaryV1,
        "relay re-stamps binary for a capable next hop"
    );
}

#[test]
fn test_relay_drops_exhausted_ttl() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let relay_events_clone = relay_events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::MessageRelayed { .. }) {
            relay_events_clone.lock().unwrap().push(event);
        }
    });

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();

    // Create a message from alice to bob, then exhaust its TTL
    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "expiring",
    );
    // Exhaust TTL by decrementing to 0
    while !msg.is_ttl_exhausted() {
        let _ = msg.decrement_ttl();
    }
    mock.queue_message(msg);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let received = protocol.receive_message();
    assert!(
        received.is_none(),
        "TTL-exhausted message should not be returned"
    );

    // No relay event should be emitted
    let events = relay_events.lock().unwrap();
    assert!(
        events.is_empty(),
        "TTL-exhausted message should not emit MessageRelayed"
    );
}

#[test]
fn test_relay_disabled_does_not_forward() {
    use offline_protocol_router::relay::RelayPriority;

    let mut config = create_test_config();
    config.relay.relay_priority = RelayPriority::Never;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let relay_events_clone = relay_events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::MessageRelayed { .. }) {
            relay_events_clone.lock().unwrap().push(event);
        }
    });

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();

    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "should not relay",
    );
    mock.queue_message(msg);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let received = protocol.receive_message();
    assert!(
        received.is_none(),
        "Message for third party should not be returned"
    );

    let events = relay_events.lock().unwrap();
    assert!(
        events.is_empty(),
        "Relay-disabled node should not emit MessageRelayed"
    );
}

#[test]
fn test_relay_preserves_original_sender() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let relay_events_clone = relay_events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::MessageRelayed { .. }) {
            relay_events_clone.lock().unwrap().push(event);
        }
    });

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();

    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "original content",
    );
    let original_id = msg.id.as_str();
    mock.queue_message(msg);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    protocol.receive_message();

    let events = relay_events.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::MessageRelayed {
            message_id,
            sender,
            recipient,
            ..
        } => {
            assert_eq!(message_id, &original_id, "Message ID must be preserved");
            assert_eq!(sender, "alice", "Sender must be preserved");
            assert_eq!(recipient, "bob", "Recipient must be preserved");
        }
        _ => panic!("Expected MessageRelayed event"),
    }
}

#[test]
fn test_local_message_not_relayed() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let relay_events_clone = relay_events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::MessageRelayed { .. }) {
            relay_events_clone.lock().unwrap().push(event);
        }
    });

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();

    // Message addressed to us (user123)
    let msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "Hello user123",
    );
    mock.queue_message(msg.clone());

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    // Message for us should be returned normally
    let received = protocol.receive_message();
    assert!(received.is_some(), "Message for us should be returned");
    assert_eq!(received.unwrap().id, msg.id);

    // No relay event should be emitted
    let events = relay_events.lock().unwrap();
    assert!(
        events.is_empty(),
        "Local message should not emit MessageRelayed"
    );
}

#[test]
fn test_pending_forwarded_message_preserves_forward_count() {
    // Verifies that a forwarded message queued in the pending queue does NOT
    // double-increment forward_count when flushed.
    use offline_protocol_core::ForwardInfo;

    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let original_sender = UserId::new("alice").unwrap();
    let original_msg_id = MessageId::new();
    let forward_info = ForwardInfo {
        original_sender: original_sender.clone(),
        original_message_id: original_msg_id.clone(),
        original_timestamp: offline_protocol_core::Timestamp::now(),
        forward_count: 1,
    };

    // Queue a forwarded message with forward_count = 1
    protocol.queue_pending_message(
        "bob",
        "Hello from Alice",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        Some(forward_info),
        ContentType::default(),
        None,
        None,
    );

    // Verify the stored forward_count is 1
    let bob_pending = protocol.pending_encrypted_messages.get("bob").unwrap();
    assert_eq!(bob_pending.len(), 1);
    let stored = bob_pending[0].forwarded_from.as_ref().unwrap();
    assert_eq!(stored.forward_count, 1);
    assert_eq!(stored.original_sender, original_sender);
    assert_eq!(stored.original_message_id, original_msg_id);
}

#[test]
fn test_forward_message_rejects_internal_prefix_content() {
    use crate::protocol::internal_prefixes;

    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    // Build a message whose content starts with an internal prefix
    let malicious_content = format!("{}evil-payload", internal_prefixes::KEY_PACKAGE);
    let original = offline_protocol_core::Message::builder(
        UserId::new("attacker").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(&malicious_content)
    .build();

    let result = protocol.forward_message(&original, "bob", None);
    let err = result.unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "Prefix injection is a caller-input defect, got: {:?}",
        err
    );
    assert!(
        err.to_string().contains("reserved internal prefix"),
        "Expected prefix injection error, got: {}",
        err
    );
}

#[test]
fn test_forward_message_rejects_excessive_forward_count() {
    use offline_protocol_core::ForwardInfo;

    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    // Build a message that has already been forwarded MAX_FORWARD_COUNT times
    let forward_info = ForwardInfo {
        original_sender: UserId::new("alice").unwrap(),
        original_message_id: MessageId::new(),
        original_timestamp: offline_protocol_core::Timestamp::now(),
        forward_count: crate::constants::MAX_FORWARD_COUNT,
    };
    let original = offline_protocol_core::Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content("Hello")
    .forwarded_from(forward_info)
    .build();

    // from_message will increment to MAX_FORWARD_COUNT + 1, which should be rejected
    let result = protocol.forward_message(&original, "charlie", None);
    let err = result.unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "Forward-count cap is a caller-input defect, got: {:?}",
        err
    );
    assert!(
        err.to_string().contains("exceeds maximum"),
        "Expected forward count cap error, got: {}",
        err
    );
}

#[test]
fn test_forward_message_rejects_overflow_forward_count() {
    use offline_protocol_core::ForwardInfo;

    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    // A peer-supplied forward_count of u32::MAX must not wrap past the cap.
    // Pre-fix: `+ 1` wrapped to 0, slipping past `forward_count > MAX_FORWARD_COUNT`.
    // Post-fix: `saturating_add(1)` clamps to u32::MAX, which the cap then rejects.
    let forward_info = ForwardInfo {
        original_sender: UserId::new("alice").unwrap(),
        original_message_id: MessageId::new(),
        original_timestamp: offline_protocol_core::Timestamp::now(),
        forward_count: u32::MAX,
    };
    let original = offline_protocol_core::Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content("Hello")
    .forwarded_from(forward_info)
    .build();

    let result = protocol.forward_message(&original, "charlie", None);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("exceeds maximum"),
        "Expected forward count cap error, got: {}",
        err_msg
    );
}

#[test]
fn test_forward_message_to_group_rejects_overflow_forward_count() {
    use offline_protocol_core::ForwardInfo;

    let config = create_test_config();
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    protocol.start().unwrap();

    // The group-forward path has its own cap check (group_mesh.rs:1620) that runs
    // before any MLS work; an overflow seed must trip it before group_id is used.
    let forward_info = ForwardInfo {
        original_sender: UserId::new("alice").unwrap(),
        original_message_id: MessageId::new(),
        original_timestamp: offline_protocol_core::Timestamp::now(),
        forward_count: u32::MAX,
    };
    let original = offline_protocol_core::Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content("Hello")
    .forwarded_from(forward_info)
    .build();

    let result = protocol.forward_message_to_group(&original, "any-group", None);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("exceeds maximum"),
        "Expected forward count cap error, got: {}",
        err_msg
    );
}

#[test]
fn test_send_message_returns_ok_and_emits_deferred_when_transport_fails() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000; // long delay so nothing retries during test
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Use a FlakyTransport that always fails
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    flaky.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let observed_clone = observed_events.clone();
    protocol.on_event(move |event| {
        observed_clone.lock().unwrap().push(event);
    });

    protocol.start().unwrap();

    // send_message should return Ok even though the transport fails
    let result = protocol.send_message("bob", "Hello!", None::<MessagePriority>, None::<String>);
    assert!(result.is_ok(), "Expected Ok, got: {:?}", result);

    // Should have emitted a MessageDeferred event
    let events = observed_events.lock().unwrap();
    let deferred = events
        .iter()
        .find(|e| matches!(e, Event::MessageDeferred { .. }));
    assert!(
        deferred.is_some(),
        "Expected MessageDeferred event, got: {:?}",
        *events
    );

    // Message should be in the outbox for retry
    assert!(
        protocol.retry_queue_size() > 0,
        "Expected message in retry queue"
    );
}

#[test]
fn test_flush_outbox_for_peer_on_discovery() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000; // long delay
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    // Don't start it — transport is unavailable so sends go to outbox
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Send a message — it will be deferred
    let _msg_id = protocol
        .send_message("bob", "queued msg", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert!(protocol.retry_queue_size() > 0);

    // Now replace with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let observed_clone = observed_events.clone();
    protocol.on_event(move |event| {
        observed_clone.lock().unwrap().push(event);
    });

    // Discover the peer — should flush the pending message
    protocol.on_neighbor_discovered("bob");

    // The message should have been flushed from the retry queue
    assert_eq!(
        protocol.retry_queue_size(),
        0,
        "Retry queue should be empty after flush"
    );
}

#[test]
fn test_flush_batch_limit_caps_sends() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails to fill the outbox
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Queue more messages than the batch limit
    let total = crate::constants::FLUSH_BATCH_LIMIT + 5;
    for i in 0..total {
        let _ = protocol.send_message(
            "bob",
            &format!("msg-{}", i),
            None::<MessagePriority>,
            None::<String>,
        );
    }
    assert_eq!(protocol.retry_queue_size(), total);

    // Replace with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    // Flush for peer — should send at most FLUSH_BATCH_LIMIT
    protocol.on_neighbor_discovered("bob");

    // Remaining messages should still be in the outbox (not in retry queue
    // since they weren't attempted and thus stay in outbox only)
    assert!(
        protocol.retry_queue_size() <= 5,
        "Expected at most 5 remaining in retry queue, got: {}",
        protocol.retry_queue_size()
    );
}

#[test]
fn test_flush_outbox_all_on_internet_reconnect() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Queue several messages to different peers
    for peer in &["alice", "bob", "carol"] {
        let _ = protocol.send_message(*peer, "hello", None::<MessagePriority>, None::<String>);
    }
    assert_eq!(protocol.retry_queue_size(), 3);

    // Replace with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let observed_clone = observed_events.clone();
    protocol.on_event(move |event| {
        observed_clone.lock().unwrap().push(event);
    });

    // Simulate internet reconnect — flush all
    protocol.flush_outbox_all();

    // All messages should have been flushed from the retry queue
    assert_eq!(
        protocol.retry_queue_size(),
        0,
        "Retry queue should be empty after flush_outbox_all"
    );
}

#[test]
fn test_flush_outbox_all_collects_stranded_outbox_entries() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Queue a message — goes to outbox AND retry queue
    let msg_id = protocol
        .send_message("bob", "stranded", None::<MessagePriority>, None::<String>)
        .unwrap();
    assert_eq!(protocol.retry_queue_size(), 1);
    assert!(protocol.outbox_entry_count() > 0);

    // Manually remove from retry queue to simulate a "stranded" outbox entry
    // (e.g., from a prior max_retries rejection in the old code path)
    protocol.retry_queue_mut().remove(&msg_id.as_str());
    assert_eq!(protocol.retry_queue_size(), 0);
    // Outbox entry still exists
    assert!(protocol.outbox_entry_count() > 0);

    // Replace with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    // flush_outbox_all should pick up the stranded outbox entry
    protocol.flush_outbox_all();

    // The stranded message should have been sent
    let sent = mock_clone.sent_messages();
    assert!(
        !sent.is_empty(),
        "Expected stranded outbox message to be sent"
    );
}

// ============================================================================
// OUTBOX PERSISTENCE
// ============================================================================

/// Builds a config with long retry delays so a failed send parks the message
/// in the outbox instead of racing a backoff timer during the test.
fn outbox_persistence_config() -> ProtocolConfig {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    config
}

fn test_message(recipient: &str, content: &str) -> Message {
    Message::new(
        UserId::new("user123").unwrap(),
        UserId::new(recipient).unwrap(),
        AppId::new("test-app").unwrap(),
        content,
    )
}

fn store_outbox_entry(storage: &InMemoryStorage, entry: &OutboxEntry) {
    seed_sealed_state_record(
        storage,
        storage_keys::OUTBOX,
        &entry.message.id.as_str(),
        &serde_json::to_vec(entry).unwrap(),
    );
}

#[test]
fn test_outbox_persisted_and_restored_after_restart() {
    let storage = Arc::new(InMemoryStorage::new());

    let msg_id;
    {
        let mut protocol = OfflineProtocol::new(outbox_persistence_config()).unwrap();
        protocol
            .enable_message_persistence_for_test(storage.clone())
            .unwrap();

        // Failing transport so the send is deferred into the outbox.
        let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(flaky));
        protocol.start().unwrap();

        msg_id = protocol
            .send_message("bob", "durable", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert!(protocol.outbox_entry_count() > 0);
        assert_eq!(
            storage.list_keys(storage_keys::OUTBOX).unwrap().len(),
            1,
            "Deferred message should be persisted to the outbox"
        );
    }

    // New protocol instance, same storage: the outbox should be restored.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "Outbox entry should be restored after restart"
    );
    let restored = protocol.outbox_messages().next().unwrap();
    assert_eq!(restored.id, msg_id);
    assert_eq!(restored.content, "durable");
}

#[test]
fn test_remove_outbox_entry_clears_storage() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(outbox_persistence_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    let msg_id = protocol
        .send_message(
            "bob",
            "to-be-acked",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert_eq!(storage.list_keys(storage_keys::OUTBOX).unwrap().len(), 1);

    // Both delivery-ACK and max-retries removal funnel through
    // remove_outbox_entry, which must also delete the persisted copy.
    let removed = protocol.remove_outbox_entry(&msg_id);
    assert!(removed.is_some());
    assert!(
        storage.list_keys(storage_keys::OUTBOX).unwrap().is_empty(),
        "Persisted outbox entry should be deleted on removal"
    );
}

#[test]
fn test_outbox_eviction_deletes_from_storage() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(outbox_persistence_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    // One more than capacity: the oldest entry is evicted, and its persisted
    // copy must be deleted so storage never exceeds the in-memory cap.
    let total = crate::constants::MAX_OUTBOX_ENTRIES + 1;
    for i in 0..total {
        let _ = protocol.send_message(
            "bob",
            &format!("msg-{i}"),
            None::<MessagePriority>,
            None::<String>,
        );
    }

    assert_eq!(
        storage.list_keys(storage_keys::OUTBOX).unwrap().len(),
        crate::constants::MAX_OUTBOX_ENTRIES,
        "Persisted outbox must be pruned to capacity on eviction"
    );
}

#[test]
fn test_restore_outbox_skips_corrupted_entries() {
    let storage = Arc::new(InMemoryStorage::new());

    let valid = test_message("bob", "valid");
    let valid_id = valid.id.clone();
    store_outbox_entry(
        &storage,
        &OutboxEntry {
            message: valid,
            attempt_count: 2,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: None,
            reseal: None,
        },
    );
    storage
        .store(storage_keys::OUTBOX, "corrupt-id", b"not json")
        .unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "Only the valid entry should be restored"
    );
    assert_eq!(
        protocol.outbox_messages().next().unwrap().id,
        valid_id,
        "The restored entry should be the valid one"
    );
    let keys = storage.list_keys(storage_keys::OUTBOX).unwrap();
    assert_eq!(
        keys.len(),
        1,
        "Corrupted key should be deleted from storage"
    );
    assert!(!keys.iter().any(|k| k == "corrupt-id"));
}

#[test]
fn test_restore_outbox_prunes_overflow() {
    let storage = Arc::new(InMemoryStorage::new());
    let base = chrono::Utc::now();

    // Persist more than capacity, each with a strictly increasing last_sent_at
    // so "newest kept" is unambiguous.
    let total = crate::constants::MAX_OUTBOX_ENTRIES + 10;
    let mut oldest_id = None;
    for i in 0..total {
        let entry = OutboxEntry {
            message: test_message("bob", &format!("m{i}")),
            attempt_count: 0,
            first_sent_at: base,
            last_sent_at: base + ChronoDuration::seconds(i as i64),
            last_transport: None,
            reseal: None,
        };
        if i == 0 {
            oldest_id = Some(entry.message.id.as_str());
        }
        store_outbox_entry(&storage, &entry);
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    assert_eq!(
        protocol.outbox_entry_count(),
        crate::constants::MAX_OUTBOX_ENTRIES,
        "Restore should prune the in-memory outbox to capacity"
    );
    let keys = storage.list_keys(storage_keys::OUTBOX).unwrap();
    assert_eq!(
        keys.len(),
        crate::constants::MAX_OUTBOX_ENTRIES,
        "Pruned overflow should be deleted from storage, not left to re-restore"
    );
    // The oldest (smallest last_sent_at) is what gets dropped.
    let oldest_id = oldest_id.unwrap();
    assert!(!keys.iter().any(|k| *k == oldest_id));
}

#[test]
fn test_restore_outbox_refreshes_expired_ttl_carrier_relative() {
    let storage = Arc::new(InMemoryStorage::new());

    // Persist an entry whose last_sent_at is older than the default 1h outbox
    // lifetime — as if the app was killed and reopened hours later.
    let old = chrono::Utc::now() - ChronoDuration::hours(2);
    store_outbox_entry(
        &storage,
        &OutboxEntry {
            message: test_message("bob", "aged"),
            attempt_count: 1,
            first_sent_at: old,
            last_sent_at: old,
            last_transport: None,
            reseal: None,
        },
    );

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "An entry past its wall-clock TTL must still be restored"
    );

    // A cleanup tick must NOT reap it: the TTL clock is carrier-relative and
    // was refreshed on restore, so the message keeps its delivery window.
    protocol.cleanup_outbox();
    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "Refreshed entry should survive cleanup (carrier-relative TTL)"
    );
}

#[test]
fn test_media_outbox_entry_not_persisted() {
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    let mut msg = test_message("bob", "chunk");
    msg.content_type = ContentType::FileChunk;
    protocol.ensure_outbox_entry(&msg);

    // The chunk lands in the in-memory media outbox...
    assert!(protocol.outbox_entry_count() > 0);
    // ...but is never persisted: file transfers are not durable, so a
    // resurrected chunk could never complete its transfer.
    assert!(
        storage.list_keys(storage_keys::OUTBOX).unwrap().is_empty(),
        "Media (file-chunk) entries must not be persisted"
    );
}

#[test]
fn test_restored_outbox_entry_flushed_on_start() {
    let storage = Arc::new(InMemoryStorage::new());

    let msg_id;
    {
        let mut protocol = OfflineProtocol::new(outbox_persistence_config()).unwrap();
        protocol
            .enable_message_persistence_for_test(storage.clone())
            .unwrap();
        let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
        protocol
            .transport_manager_mut()
            .add_transport(TransportType::BLE, Box::new(flaky));
        protocol.start().unwrap();
        msg_id = protocol
            .send_message("bob", "resend-me", None::<MessagePriority>, None::<String>)
            .unwrap();
        assert_eq!(storage.list_keys(storage_keys::OUTBOX).unwrap().len(), 1);
    }

    // Restart with a working transport; start() should flush the restored
    // entry and actually deliver it.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(protocol.outbox_entry_count(), 1);

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    protocol.start().unwrap();

    let sent = mock_clone.sent_messages();
    assert!(
        sent.iter().any(|m| m.id == msg_id),
        "Restored outbox entry should be flushed and sent on start()"
    );
}

#[test]
fn test_restore_outbox_preserves_preexisting_in_memory_entry() {
    // An entry queued in memory *before* persistence is enabled is not in
    // storage. restore_outbox must merge, not clear — otherwise this entry is
    // silently dropped, and if it was awaiting an ACK (not in the retry queue)
    // it has no recovery path. Restore must also persist it so it survives the
    // next restart.
    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Populate the in-memory outbox while storage is still None.
    let msg = test_message("bob", "queued-before-persistence");
    let msg_id = msg.id.clone();
    protocol.ensure_outbox_entry(&msg);
    assert_eq!(protocol.outbox_entry_count(), 1);
    assert!(
        storage.list_keys(storage_keys::OUTBOX).unwrap().is_empty(),
        "Nothing is persisted before persistence is enabled"
    );

    // Enabling persistence runs restore_outbox. The pre-existing entry must
    // survive and become durable.
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "Pre-existing in-memory entry must survive restore (merge, not clear)"
    );
    assert_eq!(protocol.outbox_messages().next().unwrap().id, msg_id);
    let keys = storage.list_keys(storage_keys::OUTBOX).unwrap();
    assert_eq!(
        keys.len(),
        1,
        "Pre-existing entry must be persisted on restore"
    );
    assert_eq!(keys[0], msg_id.as_str());
}

#[test]
fn test_restore_outbox_prune_keeps_fresh_over_refreshed_stale() {
    // Prune must run before the carrier-relative TTL refresh. A lapsed entry
    // refreshed *before* the prune would be stamped `now` and sort as the
    // newest, crowding genuinely-fresh entries out of the kept set. Here the
    // lapsed entries are the overflow and must be dropped, not resurrected.
    let storage = Arc::new(InMemoryStorage::new());
    let now = chrono::Utc::now();

    // Exactly capacity's worth of fresh entries (well within the 1h lifetime).
    for i in 0..crate::constants::MAX_OUTBOX_ENTRIES {
        store_outbox_entry(
            &storage,
            &OutboxEntry {
                message: test_message("bob", &format!("fresh-{i}")),
                attempt_count: 0,
                first_sent_at: now,
                last_sent_at: now - ChronoDuration::seconds((i + 1) as i64),
                last_transport: None,
                reseal: None,
            },
        );
    }
    // A handful of lapsed entries (older than the 1h lifetime) — overflow.
    let mut lapsed_ids = Vec::new();
    for i in 0..5 {
        let entry = OutboxEntry {
            message: test_message("bob", &format!("lapsed-{i}")),
            attempt_count: 0,
            first_sent_at: now - ChronoDuration::hours(2),
            last_sent_at: now - ChronoDuration::hours(2),
            last_transport: None,
            reseal: None,
        };
        lapsed_ids.push(entry.message.id.as_str().to_string());
        store_outbox_entry(&storage, &entry);
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    assert_eq!(
        protocol.outbox_entry_count(),
        crate::constants::MAX_OUTBOX_ENTRIES,
        "Restore should prune to capacity"
    );
    // The lapsed (oldest) entries are the pruned overflow — they must NOT have
    // been refreshed-and-kept, and must be gone from both memory and storage.
    let restored_ids: std::collections::HashSet<String> = protocol
        .outbox_messages()
        .map(|m| m.id.as_str().to_string())
        .collect();
    let keys = storage.list_keys(storage_keys::OUTBOX).unwrap();
    for id in &lapsed_ids {
        assert!(
            !restored_ids.contains(id),
            "Lapsed overflow entry {id} must be pruned, not refreshed-and-kept"
        );
        assert!(
            !keys.iter().any(|k| k == id),
            "Pruned lapsed entry {id} must be deleted from storage"
        );
    }
}

#[test]
fn test_ack_receipt_cleans_up_retry_queue() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock.clone()));

    let observed_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let observed_clone = observed_events.clone();
    protocol.on_event(move |event| {
        observed_clone.lock().unwrap().push(event);
    });

    protocol.start().unwrap();

    // Send a message — succeeds, enters ACK tracking
    let msg_id = protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Manually enqueue into retry queue to simulate a retry-in-flight scenario
    let msg = protocol.outbox_messages().next().unwrap().clone();
    protocol.retry_queue_mut().enqueue(msg, 1);
    assert!(protocol.retry_queue_size() > 0);

    // Simulate receiving an ACK from bob for this message
    let ack_message = Message::builder(
        UserId::new("bob").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .requires_ack(false)
    .metadata(ACK_FOR_KEY, msg_id.as_str())
    .metadata("ack_hop_count", "0")
    .metadata("ack_transport", "ble")
    .build();

    // Queue the ACK so it's received on next process
    mock.queue_message(ack_message);
    let _ = protocol.receive_message();

    // The retry queue should be empty after ACK receipt
    assert_eq!(
        protocol.retry_queue_size(),
        0,
        "Retry queue should be cleaned up after ACK"
    );

    // Outbox should also be cleaned up
    assert_eq!(
        protocol.outbox_entry_count(),
        0,
        "Outbox should be cleaned up after ACK"
    );

    // MessageDelivered event should have been emitted
    let events = observed_events.lock().unwrap();
    let delivered = events
        .iter()
        .find(|e| matches!(e, Event::MessageDelivered { .. }));
    assert!(
        delivered.is_some(),
        "Expected MessageDelivered event, got: {:?}",
        *events
    );
}

#[test]
fn test_flush_outbox_all_re_enqueues_overflow_beyond_batch_limit() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails to fill the outbox
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Queue more messages than the batch limit across different peers
    let total = crate::constants::FLUSH_BATCH_LIMIT + 10;
    for i in 0..total {
        let _ = protocol.send_message(
            &format!("peer-{}", i),
            &format!("msg-{}", i),
            None::<MessagePriority>,
            None::<String>,
        );
    }
    assert_eq!(protocol.retry_queue_size(), total);

    // Replace with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    // Flush all — should send FLUSH_BATCH_LIMIT and re-enqueue the rest
    protocol.flush_outbox_all();

    // The overflow messages must still be in the retry queue, not lost
    assert_eq!(
        protocol.retry_queue_size(),
        10,
        "Overflow messages beyond batch limit should be re-enqueued"
    );

    // Outbox entries for overflowed messages should still exist
    assert!(
        protocol.outbox_entry_count() >= 10,
        "Outbox entries for overflow messages should survive"
    );
}

#[test]
fn test_flush_send_failure_re_enqueues_message() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Queue a message — deferred
    let _ = protocol
        .send_message(
            "bob",
            "will fail again",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert_eq!(protocol.retry_queue_size(), 1);

    // Flush for peer — transport still fails, so message should be re-enqueued
    protocol.on_neighbor_discovered("bob");

    assert_eq!(
        protocol.retry_queue_size(),
        1,
        "Message should be re-enqueued after flush send failure"
    );

    // Outbox entry should still exist
    assert!(
        protocol.outbox_entry_count() > 0,
        "Outbox entry should survive flush send failure"
    );
}

#[test]
fn test_flush_outbox_all_skips_messages_awaiting_ack() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    protocol.start().unwrap();

    // Send a message — succeeds, enters ACK tracking
    let _msg_id = protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Message was sent successfully: it should be in outbox (awaiting ACK)
    // but NOT in the retry queue
    assert_eq!(protocol.retry_queue_size(), 0);
    assert!(protocol.outbox_entry_count() > 0);

    let sent_before = mock_clone.sent_messages().len();

    // flush_outbox_all should NOT re-send this message because it already
    // has a pending ACK
    protocol.flush_outbox_all();

    let sent_after = mock_clone.sent_messages().len();
    assert_eq!(
        sent_before, sent_after,
        "flush_outbox_all should not re-send messages awaiting ACK"
    );
}

#[test]
fn test_cleanup_outbox_removes_retry_queue_entry() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    // Very short outbox lifetime so entries expire immediately
    config.reliability.retry.outbox_max_lifetime_ms = 1;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a transport that always fails
    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    // Send a message — deferred, enters outbox + retry queue
    let _ = protocol
        .send_message(
            "bob",
            "will expire",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert_eq!(protocol.retry_queue_size(), 1);
    assert!(protocol.outbox_entry_count() > 0);

    // Wait a tiny bit for the outbox lifetime to expire
    std::thread::sleep(Duration::from_millis(5));

    // Run cleanup — should remove both outbox entry AND retry queue entry
    protocol.cleanup_expired_entries();

    assert_eq!(
        protocol.outbox_entry_count(),
        0,
        "Outbox entry should be cleaned up after expiry"
    );
    assert_eq!(
        protocol.retry_queue_size(),
        0,
        "Retry queue entry should be cleaned up when outbox entry expires"
    );
}

/// Outbox expiry is terminal and must not be silent: the app is otherwise
/// left showing the message as pending forever.
#[test]
fn test_cleanup_outbox_expiry_emits_message_failed() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    config.reliability.retry.outbox_max_lifetime_ms = 1;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let msg_id = protocol
        .send_message(
            "bob",
            "will expire",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    assert!(protocol.outbox_entry_count() > 0);

    std::thread::sleep(Duration::from_millis(5));
    protocol.cleanup_expired_entries();

    assert_eq!(protocol.outbox_entry_count(), 0);
    let captured = events.lock().unwrap();
    let failed = captured
        .iter()
        .find_map(|e| match e {
            Event::MessageFailed {
                message_id, reason, ..
            } => Some((message_id.clone(), reason.clone())),
            _ => None,
        })
        .expect("outbox expiry must emit a terminal message_failed");
    assert_eq!(failed.0, msg_id.as_str());
    assert_eq!(failed.1, "Outbox lifetime exceeded");
    assert!(
        !captured
            .iter()
            .any(|e| matches!(e, Event::ConnectionRequestUndeliverable { .. })),
        "an ordinary message must not emit the connection-request event"
    );
}

/// A connection request expiring out of the outbox settles its pending
/// entry and emits the typed undeliverable event, mirroring the
/// max-retries exhaustion path.
#[test]
fn test_cleanup_outbox_expiry_settles_pending_connection_request() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    config.reliability.retry.outbox_max_lifetime_ms = 1;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));

    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();

    std::thread::sleep(Duration::from_millis(5));
    protocol.cleanup_expired_entries();

    let captured = events.lock().unwrap();
    let undeliverable = captured
        .iter()
        .find_map(|e| match e {
            Event::ConnectionRequestUndeliverable {
                recipient,
                message_id,
                reason,
            } => Some((recipient.clone(), message_id.clone(), reason.clone())),
            _ => None,
        })
        .expect("connection request expiry must emit undeliverable");
    assert_eq!(undeliverable.0, "bob");
    assert_eq!(undeliverable.1, sent_id.as_str());
    assert_eq!(undeliverable.2, "outbox_lifetime_exceeded");
    drop(captured);

    assert!(
        protocol.pending_connection_requests.is_empty(),
        "outbox expiry must settle the pending connection request entry"
    );
}

/// Capacity eviction is as terminal as expiry: the entry and its persisted
/// copy are gone, so it must emit `message_failed` rather than leave the
/// app showing the message as pending forever.
#[test]
fn test_outbox_capacity_eviction_emits_message_failed() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let first_id = protocol
        .send_message(
            "bob",
            "oldest, will be evicted",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    // Backdate so the eviction victim is deterministic regardless of
    // timestamp resolution across the flood below.
    protocol
        .outbox
        .get_mut(&first_id)
        .expect("first message must be in the outbox")
        .last_sent_at -= ChronoDuration::seconds(60);

    for i in 0..crate::constants::MAX_OUTBOX_ENTRIES {
        let _ = protocol.send_message(
            "bob",
            &format!("filler-{i}"),
            None::<MessagePriority>,
            None::<String>,
        );
    }

    assert_eq!(
        protocol.outbox_entry_count(),
        crate::constants::MAX_OUTBOX_ENTRIES
    );
    let captured = events.lock().unwrap();
    let evicted: Vec<_> = captured
        .iter()
        .filter_map(|e| match e {
            Event::MessageFailed {
                message_id, reason, ..
            } if reason == "Outbox capacity exceeded" => Some(message_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        evicted,
        vec![first_id.as_str()],
        "capacity eviction must emit exactly one terminal message_failed, for the oldest entry"
    );
}

/// A connection request evicted at outbox capacity settles its pending
/// entry and emits the typed undeliverable event, like the expiry and
/// max-retries paths.
#[test]
fn test_outbox_capacity_eviction_settles_pending_connection_request() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let flaky = FlakyTransport::fail_first(TransportType::BLE, u32::MAX);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(flaky));
    protocol.start().unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let sent_id = protocol
        .send_connection_request("bob", "Alice", None, None)
        .unwrap();
    protocol
        .outbox
        .get_mut(&sent_id)
        .expect("connection request must be in the outbox")
        .last_sent_at -= ChronoDuration::seconds(60);

    for i in 0..crate::constants::MAX_OUTBOX_ENTRIES {
        let _ = protocol.send_message(
            "bob",
            &format!("filler-{i}"),
            None::<MessagePriority>,
            None::<String>,
        );
    }

    let captured = events.lock().unwrap();
    let undeliverable = captured
        .iter()
        .find_map(|e| match e {
            Event::ConnectionRequestUndeliverable {
                recipient,
                message_id,
                reason,
            } => Some((recipient.clone(), message_id.clone(), reason.clone())),
            _ => None,
        })
        .expect("connection request eviction must emit undeliverable");
    assert_eq!(undeliverable.0, "bob");
    assert_eq!(undeliverable.1, sent_id.as_str());
    assert_eq!(undeliverable.2, "outbox_capacity_exceeded");
    drop(captured);

    assert!(
        protocol.pending_connection_requests.is_empty(),
        "capacity eviction must settle the pending connection request entry"
    );
}

/// Media chunks settle through the transfer abort (`media_resend_required`
/// path), never a per-chunk `message_failed` — lock down the asymmetry.
#[test]
fn test_cleanup_outbox_media_expiry_does_not_emit_message_failed() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    config.reliability.retry.outbox_max_lifetime_ms = 1;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    let media_msg = Message::builder(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .content_type(ContentType::FileChunk)
    .requires_ack(true)
    .build();
    protocol.media_outbox.insert(
        media_msg.id.clone(),
        OutboxEntry {
            message: media_msg.clone(),
            attempt_count: 0,
            first_sent_at: chrono::Utc::now() - ChronoDuration::seconds(1),
            last_sent_at: chrono::Utc::now() - ChronoDuration::seconds(1),
            last_transport: None,
            reseal: None,
        },
    );

    protocol.cleanup_expired_entries();

    assert!(protocol.media_outbox.is_empty(), "media entry must expire");
    let captured = events.lock().unwrap();
    assert!(
        !captured
            .iter()
            .any(|e| matches!(e, Event::MessageFailed { .. })),
        "media chunk expiry must not emit a per-chunk message_failed"
    );
}

/// The carrier-relative TTL refresh on restore must not re-grant a fresh
/// window forever: past the absolute cap (factor × lifetime from
/// `first_sent_at`) the entry is dropped terminally instead.
#[test]
fn test_restore_outbox_drops_absolutely_expired() {
    let storage = Arc::new(InMemoryStorage::new());
    let now = chrono::Utc::now();
    // lifetime 1s → absolute cap 4s.
    let lifetime_ms = 1000;

    let dead = test_message("bob", "past the absolute cap");
    let dead_id = dead.id.clone();
    store_outbox_entry(
        &storage,
        &OutboxEntry {
            message: dead,
            attempt_count: 3,
            first_sent_at: now - ChronoDuration::seconds(10),
            last_sent_at: now - ChronoDuration::seconds(5),
            last_transport: None,
            reseal: None,
        },
    );

    let lapsed = test_message("bob", "lapsed but within the cap");
    let lapsed_id = lapsed.id.clone();
    store_outbox_entry(
        &storage,
        &OutboxEntry {
            message: lapsed,
            attempt_count: 1,
            first_sent_at: now - ChronoDuration::seconds(3),
            last_sent_at: now - ChronoDuration::seconds(2),
            last_transport: None,
            reseal: None,
        },
    );

    let mut config = create_test_config();
    config.reliability.retry.outbox_max_lifetime_ms = lifetime_ms;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    assert_eq!(
        protocol.outbox_entry_count(),
        1,
        "only the within-cap entry should be restored"
    );
    let restored = protocol
        .outbox
        .get(&lapsed_id)
        .expect("the lapsed-but-within-cap entry must be restored");
    assert!(
        restored.last_sent_at + ChronoDuration::milliseconds(lifetime_ms as i64) > now,
        "the restored entry's carrier-relative TTL must be refreshed"
    );

    let keys = storage.list_keys(storage_keys::OUTBOX).unwrap();
    assert_eq!(keys.len(), 1, "the dead entry must be deleted from storage");
    assert!(!keys.iter().any(|k| *k == dead_id.as_str()));

    let captured = events.lock().unwrap();
    let failed: Vec<_> = captured
        .iter()
        .filter_map(|e| match e {
            Event::MessageFailed {
                message_id, reason, ..
            } => Some((message_id.clone(), reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        failed,
        vec![(dead_id.as_str(), "Outbox lifetime exceeded".to_string())],
        "the absolutely-expired entry must settle terminally, and only that one"
    );
}

#[test]
fn test_flush_outbox_for_peer_includes_media_outbox() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    protocol.start().unwrap();

    // Manually insert a FileChunk message into media_outbox to simulate
    // a media chunk that was deferred while the peer was unreachable
    let media_msg = Message::builder(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .content_type(ContentType::FileChunk)
    .requires_ack(true)
    .build();

    let media_msg_id = media_msg.id.clone();

    // Insert into media_outbox
    protocol.media_outbox.insert(
        media_msg.id.clone(),
        OutboxEntry {
            message: media_msg.clone(),
            attempt_count: 0,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: None,
            reseal: None,
        },
    );

    // Also enqueue in retry queue
    protocol.retry_queue_mut().enqueue(media_msg, 0);
    assert_eq!(protocol.retry_queue_size(), 1);

    // Discover the peer — should flush the media outbox message too
    protocol.on_neighbor_discovered("bob");

    // The media message should have been sent
    let sent = mock_clone.sent_messages();
    let found = sent.iter().any(|m| m.id == media_msg_id);
    assert!(
        found,
        "Media outbox message should be sent on peer discovery"
    );

    // Retry queue should be empty
    assert_eq!(
        protocol.retry_queue_size(),
        0,
        "Retry queue should be empty after flushing media message"
    );
}

#[test]
fn test_flush_outbox_all_includes_media_outbox() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    // Start with a working transport
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    protocol.start().unwrap();

    // Insert a stranded FileChunk in media_outbox (not in retry queue)
    let media_msg = Message::builder(
        UserId::new("user123").unwrap(),
        UserId::new("alice").unwrap(),
        AppId::new("test-app").unwrap(),
    )
    .content(String::new())
    .content_type(ContentType::FileChunk)
    .requires_ack(true)
    .build();

    let media_msg_id = media_msg.id.clone();

    protocol.media_outbox.insert(
        media_msg.id.clone(),
        OutboxEntry {
            message: media_msg,
            attempt_count: 1,
            first_sent_at: chrono::Utc::now(),
            last_sent_at: chrono::Utc::now(),
            last_transport: None,
            reseal: None,
        },
    );

    // No retry queue entry — simulates a stranded media outbox entry

    // flush_outbox_all should pick up the stranded media entry
    protocol.flush_outbox_all();

    let sent = mock_clone.sent_messages();
    let found = sent.iter().any(|m| m.id == media_msg_id);
    assert!(
        found,
        "Stranded media outbox message should be sent by flush_outbox_all"
    );
}

#[test]
fn test_flush_outbox_for_peer_skips_messages_awaiting_ack() {
    let mut config = create_test_config();
    config.reliability.retry.initial_delay_ms = 60_000;
    config.reliability.retry.max_delay_ms = 60_000;
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    protocol.start().unwrap();

    // Send a message — succeeds, enters ACK tracking
    let _msg_id = protocol
        .send_message("bob", "Hello!", None::<MessagePriority>, None::<String>)
        .unwrap();

    // Message was sent successfully: it should be in outbox (awaiting ACK)
    // but NOT in the retry queue
    assert_eq!(protocol.retry_queue_size(), 0);
    assert!(protocol.outbox_entry_count() > 0);

    let sent_before = mock_clone.sent_messages().len();

    // Discovering the peer again should NOT re-send the message because it
    // already has a pending ACK
    protocol.on_neighbor_discovered("bob");

    let sent_after = mock_clone.sent_messages().len();
    assert_eq!(
        sent_before, sent_after,
        "flush_outbox_for_peer should not re-send messages awaiting ACK"
    );
}

// --- Telemetry categories (metrics snapshot, transport state, device, routing) ---

#[test]
fn test_process_emits_metrics_snapshot_on_first_tick() {
    // With cadence defaulted to 5s and `last_metrics_emit_at` starting at
    // `None`, the first process() tick is unconditionally due and must
    // emit one MetricsSnapshot.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.start().unwrap();

    protocol.process().unwrap();

    let records = sink.take();
    let snapshots: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, TelemetryRecord::MetricsSnapshot(_)))
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "first process() tick must emit one MetricsSnapshot, got {records:?}",
    );
}

#[test]
fn test_process_skips_metrics_snapshot_when_cadence_is_none() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(
            Arc::new(sink.clone()),
            TelemetryConfig::default().with_metrics_cadence(None),
        )
        .unwrap();
    protocol.start().unwrap();

    protocol.process().unwrap();

    let records = sink.take();
    assert!(
        records
            .iter()
            .all(|r| !matches!(r, TelemetryRecord::MetricsSnapshot(_))),
        "cadence=None must suppress MetricsSnapshot emission, got {records:?}",
    );
}

#[test]
fn test_install_seeds_transport_state_so_first_tick_emits_no_bootstrap() {
    // Regression guard: a sink installed after start() must NOT observe a
    // synthetic `Unavailable → Available` transition on the first tick for
    // transports that were already running at install time. install_telemetry_sink
    // seeds `transport_status_snapshot` from the live statuses so the
    // first tick diff finds nothing changed.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    protocol.process().unwrap();

    let records = sink.take();
    let transitions: Vec<&TransportStateEvent> = records
        .iter()
        .filter_map(|r| match r {
            TelemetryRecord::TransportState(e) => Some(e),
            _ => None,
        })
        .collect();
    assert!(
        transitions.is_empty(),
        "install-after-start must not synthesise a bootstrap transition, got {transitions:?}",
    );
}

#[test]
fn test_process_emits_transport_state_on_status_transition() {
    // After install seeds the snapshot, only genuine transitions fire.
    // Flip the mock to Disconnected and verify exactly one
    // Available→Disconnected transition.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    let mock_clone = mock.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    // First tick — should emit nothing transport-state-wise (seeded).
    protocol.process().unwrap();
    sink.take();

    mock_clone.set_status(TransportStatus::Disconnected);
    protocol.process().unwrap();

    let records = sink.take();
    let transitions: Vec<&TransportStateEvent> = records
        .iter()
        .filter_map(|r| match r {
            TelemetryRecord::TransportState(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(
        transitions.len(),
        1,
        "exactly one TransportState should fire for the Available→Disconnected transition, got {records:?}",
    );
    assert_eq!(transitions[0].transport, TransportType::BLE);
    assert_eq!(transitions[0].previous, TransportStatus::Available);
    assert_eq!(transitions[0].current, TransportStatus::Disconnected);
}

#[test]
fn test_install_seeds_device_capability_so_first_tick_is_silent() {
    // install_telemetry_sink seeds `device_capability_snapshot` so the first
    // tick's diff against the seeded state yields no change record. Apps
    // that want the current device state at install time pull it
    // explicitly rather than relying on a bootstrap event.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol.start().unwrap();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    protocol.process().unwrap();

    let records = sink.take();
    let device: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, TelemetryRecord::Device(_)))
        .collect();
    assert!(
        device.is_empty(),
        "install seeds device snapshot; first tick must not re-emit it, got {records:?}",
    );
}

#[test]
fn test_legacy_dors_events_still_fire_with_routing_callback_wired() {
    // Regression guard: step 3 (wiring `set_routing_decision_callback`)
    // must not displace the existing `dors_event_callback` → EventCallback
    // path. Apps that consume `DorsScoreUpdated`/`DorsTransportSelected`
    // via `on_event` continue to see them whether or not a telemetry sink
    // is installed.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let captured: Arc<Mutex<Vec<crate::events::Event>>> = Arc::new(Mutex::new(Vec::new()));
    let handler = captured.clone();
    protocol.on_event(move |event| handler.lock().unwrap().push(event));

    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    // Install a telemetry sink AFTER start() so the routing callback is
    // already wired — exercising the path where both callbacks coexist.
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    // Trigger a DORS scoring pass by sending a message.
    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "ping",
    );
    protocol.transport_manager_mut().send(&msg).unwrap();

    let legacy_events = captured.lock().unwrap().clone();
    assert!(
        legacy_events
            .iter()
            .any(|e| matches!(e, crate::events::Event::DorsScoreUpdated { .. })),
        "Event::DorsScoreUpdated must still reach legacy callback, got {legacy_events:?}",
    );
    assert!(
        legacy_events
            .iter()
            .any(|e| matches!(e, crate::events::Event::DorsTransportSelected { .. })),
        "Event::DorsTransportSelected must still reach legacy callback, got {legacy_events:?}",
    );

    // And the sink now receives the new Routing records alongside.
    let sink_records = sink.take();
    assert!(
        sink_records
            .iter()
            .any(|r| matches!(r, TelemetryRecord::Routing(_))),
        "sink must observe at least one Routing record, got {sink_records:?}",
    );
}

#[test]
fn test_process_skips_telemetry_tick_without_sink() {
    // With no sink installed, `tick_telemetry_categories` must early-return
    // before touching any of the per-tick aggregator state. We assert that
    // by registering a transport (so a real aggregator pass would observe
    // an `Unavailable → Available` transition and seed the snapshots) and
    // then verifying the three state fields the aggregator would mutate
    // remain at their pre-tick defaults across multiple ticks.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    let captured: Arc<Mutex<Vec<crate::events::Event>>> = Arc::new(Mutex::new(Vec::new()));
    let handler = captured.clone();
    protocol.on_event(move |event| handler.lock().unwrap().push(event));

    protocol.start().unwrap();

    assert!(
        protocol.telemetry.is_none(),
        "precondition: no sink installed",
    );

    for _ in 0..5 {
        protocol.process().unwrap();
    }

    // The aggregator never ran, so its state is untouched. If the early
    // return regressed, the BLE transport would have been observed and
    // these fields would have been populated.
    assert!(
        protocol.last_metrics_emit_at.is_none(),
        "no sink installed; metrics cadence tracker must stay None across ticks",
    );
    assert!(
        protocol.transport_status_snapshot.is_empty(),
        "no sink installed; transport status snapshot must stay empty across ticks, got {:?}",
        protocol.transport_status_snapshot,
    );
    assert!(
        protocol.device_capability_snapshot.is_none(),
        "no sink installed; device capability snapshot must stay None across ticks",
    );
}

#[test]
fn test_metrics_cadence_rate_limits_emission_across_ticks() {
    // Install with a very long cadence so only the first tick after install
    // is "due". Subsequent ticks within the cadence window must not emit.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(
            Arc::new(sink.clone()),
            TelemetryConfig::default().with_metrics_cadence(Some(Duration::from_secs(3600))),
        )
        .unwrap();
    protocol.start().unwrap();

    for _ in 0..3 {
        protocol.process().unwrap();
    }

    let records = sink.take();
    let snapshots: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, TelemetryRecord::MetricsSnapshot(_)))
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "cadence must rate-limit: three ticks within the window produce exactly one snapshot, got {records:?}",
    );
}

#[test]
fn test_reinstall_rearms_diff_snapshots_and_reemits_metrics_frame() {
    // Install a sink, tick once to burn the initial metrics snapshot, then
    // install a second sink. The second install must rearm last_metrics_emit_at
    // so the next tick emits a bootstrap metrics frame to the new sink,
    // and must re-seed transport/device snapshots so no fake transition is
    // reported against state the first sink already acknowledged.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let sink1 = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink1.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.process().unwrap();
    let _ = sink1.take();

    let sink2 = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink2.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.process().unwrap();

    let records = sink2.take();
    let snapshots: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, TelemetryRecord::MetricsSnapshot(_)))
        .collect();
    let transitions: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, TelemetryRecord::TransportState(_)))
        .collect();
    let devices: Vec<_> = records
        .iter()
        .filter(|r| matches!(r, TelemetryRecord::Device(_)))
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "re-installed sink receives a fresh metrics frame, got {records:?}",
    );
    assert!(
        transitions.is_empty(),
        "re-install must not synthesise transport transitions against seeded snapshot, got {records:?}",
    );
    assert!(
        devices.is_empty(),
        "re-install must not re-emit device snapshot against seeded snapshot, got {records:?}",
    );
}

#[test]
fn test_routing_diagnostic_populates_scores_when_enabled() {
    // With routing_diagnostic=true, every RoutingDecision emitted from a
    // send() must carry a populated scores vector (one entry per available
    // transport). Default (false) leaves scores empty — exercised by
    // test_legacy_dors_events_still_fire_with_routing_callback_wired.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(
            Arc::new(sink.clone()),
            TelemetryConfig::default().with_routing_diagnostic(true),
        )
        .unwrap();

    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "ping",
    );
    protocol.transport_manager_mut().send(&msg).unwrap();

    let records = sink.take();
    let routings: Vec<&RoutingDecision> = records
        .iter()
        .filter_map(|r| match r {
            TelemetryRecord::Routing(d) => Some(d.as_ref()),
            _ => None,
        })
        .collect();
    assert!(
        !routings.is_empty(),
        "routing records must fire on send, got {records:?}",
    );
    for decision in &routings {
        assert!(
            !decision.scores.is_empty(),
            "routing_diagnostic=true must populate scores, got {decision:?}",
        );
        // BLE is registered, so it must appear in the breakdown.
        assert!(
            decision
                .scores
                .iter()
                .any(|(t, _)| *t == TransportType::BLE),
            "scores must include every available transport, got {:?}",
            decision.scores,
        );
    }
}

#[test]
fn test_routing_diagnostic_default_leaves_scores_empty() {
    // Default TelemetryConfig has routing_diagnostic=false; the scores
    // vector must stay empty even when the callback fires on every decision.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();

    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "ping",
    );
    protocol.transport_manager_mut().send(&msg).unwrap();

    let records = sink.take();
    for r in &records {
        if let TelemetryRecord::Routing(d) = r {
            assert!(
                d.scores.is_empty(),
                "routing_diagnostic=false must leave scores empty, got {:?}",
                d.scores,
            );
        }
    }
}

#[test]
fn test_metrics_frame_is_local_relay_matches_role() {
    // Regression guard for the renamed field: is_local_relay must reflect
    // RelayManager::current_role() and nothing else (no stale "peer count"
    // semantics). Default role is Regular; the flag must be false.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.start().unwrap();
    protocol.process().unwrap();

    let records = sink.take();
    let frame = records
        .iter()
        .find_map(|r| match r {
            TelemetryRecord::MetricsSnapshot(f) => Some(f.as_ref()),
            _ => None,
        })
        .expect("first tick emits a metrics frame");
    assert!(
        !frame.is_local_relay,
        "default RelayRole::Regular ⇒ is_local_relay=false, got {frame:?}",
    );
}

/// Drives a real `process()` tick with enough connections and healthy
/// battery and asserts the engine emits `RelayPromoted` on the role
/// transition, then `RelayDemotedBattery` once the battery falls below the
/// relay minimum. Guards the OQ #12 wiring: the three relay-role events must
/// fire on actual transitions, not just exist as unreachable variants.
#[test]
fn test_relay_role_transitions_emit_events() {
    fn battery_metrics(level: u8) -> TransportMetrics {
        TransportMetrics {
            battery_level: Some(level),
            is_charging: false,
            ..TransportMetrics::default()
        }
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    mock.set_status(TransportStatus::Available);
    mock.set_metrics(battery_metrics(80));
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock.clone()));

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.start().unwrap();

    // Default relay_threshold is 3; give it enough neighbors to promote.
    for i in 0..3 {
        protocol.on_neighbor_discovered(&format!("peer-{i}"));
    }

    protocol.process().unwrap();
    let promoted = sink.take();
    assert!(
        promoted.iter().any(|r| matches!(
            r,
            TelemetryRecord::Protocol(ev)
                if matches!(ev.as_ref(), Event::RelayPromoted { .. })
        )),
        "healthy battery + enough connections must emit RelayPromoted, got {promoted:?}",
    );

    // A second stable tick must NOT re-emit (transition fired once).
    protocol.process().unwrap();
    let stable = sink.take();
    assert!(
        !stable.iter().any(|r| matches!(
            r,
            TelemetryRecord::Protocol(ev) if matches!(ev.as_ref(), Event::RelayPromoted { .. })
        )),
        "a stable relay role must not re-emit RelayPromoted, got {stable:?}",
    );

    // Drop the battery below the relay minimum (default 30, not charging):
    // the next tick must demote with the battery-specific event.
    mock.set_metrics(battery_metrics(20));
    protocol.process().unwrap();
    let demoted = sink.take();
    assert!(
        demoted.iter().any(|r| matches!(
            r,
            TelemetryRecord::Protocol(ev)
                if matches!(
                    ev.as_ref(),
                    Event::RelayDemotedBattery { battery_level: 20, min_required: 30 }
                )
        )),
        "battery below minimum must emit RelayDemotedBattery{{20, 30}}, got {demoted:?}",
    );
}

#[test]
fn test_install_before_start_wires_routing_callback() {
    // Regression guard for the post-review lifecycle fix: the routing-
    // decision callback is wired by `install_telemetry_sink`, not by
    // `start()`. A sink installed BEFORE `start()` must therefore
    // observe routing records from the very first `send()` — the old
    // wiring-in-start() path made this silently impossible (install
    // happened, callback stayed unwired, records were dropped on the
    // floor until a stop→start cycle ran start() again).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.start().unwrap();

    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "ping",
    );
    protocol.transport_manager_mut().send(&msg).unwrap();

    let records = sink.take();
    assert!(
        records
            .iter()
            .any(|r| matches!(r, TelemetryRecord::Routing(_))),
        "install-before-start must wire the routing callback immediately, got {records:?}",
    );
}

#[test]
fn test_routing_callback_persists_across_stop_start_cycle() {
    // Regression guard for the post-review lifecycle fix: `stop()` no
    // longer tears down the routing-decision callback. The callback's
    // lifetime is sink-scoped — wired on install, replaced on re-install
    // — not protocol-running-scoped. So a `stop() → start()` cycle must
    // preserve the wiring without the app having to re-install the sink.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    let sink = RecordingTelemetrySink::default();
    protocol
        .install_telemetry_sink(Arc::new(sink.clone()), TelemetryConfig::default())
        .unwrap();
    protocol.start().unwrap();
    protocol.stop().unwrap();
    protocol.start().unwrap();

    // Discard anything emitted during the first start/stop cycle so we
    // can narrow the assertion below to the post-restart send().
    let _ = sink.take();

    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "ping",
    );
    protocol.transport_manager_mut().send(&msg).unwrap();

    let records = sink.take();
    assert!(
        records
            .iter()
            .any(|r| matches!(r, TelemetryRecord::Routing(_))),
        "routing callback must persist across stop→start without re-install, got {records:?}",
    );
}

#[test]
fn test_sink_panic_advances_transport_snapshot_for_at_most_once_delivery() {
    // Regression guard for the per-tick snapshot ordering in
    // `tick_telemetry_categories`: each transport's snapshot entry
    // advances BEFORE the emit call that observes it. A panicking sink
    // is now isolated by `dispatch_record` and the tick continues — but
    // the at-most-once invariant we lock in here is independent of that
    // isolation: even before the sink-panic isolation, the aggregator's
    // cursors had to be at the post-transition state when the panic was
    // observed so the next tick would NOT re-emit the same transition.
    #[derive(Default, Clone)]
    struct PanickingTransportStateSink;
    impl TelemetrySink for PanickingTransportStateSink {
        fn emit(&self, record: &TelemetryRecord) {
            if matches!(record, TelemetryRecord::TransportState(_)) {
                panic!("simulated transport-state sink panic");
            }
        }
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));

    // Start first so BLE becomes Available, then install so the sink's
    // baseline is the current state. This isolates the single synthetic
    // transition we want to observe the panic against.
    protocol.start().unwrap();
    protocol
        .install_telemetry_sink(
            Arc::new(PanickingTransportStateSink),
            TelemetryConfig::default(),
        )
        .unwrap();

    // Precondition: install seeded the snapshot with the live status.
    assert_eq!(
        protocol
            .transport_status_snapshot
            .get(&TransportType::BLE)
            .copied(),
        Some(TransportStatus::Available),
        "install_telemetry_sink must seed the transport-status snapshot",
    );

    // Remove the transport so the next tick synthesises an
    // `Available → Unavailable` transition — the sink panics on it.
    protocol
        .transport_manager_mut()
        .remove_transport(TransportType::BLE);

    // `dispatch_record` isolates the sink panic, so `process()` returns
    // normally even though the sink raised on the TransportState record.
    let result = protocol.process();
    assert!(
        result.is_ok(),
        "sink panic must be isolated by dispatch_record, got {result:?}",
    );

    // The snapshot must have been advanced BEFORE the emit so the next
    // tick does NOT re-emit the same transition to a freshly-installed
    // sink. If the ordering regressed (emit-then-assign), the snapshot
    // would still contain BLE=Available here.
    assert!(
        !protocol
            .transport_status_snapshot
            .contains_key(&TransportType::BLE),
        "transport_status_snapshot must reflect the post-transition state \
         even when the emit panicked, got {:?}",
        protocol.transport_status_snapshot,
    );
}

#[test]
fn test_sink_panic_does_not_drop_unemitted_transport_transitions() {
    // Regression guard for multi-transition panic loss: when a tick
    // produces multiple TransportState records and the sink panics on the
    // first, the ONE transport the panic was observed for must be
    // committed in `transport_status_snapshot` (at-most-once for that
    // transition), and every OTHER pending transition must remain
    // un-advanced so the next tick re-diffs and emits it. The old
    // implementation committed the whole snapshot in a single assignment
    // before the emit loop, silently dropping every post-panic transition.
    //
    // Sink-panic isolation via `dispatch_record` means a single panicking
    // record no longer aborts the whole tick — every record after it in
    // the same `for` loop is still attempted. The post-panic record runs
    // a second time through the same sink: this test deliberately panics
    // ONLY on the first call, so the second record reaches the sink
    // (assertion below: the second transport's snapshot advances to
    // Disconnected too).
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    struct PanicOnFirstTransportState {
        seen: StdMutex<Option<TransportType>>,
        count: AtomicUsize,
    }
    impl TelemetrySink for PanicOnFirstTransportState {
        fn emit(&self, record: &TelemetryRecord) {
            if let TelemetryRecord::TransportState(e) = record {
                let first = self.count.fetch_add(1, Ordering::SeqCst) == 0;
                if first {
                    *self.seen.lock().unwrap() = Some(e.transport);
                    panic!("simulated transport-state sink panic");
                }
            }
        }
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let ble = MockTransport::new(TransportType::BLE);
    let net = MockTransport::new(TransportType::Internet);
    let ble_handle = ble.clone();
    let net_handle = net.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(net));
    protocol.start().unwrap();

    let sink = Arc::new(PanicOnFirstTransportState {
        seen: StdMutex::new(None),
        count: AtomicUsize::new(0),
    });
    protocol
        .install_telemetry_sink(sink.clone(), TelemetryConfig::default())
        .unwrap();

    // Flip both transports so the tick produces two TransportState
    // transitions — iteration order through the diff's HashSet is
    // unspecified, so either transport may be first.
    ble_handle.set_status(TransportStatus::Disconnected);
    net_handle.set_status(TransportStatus::Disconnected);

    // `dispatch_record` isolates the first-record panic, so the loop
    // continues into the second record and `process()` returns normally.
    let result = protocol.process();
    assert!(
        result.is_ok(),
        "sink panic must be isolated by dispatch_record, got {result:?}",
    );

    let panicked_on = sink
        .seen
        .lock()
        .unwrap()
        .expect("sink recorded which transport it panicked on");
    let other = if panicked_on == TransportType::BLE {
        TransportType::Internet
    } else {
        TransportType::BLE
    };

    // The panicking transition must be committed to the snapshot so it
    // is NOT re-delivered next tick (at-most-once for that entry).
    assert_eq!(
        protocol
            .transport_status_snapshot
            .get(&panicked_on)
            .copied(),
        Some(TransportStatus::Disconnected),
        "snapshot entry for the panicking transition must be advanced, got {:?}",
        protocol.transport_status_snapshot,
    );

    // With dispatch_record isolating the first panic, the second record
    // in the same tick still reaches the sink (the count check inside
    // `PanicOnFirstTransportState` only panics on the first call). Its
    // snapshot must therefore also advance to Disconnected. The second
    // record was actually emitted in this same tick — the count went
    // from 1 to 2 — but it did not panic, so there is no observable side
    // effect on `seen` to compare against. The snapshot advance is the
    // load-bearing assertion.
    assert_eq!(
        protocol.transport_status_snapshot.get(&other).copied(),
        Some(TransportStatus::Disconnected),
        "snapshot entry for the post-panic transition must also advance \
         because dispatch_record isolated the panic and the loop continued, \
         got {:?}",
        protocol.transport_status_snapshot,
    );
    assert_eq!(
        sink.count.load(Ordering::SeqCst),
        2,
        "sink must have been called for both transitions in the same tick \
         after the first panicked",
    );
}

#[test]
fn test_sink_panic_in_protocol_event_does_not_poison_shared_state() {
    // Regression guard for the structural panic-poisoning bug:
    // `SharedState::emit_event` is called by callers that hold a live
    // `MutexGuard<SharedState>` (e.g. `receive.rs`, `message_dispatch.rs`).
    // Without `dispatch_record`'s `catch_unwind`, a panicking sink would
    // unwind through the guard, poison the mutex on drop, and degrade
    // every subsequent SDK operation that needs the lock. After the fix,
    // the panic is isolated and a follow-up `emit_event` succeeds — proof
    // the mutex was not poisoned.
    #[derive(Default)]
    struct PanicOnProtocolSink;
    impl TelemetrySink for PanicOnProtocolSink {
        fn emit(&self, record: &TelemetryRecord) {
            if matches!(record, TelemetryRecord::Protocol(_)) {
                panic!("simulated protocol-event sink panic");
            }
        }
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // Register a legacy `on_event` handler. The legacy contract is that it
    // sees every event regardless of sink behavior — verifying it after a
    // sink panic proves the legacy fan-out wasn't damaged either.
    let legacy: Arc<Mutex<Vec<crate::events::Event>>> = Arc::new(Mutex::new(Vec::new()));
    let legacy_handler = legacy.clone();
    protocol.on_event(move |event| legacy_handler.lock().unwrap().push(event));

    protocol
        .install_telemetry_sink(Arc::new(PanicOnProtocolSink), TelemetryConfig::default())
        .unwrap();

    // First emit: sink panics, must be isolated.
    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));

    // Second emit: lock must not be poisoned. If the panic had unwound
    // through the live `MutexGuard`, this call would fail to acquire the
    // lock and the legacy handler below would be missing the second event.
    protocol.emit_event(crate::events::Event::neighbor_lost("bob".into()));

    let legacy_events = legacy.lock().unwrap().clone();
    assert_eq!(
        legacy_events.len(),
        2,
        "legacy callback must have observed both events; missing one means \
         the sink panic poisoned SharedState's mutex. got {legacy_events:?}",
    );

    // And the protocol's lock is observably healthy: emit a third event
    // and verify it lands.
    protocol.emit_event(crate::events::Event::neighbor_lost("carol".into()));
    let post_count = legacy.lock().unwrap().len();
    assert_eq!(
        post_count, 3,
        "post-panic emit must succeed; mutex must not be poisoned",
    );
}

#[test]
fn test_sink_panic_in_routing_decision_does_not_poison_shared_state() {
    // The routing-decision callback installed by `install_telemetry_sink`
    // locks `shared_routing` (a clone of `self.shared_state`) and then
    // dispatches to the sink. A panic from the sink unwinds through the
    // live `MutexGuard` from the closure-side `lock()` call. After the
    // fix, dispatch is panic-isolated and the next `send()`/`emit_event`
    // continues to acquire the lock cleanly.
    #[derive(Default)]
    struct PanicOnRoutingSink;
    impl TelemetrySink for PanicOnRoutingSink {
        fn emit(&self, record: &TelemetryRecord) {
            if matches!(record, TelemetryRecord::Routing(_)) {
                panic!("simulated routing-decision sink panic");
            }
        }
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mock = MockTransport::new(TransportType::BLE);
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    protocol
        .install_telemetry_sink(Arc::new(PanicOnRoutingSink), TelemetryConfig::default())
        .unwrap();

    // Trigger a routing decision via send — the routing-decision callback
    // fires inside `TransportManager::send`.
    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "ping",
    );
    protocol.transport_manager_mut().send(&msg).unwrap();

    // If the sink panic poisoned `SharedState`, this `emit_event` call
    // would fail to acquire the lock and the legacy handler would never
    // record the event. (Lock poisoning surfaces as `Err` from
    // `lock_shared_state`; the public `emit_event` swallows that error
    // silently, so we use a legacy handler as the observation point.)
    let legacy: Arc<Mutex<Vec<crate::events::Event>>> = Arc::new(Mutex::new(Vec::new()));
    let legacy_handler = legacy.clone();
    protocol.on_event(move |event| legacy_handler.lock().unwrap().push(event));
    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));

    let count = legacy.lock().unwrap().len();
    assert_eq!(
        count, 1,
        "post-routing-panic emit_event must reach the legacy handler; \
         missing means the routing-callback panic poisoned SharedState",
    );
}

#[test]
fn test_sink_panic_in_mls_lifecycle_does_not_panic_protocol() {
    // `OfflineProtocol::initialize_mls` calls `emit_mls_initialized`,
    // which dispatches the lifecycle event through the sink. Before
    // `dispatch_record`, a panicking sink would unwind through
    // `initialize_mls` and the call would return via panic instead of
    // `Result`. After the fix, `initialize_mls` returns `Ok(())` and the
    // legacy MLS emitter still observes the event.
    #[derive(Default)]
    struct PanicOnMlsSink;
    impl TelemetrySink for PanicOnMlsSink {
        fn emit(&self, record: &TelemetryRecord) {
            if matches!(record, TelemetryRecord::Mls(_)) {
                panic!("simulated MLS-lifecycle sink panic");
            }
        }
    }

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let legacy_emitter = RecordingMlsEmitter::default();
    protocol.set_mls_event_emitter(Arc::new(legacy_emitter.clone()));

    protocol
        .install_telemetry_sink(Arc::new(PanicOnMlsSink), TelemetryConfig::default())
        .unwrap();

    // Drives `emit_mls_initialized`. With the fix, this returns Ok even
    // though the sink raised. Without the fix, the panic would propagate
    // and this test would fail with the propagated panic message instead
    // of an assertion failure.
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let result = protocol.initialize_mls_for_test(storage);
    assert!(
        result.is_ok(),
        "initialize_mls must return Ok even when the sink panics on MLS records, got {result:?}",
    );

    let legacy_events = legacy_emitter.take();
    assert!(
        legacy_events
            .iter()
            .any(|e| matches!(e, MlsLifecycleEvent::Initialized { .. })),
        "legacy MLS emitter must still observe Initialized; sink panic must not \
         break the legacy fan-out, got {legacy_events:?}",
    );
}

#[test]
fn test_legacy_event_handler_panic_does_not_poison_shared_state() {
    // Symmetry guard for the legacy `EventCallback` fan-out: handlers
    // registered via `on_event` are also called from `SharedState::emit_event`
    // while a `MutexGuard<SharedState>` is held by the caller. A panicking
    // legacy handler would have the same mutex-poisoning effect as a
    // panicking sink. The fan-out wraps each handler in `catch_unwind` for
    // exactly this reason.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let panicked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let panicked_handle = panicked.clone();
    protocol.on_event(move |_event| {
        panicked_handle.store(true, std::sync::atomic::Ordering::SeqCst);
        panic!("simulated legacy handler panic");
    });

    let surviving: Arc<Mutex<Vec<crate::events::Event>>> = Arc::new(Mutex::new(Vec::new()));
    let surviving_handle = surviving.clone();
    protocol.on_event(move |event| surviving_handle.lock().unwrap().push(event));

    // First emit triggers the panic from handler 1.
    protocol.emit_event(crate::events::Event::neighbor_lost("alice".into()));

    assert!(
        panicked.load(std::sync::atomic::Ordering::SeqCst),
        "first handler must have been invoked",
    );

    // Handler 2 must still have observed the same event despite handler 1
    // panicking — proves the for-loop didn't unwind through the second
    // handler.
    let surviving_events = surviving.lock().unwrap().clone();
    assert_eq!(
        surviving_events.len(),
        1,
        "subsequent handler must run after a previous handler panicked, got {surviving_events:?}",
    );

    // Second emit proves the lock is not poisoned.
    protocol.emit_event(crate::events::Event::neighbor_lost("bob".into()));
    let post_count = surviving.lock().unwrap().len();
    assert_eq!(
        post_count, 2,
        "post-panic emit must reach surviving handler; mutex must not be poisoned",
    );
}

use crate::telemetry::routing::RoutingDecision;
use crate::telemetry::transport_state::TransportStateEvent;

// `device_battery_from_available` lives in `crate::telemetry::aggregator`
// and is exercised by the unit tests in that module — including the
// current-not-in-available fall-through path that was previously uncovered.

// ============================================================================
// PERSISTENT TELEMETRY SCRUB SECRET
// ============================================================================

#[test]
fn scrub_secret_is_persisted_and_stable_across_instances() {
    // Shared storage stands in for an app's secure store across two launches.
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    let mut first = OfflineProtocol::new(create_test_config()).unwrap();
    first
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    let first_secret = first.telemetry_fallback_secret;
    assert!(first.telemetry_secret_persisted);

    // The secret was written to storage under the documented key.
    let stored = storage
        .load(storage_keys::SCRUB_SECRET, storage_keys::SCRUB_SECRET_ID)
        .unwrap()
        .expect("scrub secret should be persisted");
    assert_eq!(stored, first_secret.to_vec());

    // A second instance backed by the same storage adopts the same secret, so
    // the same raw identifier hashes to the same opaque id across sessions.
    let mut second = OfflineProtocol::new(create_test_config()).unwrap();
    second
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(second.telemetry_fallback_secret, first_secret);
    assert_eq!(
        first.telemetry_scrubber.hash_id("peer:alice"),
        second.telemetry_scrubber.hash_id("peer:alice"),
    );
}

#[test]
fn scrub_secret_load_is_idempotent_across_entry_paths() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // initialize_mls also enables persistence; a later explicit
    // enable_message_persistence must not rotate or rewrite the secret.
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol.initialize_mls_for_test(storage.clone()).unwrap();
    let secret_after_init = protocol.telemetry_fallback_secret;
    assert!(protocol.telemetry_secret_persisted);

    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();
    assert_eq!(protocol.telemetry_fallback_secret, secret_after_init);
}

#[test]
fn corrupt_persisted_scrub_secret_is_regenerated() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    // Pre-seed a wrong-length blob to simulate corruption.
    storage
        .store(
            storage_keys::SCRUB_SECRET,
            storage_keys::SCRUB_SECRET_ID,
            &[1, 2, 3],
        )
        .unwrap();

    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    // A fresh 16-byte secret replaces the corrupt entry rather than panicking
    // or pinning every future session to the random fallback.
    let stored = storage
        .load(storage_keys::SCRUB_SECRET, storage_keys::SCRUB_SECRET_ID)
        .unwrap()
        .expect("corrupt secret should be overwritten");
    assert_eq!(stored.len(), 16);
    assert_eq!(stored, protocol.telemetry_fallback_secret.to_vec());
    assert!(protocol.telemetry_secret_persisted);
}

#[test]
fn explicit_config_secret_wins_over_persistent_fallback() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();

    // Installing a sink with an explicit scrub_secret must use that secret for
    // opaque-id hashing, not the SDK-managed persistent fallback.
    let explicit = [0xAB; 16];
    let cfg = TelemetryConfig::default().with_scrub_secret(Some(explicit));
    assert_ne!(explicit, protocol.telemetry_fallback_secret);
    protocol
        .install_telemetry_sink(Arc::new(NoopTelemetrySink), cfg)
        .unwrap();

    let installed = protocol.telemetry.as_ref().unwrap();
    let expected = crate::telemetry::Scrubber::new(true, explicit)
        .hash_id("peer:alice")
        .into_owned();
    assert_eq!(installed.scrubber.hash_id("peer:alice"), expected);
}

#[test]
fn scrub_secret_without_storage_keeps_random_per_instance_fallback() {
    // No storage provided: behavior is unchanged from the legacy random
    // per-instance fallback — two instances differ and nothing is persisted.
    let a = OfflineProtocol::new(create_test_config()).unwrap();
    let b = OfflineProtocol::new(create_test_config()).unwrap();
    assert!(!a.telemetry_secret_persisted);
    assert_ne!(a.telemetry_fallback_secret, b.telemetry_fallback_secret);
}

// ============================================================================
// PERSISTENT NOSTR SIGNING SECRET (SEC-M4)
// ============================================================================

fn protocol_with_nostr_transport() -> OfflineProtocol {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let nostr = offline_protocol_transport::NostrTransport::new("test_user").unwrap();
    protocol
        .transport_manager
        .add_transport(TransportType::Nostr, Box::new(nostr));
    protocol
}

fn nostr_signing_pubkey(protocol: &OfflineProtocol) -> String {
    let arc = protocol
        .transport_manager
        .get_transport(TransportType::Nostr)
        .expect("nostr transport registered");
    arc.as_any()
        .downcast_ref::<offline_protocol_transport::NostrTransport>()
        .expect("registered transport should be a NostrTransport")
        .public_key_hex()
}

#[test]
fn nostr_signing_secret_is_persisted_and_stable_across_instances() {
    // Shared storage stands in for an app's secure store across two launches.
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    let mut first = protocol_with_nostr_transport();
    let ephemeral_pubkey = nostr_signing_pubkey(&first);
    first
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert!(first.nostr_secret_persisted);

    // The persisted secret replaced the construction-time ephemeral key.
    let first_pubkey = nostr_signing_pubkey(&first);
    assert_ne!(first_pubkey, ephemeral_pubkey);

    // A 32-byte secret was written under the documented key.
    let stored = storage
        .load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        )
        .unwrap()
        .expect("nostr signing secret should be persisted");
    assert_eq!(stored.len(), 32);

    // A second instance backed by the same storage derives the same signing
    // identity — the install's Nostr pubkey is stable across restarts.
    let mut second = protocol_with_nostr_transport();
    assert_ne!(nostr_signing_pubkey(&second), first_pubkey);
    second
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    assert_eq!(nostr_signing_pubkey(&second), first_pubkey);
}

#[test]
fn nostr_signing_secret_load_is_idempotent_across_entry_paths() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    // initialize_mls also enables persistence; a later explicit
    // enable_message_persistence must not rotate the signing identity.
    let mut protocol = protocol_with_nostr_transport();
    protocol.initialize_mls_for_test(storage.clone()).unwrap();
    assert!(protocol.nostr_secret_persisted);
    let pubkey_after_init = nostr_signing_pubkey(&protocol);

    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();
    assert_eq!(nostr_signing_pubkey(&protocol), pubkey_after_init);
}

#[test]
fn corrupt_persisted_nostr_secret_is_regenerated() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    // Pre-seed a wrong-length blob to simulate corruption.
    storage
        .store(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
            &[1, 2, 3],
        )
        .unwrap();

    let mut protocol = protocol_with_nostr_transport();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    // A fresh 32-byte secret replaces the corrupt entry rather than pinning
    // every future session to the ephemeral key.
    let stored = storage
        .load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        )
        .unwrap()
        .expect("corrupt secret should be overwritten");
    assert_eq!(stored.len(), 32);
    assert!(protocol.nostr_secret_persisted);
}

#[test]
fn nostr_secret_store_failure_retries_the_same_secret() {
    let storage = Arc::new(FailingNostrSecretStorage::default());
    storage
        .fail_store
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let mut protocol = protocol_with_nostr_transport();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    // Store failed: the fresh secret was installed (stable identity for this
    // session) but not persisted, and is kept for a retry.
    assert!(!protocol.nostr_secret_persisted);
    assert!(protocol.nostr_unpersisted_secret.is_some());
    let session_pubkey = nostr_signing_pubkey(&protocol);
    assert!(storage
        .load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        )
        .unwrap()
        .is_none());

    // Once storage recovers, the next entry path persists the *same* secret
    // instead of rotating the identity mid-session.
    storage
        .fail_store
        .store(false, std::sync::atomic::Ordering::SeqCst);
    protocol.initialize_mls_for_test(storage.clone()).unwrap();

    assert!(protocol.nostr_secret_persisted);
    assert!(protocol.nostr_unpersisted_secret.is_none());
    assert_eq!(nostr_signing_pubkey(&protocol), session_pubkey);
    let stored = storage
        .load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        )
        .unwrap()
        .expect("secret should be persisted after retry");
    assert_eq!(stored.len(), 32);
}

#[test]
fn nostr_secret_restore_without_nostr_transport_is_a_noop() {
    // Storage-backed protocols without a Nostr transport must not persist a
    // secret they cannot install (and must not fail initialization).
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();

    assert!(!protocol.nostr_secret_persisted);
    assert!(storage
        .load(
            storage_keys::NOSTR_SIGNING_SECRET,
            storage_keys::NOSTR_SIGNING_SECRET_ID,
        )
        .unwrap()
        .is_none());
}

#[test]
fn telemetry_install_id_is_none_without_storage() {
    // The per-instance fallback secret is random, so an id derived from it
    // would rotate every launch — the accessor must expose nothing instead.
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();
    assert_eq!(protocol.telemetry_install_id(), None);
}

#[test]
fn telemetry_install_id_is_stable_across_instances_sharing_storage() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());

    let mut first = OfflineProtocol::new(create_test_config()).unwrap();
    first
        .enable_message_persistence_for_test(storage.clone())
        .unwrap();
    let first_id = first
        .telemetry_install_id()
        .expect("id should be available once the secret is persisted");
    assert_eq!(first_id.len(), 32);
    assert!(first_id.chars().all(|c| c.is_ascii_hexdigit()));

    // Same storage = same install: the id must match across sessions so
    // backend distinct-device aggregation counts one device, not many.
    let mut second = OfflineProtocol::new(create_test_config()).unwrap();
    second.enable_message_persistence_for_test(storage).unwrap();
    assert_eq!(second.telemetry_install_id(), Some(first_id));
}

#[test]
fn telemetry_install_id_differs_across_installs() {
    let mut a = OfflineProtocol::new(create_test_config()).unwrap();
    a.enable_message_persistence_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let mut b = OfflineProtocol::new(create_test_config()).unwrap();
    b.enable_message_persistence_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    assert_ne!(a.telemetry_install_id(), b.telemetry_install_id());
}

#[test]
fn telemetry_install_id_survives_explicit_config_secret() {
    // An app-supplied scrub_secret overrides opaque-id hashing for telemetry
    // records, but must not rotate (or make computable) the install id —
    // it stays pinned to the SDK-managed persistent secret.
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();
    let before = protocol.telemetry_install_id();
    assert!(before.is_some());

    let cfg = TelemetryConfig::default().with_scrub_secret(Some([0xAB; 16]));
    protocol
        .install_telemetry_sink(Arc::new(NoopTelemetrySink), cfg)
        .unwrap();
    assert_eq!(protocol.telemetry_install_id(), before);
}

#[test]
fn telemetry_install_id_is_domain_separated_from_scrubbed_ids() {
    // Ordinary scrubbed leaf identifiers must not correlate with the install
    // id: both derive from the same secret, so separation requires that no
    // raw identifier reaching the scrubber can ever equal the domain string.
    // The domain contains ':', which id validation rejects in every
    // UserId/AppId — assert that structural invariant directly so a future
    // relaxation of the charset (or a domain edit) fails loudly here.
    assert!(
        UserId::new("telemetry:install-id").is_err(),
        "domain must never be a constructible UserId"
    );
    assert!(
        AppId::new("telemetry:install-id").is_err(),
        "domain must never be a constructible AppId"
    );

    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();
    let id = protocol.telemetry_install_id().unwrap();
    assert_ne!(
        id,
        protocol
            .telemetry_scrubber
            .hash_id("some-user-id")
            .into_owned()
    );
}

#[test]
fn telemetry_install_id_derivation_is_frozen() {
    // Contract test: the install id is SHA-256(secret || "telemetry:install-id")
    // truncated to 32 hex chars. Backends key device aggregation on this value,
    // so changing the domain string, hash, or truncation silently rotates every
    // install id in the field. If this test fails, the change is a breaking one
    // — do not update the expected value without a migration story.
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    storage
        .store(
            storage_keys::SCRUB_SECRET,
            storage_keys::SCRUB_SECRET_ID,
            &[0x42; 16],
        )
        .unwrap();
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();
    assert_eq!(
        protocol.telemetry_install_id().as_deref(),
        Some("888112d3efecdb64bbc60cb9b55359c6")
    );
}

#[test]
fn telemetry_install_id_is_none_when_secret_persist_fails() {
    // When the freshly generated secret cannot be persisted, the SDK keeps a
    // session-local secret — the id would rotate next launch, so the accessor
    // must expose nothing (the documented "persisting failed this session"
    // contract in the UDL/TS docs).
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    protocol
        .enable_message_persistence_for_test(Arc::new(FailingScrubSecretStorage::default()))
        .unwrap();
    assert_eq!(protocol.telemetry_install_id(), None);
}

// ============================================================================
// MEDIA ENCRYPTION (SEC-H1)
// ============================================================================

/// Creates a protocol instance with MLS initialized, auto-encryption enabled,
/// and a started BLE MockTransport. Returns the transport handle for wire
/// inspection and message injection.
fn media_test_protocol(user_id: &str) -> (OfflineProtocol, MockTransport) {
    let mut config = create_test_config_for_user(user_id);
    config.encryption.enabled = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();
    (protocol, handle)
}

/// Establishes a real, confirmed 1:1 MLS session between two protocol
/// instances by wiring the key package and Welcome at the manager level.
fn establish_media_session(alice: &mut OfflineProtocol, bob: &mut OfflineProtocol) {
    let bob_kp = {
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.generate_key_package().unwrap()
    };
    let welcome = {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    {
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.process_welcome(&welcome).unwrap();
    }
    alice.confirm_session_state("bob", "test_setup").unwrap();
    bob.confirm_session_state("alice", "test_setup").unwrap();
}

type ReceivedFile = (
    String,
    String,
    String,
    Option<offline_protocol_core::MediaMetadata>,
    Vec<u8>,
);

/// Captures FileReceived events as (file_name, sender, content_type,
/// media_metadata, decoded file bytes).
fn capture_file_received(protocol: &mut OfflineProtocol) -> Arc<Mutex<Vec<ReceivedFile>>> {
    let store: Arc<Mutex<Vec<ReceivedFile>>> = Arc::new(Mutex::new(Vec::new()));
    let store_clone = store.clone();
    protocol.on_event(move |event| {
        if let Event::FileReceived {
            file_name,
            sender,
            content_type,
            media_metadata,
            file_data,
            ..
        } = event
        {
            use base64::{engine::general_purpose::STANDARD, Engine};
            store_clone.lock().unwrap().push((
                file_name.clone(),
                sender.clone(),
                content_type.clone(),
                media_metadata.clone(),
                STANDARD.decode(file_data).unwrap(),
            ));
        }
    });
    store
}

fn sample_media_metadata(file_size: u64) -> offline_protocol_core::MediaMetadata {
    offline_protocol_core::MediaMetadata {
        mime_type: "image/jpeg".to_string(),
        file_name: "secret-photo.jpg".to_string(),
        file_size,
        duration_ms: None,
        width: Some(10),
        height: Some(10),
        thumbnail_base64: Some("c2VjcmV0LXRodW1i".to_string()),
        media_id: None,
        download_url: None,
        thumbnail_url: None,
        encryption_key: None,
        iv: None,
        ciphertext_hash: None,
        sticker_provider: None,
        sticker_remote_id: None,
        sticker_kind: None,
    }
}

/// Builds a legacy (pre-encryption wire format) plaintext chunk message.
fn legacy_chunk_message(sender: &str, recipient: &str, data: &[u8]) -> Message {
    use crate::file_transfer::FileChunk;
    use sha2::{Digest, Sha256};

    let chunk = FileChunk {
        file_id: "file_legacy1".to_string(),
        file_name: "legacy.bin".to_string(),
        file_size: data.len() as u64,
        total_chunks: 1,
        chunk_index: 0,
        chunk_data: data.to_vec(),
        file_checksum: format!("{:x}", Sha256::digest(data)),
    };

    let mut msg = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new(recipient).unwrap(),
        AppId::new("test-app").unwrap(),
        "",
    );
    msg.content_type = ContentType::FileChunk;
    msg.binary_content = Some(chunk.to_bytes());
    msg
}

#[test]
fn compact_envelope_end_to_end_with_negotiated_peers() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    // Alice learned bob's compact-envelope capability from his key package.
    feed_key_package_with_env(&mut alice, "bob", vec![MLS_ENVELOPE_COMPACT_V1]);

    alice
        .send_message(
            "bob",
            "hello over compact",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("an __MLS_ENC__ message must reach the wire");
    let body = enc
        .content
        .strip_prefix(internal_prefixes::ENCRYPTED)
        .unwrap();
    assert!(
        !body.starts_with('{'),
        "negotiated recipient must get the compact envelope"
    );

    // Bob decrypts it through the normal inbound path.
    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "hello over compact"),
        "compact envelope must decrypt end-to-end, got {result:?}"
    );
}

#[test]
fn compact_envelope_interop_legacy_recipient_and_ungated_parsing() {
    // Alice (compact on) -> bob, who never advertised the capability: the
    // envelope must stay legacy JSON and still decrypt.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    alice
        .send_message(
            "bob",
            "hello legacy",
            None::<MessagePriority>,
            None::<String>,
        )
        .unwrap();
    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("an __MLS_ENC__ message must reach the wire");
    let body = enc
        .content
        .strip_prefix(internal_prefixes::ENCRYPTED)
        .unwrap();
    assert!(
        body.starts_with('{'),
        "unadvertised capability must fall back to the JSON envelope"
    );
    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "hello legacy")
    );

    // Reverse direction with carol, whose OWN kill switch is off: parsing is
    // never gated, so a compact envelope sent to her still decrypts. (This is
    // also the safety net if a stale capability record ever mis-gates a send.)
    let mut carol_config = create_test_config_for_user("carol");
    carol_config.encryption.enabled = true;
    carol_config.encryption.compact_envelope_enabled = false;
    let mut carol = OfflineProtocol::new(carol_config).unwrap();
    carol
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let (mut dave, dave_handle) = media_test_protocol("dave");
    // establish_media_session hardcodes alice/bob ids; wire dave<->carol by hand.
    let carol_kp = {
        let mls = carol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.generate_key_package().unwrap()
    };
    let welcome = {
        let mls = dave.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("carol", &carol_kp.key_package_data)
            .unwrap();
        manager.create_session("carol").unwrap()
    };
    {
        let mls = carol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.process_welcome(&welcome).unwrap();
    }
    dave.confirm_session_state("carol", "test_setup").unwrap();
    carol.confirm_session_state("dave", "test_setup").unwrap();
    feed_key_package_with_env(&mut dave, "carol", vec![MLS_ENVELOPE_COMPACT_V1]);

    dave.send_message(
        "carol",
        "compact anyway",
        None::<MessagePriority>,
        None::<String>,
    )
    .unwrap();
    let sent = dave_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .unwrap();
    let body = enc
        .content
        .strip_prefix(internal_prefixes::ENCRYPTED)
        .unwrap();
    assert!(!body.starts_with('{'));
    let result = carol.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "compact anyway"),
        "flag-off receiver must still parse the compact envelope, got {result:?}"
    );
}

#[test]
fn rich_payload_end_to_end_with_negotiated_peer() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    // Alice learned bob's rich-payload capability from his key package.
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    let extras = sample_rich_extras();
    alice
        .send_message_with(
            "bob",
            "hello rich",
            SendMessageOptions {
                reply_context: extras.reply_context.clone(),
                media_metadata: extras.media_metadata.clone(),
                forward_info: extras.forward_info.clone(),
                ..Default::default()
            },
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("an __MLS_ENC__ message must reach the wire");

    // The outer message must carry no rich cleartext anywhere.
    assert!(enc.reply_context.is_none());
    assert!(enc.media_metadata.is_none());
    assert!(enc.forwarded_from.is_none());
    let outer_json = serde_json::to_string(enc).unwrap();
    assert!(!outer_json.contains("the quoted message"));
    assert!(!outer_json.contains("a2V5LWJ5dGVz"));

    // Bob decrypts to the sealed body; the shared restore seam repopulates
    // the rich fields for both the live and delayed emission paths.
    let result = bob.process_internal_message(enc);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("rich message must decrypt, got {result:?}");
    };
    assert!(text.starts_with(internal_prefixes::RICH_V1));
    let mut delivered = enc.clone();
    OfflineProtocol::apply_decrypted_content(&mut delivered, text);
    assert_eq!(delivered.content, "hello rich");
    assert_eq!(
        delivered.reply_context.as_ref().map(|rc| rc.text.as_str()),
        Some("the quoted message")
    );
    assert_eq!(
        delivered
            .media_metadata
            .as_ref()
            .and_then(|m| m.encryption_key.as_deref()),
        Some("a2V5LWJ5dGVz")
    );
    assert_eq!(
        delivered
            .forwarded_from
            .as_ref()
            .map(|f| f.original_sender.as_str()),
        Some("dave")
    );
}

#[test]
fn rich_payload_degrades_to_plain_text_for_legacy_recipient() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    // Bob never advertised rich_versions.

    let extras = sample_rich_extras();
    alice
        .send_message_with(
            "bob",
            "hello plain",
            SendMessageOptions {
                reply_context: extras.reply_context.clone(),
                media_metadata: extras.media_metadata.clone(),
                ..Default::default()
            },
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("an __MLS_ENC__ message must reach the wire");

    // Degrade rule: rich extras are dropped, never cleartext — nowhere on
    // the outer frame.
    assert!(enc.reply_context.is_none());
    assert!(enc.media_metadata.is_none());
    let outer_json = serde_json::to_string(enc).unwrap();
    assert!(!outer_json.contains("the quoted message"));
    assert!(!outer_json.contains("a2V5LWJ5dGVz"));

    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "hello plain"),
        "legacy recipient must get plain text with rich extras dropped, got {result:?}"
    );
}

#[test]
fn rich_payload_flush_pending_seals_with_current_capability() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    // Queue a rich message as if the session had not been ready at send
    // time; the extras ride the pending queue in `PendingMessage::rich`.
    alice.queue_pending_message(
        "bob",
        "queued rich",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
        Some(sample_rich_extras()),
    );
    alice.flush_pending_messages("bob").unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("flushed rich message must reach the wire");
    let result = bob.process_internal_message(enc);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("flushed rich message must decrypt, got {result:?}");
    };
    assert!(
        text.starts_with(internal_prefixes::RICH_V1),
        "flush must re-make the seal decision against current capability"
    );
    let mut delivered = enc.clone();
    OfflineProtocol::apply_decrypted_content(&mut delivered, text);
    assert_eq!(delivered.content, "queued rich");
    assert_eq!(
        delivered.reply_context.as_ref().map(|rc| rc.text.as_str()),
        Some("the quoted message")
    );
}

#[test]
fn rich_payload_flush_pending_drops_extras_for_legacy_recipient() {
    // The negative half of the flush-time re-decision: extras queued toward
    // a recipient who never advertised rich_versions must be DROPPED at
    // flush — bare text inside the ciphertext, nothing rich on the outer
    // frame. This is the "sealed or dropped, never cleartext" invariant at
    // its most fragile seam.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    // Bob never fed alice a rich-capable key package.

    alice.queue_pending_message(
        "bob",
        "queued rich",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
        Some(sample_rich_extras()),
    );
    alice.flush_pending_messages("bob").unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("flushed message must reach the wire");

    // Nothing rich anywhere on the outer frame.
    assert!(enc.reply_context.is_none());
    assert!(enc.media_metadata.is_none());
    assert!(enc.forwarded_from.is_none());
    let outer_json = serde_json::to_string(enc).unwrap();
    assert!(!outer_json.contains("the quoted message"));
    assert!(!outer_json.contains("a2V5LWJ5dGVz"));

    // And the plaintext is bare text, not a sealed rich body.
    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "queued rich"),
        "flush toward a legacy recipient must drop extras to bare text, got {result:?}"
    );
}

#[test]
fn rich_send_seals_content_type_against_relay_rewrite() {
    // The sealed content_type copy end-to-end: a relay rewriting the outer
    // rendering hint on a rich message (Image -> FileChunk would even
    // misroute it into the file-transfer manager) must lose to the sealed
    // copy once the recipient decrypts.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    alice
        .send_message_with(
            "bob",
            "look at this",
            SendMessageOptions {
                content_type: Some(ContentType::Image),
                media_metadata: Some(sample_media_metadata(9)),
                ..Default::default()
            },
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("rich message must reach the wire");

    // Simulate a hostile relay restamping the outer hint in transit.
    let mut tampered = enc.clone();
    tampered.content_type = ContentType::FileChunk;

    let result = bob.process_internal_message(&tampered);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("tampered outer hint must not break decryption, got {result:?}");
    };
    OfflineProtocol::apply_decrypted_content(&mut tampered, text);
    assert_eq!(tampered.content, "look at this");
    assert_eq!(
        tampered.content_type,
        ContentType::Image,
        "sealed content_type must overwrite the relay-rewritten outer hint"
    );
}

/// Fixture: a received cloud-media message, as an app hands it back to
/// `forward_message` — download URL plus the content-encryption secrets.
#[cfg(test)]
fn cloud_media_original(sender: &str, recipient: &str) -> Message {
    let mut original = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new(recipient).unwrap(),
        AppId::new("test-app").unwrap(),
        "check out this photo",
    );
    original.content_type = ContentType::Image;
    let mut media = sample_media_metadata(42);
    media.download_url = Some("https://cdn.example/blob/1".to_string());
    media.encryption_key = Some("a2V5LWJ5dGVz".to_string());
    media.iv = Some("aXYtYnl0ZXM=".to_string());
    original.media_metadata = Some(media);
    original
}

#[test]
fn forward_message_seals_media_secrets_for_capable_recipient() {
    // The forward-path half of the cloud-media contract: forwarding a
    // message whose metadata carries the content key must deliver that key
    // to a rich-capable recipient — inside the sealed body, never outer.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    let original = cloud_media_original("dave", "alice");
    alice.forward_message(&original, "bob", None).unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("forwarded message must reach the wire");
    // The secrets ride only inside the ciphertext (the outer copy is
    // stripped by the transport chokepoint at wire encoding).
    assert!(!enc.content.contains("a2V5LWJ5dGVz"));

    let result = bob.process_internal_message(enc);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("forwarded message must decrypt, got {result:?}");
    };
    let mut delivered = enc.clone();
    OfflineProtocol::apply_decrypted_content(&mut delivered, text);

    assert_eq!(delivered.content, "check out this photo");
    let media = delivered
        .media_metadata
        .expect("sealed media metadata restored");
    assert_eq!(media.encryption_key.as_deref(), Some("a2V5LWJ5dGVz"));
    assert_eq!(media.iv.as_deref(), Some("aXYtYnl0ZXM="));
    assert_eq!(
        media.download_url.as_deref(),
        Some("https://cdn.example/blob/1")
    );
    let fwd = delivered
        .forwarded_from
        .expect("attribution restored from the sealed body");
    assert_eq!(fwd.original_sender.as_str(), "dave");
    assert_eq!(fwd.forward_count, 1);
    assert_eq!(
        delivered.content_type,
        ContentType::Image,
        "sealed content_type copy must survive the forward"
    );
}

#[test]
fn forward_message_outer_fallback_for_legacy_recipient() {
    // Toward a recipient that never advertised rich_versions the extras
    // must not seal: the plaintext stays bare text, attribution and the
    // display-hint metadata ride outer (secrets stripped at the wire), and
    // nothing rich leaks into the ciphertext.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    // Bob never fed alice a rich-capable key package.

    let original = cloud_media_original("dave", "alice");
    alice.forward_message(&original, "bob", None).unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("forwarded message must reach the wire");

    // Outer fallback: attribution and non-secret metadata preserved for the
    // legacy recipient.
    let outer_fwd = enc.forwarded_from.as_ref().expect("outer attribution");
    assert_eq!(outer_fwd.original_sender.as_str(), "dave");
    let outer_media = enc.media_metadata.as_ref().expect("outer media metadata");
    assert_eq!(
        outer_media.download_url.as_deref(),
        Some("https://cdn.example/blob/1")
    );

    // Bare text inside the ciphertext — no sealed body toward a legacy peer.
    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "check out this photo"),
        "forward toward a legacy recipient must drop extras to bare text, got {result:?}"
    );
}

#[test]
fn forwarded_media_flush_seals_secrets_for_capable_recipient() {
    // The queued half: a forward parked behind session establishment stores
    // both the outer copies and the rich extras (the exact shape
    // `forward_message` queues), and the flush re-seal must deliver the
    // media secrets end-to-end.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    let original = cloud_media_original("dave", "alice");
    let forward_info = offline_protocol_core::ForwardInfo::from_message(&original);
    let rich = RichSendExtras {
        reply_context: None,
        media_metadata: original.media_metadata.clone(),
        forward_info: Some(forward_info.clone()),
    };
    alice.queue_pending_message(
        "bob",
        &original.content,
        MessagePriority::Medium,
        MessageId::new(),
        None,
        Some(forward_info),
        ContentType::Image,
        original.media_metadata.clone(),
        Some(rich),
    );
    alice.flush_pending_messages("bob").unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("flushed forward must reach the wire");

    let result = bob.process_internal_message(enc);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("flushed forward must decrypt, got {result:?}");
    };
    let mut delivered = enc.clone();
    OfflineProtocol::apply_decrypted_content(&mut delivered, text);

    assert_eq!(delivered.content, "check out this photo");
    let media = delivered
        .media_metadata
        .expect("sealed media metadata restored after flush");
    assert_eq!(media.encryption_key.as_deref(), Some("a2V5LWJ5dGVz"));
    assert!(delivered.forwarded_from.is_some());
}

#[test]
fn forward_message_rejects_oversized_media_metadata() {
    // The forward boundary enforces the same extras cap as
    // `send_message_with`, so a queued forward can never fail its seal at
    // flush time and re-queue forever.
    let (mut alice, _alice_handle) = media_test_protocol("alice");

    let mut original = cloud_media_original("dave", "alice");
    if let Some(media) = original.media_metadata.as_mut() {
        media.thumbnail_base64 = Some("x".repeat(MAX_RICH_EXTRAS_BYTES + 1));
    }
    let err = alice.forward_message(&original, "bob", None).unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "oversized forward extras must be rejected, got: {err:?}"
    );
    assert!(err.to_string().contains("Rich extras too large"));
}

#[test]
fn forward_message_rejects_file_chunk_content_type() {
    // FileChunk is internal transport routing: forwarding it would get the
    // message ACKed and then dropped by the receiver's file-transfer
    // manager. Rejected at the boundary, like `send_message_with`.
    let (mut alice, _alice_handle) = media_test_protocol("alice");

    let mut original = cloud_media_original("dave", "alice");
    original.content_type = ContentType::FileChunk;
    let err = alice.forward_message(&original, "bob", None).unwrap_err();
    assert!(
        matches!(err, crate::Error::InvalidArgument(_)),
        "FileChunk forward must be rejected, got: {err:?}"
    );
    assert!(err.to_string().contains("cannot be forwarded"));
}

#[test]
fn content_type_only_send_seals_hint_against_relay_rewrite() {
    // Even WITHOUT rich extras, a non-Text hint toward a capable recipient
    // seals a hint-only body — otherwise a relay could still restamp plain
    // sealed sends FileChunk (ACKed as delivered, then dropped by the
    // file-transfer manager).
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    alice
        .send_message_with(
            "bob",
            "voice caption",
            SendMessageOptions {
                content_type: Some(ContentType::VoiceNote),
                ..Default::default()
            },
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("hint-only message must reach the wire");
    let mut tampered = enc.clone();
    tampered.content_type = ContentType::FileChunk;

    let result = bob.process_internal_message(&tampered);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("tampered outer hint must not break decryption, got {result:?}");
    };
    assert!(
        text.starts_with(internal_prefixes::RICH_V1),
        "a non-Text hint must seal even without extras"
    );
    OfflineProtocol::apply_decrypted_content(&mut tampered, text);
    assert_eq!(tampered.content, "voice caption");
    assert_eq!(
        tampered.content_type,
        ContentType::VoiceNote,
        "sealed hint must overwrite the relay-rewritten outer value"
    );
}

#[test]
fn plain_text_send_without_extras_stays_bare() {
    // Bare Text toward a capable peer must NOT grow a sealed wrapper:
    // there is no hint to protect, and the unsealed plaintext form stays
    // the interop floor.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    alice
        .send_message_with("bob", "just text", SendMessageOptions::default())
        .unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("message must reach the wire");
    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "just text"),
        "bare Text must stay unsealed, got {result:?}"
    );
}

#[test]
fn rich_payload_flush_after_restart_seals_with_restored_capability() {
    // Issue #200's user-visible half: a rich message queued before an app
    // restart must still seal its extras when the post-restart flush runs.
    // Before capability persistence, the restarted instance had an empty
    // peer_rich_payload set and the flush silently degraded the message to
    // bare text.
    let alice_storage = Arc::new(InMemoryStorage::new());
    let mut alice_config = create_test_config_for_user("alice");
    alice_config.encryption.enabled = true;
    alice_config.encryption.store_pending = true;
    let mut alice = OfflineProtocol::new(alice_config).unwrap();
    alice
        .initialize_mls_for_test(alice_storage.clone())
        .unwrap();

    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    feed_key_package_with_rich(&mut alice, "bob", vec![RICH_PAYLOAD_V1]);

    alice.queue_pending_message(
        "bob",
        "queued rich",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::default(),
        None,
        Some(sample_rich_extras()),
    );

    // Restart alice on the same storage; start() flushes the restored queue.
    let mut alice2_config = create_test_config_for_user("alice");
    alice2_config.encryption.enabled = true;
    alice2_config.encryption.store_pending = true;
    let mut alice2 = OfflineProtocol::new(alice2_config).unwrap();
    alice2.initialize_mls_for_test(alice_storage).unwrap();
    assert!(
        alice2.peer_supports_rich_payload("bob"),
        "restored capability must be visible before the flush runs"
    );
    let transport = MockTransport::new(TransportType::BLE);
    transport.start().unwrap();
    let transport_handle = transport.clone();
    alice2
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(transport));
    alice2.start().unwrap();

    let sent = transport_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("restored rich message must flush to the wire");
    let result = bob.process_internal_message(enc);
    let Some(InternalMessageResult::Decrypted(text)) = result else {
        panic!("flushed message must decrypt, got {result:?}");
    };
    assert!(
        text.starts_with(internal_prefixes::RICH_V1),
        "post-restart flush must seal rich extras against the restored capability"
    );
    let mut delivered = enc.clone();
    OfflineProtocol::apply_decrypted_content(&mut delivered, text);
    assert_eq!(delivered.content, "queued rich");
    assert_eq!(
        delivered.reply_context.as_ref().map(|rc| rc.text.as_str()),
        Some("the quoted message")
    );
}

#[test]
fn flush_pending_preserves_content_type_without_rich_extras() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    // A content_type-only send queued behind session establishment: no rich
    // extras (`rich: None`), so the flush takes the non-forward branch —
    // which must restore the stored content_type instead of resetting to
    // Text.
    alice.queue_pending_message(
        "bob",
        "queued image caption",
        MessagePriority::Medium,
        MessageId::new(),
        None,
        None,
        ContentType::Image,
        None,
        None,
    );
    alice.flush_pending_messages("bob").unwrap();

    let sent = alice_handle.sent_messages();
    let enc = sent
        .iter()
        .find(|m| m.content.starts_with(internal_prefixes::ENCRYPTED))
        .expect("flushed message must reach the wire");
    assert_eq!(
        enc.content_type,
        ContentType::Image,
        "stored content_type must survive a pending-queue flush"
    );

    // No rich extras and no negotiated capability: the plaintext must be
    // the bare text, not a sealed rich body.
    let result = bob.process_internal_message(enc);
    assert!(
        matches!(result, Some(InternalMessageResult::Decrypted(ref text)) if text == "queued image caption"),
        "content_type-only flush must decrypt to bare text, got {result:?}"
    );
}

/// Durable size/fragment report for encrypted text DMs across the four
/// envelope x wire-codec combinations, using a real OpenMLS ciphertext and
/// realistic identifier lengths (27-char base58 user ids). Prints the table
/// under `--nocapture` and pins the headline win with margin-tolerant
/// assertions so a regression in either lever fails loudly.
///
/// BLE fragment math: usable payload per fragment is
/// `BLE_MAX_FRAGMENT_SIZE - FRAGMENT_HEADER_FIXED - 36` (the header carries
/// the 36-char message id), which reproduces the measured fragment counts
/// from the wire-codec PR (#187): 313B->3, 93B->1, 3986B->29, 1117B->9.
#[test]
fn wire_size_and_fragment_report_for_encrypted_dms() {
    use offline_protocol_transport::constants::{BLE_MAX_FRAGMENT_SIZE, FRAGMENT_HEADER_FIXED};

    let alice_id = "3sK9vT2mQ8xW5nRbY7cJ4dHpZ1e"; // 27 chars, base58-shaped
    let bob_id = "9fLm2wN6qA1sD4gK8jP3xC7vB5t";

    // A real MLS session so the ciphertext has authentic OpenMLS overhead.
    let alice_mgr =
        crate::mls::MlsManager::new(alice_id, Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();
    let bob_mgr =
        crate::mls::MlsManager::new(bob_id, Arc::new(crate::mls::InMemoryStorage::new())).unwrap();
    let bob_kp = bob_mgr.generate_key_package().unwrap();
    alice_mgr
        .import_key_package(bob_id, &bob_kp.key_package_data)
        .unwrap();
    let welcome = alice_mgr.create_session(bob_id).unwrap();
    bob_mgr.join_session(&welcome).unwrap();

    let plaintext = "Hey! Are we still on for tonight at 8?";
    let encrypted = alice_mgr
        .encrypt_for_user(bob_id, plaintext.as_bytes())
        .unwrap();

    let legacy_body = serde_json::to_string(&encrypted).unwrap();
    let compact_body = base64_encode(&encrypted.to_bytes());

    let usable = BLE_MAX_FRAGMENT_SIZE - FRAGMENT_HEADER_FIXED - 36;
    let build = |body: &str| {
        Message::new(
            UserId::new(alice_id).unwrap(),
            UserId::new(bob_id).unwrap(),
            AppId::new("com.example.chat").unwrap(),
            format!("{}{}", internal_prefixes::ENCRYPTED, body),
        )
    };
    let frags = |len: usize| len.div_ceil(usable);

    let legacy_msg = build(&legacy_body);
    let compact_msg = build(&compact_body);
    let legacy_json = legacy_msg.to_bytes().unwrap().len();
    let legacy_bin = legacy_msg.to_wire_v1_bytes().unwrap().len();
    let compact_json = compact_msg.to_bytes().unwrap().len();
    let compact_bin = compact_msg.to_wire_v1_bytes().unwrap().len();

    println!(
        "ciphertext: {}B raw for a {}-char plaintext",
        encrypted.ciphertext.len(),
        plaintext.len()
    );
    println!("envelope x wire        bytes  BLE frags (usable {usable}B/frag)");
    println!(
        "legacy  x JSON      : {legacy_json:>6}  {}",
        frags(legacy_json)
    );
    println!(
        "legacy  x binary v1 : {legacy_bin:>6}  {}",
        frags(legacy_bin)
    );
    println!(
        "compact x JSON      : {compact_json:>6}  {}",
        frags(compact_json)
    );
    println!(
        "compact x binary v1 : {compact_bin:>6}  {}",
        frags(compact_bin)
    );

    // Monotonic: each lever only ever helps.
    assert!(legacy_bin < legacy_json);
    assert!(compact_json < legacy_json);
    assert!(compact_bin < compact_json);
    // Headline: compact+binary at least halves today's best (legacy+binary)
    // and cuts BLE fragments for a short text DM.
    assert!(
        compact_bin * 2 <= legacy_bin,
        "compact+binary ({compact_bin}B) must halve legacy+binary ({legacy_bin}B)"
    );
    assert!(
        frags(compact_bin) < frags(legacy_bin),
        "fragment count must drop: {} vs {}",
        frags(compact_bin),
        frags(legacy_bin)
    );
}

#[test]
fn test_send_media_requires_confirmed_session() {
    let (mut alice, alice_handle) = media_test_protocol("alice");

    let result = alice.send_media("bob", vec![1, 2, 3], "f.bin", ContentType::File, None);

    assert!(matches!(result, Err(Error::SessionNotReady(_))));
    assert!(
        alice_handle.sent_messages().is_empty(),
        "no plaintext chunk may reach the wire without a session"
    );
}

#[test]
fn test_send_media_fails_closed_when_encryption_required_but_uninitialized() {
    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut protocol = OfflineProtocol::new(config).unwrap();
    // MLS deliberately NOT initialized.
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();

    let result = protocol.send_media("bob", vec![1, 2, 3], "f.bin", ContentType::File, None);
    assert!(matches!(result, Err(Error::EncryptFailed(_))));
}

#[test]
fn test_send_media_encrypts_chunks_and_metadata_on_wire() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let file_data = vec![0xAB; 2048]; // single chunk (< 4 KB BLE chunk size)
    alice
        .send_media(
            "bob",
            file_data.clone(),
            "secret-photo.jpg",
            ContentType::Image,
            Some(sample_media_metadata(2048)),
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    assert!(!sent.is_empty());
    for msg in &sent {
        assert_eq!(msg.content_type, ContentType::FileChunk);
        let binary = msg
            .binary_content
            .as_ref()
            .expect("chunk message must carry binary content");
        assert!(
            crate::media_envelope::is_media_envelope(binary),
            "chunk binary content must be an encrypted media envelope"
        );
        assert!(
            !binary.windows(64).any(|w| w == &file_data[..64]),
            "file bytes must not appear on the wire"
        );
        assert!(
            msg.media_metadata.is_none(),
            "media metadata (file name, thumbnail) must not ride the wire in cleartext"
        );
        assert!(
            !msg.metadata
                .contains_key(crate::constants::ORIGINAL_CONTENT_TYPE_KEY),
            "original content type must not ride the wire in cleartext"
        );
        assert!(msg.content.is_empty());
    }
}

#[test]
fn test_media_end_to_end_encrypted_transfer() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let received = capture_file_received(&mut bob);

    // 10 KB over 4 KB BLE chunks = 3 chunks, window size 2 — exercises the
    // ACK-driven window pump across multiple rounds.
    let file_data: Vec<u8> = (0..10_240u32).map(|i| (i % 251) as u8).collect();
    alice
        .send_media(
            "bob",
            file_data.clone(),
            "secret-photo.jpg",
            ContentType::Image,
            Some(sample_media_metadata(file_data.len() as u64)),
        )
        .unwrap();

    let mut rounds = 0;
    while received.lock().unwrap().is_empty() {
        rounds += 1;
        assert!(rounds < 32, "media transfer did not complete");

        // Ferry alice → bob.
        let outbound = alice_handle.sent_messages();
        alice_handle.clear_sent_messages();
        for msg in outbound {
            bob_handle.queue_message(msg);
        }
        while bob.receive_message().is_some() {}

        // Ferry bob's ACKs → alice, then pump the send window.
        let acks = bob_handle.sent_messages();
        bob_handle.clear_sent_messages();
        for msg in acks {
            alice_handle.queue_message(msg);
        }
        while alice.receive_message().is_some() {}
        alice.pump_media_transfers();
    }

    let got = received.lock().unwrap();
    assert_eq!(got.len(), 1);
    let (file_name, sender, content_type, media_metadata, data) = &got[0];
    assert_eq!(file_name, "secret-photo.jpg");
    assert_eq!(sender, "alice");
    assert_eq!(content_type, &ContentType::Image.to_string());
    assert_eq!(data, &file_data);
    let meta = media_metadata.as_ref().expect("metadata must survive E2E");
    assert_eq!(meta.file_name, "secret-photo.jpg");
    assert_eq!(meta.thumbnail_base64.as_deref(), Some("c2VjcmV0LXRodW1i"));
}

/// Rich options for media sends used across the rich-media tests: caption,
/// reply threading, quoted-reply context, and forward attribution.
fn sample_media_rich_options(reply_to: &str) -> MediaSendOptions {
    MediaSendOptions {
        media_metadata: Some(sample_media_metadata(2048)),
        caption: Some("vacation shot".to_string()),
        reply_to_msg: Some(reply_to.to_string()),
        reply_context: Some(offline_protocol_core::ReplyContext {
            sender: UserId::new("bob").unwrap(),
            text: "quoted text".to_string(),
            timestamp: None,
            reply_media_label: None,
            reply_content_type: Some("text".to_string()),
        }),
        forward_info: Some(offline_protocol_core::ForwardInfo {
            original_sender: UserId::new("dave").unwrap(),
            original_message_id: offline_protocol_core::MessageId::new(),
            original_timestamp: offline_protocol_core::Timestamp::now(),
            forward_count: 2,
        }),
        file_id: None,
    }
}

/// Captures full FileReceived events (not the reduced tuple) for rich-field
/// assertions.
fn capture_file_received_events(protocol: &mut OfflineProtocol) -> Arc<Mutex<Vec<Event>>> {
    let store: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let store_clone = store.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::FileReceived { .. }) {
            store_clone.lock().unwrap().push(event.clone());
        }
    });
    store
}

#[test]
fn test_send_media_rich_e2e_seals_extras_and_bumps_chunk0_envelope() {
    use crate::media_envelope::{MEDIA_ENVELOPE_VERSION_MLS_V1, MEDIA_ENVELOPE_VERSION_MLS_V2};

    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    alice.peer_rich_payload.insert("bob".to_string());

    let received = capture_file_received_events(&mut bob);

    // 10 KB over 4 KB BLE chunks = 3 chunks — proves the rich extras (and
    // the v2 envelope) ride chunk 0 only.
    let file_data: Vec<u8> = (0..10_240u32).map(|i| (i % 251) as u8).collect();
    let options = sample_media_rich_options(&offline_protocol_core::MessageId::new().as_str());
    let reply_id = options.reply_to_msg.clone().unwrap();
    alice
        .send_media_with(
            "bob",
            file_data.clone(),
            "rich-photo.jpg",
            ContentType::Image,
            options,
        )
        .unwrap();

    let mut envelope_versions: Vec<u8> = Vec::new();
    let mut rounds = 0;
    while received.lock().unwrap().is_empty() {
        rounds += 1;
        assert!(rounds < 32, "media transfer did not complete");

        let outbound = alice_handle.sent_messages();
        alice_handle.clear_sent_messages();
        for msg in outbound {
            if msg.content_type == ContentType::FileChunk {
                let binary = msg.binary_content.as_ref().expect("chunk carries binary");
                assert!(crate::media_envelope::is_media_envelope(binary));
                envelope_versions.push(binary[2]);
                // The rich extras must never appear in wire cleartext.
                let wire = serde_json::to_string(&msg).unwrap();
                assert!(!wire.contains("vacation shot"));
                assert!(!wire.contains("quoted text"));
                assert!(!wire.contains("dave"));
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

    assert_eq!(envelope_versions.len(), 3);
    assert_eq!(
        envelope_versions
            .iter()
            .filter(|v| **v == MEDIA_ENVELOPE_VERSION_MLS_V2)
            .count(),
        1,
        "exactly chunk 0 ships under the v2 envelope"
    );
    assert!(envelope_versions
        .iter()
        .all(|v| *v == MEDIA_ENVELOPE_VERSION_MLS_V1 || *v == MEDIA_ENVELOPE_VERSION_MLS_V2));

    let got = received.lock().unwrap();
    assert_eq!(got.len(), 1);
    match &got[0] {
        Event::FileReceived {
            file_name,
            sender,
            content_type,
            caption,
            reply_to_msg,
            reply_context,
            forward_info,
            timestamp,
            file_data: file_data_b64,
            ..
        } => {
            assert_eq!(file_name, "rich-photo.jpg");
            assert_eq!(sender, "alice");
            assert_eq!(content_type, &ContentType::Image.to_string());
            assert_eq!(caption.as_deref(), Some("vacation shot"));
            assert_eq!(reply_to_msg.as_deref(), Some(reply_id.as_str()));
            let rc = reply_context.as_ref().expect("reply context must survive");
            assert_eq!(rc.sender, "bob");
            assert_eq!(rc.text, "quoted text");
            let fwd = forward_info.as_ref().expect("forward info must survive");
            assert_eq!(fwd.original_sender, "dave");
            assert_eq!(fwd.forward_count, 2);
            assert!(
                timestamp.is_some(),
                "chunk-0 message timestamp must surface"
            );
            use base64::{engine::general_purpose::STANDARD, Engine};
            assert_eq!(STANDARD.decode(file_data_b64).unwrap(), file_data);
        }
        _ => unreachable!(),
    }
}

#[test]
fn test_send_media_rich_extras_dropped_for_non_capable_peer() {
    use crate::media_envelope::MEDIA_ENVELOPE_VERSION_MLS_V1;

    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    // bob never advertised RICH_PAYLOAD_V1.

    let received = capture_file_received_events(&mut bob);

    alice
        .send_media_with(
            "bob",
            vec![0xAB; 1024],
            "plain-photo.jpg",
            ContentType::Image,
            sample_media_rich_options(&offline_protocol_core::MessageId::new().as_str()),
        )
        .unwrap();

    // The transfer degrades to what plain send_media sends: v1 envelope, no
    // rich extras anywhere on the wire.
    let outbound = alice_handle.sent_messages();
    alice_handle.clear_sent_messages();
    assert!(!outbound.is_empty());
    for msg in &outbound {
        let binary = msg.binary_content.as_ref().unwrap();
        assert_eq!(binary[2], MEDIA_ENVELOPE_VERSION_MLS_V1);
        let wire = serde_json::to_string(msg).unwrap();
        assert!(!wire.contains("vacation shot"));
    }
    for msg in outbound {
        bob_handle.queue_message(msg);
    }
    while bob.receive_message().is_some() {}

    let got = received.lock().unwrap();
    assert_eq!(got.len(), 1);
    match &got[0] {
        Event::FileReceived {
            caption,
            reply_to_msg,
            reply_context,
            forward_info,
            media_metadata,
            ..
        } => {
            assert!(caption.is_none());
            assert!(reply_to_msg.is_none());
            assert!(reply_context.is_none());
            assert!(forward_info.is_none());
            // Non-rich metadata still travels as before.
            assert!(media_metadata.is_some());
        }
        _ => unreachable!(),
    }
}

#[test]
fn test_send_media_rich_extras_dropped_when_kill_switch_off() {
    use crate::media_envelope::MEDIA_ENVELOPE_VERSION_MLS_V1;

    let mut config = create_test_config_for_user("alice");
    config.encryption.enabled = true;
    config.encryption.rich_payload_enabled = false;
    let mut alice = OfflineProtocol::new(config).unwrap();
    alice
        .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let alice_handle = mock_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    alice.start().unwrap();

    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);
    // Even a recorded peer capability must not override the kill switch.
    alice.peer_rich_payload.insert("bob".to_string());

    alice
        .send_media_with(
            "bob",
            vec![0xCD; 512],
            "gated.jpg",
            ContentType::Image,
            sample_media_rich_options(&offline_protocol_core::MessageId::new().as_str()),
        )
        .unwrap();

    for msg in alice_handle.sent_messages() {
        let binary = msg.binary_content.as_ref().unwrap();
        assert_eq!(binary[2], MEDIA_ENVELOPE_VERSION_MLS_V1);
        let wire = serde_json::to_string(&msg).unwrap();
        assert!(!wire.contains("vacation shot"));
    }
}

#[test]
fn test_send_media_rich_extras_never_ride_plaintext_transfer() {
    // Explicit encryption opt-out: the transfer leaves as legacy plaintext
    // chunks and the rich extras are dropped entirely — never cleartext.
    let mut config = create_test_config_for_user("alice");
    config.encryption = crate::EncryptionConfig::disabled();

    let mut alice = OfflineProtocol::new(config).unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let alice_handle = mock_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    alice.start().unwrap();

    alice
        .send_media_with(
            "bob",
            vec![9u8; 512],
            "open.bin",
            ContentType::File,
            sample_media_rich_options(&offline_protocol_core::MessageId::new().as_str()),
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    assert!(!sent.is_empty());
    for msg in &sent {
        let binary = msg.binary_content.as_ref().unwrap();
        assert!(!crate::media_envelope::is_media_envelope(binary));
        let wire = serde_json::to_string(msg).unwrap();
        assert!(!wire.contains("vacation shot"));
        assert!(!wire.contains("quoted text"));
    }
}

#[test]
fn test_send_media_rich_invalid_reply_to_rejected() {
    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let result = alice.send_media_with(
        "bob",
        vec![1, 2, 3],
        "f.bin",
        ContentType::File,
        MediaSendOptions {
            reply_to_msg: Some("not-a-message-id".to_string()),
            ..Default::default()
        },
    );
    assert!(matches!(result, Err(crate::Error::InvalidArgument(_))));
}

#[test]
fn test_send_media_rich_oversized_extras_rejected() {
    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let result = alice.send_media_with(
        "bob",
        vec![1, 2, 3],
        "f.bin",
        ContentType::File,
        MediaSendOptions {
            caption: Some("A".repeat(MAX_RICH_EXTRAS_BYTES + 1)),
            ..Default::default()
        },
    );
    assert!(matches!(result, Err(crate::Error::InvalidArgument(_))));
}

#[test]
fn test_send_media_custom_file_id_honored_and_validated() {
    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let file_id = alice
        .send_media_with(
            "bob",
            vec![1u8; 64],
            "custom.bin",
            ContentType::File,
            MediaSendOptions {
                file_id: Some("file_custom_1".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(file_id, "file_custom_1");

    // Reusing the id while the transfer is active must fail...
    let collision = alice.send_media_with(
        "bob",
        vec![2u8; 64],
        "again.bin",
        ContentType::File,
        MediaSendOptions {
            file_id: Some("file_custom_1".to_string()),
            ..Default::default()
        },
    );
    assert!(matches!(collision, Err(crate::Error::InvalidArgument(_))));

    // ...as must out-of-bounds ids.
    for bad in [String::new(), "x".repeat(4097)] {
        let result = alice.send_media_with(
            "bob",
            vec![3u8; 64],
            "bad.bin",
            ContentType::File,
            MediaSendOptions {
                file_id: Some(bad),
                ..Default::default()
            },
        );
        assert!(matches!(result, Err(crate::Error::InvalidArgument(_))));
    }
}

fn plaintext_media_protocol_with_storage(
    user_id: &str,
    storage: Arc<crate::mls::InMemoryStorage>,
) -> (OfflineProtocol, MockTransport, Arc<Mutex<Vec<Event>>>) {
    let mut config = create_test_config_for_user(user_id);
    config.encryption = crate::EncryptionConfig::disabled();
    let mut protocol = OfflineProtocol::new(config).unwrap();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol
        .enable_message_persistence_for_test(storage)
        .unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();
    (protocol, handle, events)
}

#[test]
fn test_media_descriptor_persisted_and_cleared_on_completion() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let (mut alice, _handle, _events) =
        plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));

    let file_id = alice
        .send_media("bob", vec![7u8; 64], "photo.bin", ContentType::File, None)
        .unwrap();

    assert!(
        storage
            .load(storage_keys::MEDIA_DESCRIPTORS, &file_id)
            .unwrap()
            .is_some(),
        "Descriptor must be persisted while the transfer is in flight"
    );

    // Deliver every chunk: the completed transfer must take its
    // crash-recovery descriptor with it.
    let chunk_ids: Vec<_> = alice
        .outbound_media_chunks
        .iter()
        .filter(|(_, (fid, _))| fid == &file_id)
        .map(|(id, _)| id.clone())
        .collect();
    assert!(!chunk_ids.is_empty());
    for chunk_id in chunk_ids {
        alice.handle_outbound_media_chunk_delivered(&chunk_id);
    }

    assert!(!alice.outbound_media_transfers.contains_key(&file_id));
    assert!(
        storage
            .load(storage_keys::MEDIA_DESCRIPTORS, &file_id)
            .unwrap()
            .is_none(),
        "Completed transfer must delete its descriptor"
    );
}

#[test]
fn test_media_descriptor_cleared_on_abort() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let (mut alice, _handle, _events) =
        plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));

    let file_id = alice
        .send_media("bob", vec![7u8; 64], "photo.bin", ContentType::File, None)
        .unwrap();
    assert!(storage
        .load(storage_keys::MEDIA_DESCRIPTORS, &file_id)
        .unwrap()
        .is_some());

    alice.abort_outbound_media_transfer(&file_id, "test abort");

    assert!(
        storage
            .load(storage_keys::MEDIA_DESCRIPTORS, &file_id)
            .unwrap()
            .is_none(),
        "Aborted transfer must delete its descriptor (MediaSendFailed already settled it)"
    );
}

#[test]
fn test_media_resend_required_after_restart_with_checksum_validation() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let original_bytes = vec![7u8; 256];

    // First life: transfer starts, process dies before completion.
    let file_id = {
        let (mut alice, _handle, _events) =
            plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));
        alice
            .send_media(
                "bob",
                original_bytes.clone(),
                "photo.bin",
                ContentType::File,
                None,
            )
            .unwrap()
    };

    // Second life: restore must surface the interrupted transfer.
    let (mut alice, _handle, events) =
        plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));

    let resend = events
        .lock()
        .unwrap()
        .iter()
        .find_map(|event| match event {
            Event::MediaResendRequired {
                file_id: fid,
                recipient,
                file_name,
                file_size,
            } => Some((
                fid.clone(),
                recipient.clone(),
                file_name.clone(),
                *file_size,
            )),
            _ => None,
        })
        .expect("MediaResendRequired must be emitted on start() after a mid-transfer restart");
    assert_eq!(resend.0, file_id);
    assert_eq!(resend.1, "bob");
    assert_eq!(resend.2, "photo.bin");
    assert_eq!(resend.3, 256);

    // Resending different bytes under the interrupted file_id is rejected.
    let wrong = alice.send_media_with(
        "bob",
        vec![8u8; 256],
        "photo.bin",
        ContentType::File,
        MediaSendOptions {
            file_id: Some(file_id.clone()),
            ..Default::default()
        },
    );
    assert!(
        matches!(wrong, Err(crate::Error::InvalidArgument(_))),
        "Checksum mismatch must reject the resend, got {:?}",
        wrong.map(|_| ())
    );

    // The original bytes pass validation and consume the descriptor.
    let resent = alice
        .send_media_with(
            "bob",
            original_bytes,
            "photo.bin",
            ContentType::File,
            MediaSendOptions {
                file_id: Some(file_id.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(resent, file_id);
    assert!(
        !alice.restored_media_descriptors.contains_key(&file_id),
        "A successful resend must consume the restored descriptor"
    );
    assert!(
        storage
            .load(storage_keys::MEDIA_DESCRIPTORS, &file_id)
            .unwrap()
            .is_some(),
        "The resent transfer persists a fresh descriptor of its own"
    );
}

#[test]
fn test_media_resend_rejects_recipient_mismatch() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let original_bytes = vec![7u8; 256];

    // First life: transfer to bob starts, process dies before completion.
    let file_id = {
        let (mut alice, _handle, _events) =
            plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));
        alice
            .send_media(
                "bob",
                original_bytes.clone(),
                "photo.bin",
                ContentType::File,
                None,
            )
            .unwrap()
    };

    let (mut alice, _handle, _events) =
        plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));

    // Same bytes, wrong recipient: a redirect would consume the descriptor
    // and orphan bob's interrupted transfer forever.
    let redirected = alice.send_media_with(
        "carol",
        original_bytes.clone(),
        "photo.bin",
        ContentType::File,
        MediaSendOptions {
            file_id: Some(file_id.clone()),
            ..Default::default()
        },
    );
    assert!(
        matches!(redirected, Err(crate::Error::InvalidArgument(_))),
        "Recipient mismatch must reject the resend, got {:?}",
        redirected.map(|_| ())
    );
    assert!(
        alice.restored_media_descriptors.contains_key(&file_id),
        "A rejected resend must leave the descriptor parked"
    );

    // The original recipient still passes and consumes the descriptor.
    let resent = alice
        .send_media_with(
            "bob",
            original_bytes,
            "photo.bin",
            ContentType::File,
            MediaSendOptions {
                file_id: Some(file_id.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(resent, file_id);
    assert!(!alice.restored_media_descriptors.contains_key(&file_id));
}

#[test]
fn test_media_descriptor_restore_prunes_expired() {
    let storage = Arc::new(crate::mls::InMemoryStorage::new());
    let (mut alice, _handle, _events) =
        plaintext_media_protocol_with_storage("alice", Arc::clone(&storage));

    // Plant a descriptor older than the outbox lifetime (default 7 days).
    let stale = MediaTransferDescriptor {
        file_id: "file_stale".to_string(),
        recipient: "bob".to_string(),
        file_name: "old.bin".to_string(),
        file_size: 10,
        file_checksum: "00".repeat(32),
        content_type: ContentType::File,
        queued_at: chrono::Utc::now() - ChronoDuration::days(8),
    };
    alice.persist_media_descriptor(&stale);

    alice.restore_media_descriptors().unwrap();

    assert!(
        !alice.restored_media_descriptors.contains_key("file_stale"),
        "Expired descriptor must not be restored"
    );
    assert!(
        storage
            .load(storage_keys::MEDIA_DESCRIPTORS, "file_stale")
            .unwrap()
            .is_none(),
        "Expired descriptor must be deleted from storage"
    );
}

#[test]
fn test_plaintext_media_rejected_once_session_confirmed() {
    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let received = capture_file_received(&mut bob);
    bob_handle.queue_message(legacy_chunk_message("alice", "bob", &[7u8; 128]));
    while bob.receive_message().is_some() {}

    assert!(
        received.lock().unwrap().is_empty(),
        "plaintext media from a session-confirmed peer must be rejected (downgrade/forgery)"
    );
}

#[test]
fn test_plaintext_media_accepted_without_session_when_not_required() {
    // Encryption enabled but no session with the sender and
    // require_encryption=false: legacy plaintext media stays interoperable.
    let (mut bob, bob_handle) = media_test_protocol("bob");

    let received = capture_file_received(&mut bob);
    bob_handle.queue_message(legacy_chunk_message("alice", "bob", &[7u8; 128]));
    while bob.receive_message().is_some() {}

    let got = received.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].4, vec![7u8; 128]);
}

#[test]
fn test_plaintext_media_rejected_when_encryption_required() {
    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut bob = OfflineProtocol::new(config).unwrap();
    bob.initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let bob_handle = mock_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    let received = capture_file_received(&mut bob);
    bob_handle.queue_message(legacy_chunk_message("alice", "bob", &[7u8; 128]));
    while bob.receive_message().is_some() {}

    assert!(
        received.lock().unwrap().is_empty(),
        "plaintext media must be rejected when encryption is required"
    );
}

/// Builds a plaintext (no internal prefix) text message.
fn plaintext_text_message(sender: &str, recipient: &str, content: &str) -> Message {
    Message::new(
        UserId::new(sender).unwrap(),
        UserId::new(recipient).unwrap(),
        AppId::new("test-app").unwrap(),
        content,
    )
}

#[test]
fn test_plaintext_text_rejected_when_encryption_required() {
    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;

    let mut bob = OfflineProtocol::new(config).unwrap();
    bob.initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let bob_handle = mock_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    let received_events: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let warnings: Arc<Mutex<Vec<(String, crate::events::SecurityWarningCode)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let received_clone = received_events.clone();
    let warnings_clone = warnings.clone();
    bob.on_event(move |event| match event {
        Event::MessageReceived { encrypted, .. } => {
            received_clone.lock().unwrap().push(encrypted);
        }
        Event::SecurityWarning {
            peer_id,
            reason_code,
            ..
        } => {
            warnings_clone.lock().unwrap().push((peer_id, reason_code));
        }
        _ => {}
    });

    let mut injected = plaintext_text_message("alice", "bob", "injected cleartext");
    injected.requires_ack = true;
    bob_handle.queue_message(injected);
    assert!(
        bob.receive_message().is_none(),
        "plaintext text must not surface when encryption is required"
    );

    // A second injection from the same peer is also dropped, but the
    // security warning stays once-per-peer.
    bob_handle.queue_message(plaintext_text_message("alice", "bob", "again"));
    assert!(bob.receive_message().is_none());

    assert!(
        received_events.lock().unwrap().is_empty(),
        "no MessageReceived event may fire for rejected plaintext"
    );
    let got_warnings = warnings.lock().unwrap();
    assert_eq!(
        got_warnings.as_slice(),
        &[(
            "alice".to_string(),
            crate::events::SecurityWarningCode::PlaintextReceiveRejected
        )],
        "exactly one PlaintextReceiveRejected warning per peer"
    );
    assert!(
        bob_handle.sent_messages().is_empty(),
        "rejection must not send a delivery ACK (mirrors SecurityRejected)"
    );
}

#[test]
fn test_plaintext_text_rejected_once_session_confirmed() {
    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    bob_handle.queue_message(plaintext_text_message("alice", "bob", "downgrade attempt"));
    assert!(
        bob.receive_message().is_none(),
        "plaintext text from a session-confirmed peer must be rejected (downgrade/forgery)"
    );
}

#[test]
fn test_plaintext_text_accepted_without_session_when_not_required() {
    // Encryption enabled but no session with the sender and
    // require_encryption=false: legacy plaintext text stays interoperable.
    let (mut bob, bob_handle) = media_test_protocol("bob");

    let encrypted_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let flags_clone = encrypted_flags.clone();
    bob.on_event(move |event| {
        if let Event::MessageReceived { encrypted, .. } = event {
            flags_clone.lock().unwrap().push(encrypted);
        }
    });

    bob_handle.queue_message(plaintext_text_message("alice", "bob", "legacy hello"));
    let received = bob
        .receive_message()
        .expect("legacy plaintext interop must be preserved under the opt-out");
    assert_eq!(received.content, "legacy hello");
    assert_eq!(
        *encrypted_flags.lock().unwrap(),
        vec![false],
        "plaintext delivery must surface with encrypted=false"
    );
}

/// Builds a strict-mode (`require_encryption = true`) protocol instance for
/// inbound plaintext gate tests, optionally with MLS initialized — the gate
/// must hold in both states.
fn strict_text_protocol(user_id: &str, init_mls: bool) -> (OfflineProtocol, MockTransport) {
    let mut config = create_test_config_for_user(user_id);
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    if init_mls {
        protocol
            .initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
            .unwrap();
    }
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let handle = mock_transport.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    protocol.start().unwrap();
    (protocol, handle)
}

/// Captures SecurityWarning events as (peer_id, reason_code) pairs.
fn capture_security_warnings(
    protocol: &mut OfflineProtocol,
) -> Arc<Mutex<Vec<(String, SecurityWarningCode)>>> {
    let store: Arc<Mutex<Vec<(String, SecurityWarningCode)>>> = Arc::new(Mutex::new(Vec::new()));
    let store_clone = store.clone();
    protocol.on_event(move |event| {
        if let Event::SecurityWarning {
            peer_id,
            reason_code,
            ..
        } = event
        {
            store_clone.lock().unwrap().push((peer_id, reason_code));
        }
    });
    store
}

#[test]
fn test_plaintext_rejected_warning_fires_per_distinct_peer() {
    let (mut bob, bob_handle) = strict_text_protocol("bob", true);
    let warnings = capture_security_warnings(&mut bob);

    bob_handle.queue_message(plaintext_text_message("alice", "bob", "from alice"));
    bob_handle.queue_message(plaintext_text_message("carol", "bob", "from carol"));
    assert!(bob.receive_message().is_none());

    assert_eq!(
        warnings.lock().unwrap().as_slice(),
        &[
            (
                "alice".to_string(),
                SecurityWarningCode::PlaintextReceiveRejected
            ),
            (
                "carol".to_string(),
                SecurityWarningCode::PlaintextReceiveRejected
            ),
        ],
        "each distinct peer gets its own once-per-peer warning"
    );
}

#[test]
fn test_plaintext_text_rejected_when_mls_uninitialized() {
    // The gate must fail closed on the config alone, before initialize_mls.
    let (mut bob, bob_handle) = strict_text_protocol("bob", false);
    let warnings = capture_security_warnings(&mut bob);

    let mut injected = plaintext_text_message("alice", "bob", "pre-init injection");
    injected.requires_ack = true;
    bob_handle.queue_message(injected);

    assert!(
        bob.receive_message().is_none(),
        "plaintext must be rejected even with MLS uninitialized"
    );
    assert!(
        bob_handle.sent_messages().is_empty(),
        "rejection must not send a delivery ACK"
    );
    assert_eq!(
        warnings.lock().unwrap().as_slice(),
        &[(
            "alice".to_string(),
            SecurityWarningCode::PlaintextReceiveRejected
        )],
    );
}

#[test]
fn test_rejected_plaintext_replay_is_not_reacked() {
    let (mut bob, bob_handle) = strict_text_protocol("bob", true);
    let warnings = capture_security_warnings(&mut bob);

    let mut injected = plaintext_text_message("alice", "bob", "replayed injection");
    injected.requires_ack = true;

    // Exact replay: the same wire message (same id) delivered twice. The
    // first copy is rejected and deliberately forgotten by the deduplicator,
    // so the replay re-enters the policy gate instead of the duplicate
    // re-ACK path — neither copy may produce an ACK (presence leak).
    bob_handle.queue_message(injected.clone());
    assert!(bob.receive_message().is_none());
    bob_handle.queue_message(injected);
    assert!(bob.receive_message().is_none());

    assert!(
        bob_handle.sent_messages().is_empty(),
        "a replayed rejected message must not be re-ACKed as a duplicate"
    );
    assert_eq!(
        warnings.lock().unwrap().len(),
        1,
        "once-per-peer warning throttle holds across replays"
    );
}

#[test]
fn test_plaintext_receive_warned_set_is_bounded() {
    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.encryption.require_encryption = true;
    let mut bob = OfflineProtocol::new(config).unwrap();
    let warnings = capture_security_warnings(&mut bob);

    let total = MAX_PLAINTEXT_RECEIVE_WARNED_PEERS + 10;
    for i in 0..total {
        bob.warn_plaintext_receive_rejected(&format!("forged-{i}"), "test");
    }

    assert_eq!(
        warnings.lock().unwrap().len(),
        total,
        "every distinct forged peer still warns, even past the cap"
    );
    assert!(
        bob.plaintext_receive_warned.len() <= MAX_PLAINTEXT_RECEIVE_WARNED_PEERS,
        "warned-peer tracking must stay bounded under a forged-sender flood"
    );
}

#[test]
fn test_send_media_plaintext_when_encryption_disabled() {
    let mut config = create_test_config_for_user("alice");
    config.encryption = crate::EncryptionConfig::disabled();

    let mut alice = OfflineProtocol::new(config).unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let alice_handle = mock_transport.clone();
    alice
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    alice.start().unwrap();

    alice
        .send_media(
            "bob",
            vec![9u8; 512],
            "open.bin",
            ContentType::File,
            Some(sample_media_metadata(512)),
        )
        .unwrap();

    let sent = alice_handle.sent_messages();
    assert!(!sent.is_empty());
    let chunk0 = &sent[0];
    let binary = chunk0.binary_content.as_ref().unwrap();
    assert!(
        !crate::media_envelope::is_media_envelope(binary),
        "explicitly-disabled encryption keeps the legacy wire format"
    );
    assert!(chunk0.media_metadata.is_some());
    assert!(chunk0
        .metadata
        .contains_key(crate::constants::ORIGINAL_CONTENT_TYPE_KEY));
}

#[test]
fn test_encrypted_media_chunk_queued_until_session_ready() {
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");

    // Alice-side session only: bob has NOT processed the Welcome yet.
    let bob_kp = {
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.generate_key_package().unwrap()
    };
    let welcome = {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap()
    };
    alice.confirm_session_state("bob", "test_setup").unwrap();

    let received = capture_file_received(&mut bob);
    let file_data = vec![5u8; 512];
    alice
        .send_media(
            "bob",
            file_data.clone(),
            "queued.bin",
            ContentType::File,
            None,
        )
        .unwrap();

    for msg in alice_handle.sent_messages() {
        bob_handle.queue_message(msg);
    }
    while bob.receive_message().is_some() {}
    assert!(
        received.lock().unwrap().is_empty(),
        "chunk must not decrypt before the session exists"
    );

    // Bob processes the Welcome; the queued chunk drains and completes.
    {
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.process_welcome(&welcome).unwrap();
    }
    bob.confirm_session_state("alice", "test_setup").unwrap();
    bob.process_pending_decryption("alice");

    let got = received.lock().unwrap();
    assert_eq!(
        got.len(),
        1,
        "queued chunk must complete after session setup"
    );
    assert_eq!(got[0].0, "queued.bin");
    assert_eq!(got[0].4, file_data);
}

#[test]
fn test_encrypted_media_chunk_with_mismatched_group_rejected() {
    use crate::events::SecurityWarningCode;
    use crate::file_transfer::FileChunk;
    use crate::media_envelope::{encode_media_envelope, MediaChunkPlaintext};
    use sha2::{Digest, Sha256};

    // Carol holds a real, confirmed MLS session with bob. A valid ciphertext
    // from that session delivered under wire sender "alice" must be rejected:
    // MLS authenticates group membership, not the wire sender claim.
    let (mut bob, bob_handle) = media_test_protocol("bob");
    let (mut carol, _carol_handle) = media_test_protocol("carol");
    {
        // establish carol <-> bob at the manager level (mirrors
        // establish_media_session, which is hardcoded to alice/bob).
        let bob_kp = {
            let mls = bob.mls_manager.as_ref().unwrap().clone();
            let manager = mls.read().unwrap();
            manager.generate_key_package().unwrap()
        };
        let welcome = {
            let mls = carol.mls_manager.as_ref().unwrap().clone();
            let manager = mls.read().unwrap();
            manager
                .import_key_package("bob", &bob_kp.key_package_data)
                .unwrap();
            manager.create_session("bob").unwrap()
        };
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.process_welcome(&welcome).unwrap();
    }
    carol.confirm_session_state("bob", "test_setup").unwrap();
    bob.confirm_session_state("carol", "test_setup").unwrap();

    let received = capture_file_received(&mut bob);
    let warnings: Arc<Mutex<Vec<(String, SecurityWarningCode)>>> = Arc::new(Mutex::new(Vec::new()));
    let warnings_clone = warnings.clone();
    bob.on_event(move |event| {
        if let Event::SecurityWarning {
            peer_id,
            reason_code,
            ..
        } = event
        {
            warnings_clone
                .lock()
                .unwrap()
                .push((peer_id.clone(), reason_code));
        }
    });

    // A perfectly valid encrypted media chunk for session:bob:carol...
    let data = vec![3u8; 64];
    let chunk = FileChunk {
        file_id: "file_forged1".to_string(),
        file_name: "forged.bin".to_string(),
        file_size: data.len() as u64,
        total_chunks: 1,
        chunk_index: 0,
        chunk_data: data.clone(),
        file_checksum: format!("{:x}", Sha256::digest(&data)),
    };
    let inner = MediaChunkPlaintext {
        chunk_bytes: chunk.to_bytes(),
        media_metadata: None,
        original_content_type: None,
        rich_extras: None,
    };
    let encrypted = {
        let mls = carol.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .encrypt_for_user("bob", &inner.encode().unwrap())
            .unwrap()
    };

    // ...delivered with wire sender "alice".
    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "",
    );
    msg.content_type = ContentType::FileChunk;
    msg.binary_content = Some(encode_media_envelope(
        &encrypted,
        crate::media_envelope::MEDIA_ENVELOPE_VERSION_MLS_V1,
    ));
    bob_handle.queue_message(msg);
    while bob.receive_message().is_some() {}

    assert!(
        received.lock().unwrap().is_empty(),
        "media from a mismatched MLS group must never surface under the claimed sender"
    );
    assert!(
        warnings
            .lock()
            .unwrap()
            .iter()
            .any(|(peer, code)| peer == "alice"
                && *code == SecurityWarningCode::MediaSenderGroupMismatch),
        "a MediaSenderGroupMismatch security warning must be emitted"
    );
}

#[test]
fn test_send_media_caps_concurrent_transfers_per_peer() {
    use crate::constants::MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER;

    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, _bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    // Start the maximum number of transfers; none complete (no ACKs ferried),
    // so they all stay active.
    for i in 0..MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER {
        alice
            .send_media(
                "bob",
                vec![i as u8; 512],
                &format!("f{}.bin", i),
                ContentType::File,
                None,
            )
            .unwrap();
    }

    // One more must be rejected: the combined in-flight windows would exceed
    // the receiver's sender-ratchet out-of-order tolerance budget.
    let result = alice.send_media(
        "bob",
        vec![9u8; 512],
        "overflow.bin",
        ContentType::File,
        None,
    );
    assert!(
        matches!(result, Err(Error::MediaTransferLimit(_))),
        "transfer beyond the per-peer cap must be rejected, got {:?}",
        result.map(|_| ())
    );

    // A different recipient is not affected by bob's cap.
    // (No session with carol -> SessionNotReady, NOT MediaTransferLimit.)
    let other = alice.send_media("carol", vec![1u8; 64], "c.bin", ContentType::File, None);
    assert!(matches!(other, Err(Error::SessionNotReady(_))));
}

#[test]
fn test_dropped_pending_media_chunk_fails_loudly() {
    use crate::events::DecryptionFailureCode;

    // Bob's per-peer pending queue holds only 1 message, and his session with
    // alice is not ready (Welcome never processed). Alice's initial transfer
    // window (2 chunks) already outruns the queue: the second chunk evicts the
    // first under DropOldest. A dropped encrypted media chunk MUST surface a
    // loud, machine-readable PendingQueueDropped failure rather than vanish
    // silently, so the app can react instead of watching the transfer stall.
    //
    // (Under the deferred-ACK model bob no longer ACKs a queued chunk, so the
    // sender's window does not advance on its own — the cap of 1 is what makes
    // the overflow deterministic without relying on ACK-driven pumping.)
    let mut config = create_test_config_for_user("bob");
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 1;
    let mut bob = OfflineProtocol::new(config).unwrap();
    bob.initialize_mls_for_test(Arc::new(crate::mls::InMemoryStorage::new()))
        .unwrap();
    let mock_transport = MockTransport::new(TransportType::BLE);
    mock_transport.start().unwrap();
    let bob_handle = mock_transport.clone();
    bob.transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock_transport));
    bob.start().unwrap();

    let (mut alice, alice_handle) = media_test_protocol("alice");

    // Alice-side session only: bob never processes the Welcome.
    let bob_kp = {
        let mls = bob.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.generate_key_package().unwrap()
    };
    {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager
            .import_key_package("bob", &bob_kp.key_package_data)
            .unwrap();
        manager.create_session("bob").unwrap();
    }
    alice.confirm_session_state("bob", "test_setup").unwrap();

    let dropped_events: Arc<Mutex<Vec<(String, DecryptionFailureCode)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let dropped_clone = dropped_events.clone();
    bob.on_event(move |event| {
        if let Event::MessageDecryptionFailed { sender, code, .. } = event {
            dropped_clone.lock().unwrap().push((sender.clone(), code));
        }
    });

    // 3 chunks over 4 KB BLE chunks (window 2): the initial 2-chunk window
    // alone overflows bob's 1-slot queue, evicting the first chunk.
    let file_data = vec![0x5Au8; 9 * 1024];
    alice
        .send_media("bob", file_data, "big.bin", ContentType::File, None)
        .unwrap();

    let mut rounds = 0;
    while !dropped_events
        .lock()
        .unwrap()
        .iter()
        .any(|(_, code)| *code == DecryptionFailureCode::PendingQueueDropped)
    {
        rounds += 1;
        assert!(
            rounds < 16,
            "pending queue overflow never surfaced a PendingQueueDropped failure"
        );

        // Ferry alice -> bob (bob queues without ACKing; overflow drops the
        // oldest queued chunk and emits PendingQueueDropped).
        let outbound = alice_handle.sent_messages();
        alice_handle.clear_sent_messages();
        for msg in outbound {
            bob_handle.queue_message(msg);
        }
        while bob.receive_message().is_some() {}

        // Ferry bob's ACKs -> alice, then pump the send window.
        let acks = bob_handle.sent_messages();
        bob_handle.clear_sent_messages();
        for msg in acks {
            alice_handle.queue_message(msg);
        }
        while alice.receive_message().is_some() {}
        alice.pump_media_transfers();
    }

    let events = dropped_events.lock().unwrap();
    assert!(
        events.iter().any(|(sender, code)| sender == "alice"
            && *code == DecryptionFailureCode::PendingQueueDropped),
        "the dropped chunk must be attributed to its sender, got {:?}",
        events
    );
}

/// Builds a legacy chunk message using the pre-binary JSON content format.
fn legacy_json_chunk_message(sender: &str, recipient: &str, data: &[u8]) -> Message {
    use crate::file_transfer::FileChunk;
    use sha2::{Digest, Sha256};

    let chunk = FileChunk {
        file_id: "file_legacy_json".to_string(),
        file_name: "legacy-json.bin".to_string(),
        file_size: data.len() as u64,
        total_chunks: 1,
        chunk_index: 0,
        chunk_data: data.to_vec(),
        file_checksum: format!("{:x}", Sha256::digest(data)),
    };

    let mut msg = Message::new(
        UserId::new(sender).unwrap(),
        UserId::new(recipient).unwrap(),
        AppId::new("test-app").unwrap(),
        chunk.to_json().unwrap(),
    );
    msg.content_type = ContentType::FileChunk;
    msg
}

#[test]
fn test_plaintext_json_media_rejected_once_session_confirmed() {
    // The legacy JSON-content chunk path (no binary_content) must be gated by
    // the same downgrade policy as the binary path.
    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let received = capture_file_received(&mut bob);
    bob_handle.queue_message(legacy_json_chunk_message("alice", "bob", &[7u8; 64]));
    while bob.receive_message().is_some() {}

    assert!(
        received.lock().unwrap().is_empty(),
        "JSON-content plaintext media from a session-confirmed peer must be rejected"
    );
}

#[test]
fn test_plaintext_json_media_accepted_without_session_when_not_required() {
    let (mut bob, bob_handle) = media_test_protocol("bob");

    let received = capture_file_received(&mut bob);
    bob_handle.queue_message(legacy_json_chunk_message("alice", "bob", &[7u8; 64]));
    while bob.receive_message().is_some() {}

    let got = received.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].4, vec![7u8; 64]);
}

#[test]
fn test_media_transfer_encryption_failure_aborts_loudly() {
    // A chunk that fails to encrypt mid-transfer (session invalidated
    // concurrently) must abort the whole transfer — emitting MediaSendFailed
    // and freeing the per-peer transfer slot — instead of wedging until the
    // stale sweep.
    let (mut alice, alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let failed: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let failed_clone = failed.clone();
    alice.on_event(move |event| {
        if let Event::MediaSendFailed {
            file_id, recipient, ..
        } = event
        {
            failed_clone
                .lock()
                .unwrap()
                .push((file_id.clone(), recipient.clone()));
        }
    });

    // 9 KB over 4 KB BLE chunks (window 2): chunk 2 is sent by a later pump.
    let file_data = vec![0x77u8; 9 * 1024];
    let file_id = alice
        .send_media("bob", file_data, "wedge.bin", ContentType::File, None)
        .unwrap();

    // Invalidate the session mid-transfer; the next pumped batch cannot encrypt.
    {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.delete_session("bob").unwrap();
    }

    // Ferry the initial window to bob, ferry his ACKs back, then pump.
    let outbound = alice_handle.sent_messages();
    alice_handle.clear_sent_messages();
    for msg in outbound {
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

    {
        let got = failed.lock().unwrap();
        assert_eq!(
            got.len(),
            1,
            "encryption failure must surface exactly one MediaSendFailed, got {:?}",
            got
        );
        assert_eq!(got[0].0, file_id);
        assert_eq!(got[0].1, "bob");
    }
    assert!(
        alice.outbound_media_transfers.is_empty(),
        "aborted transfer must release all tracking state"
    );

    // The freed slot means a retry is gated on the missing session — it must
    // never be rejected with MediaTransferLimit by the dead transfer.
    let retry = alice.send_media("bob", vec![1u8; 64], "retry.bin", ContentType::File, None);
    assert!(
        matches!(retry, Err(Error::SessionNotReady(_))),
        "retry after abort must be gated on the session, got {:?}",
        retry.map(|_| ())
    );
}

#[test]
fn test_send_media_initial_encryption_failure_leaves_no_zombie_transfer() {
    use crate::constants::MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER;

    let (mut alice, alice_handle) = media_test_protocol("alice");

    // Repeated initial-batch encryption failures (stale confirmed cache, no
    // real MLS session) must never accumulate transfers toward the per-peer
    // cap: each failed send aborts and frees its slot.
    for _ in 0..=MAX_CONCURRENT_MEDIA_TRANSFERS_PER_PEER {
        alice.confirmed_sessions.insert("bob".to_string());
        let result = alice.send_media("bob", vec![1u8; 64], "z.bin", ContentType::File, None);
        assert!(
            matches!(result, Err(Error::SessionNotReady(_))),
            "failed initial batch must return SessionNotReady (never MediaTransferLimit), got {:?}",
            result.map(|_| ())
        );
        assert!(
            alice.outbound_media_transfers.is_empty(),
            "failed initial batch must not leave a zombie transfer"
        );
    }
    assert!(
        alice_handle.sent_messages().is_empty(),
        "no chunk may reach the wire when encryption fails"
    );
}

#[test]
fn test_media_chunk_hard_decrypt_failure_emits_decryption_failed_event() {
    use crate::events::DecryptionFailureCode;
    use crate::media_envelope::encode_media_envelope;

    let (mut alice, _alice_handle) = media_test_protocol("alice");
    let (mut bob, bob_handle) = media_test_protocol("bob");
    establish_media_session(&mut alice, &mut bob);

    let failures: Arc<Mutex<Vec<(String, DecryptionFailureCode, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let failures_clone = failures.clone();
    bob.on_event(move |event| {
        if let Event::MessageDecryptionFailed {
            sender,
            code,
            reason,
            ..
        } = event
        {
            failures_clone
                .lock()
                .unwrap()
                .push((sender.clone(), code, reason.clone()));
        }
    });

    // A structurally valid envelope for the correct session group whose MLS
    // ciphertext is garbage: decryption hard-fails (not session-not-ready),
    // and the loss is permanent because the chunk was ACKed on receipt.
    let mut encrypted = {
        let mls = alice.mls_manager.as_ref().unwrap().clone();
        let manager = mls.read().unwrap();
        manager.encrypt_for_user("bob", b"payload").unwrap()
    };
    encrypted.ciphertext = vec![0u8; 24];

    let mut msg = Message::new(
        UserId::new("alice").unwrap(),
        UserId::new("bob").unwrap(),
        AppId::new("test-app").unwrap(),
        "",
    );
    msg.content_type = ContentType::FileChunk;
    msg.binary_content = Some(encode_media_envelope(
        &encrypted,
        crate::media_envelope::MEDIA_ENVELOPE_VERSION_MLS_V1,
    ));
    bob_handle.queue_message(msg);
    while bob.receive_message().is_some() {}

    let got = failures.lock().unwrap();
    assert!(
        got.iter().any(|(sender, code, reason)| sender == "alice"
            && *code != DecryptionFailureCode::PendingQueueDropped
            && reason.contains("file transfer cannot complete")),
        "hard decrypt failure of a media chunk must surface MessageDecryptionFailed, got {:?}",
        got
    );
}

// ============================================================================
// PENDING QUEUE BYTE BUDGETS
// ============================================================================

fn pending_test_message_with_bytes(sender: &str, payload_len: usize) -> Message {
    let mut msg = pending_test_message(sender, "");
    msg.binary_content = Some(vec![0u8; payload_len]);
    msg
}

#[test]
fn test_pending_queue_per_peer_byte_budget_evicts_oldest() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_bytes_per_peer = 1024;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    let m1 = pending_test_message_with_bytes("peer", 400);
    let m2 = pending_test_message_with_bytes("peer", 400);
    let m3 = pending_test_message_with_bytes("peer", 400);

    assert!(protocol
        .pending_queue
        .enqueue(&queue_config, "peer", &m1)
        .is_empty());
    assert!(protocol
        .pending_queue
        .enqueue(&queue_config, "peer", &m2)
        .is_empty());

    // 1200 bytes would exceed the 1024 budget: the oldest is evicted.
    let dropped = protocol.pending_queue.enqueue(&queue_config, "peer", &m3);
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].message.id, m1.id, "oldest entry must be evicted");
    assert_eq!(protocol.pending_queue.peer_queue_len("peer"), 2);
    assert!(protocol.pending_queue.total_bytes() <= 1024);
}

#[test]
fn test_pending_queue_per_peer_byte_budget_drop_newest_policy() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_bytes_per_peer = 1024;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    let m1 = pending_test_message_with_bytes("peer", 600);
    let m2 = pending_test_message_with_bytes("peer", 600);

    assert!(protocol
        .pending_queue
        .enqueue(&queue_config, "peer", &m1)
        .is_empty());
    let dropped = protocol.pending_queue.enqueue(&queue_config, "peer", &m2);
    assert_eq!(dropped.len(), 1);
    assert_eq!(
        dropped[0].message.id, m2.id,
        "DropNewest must reject the incoming message"
    );
    assert_eq!(protocol.pending_queue.peer_queue_len("peer"), 1);
}

#[test]
fn test_pending_queue_global_byte_budget_evicts_oldest_across_peers() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_bytes_global = 1024;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    let m1 = pending_test_message_with_bytes("peer-a", 600);
    let m2 = pending_test_message_with_bytes("peer-b", 600);

    assert!(protocol
        .pending_queue
        .enqueue(&queue_config, "peer-a", &m1)
        .is_empty());
    let dropped = protocol.pending_queue.enqueue(&queue_config, "peer-b", &m2);
    assert_eq!(dropped.len(), 1);
    assert_eq!(
        dropped[0].message.id, m1.id,
        "global budget must evict the globally oldest"
    );
    assert!(!protocol.pending_queue.contains_peer("peer-a"));
    assert_eq!(protocol.pending_queue.peer_queue_len("peer-b"), 1);
    assert!(protocol.pending_queue.total_bytes() <= 1024);
}

#[test]
fn test_pending_queue_oversized_message_dropped_outright() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_bytes_per_peer = 256;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    // A message that can never fit is rejected without evicting anything.
    let small = pending_test_message_with_bytes("peer", 100);
    assert!(protocol
        .pending_queue
        .enqueue(&queue_config, "peer", &small)
        .is_empty());

    let oversized = pending_test_message_with_bytes("peer", 512);
    let dropped = protocol
        .pending_queue
        .enqueue(&queue_config, "peer", &oversized);
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].message.id, oversized.id);
    assert_eq!(
        protocol.pending_queue.peer_queue_len("peer"),
        1,
        "existing entries must not be evicted for a message that cannot fit"
    );
}

#[test]
fn test_pending_queue_byte_accounting_across_drain() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    let m1 = pending_test_message_with_bytes("peer", 300);
    let m2 = pending_test_message_with_bytes("peer", 200);
    protocol.pending_queue.enqueue(&queue_config, "peer", &m1);
    protocol.pending_queue.enqueue(&queue_config, "peer", &m2);
    // Footprint is the full retained size (payload + sender/recipient/app_id +
    // metadata), not payload-only, so compute the expectation from the same
    // accounting the queue uses rather than hardcoding the 300+200 payloads.
    let expected = PendingDecryptionQueue::message_footprint(&m1)
        + PendingDecryptionQueue::message_footprint(&m2);
    assert_eq!(protocol.pending_queue.total_bytes(), expected);
    assert_eq!(
        protocol.pending_queue.metrics().pending_bytes_current,
        expected
    );

    let drained = protocol.pending_queue.drain_for_peer(&queue_config, "peer");
    assert_eq!(drained.len(), 2);
    assert_eq!(protocol.pending_queue.total_bytes(), 0);
}

#[test]
fn test_pending_queue_byte_budget_counts_metadata_not_just_payload() {
    // Regression (remote unauth OOM): a crafted encrypted message with a tiny
    // payload but a large `metadata` map must be charged against the byte
    // budget. Before the fix `message_bytes` counted only content+binary, so
    // such a message slipped past the per-peer/global byte budgets and could
    // drive the queue to multi-GB retention while the accounting reported
    // near-zero. Reachable unauth via the `__MLS_ENC__` (GroupNotFound) path.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_bytes_per_peer = 4096;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    // ~3 KB of metadata behind a 2-byte payload.
    let mut m1 = pending_test_message("peer", "hi");
    m1.metadata.insert("blob".to_string(), "x".repeat(3000));
    assert!(protocol
        .pending_queue
        .enqueue(&queue_config, "peer", &m1)
        .is_empty());
    assert!(
        protocol.pending_queue.total_bytes() >= 3000,
        "metadata bytes must be charged to the byte budget, got {}",
        protocol.pending_queue.total_bytes()
    );

    // A second such message exceeds the 4096 budget, so the oldest is evicted
    // (DropOldest default): memory stays bounded instead of growing per message.
    let mut m2 = pending_test_message("peer", "hi");
    m2.metadata.insert("blob".to_string(), "y".repeat(3000));
    protocol.pending_queue.enqueue(&queue_config, "peer", &m2);
    assert!(protocol.pending_queue.total_bytes() <= 4096);
    assert_eq!(protocol.pending_queue.peer_queue_len("peer"), 1);
}

#[test]
fn test_peer_overflow_hits_pruned_on_drain() {
    // Regression (unbounded leak): `peer_overflow_hits` is keyed by the
    // attacker-controllable wire sender and was never pruned. Draining a peer
    // must drop its overflow-pressure counter so it cannot leak per-sender.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 1;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    let m1 = pending_test_message("peer", "a");
    let m2 = pending_test_message("peer", "b");
    protocol.pending_queue.enqueue(&queue_config, "peer", &m1);
    // Second message trips the per-peer limit → records an overflow hit; with
    // DropNewest the queue keeps m1 (non-empty), so the counter is retained.
    protocol.pending_queue.enqueue(&queue_config, "peer", &m2);
    assert!(protocol.pending_queue.has_overflow_hits("peer"));

    protocol.pending_queue.drain_for_peer(&queue_config, "peer");
    assert!(!protocol.pending_queue.has_overflow_hits("peer"));
    assert_eq!(protocol.pending_queue.overflow_hits_tracked(), 0);
}

#[test]
fn test_peer_overflow_hits_pruned_when_last_entry_evicted() {
    // Same leak, via the other cleanup path: when the peer's last queued entry
    // is removed (here by TTL expiry through `remove_entry_by_sequence`), its
    // overflow-pressure counter must be pruned with the queue.
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.pending_queue.max_pending_per_peer = 1;
    config.encryption.pending_queue.overflow_policy = crate::config::OverflowPolicy::DropNewest;
    config.encryption.pending_queue.pending_ttl_ms = 1000;
    let mut protocol = OfflineProtocol::new(config).unwrap();
    let queue_config = protocol.config.encryption.pending_queue.clone();

    let m1 = pending_test_message("peer", "a");
    let m2 = pending_test_message("peer", "b");
    protocol.pending_queue.enqueue(&queue_config, "peer", &m1);
    protocol.pending_queue.enqueue(&queue_config, "peer", &m2);
    assert!(protocol.pending_queue.has_overflow_hits("peer"));

    // Age the sole remaining entry past its TTL and prune it.
    let past = std::time::Instant::now() - std::time::Duration::from_millis(10_000);
    protocol.pending_queue.set_front_received_at("peer", past);
    protocol
        .pending_queue
        .prune_expired_for_peer(&queue_config, "peer", std::time::Instant::now());
    assert!(!protocol.pending_queue.contains_peer("peer"));
    assert!(!protocol.pending_queue.has_overflow_hits("peer"));
}

#[test]
fn test_group_registration_enqueue_does_not_mark_relay_synced() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    let internet_handle = internet.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let group_info = protocol.create_group("relay-sync-group").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    // The self-addressed registration frame still goes out for a relay (or
    // bridge adapter) that understands the prefix...
    let sent = internet_handle.sent_messages();
    assert!(
        sent.iter().any(|m| m.recipient.as_str() == "user123"
            && m.content
                .starts_with(internal_prefixes::GROUP_RELAY_REGISTER)),
        "expected a __GRP_RELAY_REG__ frame to self"
    );

    // ...but enqueueing proves nothing about relay support: a prefix-unaware
    // relay echoes a self-addressed frame back instead of registering the
    // group. Sync may only be set by the relay's GroupCreated acknowledgment,
    // otherwise send_group_message takes the broadcast path and the messages
    // vanish into the echo.
    assert!(!protocol.group_mesh.relay_synced.contains(&group_id));

    protocol.stop().unwrap();
}

#[test]
fn test_group_send_takes_broadcast_path_only_when_relay_synced() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    let internet_handle = internet.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let group_info = protocol.create_group("relay-bcast-group").unwrap();
    let group_id = group_info.group_id.as_str().to_string();

    // Unsynced: the broadcast frame must not be emitted (a prefix-unaware
    // relay would swallow it). A solo group has no other members, so the
    // per-member fan-out legitimately sends nothing.
    let ids = protocol
        .send_group_message(&group_id, "hello", None, None)
        .unwrap();
    assert!(ids.is_empty());
    assert!(!internet_handle.sent_messages().iter().any(|m| m
        .content
        .starts_with(internal_prefixes::GROUP_RELAY_BROADCAST)));

    // Simulate the relay's GroupCreated acknowledgment: only now may the
    // O(1) broadcast path be taken.
    protocol.group_mesh.relay_synced.insert(group_id.clone());
    let ids = protocol
        .send_group_message(&group_id, "hello again", None, None)
        .unwrap();
    assert_eq!(ids.len(), 1);
    let sent = internet_handle.sent_messages();
    assert!(sent.iter().any(|m| m.recipient.as_str() == "user123"
        && m.content
            .starts_with(internal_prefixes::GROUP_RELAY_BROADCAST)));

    protocol.stop().unwrap();
}

#[test]
fn test_recipient_unreachable_reason_parks_welcome_without_burning_budget() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 1;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
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
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    let welcome_id = lifecycle.welcome_message.id.as_str().to_string();
    assert_eq!(lifecycle.attempt, 1);
    assert_eq!(lifecycle.state, WelcomeDeliveryState::SendAttempted);

    // The internet transport is UP, and attempt == max_retries — a plain
    // carrier-backed failure here would expire the welcome terminally. A
    // recipient_unreachable-tagged reason (the bridge's translation of the
    // relay's DeliveryError) must instead park it: the attempt is refunded
    // and the TTL pushed, so the probes below never age the welcome out or
    // burn its real-delivery budget.
    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(lifecycle.attempt, 0, "speculative attempt must roll back");
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    // Internet-only: the park still schedules an escalating reachability
    // probe. Parking edge-only here would leave the welcome dependent solely
    // on a presence-online answer — the platform's polling cadence — which is
    // the dead end that stalled parked DMs.
    let first_retry = lifecycle
        .next_retry_at
        .expect("peer-unreachable park must schedule a probe on every carrier");
    assert!(first_retry <= Utc::now() + ChronoDuration::seconds(15));
    assert_eq!(lifecycle.unreachable_parks, 1);
    assert!(lifecycle.expires_at > Utc::now() + ChronoDuration::seconds(60));

    protocol.stop().unwrap();
}

#[test]
fn test_carrier_backed_failure_still_burns_welcome_budget() {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 1;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
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
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    let welcome_id = protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .welcome_message
        .id
        .as_str()
        .to_string();

    // Same conditions as the parking test, but an untagged carrier-backed
    // failure: the budget (max_retries = 1, attempt = 1) is exhausted and
    // the welcome must expire terminally — classification must not widen.
    protocol
        .on_transport_send_failed(&welcome_id, Some("socket write failed".to_string()))
        .unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Expired);
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::RetryExhausted)
    );

    protocol.stop().unwrap();
}

fn setup_internet_protocol_with_pending_welcome() -> (OfflineProtocol, MockTransport, String) {
    let mut config = create_test_config();
    config.encryption.enabled = true;
    config.encryption.store_pending = true;
    config.reliability.retry.max_retries = 3;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    let internet_handle = internet.clone();
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
        .send_message("bob", "hello", None::<MessagePriority>, None::<String>)
        .unwrap();
    let welcome_id = protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .welcome_message
        .id
        .as_str()
        .to_string();
    (protocol, internet_handle, welcome_id)
}

#[test]
fn test_peer_presence_offline_parks_failed_welcome() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    // A carrier-backed failure leaves the welcome Failed on the data-plane
    // retry track, budget burned (attempt = 1 of 3).
    protocol
        .on_transport_send_failed(&welcome_id, Some("network blip".to_string()))
        .unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(lifecycle.attempt, 1);

    // The relay says bob is offline: park pending a reachability edge with
    // the truthful reason, without touching the burned-budget counter and
    // without scheduling a timed retry (which would re-send over the healthy
    // socket into another DeliveryError).
    protocol.on_peer_presence("bob", false, Some(1_000));

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(lifecycle.attempt, 1);
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert!(lifecycle.next_retry_at.is_none());
    assert!(lifecycle.expires_at > Utc::now() + ChronoDuration::seconds(60));

    protocol.stop().unwrap();
}

#[test]
fn test_internet_only_welcome_probe_escalates_and_resets_on_edge() {
    // The welcome unreachable park shares the DM ladder: each consecutive
    // verdict doubles the probe interval (15s -> 30s, 600s cap) and every
    // verdict refunds the attempt and pushes the TTL, so probing can never
    // age the welcome out or burn its real-delivery budget.
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.unreachable_parks, 1);
    let first_retry = lifecycle
        .next_retry_at
        .expect("internet-only park must schedule a probe");
    assert!(first_retry <= Utc::now() + ChronoDuration::seconds(15));

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.unreachable_parks, 2);
    let second_retry = lifecycle
        .next_retry_at
        .expect("second park must re-schedule the probe");
    assert!(second_retry > Utc::now() + ChronoDuration::seconds(20));
    assert!(second_retry <= Utc::now() + ChronoDuration::seconds(30));
    assert_eq!(
        lifecycle.attempt, 0,
        "every verdict refunds the speculative attempt"
    );

    // A reachability edge resets the escalation.
    protocol.on_neighbor_discovered("bob");
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.unreachable_parks, 0);

    protocol.stop().unwrap();
}

#[test]
fn test_presence_offline_does_not_cancel_welcome_unreachable_probe() {
    // On an internet-only device a presence-offline answer arrives once per
    // presence rotation for as long as the peer is down. It must not cancel
    // the escalating probe the relay verdict scheduled — doing so would
    // re-strand the welcome edge-only after every single verdict, which is
    // exactly the dead end this branch fixes. (The budget-burning data-plane
    // retry IS still cancelled — see
    // test_peer_presence_offline_parks_failed_welcome.)
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    let scheduled = protocol
        .welcome_lifecycles
        .get("bob")
        .unwrap()
        .next_retry_at
        .expect("verdict must schedule a probe");

    protocol.on_peer_presence("bob", false, Some(1_000));

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.next_retry_at,
        Some(scheduled),
        "a presence-offline answer must leave the unreachable probe intact"
    );
    assert_eq!(lifecycle.unreachable_parks, 1);

    protocol.stop().unwrap();
}

#[test]
fn test_welcome_unreachable_probe_survives_its_own_escalated_interval() {
    // The ladder outgrows WELCOME_LIFECYCLE_TTL_SECS (300s): 480s at six
    // consecutive parks, 600s at the cap. try_send_welcome checks expires_at
    // BEFORE running a due retry, so a park whose TTL does not cover the probe
    // it just scheduled gets expired terminally by that very probe —
    // welcome_send_expired + secure_session_failed, i.e. the self-recovery this
    // branch adds turning into a dead session for any peer offline ~8 minutes.
    // Every rung must keep expires_at strictly past next_retry_at.
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    for parks in 1..=7 {
        protocol
            .on_transport_send_failed(
                &welcome_id,
                Some("recipient_unreachable: Recipient is offline".to_string()),
            )
            .unwrap();
        let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(lifecycle.unreachable_parks, parks);
        let next_retry = lifecycle
            .next_retry_at
            .expect("every park must schedule a probe");
        assert!(
            lifecycle.expires_at > next_retry,
            "park {} scheduled a probe at {} but let the TTL lapse at {}",
            parks,
            next_retry,
            lifecycle.expires_at
        );
    }

    // Past the point where the old fixed 300s TTL would have lapsed first.
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert!(
        lifecycle.next_retry_at.unwrap() > Utc::now() + ChronoDuration::seconds(300),
        "the ladder must have escalated beyond the base TTL for this to bite"
    );

    // Run the probe when it comes due: it must send, not expire.
    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.next_retry_at = Some(Utc::now() - ChronoDuration::seconds(1));
    }
    protocol.process_welcome_retry_queue().unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.state,
        WelcomeDeliveryState::SendAttempted,
        "a due probe must re-send the welcome, not settle it terminally"
    );
    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::WelcomeSendExpired { .. })),
        "probing must never age the welcome out"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::SecureSessionFailed { .. })),
        "probing must never abort the pending session"
    );
    drop(events);

    protocol.stop().unwrap();
}

/// Drives one probe round: the scheduled probe fires and the relay ACCEPTS
/// the frame (its push fallback succeeded), so no `DeliveryError` ever comes
/// back and the probe resolves at the confirm timeout instead. Both steps
/// force `next_retry_at` due rather than sleeping.
fn run_accepted_welcome_probe_round(protocol: &mut OfflineProtocol, peer_id: &str) {
    for _ in 0..2 {
        let record = protocol.welcome_lifecycles.get_mut(peer_id).unwrap();
        record.next_retry_at = Some(Utc::now() - ChronoDuration::seconds(1));
        protocol.process_welcome_retry_queue().unwrap();
    }
}

#[test]
fn test_welcome_probe_confirm_timeout_reparks_instead_of_expiring() {
    // The probe must not destroy the welcome it exists to recover. The relay
    // only answers DeliveryError when the recipient is undeliverable AND its
    // push fallback also failed — so whenever push succeeds the probe earns no
    // verdict at all and resolves at the 10s confirm timeout. Scored as a
    // carrier-backed failure that charges the attempt and arms a plain
    // data-plane retry, and with nothing in that ladder pushing expires_at,
    // the record walks into terminal welcome_send_expired +
    // secure_session_failed within one TTL window — for a peer that is merely
    // offline. Before the probe existed this configuration simply parked quiet
    // and survived, so scoring it that way would trade a stalled welcome for a
    // failed one. It must re-park as another unreachable verdict instead.
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    assert_eq!(
        protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .unreachable_parks,
        1
    );

    // Well past max_retries (3 in this helper), which is what used to settle
    // the record terminally.
    for round in 1..=8 {
        run_accepted_welcome_probe_round(&mut protocol, "bob");

        let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(
            lifecycle.state,
            WelcomeDeliveryState::Failed,
            "round {}: an unanswered probe must land back on the parked ladder",
            round
        );
        assert_eq!(
            lifecycle.last_reason_code,
            Some(crate::events::WelcomeReasonCode::PeerUnreachable),
            "round {}: the confirm timeout is the probe resolving, not a delivery failing",
            round
        );
        assert_eq!(
            lifecycle.attempt, 0,
            "round {}: the probe must refund its speculative attempt, leaving the \
             real-delivery budget intact",
            round
        );
        assert_eq!(
            lifecycle.unreachable_parks,
            round + 1,
            "round {}: each unanswered probe escalates the interval like any other park",
            round
        );
        let next_retry = lifecycle
            .next_retry_at
            .expect("the ladder must stay armed so the welcome keeps self-recovering");
        assert!(
            lifecycle.expires_at > next_retry,
            "round {}: the TTL must still outlive the retry it schedules",
            round
        );
    }

    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::WelcomeSendExpired { .. })),
        "probing a peer that is merely offline must never age the welcome out"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::SecureSessionFailed { .. })),
        "probing must never abort the pending session"
    );
    drop(events);

    protocol.stop().unwrap();
}

#[test]
fn test_wire_confirmed_welcome_probe_reparks_instead_of_going_quiet() {
    // The production internet sequence, which the mesh-driven tests above
    // never exercise: the platform bridge calls internet_confirm_sent on
    // socket-write success — before the relay can possibly answer. Left
    // unguarded, that wire confirm upgrades the in-flight probe to `Sent`
    // and clears next_retry_at; `Sent` sits outside every retry scan and
    // even rearm_welcome_for_peer no-ops on it, so when the relay then
    // ACCEPTS the frame (push fallback succeeded — no DeliveryError ever
    // comes back) the welcome parks silently with no timer: the exact
    // edge-only dead end this branch exists to close, re-opened through the
    // confirm path. A wire confirm during a live probe round must leave the
    // record SendAttempted with its confirm deadline armed, so the timeout
    // can re-park it — and must not emit a welcome_send_succeeded the next
    // verdict or timeout would only have to correct.
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    assert_eq!(
        protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .unreachable_parks,
        1
    );

    // The probe comes due and fires.
    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.next_retry_at = Some(Utc::now() - ChronoDuration::seconds(1));
    }
    protocol.process_welcome_retry_queue().unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );

    // The bridge wire-confirms the socket write.
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.state,
        WelcomeDeliveryState::SendAttempted,
        "a wire confirm must not upgrade a live probe to Sent"
    );
    assert!(
        lifecycle.next_retry_at.is_some(),
        "the confirm deadline must stay armed so the timeout can resolve the probe"
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::WelcomeSendSucceeded { .. })),
        "a probe's socket write proves nothing; success must not be reported"
    );

    // The relay accepted the frame, so no verdict ever arrives; the confirm
    // deadline lapses instead.
    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.next_retry_at = Some(Utc::now() - ChronoDuration::seconds(1));
    }
    protocol.process_welcome_retry_queue().unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.state,
        WelcomeDeliveryState::Failed,
        "the unanswered probe must land back on the parked ladder"
    );
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert_eq!(lifecycle.unreachable_parks, 2, "the re-park escalates");
    assert_eq!(lifecycle.attempt, 0, "the probe refunds its attempt");
    let next_retry = lifecycle
        .next_retry_at
        .expect("the ladder must stay armed past the wire confirm");
    assert!(lifecycle.expires_at > next_retry);

    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::WelcomeSendExpired { .. })),
        "a wire-confirmed probe must never age the welcome out"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::SecureSessionFailed { .. })),
        "a wire-confirmed probe must never abort the pending session"
    );
    drop(events);

    protocol.stop().unwrap();
}

#[test]
fn test_welcome_probe_repark_stops_shielding_once_the_record_ages_out() {
    // The re-park above must not make a welcome immortal: a peer that never
    // returns would otherwise keep one probing at the 600s cap forever, since
    // only rearm_welcome_for_peer clears the counter. The bound is
    // WELCOME_WATCHLIST_MAX_AGE_SECS from created_at — the same threshold that
    // gives up watching a peer as permanently dead — past which the probe
    // stops shielding and normal ageing settles the record. This is the
    // welcome's twin of the DM probe's absolute outbox bound.
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();

    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.created_at =
            Utc::now() - ChronoDuration::seconds(super::types::WELCOME_WATCHLIST_MAX_AGE_SECS + 1);
    }

    run_accepted_welcome_probe_round(&mut protocol, "bob");

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::Timeout),
        "past the watchlist age the confirm timeout is scored carrier-backed again"
    );
    assert_eq!(
        lifecycle.attempt, 1,
        "and it charges the attempt, so the record can finally age out"
    );

    protocol.stop().unwrap();
}

#[test]
fn test_presence_offline_still_parks_carrier_backed_retry_after_a_verdict() {
    // The probe guard keys on the reason code, not on the sticky park counter.
    // unreachable_parks is cleared only by rearm_welcome_for_peer, so a
    // counter-only guard would shield the record for its whole life after one
    // verdict — including the case here. Past WELCOME_WATCHLIST_MAX_AGE_SECS a
    // confirm timeout stops re-parking (see the test above) and once again
    // charges an attempt and arms a plain data-plane retry, with the counter
    // still live. That retry is exactly what this park exists to cancel; left
    // running it burns the welcome's budget to terminal expiry.
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    // One verdict arms the probe and the sticky counter.
    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    assert_eq!(
        protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .unreachable_parks,
        1
    );

    // Age it past the point where the probe track shields it, then let a probe
    // fire and be accepted by the relay so no verdict comes back.
    {
        let record = protocol.welcome_lifecycles.get_mut("bob").unwrap();
        record.created_at =
            Utc::now() - ChronoDuration::seconds(super::types::WELCOME_WATCHLIST_MAX_AGE_SECS + 1);
    }
    run_accepted_welcome_probe_round(&mut protocol, "bob");

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::Timeout),
        "the confirm timeout is a carrier-backed failure, not a relay verdict"
    );
    assert_eq!(lifecycle.attempt, 1, "and it charges the attempt");
    assert!(lifecycle.next_retry_at.is_some());
    assert_eq!(
        lifecycle.unreachable_parks, 1,
        "the park counter is still live — a counter-only guard would stop here"
    );

    // The presence-offline answer must park THIS retry.
    protocol.on_peer_presence("bob", false, Some(1_000));

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert!(
        lifecycle.next_retry_at.is_none(),
        "a carrier-backed data-plane retry must still be cancelled by the park"
    );
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert_eq!(
        lifecycle.attempt, 1,
        "parking never touches the burnt budget"
    );

    protocol.stop().unwrap();
}

#[test]
fn test_peer_presence_online_resends_unconfirmed_sent_welcome() {
    let (mut protocol, internet_handle, welcome_id) =
        setup_internet_protocol_with_pending_welcome();

    // Wire-confirm marks the welcome Sent — but over a store-less relay the
    // content may have been dropped (recipient offline, push-only delivery).
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );
    let welcomes_before = internet_handle
        .sent_messages()
        .iter()
        .filter(|m| {
            m.recipient.as_str() == "bob" && m.content.starts_with(internal_prefixes::WELCOME)
        })
        .count();

    // The peer is provably online and the session was never proven: rebuild
    // and re-send (receiver dedups by message id if the original landed).
    protocol.on_peer_presence("bob", true, None);

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::SendAttempted);
    assert_eq!(lifecycle.attempt, 1);
    let welcomes_after = internet_handle
        .sent_messages()
        .iter()
        .filter(|m| {
            m.recipient.as_str() == "bob" && m.content.starts_with(internal_prefixes::WELCOME)
        })
        .count();
    assert_eq!(welcomes_after, welcomes_before + 1);

    protocol.stop().unwrap();
}

#[test]
fn test_peer_presence_emits_unified_event_with_last_seen() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol.on_peer_presence("carol", false, Some(98_765));

    let captured = events.lock().unwrap();
    let presence = captured
        .iter()
        .find_map(|e| match e {
            Event::PresenceUpdated {
                peer_id,
                status,
                last_seen_ms,
                source,
                ..
            } => Some((peer_id.clone(), *status, *last_seen_ms, *source)),
            _ => None,
        })
        .expect("presence_updated event must be emitted");
    assert_eq!(presence.0, "carol");
    assert_eq!(presence.1, PresenceStatus::Offline);
    assert_eq!(presence.2, Some(98_765));
    assert_eq!(presence.3, PresenceSource::Internet);

    // Self and empty peer ids are ignored entirely.
    drop(captured);
    events.lock().unwrap().clear();
    protocol.on_peer_presence("user123", true, None);
    protocol.on_peer_presence("", true, None);
    assert!(events.lock().unwrap().is_empty());
}

/// Contract: every relay presence answer produces a `presence_updated`
/// event, even when nothing changed since the previous answer. Apps drive
/// chat-header refreshes off this (manual `CheckPresence` on chat open must
/// always yield a fresh event); a changed-status dedup here would silently
/// break them.
#[test]
fn test_peer_presence_unchanged_status_reemits_event() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    protocol.on_peer_presence("dave", false, Some(11_111));
    protocol.on_peer_presence("dave", false, Some(11_111));
    protocol.on_peer_presence("dave", true, None);
    protocol.on_peer_presence("dave", true, None);

    let captured = events.lock().unwrap();
    let presence_events: Vec<(PresenceStatus, Option<i64>)> = captured
        .iter()
        .filter_map(|e| match e {
            Event::PresenceUpdated {
                peer_id,
                status,
                last_seen_ms,
                source,
                ..
            } if peer_id == "dave" => {
                assert_eq!(*source, PresenceSource::Internet);
                Some((*status, *last_seen_ms))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        presence_events,
        vec![
            (PresenceStatus::Offline, Some(11_111)),
            (PresenceStatus::Offline, Some(11_111)),
            (PresenceStatus::Online, None),
            (PresenceStatus::Online, None),
        ],
        "identical repeated answers must each re-emit presence_updated"
    );
}

#[test]
fn test_welcome_pending_peers_tracks_unconfirmed_sessions() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    assert_eq!(protocol.welcome_pending_peers(), vec!["bob".to_string()]);

    // Still pending after wire-confirm: Sent is not session-proven, and only
    // presence can rescue a false Sent over a store-less relay.
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    assert_eq!(protocol.welcome_pending_peers(), vec!["bob".to_string()]);

    // The peer proving the session ends the watch.
    protocol
        .confirm_session_state("bob", "confirmation_ack_received")
        .unwrap();
    assert!(protocol.welcome_pending_peers().is_empty());

    protocol.stop().unwrap();
}

#[test]
fn test_internet_control_op_classification() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let make_from = |sender: &str, recipient: &str, content: &str| {
        Message::new(
            UserId::new(sender).unwrap(),
            UserId::new(recipient).unwrap(),
            AppId::new("test-app").unwrap(),
            content,
        )
    };
    let make = |recipient: &str, content: &str| make_from("user123", recipient, content);

    // Connection ops ship verbatim so the Ed25519 control signature in the
    // message metadata survives to the receiver's security gate — a
    // relay-native replacement would arrive unsigned and be rejected as a
    // signature downgrade once the sender's key is TOFU-pinned.
    for content in [
        "__CONN_REQ__{\"sender_name\":\"Alice\"}",
        "__CONN_ACC__{\"accepted_by_name\":\"Alice\"}",
        "__CONN_REJ__",
        "__CONN_CAN__",
    ] {
        let msg = make("bob", content);
        assert_eq!(
            protocol.internet_control_op(&msg),
            None,
            "connection op {:?} must stay a verbatim SendMessage",
            content
        );
    }

    // Leave is a tap op: classified for the per-member notification.
    let msg = make("bob", "__GRP_MLS_LEAVE__{\"group_id\":\"g1\"}");
    assert_eq!(
        protocol.internet_control_op(&msg),
        Some(("group_mls_leave", "{\"group_id\":\"g1\"}".to_string()))
    );

    // A relayed third-party frame (mesh forwarding puts other users'
    // messages in our internet outbox verbatim) must NOT classify: a
    // relay-native replacement would be issued on OUR authenticated
    // connection and misattribute the op to us. It stays an opaque
    // SendMessage.
    for content in [
        "__CONN_REQ__{\"sender_name\":\"Bea\"}",
        "__CONN_ACC__{\"accepted_by_name\":\"Bea\"}",
        "__CONN_REJ__",
        "__CONN_CAN__",
        "__GRP_MLS_LEAVE__{\"group_id\":\"g1\"}",
    ] {
        let msg = make_from("bea", "carol", content);
        assert_eq!(
            protocol.internet_control_op(&msg),
            None,
            "relayed third-party frame {:?} must not classify",
            content
        );
    }

    // Relay hints classify only when self-addressed...
    let msg = make("user123", "__GRP_RELAY_REG__{\"group_id\":\"g1\"}");
    assert_eq!(
        protocol.internet_control_op(&msg),
        Some(("group_relay_register", "{\"group_id\":\"g1\"}".to_string()))
    );
    let msg = make("user123", "__GRP_RELAY_BCAST__{\"group_id\":\"g1\"}");
    assert_eq!(
        protocol.internet_control_op(&msg),
        Some(("group_relay_broadcast", "{\"group_id\":\"g1\"}".to_string()))
    );
    // ...never for another recipient (a peer-addressed frame with a relay
    // prefix is not a relay hint).
    let msg = make("bob", "__GRP_RELAY_BCAST__{\"group_id\":\"g1\"}");
    assert_eq!(protocol.internet_control_op(&msg), None);
    // ...and never for a third-party frame addressed to us: only frames this
    // device both originated and self-addressed are relay hints.
    let msg = make_from("bea", "user123", "__GRP_RELAY_REG__{\"group_id\":\"g1\"}");
    assert_eq!(protocol.internet_control_op(&msg), None);
    let msg = make_from("bea", "user123", "__GRP_RELAY_BCAST__{\"group_id\":\"g1\"}");
    assert_eq!(protocol.internet_control_op(&msg), None);

    // Normal traffic — including MLS/e2e frames — is never classified.
    for content in [
        "hello",
        "__MLS_WELCOME__abc",
        "__MLS_ENC__abc",
        "__TYPING__{}",
        "__READ_RECEIPT__{}",
        "__PRESENCE__{}",
        "__GRP_MLS_MSG__{}",
        "__GRP_MLS_WELCOME__{}",
        "__GRP_MLS_COMMIT__{}",
    ] {
        let msg = make("bob", content);
        assert_eq!(
            protocol.internet_control_op(&msg),
            None,
            "content {:?} must not classify",
            content
        );
    }
}

/// Pins the closed set of server-plane control ops `internet_control_op`
/// can emit, by sweeping every internal prefix through both classifying
/// shapes (self-originated peer-addressed, and self-originated
/// self-addressed for relay hints).
///
/// **If this test fails because you added an op:** the platform bridges
/// translate ops by name, and an op they don't recognize degrades to an
/// opaque `SendMessage` the transport merely echoes/forwards. Before
/// extending this list, update BOTH translators —
/// `bindings/react-native/android/.../RelayControlOpTranslator.kt` and
/// `bindings/react-native/ios/RelayControlOpTranslator.swift`. This test is
/// the canonical registry of the closed op set — keep it authoritative.
#[test]
fn test_internet_control_op_registry_is_closed() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();
    let mut ops = std::collections::BTreeSet::new();

    for prefix in INTERNAL_PREFIXES {
        let content = format!("{}{{}}", prefix);
        for recipient in ["bob", "user123"] {
            let msg = Message::new(
                UserId::new("user123").unwrap(),
                UserId::new(recipient).unwrap(),
                AppId::new("test-app").unwrap(),
                content.as_str(),
            );
            if let Some((op, _)) = protocol.internet_control_op(&msg) {
                ops.insert(op);
            }
        }
    }

    let expected: std::collections::BTreeSet<&str> = [
        "group_mls_leave",
        "group_relay_register",
        "group_relay_broadcast",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        ops, expected,
        "internet_control_op emitted an op outside the pinned registry — \
         update both bridge translators and the spec table before extending it"
    );
}

#[test]
fn test_relayed_frame_with_hops_never_classifies() {
    let protocol = OfflineProtocol::new(create_test_config()).unwrap();

    // A mesh peer can forge `sender == self` on a frame we then relay —
    // `sender` is an unauthenticated wire field. The relay path increments
    // the hop before re-sending, while every locally-originated frame
    // leaves send_internal_message at hop 0, so a nonzero hop count must
    // veto classification no matter what the sender field claims.
    for (recipient, content) in [
        ("carol", "__GRP_MLS_LEAVE__{\"group_id\":\"g1\"}"),
        ("user123", "__GRP_RELAY_REG__{\"group_id\":\"g1\"}"),
        ("user123", "__GRP_RELAY_BCAST__{\"group_id\":\"g1\"}"),
    ] {
        let mut msg = Message::new(
            UserId::new("user123").unwrap(),
            UserId::new(recipient).unwrap(),
            AppId::new("test-app").unwrap(),
            content,
        );
        assert!(
            protocol.internet_control_op(&msg).is_some(),
            "sanity: {:?} classifies at hop 0",
            content
        );
        msg.increment_hop().unwrap();
        assert_eq!(
            protocol.internet_control_op(&msg),
            None,
            "forged self-frame {:?} with hops must not classify",
            content
        );
    }
}

#[test]
fn test_inbound_frame_forging_our_origin_is_not_relayed() {
    let mut protocol = OfflineProtocol::new(create_test_config()).unwrap();

    let relay_events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let relay_events_clone = relay_events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::MessageRelayed { .. }) {
            relay_events_clone.lock().unwrap().push(event);
        }
    });

    let mock = MockTransport::new(TransportType::BLE);
    mock.start().unwrap();
    let mock_handle = mock.clone();

    // A frame claiming OUR origin, addressed to a third party, arriving
    // inbound: a genuine self-originated frame is never received this way
    // (the send path does not loop back), so it is a routing loop or a
    // forgery aimed at internet_control_op's self-origination gate.
    // Relaying it would put a sender==self frame in our own outbox, where
    // the bridge would execute it as a relay-native op on our
    // authenticated connection.
    let msg = Message::new(
        UserId::new("user123").unwrap(),
        UserId::new("carol").unwrap(),
        AppId::new("test-app").unwrap(),
        "__GRP_MLS_LEAVE__{\"group_id\":\"g1\"}",
    );
    mock.queue_message(msg);

    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(mock));
    protocol.start().unwrap();

    let received = protocol.receive_message();
    assert!(received.is_none(), "forged-origin frame must not surface");
    assert!(
        relay_events.lock().unwrap().is_empty(),
        "forged-origin frame must not be relayed"
    );
    assert!(
        mock_handle.sent_messages().is_empty(),
        "forged-origin frame must not be re-sent"
    );

    protocol.stop().unwrap();
}

#[test]
fn test_group_created_ack_gates_relay_sync() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let group_info = protocol.create_group("ack-gate-group").unwrap();
    let group_id = group_info.group_id.as_str().to_string();
    assert!(!protocol.group_mesh.relay_synced.contains(&group_id));

    // A __GROUP_CREATED__ arriving over a mesh transport (or an unknown
    // path) is spoofable by any peer and must NOT enable the broadcast
    // path — a false sync flag black-holes group sends into a relay that
    // never registered the group.
    let ack = Message::new(
        UserId::new("relay").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "__GROUP_CREATED__{{\"group_id\":\"{}\",\"name\":\"ack-gate-group\"}}",
            group_id
        ),
    );
    protocol.process_internal_message_via(&ack, Some(TransportType::BLE));
    assert!(
        !protocol.group_mesh.relay_synced.contains(&group_id),
        "mesh-arrived GroupCreated must not mark the group relay-synced"
    );
    protocol.process_internal_message(&ack);
    assert!(
        !protocol.group_mesh.relay_synced.contains(&group_id),
        "unknown-transport GroupCreated must not mark the group relay-synced"
    );

    // The relay's GroupCreated answer (bridged as __GROUP_CREATED__ over
    // the internet path) is the registration acknowledgment: create_group
    // armed the pending-registration correlation when it enqueued the
    // __GRP_RELAY_REG__ frame, so only now is broadcast fan-out trusted —
    // and the correlation is consumed by the ack.
    assert!(protocol
        .group_mesh
        .relay_register_pending
        .contains_key(&group_id));
    protocol.process_internal_message_via(&ack, Some(TransportType::Internet));
    assert!(protocol.group_mesh.relay_synced.contains(&group_id));
    assert!(!protocol
        .group_mesh
        .relay_register_pending
        .contains_key(&group_id));

    // An ack for a group we don't track locally must not create sync state.
    let foreign_ack = Message::new(
        UserId::new("relay").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        "__GROUP_CREATED__{\"group_id\":\"someone-elses-group\",\"name\":\"x\"}",
    );
    protocol.process_internal_message_via(&foreign_ack, Some(TransportType::Internet));
    assert!(!protocol
        .group_mesh
        .relay_synced
        .contains("someone-elses-group"));

    // A group-scoped relay error revokes the sync AND the pending
    // correlation: fall back to per-member.
    let error = Message::new(
        UserId::new("relay").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "__GROUP_ERROR__{{\"reason\":\"Only admins can sync this group\",\"group_id\":\"{}\"}}",
            group_id
        ),
    );
    protocol.process_internal_message(&error);
    assert!(!protocol.group_mesh.relay_synced.contains(&group_id));

    // The relay forwards peer message content verbatim, so a malicious peer
    // can deliver a crafted __GROUP_CREATED__ over the *internet* path too.
    // With no registration outstanding (the GroupError above consumed it),
    // the forged ack must not resurrect the sync flag — otherwise one forged
    // frame per revocation permanently black-holes group sends.
    protocol.process_internal_message_via(&ack, Some(TransportType::Internet));
    assert!(
        !protocol.group_mesh.relay_synced.contains(&group_id),
        "internet-arrived GroupCreated with no registration outstanding must not mark the group relay-synced"
    );

    // A genuine re-registration re-arms the correlation, after which the
    // relay's answer is accepted again.
    protocol.group_mesh.relay_register_pending.insert(
        group_id.clone(),
        crate::group_mesh::RelayRegisterPending {
            armed_at: chrono::Utc::now(),
            attempts: 1,
        },
    );
    protocol.process_internal_message_via(&ack, Some(TransportType::Internet));
    assert!(protocol.group_mesh.relay_synced.contains(&group_id));

    protocol.stop().unwrap();
}

#[test]
fn test_unanswered_relay_registration_expires_and_retries() {
    let mut config = create_test_config();
    config.encryption.enabled = true;

    let storage = Arc::new(InMemoryStorage::new());
    let mut protocol = OfflineProtocol::new(config).unwrap();
    protocol.initialize_mls_for_test(storage).unwrap();

    let internet = MockTransport::new(TransportType::Internet);
    internet.start().unwrap();
    let internet_handle = internet.clone();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Internet, Box::new(internet));
    protocol.start().unwrap();

    let group_info = protocol.create_group("ack-timeout-group").unwrap();
    let group_id = group_info.group_id.as_str().to_string();
    let register_frames = |handle: &MockTransport| {
        handle
            .sent_messages()
            .iter()
            .filter(|m| {
                m.content
                    .starts_with(internal_prefixes::GROUP_RELAY_REGISTER)
            })
            .count()
    };
    assert_eq!(
        protocol
            .group_mesh
            .relay_register_pending
            .get(&group_id)
            .map(|p| p.attempts),
        Some(1)
    );
    let initial_frames = register_frames(&internet_handle);

    // Not yet due — the correlation is left armed untouched.
    protocol.process_relay_register_retries();
    assert_eq!(register_frames(&internet_handle), initial_frames);
    assert_eq!(
        protocol
            .group_mesh
            .relay_register_pending
            .get(&group_id)
            .map(|p| p.attempts),
        Some(1)
    );

    // Past the ack deadline the registration is re-sent (the frame may
    // have been lost) and the correlation re-armed with the attempt count
    // carried forward.
    protocol
        .group_mesh
        .relay_register_pending
        .get_mut(&group_id)
        .unwrap()
        .armed_at = Utc::now() - ChronoDuration::seconds(31);
    protocol.process_relay_register_retries();
    assert_eq!(register_frames(&internet_handle), initial_frames + 1);
    assert_eq!(
        protocol
            .group_mesh
            .relay_register_pending
            .get(&group_id)
            .map(|p| p.attempts),
        Some(2)
    );

    // Past the attempt budget the correlation closes for good: against a
    // relay that never answers (prefix-unaware echo relay, legacy server)
    // an armed entry would otherwise sit indefinitely for a forged ack to
    // claim. The group just stays unsynced — per-member fan-out.
    {
        let pending = protocol
            .group_mesh
            .relay_register_pending
            .get_mut(&group_id)
            .unwrap();
        pending.armed_at = Utc::now() - ChronoDuration::seconds(31);
        pending.attempts = 3;
    }
    protocol.process_relay_register_retries();
    assert!(!protocol
        .group_mesh
        .relay_register_pending
        .contains_key(&group_id));
    assert_eq!(
        register_frames(&internet_handle),
        initial_frames + 1,
        "an exhausted registration must not be re-sent"
    );

    let forged_ack = Message::new(
        UserId::new("relay").unwrap(),
        UserId::new("user123").unwrap(),
        AppId::new("test-app").unwrap(),
        &format!(
            "__GROUP_CREATED__{{\"group_id\":\"{}\",\"name\":\"ack-timeout-group\"}}",
            group_id
        ),
    );
    protocol.process_internal_message_via(&forged_ack, Some(TransportType::Internet));
    assert!(
        !protocol.group_mesh.relay_synced.contains(&group_id),
        "a forged ack after the window closes must not sync the group"
    );

    protocol.stop().unwrap();
}

#[test]
fn test_delivery_error_after_wire_confirm_corrects_false_sent() {
    let (mut protocol, internet_handle, welcome_id) =
        setup_internet_protocol_with_pending_welcome();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    // The bridge wire-confirms on socket-write success, BEFORE the relay can
    // answer — so when its DeliveryError arrives the record is already Sent.
    // This is the production ordering; the failure must not no-op on it.
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent
    );

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();

    // The false Sent is corrected: parked Failed, attempt refunded, and an
    // escalating probe scheduled (not due for 15s — see below).
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(lifecycle.attempt, 0, "the peer never saw the frame");
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert!(lifecycle.next_retry_at.is_some());

    // The app's earlier welcome_send_succeeded is superseded by a retryable
    // welcome_send_failed so its UI reflects the truth.
    let correction = events.lock().unwrap().iter().any(|e| {
        matches!(
            e,
            Event::WelcomeSendFailed {
                reason_code: crate::events::WelcomeReasonCode::PeerUnreachable,
                retryable: true,
                ..
            }
        )
    });
    assert!(
        correction,
        "expected a corrective welcome_send_failed event"
    );

    // The probe is scheduled, not immediate: a retry-queue tick right after
    // the verdict must not re-send over the healthy socket. The escalating
    // interval is what keeps the resend rate bounded while bob stays offline.
    let welcomes_before = internet_handle
        .sent_messages()
        .iter()
        .filter(|m| m.content.starts_with(internal_prefixes::WELCOME))
        .count();
    protocol.process_welcome_retry_queue().unwrap();
    let welcomes_after = internet_handle
        .sent_messages()
        .iter()
        .filter(|m| m.content.starts_with(internal_prefixes::WELCOME))
        .count();
    assert_eq!(welcomes_after, welcomes_before);

    // The presence-online edge is what re-arms it.
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );

    protocol.stop().unwrap();
}

#[test]
fn test_late_wire_confirm_does_not_resurrect_unreachable_verdict() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let events_handle = Arc::clone(&events);
    protocol.on_event(move |event| {
        events_handle.lock().unwrap().push(event);
    });

    // The reverse race of the false-Sent correction: the relay's
    // DeliveryError beats the bridge's socket-write confirm, so the
    // unreachable verdict lands while the record is still SendAttempted.
    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert!(lifecycle.next_retry_at.is_some());

    // The late confirm belongs to the very send the relay already failed.
    // It must not resurrect the false Sent (Failed -> Sent is otherwise
    // legal) nor emit a welcome_send_succeeded contradicting the corrective
    // welcome_send_failed the app already saw.
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::WelcomeSendSucceeded { .. })),
        "a stale confirm must not report success for a relay-failed send"
    );

    // The peer stays rescuable: the presence-online edge re-arms as usual.
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );

    protocol.stop().unwrap();
}

#[test]
fn test_delivery_error_with_mesh_carrier_keeps_timed_retry() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    // A live mesh carrier means the peer may still be reachable over BLE
    // even though the relay reports it offline on the internet path (DORS
    // can pick internet with mesh present).
    let ble = MockTransport::new(TransportType::BLE);
    ble.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));

    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();

    // The false Sent is still corrected and the attempt refunded, but the
    // record keeps a timed retry instead of parking edge-only: relay
    // presence is only authoritative for the internet path.
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(lifecycle.attempt, 0, "the peer never saw the frame");
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert!(
        lifecycle.next_retry_at.is_some(),
        "a live mesh carrier must keep the timed retry track alive"
    );
    assert!(lifecycle.last_transport_error.is_some());

    protocol.stop().unwrap();
}

#[test]
fn test_unreachable_parks_escalate_and_reset_on_presence() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();
    let ble = MockTransport::new(TransportType::BLE);
    ble.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));

    // Each consecutive relay verdict doubles the timed-retry interval:
    // adapter availability is not peer reachability, and DORS may keep
    // routing the retry to the internet path, where every round trips
    // another budget-refunded DeliveryError. Without escalation that is an
    // unbounded 15-second welcome-resend loop into the relay.
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    for (round, expected_secs) in [(1u32, 15i64), (2, 30), (3, 60)] {
        protocol
            .on_transport_send_failed(
                &welcome_id,
                Some("recipient_unreachable: Recipient is offline".to_string()),
            )
            .unwrap();
        let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
        assert_eq!(lifecycle.unreachable_parks, round);
        let delay = (lifecycle.next_retry_at.unwrap() - Utc::now()).num_seconds();
        assert!(
            (expected_secs - 2..=expected_secs).contains(&delay),
            "park {} should retry in ~{}s, got {}s",
            round,
            expected_secs,
            delay
        );
    }

    // A reachability edge resets the ladder...
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol
            .welcome_lifecycles
            .get("bob")
            .unwrap()
            .unreachable_parks,
        0
    );

    // ...so the next verdict starts over from the base interval.
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();
    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.unreachable_parks, 1);
    let delay = (lifecycle.next_retry_at.unwrap() - Utc::now()).num_seconds();
    assert!(
        (13..=15).contains(&delay),
        "post-reset park should retry from the base interval, got {}s",
        delay
    );

    protocol.stop().unwrap();
}

#[test]
fn test_unreachable_park_probe_is_carrier_agnostic() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    // Nostr is "available" on every internet-connected device and is an
    // internet-dependent relay transport, so its adapter status says nothing
    // about local peer reachability. It used to veto the timed retry track;
    // now no carrier does — the park schedules the same escalating probe
    // whatever is available, and the escalation (not a carrier guard) is what
    // bounds resends into the relay.
    let nostr = MockTransport::new(TransportType::Nostr);
    nostr.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::Nostr, Box::new(nostr));

    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(lifecycle.state, WelcomeDeliveryState::Failed);
    assert_eq!(
        lifecycle.last_reason_code,
        Some(crate::events::WelcomeReasonCode::PeerUnreachable)
    );
    assert_eq!(lifecycle.unreachable_parks, 1);
    let delay = (lifecycle
        .next_retry_at
        .expect("every carrier keeps the probe alive")
        - Utc::now())
    .num_seconds();
    assert!(
        (13..=15).contains(&delay),
        "probe should start at the base interval, got {}s",
        delay
    );

    protocol.stop().unwrap();
}

#[test]
fn test_delivery_error_after_session_confirmed_is_ignored() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();

    // The welcome was rescued over another path and the peer proved the
    // session. A late relay verdict for the original internet copy must not
    // corrupt the converged lifecycle — flipping it to Failed, refunding an
    // attempt, and emitting welcome_send_failed AFTER the app already saw
    // secure_session_established.
    protocol.confirmed_sessions.insert("bob".to_string());

    let events = Arc::new(Mutex::new(Vec::<Event>::new()));
    let events_clone = events.clone();
    protocol.on_event(move |event| {
        if matches!(event, Event::WelcomeSendFailed { .. }) {
            events_clone.lock().unwrap().push(event);
        }
    });

    protocol
        .on_transport_send_failed(
            &welcome_id,
            Some("recipient_unreachable: Recipient is offline".to_string()),
        )
        .unwrap();

    let lifecycle = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(
        lifecycle.state,
        WelcomeDeliveryState::Sent,
        "a stale verdict must not flip a proven session's welcome to Failed"
    );
    assert!(
        events.lock().unwrap().is_empty(),
        "no welcome_send_failed may contradict secure_session_established"
    );

    protocol.stop().unwrap();
}

#[test]
fn test_watchlist_excludes_stale_welcome_lifecycles() {
    let (mut protocol, _internet, _welcome_id) = setup_internet_protocol_with_pending_welcome();
    assert_eq!(protocol.welcome_pending_peers(), vec!["bob".to_string()]);

    // A permanently-dead peer must not hold a watch-rotation slot forever:
    // past the age limit the peer drops off the watchlist, offline answers
    // stop pushing expires_at, and the record ages out through normal
    // expiry. Recovery degrades to peer-initiated contact or mesh
    // discovery, both of which rebuild the lifecycle.
    protocol
        .welcome_lifecycles
        .get_mut("bob")
        .unwrap()
        .created_at = Utc::now() - ChronoDuration::days(15);
    assert!(protocol.welcome_pending_peers().is_empty());

    protocol.stop().unwrap();
}

#[test]
fn test_presence_online_rescue_backoff() {
    let (mut protocol, internet_handle, welcome_id) =
        setup_internet_protocol_with_pending_welcome();
    let count_welcomes = |handle: &MockTransport| {
        handle
            .sent_messages()
            .iter()
            .filter(|m| m.content.starts_with(internal_prefixes::WELCOME))
            .count()
    };

    protocol.on_transport_send_confirmed(&welcome_id).unwrap();

    // First online answer: the rescue is free.
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );
    protocol.on_transport_send_confirmed(&welcome_id).unwrap();
    let welcomes_after_first = count_welcomes(&internet_handle);

    // Presence answers arrive on the platform's ~20s watch tick. A peer that
    // is online but never proves the session (stale key package after a
    // reinstall) must NOT be re-sent the welcome every tick: the second
    // consecutive online answer is throttled.
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::Sent,
        "second immediate online answer must not rescue again"
    );
    assert_eq!(count_welcomes(&internet_handle), welcomes_after_first);

    // Once the backoff window elapses, the rescue runs again.
    protocol
        .welcome_presence_rescue
        .get_mut("bob")
        .expect("throttle entry for bob")
        .next_allowed_at = Utc::now() - ChronoDuration::seconds(1);
    protocol.on_peer_presence("bob", true, None);
    assert_eq!(
        protocol.welcome_lifecycles.get("bob").unwrap().state,
        WelcomeDeliveryState::SendAttempted
    );
    assert_eq!(count_welcomes(&internet_handle), welcomes_after_first + 1);

    // Only executed rescues count toward the doubling backoff — the
    // throttled tick in between must not inflate it.
    assert_eq!(
        protocol.welcome_presence_rescue.get("bob").unwrap().rescues,
        2
    );

    protocol.stop().unwrap();
}

#[test]
fn test_peer_presence_offline_defers_to_mesh_carrier() {
    let (mut protocol, _internet, welcome_id) = setup_internet_protocol_with_pending_welcome();

    // A mesh carrier is up alongside the relay socket.
    let ble = MockTransport::new(TransportType::BLE);
    ble.start().unwrap();
    protocol
        .transport_manager_mut()
        .add_transport(TransportType::BLE, Box::new(ble));

    // Carrier-backed failure: the welcome sits on the data-plane retry track.
    protocol
        .on_transport_send_failed(&welcome_id, Some("network blip".to_string()))
        .unwrap();
    let before = protocol.welcome_lifecycles.get("bob").unwrap().clone();
    assert_eq!(before.state, WelcomeDeliveryState::Failed);
    assert!(before.next_retry_at.is_some());

    // Relay presence is only authoritative for the internet path: with BLE
    // also available the offline answer must not touch the retry track —
    // the peer may be sitting right next to us.
    protocol.on_peer_presence("bob", false, None);
    let after = protocol.welcome_lifecycles.get("bob").unwrap();
    assert_eq!(after.state, WelcomeDeliveryState::Failed);
    assert_eq!(after.next_retry_at, before.next_retry_at);
    assert_eq!(after.last_reason_code, before.last_reason_code);

    protocol.stop().unwrap();
}
