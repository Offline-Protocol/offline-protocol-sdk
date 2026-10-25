//! WebRTC signalling through Nostr relays, by room (W11).
//!
//! Two browsers that share a short room code exchange their WebRTC
//! connection codes as Nostr events, instead of a person carrying them. The
//! page holds the relay sockets and moves their text; everything that gives
//! the text meaning is here.
//!
//! # Invariants
//!
//! - **The room code is the only secret, and it is not an identity.** Both
//!   sides derive the room's topic and its sealing key from it. A holder of
//!   the code can read the connection codes and open a carrier toward the
//!   room's host; who is on that carrier is still what the engine's preamble
//!   and MLS session prove (W3). The topic is public, so the code must carry
//!   enough entropy that it cannot be found from the topic: [`new_room`]
//!   gives eighty bits, and a code shorter than [`MIN_ROOM_CHARS`] is refused.
//! - **A relay sees a topic and ciphertext.** The connection code, which
//!   names local addresses, is sealed under the room key with the topic and
//!   the role as associated data, so an answer cannot be replayed as an
//!   offer. Each event is signed by a key used once, so the events of one
//!   room are not linked by their author.
//! - **Signalling events are ephemeral.** Their kind is in NIP-01's ephemeral
//!   range, so a relay forwards them to whoever is subscribed and keeps
//!   nothing; an event older than [`MAX_EVENT_AGE_SECS`] is refused, so a
//!   relay cannot hold one back and replay it later.
//! - **Nothing a relay sends is trusted.** An event opens only when its id is
//!   its NIP-01 hash, its signature verifies, its kind and topic are this
//!   room's, and its content authenticates under the room key.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use offline_protocol_core::time::{SystemTime, UNIX_EPOCH};
use offline_protocol_transport::nostr_crypto::{
    create_signal_subscription, verify_event, NOSTR_SIGNAL_KIND,
};
use offline_protocol_transport::{NostrEvent, NostrKeypair};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroizing;

/// The alphabet of a generated room code: lowercase letters and digits,
/// less the ones a person confuses (`l`, `o`, `0`, `1`). 32 symbols, five
/// bits each.
const ROOM_ALPHABET: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";

/// Length of a generated room code: eighty bits.
pub const ROOM_CHARS: usize = 16;

/// Shortest room code accepted: fifty bits from this alphabet, which an
/// offline search of the public topic does not find in the minutes a room is
/// open.
pub const MIN_ROOM_CHARS: usize = 10;

/// Oldest signalling event accepted, and how far ahead of this clock one may
/// be.
pub const MAX_EVENT_AGE_SECS: i64 = 120;

/// Largest connection code carried: a description with every candidate is a
/// few kilobytes.
const MAX_CONNECTION_CODE_BYTES: usize = 64 * 1024;

const HKDF_SALT: &[u8] = b"offline-protocol web signal v1";
const NONCE_BYTES: usize = 12;
const VERSION: u64 = 1;

/// One side of a connection's signalling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The joiner's description, sent to the room.
    Offer,
    /// The host's description, sent back for one session.
    Answer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Answer => "answer",
        }
    }

    /// Parses the role a page names.
    pub fn parse(role: &str) -> Result<Self, String> {
        match role {
            "offer" => Ok(Self::Offer),
            "answer" => Ok(Self::Answer),
            other => Err(format!("unknown signalling role {other}")),
        }
    }
}

/// What a signalling event carried, once opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    /// Which side sent it.
    pub role: Role,
    /// The joiner's session, which pairs an answer with its offer.
    pub session: String,
    /// The WebRTC connection code (`op1.…`).
    pub code: String,
}

/// A signal a relay delivered, and until when it stays fresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// What the event carried.
    pub signal: Signal,
    /// The last Unix second, on this clock, at which [`open`] still accepts
    /// the event: its `created_at` plus [`MAX_EVENT_AGE_SECS`]. A relay can
    /// deliver the same event again until then, so a host that forgot its
    /// session sooner would take the repeat for a new joiner.
    pub fresh_until: i64,
}

impl Opened {
    /// As JSON for the page, with the freshness bound in milliseconds so the
    /// page compares it with `Date.now()` directly.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "role": self.signal.role.as_str(),
            "session": self.signal.session,
            "code": self.signal.code,
            "freshUntilMs": self.fresh_until.saturating_mul(1000),
        })
        .to_string()
    }
}

/// A room's public topic and its sealing key.
struct Room {
    topic: String,
    cipher: ChaCha20Poly1305,
}

impl Room {
    fn derive(code: &str) -> Result<Self, String> {
        let code = normalise(code)?;
        let hkdf = Hkdf::<Sha256>::new(Some(HKDF_SALT), code.as_bytes());
        let mut topic = [0u8; 16];
        hkdf.expand(b"topic", &mut topic)
            .map_err(|_| "room derivation failed".to_string())?;
        let mut key = Zeroizing::new([0u8; 32]);
        hkdf.expand(b"key", key.as_mut())
            .map_err(|_| "room derivation failed".to_string())?;
        Ok(Self {
            topic: hex_encode(&topic),
            cipher: ChaCha20Poly1305::new(Key::from_slice(key.as_ref())),
        })
    }

