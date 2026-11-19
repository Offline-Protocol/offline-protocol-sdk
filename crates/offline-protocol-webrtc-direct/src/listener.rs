//! The listener: one UDP port, one certificate, one ICE username and
//! password, and many dials.
//!
//! Sans-I/O. The host owns the socket and the clock: it hands every
//! datagram to [`Listener::handle_datagram`], calls
//! [`Listener::handle_timeout`] at [`Listener::next_timeout`], sends what
//! [`Listener::poll_transmit`] yields, and acts on what
//! [`Listener::poll_event`] yields.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_core::Address;
use offline_protocol_transport::stream_framing::{frame, PeerStreamReader, StreamEvent};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::config::{CryptoProvider, DtlsCert, Fingerprint};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, IceCreds, Input, Output, Rtc, RtcConfig};

use crate::dial_code::{check_host, DialCode, DialCodeError};
use crate::nomination;
use crate::stun::{self, Authenticated};

/// At most this many dials may be authenticated and not yet proved.
pub const MAX_PENDING_DIALS: usize = 4;

/// A dial that has not proved an address this long after its first
/// admitted check is dropped.
pub const PROVE_DEADLINE: Duration = Duration::from_secs(10);

/// A stream with no admitted check for this long is ended: the page's
/// consent checks are the only liveness signal an ICE-lite agent receives.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(15);

/// The largest chunk written to the channel. Every browser accepts it.
pub const MAX_CHUNK_BYTES: usize = 16 * 1024;

/// The most the channel holds unacknowledged before writes wait.
pub const HIGH_WATER_BYTES: usize = 1024 * 1024;

/// A stream with more than this waiting behind the channel is closed, so a
/// stalled page holds the listener's memory to about 9 MiB per stream.
pub const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;

/// The negotiated channel the stream runs on.
const STREAM_CHANNEL: u16 = 0;

/// What a listener writes into its code, from a CSPRNG.
const UFRAG_CHARS: usize = 16;
const PASSWORD_CHARS: usize = 32;
/// How many ended dials' page usernames a listener refuses. A browser
/// makes new credentials for every connection, so the bound only limits
/// memory.
const REMEMBERED_ENDED_DIALS: usize = 64;

const ICE_ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// One dial, as the host names it. Allocated when a dial is admitted, and
/// first shown to the host in [`ListenerEvent::Proved`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(pub u64);

/// What a listener needs from its host.
#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// The address the platform's UDP socket is bound to: a private or
    /// link-local IP and the real port, never a wildcard. It goes into the
    /// code, and str0m answers only checks addressed to it.
    pub local: SocketAddr,
    /// This device's canonical address, the claim the code carries.
    pub address: String,
    /// This device's identity assertion, unframed. It must prove `address`.
    pub preamble: Vec<u8>,
}

/// A datagram for the host to send from the listener's socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transmit {
    /// Where to send it.
    pub destination: SocketAddr,
    /// The datagram.
    pub bytes: Vec<u8>,
}

/// What a listener tells its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerEvent {
    /// A dial's preamble verified. The stream carries this address from
    /// now on: what `wifi_direct_peer_connected` takes.
    Proved {
        /// The stream.
        stream: StreamId,
        /// The address the preamble proved, derived and never claimed.
        address: String,
        /// Where the dial's first admitted check came from. A bridge counts
        /// streams per remote IP against its caps with it; it is never an
        /// identity.
        remote: SocketAddr,
    },
    /// One message body from a proved stream, undecoded.
    Body {
        /// The stream.
        stream: StreamId,
        /// Exactly one message.
        bytes: Vec<u8>,
    },
    /// A proved stream ended, for any reason, including the host's own
    /// [`Listener::close_stream`] and [`Listener::close`]. Emitted exactly
    /// once for every stream that was proved, and never for one that was
    /// not. What the listener queued to tell the page is in
    /// [`Listener::poll_transmit`] by then.
    Closed {
        /// The stream.
        stream: StreamId,
    },
}

