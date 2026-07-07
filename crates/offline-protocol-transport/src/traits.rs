//! Transport trait definitions.

use crate::{Result, TransportMetrics, TransportType};
use offline_protocol_core::Message;
use std::any::Any;

/// Status of a transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportStatus {
    /// Transport is available and ready to use.
    Available,
    /// Transport is unavailable (not supported or disabled).
    Unavailable,
    /// Transport is connecting or initializing.
    Connecting,
    /// Transport is disconnected.
    Disconnected,
    /// Transport encountered an error.
    Error,
}

/// Trait for transport implementations.
///
/// This is the engine-facing side of a transport: enqueue outbound
/// messages, dequeue inbound ones, and report status and metrics. The
/// implementations in this crate are I/O-free queue engines — the
/// platform-specific delivery details live in the platform bridge (see the
/// crate-level docs).
pub trait Transport: Send + Sync + Any {
    /// Returns this transport as `&dyn Any` for safe downcasting.
    fn as_any(&self) -> &dyn Any;
    /// Returns the type of this transport.
    fn transport_type(&self) -> TransportType;

    /// Returns the current status of the transport.
    fn status(&self) -> TransportStatus;

    /// Gets current metrics for this transport.
    fn metrics(&self) -> TransportMetrics;

    /// Queues a message for delivery through this transport.
    ///
    /// Implementations in this crate perform no I/O here: the message is
    /// enqueued for the platform bridge to drain and put on the wire.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the transport accepted the message, `Err`
    /// otherwise (e.g. the transport is not `Available`). `Ok` means
    /// enqueued, not delivered — the platform bridge confirms or fails
    /// delivery asynchronously.
    fn send(&self, message: &Message) -> Result<()>;

    /// Attempts to receive a message from this transport.
    ///
    /// Messages arrive in this queue after the platform bridge injects
    /// inbound bytes via the transport's `on_data_received` /
    /// `on_fragment_received` methods.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(Message))` if a message was received, `Ok(None)` if no message
    /// is available, or `Err` if an error occurred.
    fn receive(&self) -> Result<Option<Message>>;

    /// Starts the transport.
    ///
    /// This performs no I/O. Most implementations stay `Unavailable` until
    /// the platform bridge reports connectivity via their
    /// `on_status_changed()` method; BLE is the exception and optimistically
    /// sets `Available` (the platform can still override it).
    fn start(&mut self) -> Result<()>;

    /// Stops the transport, marking it `Disconnected` and clearing queued
    /// state.
    fn stop(&mut self) -> Result<()>;
}