    fn aad(&self, role: Role) -> Vec<u8> {
        let mut aad = b"offline-protocol web signal/".to_vec();
        aad.extend_from_slice(self.topic.as_bytes());
        aad.push(b'/');
        aad.extend_from_slice(role.as_str().as_bytes());
        aad
    }
}

/// A fresh room code.
pub fn new_room() -> String {
    let mut bytes = [0u8; ROOM_CHARS];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ROOM_ALPHABET[usize::from(b & 31)] as char)
        .collect()
}

/// The relay `REQ` that receives a room's signalling from `since` on.
pub fn subscription(room: &str, subscription_id: &str, since: i64) -> Result<String, String> {
    let room = Room::derive(room)?;
    create_signal_subscription(&room.topic, subscription_id, since).map_err(|e| e.to_string())
}

/// The relay `EVENT` message carrying one sealed, signed signal.
pub fn event(room: &str, signal: &Signal) -> Result<String, String> {
    check_session(&signal.session)?;
    check_code(&signal.code)?;
    let room = Room::derive(room)?;
    let plain = Zeroizing::new(
        serde_json::json!({
            "v": VERSION,
            "session": signal.session,
            "code": signal.code,
        })
        .to_string(),
    );
    let mut nonce = [0u8; NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce);
    let aad = room.aad(signal.role);
    let sealed = room
        .cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| "sealing the signal failed".to_string())?;
    let mut body = nonce.to_vec();
    body.extend_from_slice(&sealed);
    let content = format!("{}:{}", signal.role.as_str(), BASE64.encode(body));
    let keypair = NostrKeypair::generate_ephemeral().map_err(|e| e.to_string())?;
    NostrEvent::create_signal(&keypair, &room.topic, &content)
        .and_then(|event| event.to_relay_message())
        .map_err(|e| e.to_string())
}

/// Opens a relay message: the signal it carries for `room`, or `None` for
/// anything else (another subscription's traffic, a notice, a forgery, an
/// event too old).
pub fn open(room: &str, relay_message: &str, now: i64) -> Option<Opened> {
    let room = Room::derive(room).ok()?;
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
            tag.len() >= 2 && tag[0] == "t" && tag[1].as_str() == Some(room.topic.as_str())
        })
    });
    if !on_topic || !verify_event(event) {
        return None;
    }
    let (role, body) = event.get("content")?.as_str()?.split_once(':')?;
    let role = Role::parse(role).ok()?;
    let body = BASE64.decode(body).ok()?;
    if body.len() < NONCE_BYTES {
        return None;
    }
    let aad = room.aad(role);
    let plain = room
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
    let signal = Signal {
        role,
        session: plain.get("session")?.as_str()?.to_string(),
        code: plain.get("code")?.as_str()?.to_string(),
    };
    check_session(&signal.session).ok()?;
    check_code(&signal.code).ok()?;
    Some(Opened {
        signal,
        fresh_until: created_at.saturating_add(MAX_EVENT_AGE_SECS),
    })
}

/// This clock in Unix seconds, through the seam the browser can read (W4).
pub fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn normalise(code: &str) -> Result<String, String> {
    let code: String = code
        .trim()
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .collect::<String>()
        .to_lowercase();
    if code.chars().count() < MIN_ROOM_CHARS {
        return Err(format!(
            "a room code needs at least {MIN_ROOM_CHARS} characters"
        ));
    }
    Ok(code)
}

pub(crate) fn check_session(session: &str) -> Result<(), String> {
    if session.len() == 32 && session.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("a signalling session is 32 hex characters".to_string())
    }
}