/// Why a listener refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ListenerError {
    /// The local address is not one a dial code may name.
    #[error("the listener's address cannot go in a dial code: {0}")]
    LocalAddress(DialCodeError),
    /// The local address names port 0, or str0m cannot use it as a host
    /// candidate (an IPv4 link-local address, for one).
    #[error("the listener's socket address is not usable")]
    Socket,
    /// The configured address is not a canonical address.
    #[error("the listener's address is not a canonical address")]
    Address,
    /// The preamble does not verify, or proves another address.
    #[error("the preamble does not prove the listener's address")]
    Preamble,
    /// The crypto provider could not make a certificate.
    #[error("no DTLS certificate could be generated")]
    Certificate,
    /// The system's random source failed.
    #[error("the system random source failed")]
    Entropy,
    /// No proved stream has this id.
    #[error("no proved stream has this id")]
    UnknownStream,
    /// The body cannot be one frame: empty, or over the ceiling.
    #[error("the body cannot be framed: {0}")]
    Frame(String),
    /// More than [`MAX_QUEUED_BYTES`] would wait behind the channel. The
    /// stream is closed and [`ListenerEvent::Closed`] follows.
    #[error("the stream's outbound queue is full, and the stream is closed")]
    Overflow,
    /// The listener is closed.
    #[error("the listener is closed")]
    Closed,
}

/// A WebRTC Direct listener. See the crate documentation.
pub struct Listener {
    provider: Arc<CryptoProvider>,
    cert: DtlsCert,
    candidate: Candidate,
    code: DialCode,
    /// The listener's own fingerprint. str0m refuses to start DTLS without
    /// a remote one even with verification off, and the page's is unknown.
    placeholder_fingerprint: Fingerprint,
    framed_preamble: Vec<u8>,
    dials: Vec<Dial>,
    /// Page usernames of dials that ended, newest last.
    ended: VecDeque<String>,
    next_stream: u64,
    transmits: VecDeque<Transmit>,
    events: VecDeque<ListenerEvent>,
    /// The latest time the host has given. Time handed to str0m never
    /// goes backwards.
    now: Instant,
    closed: bool,
}

impl core::fmt::Debug for Listener {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Listener")
            .field("code", &self.code)
            .field("pending_dials", &self.pending_dials())
            .field("streams", &self.streams())
            .field("closed", &self.closed)
            .finish()
    }
}

/// How a dial's state machine ended.
struct End;

struct Dial {
    stream: StreamId,
    rtc: Rtc,
    channel: ChannelId,
    /// The page's own ICE username, the second half of every STUN USERNAME
    /// it sends. A check is routed by it, never by its source alone.
    remote_ufrag: String,
    /// The source of the first admitted check.
    remote: SocketAddr,
    first_check: Instant,
    last_check: Instant,
    rtc_timeout: Instant,
    open: bool,
    reader: PeerStreamReader,
    proved: Option<String>,
    outbound: VecDeque<Vec<u8>>,
    queued: usize,
}

/// The parts of the listener a dial writes to while it runs.
struct Outbox<'a> {
    transmits: &'a mut VecDeque<Transmit>,
    events: &'a mut VecDeque<ListenerEvent>,
    framed_preamble: &'a [u8],
}

impl Listener {
    /// Opens a listener with a fresh certificate, ICE username and password.
    ///
    /// # Errors
    ///
    /// A [`ListenerError`] naming what in `config` cannot be used, or that
    /// the platform could not make a certificate or random credentials.
    pub fn new(config: ListenerConfig, now: Instant) -> Result<Self, ListenerError> {
        check_host(config.local.ip()).map_err(ListenerError::LocalAddress)?;
        if config.local.port() == 0 {
            return Err(ListenerError::Socket);
        }
        let candidate = Candidate::host(config.local, "udp").map_err(|_| ListenerError::Socket)?;
        let address = Address::from_str(&config.address).map_err(|_| ListenerError::Address)?;
        let proved = offline_protocol_mls::verify_identity_assertion(&config.preamble)
            .map_err(|_| ListenerError::Preamble)?;
        if proved != address {
            return Err(ListenerError::Preamble);
        }
        let framed_preamble = frame(&config.preamble).map_err(|_| ListenerError::Preamble)?;

        let provider = str0m::crypto::from_feature_flags();
        let cert = provider
            .dtls_provider
            .generate_certificate()
            .ok_or(ListenerError::Certificate)?;
        let certhash = provider.sha256_provider.sha256(&cert.certificate);
        let code = DialCode {
            host: config.local.ip(),
            port: config.local.port(),
            certhash,
            ufrag: random_alnum(UFRAG_CHARS)?,
            password: random_alnum(PASSWORD_CHARS)?,
            address: config.address,
        };
        Ok(Self {
            provider: Arc::new(provider),
            cert,
            candidate,
            code,
            placeholder_fingerprint: Fingerprint {
                hash_func: "sha-256".to_string(),
                bytes: certhash.to_vec(),
            },
            framed_preamble,
            dials: Vec::new(),
            ended: VecDeque::new(),
            next_stream: 0,
            transmits: VecDeque::new(),
            events: VecDeque::new(),
            now,
            closed: false,
        })
    }

