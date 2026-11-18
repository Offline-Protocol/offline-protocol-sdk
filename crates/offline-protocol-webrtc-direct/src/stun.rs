//! The listener's admission check: is this datagram a STUN binding request
//! for this listener, signed with its password?
//!
//! A listener on a shared network receives whatever anyone sends to its
//! port. Only an authenticated check may make it keep state or reply
//! (`docs/spec/stream-framing.md`, "The listener's side", step one), so
//! this check runs on the raw bytes, allocates nothing and costs one
//! HMAC-SHA1 at most. Without it, every datagram would cost a DTLS and SCTP
//! stack, and a reply to an unauthenticated one would make the device a
//! reflector.

use str0m::crypto::Sha1HmacProvider;
use subtle::ConstantTimeEq;

/// STUN's fixed header: type, length, magic cookie, transaction id.
pub(crate) const HEADER_LEN: usize = 20;
const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];
const BINDING_REQUEST: u16 = 0x0001;

pub(crate) const ATTR_USERNAME: u16 = 0x0006;
pub(crate) const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
pub(crate) const ATTR_USE_CANDIDATE: u16 = 0x0025;
pub(crate) const ATTR_FINGERPRINT: u16 = 0x8028;
pub(crate) const INTEGRITY_LEN: usize = 20;

/// A binding request as far as admission reads it. Borrowed from the
/// datagram, so reading one allocates nothing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BindingRequest<'a> {
    /// The first half of USERNAME: the listener's ufrag, if the sender has
    /// the code.
    pub(crate) local_ufrag: &'a str,
    /// The second half: the page's own ufrag, which keys its dial.
    pub(crate) remote_ufrag: &'a str,
    /// Whether USE-CANDIDATE comes before MESSAGE-INTEGRITY.
    pub(crate) use_candidate: bool,
    /// Byte offset of the MESSAGE-INTEGRITY attribute's header.
    pub(crate) integrity_at: usize,
    /// Whether FINGERPRINT follows MESSAGE-INTEGRITY.
    pub(crate) fingerprint: bool,
}

/// Reads `bytes` as a binding request with a USERNAME and a
/// MESSAGE-INTEGRITY. Anything else, including any STUN message of another
/// class or method, is `None`.
///
/// Attributes after MESSAGE-INTEGRITY are ignored except FINGERPRINT, as
/// RFC 8489 section 14.5 requires.
pub(crate) fn parse_binding_request(bytes: &[u8]) -> Option<BindingRequest<'_>> {
    if bytes.len() < HEADER_LEN || bytes[0] & 0xC0 != 0 {
        return None;
    }
    let kind = u16::from_be_bytes([bytes[0], bytes[1]]);
    let length = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    if kind != BINDING_REQUEST
        || bytes[4..8] != MAGIC_COOKIE
        || length % 4 != 0
        || HEADER_LEN + length != bytes.len()
    {
        return None;
    }

    let mut username = None;
    let mut use_candidate = false;
    let mut integrity_at = None;
    let mut fingerprint = false;
    let mut at = HEADER_LEN;
    while at + 4 <= bytes.len() {
        let kind = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let value = bytes.get(at + 4..at + 4 + len)?;
        match (kind, integrity_at.is_some()) {
            (ATTR_USERNAME, false) => username = Some(value),
            (ATTR_USE_CANDIDATE, false) => use_candidate = true,
            (ATTR_MESSAGE_INTEGRITY, false) => {
                if len != INTEGRITY_LEN {
                    return None;
                }
                integrity_at = Some(at);
            }
            (ATTR_FINGERPRINT, true) => fingerprint = true,
            _ => {}
        }
        at += 4 + len.div_ceil(4) * 4;
    }

    let username = core::str::from_utf8(username?).ok()?;
    let (local_ufrag, remote_ufrag) = username.split_once(':')?;
    if local_ufrag.is_empty() || remote_ufrag.is_empty() {
        return None;
    }
    Some(BindingRequest {
        local_ufrag,
        remote_ufrag,
        use_candidate,
        integrity_at: integrity_at?,
        fingerprint,
    })
}

/// HMAC-SHA1 under `password` over the header (its length counting the
/// attributes up to and including MESSAGE-INTEGRITY) and every attribute
/// before MESSAGE-INTEGRITY, as RFC 8489 section 14.5 defines it.
///
/// `before_integrity` is the message up to the MESSAGE-INTEGRITY attribute.
pub(crate) fn integrity(
    before_integrity: &[u8],
    password: &[u8],
    hmac: &dyn Sha1HmacProvider,
) -> [u8; INTEGRITY_LEN] {
    // The length a receiver authenticates: the attributes so far plus the
    // integrity attribute itself. It fits u16, as the parsed length did.
    let length = (before_integrity.len() - HEADER_LEN + 4 + INTEGRITY_LEN) as u16;
    hmac.sha1_hmac(
        password,
        &[
            &before_integrity[..2],
            &length.to_be_bytes(),
            &before_integrity[4..],
        ],
    )
}

/// A binding request that passed [`BindingRequest::authenticate`]. Nothing
/// else makes one, so code that must only ever see an authenticated request
/// cannot be handed a merely parsed one. The nomination rewrite is that
/// code: it signs with the listener's own password, so a request it
/// rewrote before the check would verify whoever had sent it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Authenticated<'a>(BindingRequest<'a>);

