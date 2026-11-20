//! The bridge between a WebRTC Direct listener and the engine's peer-stream
//! slot. This is the part a native bridge (Swift, Kotlin, Python) copies.
//!
//! The listener proves who is on each stream and moves bodies; the bridge
//! decides which streams the engine hears about. It keeps one stream table,
//! keyed by stream and by proved address, and holds these rules
//! (`docs/spec/stream-framing.md`, "What a receiver owes" and "Dialling a
//! listener from a browser"):
//!
//! - **The engine sees one announcement and, later, one loss per address.**
//!   At most one stream holds an address. A stream refused or superseded
//!   while another holds its address is never reported lost, and a
//!   superseding stream is never announced a second time. The engine keys
//!   its peer-stream links by address, so a bridge that announced twice and
//!   reported the first close would tell the engine a reachable peer is
//!   gone, and a copied preamble is enough to make it do so.
//! - **Of two dialled streams proving one address, the newer wins.** Only
//!   the page dials, so a page that reloaded, or came back after a Wi-Fi
//!   drop, is the newer stream, and keeping the older would refuse it until
//!   the dead stream's consent timed out, up to 15 s. Against a stream of
//!   another kind (a TCP stream in a bridge that holds both) the existing
//!   stream stays. Every stream in this example's table is dialled.
//! - **A proved stream counts against the slot's caps**: sixteen streams,
//!   of which at most twelve are inbound and four come from one remote IP.
//!   A stream past a cap is closed and never announced. Every dialled
//!   stream is inbound.
//! - **A body is handed up under the address its stream proved**, never a
//!   socket address, and an outbound body goes to the stream proving its
//!   recipient or nowhere.
//! - **A stream that proves this device's own address is refused**: it is a
//!   copied preamble or a loop, and announcing it would make the engine
//!   route to itself.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};

use offline_protocol_core::time::Instant;
use offline_protocol_webrtc_direct::{
    DialCode, Listener, ListenerConfig, ListenerError, ListenerEvent, StreamId, Transmit,
};

/// Streams the slot holds at once, of every kind.
pub const MAX_STREAMS: usize = 16;
/// Inbound streams the slot holds at once. A dialled stream is inbound.
pub const MAX_INBOUND_STREAMS: usize = 12;
/// Streams from one remote IP the slot holds at once.
pub const MAX_STREAMS_PER_REMOTE_IP: usize = 4;

/// The engine's peer-stream slot, as the FFI names it. A native bridge calls
/// the generated methods of the same names.
pub trait Slot {
    /// The stream layer is up (`true`) or down.
    fn wifi_direct_status_changed(&mut self, is_connected: bool);
    /// A stream proved `peer_id`. Called once per announced stream.
    fn wifi_direct_peer_connected(&mut self, peer_id: &str);
    /// One body from the stream that proved `sender_id`.
    fn wifi_direct_message_received(&mut self, sender_id: &str, data: Vec<u8>);
    /// The next body to write, and the address of the stream to write it on.
    fn wifi_direct_get_next_message(&mut self) -> Option<(String, Vec<u8>)>;
    /// The announced stream for `peer_id` ended.
    fn wifi_direct_peer_disconnected(&mut self, peer_id: &str);
}

/// What the bridge decided, for the host to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A stream proved `address` and holds it now. `superseded` is the
    /// older dialled stream it replaced, which was closed without a loss.
    Proved {
        stream: StreamId,
        address: String,
        remote: SocketAddr,
        superseded: Option<StreamId>,
    },
    /// A proved stream was closed and never announced.
    Refused {
        stream: StreamId,
        address: String,
        reason: &'static str,
    },
    /// The stream holding `address` ended, and the engine was told.
    Lost { stream: StreamId, address: String },
    /// An outbound body could not be written.
    Dropped { recipient: String, reason: String },
}

/// One announced stream.
struct Held {
    address: String,
    remote: IpAddr,
}

/// A listener and the stream table in front of the engine's slot.
pub struct Bridge {
    listener: Listener,
    local_address: String,
    /// Every announced stream. A stream that is not here is never
    /// attributed: its bodies and its end are ignored.
    streams: HashMap<StreamId, Held>,
    /// The one stream holding each announced address.
    by_address: HashMap<String, StreamId>,
    notes: VecDeque<Note>,
    up: bool,
}

impl Bridge {
    /// Opens the listener and reports the slot's layer up.
    pub fn open(
        slot: &mut impl Slot,
        config: ListenerConfig,
        now: Instant,
    ) -> Result<Self, ListenerError> {
        let local_address = config.address.clone();
        let listener = Listener::new(config, now)?;
        slot.wifi_direct_status_changed(true);
        Ok(Self {
            listener,
            local_address,
            streams: HashMap::new(),
            by_address: HashMap::new(),
            notes: VecDeque::new(),
            up: true,
        })
    }

    /// The code a page dials.
    pub fn dial_code(&self) -> &DialCode {
        self.listener.dial_code()
    }

    /// One datagram from the socket.
    pub fn handle_datagram(
        &mut self,
        slot: &mut impl Slot,
        now: Instant,
        source: SocketAddr,
        bytes: &[u8],
    ) {
        self.listener.handle_datagram(now, source, bytes);
        self.pump(slot);
    }