    /// The code a page dials this listener with.
    pub fn dial_code(&self) -> &DialCode {
        &self.code
    }

    /// One datagram the socket received from `source`.
    ///
    /// A datagram that belongs to no dial is admitted only if it is a
    /// binding request for this listener's ufrag that verifies under its
    /// password; anything else is dropped with no reply and no state.
    pub fn handle_datagram(&mut self, now: Instant, source: SocketAddr, bytes: &[u8]) {
        if self.closed {
            return;
        }
        let now = self.advance(now);
        self.sweep(now);

        // A binding request must authenticate before anything else reads
        // it; every other datagram can only reach an existing dial.
        let request = match stun::parse_binding_request(bytes) {
            None => None,
            Some(parsed) => match parsed.authenticate(
                bytes,
                &self.code.ufrag,
                &self.code.password,
                self.provider.sha1_hmac_provider,
            ) {
                Some(admitted) => Some(admitted),
                None => return,
            },
        };
        let Ok(receive) = Receive::new(Protocol::Udp, source, self.code_local(), bytes) else {
            return;
        };
        let input = Input::Receive(now, receive);

        // A check goes to the dial of the page that signed it. str0m's
        // `accepts` takes anything from a dial's nominated source first, so
        // on its own it would hand a second page behind the same address
        // to the first page's dial.
        let found = self.dials.iter().position(|dial| {
            request
                .as_ref()
                .is_none_or(|r| r.request().remote_ufrag == dial.remote_ufrag)
                && dial.rtc.accepts(&input)
        });
        if let Some(index) = found {
            let dial = &mut self.dials[index];
            if request.is_some() {
                dial.last_check = now;
            }
            let fed = dial.rtc.handle_input(input).is_ok();
            self.run(index, fed);
            return;
        }
        if let Some(request) = request {
            self.admit(now, source, bytes, &request);
        }
    }