impl<'a> Authenticated<'a> {
    /// The request that authenticated.
    pub(crate) fn request(&self) -> &BindingRequest<'a> {
        &self.0
    }
}

impl<'a> BindingRequest<'a> {
    /// The request, if it is for `ufrag` and signed with `password`.
    pub(crate) fn authenticate(
        self,
        bytes: &[u8],
        ufrag: &str,
        password: &str,
        hmac: &dyn Sha1HmacProvider,
    ) -> Option<Authenticated<'a>> {
        self.is_for(bytes, ufrag, password, hmac)
            .then_some(Authenticated(self))
    }

    /// Whether the request is for `ufrag` and signed with `password`.
    /// Compared in constant time, so a sender learns nothing from timing
    /// about how close a guess was.
    pub(crate) fn is_for(
        &self,
        bytes: &[u8],
        ufrag: &str,
        password: &str,
        hmac: &dyn Sha1HmacProvider,
    ) -> bool {
        if self.local_ufrag != ufrag {
            return false;
        }
        let expected = integrity(&bytes[..self.integrity_at], password.as_bytes(), hmac);
        let carried = &bytes[self.integrity_at + 4..self.integrity_at + 4 + INTEGRITY_LEN];
        expected.ct_eq(carried).into()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use str0m::ice::{StunMessage, TransId};

    pub(crate) const UFRAG: &str = "ListenerUfrag123";
    pub(crate) const PASSWORD: &str = "ListenerPassword0123456789abcdef";

    pub(crate) fn provider() -> str0m::config::CryptoProvider {
        str0m::crypto::from_feature_flags()
    }

    /// A binding request as a browser sends one, built by str0m's own STUN
    /// code rather than this module's.
    pub(crate) fn request(username: &str, password: &str, use_candidate: bool) -> Vec<u8> {
        let hmac = provider().sha1_hmac_provider;
        let message = StunMessage::binding_request(
            username,
            TransId::new(),
            true,
            0x1234_5678_9abc_def0,
            1_845_501_695,
            use_candidate,
        );
        let mut buf = vec![0u8; 1500];
        let n = message
            .to_bytes(Some(password.as_bytes()), &mut buf, |k, p| {
                hmac.sha1_hmac(k, p)
            })
            .unwrap();
        buf.truncate(n);
        buf
    }

    #[test]
    fn a_request_signed_with_the_password_is_admitted() {
        let hmac = provider().sha1_hmac_provider;
        let bytes = request(&format!("{UFRAG}:pageufrag"), PASSWORD, false);
        let parsed = parse_binding_request(&bytes).unwrap();
        assert_eq!(parsed.remote_ufrag, "pageufrag");
        assert!(!parsed.use_candidate);
        assert!(parsed.fingerprint);
        assert!(parsed.is_for(&bytes, UFRAG, PASSWORD, hmac));
    }

    #[test]
    fn a_wrong_password_or_ufrag_is_refused() {
        let hmac = provider().sha1_hmac_provider;
        let bytes = request(
            &format!("{UFRAG}:pageufrag"),
            "SomeoneElsesPassword0123456789ab",
            false,
        );
        let parsed = parse_binding_request(&bytes).unwrap();
        assert!(!parsed.is_for(&bytes, UFRAG, PASSWORD, hmac));

        let bytes = request("OtherUfrag:pageufrag", PASSWORD, false);
        let parsed = parse_binding_request(&bytes).unwrap();
        assert!(!parsed.is_for(&bytes, UFRAG, PASSWORD, hmac));
    }

    #[test]
    fn a_flipped_bit_anywhere_before_the_integrity_is_refused() {
        let hmac = provider().sha1_hmac_provider;
        let bytes = request(&format!("{UFRAG}:pageufrag"), PASSWORD, false);
        let integrity_at = parse_binding_request(&bytes).unwrap().integrity_at;
        // The transaction id and every attribute byte: the type and length
        // words are structural and refuse at parse instead.
        for i in 8..integrity_at + 4 + INTEGRITY_LEN {
            let mut tampered = bytes.clone();
            tampered[i] ^= 0x01;
            if let Some(parsed) = parse_binding_request(&tampered) {
                assert!(
                    !parsed.is_for(&tampered, UFRAG, PASSWORD, hmac),
                    "flipping byte {i} must not verify"
                );
            }
        }
    }

    #[test]
    fn everything_but_a_binding_request_with_a_username_is_refused() {
        let good = request(&format!("{UFRAG}:pageufrag"), PASSWORD, false);
        // A DTLS ClientHello, a short datagram, an indication, a response.
        assert!(parse_binding_request(&[22, 254, 253, 0, 0]).is_none());
        assert!(parse_binding_request(&good[..19]).is_none());
        for kind in [0x0011u16, 0x0101, 0x0111] {
            let mut other = good.clone();
            other[..2].copy_from_slice(&kind.to_be_bytes());
            assert!(parse_binding_request(&other).is_none(), "{kind:#06x}");
        }
        // A length that disagrees with the datagram.
        let mut truncated = good.clone();
        truncated.pop();
        assert!(parse_binding_request(&truncated).is_none());
        // No colon in USERNAME.
        let bytes = request(UFRAG, PASSWORD, false);
        assert!(parse_binding_request(&bytes).is_none());
    }
}
