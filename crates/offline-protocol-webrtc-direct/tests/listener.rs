//! The listener against a page, in memory.
//!
//! "The page" is the str0m client in `common`, set up the way a browser is
//! after it builds the listener's description from a code. Datagrams cross an in-memory network under a simulated
//! clock, so every run is deterministic and no socket is opened.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::VecDeque;
use std::net::SocketAddr;

use common::{identity, is_nominating_check, is_stun, listener_addr, Identity, Page, PageSetup};
use offline_protocol_core::time::{Duration, Instant};
use offline_protocol_webrtc_direct::{
    DialCode, Listener, ListenerConfig, ListenerError, ListenerEvent, StreamId, MAX_CHUNK_BYTES,
    MAX_PENDING_DIALS, MAX_QUEUED_BYTES,
};
use str0m::net::{Protocol, Receive};
use str0m::Input;

const LATENCY: Duration = Duration::from_millis(1);

type Filter = Box<dyn FnMut(usize, &[u8]) -> bool>;

/// The listener, its pages and the network between them.
struct Net {
    now: Instant,
    listener: Listener,
    identity: Identity,
    pages: Vec<Page>,
    /// (deliver at, from page, datagram)
    to_listener: VecDeque<(Instant, usize, Vec<u8>)>,
    /// (deliver at, destination, datagram)
    to_pages: VecDeque<(Instant, SocketAddr, Vec<u8>)>,
    /// Whether a page's datagram reaches the listener.
    page_to_listener: Filter,
    /// Whether the listener's datagrams reach the pages.
    listener_to_pages: bool,
    events: Vec<(Instant, ListenerEvent)>,
    /// Datagrams the listener sent, by destination.
    sent: Vec<SocketAddr>,
}

impl Net {
    fn new() -> Self {
        let now = Instant::now();
        let identity = identity("listener");
        let listener = Listener::new(
            ListenerConfig {
                local: listener_addr(),
                address: identity.address.clone(),
                preamble: identity.assertion.clone(),
            },
            now,
        )
        .unwrap();
        Net {
            now,
            listener,
            identity,
            pages: Vec::new(),
            to_listener: VecDeque::new(),
            to_pages: VecDeque::new(),
            page_to_listener: Box::new(|_, _| true),
            listener_to_pages: true,
            events: Vec::new(),
            sent: Vec::new(),
        }
    }

    fn code(&self) -> DialCode {
        // Through the fragment, as a page receives it.
        DialCode::parse(&self.listener.dial_code().to_fragment()).unwrap()
    }

    fn add_page(&mut self, setup: PageSetup) -> usize {
        let index = self.pages.len();
        let page = Page::new(index, &self.code(), &setup, self.now);
        self.pages.push(page);
        index
    }

    fn collect(&mut self) {
        for index in 0..self.pages.len() {
            let mut wire = Vec::new();
            self.pages[index].pump(&mut wire);
            for bytes in wire {
                if (self.page_to_listener)(index, &bytes) {
                    self.to_listener
                        .push_back((self.now + LATENCY, index, bytes));
                }
            }
        }
        while let Some(t) = self.listener.poll_transmit() {
            self.sent.push(t.destination);
            if self.listener_to_pages {
                self.to_pages
                    .push_back((self.now + LATENCY, t.destination, t.bytes));
            }
        }
        while let Some(event) = self.listener.poll_event() {
            self.events.push((self.now, event));
        }
    }