    /// Fires the listener's timers if they are due.
    pub fn handle_timeout(&mut self, slot: &mut impl Slot, now: Instant) {
        if self.listener.next_timeout().is_some_and(|at| at <= now) {
            self.listener.handle_timeout(now);
        }
        self.pump(slot);
    }

    /// When [`Self::handle_timeout`] must next run, or `None` while nothing
    /// is timed.
    pub fn next_timeout(&self) -> Option<Instant> {
        self.listener.next_timeout()
    }

    /// The next datagram for the socket.
    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.listener.poll_transmit()
    }

    /// The next decision to log.
    pub fn poll_note(&mut self) -> Option<Note> {
        self.notes.pop_front()
    }

    /// Streams the bridge holds.
    #[allow(dead_code)] // Read by tests/example_engine.rs.
    pub fn streams(&self) -> usize {
        self.streams.len()
    }

    /// Closes the listener: every held stream is reported lost, then the
    /// layer goes down. The code is dead from here on.
    pub fn close(&mut self, slot: &mut impl Slot) {
        if !self.up {
            return;
        }
        self.listener.close();
        self.events(slot);
        self.up = false;
        slot.wifi_direct_status_changed(false);
    }

    /// Hands the listener's events to the slot and the slot's outbound
    /// bodies to the listener. Call it after anything that fed the listener
    /// and after anything that may have queued a message in the engine.
    pub fn pump(&mut self, slot: &mut impl Slot) {
        self.events(slot);
        self.outbound(slot);
        // A send past the queue limit closes its stream.
        self.events(slot);
    }

    fn events(&mut self, slot: &mut impl Slot) {
        while let Some(event) = self.listener.poll_event() {
            match event {
                ListenerEvent::Proved {
                    stream,
                    address,
                    remote,
                } => self.proved(slot, stream, address, remote),
                ListenerEvent::Body { stream, bytes } => {
                    if let Some(held) = self.streams.get(&stream) {
                        slot.wifi_direct_message_received(&held.address, bytes);
                    }
                }
                ListenerEvent::Closed { stream } => {
                    // A refused or superseded stream was never in the table,
                    // or left it when it was replaced: no loss for it.
                    let Some(held) = self.streams.remove(&stream) else {
                        continue;
                    };
                    self.by_address.remove(&held.address);
                    slot.wifi_direct_peer_disconnected(&held.address);
                    self.notes.push_back(Note::Lost {
                        stream,
                        address: held.address,
                    });
                }
            }
        }
    }

    fn proved(
        &mut self,
        slot: &mut impl Slot,
        stream: StreamId,
        address: String,
        remote: SocketAddr,
    ) {
        if address == self.local_address {
            return self.refuse(
                stream,
                address,
                "the stream proves this device's own address",
            );
        }
        // Every held stream is dialled, so the newer one wins.
        let superseded = self.by_address.get(&address).copied();
        // The caps count what is held once the older stream is gone.
        let kept = |id: &StreamId| Some(*id) != superseded;
        let held = self.streams.keys().filter(|id| kept(id)).count();
        // A dialled stream is inbound; this host holds no other kind.
        let inbound = held;
        let from_ip = self
            .streams
            .iter()
            .filter(|(id, h)| kept(id) && h.remote == remote.ip())
            .count();
        if held >= MAX_STREAMS || inbound >= MAX_INBOUND_STREAMS {
            return self.refuse(stream, address, "the slot holds its most streams");
        }
        if from_ip >= MAX_STREAMS_PER_REMOTE_IP {
            return self.refuse(stream, address, "the remote IP holds its most streams");
        }

        if let Some(older) = superseded {
            // Out of the table first, so its Closed reports nothing.
            self.streams.remove(&older);
            self.listener.close_stream(older);
        }
        self.streams.insert(
            stream,
            Held {
                address: address.clone(),
                remote: remote.ip(),
            },
        );
        self.by_address.insert(address.clone(), stream);
        if superseded.is_none() {
            slot.wifi_direct_peer_connected(&address);
        }
        self.notes.push_back(Note::Proved {
            stream,
            address,
            remote,
            superseded,
        });
    }

    fn refuse(&mut self, stream: StreamId, address: String, reason: &'static str) {
        // Never in the table, so the Closed that follows reports nothing.
        self.listener.close_stream(stream);
        self.notes.push_back(Note::Refused {
            stream,
            address,
            reason,
        });
    }

    fn outbound(&mut self, slot: &mut impl Slot) {
        if !self.up {
            return;
        }
        while let Some((recipient, body)) = slot.wifi_direct_get_next_message() {
            // The engine queues only for a proved recipient, so a miss is a
            // stream that ended after the engine queued for it.
            let Some(&stream) = self.by_address.get(&recipient) else {
                self.notes.push_back(Note::Dropped {
                    recipient,
                    reason: "no stream proves the recipient".to_string(),
                });
                continue;
            };
            if let Err(error) = self.listener.send(stream, &body) {
                self.notes.push_back(Note::Dropped {
                    recipient,
                    reason: error.to_string(),
                });
            }
        }
    }
}
