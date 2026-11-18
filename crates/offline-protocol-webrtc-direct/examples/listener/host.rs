//! One host: an engine, a listener bridged into its slot, the tick, and the
//! test host's habits (accept every connection request, echo every direct
//! message). Socket-free, so the example's loop and the test drive the
//! same code; the caller moves datagrams and reads the clock.

use std::net::SocketAddr;

use offline_protocol::Event;
use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_webrtc_direct::{ListenerConfig, Transmit};
use serde_json::{json, Value};

use crate::bridge::{Bridge, Note};
use crate::engine::Engine;

/// How often the engine ticks, as every bridge does.
pub const TICK: Duration = Duration::from_millis(100);

/// The prefix of an echo, which is never echoed back.
pub const ECHO: &str = "echo:";

/// A message's text is printed only up to this many bytes; `len` is always.
pub const MAX_PRINTED_TEXT: usize = 4096;

/// One line of the example's output.
#[derive(Debug, Clone, PartialEq)]
pub struct Line(pub Value);

impl Line {
    fn new(kind: &str, mut fields: Value) -> Self {
        if let Value::Object(map) = &mut fields {
            map.insert("type".to_string(), json!(kind));
        }
        Self(fields)
    }

    /// An error line.
    pub fn error(reason: impl Into<String>) -> Self {
        Self::new("error", json!({ "reason": reason.into() }))
    }
}

/// What a command asked for.
pub enum Command {
    /// Keep running; print these.
    Continue(Vec<Line>),
    /// Exit.
    Quit,
}

/// An engine and its listener.
pub struct Host {
    pub engine: Engine,
    pub bridge: Bridge,
    next_tick: Instant,
}

