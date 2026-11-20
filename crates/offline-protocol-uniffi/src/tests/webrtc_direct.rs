//! `webrtc_direct_*`: the WebRTC Direct listener behind the FFI (ADR 0039).
//!
//! The listener itself is pinned in `offline-protocol-webrtc-direct`. These
//! tests pin what the boundary adds: the code carries this instance's own
//! address, a dial driven through nothing but the FFI methods yields the
//! three events with the right fields, stream ids are never reused across
//! listeners, a listener's `Closed` events survive its close, and a library
//! built without the feature refuses with a message that names it.

use super::*;

fn initialized() -> OfflineProtocol {
    let device = OfflineProtocol::new(create_test_config()).unwrap();
    device
        .initialize_mls(
            Box::new(TestMlsStorageProvider::default()),
            Box::new(TestMlsStorageProvider::default()),
        )
        .unwrap();
    device
}

#[cfg(not(feature = "webrtc-direct"))]
#[test]
fn without_the_feature_every_method_refuses_or_has_nothing() {
    let device = initialized();
    let refusal = |e: ProtocolError| match e {
        ProtocolError::InvalidState(message) => message,
        other => panic!("refused with {other:?}"),
    };
    let message = refusal(
        device
            .webrtc_direct_open("192.168.1.10".into(), 40000)
            .unwrap_err(),
    );
    assert!(
        message.contains("built without the webrtc-direct feature"),
        "{message}"
    );
    let message = refusal(
        device
            .webrtc_direct_datagram_received("192.168.1.20".into(), 50000, vec![0; 20])
            .unwrap_err(),
    );
    assert!(message.contains("webrtc-direct feature"), "{message}");
    let message = refusal(device.webrtc_direct_send(0, b"body".to_vec()).unwrap_err());
    assert!(message.contains("webrtc-direct feature"), "{message}");
    device.webrtc_direct_close_stream(0);
    device.webrtc_direct_close();
    assert!(device.webrtc_direct_next_datagram().is_none());
    assert!(device.webrtc_direct_next_event().is_none());
    assert!(device.webrtc_direct_tick().is_none());
}

#[cfg(feature = "webrtc-direct")]
mod with_the_feature {
    use super::*;

    use offline_protocol_core::time::{Duration, Instant};
    use offline_protocol_transport::stream_framing::{frame, PeerStreamReader, StreamEvent};
    use offline_protocol_webrtc_direct::{DialCode, MAX_CHUNK_BYTES};
    use std::net::{IpAddr, SocketAddr};
    use str0m::channel::{ChannelConfig, ChannelId, Reliability};
    use str0m::config::Fingerprint;
    use str0m::net::{Protocol, Receive};
    use str0m::{Candidate, Event, IceCreds, Input, Output, Rtc, RtcConfig};

    const LISTENER_IP: &str = "192.168.1.10";
    const LISTENER_PORT: u16 = 40000;

