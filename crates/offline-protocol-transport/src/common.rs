//! Common helper functions shared by all platform-bridged transports.
//!
//! These functions encapsulate the identical logic used by BLE, WiFi Direct,
//! Internet, and any future transport implementations. Each transport delegates
//! to these helpers from its own `pub fn` methods, keeping the per-transport
//! boilerplate to one-liner delegation calls.

use crate::constants::DEFAULT_MAX_MESSAGE_SIZE;
use crate::{Error, Result, TransportMetrics};
use offline_protocol_core::{Message, MutexExt};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Queues a received message.
pub fn on_message_received(receive_queue: &Mutex<VecDeque<Message>>, message: Message) {
    receive_queue.lock_or_recover().push_back(message);
}

/// Sets the transport peer identity on a message and queues it.
/// Drops the message (with a warning) if the peer_id is invalid.
pub fn on_message_received_from(
    receive_queue: &Mutex<VecDeque<Message>>,
    mut message: Message,
    peer_id: String,
) {
    if let Err(e) = message.set_transport_peer_id(peer_id) {
        tracing::warn!(
            error = %e,
            message_id = %message.id,
            "Dropping message: transport provided invalid peer_id"
        );
        return;
    }
    receive_queue.lock_or_recover().push_back(message);
}

/// Deserialises raw bytes into a message and queues it.
/// Returns `Err` only for oversized payloads; malformed data is silently dropped.
pub fn on_data_received(receive_queue: &Mutex<VecDeque<Message>>, data: Vec<u8>) -> Result<()> {
    if data.len() > DEFAULT_MAX_MESSAGE_SIZE {
        return Err(Error::MessageTooLarge(data.len(), DEFAULT_MAX_MESSAGE_SIZE));
    }
    match deserialize_message(&data) {
        Ok(message) => {
            receive_queue.lock_or_recover().push_back(message);
            Ok(())
        }
        Err(e) => {
            tracing::warn!(error = %e, "Error deserializing message, dropping bad data");
            Ok(())
        }
    }
}

/// Deserialises raw bytes, sets the transport peer identity, and queues the message.
/// Returns `Err` for oversized payloads or invalid peer_id; malformed data is silently dropped.
pub fn on_data_received_from(
    receive_queue: &Mutex<VecDeque<Message>>,
    data: Vec<u8>,
    peer_id: String,
) -> Result<()> {
    if data.len() > DEFAULT_MAX_MESSAGE_SIZE {
        return Err(Error::MessageTooLarge(data.len(), DEFAULT_MAX_MESSAGE_SIZE));
    }
    match deserialize_message(&data) {
        Ok(mut message) => {
            message.set_transport_peer_id(peer_id)?;
            receive_queue.lock_or_recover().push_back(message);
            Ok(())
        }
        Err(e) => {
            tracing::warn!(error = %e, "Error deserializing message, dropping bad data");
            Ok(())
        }
    }
}

/// Serialises a message to JSON bytes.
pub fn serialize_message(message: &Message) -> Result<Vec<u8>> {
    serde_json::to_vec(message)
        .map_err(|e| Error::SerializationError(format!("Failed to serialize message: {}", e)))
}

/// Deserialises a message from JSON bytes.
pub fn deserialize_message(data: &[u8]) -> Result<Message> {
    serde_json::from_slice(data)
        .map_err(|e| Error::SerializationError(format!("Failed to deserialize message: {}", e)))
}

/// Stores an opaque platform handle.
pub fn set_platform_handle(handle_lock: &Mutex<Option<usize>>, handle: usize) {
    *handle_lock.lock_or_recover() = Some(handle);
}

/// Retrieves the stored platform handle.
pub fn platform_handle(handle_lock: &Mutex<Option<usize>>) -> Option<usize> {
    *handle_lock.lock_or_recover()
}

/// Recalculates `delivery_ratio` and `drop_rate` from the current success/failure counts.
pub fn recalculate_delivery_ratios(metrics: &mut TransportMetrics) {
    let total = metrics.success_count + metrics.failure_count;
    if total > 0 {
        let ratio = metrics.success_count as f32 / total as f32;
        metrics.delivery_ratio = Some(ratio.clamp(0.0, 1.0));
        metrics.drop_rate = Some((1.0 - ratio).clamp(0.0, 1.0));
    }
}