pub(crate) fn check_code(code: &str) -> Result<(), String> {
    if code.starts_with("op1.") && code.len() <= MAX_CONNECTION_CODE_BYTES {
        Ok(())
    } else {
        Err("not a connection code".to_string())
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &str = "pong-table-7x4k9q";

    fn signal(role: Role) -> Signal {
        Signal {
            role,
            session: "0123456789abcdef0123456789abcdef".into(),
            code: "op1.abc".into(),
        }
    }

    /// What a relay forwards to a subscriber: the event under its id.
    fn forwarded(event_message: &str) -> String {
        let parsed: serde_json::Value = serde_json::from_str(event_message).unwrap();
        serde_json::json!(["EVENT", "sub", parsed[1]]).to_string()
    }

    #[test]
    fn a_signal_opens_in_its_room_and_nowhere_else() {
        let sent = signal(Role::Offer);
        let message = forwarded(&event(ROOM, &sent).unwrap());
        assert_eq!(
            open(ROOM, &message, now_unix_secs()).map(|o| o.signal),
            Some(sent)
        );
        assert_eq!(open("another-room-code", &message, now_unix_secs()), None);
    }

    #[test]
    fn spacing_and_case_do_not_change_the_room() {
        let sent = signal(Role::Answer);
        let message = forwarded(&event("PONG-TABLE 7X4K9Q", &sent).unwrap());
        assert_eq!(
            open(ROOM, &message, now_unix_secs()).map(|o| o.signal),
            Some(sent)
        );
    }

    #[test]
    fn an_answer_cannot_be_replayed_as_an_offer() {
        // Anyone can sign, so a relay re-signs the relabelled content with a
        // key of its own: only the sealing's associated data refuses it.
        let message = forwarded(&event(ROOM, &signal(Role::Answer)).unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
        let content = parsed[2]["content"].as_str().unwrap();
        let relabelled = content.replacen("answer:", "offer:", 1);
        let topic = parsed[2]["tags"][0][1].as_str().unwrap();
        let keypair = NostrKeypair::generate_ephemeral().unwrap();
        let resigned = NostrEvent::create_signal(&keypair, topic, &relabelled)
            .unwrap()
            .to_relay_message()
            .unwrap();
        assert_eq!(open(ROOM, &forwarded(&resigned), now_unix_secs()), None);
        // The same re-signing with the content untouched still opens, so the
        // refusal above is the label and nothing else.
        let honest = NostrEvent::create_signal(&keypair, topic, content)
            .unwrap()
            .to_relay_message()
            .unwrap();
        assert!(open(ROOM, &forwarded(&honest), now_unix_secs()).is_some());
    }

    #[test]
    fn an_opened_signal_says_how_long_a_relay_can_repeat_it() {
        let message = forwarded(&event(ROOM, &signal(Role::Offer)).unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
        let created_at = parsed[2]["created_at"].as_i64().unwrap();
        let opened = open(ROOM, &message, created_at).unwrap();
        assert_eq!(opened.fresh_until, created_at + MAX_EVENT_AGE_SECS);
        // The bound is exact: the event still opens at it, and not after.
        assert!(open(ROOM, &message, opened.fresh_until).is_some());
        assert_eq!(open(ROOM, &message, opened.fresh_until + 1), None);
        // A joiner whose clock runs ahead is fresh for longer after the host
        // first sees it, which a hold counted from receipt would miss.
        let early = open(ROOM, &message, created_at - MAX_EVENT_AGE_SECS).unwrap();
        assert_eq!(
            early.fresh_until - (created_at - MAX_EVENT_AGE_SECS),
            2 * MAX_EVENT_AGE_SECS
        );
        let json: serde_json::Value = serde_json::from_str(&opened.to_json()).unwrap();
        assert_eq!(json["freshUntilMs"], opened.fresh_until * 1000);
    }

    #[test]
    fn a_tampered_or_stale_event_is_refused() {
        let message = forwarded(&event(ROOM, &signal(Role::Offer)).unwrap());
        let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
        let created_at = parsed[2]["created_at"].as_i64().unwrap();
        assert_eq!(
            open(ROOM, &message, created_at + MAX_EVENT_AGE_SECS + 1),
            None
        );
        // A changed signature no longer verifies.
        let sig = parsed[2]["sig"].as_str().unwrap();
        let mut forged = sig.to_string();
        let last = if forged.ends_with('0') { "1" } else { "0" };
        forged.replace_range(forged.len() - 1.., last);
        assert_eq!(
            open(ROOM, &message.replace(sig, &forged), now_unix_secs()),
            None
        );
    }

    #[test]
    fn the_topic_is_all_a_relay_learns() {
        let message = event(ROOM, &signal(Role::Offer)).unwrap();
        assert!(!message.contains("op1.abc"));
        assert!(!message.contains("0123456789abcdef0123456789abcdef"));
        assert!(!message.contains(ROOM));
        let subscription = subscription(ROOM, "s", 0).unwrap();
        let topic: serde_json::Value = serde_json::from_str(&subscription).unwrap();
        let topic = topic[2]["#t"][0].as_str().unwrap();
        assert!(message.contains(topic));
    }

    #[test]
    fn short_room_codes_and_bad_fields_are_refused() {
        assert!(event("short", &signal(Role::Offer)).is_err());
        let mut bad = signal(Role::Offer);
        bad.session = "not hex".into();
        assert!(event(ROOM, &bad).is_err());
        let mut bad = signal(Role::Offer);
        bad.code = "something else".into();
        assert!(event(ROOM, &bad).is_err());
    }

    #[test]
    fn a_new_room_has_eighty_bits_from_the_alphabet() {
        let room = new_room();
        assert_eq!(room.len(), ROOM_CHARS);
        assert!(room.bytes().all(|b| ROOM_ALPHABET.contains(&b)));
        assert_ne!(new_room(), room);
    }
}