    /// Delivers what is due, fires what is due, then moves the clock to the
    /// next thing that will happen.
    fn step(&mut self) {
        let now = self.now;
        while self
            .to_listener
            .front()
            .is_some_and(|(at, _, _)| *at <= now)
        {
            let (_, from, bytes) = self.to_listener.pop_front().unwrap();
            let source = self.pages[from].addr;
            self.listener.handle_datagram(now, source, &bytes);
            self.collect();
        }
        while self.to_pages.front().is_some_and(|(at, _, _)| *at <= now) {
            let (_, to, bytes) = self.to_pages.pop_front().unwrap();
            if let Some(page) = self.pages.iter_mut().find(|p| p.addr == to) {
                if !page.failed {
                    let receive = Receive::new(Protocol::Udp, listener_addr(), to, &bytes).unwrap();
                    if page.rtc.handle_input(Input::Receive(now, receive)).is_err() {
                        page.failed = true;
                    }
                }
            }
            self.collect();
        }
        if self.listener.next_timeout().is_some_and(|at| at <= now) {
            self.listener.handle_timeout(now);
        }
        for page in &mut self.pages {
            if !page.failed
                && page.timeout <= now
                && page.rtc.handle_input(Input::Timeout(now)).is_err()
            {
                page.failed = true;
            }
        }
        self.collect();

        let next = [
            self.listener.next_timeout(),
            self.to_listener.front().map(|(at, _, _)| *at),
            self.to_pages.front().map(|(at, _, _)| *at),
        ]
        .into_iter()
        .flatten()
        .chain(self.pages.iter().filter(|p| !p.failed).map(|p| p.timeout))
        .min();
        self.now = match next {
            Some(at) if at > now => at,
            _ => now + Duration::from_millis(1),
        };
    }