    /// Advances every dial to `now`: str0m's timers, the proving deadline
    /// and the consent timeout.
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.closed {
            return;
        }
        let now = self.advance(now);
        self.sweep(now);
        let mut index = 0;
        while index < self.dials.len() {
            let fed = self.dials[index]
                .rtc
                .handle_input(Input::Timeout(now))
                .is_ok();
            if self.run(index, fed) {
                index += 1;
            }
        }
    }

    /// When the host must next call [`Listener::handle_timeout`], or `None`
    /// while there is nothing to time.
    pub fn next_timeout(&self) -> Option<Instant> {
        self.dials.iter().map(Dial::next_timeout).min()
    }

    /// The next datagram to send.
    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.transmits.pop_front()
    }

    /// The next event for the host.
    pub fn poll_event(&mut self) -> Option<ListenerEvent> {
        self.events.pop_front()
    }

    /// Sends one message body on a proved stream: framed, cut into chunks
    /// of at most [`MAX_CHUNK_BYTES`], and queued behind the channel.
    ///
    /// # Errors
    ///
    /// [`ListenerError::UnknownStream`] for a stream that is not proved or
    /// has ended, [`ListenerError::Frame`] for a body that cannot be one
    /// frame, and [`ListenerError::Overflow`] when the queue would pass
    /// [`MAX_QUEUED_BYTES`], which also closes the stream.
    pub fn send(&mut self, stream: StreamId, body: &[u8]) -> Result<(), ListenerError> {
        if self.closed {
            return Err(ListenerError::Closed);
        }
        let index = self
            .dials
            .iter()
            .position(|dial| dial.stream == stream && dial.proved.is_some())
            .ok_or(ListenerError::UnknownStream)?;
        let framed = frame(body).map_err(|e| ListenerError::Frame(e.to_string()))?;
        if self.dials[index].queued + framed.len() > MAX_QUEUED_BYTES {
            tracing::debug!(
                stream = stream.0,
                "webrtc direct: outbound queue full, closing"
            );
            self.end(index);
            return Err(ListenerError::Overflow);
        }
        self.dials[index].enqueue(&framed);
        self.run(index, true);
        Ok(())
    }

    /// Ends one stream. [`ListenerEvent::Closed`] follows if it was proved.
    pub fn close_stream(&mut self, stream: StreamId) {
        if let Some(index) = self.dials.iter().position(|dial| dial.stream == stream) {
            self.end(index);
        }
    }

    /// Closes the listener: every stream ends, with
    /// [`ListenerEvent::Closed`] for each that was proved, and no datagram
    /// is admitted again. The code is dead from here on. The host sends
    /// what [`Listener::poll_transmit`] still yields before it closes the
    /// socket: that is each page's channel closing.
    pub fn close(&mut self) {
        while !self.dials.is_empty() {
            self.end(0);
        }
        self.closed = true;
    }

    /// Dials that are authenticated and have not yet proved an address.
    pub fn pending_dials(&self) -> usize {
        self.dials.iter().filter(|d| d.proved.is_none()).count()
    }

    /// Streams that have proved an address and not ended.
    pub fn streams(&self) -> usize {
        self.dials.iter().filter(|d| d.proved.is_some()).count()
    }

    fn code_local(&self) -> SocketAddr {
        SocketAddr::new(self.code.host, self.code.port)
    }

    fn advance(&mut self, now: Instant) -> Instant {
        if now > self.now {
            self.now = now;
        }
        self.now
    }

    /// Admits a new dial for an authenticated request no dial took.
    fn admit(
        &mut self,
        now: Instant,
        source: SocketAddr,
        bytes: &[u8],
        request: &Authenticated<'_>,
    ) {
        let remote_ufrag = request.request().remote_ufrag;
        if self.ended.iter().any(|ufrag| ufrag == remote_ufrag) {
            return;
        }
        if self.pending_dials() >= MAX_PENDING_DIALS {
            tracing::debug!("webrtc direct: refused a dial, {MAX_PENDING_DIALS} are pending");
            return;
        }
        let nominating = nomination::nominating(
            bytes,
            request,
            &self.code.password,
            self.provider.sha1_hmac_provider,
        );
        let datagram = nominating.as_deref().unwrap_or(bytes);
        let Ok(receive) = Receive::new(Protocol::Udp, source, self.code_local(), datagram) else {
            return;
        };
        let input = Input::Receive(now, receive);
        let Some((mut rtc, channel)) = self.build_rtc(now, remote_ufrag) else {
            return;
        };
        if !rtc.accepts(&input) || rtc.handle_input(input).is_err() {
            return;
        }
        let stream = StreamId(self.next_stream);
        self.next_stream += 1;
        tracing::debug!(stream = stream.0, "webrtc direct: admitted a dial");
        let verify = Box::new(|body: &[u8]| {
            offline_protocol_mls::verify_identity_assertion(body)
                .ok()
                .map(|address| address.to_string())
        });
        self.dials.push(Dial {
            stream,
            rtc,
            channel,
            remote_ufrag: remote_ufrag.to_string(),
            remote: source,
            first_check: now,
            last_check: now,
            rtc_timeout: now,
            open: false,
            reader: PeerStreamReader::new(verify),
            proved: None,
            outbound: VecDeque::new(),
            queued: 0,
        });
        self.run(self.dials.len() - 1, true);
    }

    /// An ICE-lite, controlled, DTLS-server `Rtc` for one page, with the
    /// stream's channel negotiated.
    fn build_rtc(&self, now: Instant, remote_ufrag: &str) -> Option<(Rtc, ChannelId)> {
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(Arc::clone(&self.provider))
            .set_ice_lite(true)
            .set_dtls_cert(self.cert.clone())
            // The page's certificate is unknown and proves nothing this
            // listener relies on: the preamble does that.
            .set_fingerprint_verification(false)
            .set_local_ice_credentials(IceCreds {
                ufrag: self.code.ufrag.clone(),
                pass: self.code.password.clone(),
            })
            .set_sctp_max_buffered_amount(HIGH_WATER_BYTES)
            .build(now);
        rtc.add_local_candidate(self.candidate.clone())?;
        let mut api = rtc.direct_api();
        // An ICE-lite agent sends no checks, so the page's password is
        // never used; str0m wants a value.
        api.set_remote_ice_credentials(IceCreds {
            ufrag: remote_ufrag.to_string(),
            pass: remote_ufrag.to_string(),
        });
        api.set_ice_controlling(false);
        api.set_remote_fingerprint(self.placeholder_fingerprint.clone());
        api.start_dtls(false).ok()?;
        api.start_sctp(false);
        let channel = api.create_data_channel(ChannelConfig {
            label: String::new(),
            ordered: true,
            reliability: Reliability::Reliable,
            negotiated: Some(STREAM_CHANNEL),
            protocol: String::new(),
        });
        Some((rtc, channel))
    }

    /// Drops dials past a deadline: silently for one that never proved,
    /// with [`ListenerEvent::Closed`] for a stream whose consent lapsed.
    fn sweep(&mut self, now: Instant) {
        let mut index = 0;
        while index < self.dials.len() {
            let dial = &self.dials[index];
            if now >= dial.deadline() {
                tracing::debug!(
                    stream = dial.stream.0,
                    proved = dial.proved.is_some(),
                    "webrtc direct: dial timed out"
                );
                self.end(index);
            } else {
                index += 1;
            }
        }
    }

    /// Pumps dial `index` after an input. Returns whether it is still
    /// there; a dial whose input failed or whose state machine ended is
    /// removed.
    fn run(&mut self, index: usize, fed: bool) -> bool {
        let mut out = Outbox {
            transmits: &mut self.transmits,
            events: &mut self.events,
            framed_preamble: &self.framed_preamble,
        };
        let dial = &mut self.dials[index];
        let alive = fed && dial.pump(&mut out).is_ok() && dial.rtc.is_alive();
        if !alive {
            self.end(index);
        }
        alive
    }

    /// Removes dial `index`, queues its connection's close for the page
    /// (see [`Dial::close_connection`]), and emits [`ListenerEvent::Closed`]
    /// if it was proved.
    ///
    /// The page's username is remembered, so its later checks are refused
    /// rather than admitted as a new dial. A page whose close was lost goes
    /// on checking, and admitting those checks would answer them from a
    /// fresh `Rtc` that never completes DTLS, so the page would never learn
    /// its stream had ended, and it would
    /// hold a pending slot again every ten seconds. A page that dials again
    /// has a new username, because a browser makes new ICE credentials for
    /// every connection.
    fn end(&mut self, index: usize) {
        let mut dial = self.dials.remove(index);
        dial.close_connection(&mut self.transmits);
        if self.ended.len() == REMEMBERED_ENDED_DIALS {
            self.ended.pop_front();
        }
        self.ended.push_back(dial.remote_ufrag);
        if dial.proved.is_some() {
            self.events.push_back(ListenerEvent::Closed {
                stream: dial.stream,
            });
        }
    }
}

