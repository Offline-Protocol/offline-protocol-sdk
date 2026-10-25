//! Finding devices on the same network, through Nostr relays (W13).
//!
//! A page cannot browse DNS-SD, advertise over Bluetooth or join a Wi-Fi
//! Direct group, so two browsers on one network have no way to notice each
//! other. What they do share is the address the internet sees them from. A
//! nearby room is a Nostr topic derived from that address and the
//! application id: every device behind the same address lands in the same
//! room, announces itself there, and sets up a WebRTC link with whichever
//! announced device its person picks. The link then runs on the local
//! network; the relays carry only the setup.
//!
//! # Invariants
//!
//! - **The room is weak, by construction.** Its key is derived from an
//!   address anyone can enumerate, so anyone may compute a room's topic and
//!   key and read what is sealed under it. Only presences are sealed under it
//!   alone: a presence announces a key used for this room only and a label
//!   the application chose, and nothing that identifies the device or its
//!   local addresses.
//! - **A connection code is sealed to one presence key.** An offer is sealed
//!   with NIP-44 from the joiner's room key to the presence key it picked,
//!   and the answer back to the joiner's, so a holder of the room key reads
//!   neither, and cannot read the local addresses a code names (R27). Both
//!   layers bind their role, so an answer is not replayed as an offer.
//! - **A host admits by its application's decision.** Anyone holding the
//!   room key can offer, so the page asks the application before it answers
//!   one, and nothing here answers on its own. Who is on a link that forms is
//!   still what the engine's preamble and MLS session prove (W3).
//! - **IPv6 rooms are per prefix.** Behind IPv4 the devices of one network
//!   share one public address; with IPv6 each has its own, from the
//!   network's /64. The room is derived from the address for IPv4 and from
//!   the /64 for IPv6.
//! - **Events are ephemeral and fresh.** The signalling kind (25912), each
//!   event signed by a key used once, refused when more than
//!   [`signal::MAX_EVENT_AGE_SECS`] from this clock, as in a signalling room.

use std::net::IpAddr;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use offline_protocol_transport::nostr_crypto::{
    create_signal_subscription, open_from_key, seal_to_key, verify_event, NOSTR_SIGNAL_KIND,
};
use offline_protocol_transport::{NostrEvent, NostrKeypair};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::signal::{self, check_code, check_session, hex_encode, MAX_EVENT_AGE_SECS};

const HKDF_SALT: &[u8] = b"offline-protocol web nearby v1";
const NONCE_BYTES: usize = 12;
const VERSION: u64 = 1;

/// The longest label a presence carries, in characters.
pub const MAX_LABEL_CHARS: usize = 64;

/// What an event in a nearby room is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// "This device is here", sealed under the room key.
    Presence,
    /// A joiner's connection code, sealed to one presence key.
    Offer,
    /// The host's connection code, sealed back to the joiner.
    Answer,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Presence => "presence",
            Self::Offer => "offer",
            Self::Answer => "answer",
        }
    }

    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "presence" => Some(Self::Presence),
            "offer" => Some(Self::Offer),
            "answer" => Some(Self::Answer),
            _ => None,
        }
    }
}

/// One event a relay delivered, opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// What it is.
    pub kind: Kind,
    /// The sender's presence key: who to offer to, or who offered.
    pub peer: String,
    /// The label the sender's application chose. Empty on an answer.
    pub label: String,
    /// The joiner's session, on an offer or an answer.
    pub session: Option<String>,
    /// The connection code, on an offer or an answer.
    pub code: Option<String>,
    /// When the sender created it, in Unix seconds.
    pub created_at: i64,
}

impl Opened {
    /// As JSON for the page, times in milliseconds. `freshUntilMs` is when
    /// this clock stops accepting the event, as for a signal.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "kind": self.kind.as_str(),
            "peer": self.peer,
            "label": self.label,
            "session": self.session,
            "code": self.code,
            "createdAtMs": self.created_at.saturating_mul(1000),
            "freshUntilMs": self.created_at.saturating_add(MAX_EVENT_AGE_SECS).saturating_mul(1000),
        })
        .to_string()
    }
}

/// One device's place in one nearby room: the room's topic and key, and the
/// key this device announces there.
pub struct Nearby {
    topic: String,
    cipher: ChaCha20Poly1305,
    keys: NostrKeypair,
    label: String,
}

