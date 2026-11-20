//! Answering on the first authenticated pair, ahead of the page's
//! nomination.
//!
//! # The failure this prevents
//!
//! str0m 0.25's ICE-lite agent gets an address to send to only when a check
//! carrying USE-CANDIDATE arrives, so it holds the page's DTLS ClientHello
//! unanswered until the page nominates a pair. Real iPhone Safari sometimes
//! nominated about one second after its first check, and connect took 1.2
//! to 3.0 s instead of 82 to 250 ms (ADR 0039, decision 5).
//!
//! An ICE-lite agent cannot run checks of its own, so a check that passed
//! integrity is all the validation it will ever have, and waiting for the
//! nomination buys nothing. The listener therefore marks the first admitted
//! request of each dial as nominating before str0m reads it: it adds
//! USE-CANDIDATE, signs the request again under the listener's own
//! password (the one the page signed it with), and recomputes FINGERPRINT.
//! A later nomination by the page still moves the pair, because str0m acts
//! on each one.
//!
//! This grants nothing a sender does not already have: anyone holding the
//! password can send USE-CANDIDATE themselves.
//!
//! # When to remove it
//!
//! The change belongs in str0m's ICE agent and has been offered upstream.
//! Delete this module once a str0m release answers an ICE-lite check
//! without waiting for USE-CANDIDATE, and keep the listener test that drops
//! every nominating request: it is what proves the upstream behaviour.

use crc::{Crc, CRC_32_ISO_HDLC};
use str0m::crypto::Sha1HmacProvider;

use crate::stun::{
    integrity, Authenticated, ATTR_FINGERPRINT, ATTR_MESSAGE_INTEGRITY, ATTR_USE_CANDIDATE,
    HEADER_LEN, INTEGRITY_LEN,
};

/// RFC 8489 section 14.7: the CRC-32 is XORed with this so a STUN
/// fingerprint never matches another protocol's.
const FINGERPRINT_XOR: u32 = 0x5354_554e;

/// `bytes`, an authenticated binding request, rewritten to carry
/// USE-CANDIDATE. It takes [`Authenticated`] and not a parsed request
/// because the rewrite signs with the listener's password: run before the
/// check, it would make any request verify.
///
/// `None` when it already carries one: the request is then handed to
/// str0m unchanged. Attributes after MESSAGE-INTEGRITY other than
/// FINGERPRINT are dropped, which changes nothing, since a receiver ignores
/// them.
pub(crate) fn nominating(
    bytes: &[u8],
    request: &Authenticated<'_>,
    password: &str,
    hmac: &dyn Sha1HmacProvider,
) -> Option<Vec<u8>> {
    let request = request.request();
    if request.use_candidate {
        return None;
    }
    let before = &bytes[..request.integrity_at];
    let fingerprint_len = if request.fingerprint { 8 } else { 0 };
    let mut out = Vec::with_capacity(before.len() + 4 + 4 + INTEGRITY_LEN + fingerprint_len);
    out.extend_from_slice(before);
    out.extend_from_slice(&ATTR_USE_CANDIDATE.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());

    let mac = integrity(&out, password.as_bytes(), hmac);
    out.extend_from_slice(&ATTR_MESSAGE_INTEGRITY.to_be_bytes());
    out.extend_from_slice(&(INTEGRITY_LEN as u16).to_be_bytes());
    out.extend_from_slice(&mac);

    if request.fingerprint {
        set_length(&mut out, 8);
        let crc = Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(&out) ^ FINGERPRINT_XOR;
        out.extend_from_slice(&ATTR_FINGERPRINT.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&crc.to_be_bytes());
    } else {
        set_length(&mut out, 0);
    }
    Some(out)
}