impl Dial {
    /// The earlier of the proving deadline (pending) or the consent
    /// timeout (proved).
    fn deadline(&self) -> Instant {
        match self.proved {
            None => self.first_check + PROVE_DEADLINE,
            Some(_) => self.last_check + CONSENT_TIMEOUT,
        }
    }

    fn next_timeout(&self) -> Instant {
        self.rtc_timeout.min(self.deadline())
    }

    /// Closes the connection and queues what that sends, so the page closes
    /// its link now.
    ///
    /// Without it the page learned of an end only when its consent checks
    /// went unanswered: "disconnected", then its own deadline, 15 to 20 s on
    /// every engine. Until then it held a link it believed was up, sent into
    /// it, and (for a stream the host refused at its caps) had already
    /// announced the listener and sent its key package to nobody.
    ///
    /// str0m's close sends the DTLS close_notify (and asks SCTP to shut
    /// down) without waiting for a reply, which is what a dial about to be
    /// dropped needs. Closing only the channel is not enough: its SCTP
    /// stream reset waits until the page acknowledges every byte already
    /// sent on it, and no acknowledgement reaches a dropped dial. Only on an
    /// open channel of a connection still alive: before the channel opens
    /// the page holds no link, only a dial its own timeout ends, and a dead
    /// connection sends nothing.
    fn close_connection(&mut self, transmits: &mut VecDeque<Transmit>) {
        if !self.open || !self.rtc.is_alive() || self.rtc.close().is_err() {
            return;
        }
        // Until the close has drained; events are for a dial that is gone.
        while let Ok(output) = self.rtc.poll_output() {
            match output {
                Output::Transmit(t) => transmits.push_back(Transmit {
                    destination: t.destination,
                    bytes: t.contents.to_vec(),
                }),
                Output::Event(_) => {}
                Output::Timeout(_) => break,
            }
        }
    }

    fn enqueue(&mut self, framed: &[u8]) {
        for chunk in framed.chunks(MAX_CHUNK_BYTES) {
            self.outbound.push_back(chunk.to_vec());
        }
        self.queued += framed.len();
    }