impl Nearby {
    /// Joins the room for `app_id` behind `network`, the address the
    /// internet sees this device from (as a STUN server reports it), under
    /// `label`. Refuses an address that does not parse, a label over
    /// [`MAX_LABEL_CHARS`], and one with control characters.
    pub fn new(app_id: &str, network: &str, label: &str) -> Result<Self, String> {
        if app_id.is_empty() {
            return Err("a nearby room needs the application id".to_string());
        }
        check_label(label)?;
        let scope = network_scope(network)?;
        let mut ikm = app_id.as_bytes().to_vec();
        ikm.push(0);
        ikm.extend_from_slice(scope.as_bytes());
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), &ikm);
        let mut topic = [0u8; 16];
        hkdf.expand(b"topic", &mut topic)
            .map_err(|_| "room derivation failed".to_string())?;
        let mut key = Zeroizing::new([0u8; 32]);
        hkdf.expand(b"key", key.as_mut())
            .map_err(|_| "room derivation failed".to_string())?;
        Ok(Self {
            topic: hex_encode(&topic),
            cipher: ChaCha20Poly1305::new(Key::from_slice(key.as_ref())),
            keys: NostrKeypair::generate_ephemeral().map_err(|e| e.to_string())?,
            label: label.to_string(),
        })
    }

    /// The room's public topic: what a relay sees.
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// The key this device announces in the room.
    pub fn key(&self) -> &str {
        self.keys.public_key_hex()
    }

    /// The relay `REQ` that receives the room from `since` on.
    pub fn subscription(&self, subscription_id: &str, since: i64) -> Result<String, String> {
        create_signal_subscription(&self.topic, subscription_id, since).map_err(|e| e.to_string())
    }

    /// The relay `EVENT` announcing this device.
    pub fn presence(&self) -> Result<String, String> {
        self.publish(
            Kind::Presence,
            serde_json::json!({ "v": VERSION, "key": self.key(), "label": self.label }),
        )
    }

    /// The relay `EVENT` carrying a joiner's connection code to the device
    /// that announced `to`.
    pub fn offer(&self, to: &str, session: &str, code: &str) -> Result<String, String> {
        self.addressed(Kind::Offer, to, session, code)
    }

    /// The relay `EVENT` carrying the host's connection code back to the
    /// joiner whose presence key is `to`.
    pub fn answer(&self, to: &str, session: &str, code: &str) -> Result<String, String> {
        self.addressed(Kind::Answer, to, session, code)
    }

    /// Opens a relay message: a presence from another device, or an offer or
    /// answer addressed to this one. `None` for anything else, this device's
    /// own events included.
    pub fn open(&self, relay_message: &str, now: i64) -> Option<Opened> {
        let message: serde_json::Value = serde_json::from_str(relay_message).ok()?;
        let parts = message.as_array()?;
        if parts.len() != 3 || parts[0].as_str()? != "EVENT" {
            return None;
        }
        let event = &parts[2];
        if event.get("kind")?.as_u64()? != u64::from(NOSTR_SIGNAL_KIND) {
            return None;
        }
        let created_at = event.get("created_at")?.as_i64()?;
        if (now - created_at).abs() > MAX_EVENT_AGE_SECS {
            return None;
        }
        let on_topic = event.get("tags")?.as_array()?.iter().any(|tag| {
            tag.as_array().is_some_and(|tag| {
                tag.len() >= 2 && tag[0] == "t" && tag[1].as_str() == Some(self.topic.as_str())
            })
        });
        if !on_topic || !verify_event(event) {
            return None;
        }
        let (kind, body) = event.get("content")?.as_str()?.split_once(':')?;
        let kind = Kind::parse(kind)?;
        let body = BASE64.decode(body).ok()?;
        if body.len() < NONCE_BYTES {
            return None;
        }
        let aad = self.aad(kind);
        let plain = self
            .cipher
            .decrypt(
                Nonce::from_slice(&body[..NONCE_BYTES]),
                Payload {
                    msg: &body[NONCE_BYTES..],
                    aad: &aad,
                },
            )
            .ok()?;
        let plain: serde_json::Value = serde_json::from_slice(&plain).ok()?;
        if plain.get("v")?.as_u64()? != VERSION {
            return None;
        }
        match kind {
            Kind::Presence => {
                let peer = plain.get("key")?.as_str()?.to_string();
                let label = plain.get("label")?.as_str()?.to_string();
                if peer == self.key() || !is_key(&peer) || check_label(&label).is_err() {
                    return None;
                }
                Some(Opened {
                    kind,
                    peer,
                    label,
                    session: None,
                    code: None,
                    created_at,
                })
            }
            Kind::Offer | Kind::Answer => {
                if plain.get("to")?.as_str()? != self.key() {
                    return None;
                }
                let peer = plain.get("from")?.as_str()?.to_string();
                let label = plain.get("label")?.as_str()?.to_string();
                if !is_key(&peer) || check_label(&label).is_err() {
                    return None;
                }
                let sealed = BASE64.decode(plain.get("sealed")?.as_str()?).ok()?;
                let inner = open_from_key(&self.keys, &peer, &sealed).ok()?;
                let inner: serde_json::Value = serde_json::from_slice(&inner).ok()?;
                if inner.get("v")?.as_u64()? != VERSION
                    || inner.get("kind")?.as_str()? != kind.as_str()
                {
                    return None;
                }
                let session = inner.get("session")?.as_str()?.to_string();
                let code = inner.get("code")?.as_str()?.to_string();
                check_session(&session).ok()?;
                check_code(&code).ok()?;
                Some(Opened {
                    kind,
                    peer,
                    label,
                    session: Some(session),
                    code: Some(code),
                    created_at,
                })
            }
        }
    }

    fn addressed(&self, kind: Kind, to: &str, session: &str, code: &str) -> Result<String, String> {
        if !is_key(to) {
            return Err("not a presence key".to_string());
        }
        check_session(session)?;
        check_code(code)?;
        let inner = Zeroizing::new(
            serde_json::json!({
                "v": VERSION,
                "kind": kind.as_str(),
                "session": session,
                "code": code,
            })
            .to_string(),
        );
        let sealed = seal_to_key(&self.keys, to, inner.as_bytes()).map_err(|e| e.to_string())?;
        self.publish(
            kind,
            serde_json::json!({
                "v": VERSION,
                "from": self.key(),
                "to": to,
                "label": self.label,
                "sealed": sealed,
            }),
        )
    }

    fn publish(&self, kind: Kind, body: serde_json::Value) -> Result<String, String> {
        let plain = Zeroizing::new(body.to_string());
        let mut nonce = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let aad = self.aad(kind);
        let sealed = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| "sealing the event failed".to_string())?;
        let mut content = nonce.to_vec();
        content.extend_from_slice(&sealed);
        let content = format!("{}:{}", kind.as_str(), BASE64.encode(content));
        let author = NostrKeypair::generate_ephemeral().map_err(|e| e.to_string())?;
        NostrEvent::create_signal(&author, &self.topic, &content)
            .and_then(|event| event.to_relay_message())
            .map_err(|e| e.to_string())
    }

    fn aad(&self, kind: Kind) -> Vec<u8> {
        let mut aad = b"offline-protocol web nearby/".to_vec();
        aad.extend_from_slice(self.topic.as_bytes());
        aad.push(b'/');
        aad.extend_from_slice(kind.as_str().as_bytes());
        aad
    }
}

