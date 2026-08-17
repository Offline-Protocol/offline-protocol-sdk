//! The house construction for domain-separated signing payloads.
//!
//! Every signature in this protocol is taken over
//! `domain ‖ Σ(u32be(len) ‖ field_bytes)`. The length prefix is what makes the
//! encoding unambiguous: without it, two different field splits can serialize
//! to the same bytes, and a signature over one is a valid signature over the
//! other. The domain is what stops a signature produced in one context from
//! being replayed in another that happens to reuse the same identity key.
//!
//! The construction is duplicated in three other places on purpose, because
//! they are different codebases and a shared crate would not reach them:
//! `OfflineProtocol::build_canonical_payload` (control frames), the relay
//! server's `address_proof_payload`, and the two bridge implementations of the
//! relay address proof. What keeps them honest is that the domains must be
//! mutually non-prefixing, which
//! `signing_domains_are_mutually_non_prefixing` pins over all four.
//!
//! # Why mutual non-prefixing matters
//!
//! If one domain were a prefix of another, the shorter domain's payload could
//! be made to collide with the longer one's by choosing a first field that
//! supplies the remaining domain bytes. The signature would then verify in a
//! context it was never issued for. Length-prefixing the fields does not
//! prevent this on its own, because the domain itself is not length-prefixed.

use crate::error::{MlsError, Result};

/// Builds `domain ‖ Σ(u32be(len) ‖ field)`.
///
/// # Errors
///
/// Returns [`MlsError::Serialization`] if a field exceeds `u32::MAX` bytes,
/// which no caller in this crate can reach — every field is bounded far below
/// it — but which must not silently truncate a length prefix if one ever could.
pub(crate) fn canonical_payload(domain: &[u8], fields: &[&[u8]]) -> Result<Vec<u8>> {
    let mut buf =
        Vec::with_capacity(domain.len() + fields.iter().map(|f| 4 + f.len()).sum::<usize>());
    buf.extend_from_slice(domain);
    for field in fields {
        let len: u32 = field.len().try_into().map_err(|_| {
            MlsError::Serialization(format!(
                "Field too large for canonical payload length prefix: {} bytes",
                field.len()
            ))
        })?;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(field);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_payload_length_prefixes_every_field() {
        let payload = canonical_payload(b"dom", &[b"ab", b"c"]).expect("payload");
        assert_eq!(payload, b"dom\x00\x00\x00\x02ab\x00\x00\x00\x01c".to_vec());
    }

    /// The property the length prefix buys: two different field splits that
    /// concatenate to the same bytes must not produce the same payload.
    #[test]
    fn canonical_payload_distinguishes_field_splits() {
        let a = canonical_payload(b"dom", &[b"ab", b"c"]).expect("payload");
        let b = canonical_payload(b"dom", &[b"a", b"bc"]).expect("payload");
        assert_ne!(a, b);
    }

    #[test]
    fn canonical_payload_distinguishes_domains() {
        let a = canonical_payload(b"dom-a", &[b"x"]).expect("payload");
        let b = canonical_payload(b"dom-b", &[b"x"]).expect("payload");
        assert_ne!(a, b);
    }
}
