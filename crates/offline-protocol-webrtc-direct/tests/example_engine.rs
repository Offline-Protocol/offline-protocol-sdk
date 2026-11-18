//! The example listener's host loop (`examples/listener`) against a page
//! that runs a second full engine, in memory and in real time.
//!
//! The listener side is the example's own `Host`: an engine, a listener and
//! the bridge between them, compiled from the example's files. The page side
//! is the str0m page in `common`, carrying the second engine through the
//! same slot calls a browser node makes. Datagrams cross in memory with no
//! delay; the clock is the real one, because the engines' acknowledgement
//! and retry timers are.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

#[allow(dead_code)]
#[path = "../examples/listener/bridge.rs"]
mod bridge;
#[allow(dead_code)]
#[path = "../examples/listener/engine.rs"]
mod engine;
#[allow(dead_code)]
#[path = "../examples/listener/host.rs"]
mod host;

use std::net::SocketAddr;

use bridge::{Slot, MAX_STREAMS_PER_REMOTE_IP};
use common::{listener_addr, Identity, Page, PageSetup};
use engine::{Engine, MemoryStateStorage};
use host::{Host, Line, TICK};
use offline_protocol::Event;
use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_webrtc_direct::DialCode;

const APP: &str = "webrtc-direct-example";

/// The page's engine, and the page connections it dialled.
struct Visitor {
    engine: Engine,
    events: Vec<Event>,
    /// The connection the engine's link rides on.
    active: Option<usize>,
    announced: bool,
    next_tick: Instant,
}

struct World {
    host: Host,
    lines: Vec<Line>,
    pages: Vec<Page>,
    visitor: Visitor,
}

impl World {
    fn new() -> Self {
        let now = Instant::now();
        let host = Host::open(APP, "listener", listener_addr(), now).unwrap();
        let mut engine = Engine::new(APP, "visitor").unwrap();
        engine.wifi_direct_status_changed(true);
        World {
            host,
            lines: Vec::new(),
            pages: Vec::new(),
            visitor: Visitor {
                engine,
                events: Vec::new(),
                active: None,
                announced: false,
                next_tick: now,
            },
        }
    }

    fn listener_address(&self) -> String {
        self.host.engine.address().to_string()
    }

    fn visitor_address(&self) -> String {
        self.visitor.engine.address().to_string()
    }

    fn visitor_identity(&self) -> Identity {
        Identity {
            assertion: self.visitor.engine.identity_assertion().unwrap(),
            address: self.visitor_address(),
        }
    }

    /// Opens a page connection from the code, as a browser does.
    fn add_page(&mut self, setup: PageSetup) -> usize {
        let code = DialCode::parse(&self.host.bridge.dial_code().to_fragment()).unwrap();
        let index = self.pages.len();
        self.pages
            .push(Page::new(index, &code, &setup, Instant::now()));
        index
    }

    /// Dials with the visitor's identity and carries its engine's link on
    /// the new connection. Like the browser node, the page closes its
    /// earlier dialled link to the listener first, and reports it lost.
    fn visitor_dials(&mut self, addr: SocketAddr) -> usize {
        if let Some(old) = self.visitor.active.take() {
            self.pages[old].failed = true;
            let listener = self.listener_address();
            self.visitor.engine.wifi_direct_peer_disconnected(&listener);
            self.visitor.announced = false;
        }
        let identity = self.visitor_identity();
        let page = self.add_page(PageSetup {
            identity: Some(identity),
            addr: Some(addr),
            ..PageSetup::default()
        });
        self.visitor.active = Some(page);
        page
    }

