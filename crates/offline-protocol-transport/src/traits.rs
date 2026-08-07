//! Transport trait definitions.

use crate::{Result, TransportMetrics, TransportType};
use offline_protocol_core::Message;
use std::any::Any;
use std::sync::Arc;

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
    ///
    /// # Delivery model and wire-codec safety
    ///
    /// The built-in mesh transports **unicast to `message.recipient`** and
    /// require it to be a directly connected peer (they error otherwise). The
    /// per-peer binary wire-codec negotiation depends on this: capability is
    /// recorded against `recipient`, so a binary frame only ever reaches the
    /// peer whose support was confirmed — never a legacy bystander. A transport
    /// that instead *broadcasts* serialized bytes to every neighbor would break
    /// that assumption and must re-establish per-link codec safety before
    /// emitting anything other than JSON.
    ///
    /// [`Transport::send_to_peer`] keeps the same invariant by moving the key:
    /// there the physical target is `peer_id`, so codec and MTU decisions are
    /// made against *that* peer's confirmed capabilities rather than the
    /// recipient's. Frames still go out one addressed link at a time; nothing
    /// in this crate emits one serialized buffer to many peers.
    fn send(&self, message: &Message) -> Result<()>;

    /// Queues a message for delivery to a **specific directly connected
    /// peer**, regardless of `message.recipient`.
    ///
    /// This is the addressed-hop counterpart to [`Transport::send`]: it exists
    /// so a caller can hand a frame to a neighbor that is not the frame's final
    /// recipient (mesh forwarding), while [`Transport::send`] remains the
    /// "deliver this to its recipient" call. Transports that cannot address a
    /// peer independently of the recipient return an error, which is the
    /// default.
    ///
    /// Implementations must treat `peer_id` as the physical target for every
    /// link-layer decision — MTU selection, queue keying, wire-codec choice —
    /// because that is the peer whose capabilities were negotiated. Using
    /// `message.recipient` for any of those on a forwarded frame would apply a
    /// third party's link parameters to this hop.
    ///
    /// Returns `Ok(())` if the transport accepted the message for that peer,
    /// `Err` if the peer is not a live link or the transport is unavailable.
    fn send_to_peer(&self, peer_id: &str, message: &Message) -> Result<()> {
        let _ = message;
        Err(crate::Error::Other(format!(
            "{} transport cannot address peer {} independently of the recipient",
            self.transport_type(),
            peer_id
        )))
    }

    /// Lists the peers this transport currently holds a live link to.
    ///
    /// Only transports whose links are peer-to-peer (BLE, Wi-Fi Direct) report
    /// anything; carriers that reach peers through infrastructure return the
    /// default empty list, since "directly connected" has no meaning for them.
    ///
    /// This is deliberately a *live connectivity* view, not a discovery
    /// memory: entries appear when the platform reports a connection and
    /// disappear when it reports a loss or the transport leaves
    /// [`TransportStatus::Available`]. Callers that fan out to neighbors
    /// depend on that — addressing a remembered-but-gone peer would queue
    /// frames for a link that no longer exists.
    fn connected_peers(&self) -> Vec<crate::PeerLink> {
        Vec::new()
    }

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
    /// [`Transport::on_status_changed`] method; BLE is the exception and
    /// optimistically sets `Available` (the platform can still override it).
    fn start(&self) -> Result<()>;

    /// Stops the transport, marking it `Disconnected` and clearing queued
    /// state.
    ///
    /// Callers must quiesce the platform bridge (callbacks *and* engine
    /// sends) before stopping: every entry point takes `&self`, so a
    /// racing `send()` or platform callback landing after the drain can
    /// re-seed state into what is supposed to be an at-rest transport.
    fn stop(&self) -> Result<()>;

    // ------------------------------------------------------------------
    // Platform-bridge ingress (platform → Rust)
    // ------------------------------------------------------------------

    /// Reports a connectivity change observed by the platform bridge.
    ///
    /// This is how transports leave (and re-enter) `Available`: the platform
    /// owns the radio/socket and pushes status transitions here.
    /// Implementations drain per-session state when leaving `Available` so a
    /// reconnect starts clean.
    fn on_status_changed(&self, status: TransportStatus);

    /// Injects inbound wire bytes from the platform bridge.
    ///
    /// The default rejects the data: transports that receive whole serialized
    /// messages (WiFi Direct, Internet, Nostr, Reticulum) override this.
    /// BLE receives fragments instead — see [`Transport::on_fragment_received`].
    fn on_data_received(&self, data: Vec<u8>) -> Result<()> {
        let _ = data;
        Err(crate::Error::Other(format!(
            "{} transport does not accept whole-message data; \
             use the transport's fragment/event ingress instead",
            self.transport_type()
        )))
    }

    /// Like [`Transport::on_data_received`], but attaches a
    /// transport-verified `peer_id` (the remote peer's user-level identifier,
    /// not a raw transport address) to the deserialized message.
    fn on_data_received_from(&self, data: Vec<u8>, peer_id: String) -> Result<()> {
        let _ = (data, peer_id);
        Err(crate::Error::Other(format!(
            "{} transport does not accept whole-message data; \
             use the transport's fragment/event ingress instead",
            self.transport_type()
        )))
    }

    /// Injects an inbound fragment from the platform bridge (BLE).
    ///
    /// The default rejects the data: only fragmenting transports (BLE)
    /// override this. Whole-message transports use
    /// [`Transport::on_data_received`].
    fn on_fragment_received(&self, fragment_data: Vec<u8>) -> Result<()> {
        let _ = fragment_data;
        Err(crate::Error::Other(format!(
            "{} transport does not reassemble fragments; \
             use on_data_received instead",
            self.transport_type()
        )))
    }

    /// Like [`Transport::on_fragment_received`], but attaches a
    /// transport-verified `peer_id` to the reassembled message.
    ///
    /// This is the fragmenting counterpart to
    /// [`Transport::on_data_received_from`]. The peer id it records is the
    /// link the frame physically arrived on, which is what lets the protocol
    /// layer tell "who handed me this" apart from "who wrote this" — the two
    /// are the same peer only at the first hop.
    fn on_fragment_received_from(&self, fragment_data: Vec<u8>, peer_id: String) -> Result<()> {
        let _ = (fragment_data, peer_id);
        Err(crate::Error::Other(format!(
            "{} transport does not reassemble fragments; \
             use on_data_received_from instead",
            self.transport_type()
        )))
    }

    // ------------------------------------------------------------------
    // Platform-bridge egress / poll (Rust → platform)
    // ------------------------------------------------------------------

    /// Dequeues the next outbound message for the platform bridge to put on
    /// the wire.
    ///
    /// Returns `(key, serialized_bytes)` where the `key` is
    /// transport-specific: a recipient address for peer-to-peer transports
    /// (WiFi Direct), or the message id for confirmation-loop transports
    /// (Internet, Nostr, Reticulum — pair with [`Transport::confirm_sent`] /
    /// [`Transport::report_send_failure`]). The default returns `Ok(None)`:
    /// fragmenting transports (BLE) expose
    /// [`Transport::get_next_fragment`] instead.
    fn get_next_message(&self) -> Result<Option<(String, Vec<u8>)>> {
        Ok(None)
    }

    /// Dequeues the next outbound fragment for the platform bridge (BLE).
    ///
    /// Returns `(recipient, fragment_bytes)`. The default returns
    /// `Ok(None)`: whole-message transports expose
    /// [`Transport::get_next_message`] instead.
    fn get_next_fragment(&self) -> Result<Option<(String, Vec<u8>)>> {
        Ok(None)
    }

    /// Registers the callback that wakes the platform bridge when outbound
    /// data becomes available to poll, instead of polling on a timer.
    ///
    /// The default drops the callback with a warning — a transport that
    /// supports platform-driven draining must override this.
    fn set_on_messages_available(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        let _ = callback;
        tracing::warn!(
            transport = %self.transport_type(),
            "set_on_messages_available: transport has no outbound-available callback; \
             callback dropped"
        );
    }

    // ------------------------------------------------------------------
    // Confirmation loop (platform → Rust delivery outcomes)
    // ------------------------------------------------------------------

    /// Platform confirms a message previously returned by
    /// [`Transport::get_next_message`] was sent over the wire.
    ///
    /// The default is a no-op: only confirmation-loop transports (Internet,
    /// Nostr, Reticulum) track pending outcomes.
    fn confirm_sent(&self, message_id: &str) {
        tracing::debug!(
            transport = %self.transport_type(),
            message_id,
            "confirm_sent: transport has no confirmation loop; ignoring"
        );
    }

    /// Platform reports a message previously returned by
    /// [`Transport::get_next_message`] failed to send.
    ///
    /// The default is a no-op: only confirmation-loop transports (Internet,
    /// Nostr, Reticulum) track pending outcomes.
    fn report_send_failure(&self, message_id: &str) {
        tracing::debug!(
            transport = %self.transport_type(),
            message_id,
            "report_send_failure: transport has no confirmation loop; ignoring"
        );
    }

    // ------------------------------------------------------------------
    // Link configuration
    // ------------------------------------------------------------------

    /// Records the maximum usable payload for a peer after link-layer MTU
    /// negotiation (BLE).
    ///
    /// The default warns and ignores the report: only MTU-aware transports
    /// (BLE) override this.
    fn set_peer_mtu(&self, peer_id: &str, max_payload: usize) {
        tracing::warn!(
            transport = %self.transport_type(),
            peer = %peer_id,
            max_payload,
            "set_peer_mtu: transport is not MTU-aware; ignoring"
        );
    }

    /// Removes any stored MTU for a peer (BLE; called on disconnect or
    /// before renegotiation). The default is a no-op.
    fn clear_peer_mtu(&self, peer_id: &str) {
        tracing::debug!(
            transport = %self.transport_type(),
            peer = %peer_id,
            "clear_peer_mtu: transport is not MTU-aware; ignoring"
        );
    }

    // ------------------------------------------------------------------
    // Serialization
    // ------------------------------------------------------------------

    /// Deserializes a wire payload into a [`Message`].
    ///
    /// Every transport in this crate uses the shared JSON wire format, so
    /// the provided implementation is normally correct as-is.
    fn deserialize_message(&self, data: &[u8]) -> Result<Message> {
        crate::common::deserialize_message(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_core::{AppId, UserId};

    /// Implements only the required Transport methods, so every provided
    /// default is exercised as-is.
    struct MinimalTransport;

    impl Transport for MinimalTransport {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn transport_type(&self) -> TransportType {
            TransportType::Internet
        }
        fn status(&self) -> TransportStatus {
            TransportStatus::Unavailable
        }
        fn metrics(&self) -> TransportMetrics {
            TransportMetrics::default()
        }
        fn send(&self, _message: &Message) -> Result<()> {
            Ok(())
        }
        fn receive(&self) -> Result<Option<Message>> {
            Ok(None)
        }
        fn start(&self) -> Result<()> {
            Ok(())
        }
        fn stop(&self) -> Result<()> {
            Ok(())
        }
        fn on_status_changed(&self, _status: TransportStatus) {}
    }

    #[test]
    fn default_polls_return_none() {
        let t = MinimalTransport;
        assert!(t.get_next_message().unwrap().is_none());
        assert!(t.get_next_fragment().unwrap().is_none());
    }

    #[test]
    fn default_ingress_rejects_data() {
        // Dropping inbound bytes must be loud: a transport that does not
        // override an ingress method reports an error instead of silently
        // discarding the payload.
        let t = MinimalTransport;
        assert!(t.on_data_received(vec![1, 2, 3]).is_err());
        assert!(t
            .on_data_received_from(vec![1, 2, 3], "peer".to_string())
            .is_err());
        assert!(t.on_fragment_received(vec![1, 2, 3]).is_err());
    }

    #[test]
    fn default_confirmation_and_mtu_hooks_are_noops() {
        let t = MinimalTransport;
        t.confirm_sent("msg-1");
        t.report_send_failure("msg-1");
        t.set_peer_mtu("peer", 500);
        t.clear_peer_mtu("peer");
        t.set_on_messages_available(Arc::new(|| {}));
    }

    #[test]
    fn default_deserialize_reads_shared_wire_format() {
        let t = MinimalTransport;
        let message = Message::new(
            UserId::new("alice").unwrap(),
            UserId::new("bob").unwrap(),
            AppId::new("test").unwrap(),
            "hello",
        );
        let bytes = crate::common::serialize_message(&message).unwrap();
        let roundtripped = t.deserialize_message(&bytes).unwrap();
        assert_eq!(roundtripped.id, message.id);
    }
}