/// Writes the header's length: the attributes in `message` plus `more`.
/// A request was at most one datagram, so the sum fits u16.
fn set_length(message: &mut [u8], more: usize) {
    let length = (message.len() - HEADER_LEN + more) as u16;
    message[2..4].copy_from_slice(&length.to_be_bytes());
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::SocketAddr;

    use offline_protocol_core::time::Instant;
    use str0m::ice::StunMessage;
    use str0m::net::{Protocol, Receive};
    use str0m::{Candidate, IceCreds, Input, RtcConfig};

    use super::*;
    use crate::stun::parse_binding_request;
    use crate::stun::tests::{provider, request, PASSWORD, UFRAG};

    const PAGE_UFRAG: &str = "pageufrag";

    fn rewrite(bytes: &[u8]) -> Option<Vec<u8>> {
        let hmac = provider().sha1_hmac_provider;
        let admitted = parse_binding_request(bytes)
            .unwrap()
            .authenticate(bytes, UFRAG, PASSWORD, hmac)
            .unwrap();
        nominating(bytes, &admitted, PASSWORD, hmac)
    }

    #[test]
    fn a_request_already_nominating_is_left_alone() {
        let bytes = request(&format!("{UFRAG}:{PAGE_UFRAG}"), PASSWORD, true);
        assert!(rewrite(&bytes).is_none());
    }

    #[test]
    fn the_rewritten_request_nominates_and_verifies_under_str0m() {
        let hmac = provider().sha1_hmac_provider;
        let bytes = request(&format!("{UFRAG}:{PAGE_UFRAG}"), PASSWORD, false);
        let rewritten = rewrite(&bytes).unwrap();

        // str0m's own parser reads the nomination and the integrity, and
        // keeps everything else the page sent.
        let original = StunMessage::parse(&bytes).unwrap();
        let parsed = StunMessage::parse(&rewritten).unwrap();
        assert!(parsed.use_candidate());
        assert!(parsed.verify(PASSWORD.as_bytes(), |k, p| hmac.sha1_hmac(k, p)));
        assert_eq!(parsed.trans_id(), original.trans_id());
        assert_eq!(parsed.username(), original.username());
        assert_eq!(parsed.prio(), original.prio());
        assert_eq!(parsed.ice_controlling(), original.ice_controlling());

        // And this crate's admission check still admits it.
        let ours = parse_binding_request(&rewritten).unwrap();
        assert!(ours.use_candidate && ours.fingerprint);
        assert!(ours.is_for(&rewritten, UFRAG, PASSWORD, hmac));
    }

    /// The fingerprint is checked by browsers and some STUN stacks even
    /// where str0m does not: recompute it independently.
    #[test]
    fn the_fingerprint_is_recomputed() {
        let bytes = request(&format!("{UFRAG}:{PAGE_UFRAG}"), PASSWORD, false);
        let rewritten = rewrite(&bytes).unwrap();
        let (body, tail) = rewritten.split_at(rewritten.len() - 8);
        assert_eq!(tail[..2], ATTR_FINGERPRINT.to_be_bytes());
        assert_eq!(tail[2..4], 4u16.to_be_bytes());
        let expected = crc32_bitwise(body) ^ FINGERPRINT_XOR;
        assert_eq!(tail[4..], expected.to_be_bytes());
    }

    /// CRC-32 (ISO-HDLC) written out bit by bit, so the test does not check
    /// the crate against itself.
    fn crc32_bitwise(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// What the listener relies on: str0m's router still takes the
    /// rewritten request as this dial's.
    #[test]
    fn the_rewritten_request_still_passes_accepts() {
        let local: SocketAddr = "192.168.1.10:40000".parse().unwrap();
        let page: SocketAddr = "192.168.1.20:50000".parse().unwrap();
        let mut rtc = RtcConfig::new()
            .set_crypto_provider(std::sync::Arc::new(provider()))
            .set_ice_lite(true)
            .set_local_ice_credentials(IceCreds {
                ufrag: UFRAG.to_string(),
                pass: PASSWORD.to_string(),
            })
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
        rtc.direct_api().set_remote_ice_credentials(IceCreds {
            ufrag: PAGE_UFRAG.to_string(),
            pass: PAGE_UFRAG.to_string(),
        });

        let bytes = request(&format!("{UFRAG}:{PAGE_UFRAG}"), PASSWORD, false);
        let rewritten = rewrite(&bytes).unwrap();
        let accepts = |datagram: &[u8]| {
            let receive = Receive::new(Protocol::Udp, page, local, datagram).unwrap();
            rtc.accepts(&Input::Receive(Instant::now(), receive))
        };
        assert!(accepts(&bytes));
        assert!(accepts(&rewritten));

        // A rewrite signed with any other key would be refused.
        let hmac = provider().sha1_hmac_provider;
        let other = "NotTheListenersPassword0123456789";
        let foreign = request(&format!("{UFRAG}:{PAGE_UFRAG}"), other, false);
        let admitted = parse_binding_request(&foreign)
            .unwrap()
            .authenticate(&foreign, UFRAG, other, hmac)
            .unwrap();
        let forged = nominating(&foreign, &admitted, other, hmac).unwrap();
        assert!(!accepts(&forged));
    }
}
