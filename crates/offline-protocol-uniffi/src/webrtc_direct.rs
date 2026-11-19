//! The WebRTC Direct listener behind the `webrtc_direct_*` methods (ADR 0039,
//! `docs/spec/stream-framing.md`, section "Dialling a listener from a
//! browser").
//!
//! # Invariants
//!
//! - **Every method exists whichever way the library was built.** The
//!   records and the methods are in the interface definition, so the five
//!   generated bindings are one artifact set with or without the
//!   `webrtc-direct` feature. Without it, `open` refuses with a message that
//!   names the feature, and the rest answer as if no listener were open. A
//!   method compiled out instead would be a checksum mismatch at the first
//!   call in every binding, not a refusal the application can show.
//! - **A stream id names one stream for the life of the engine.** The
//!   listener numbers its streams from zero each time it opens, so this side
//!   gives each proved stream its own id from one counter that never goes
//!   back. Otherwise a bridge that sent on stream 0 as the application
//!   reopened the listener would write one peer's body onto another peer's
//!   stream.
//! - **A closed listener's events and datagrams are still delivered.**
//!   Closing a listener yields a `Closed` event for each proved stream, and
//!   the bridge's stream table needs every one of them to report each loss.
//!   It also queues each open stream's channel close, which is how a page
//!   learns at once that its stream ended rather than 15 to 20 s later when
//!   its consent checks lapse. Both are moved out of the listener before it
//!   is dropped. A replaced listener's datagrams are dropped instead: they
//!   were for the socket its replacement does not use.
//! - **This side never decides which streams the engine hears about.** It
//!   moves datagrams and events between the platform and the listener; the
//!   platform keeps the stream table and calls `wifi_direct_*` itself, as
//!   the reference bridge in `offline-protocol-webrtc-direct`'s example does.

/// A listener's dial code. See the interface definition.
#[derive(Clone, PartialEq, Eq)]
pub struct WebrtcDirectCode {
    pub fragment: String,
    pub host: String,
    pub port: u16,
    pub address: String,
}

// The fragment carries the listener's ICE password, which admits datagrams
// to it, so it stays out of logs.
impl std::fmt::Debug for WebrtcDirectCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebrtcDirectCode")
            .field("fragment", &"<redacted>")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("address", &self.address)
            .finish()
    }
}

/// A datagram for the platform to send from the listener's socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebrtcDirectDatagram {
    pub destination_ip: String,
    pub destination_port: u16,
    pub data: Vec<u8>,
}

/// What a listener reports. See the interface definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebrtcDirectEventKind {
    Proved,
    Body,
    Closed,
}

/// One listener event. See the interface definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebrtcDirectEvent {
    pub kind: WebrtcDirectEventKind,
    pub stream: u64,
    pub address: Option<String>,
    pub remote_ip: Option<String>,
    pub remote_port: Option<u16>,
    pub data: Option<Vec<u8>>,
}

/// The message every refusal of a library built without the feature
/// carries.
#[cfg_attr(feature = "webrtc-direct", allow(dead_code))]
pub(crate) const NOT_BUILT: &str =
    "webrtc direct: this library was built without the webrtc-direct feature";

/// What a listener needs from the engine: this device's address and its
/// identity assertion, the preamble every stream starts with.
#[cfg_attr(not(feature = "webrtc-direct"), allow(dead_code))]
pub(crate) struct Identity {
    pub address: String,
    pub preamble: Vec<u8>,
}

#[cfg(not(feature = "webrtc-direct"))]
mod imp {
    use super::{Identity, WebrtcDirectCode, WebrtcDirectDatagram, WebrtcDirectEvent, NOT_BUILT};
    use crate::ProtocolError;

    /// The listener slot of a library built without the feature: it never
    /// holds a listener.
    pub(crate) struct WebrtcDirect;

    impl WebrtcDirect {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn open(
            &self,
            _local_ip: &str,
            _local_port: u16,
            _identity: impl FnOnce() -> Result<Identity, ProtocolError>,
        ) -> Result<WebrtcDirectCode, ProtocolError> {
            Err(ProtocolError::InvalidState(NOT_BUILT.to_owned()))
        }

        pub(crate) fn close(&self) {}

        pub(crate) fn datagram_received(
            &self,
            _source_ip: &str,
            _source_port: u16,
            _data: &[u8],
        ) -> Result<(), ProtocolError> {
            Err(ProtocolError::InvalidState(NOT_BUILT.to_owned()))
        }

        pub(crate) fn next_datagram(&self) -> Option<WebrtcDirectDatagram> {
            None
        }

        pub(crate) fn next_event(&self) -> Option<WebrtcDirectEvent> {
            None
        }