    /// Runs until `done` holds or `limit` of simulated time passes.
    /// Returns whether `done` held.
    fn run_until(&mut self, limit: Duration, mut done: impl FnMut(&Net) -> bool) -> bool {
        let end = self.now + limit;
        while self.now < end {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    fn run_for(&mut self, duration: Duration) {
        self.run_until(duration, |_| false);
    }

    fn proved(&self) -> Vec<(StreamId, String)> {
        self.events
            .iter()
            .filter_map(|(_, e)| match e {
                ListenerEvent::Proved {
                    stream, address, ..
                } => Some((*stream, address.clone())),
                _ => None,
            })
            .collect()
    }

    fn closed(&self) -> Vec<(Instant, StreamId)> {
        self.events
            .iter()
            .filter_map(|(at, e)| match e {
                ListenerEvent::Closed { stream } => Some((*at, *stream)),
                _ => None,
            })
            .collect()
    }

    fn bodies(&self, of: StreamId) -> Vec<Vec<u8>> {
        self.events
            .iter()
            .filter_map(|(_, e)| match e {
                ListenerEvent::Body { stream, bytes } if *stream == of => Some(bytes.clone()),
                _ => None,
            })
            .collect()
    }

    /// Dials one ordinary page to the point both ends proved each other.
    fn dial(&mut self) -> (usize, StreamId) {
        let page = self.add_page(PageSetup::default());
        let before = self.proved().len();
        assert!(
            self.run_until(Duration::from_secs(5), |net| {
                net.proved().len() > before && net.pages[page].proved.is_some()
            }),
            "the dial did not prove both ends"
        );
        let (stream, _) = self.proved()[before].clone();
        (page, stream)
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[test]
fn a_page_dials_and_both_preambles_verify() {
    let mut net = Net::new();
    let (page, stream) = net.dial();

    assert_eq!(
        net.proved(),
        vec![(stream, net.pages[page].identity.address.clone())],
        "the listener proves the page's derived address, once"
    );
    assert!(
        net.events.iter().any(|(_, e)| matches!(
            e,
            ListenerEvent::Proved { remote, .. } if *remote == net.pages[page].addr
        )),
        "the event names where the dial came from"
    );
    assert_eq!(
        net.pages[page].proved.as_deref(),
        Some(net.identity.address.as_str()),
        "the page proves the address in the code"
    );
    assert_eq!(net.listener.pending_dials(), 0);
    assert_eq!(net.listener.streams(), 1);

    // A body each way.
    net.listener.send(stream, b"hello page").unwrap();
    net.pages[page].send(b"hello listener");
    assert!(net.run_until(Duration::from_secs(2), |net| {
        !net.pages[page].bodies.is_empty() && !net.bodies(stream).is_empty()
    }));
    assert_eq!(net.pages[page].bodies, vec![b"hello page".to_vec()]);
    assert_eq!(net.bodies(stream), vec![b"hello listener".to_vec()]);

    // 100 KiB each way, in chunks no browser refuses.
    let down = pattern(100 * 1024, 7);
    let up = pattern(100 * 1024, 91);
    let messages_before = net.pages[page].messages;
    net.listener.send(stream, &down).unwrap();
    net.pages[page].send(&up);
    assert!(net.run_until(Duration::from_secs(10), |net| {
        net.pages[page].bodies.len() == 2 && net.bodies(stream).len() == 2
    }));
    assert_eq!(net.pages[page].bodies[1], down);
    assert_eq!(net.bodies(stream)[1], up);
    assert!(net.pages[page].largest_message <= MAX_CHUNK_BYTES);
    assert!(
        net.pages[page].messages - messages_before >= 7,
        "100 KiB crossed in chunks"
    );
    assert!(net.closed().is_empty());
}

#[test]
fn a_wrong_password_gets_no_reply_and_no_state() {
    let mut net = Net::new();
    let checks = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let counter = checks.clone();
    net.page_to_listener = Box::new(move |_, bytes| {
        counter.set(counter.get() + usize::from(is_stun(bytes)));
        true
    });
    let page = net.add_page(PageSetup {
        wrong_password: true,
        ..PageSetup::default()
    });
    net.run_until(Duration::from_secs(8), |net| {
        assert_eq!(net.listener.pending_dials(), 0);
        false
    });
    assert!(checks.get() > 1, "the page kept checking");
    assert!(
        net.sent.is_empty(),
        "the listener sent {} datagrams",
        net.sent.len()
    );
    assert!(!net.pages[page].open && net.pages[page].proved.is_none());
    assert!(net.events.is_empty());
}

#[test]
fn a_wrong_certhash_fails_at_dtls_and_the_dial_is_dropped_at_its_deadline() {
    let mut net = Net::new();
    let page = net.add_page(PageSetup {
        wrong_certhash: true,
        ..PageSetup::default()
    });
    let start = net.now;
    net.run_for(Duration::from_secs(5));
    assert!(
        net.pages[page].failed,
        "the page's DTLS refused the certificate"
    );
    assert!(!net.pages[page].open);
    assert_eq!(
        net.listener.pending_dials(),
        1,
        "pending until its deadline"
    );

    assert!(
        net.run_until(Duration::from_secs(6), |net| net.listener.pending_dials()
            == 0)
    );
    let dropped = net.now.duration_since(start);
    assert!(
        dropped >= Duration::from_secs(9) && dropped <= Duration::from_millis(10_100),
        "dropped after {dropped:?}"
    );
    net.run_for(Duration::from_secs(2));
    assert!(
        net.events.is_empty(),
        "never proved, so never closed: {:?}",
        net.events
    );
}

/// The Safari fix: str0m waits for USE-CANDIDATE before it has a send
/// address. With every nominating check lost, the dial still proves.
#[test]
fn dtls_is_answered_before_the_page_nominates() {
    let mut net = Net::new();
    let dropped = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let counter = dropped.clone();
    net.page_to_listener = Box::new(move |_, bytes| {
        let nominating = is_nominating_check(bytes);
        if nominating {
            counter.set(counter.get() + 1);
        }
        !nominating
    });
    let page = net.add_page(PageSetup::default());
    assert!(
        net.run_until(Duration::from_secs(5), |net| {
            !net.proved().is_empty() && net.pages[page].proved.is_some()
        }),
        "the dial did not prove without a nomination"
    );
    assert!(
        dropped.get() > 0,
        "the page nominated, and the network dropped it"
    );
}

#[test]
fn a_fifth_pending_dial_gets_no_reply_and_keeps_no_state() {
    let mut net = Net::new();
    // Checks only: no DTLS reaches the listener, so no dial can prove.
    net.page_to_listener = Box::new(|_, bytes| is_stun(bytes));
    for _ in 0..=MAX_PENDING_DIALS {
        net.add_page(PageSetup::default());
    }
    let ufrags: std::collections::HashSet<String> = net
        .pages
        .iter_mut()
        .map(|p| p.rtc.direct_api().local_ice_credentials().ufrag)
        .collect();
    assert_eq!(ufrags.len(), MAX_PENDING_DIALS + 1, "distinct page ufrags");

    net.run_for(Duration::from_secs(3));
    assert_eq!(net.listener.pending_dials(), MAX_PENDING_DIALS);
    let fifth = net.pages[MAX_PENDING_DIALS].addr;
    for page in &net.pages[..MAX_PENDING_DIALS] {
        assert!(net.sent.contains(&page.addr), "{} was answered", page.addr);
    }
    assert!(!net.sent.contains(&fifth), "the fifth dial was answered");
    assert!(net.events.is_empty());
}

#[test]
fn a_preamble_that_does_not_verify_closes_the_stream_with_no_event() {
    let mut net = Net::new();
    let mut forged = identity("forger").assertion;
    forged[40] ^= 0x01; // a signature byte
    let page = net.add_page(PageSetup {
        preamble: Some(forged),
        ..PageSetup::default()
    });
    assert!(net.run_until(Duration::from_secs(3), |net| net.pages[page].open));
    let opened = net.now;
    assert_eq!(net.listener.pending_dials(), 1);
    assert!(
        net.run_until(Duration::from_secs(3), |net| net.listener.pending_dials()
            == 0)
    );
    assert!(
        net.now.duration_since(opened) < Duration::from_secs(1),
        "closed on the refusal, not at the deadline"
    );
    // The page keeps checking. Its checks are not admitted as a new dial,
    // so it is not answered and learns the stream is gone.
    let sent = net.sent.len();
    let checks = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let counter = checks.clone();
    net.page_to_listener = Box::new(move |_, bytes| {
        counter.set(counter.get() + usize::from(is_stun(bytes)));
        true
    });
    net.run_until(Duration::from_secs(12), |net| {
        assert_eq!(net.listener.pending_dials(), 0);
        false
    });
    assert!(checks.get() > 0, "the page checked again");
    assert_eq!(net.sent.len(), sent, "the listener answered the ended dial");
    assert!(net.events.is_empty(), "{:?}", net.events);
    assert_eq!(net.listener.streams(), 0);
}

#[test]
fn a_stream_ends_after_fifteen_seconds_without_a_check() {
    let mut net = Net::new();
    let (_, stream) = net.dial();
    // Longer than the timeout: only the page's checks keep it open.
    net.run_for(Duration::from_secs(20));
    assert!(net.closed().is_empty(), "consent checks keep it alive");

    let silent = net.now;
    net.page_to_listener = Box::new(|_, _| false);
    net.run_for(Duration::from_secs(30));
    let closed = net.closed();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].1, stream);
    let after = closed[0].0.duration_since(silent);
    assert!(
        after > Duration::from_secs(10) && after <= Duration::from_millis(15_050),
        "closed {after:?} after the page fell silent"
    );
    assert_eq!(net.listener.streams(), 0);
}

#[test]
fn a_stream_closes_when_more_than_eight_mib_waits_behind_the_channel() {
    let mut net = Net::new();
    let (_, stream) = net.dial();
    net.listener_to_pages = false;

    let body = vec![0x5a; 1024 * 1024];
    let mut accepted = 0;
    let refusal = loop {
        match net.listener.send(stream, &body) {
            Ok(()) => accepted += 1,
            Err(e) => break e,
        }
        assert!(accepted < 64, "the queue never filled");
    };
    assert_eq!(refusal, ListenerError::Overflow);
    assert!(
        accepted * (body.len() + 4) >= MAX_QUEUED_BYTES,
        "refused after only {accepted} MiB"
    );
    net.collect();
    assert_eq!(net.closed().len(), 1);
    assert_eq!(net.closed()[0].1, stream);
    assert_eq!(net.listener.streams(), 0);
    assert_eq!(
        net.listener.send(stream, b"after"),
        Err(ListenerError::UnknownStream)
    );
}

#[test]
fn closing_the_listener_closes_every_proved_stream_once() {
    let mut net = Net::new();
    let (_, first) = net.dial();
    let (_, second) = net.dial();
    // A third page that never gets past its checks: pending, not proved.
    net.page_to_listener = Box::new(|index, bytes| index < 2 || is_stun(bytes));
    net.add_page(PageSetup::default());
    net.run_for(Duration::from_secs(1));
    assert_eq!(net.listener.pending_dials(), 1);

    net.listener.close();
    net.collect();
    let mut closed: Vec<StreamId> = net.closed().into_iter().map(|(_, s)| s).collect();
    closed.sort();
    assert_eq!(closed, vec![first, second]);
    assert_eq!(net.listener.streams(), 0);
    assert_eq!(net.listener.pending_dials(), 0);
    assert_eq!(net.listener.next_timeout(), None);

    // Nothing is admitted again, and nothing more is said.
    let sent = net.sent.len();
    net.run_for(Duration::from_secs(5));
    assert_eq!(net.sent.len(), sent);
    assert_eq!(net.closed().len(), 2);
}

#[test]
fn a_listener_refuses_what_its_code_could_not_carry() {
    let me = identity("listener");
    let other = identity("other");
    let open = |local: &str, address: &str, preamble: &[u8]| {
        Listener::new(
            ListenerConfig {
                local: local.parse().unwrap(),
                address: address.to_string(),
                preamble: preamble.to_vec(),
            },
            Instant::now(),
        )
        .map(|_| ())
    };
    assert_eq!(
        open("192.168.1.10:40000", &me.address, &me.assertion),
        Ok(())
    );
    assert_eq!(open("[fe80::1]:40000", &me.address, &me.assertion), Ok(()));
    assert!(matches!(
        open("127.0.0.1:40000", &me.address, &me.assertion),
        Err(ListenerError::LocalAddress(_))
    ));
    assert!(matches!(
        open("8.8.8.8:40000", &me.address, &me.assertion),
        Err(ListenerError::LocalAddress(_))
    ));
    assert_eq!(
        open("192.168.1.10:0", &me.address, &me.assertion),
        Err(ListenerError::Socket)
    );
    // str0m takes no IPv4 link-local host candidate, though a code may
    // name one: refused here rather than answering nothing later.
    assert_eq!(
        open("169.254.3.4:40000", &me.address, &me.assertion),
        Err(ListenerError::Socket)
    );
    assert_eq!(
        open("192.168.1.10:40000", "off1", &me.assertion),
        Err(ListenerError::Address)
    );
    assert_eq!(
        open("192.168.1.10:40000", &me.address, &other.assertion),
        Err(ListenerError::Preamble)
    );
}

#[test]
fn every_listener_writes_fresh_credentials() {
    let me = identity("listener");
    let open = || {
        Listener::new(
            ListenerConfig {
                local: listener_addr(),
                address: me.address.clone(),
                preamble: me.assertion.clone(),
            },
            Instant::now(),
        )
        .unwrap()
    };
    let (a, b) = (open(), open());
    let (a, b) = (a.dial_code(), b.dial_code());
    assert_ne!(a.certhash, b.certhash);
    assert_ne!(a.ufrag, b.ufrag);
    assert_ne!(a.password, b.password);
    assert_eq!((a.ufrag.len(), a.password.len()), (16, 32));
    assert_eq!(DialCode::parse(&a.to_fragment()).as_ref(), Ok(a));
}