/// What a room is derived from: the IPv4 address, or the IPv6 /64.
fn network_scope(network: &str) -> Result<String, String> {
    let address: IpAddr = network
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| format!("{network} is not an IP address"))?;
    Ok(match address {
        IpAddr::V4(v4) => format!("v4/{v4}"),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => format!("v4/{v4}"),
            None => {
                let segments = v6.segments();
                format!(
                    "v6/{:x}:{:x}:{:x}:{:x}::/64",
                    segments[0], segments[1], segments[2], segments[3]
                )
            }
        },
    })
}

fn is_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check_label(label: &str) -> Result<(), String> {
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(format!(
            "a nearby label is at most {MAX_LABEL_CHARS} characters"
        ));
    }
    if label.chars().any(char::is_control) {
        return Err("a nearby label cannot hold control characters".to_string());
    }
    Ok(())
}

/// This clock in Unix seconds.
pub fn now_unix_secs() -> i64 {
    signal::now_unix_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "pong";
    const SESSION: &str = "0123456789abcdef0123456789abcdef";

    fn room(network: &str, label: &str) -> Nearby {
        Nearby::new(APP, network, label).expect("room")
    }

    fn now() -> i64 {
        now_unix_secs()
    }

    /// What a relay delivers for a published `["EVENT", {...}]`.
    fn relayed(published: Result<String, String>) -> String {
        let sent: serde_json::Value = serde_json::from_str(&published.unwrap()).unwrap();
        serde_json::json!(["EVENT", "sub", sent[1]]).to_string()
    }

    #[test]
    fn devices_behind_one_address_share_a_room_and_see_each_other() {
        let (a, b) = (room("203.0.113.7", "Alice"), room("203.0.113.7", "Bob"));
        assert_eq!(a.topic(), b.topic());
        let seen = b.open(&relayed(a.presence()), now()).expect("presence");
        assert_eq!(seen.kind, Kind::Presence);
        assert_eq!(seen.peer, a.key());
        assert_eq!(seen.label, "Alice");
        assert!(
            a.open(&relayed(a.presence()), now()).is_none(),
            "own presence"
        );
    }

    #[test]
    fn another_network_or_application_is_another_room() {
        let a = room("203.0.113.7", "");
        assert_ne!(a.topic(), room("203.0.113.8", "").topic());
        assert_ne!(
            a.topic(),
            Nearby::new("chess", "203.0.113.7", "").unwrap().topic()
        );
        assert!(room("198.51.100.1", "")
            .open(&relayed(a.presence()), now())
            .is_none());
    }

    #[test]
    fn an_ipv6_room_is_the_networks_prefix() {
        assert_eq!(
            room("2001:db8:1:2:aaaa::1", "").topic(),
            room("2001:db8:1:2:bbbb::9", "").topic()
        );
        assert_ne!(
            room("2001:db8:1:2::1", "").topic(),
            room("2001:db8:1:3::1", "").topic()
        );
        assert_eq!(
            room("::ffff:203.0.113.7", "").topic(),
            room("203.0.113.7", "").topic()
        );
        assert!(Nearby::new(APP, "not an address", "").is_err());
    }

    #[test]
    fn an_offer_opens_for_the_key_it_was_sealed_to_and_nobody_else() {
        let (host, joiner) = (room("203.0.113.7", "Host"), room("203.0.113.7", "Joiner"));
        let eavesdropper = room("203.0.113.7", "Eve");
        let offer = relayed(joiner.offer(host.key(), SESSION, "op1.offer"));
        let opened = host.open(&offer, now()).expect("offer");
        assert_eq!(opened.kind, Kind::Offer);
        assert_eq!(opened.peer, joiner.key());
        assert_eq!(opened.label, "Joiner");
        assert_eq!(opened.code.as_deref(), Some("op1.offer"));
        assert!(
            eavesdropper.open(&offer, now()).is_none(),
            "a holder of the room key read an offer sealed to another"
        );
        let answer = relayed(host.answer(joiner.key(), SESSION, "op1.answer"));
        let opened = joiner.open(&answer, now()).expect("answer");
        assert_eq!(opened.kind, Kind::Answer);
        assert_eq!(opened.code.as_deref(), Some("op1.answer"));
        assert!(host.open(&answer, now()).is_none());
    }

    #[test]
    fn an_event_out_of_its_window_is_refused() {
        let a = room("203.0.113.7", "");
        let b = room("203.0.113.7", "");
        let presence = relayed(a.presence());
        assert!(b.open(&presence, now() + MAX_EVENT_AGE_SECS + 5).is_none());
        assert!(b.open(&presence, now() - MAX_EVENT_AGE_SECS - 5).is_none());
    }

    #[test]
    fn a_label_is_bounded_and_printable() {
        assert!(Nearby::new(APP, "203.0.113.7", &"x".repeat(MAX_LABEL_CHARS)).is_ok());
        assert!(Nearby::new(APP, "203.0.113.7", &"x".repeat(MAX_LABEL_CHARS + 1)).is_err());
        assert!(Nearby::new(APP, "203.0.113.7", "a\nb").is_err());
    }

    #[test]
    fn a_signalling_room_and_a_nearby_room_do_not_read_each_other() {
        let a = room("203.0.113.7", "");
        let code = crate::signal::new_room();
        let offer = relayed(crate::signal::event(
            &code,
            &crate::signal::Signal {
                role: crate::signal::Role::Offer,
                session: SESSION.into(),
                code: "op1.x".into(),
            },
        ));
        assert!(a.open(&offer, now()).is_none());
        assert!(crate::signal::open(&code, &relayed(a.presence()), now()).is_none());
    }
}