        pub(crate) fn send(&self, _stream: u64, _body: &[u8]) -> Result<(), ProtocolError> {
            Err(ProtocolError::InvalidState(NOT_BUILT.to_owned()))
        }

        pub(crate) fn close_stream(&self, _stream: u64) {}

        pub(crate) fn tick(&self) -> Option<u32> {
            None
        }
    }
}

#[cfg(feature = "webrtc-direct")]
mod imp {
    use std::collections::{HashMap, VecDeque};
    use std::net::{IpAddr, SocketAddr};
    use std::sync::{Mutex, MutexGuard};

    use offline_protocol_core::time::Instant;
    use offline_protocol_webrtc_direct::{
        Listener, ListenerConfig, ListenerError, ListenerEvent, StreamId, Transmit,
    };

    use super::{
        Identity, WebrtcDirectCode, WebrtcDirectDatagram, WebrtcDirectEvent, WebrtcDirectEventKind,
    };
    use crate::{recover_mutex, ProtocolError};

    /// The listener slot: at most one listener, and the ids its proved
    /// streams cross the boundary under.
    #[derive(Default)]
    pub(crate) struct WebrtcDirect {
        state: Mutex<State>,
    }

    #[derive(Default)]
    struct State {
        listener: Option<Listener>,
        /// Events translated to boundary ids, oldest first. Every event the
        /// listener yields is moved here after each call, so a listener can
        /// be replaced or closed without losing its `Closed` events.
        events: VecDeque<WebrtcDirectEvent>,
        /// The boundary id of each proved stream of the open listener that
        /// has not ended, and back.
        ids: HashMap<StreamId, u64>,
        streams: HashMap<u64, StreamId>,
        /// The next boundary id. Never reused (the second invariant).
        next_id: u64,
        /// What a closed listener queued to send, until the bridge drains
        /// it from the socket it is about to close (the third invariant).
        closing: VecDeque<WebrtcDirectDatagram>,
    }

    impl State {
        /// Moves every event the listener has into [`State::events`].
        fn collect(&mut self) {
            let Some(listener) = self.listener.as_mut() else {
                return;
            };
            while let Some(event) = listener.poll_event() {
                let translated = match event {
                    ListenerEvent::Proved {
                        stream,
                        address,
                        remote,
                    } => {
                        let id = self.next_id;
                        self.next_id += 1;
                        self.ids.insert(stream, id);
                        self.streams.insert(id, stream);
                        WebrtcDirectEvent {
                            kind: WebrtcDirectEventKind::Proved,
                            stream: id,
                            address: Some(address),
                            remote_ip: Some(remote.ip().to_string()),
                            remote_port: Some(remote.port()),
                            data: None,
                        }
                    }
                    ListenerEvent::Body { stream, bytes } => {
                        let Some(&id) = self.ids.get(&stream) else {
                            // The listener yields a body only for a stream
                            // it proved, so this is unreachable; a body with
                            // no id has no stream to be attributed to.
                            continue;
                        };
                        WebrtcDirectEvent {
                            kind: WebrtcDirectEventKind::Body,
                            stream: id,
                            address: None,
                            remote_ip: None,
                            remote_port: None,
                            data: Some(bytes),
                        }
                    }
                    ListenerEvent::Closed { stream } => {
                        let Some(id) = self.ids.remove(&stream) else {
                            continue;
                        };
                        self.streams.remove(&id);
                        WebrtcDirectEvent {
                            kind: WebrtcDirectEventKind::Closed,
                            stream: id,
                            address: None,
                            remote_ip: None,
                            remote_port: None,
                            data: None,
                        }
                    }
                };
                self.events.push_back(translated);
            }
        }

        /// Closes the open listener, if any, keeping its events and the
        /// datagrams it queued: its channels closing, for the bridge to send
        /// before it closes the socket.
        fn close(&mut self) {
            if let Some(listener) = self.listener.as_mut() {
                listener.close();
                while let Some(transmit) = listener.poll_transmit() {
                    self.closing.push_back(datagram(transmit));
                }
            }
            self.collect();
            self.listener = None;
            self.ids.clear();
            self.streams.clear();
        }
    }

    impl WebrtcDirect {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        fn lock(&self) -> MutexGuard<'_, State> {
            recover_mutex(&self.state, "webrtc_direct")
        }

        pub(crate) fn open(
            &self,
            local_ip: &str,
            local_port: u16,
            identity: impl FnOnce() -> Result<Identity, ProtocolError>,
        ) -> Result<WebrtcDirectCode, ProtocolError> {
            let ip = parse_ip(local_ip, "local_ip")?;
            let Identity { address, preamble } = identity()?;
            let listener = Listener::new(
                ListenerConfig {
                    local: SocketAddr::new(ip, local_port),
                    address,
                    preamble,
                },
                Instant::now(),
            )
            .map_err(open_error)?;
            let code = listener.dial_code();
            let code = WebrtcDirectCode {
                fragment: code.to_fragment(),
                host: code.host.to_string(),
                port: code.port,
                address: code.address.clone(),
            };
            // Only once the new listener exists: a refused open leaves the
            // listener already open as it was.
            let mut state = self.lock();
            state.close();
            // For the replaced listener's socket, which this one does not
            // use: a page ignores a datagram from an address it never paired.
            state.closing.clear();
            state.listener = Some(listener);
            Ok(code)
        }