    /// Drains str0m's output, writes what the drain made room for, and
    /// drains again until a write adds nothing: a write becomes datagrams
    /// only on the next poll.
    fn pump(&mut self, out: &mut Outbox<'_>) -> Result<(), End> {
        loop {
            self.drain(out)?;
            if !self.flush()? {
                return Ok(());
            }
        }
    }

    fn drain(&mut self, out: &mut Outbox<'_>) -> Result<(), End> {
        loop {
            match self.rtc.poll_output().map_err(|_| End)? {
                Output::Timeout(at) => {
                    self.rtc_timeout = at;
                    return Ok(());
                }
                Output::Transmit(t) => out.transmits.push_back(Transmit {
                    destination: t.destination,
                    bytes: t.contents.to_vec(),
                }),
                Output::Event(event) => self.on_event(event, out)?,
            }
        }
    }

    fn on_event(&mut self, event: Event, out: &mut Outbox<'_>) -> Result<(), End> {
        match event {
            Event::ChannelOpen(id, _) if id == self.channel => {
                // Our preamble goes first, without waiting for the page's,
                // so a page that never speaks cannot hold the stream
                // half-open on purpose.
                self.open = true;
                let preamble = out.framed_preamble.to_vec();
                self.enqueue(&preamble);
            }
            Event::ChannelOpen(id, _) => self.rtc.direct_api().close_data_channel(id),
            Event::ChannelData(data) if data.id == self.channel => {
                for event in self.reader.feed(&data.data) {
                    match event {
                        StreamEvent::Announced(address) => {
                            tracing::debug!(stream = self.stream.0, "webrtc direct: dial proved");
                            self.proved = Some(address.clone());
                            out.events.push_back(ListenerEvent::Proved {
                                stream: self.stream,
                                address,
                                remote: self.remote,
                            });
                        }
                        StreamEvent::Message { body, .. } => {
                            out.events.push_back(ListenerEvent::Body {
                                stream: self.stream,
                                bytes: body,
                            });
                        }
                        StreamEvent::Closed(reason) => {
                            tracing::debug!(
                                stream = self.stream.0,
                                %reason,
                                "webrtc direct: stream refused"
                            );
                            return Err(End);
                        }
                    }
                }
            }
            Event::ChannelData(data) => self.rtc.direct_api().close_data_channel(data.id),
            Event::ChannelClose(id) if id == self.channel => return Err(End),
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => return Err(End),
            _ => {}
        }
        Ok(())
    }

    /// Writes queued chunks while the channel takes them. Returns whether
    /// anything was written.
    fn flush(&mut self) -> Result<bool, End> {
        if !self.open || self.outbound.is_empty() {
            return Ok(false);
        }
        let Some(mut channel) = self.rtc.channel(self.channel) else {
            return Ok(false);
        };
        let mut wrote = false;
        while let Some(chunk) = self.outbound.front() {
            if !channel.write(true, chunk).map_err(|_| End)? {
                break;
            }
            self.queued -= chunk.len();
            self.outbound.pop_front();
            wrote = true;
        }
        Ok(wrote)
    }
}

/// `n` characters of `A-Z a-z 0-9` from the system CSPRNG, by rejection
/// sampling so every character is equally likely.
fn random_alnum(n: usize) -> Result<String, ListenerError> {
    // 248 is the largest multiple of 62 a byte can hold.
    const LIMIT: u8 = 248;
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 64];
    while out.len() < n {
        getrandom::getrandom(&mut buf).map_err(|_| ListenerError::Entropy)?;
        for byte in buf {
            if out.len() == n {
                break;
            }
            if byte < LIMIT {
                out.push(char::from(ICE_ALPHABET[usize::from(byte % 62)]));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A bridge holds the listener behind a lock and drives it from its
    /// socket thread.
    #[test]
    fn a_listener_can_move_between_threads() {
        fn send<T: Send>() {}
        send::<Listener>();
    }

    #[test]
    fn credentials_are_alphanumeric_of_the_written_length() {
        let ufrag = random_alnum(UFRAG_CHARS).unwrap();
        let password = random_alnum(PASSWORD_CHARS).unwrap();
        assert_eq!(ufrag.len(), 16);
        assert_eq!(password.len(), 32);
        assert!(ufrag
            .bytes()
            .chain(password.bytes())
            .all(|b| b.is_ascii_alphanumeric()));
        assert_ne!(random_alnum(PASSWORD_CHARS).unwrap(), password);
    }
}