    fn page_addr(index: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::from([192, 168, 1, 20 + index as u8]), 50000 + index)
    }

    /// A str0m client set up the way a browser is after it builds the
    /// listener's description from a code: full ICE and controlling, the
    /// code's credentials and certificate hash, DTLS and SCTP client,
    /// channel 0 negotiated, and its own identity assertion first on the
    /// channel. The same shape as `offline-protocol-webrtc-direct`'s test
    /// page, cut to what these tests drive.
    struct Page {
        addr: SocketAddr,
        listener: SocketAddr,
        address: String,
        rtc: Rtc,
        channel: ChannelId,
        timeout: Instant,
        open: bool,
        /// The device ended the connection (its DTLS close_notify) or the
        /// channel: either closes a browser's link.
        ended: bool,
        reader: PeerStreamReader,
        outbound: VecDeque<Vec<u8>>,
        preamble: Vec<u8>,
        proved: Option<String>,
        bodies: Vec<Vec<u8>>,
    }

    impl Page {
        fn new(index: u16, fragment: &str, identity: &OfflineProtocol) -> Self {
            let code = DialCode::parse(fragment).unwrap();
            let addr = page_addr(index);
            let listener = SocketAddr::new(code.host, code.port);
            let now = Instant::now();
            let mut rtc = RtcConfig::new()
                .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
                .build(now);
            rtc.add_local_candidate(Candidate::host(addr, "udp").unwrap());
            rtc.add_remote_candidate(Candidate::host(listener, "udp").unwrap());
            let mut api = rtc.direct_api();
            api.set_remote_ice_credentials(IceCreds {
                ufrag: code.ufrag.clone(),
                pass: code.password.clone(),
            });
            api.set_ice_controlling(true);
            api.set_remote_fingerprint(Fingerprint {
                hash_func: "sha-256".to_string(),
                bytes: code.certhash.to_vec(),
            });
            api.start_dtls(true).unwrap();
            api.start_sctp(true);
            let channel = api.create_data_channel(ChannelConfig {
                label: String::new(),
                ordered: true,
                reliability: Reliability::Reliable,
                negotiated: Some(0),
                protocol: String::new(),
            });
            let verify = Box::new(|body: &[u8]| verify_identity_assertion(body.to_vec()).ok());
            Page {
                addr,
                listener,
                address: identity.local_address().unwrap(),
                rtc,
                channel,
                timeout: now,
                open: false,
                ended: false,
                reader: PeerStreamReader::new(verify).expecting(code.address.clone()),
                outbound: VecDeque::new(),
                preamble: frame(&identity.identity_assertion(Vec::new()).unwrap()).unwrap(),
                proved: None,
                bodies: Vec::new(),
            }
        }

        fn send(&mut self, body: &[u8]) {
            for chunk in frame(body).unwrap().chunks(MAX_CHUNK_BYTES) {
                self.outbound.push_back(chunk.to_vec());
            }
        }

        /// Drains str0m's output into `wire` and writes what it can, until a
        /// write adds nothing.
        fn pump(&mut self, wire: &mut Vec<Vec<u8>>) {
            loop {
                loop {
                    match self.rtc.poll_output().unwrap() {
                        Output::Timeout(at) => {
                            self.timeout = at;
                            break;
                        }
                        Output::Transmit(t) => wire.push(t.contents.to_vec()),
                        Output::Event(event) => self.on_event(event),
                    }
                }
                if !self.flush() {
                    return;
                }
            }
        }

        fn on_event(&mut self, event: Event) {
            match event {
                Event::ChannelOpen(id, _) if id == self.channel => {
                    self.open = true;
                    for chunk in self.preamble.chunks(MAX_CHUNK_BYTES).rev() {
                        self.outbound.push_front(chunk.to_vec());
                    }
                }
                Event::Closed => self.ended = true,
                Event::ChannelClose(id) if id == self.channel => self.ended = true,
                Event::ChannelData(data) if data.id == self.channel => {
                    for event in self.reader.feed(&data.data) {
                        match event {
                            StreamEvent::Announced(address) => self.proved = Some(address),
                            StreamEvent::Message { body, .. } => self.bodies.push(body),
                            StreamEvent::Closed(reason) => panic!("the page refused: {reason}"),
                        }
                    }
                }
                _ => {}
            }
        }

        fn flush(&mut self) -> bool {
            if !self.open {
                return false;
            }
            let Some(mut channel) = self.rtc.channel(self.channel) else {
                return false;
            };
            let mut wrote = false;
            while let Some(chunk) = self.outbound.front() {
                if !channel.write(true, chunk).unwrap() {
                    break;
                }
                self.outbound.pop_front();
                wrote = true;
            }
            wrote
        }

        fn receive(&mut self, bytes: &[u8]) {
            let receive = Receive::new(Protocol::Udp, self.listener, self.addr, bytes).unwrap();
            self.rtc
                .handle_input(Input::Receive(Instant::now(), receive))
                .unwrap();
        }

        fn handle_timeout(&mut self) {
            let now = Instant::now();
            if self.timeout <= now {
                self.rtc.handle_input(Input::Timeout(now)).unwrap();
            }
        }
    }

    /// The device, its pages, and every event the device yielded, moved
    /// through nothing but the FFI methods a platform calls.
    struct Net<'a> {
        device: &'a OfflineProtocol,
        pages: Vec<Page>,
        events: Vec<WebrtcDirectEvent>,
    }

    impl<'a> Net<'a> {
        fn new(device: &'a OfflineProtocol) -> Self {
            Net {
                device,
                pages: Vec::new(),
                events: Vec::new(),
            }
        }

        /// One round: every page's datagrams to the device, the device's
        /// datagrams to their pages, both sets of timers, and the device's
        /// events collected.
        fn step(&mut self) {
            for index in 0..self.pages.len() {
                let mut wire = Vec::new();
                self.pages[index].handle_timeout();
                self.pages[index].pump(&mut wire);
                let source = self.pages[index].addr;
                for bytes in wire {
                    self.device
                        .webrtc_direct_datagram_received(
                            source.ip().to_string(),
                            source.port(),
                            bytes,
                        )
                        .unwrap();
                }
            }
            self.device.webrtc_direct_tick();
            while let Some(datagram) = self.device.webrtc_direct_next_datagram() {
                let destination = SocketAddr::new(
                    datagram.destination_ip.parse().unwrap(),
                    datagram.destination_port,
                );
                if let Some(page) = self.pages.iter_mut().find(|p| p.addr == destination) {
                    page.receive(&datagram.data);
                }
            }
            while let Some(event) = self.device.webrtc_direct_next_event() {
                self.events.push(event);
            }
        }

        fn run_until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done(self) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                self.step();
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn proved(&self) -> Vec<&WebrtcDirectEvent> {
            self.events
                .iter()
                .filter(|e| e.kind == WebrtcDirectEventKind::Proved)
                .collect()
        }

        fn closed(&self, stream: u64) -> bool {
            self.events
                .iter()
                .any(|e| e.kind == WebrtcDirectEventKind::Closed && e.stream == stream)
        }
    }

    #[test]
    fn open_returns_a_code_for_this_instance() {
        let device = initialized();
        let code = device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT)
            .unwrap();
        let parsed = DialCode::parse(&code.fragment).unwrap();
        let address = device.local_address().unwrap();
        assert_eq!(parsed.address, address);
        assert_eq!(code.address, address);
        assert_eq!(parsed.host.to_string(), LISTENER_IP);
        assert_eq!(code.host, LISTENER_IP);
        assert_eq!((parsed.port, code.port), (LISTENER_PORT, LISTENER_PORT));
        assert!(
            !format!("{code:?}").contains(&parsed.password),
            "the code's password stays out of logs"
        );

        // Fresh credentials and certificate on every open.
        let again = DialCode::parse(
            &device
                .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT + 1)
                .unwrap()
                .fragment,
        )
        .unwrap();
        assert_ne!(again.password, parsed.password);
        assert_ne!(again.ufrag, parsed.ufrag);
        assert_ne!(again.certhash, parsed.certhash);
        // Nothing was timed, since no dial is in progress.
        assert_eq!(device.webrtc_direct_tick(), None);
    }

    #[test]
    fn open_refuses_what_a_code_cannot_carry() {
        let fresh = OfflineProtocol::new(create_test_config()).unwrap();
        assert!(matches!(
            fresh.webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT),
            Err(ProtocolError::MlsNotInitialized)
        ));

        let device = initialized();
        for (ip, port) in [
            ("127.0.0.1", LISTENER_PORT),
            ("8.8.8.8", LISTENER_PORT),
            ("0.0.0.0", LISTENER_PORT),
            (LISTENER_IP, 0),
            ("fe80::1%en0", LISTENER_PORT),
            ("not an ip", LISTENER_PORT),
        ] {
            assert!(
                matches!(
                    device.webrtc_direct_open(ip.into(), port),
                    Err(ProtocolError::InvalidArgument(_))
                ),
                "{ip}:{port} was not refused as an argument"
            );
        }
        // None of them opened a listener.
        assert!(matches!(
            device.webrtc_direct_datagram_received("192.168.1.20".into(), 50000, vec![0; 4]),
            Err(ProtocolError::InvalidState(_))
        ));
    }

    #[test]
    fn a_refused_open_leaves_the_open_listener_as_it_was() {
        let device = initialized();
        let first = device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT)
            .unwrap();
        assert!(device
            .webrtc_direct_open("127.0.0.1".into(), LISTENER_PORT)
            .is_err());
        let page_identity = initialized();
        let mut net = Net::new(&device);
        net.pages
            .push(Page::new(0, &first.fragment, &page_identity));
        net.run_until("the first code still dials", |net| net.proved().len() == 1);
    }

    #[test]
    fn junk_gets_no_reply_and_no_event() {
        let device = initialized();
        let code = device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT)
            .unwrap();

        // A page that has the code's username but not its password: real
        // binding requests whose integrity does not verify.
        let mut wrong = DialCode::parse(&code.fragment).unwrap();
        wrong.password = wrong.password.chars().rev().collect();
        let page_identity = initialized();
        let mut page = Page::new(0, &wrong.to_fragment(), &page_identity);
        let mut checks = Vec::new();
        for _ in 0..3 {
            page.handle_timeout();
            page.pump(&mut checks);
            std::thread::sleep(Duration::from_millis(60));
        }
        assert!(!checks.is_empty(), "the page sent no checks");
        for check in checks {
            device
                .webrtc_direct_datagram_received(
                    page.addr.ip().to_string(),
                    page.addr.port(),
                    check,
                )
                .unwrap();
        }

        for junk in [
            Vec::new(),
            vec![0u8; 1],
            vec![0xff; 200],
            // A STUN binding request header with nothing behind it.
            vec![
                0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        ] {
            device
                .webrtc_direct_datagram_received("192.168.1.20".into(), 50000, junk)
                .unwrap();
        }
        assert!(device.webrtc_direct_next_datagram().is_none());
        assert!(device.webrtc_direct_next_event().is_none());
        assert_eq!(device.webrtc_direct_tick(), None, "no dial was admitted");
        assert!(matches!(
            device.webrtc_direct_datagram_received("not an ip".into(), 50000, vec![0; 4]),
            Err(ProtocolError::InvalidArgument(_))
        ));
    }

    #[test]
    fn a_dial_through_the_ffi_proves_carries_bodies_and_closes() {
        let device = initialized();
        let code = device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT)
            .unwrap();
        let page_identity = initialized();
        let mut net = Net::new(&device);
        net.pages.push(Page::new(0, &code.fragment, &page_identity));

        net.run_until("the dial to prove", |net| !net.proved().is_empty());
        let proved = net.proved()[0].clone();
        let stream = proved.stream;
        assert_eq!(
            proved.address.as_deref(),
            Some(net.pages[0].address.as_str())
        );
        assert_eq!(
            proved.remote_ip.as_deref(),
            Some(page_addr(0).ip().to_string().as_str())
        );
        assert_eq!(proved.remote_port, Some(page_addr(0).port()));
        assert_eq!(proved.data, None);

        // The page proves the listener too, from the preamble the FFI chose.
        net.run_until("the page to prove the device", |net| {
            net.pages[0].proved.is_some()
        });
        assert_eq!(net.pages[0].proved, device.local_address());

        // Page to device.
        net.pages[0].send(b"from the page");
        net.run_until("the body", |net| {
            net.events
                .iter()
                .any(|e| e.kind == WebrtcDirectEventKind::Body)
        });
        let body = net
            .events
            .iter()
            .find(|e| e.kind == WebrtcDirectEventKind::Body)
            .unwrap();
        assert_eq!(body.stream, stream);
        assert_eq!(body.data.as_deref(), Some(&b"from the page"[..]));
        assert_eq!(
            (&body.address, &body.remote_ip, body.remote_port),
            (&None, &None, None)
        );

        // Device to page, larger than one chunk.
        let large = vec![0x5a; MAX_CHUNK_BYTES * 3 + 7];
        device.webrtc_direct_send(stream, large.clone()).unwrap();
        net.run_until("the page to receive", |net| !net.pages[0].bodies.is_empty());
        assert_eq!(net.pages[0].bodies, vec![large]);

        // Refusals name the variant a caller can act on.
        assert!(matches!(
            device.webrtc_direct_send(stream, Vec::new()),
            Err(ProtocolError::InvalidArgument(_))
        ));
        assert!(matches!(
            device.webrtc_direct_send(stream + 1, b"x".to_vec()),
            Err(ProtocolError::InvalidState(_))
        ));

        device.webrtc_direct_close_stream(stream);
        net.step();
        assert!(
            net.closed(stream),
            "no Closed for the stream: {:?}",
            net.events
        );
        assert_eq!(
            net.events
                .iter()
                .filter(|e| e.kind == WebrtcDirectEventKind::Closed)
                .count(),
            1
        );
        assert!(matches!(
            device.webrtc_direct_send(stream, b"x".to_vec()),
            Err(ProtocolError::InvalidState(_))
        ));
    }

    #[test]
    fn close_ends_proved_streams_and_ids_are_never_reused() {
        let device = initialized();
        let code = device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT)
            .unwrap();
        let page_identity = initialized();
        let mut net = Net::new(&device);
        net.pages.push(Page::new(0, &code.fragment, &page_identity));
        net.run_until("the first dial to prove", |net| net.proved().len() == 1);
        let first = net.proved()[0].stream;

        device.webrtc_direct_close();
        net.step();
        assert!(
            net.closed(first),
            "close yielded no Closed: {:?}",
            net.events
        );
        // The close's datagrams outlive the listener, so the bridge sends
        // them before it closes the socket and the page learns now rather
        // than when its consent checks lapse. The page's reply is not
        // delivered: there is no listener to take it.
        net.pages[0].pump(&mut Vec::new());
        assert!(
            net.pages[0].ended,
            "the page's connection is still up after the listener closed"
        );
        assert!(device.webrtc_direct_next_datagram().is_none());
        assert!(matches!(
            device.webrtc_direct_send(first, b"x".to_vec()),
            Err(ProtocolError::InvalidState(_))
        ));
        assert!(matches!(
            device.webrtc_direct_datagram_received("192.168.1.20".into(), 50000, vec![0; 4]),
            Err(ProtocolError::InvalidState(_))
        ));

        // A second listener numbers its streams from zero; the boundary
        // must not.
        let code = device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT + 1)
            .unwrap();
        net.pages.clear();
        net.pages.push(Page::new(1, &code.fragment, &page_identity));
        net.run_until("the second dial to prove", |net| net.proved().len() == 2);
        let second = net.proved()[1].stream;
        assert_ne!(second, first, "a stream id was reused across listeners");

        // Replacing a listener delivers the old one's Closed events.
        device
            .webrtc_direct_open(LISTENER_IP.into(), LISTENER_PORT + 2)
            .unwrap();
        net.step();
        assert!(
            net.closed(second),
            "replacing yielded no Closed: {:?}",
            net.events
        );
    }
}