        pub(crate) fn close(&self) {
            self.lock().close();
        }

        pub(crate) fn datagram_received(
            &self,
            source_ip: &str,
            source_port: u16,
            data: &[u8],
        ) -> Result<(), ProtocolError> {
            let ip = parse_ip(source_ip, "source_ip")?;
            let mut state = self.lock();
            let listener = state.listener.as_mut().ok_or_else(not_open)?;
            listener.handle_datagram(Instant::now(), SocketAddr::new(ip, source_port), data);
            state.collect();
            Ok(())
        }

        pub(crate) fn next_datagram(&self) -> Option<WebrtcDirectDatagram> {
            let mut state = self.lock();
            if let Some(closing) = state.closing.pop_front() {
                return Some(closing);
            }
            state.listener.as_mut()?.poll_transmit().map(datagram)
        }

        pub(crate) fn next_event(&self) -> Option<WebrtcDirectEvent> {
            self.lock().events.pop_front()
        }

        pub(crate) fn send(&self, stream: u64, body: &[u8]) -> Result<(), ProtocolError> {
            let mut state = self.lock();
            let target = state.streams.get(&stream).copied();
            let listener = state.listener.as_mut().ok_or_else(not_open)?;
            let result = match target {
                Some(target) => listener.send(target, body),
                None => Err(ListenerError::UnknownStream),
            };
            // An overflow closes the stream, and its `Closed` event is due.
            state.collect();
            result.map_err(send_error)
        }

        pub(crate) fn close_stream(&self, stream: u64) {
            let mut state = self.lock();
            let Some(target) = state.streams.get(&stream).copied() else {
                return;
            };
            if let Some(listener) = state.listener.as_mut() {
                listener.close_stream(target);
            }
            state.collect();
        }

        pub(crate) fn tick(&self) -> Option<u32> {
            let mut state = self.lock();
            let now = Instant::now();
            let listener = state.listener.as_mut()?;
            listener.handle_timeout(now);
            let next = listener.next_timeout();
            state.collect();
            let wait = next?.saturating_duration_since(now);
            // Rounded up, so a platform that sleeps exactly this long never
            // wakes a millisecond early and finds nothing due.
            let millis = wait.as_micros().div_ceil(1000);
            Some(u32::try_from(millis).unwrap_or(u32::MAX))
        }
    }

    fn datagram(transmit: Transmit) -> WebrtcDirectDatagram {
        WebrtcDirectDatagram {
            destination_ip: transmit.destination.ip().to_string(),
            destination_port: transmit.destination.port(),
            data: transmit.bytes,
        }
    }

    fn parse_ip(text: &str, name: &str) -> Result<IpAddr, ProtocolError> {
        text.parse().map_err(|_| {
            ProtocolError::InvalidArgument(format!(
                "webrtc direct: {name} is not an IP address literal (no zone index): {text:?}"
            ))
        })
    }

    fn not_open() -> ProtocolError {
        ProtocolError::InvalidState("webrtc direct: no listener is open".to_owned())
    }

    fn open_error(e: ListenerError) -> ProtocolError {
        let message = format!("webrtc direct: {e}");
        match e {
            ListenerError::LocalAddress(_) | ListenerError::Socket | ListenerError::Address => {
                ProtocolError::InvalidArgument(message)
            }
            ListenerError::Certificate | ListenerError::Entropy => {
                ProtocolError::TransportError(message)
            }
            ListenerError::Preamble
            | ListenerError::UnknownStream
            | ListenerError::Frame(_)
            | ListenerError::Overflow
            | ListenerError::Closed => ProtocolError::InvalidState(message),
        }
    }

    fn send_error(e: ListenerError) -> ProtocolError {
        let message = format!("webrtc direct: {e}");
        match e {
            ListenerError::Frame(_) => ProtocolError::InvalidArgument(message),
            ListenerError::Overflow => ProtocolError::SendFailed(message),
            ListenerError::LocalAddress(_)
            | ListenerError::Socket
            | ListenerError::Address
            | ListenerError::Preamble
            | ListenerError::Certificate
            | ListenerError::Entropy
            | ListenerError::UnknownStream
            | ListenerError::Closed => ProtocolError::InvalidState(message),
        }
    }
}

pub(crate) use imp::WebrtcDirect;