    fn step(&mut self) {
        let now = Instant::now();
        for page in &mut self.pages {
            let mut wire = Vec::new();
            page.pump(&mut wire);
            for bytes in wire {
                self.host.handle_datagram(now, page.addr, &bytes);
            }
        }
        let lines = self.host.turn(now);
        self.lines.extend(lines);
        while let Some(t) = self.host.poll_transmit() {
            if let Some(page) = self.pages.iter_mut().find(|p| p.addr == t.destination) {
                page.receive(now, listener_addr(), &t.bytes);
            }
        }
        for page in &mut self.pages {
            page.handle_timeout(now);
        }

        // The page's half of the slot.
        let listener = self.listener_address();
        let visitor = &mut self.visitor;
        if let Some(page) = visitor.active.map(|i| &mut self.pages[i]) {
            if page.proved.is_some() && !visitor.announced {
                visitor.engine.wifi_direct_peer_connected(&listener);
                visitor.announced = true;
            }
            for body in page.bodies.drain(..) {
                visitor.engine.wifi_direct_message_received(&listener, body);
            }
        }
        if now >= visitor.next_tick {
            visitor.engine.tick().unwrap();
            visitor.next_tick = now + TICK;
        }
        while let Some((to, body)) = visitor.engine.wifi_direct_get_next_message() {
            let page = visitor.active.map(|i| &mut self.pages[i]);
            match page {
                Some(page) if to == listener && visitor.announced => page.send(&body),
                _ => panic!("the visitor's engine queued for {to} with no link to it"),
            }
        }
        while let Some(event) = visitor.engine.poll_event() {
            visitor.events.push(event);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    fn run_until(&mut self, limit: Duration, mut done: impl FnMut(&World) -> bool) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    fn lines(&self, kind: &str) -> Vec<&Line> {
        self.lines.iter().filter(|l| l.0["type"] == kind).collect()
    }

    fn count(&self, kind: &str, field: &str, value: &str) -> usize {
        self.lines(kind)
            .iter()
            .filter(|l| l.0[field] == value)
            .count()
    }

    fn host_got(&self, from: &str, text: &str) -> bool {
        self.lines("message")
            .iter()
            .any(|l| l.0["from"] == from && l.0["text"] == text && l.0["encrypted"] == true)
    }

    fn visitor_got(&self, text: &str) -> bool {
        self.visitor.events.iter().any(|e| {
            matches!(
                e,
                Event::MessageReceived { content, encrypted: true, .. } if content == text
            )
        })
    }

    fn visitor_session(&self, peer: &str) -> bool {
        self.visitor.events.iter().any(|e| {
            matches!(
                e,
                Event::SecureSessionEstablished { peer_id, .. } if peer_id == peer
            )
        })
    }
}

fn same_ip(port: u16) -> SocketAddr {
    SocketAddr::from(([192, 168, 1, 30], port))
}

/// Both engines announce each other, a session forms, a message crosses
/// each way, and a second dial from the same page identity supersedes the
/// first: the listener's engine hears one announcement and, when the
/// listener closes, one loss.
#[test]
fn a_page_engine_talks_to_the_listener_engine_and_a_redial_supersedes() {
    let mut world = World::new();
    let (listener, visitor) = (world.listener_address(), world.visitor_address());

    let first_page = world.visitor_dials(same_ip(50_000));
    assert!(
        world.run_until(Duration::from_secs(20), |w| {
            w.count("neighbor_discovered", "peer", &visitor) == 1
                && w.visitor.announced
                && w.count("session_ready", "peer", &visitor) == 1
                && w.visitor_session(&listener)
        }),
        "no announcement or session: {:?}",
        world.lines
    );
    let first = world.lines("proved")[0].0.clone();
    assert_eq!(first["address"], visitor.as_str());
    assert_eq!(first["remote"], world.pages[first_page].addr.to_string());
    assert!(first["superseded"].is_null());

    // Page to listener, and the listener's echo back.
    world
        .visitor
        .engine
        .send_text(&listener, "hello listener")
        .unwrap();
    assert!(
        world.run_until(Duration::from_secs(10), |w| {
            w.host_got(&visitor, "hello listener") && w.visitor_got("echo:hello listener")
        }),
        "{:?}",
        world.lines
    );
    // Listener to page, from a command.
    let sent = world.host.command(&format!("send {visitor} hello  page"));
    assert!(matches!(sent, host::Command::Continue(ref l) if l[0].0["type"] == "sent"));
    assert!(world.run_until(Duration::from_secs(10), |w| w.visitor_got("hello  page")));

    // The page dials again from the same identity: a reload.
    world.visitor_dials(same_ip(50_001));
    assert!(
        world.run_until(Duration::from_secs(10), |w| {
            w.lines("proved").len() == 2 && w.visitor.announced
        }),
        "{:?}",
        world.lines
    );
    let second = world.lines("proved")[1].0.clone();
    assert_eq!(second["address"], visitor.as_str());
    assert_eq!(
        second["superseded"], first["stream"],
        "the newer dialled stream replaced the older"
    );
    world
        .visitor
        .engine
        .send_text(&listener, "after the redial")
        .unwrap();
    assert!(
        world.run_until(Duration::from_secs(10), |w| {
            w.host_got(&visitor, "after the redial") && w.visitor_got("echo:after the redial")
        }),
        "the new stream carries both ways: {:?}",
        world.lines
    );
    assert_eq!(
        world.count("neighbor_discovered", "peer", &visitor),
        1,
        "one announcement for the address"
    );
    assert_eq!(world.lines("lost").len(), 0, "{:?}", world.lines);
    assert_eq!(world.count("neighbor_lost", "peer", &visitor), 0);
    assert_eq!(world.host.engine.linked_peers(), vec![visitor.clone()]);
    assert_eq!(world.host.bridge.streams(), 1);

    // Closing the listener ends the stream, and the engine hears it once.
    let closed = world.host.close();
    world.lines.extend(closed);
    world.run_until(Duration::from_millis(500), |_| false);
    let lost = world.lines("lost");
    assert_eq!(lost.len(), 1, "{:?}", world.lines);
    assert_eq!(lost[0].0["stream"], second["stream"]);
    assert_eq!(world.count("neighbor_lost", "peer", &visitor), 1);
    assert_eq!(world.lines("closed").len(), 1);
    assert!(world.host.engine.linked_peers().is_empty());
    assert_eq!(world.host.bridge.streams(), 0);
}

/// A stream past the per-IP cap proves its address and is closed without
/// the engine hearing of it; closing the listener reports only the held.
#[test]
fn a_stream_past_the_per_ip_cap_is_refused_and_never_announced() {
    let mut world = World::new();
    for n in 0..=MAX_STREAMS_PER_REMOTE_IP {
        let page = world.add_page(PageSetup {
            addr: Some(same_ip(51_000 + n as u16)),
            ..PageSetup::default()
        });
        let decided = n + 1;
        assert!(
            world.run_until(Duration::from_secs(10), |w| {
                w.pages[page].proved.is_some()
                    && w.lines("proved").len() + w.lines("refused").len() == decided
            }),
            "dial {n}: {:?}",
            world.lines
        );
    }
    assert_eq!(world.lines("proved").len(), MAX_STREAMS_PER_REMOTE_IP);
    let refused = world.lines("refused");
    assert_eq!(refused.len(), 1);
    assert_eq!(
        refused[0].0["address"],
        world.pages[MAX_STREAMS_PER_REMOTE_IP]
            .identity
            .address
            .as_str()
    );
    assert_eq!(
        world.lines("neighbor_discovered").len(),
        MAX_STREAMS_PER_REMOTE_IP
    );
    assert_eq!(world.host.bridge.streams(), MAX_STREAMS_PER_REMOTE_IP);

    // A page from another IP is still admitted.
    world.add_page(PageSetup::default());
    assert!(world.run_until(Duration::from_secs(10), |w| {
        w.lines("proved").len() == MAX_STREAMS_PER_REMOTE_IP + 1
    }));

    let closed = world.host.close();
    world.lines.extend(closed);
    assert_eq!(world.lines("lost").len(), MAX_STREAMS_PER_REMOTE_IP + 1);
    assert_eq!(
        world.lines("neighbor_lost").len(),
        MAX_STREAMS_PER_REMOTE_IP + 1
    );
}

/// A page presenting the listener's own preamble is refused, never
/// announced: the engine would route to itself.
#[test]
fn a_stream_proving_the_listeners_own_address_is_refused() {
    let mut world = World::new();
    let own = Identity {
        assertion: world.host.engine.identity_assertion().unwrap(),
        address: world.listener_address(),
    };
    world.add_page(PageSetup {
        identity: Some(own),
        ..PageSetup::default()
    });
    assert!(world.run_until(Duration::from_secs(10), |w| {
        !w.lines("refused").is_empty()
    }));
    assert!(world.lines("proved").is_empty());
    assert!(world.lines("neighbor_discovered").is_empty());
    assert_eq!(world.host.bridge.streams(), 0);
}

#[test]
fn the_example_state_store_passes_the_conformance_suite() {
    let report = offline_protocol::storage_conformance::run(&MemoryStateStorage::default());
    assert!(report.is_green(), "{}", report.summary());
}