impl Host {
    /// Starts an engine with a fresh identity and opens a listener on
    /// `local`, the address the socket is bound to.
    pub fn open(
        app_id: &str,
        profile: &str,
        local: SocketAddr,
        now: Instant,
    ) -> Result<Self, String> {
        let mut engine = Engine::new(app_id, profile)?;
        let config = ListenerConfig {
            local,
            address: engine.address().to_string(),
            preamble: engine.identity_assertion()?,
        };
        let bridge = Bridge::open(&mut engine, config, now).map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            bridge,
            next_tick: now,
        })
    }

    /// The first line: the code and where it points.
    pub fn listening(&self) -> Line {
        let code = self.bridge.dial_code();
        Line::new(
            "listening",
            json!({
                "code": code.to_fragment(),
                "address": code.address,
                "host": code.host.to_string(),
                "port": code.port,
            }),
        )
    }

    /// One datagram from the socket. Follow it with [`Self::turn`].
    pub fn handle_datagram(&mut self, now: Instant, source: SocketAddr, bytes: &[u8]) {
        self.bridge
            .handle_datagram(&mut self.engine, now, source, bytes);
    }

    /// Everything due at `now`: the listener's timers, the tick, the
    /// engine's events, and the bodies the engine queued. Returns what to
    /// print.
    pub fn turn(&mut self, now: Instant) -> Vec<Line> {
        let mut lines = Vec::new();
        self.bridge.handle_timeout(&mut self.engine, now);
        if now >= self.next_tick {
            if let Err(error) = self.engine.tick() {
                eprintln!("tick: {error}");
            }
            self.next_tick = now + TICK;
        }
        self.react(&mut lines);
        // Echoes and acknowledgements the reaction queued go out now.
        self.bridge.pump(&mut self.engine);
        self.notes(&mut lines);
        lines
    }

    /// When [`Self::turn`] must next run.
    pub fn next_wake(&self) -> Instant {
        match self.bridge.next_timeout() {
            Some(at) if at < self.next_tick => at,
            _ => self.next_tick,
        }
    }

    /// The next datagram for the socket.
    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.bridge.poll_transmit()
    }

    /// One line of standard input.
    pub fn command(&mut self, input: &str) -> Command {
        let mut words = input.split_whitespace();
        let lines = match (words.next(), words.next()) {
            (None, _) => Vec::new(),
            (Some("quit"), _) => return Command::Quit,
            (Some("close"), _) => self.close(),
            (Some("send"), Some(to)) => {
                // The text is the rest of the line, spaces kept.
                let text = input
                    .trim_start()
                    .splitn(3, char::is_whitespace)
                    .nth(2)
                    .unwrap_or_default()
                    .trim_end();
                vec![self.send(to, text)]
            }
            (Some("big"), Some(to)) => match words.next().map(str::parse::<usize>) {
                Some(Ok(n)) => vec![self.send(to, &"x".repeat(n))],
                _ => vec![Line::error("usage: big <address> <bytes>")],
            },
            (Some(other), _) => vec![Line::error(format!("unknown command: {other}"))],
        };
        Command::Continue(lines)
    }

    /// Closes the listener: every stream ends and the engine hears each
    /// loss, then the slot's layer goes down.
    pub fn close(&mut self) -> Vec<Line> {
        let mut lines = Vec::new();
        self.bridge.close(&mut self.engine);
        self.notes(&mut lines);
        self.react(&mut lines);
        lines.push(Line::new("closed", json!({})));
        lines
    }

    fn send(&mut self, to: &str, text: &str) -> Line {
        let line = match self.engine.send_text(to, text) {
            Ok(id) => Line::new("sent", json!({ "id": id, "to": to, "len": text.len() })),
            Err(error) => Line::error(format!("send to {to}: {error}")),
        };
        self.bridge.pump(&mut self.engine);
        line
    }

    fn notes(&mut self, lines: &mut Vec<Line>) {
        while let Some(note) = self.bridge.poll_note() {
            lines.push(match note {
                Note::Proved {
                    stream,
                    address,
                    remote,
                    superseded,
                } => Line::new(
                    "proved",
                    json!({
                        "stream": stream.0,
                        "address": address,
                        "remote": remote.to_string(),
                        "superseded": superseded.map(|s| s.0),
                    }),
                ),
                Note::Refused {
                    stream,
                    address,
                    reason,
                } => Line::new(
                    "refused",
                    json!({ "stream": stream.0, "address": address, "reason": reason }),
                ),
                Note::Lost { stream, address } => {
                    Line::new("lost", json!({ "stream": stream.0, "address": address }))
                }
                Note::Dropped { recipient, reason } => {
                    Line::new("dropped", json!({ "to": recipient, "reason": reason }))
                }
            });
        }
    }

    /// The engine's events, as lines, and the test host's answers to them.
    fn react(&mut self, lines: &mut Vec<Line>) {
        while let Some(event) = self.engine.poll_event() {
            match event {
                Event::NeighborDiscovered { peer_id, .. } => {
                    lines.push(Line::new("neighbor_discovered", json!({ "peer": peer_id })));
                }
                Event::NeighborLost { peer_id } => {
                    lines.push(Line::new("neighbor_lost", json!({ "peer": peer_id })));
                }
                Event::SecureSessionEstablished {
                    peer_id,
                    is_session: true,
                    ..
                } => lines.push(Line::new("session_ready", json!({ "peer": peer_id }))),
                Event::ConnectionRequestReceived { sender, .. } => {
                    lines.push(
                        match self.engine.accept_connection_request(&sender, "listener") {
                            Ok(()) => Line::new("accepted", json!({ "peer": sender })),
                            Err(error) => Line::error(format!("accept {sender}: {error}")),
                        },
                    );
                }
                Event::MessageReceived {
                    message_id,
                    sender,
                    content,
                    encrypted,
                    ..
                } => {
                    let mut fields = json!({
                        "id": message_id,
                        "from": sender,
                        "len": content.len(),
                        "encrypted": encrypted,
                    });
                    if content.len() <= MAX_PRINTED_TEXT {
                        fields["text"] = json!(content);
                    }
                    lines.push(Line::new("message", fields));
                    if !content.starts_with(ECHO) {
                        lines.push(self.send(&sender, &format!("{ECHO}{content}")));
                    }
                }
                Event::MessageDelivered { message_id, .. } => {
                    lines.push(Line::new("delivered", json!({ "id": message_id })));
                }
                Event::MessageFailed {
                    message_id, reason, ..
                } => lines.push(Line::new(
                    "failed",
                    json!({ "id": message_id, "reason": reason }),
                )),
                _ => {}
            }
        }
    }
}
