//! Transport trait definitions.

use crate::{Result, TransportMetrics, TransportType};
use offline_protocol_core::Message;

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
/// This trait defines the interface that all transport implementations must follow.
/// Implementations handle the platform-specific details of sending and receiving messages.
pub trait Transport: Send + Sync {
    /// Returns the type of this transport.
    fn transport_type(&self) -> TransportType;

    /// Returns the current status of the transport.
    fn status(&self) -> TransportStatus;

    /// Gets current metrics for this transport.
    fn metrics(&self) -> TransportMetrics;

    /// Sends a message through this transport.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to send
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the message was sent successfully, `Err` otherwise.
    fn send(&self, message: &Message) -> Result<()>;

    /// Attempts to receive a message from this transport.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(Message))` if a message was received, `Ok(None)` if no message
    /// is available, or `Err` if an error occurred.
    fn receive(&self) -> Result<Option<Message>>;

    /// Starts the transport.
    fn start(&mut self) -> Result<()>;

    /// Stops the transport.
    fn stop(&mut self) -> Result<()>;
}
