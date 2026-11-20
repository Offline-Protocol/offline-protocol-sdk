//! What the integration tests share: identities, and "the page", a str0m
//! client set up the way a browser is after it builds the listener's
//! description from a code: full ICE and controlling, the listener's
//! credentials and certificate hash from the code, DTLS client, SCTP
//! client, channel 0 negotiated, and its own identity assertion first on
//! the channel.

// Each test crate compiles this module and uses part of it.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;

use offline_protocol_core::time::Instant;
use offline_protocol_mls::storage::InMemoryStorage;
use offline_protocol_mls::{verify_identity_assertion, MlsManager};
use offline_protocol_transport::stream_framing::{frame, PeerStreamReader, StreamEvent};
use offline_protocol_webrtc_direct::{DialCode, MAX_CHUNK_BYTES};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::config::Fingerprint;
use str0m::ice::StunMessage;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceCreds, Input, Output, Rtc, RtcConfig};

#[derive(Clone)]
pub struct Identity {
    pub assertion: Vec<u8>,
    pub address: String,
}

pub fn identity(name: &str) -> Identity {
    let manager = MlsManager::new(name, Arc::new(InMemoryStorage::new())).unwrap();
    let assertion = manager.identity_assertion(&[]).unwrap();
    let address = verify_identity_assertion(&assertion).unwrap().to_string();
    Identity { assertion, address }
}

pub fn listener_addr() -> SocketAddr {
    "192.168.1.10:40000".parse().unwrap()
}

pub fn page_addr(index: usize) -> SocketAddr {
    format!("192.168.1.{}:{}", 20 + index, 50000 + index)
        .parse()
        .unwrap()
}

pub fn is_nominating_check(bytes: &[u8]) -> bool {
    StunMessage::parse(bytes)
        .map(|m| m.is_binding_request() && m.use_candidate())
        .unwrap_or(false)
}

pub fn is_stun(bytes: &[u8]) -> bool {
    StunMessage::parse(bytes).is_ok()
}

/// How the page deviates from a browser that has the right code.
#[derive(Clone, Default)]
pub struct PageSetup {
    pub wrong_password: bool,
    pub wrong_certhash: bool,
    /// Sent in place of the page's real assertion.
    pub preamble: Option<Vec<u8>>,
    /// The page's identity, in place of a fresh one.
    pub identity: Option<Identity>,
    /// The page's socket address, in place of [`page_addr`] of its index.
    pub addr: Option<SocketAddr>,
}

pub struct Page {
    pub addr: SocketAddr,
    pub identity: Identity,
    pub rtc: Rtc,
    pub channel: ChannelId,
    pub timeout: Instant,
    pub open: bool,
    /// The listener ended the connection (its DTLS close_notify) or the
    /// channel: either closes a browser's link.
    pub ended: bool,
    pub failed: bool,
    pub reader: PeerStreamReader,
    pub outbound: VecDeque<Vec<u8>>,
    pub preamble: Vec<u8>,
    pub proved: Option<String>,
    pub bodies: Vec<Vec<u8>>,
    pub largest_message: usize,
    pub messages: usize,
}

impl Page {
    pub fn new(index: usize, code: &DialCode, setup: &PageSetup, now: Instant) -> Self {
        let identity = setup
            .identity
            .clone()
            .unwrap_or_else(|| identity(&format!("page-{index}")));
        let addr = setup.addr.unwrap_or_else(|| page_addr(index));
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
            .build(now);
        rtc.add_local_candidate(Candidate::host(addr, "udp").unwrap());
        rtc.add_remote_candidate(
            Candidate::host(SocketAddr::new(code.host, code.port), "udp").unwrap(),
        );
        let mut certhash = code.certhash;
        if setup.wrong_certhash {
            certhash[0] ^= 0x01;
        }
        let mut password = code.password.clone();
        if setup.wrong_password {
            password = password.chars().rev().collect();
        }
        let mut api = rtc.direct_api();
        api.set_remote_ice_credentials(IceCreds {
            ufrag: code.ufrag.clone(),
            pass: password,
        });
        api.set_ice_controlling(true);
        api.set_remote_fingerprint(Fingerprint {
            hash_func: "sha-256".to_string(),
            bytes: certhash.to_vec(),
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
        let verify =
            Box::new(|body: &[u8]| verify_identity_assertion(body).ok().map(|a| a.to_string()));
        let preamble = frame(setup.preamble.as_ref().unwrap_or(&identity.assertion)).unwrap();
        Page {
            addr,
            identity,
            rtc,
            channel,
            timeout: now,
            open: false,
            ended: false,
            failed: false,
            reader: PeerStreamReader::new(verify).expecting(code.address.clone()),
            outbound: VecDeque::new(),
            preamble,
            proved: None,
            bodies: Vec::new(),
            largest_message: 0,
            messages: 0,
        }
    }

    pub fn send(&mut self, body: &[u8]) {
        for chunk in frame(body).unwrap().chunks(MAX_CHUNK_BYTES) {
            self.outbound.push_back(chunk.to_vec());
        }
    }

    pub fn pump(&mut self, wire: &mut Vec<Vec<u8>>) {
        if self.failed {
            return;
        }
        loop {
            loop {
                match self.rtc.poll_output() {
                    Ok(Output::Timeout(at)) => {
                        self.timeout = at;
                        break;
                    }
                    Ok(Output::Transmit(t)) => wire.push(t.contents.to_vec()),
                    Ok(Output::Event(event)) => self.on_event(event),
                    Err(_) => {
                        self.failed = true;
                        return;
                    }
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
                let preamble = self.preamble.clone();
                for chunk in preamble.chunks(MAX_CHUNK_BYTES) {
                    self.outbound.push_front(chunk.to_vec());
                }
            }
            Event::Closed => self.ended = true,
            Event::ChannelClose(id) if id == self.channel => self.ended = true,
            Event::ChannelData(data) if data.id == self.channel => {
                self.largest_message = self.largest_message.max(data.data.len());
                self.messages += 1;
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

    /// One datagram from the listener at `from`.
    pub fn receive(&mut self, now: Instant, from: SocketAddr, bytes: &[u8]) {
        if self.failed {
            return;
        }
        let receive = Receive::new(Protocol::Udp, from, self.addr, bytes).unwrap();
        if self.rtc.handle_input(Input::Receive(now, receive)).is_err() {
            self.failed = true;
        }
    }

    /// Fires str0m's timer if it is due.
    pub fn handle_timeout(&mut self, now: Instant) {
        if !self.failed
            && self.timeout <= now
            && self.rtc.handle_input(Input::Timeout(now)).is_err()
        {
            self.failed = true;
        }
    }
}
