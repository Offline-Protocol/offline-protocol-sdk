//! Canonical byte encodings for signatures.
//!
//! Every signed structure is reduced to an unambiguous byte string before
//! signing: a domain-separation tag followed by each field encoded as
//! `<4-byte big-endian length><utf-8 bytes>`. Length prefixes remove any
//! delimiter-collision risk regardless of field content. This mirrors the
//! control-message signing scheme in the protocol crate.

use crate::error::{ExchangeError, ExchangeResult};
use crate::types::{ArtifactRef, ListingKind, Terms};
use offline_protocol_core::ServiceDescriptor;

/// Domain tag for listing attestations.
const LISTING_SIGN_DOMAIN: &[u8] = b"OP-EXCHANGE-LISTING-V1";

/// Domain tag for usage receipts.
const RECEIPT_SIGN_DOMAIN: &[u8] = b"OP-EXCHANGE-RECEIPT-V1";

/// Appends one length-prefixed field to the buffer.
fn push_field(buf: &mut Vec<u8>, field: &str) -> ExchangeResult<()> {
    let len: u32 = field.len().try_into().map_err(|_| {
        ExchangeError::Serialization(format!("field too large: {} bytes", field.len()))
    })?;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(field.as_bytes());
    Ok(())
}

/// Builds the canonical bytes a publisher signs for a listing.
///
/// Covers the descriptor identity (service id + version), the kind, the full
/// terms, the artifact reference (empty marker when absent), the publisher
/// identity, and the attestation timestamp. The descriptor's free-form
/// capability entries are *not* covered: they are advertisory metadata, and
/// excluding them lets a provider refresh e.g. coverage hints without
/// re-attesting. Everything a consumer relies on to pay or load is covered.
pub fn listing_signing_bytes(
    descriptor: &ServiceDescriptor,
    kind: ListingKind,
    terms: &Terms,
    artifact: Option<&ArtifactRef>,
    publisher: &str,
    signed_at_ms: u64,
) -> ExchangeResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(LISTING_SIGN_DOMAIN);
    push_field(&mut buf, descriptor.service_id.as_str())?;
    push_field(&mut buf, &descriptor.version)?;
    push_field(&mut buf, kind.as_str())?;
    push_field(&mut buf, &terms.unit_price_minor().to_string())?;
    push_field(&mut buf, terms.unit.as_str())?;
    push_field(&mut buf, &terms.currency)?;
    push_field(&mut buf, &terms.max_payload_kb.to_string())?;
    match artifact {
        Some(a) => {
            push_field(&mut buf, &a.content_hash)?;
            push_field(&mut buf, &a.size_bytes.to_string())?;
            push_field(&mut buf, &a.base_model)?;
            push_field(&mut buf, &a.base_model_version)?;
            push_field(&mut buf, &a.chunking.chunk_size_bytes.to_string())?;
        }
        None => push_field(&mut buf, "")?,
    }
    push_field(&mut buf, publisher)?;
    push_field(&mut buf, &signed_at_ms.to_string())?;
    Ok(buf)
}

/// Field set covered by a receipt signature. Both the consumer signature and
/// the provider counter-signature cover the same bytes, so either party can
/// present the receipt as a settlement claim.
#[allow(clippy::too_many_arguments)]
pub fn receipt_signing_bytes(
    receipt_id: &str,
    request_id: &str,
    service_id: &str,
    listing_version: &str,
    unit: &str,
    unit_count: u64,
    unit_price_minor: u64,
    total_minor: u64,
    currency: &str,
    consumer_id: &str,
    provider_id: &str,
    issued_at_ms: u64,
) -> ExchangeResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(RECEIPT_SIGN_DOMAIN);
    push_field(&mut buf, receipt_id)?;
    push_field(&mut buf, request_id)?;
    push_field(&mut buf, service_id)?;
    push_field(&mut buf, listing_version)?;
    push_field(&mut buf, unit)?;
    push_field(&mut buf, &unit_count.to_string())?;
    push_field(&mut buf, &unit_price_minor.to_string())?;
    push_field(&mut buf, &total_minor.to_string())?;
    push_field(&mut buf, currency)?;
    push_field(&mut buf, consumer_id)?;
    push_field(&mut buf, provider_id)?;
    push_field(&mut buf, &issued_at_ms.to_string())?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChunkPlan, Price};
    use offline_protocol_core::ServiceId;
    use std::collections::HashMap;

    fn descriptor() -> ServiceDescriptor {
        ServiceDescriptor {
            service_id: ServiceId::new("weather.v1").unwrap(),
            version: "1.0".into(),
            capabilities: HashMap::new(),
        }
    }

    fn terms() -> Terms {
        Terms {
            price: Some(Price { amount_minor: 5 }),
            unit: crate::types::BillingUnit::PerCall,
            currency: "USD".into(),
            max_payload_kb: 64,
        }
    }

    #[test]
    fn listing_bytes_deterministic() {
        let a = listing_signing_bytes(
            &descriptor(),
            ListingKind::Service,
            &terms(),
            None,
            "alice",
            7,
        )
        .unwrap();
        let b = listing_signing_bytes(
            &descriptor(),
            ListingKind::Service,
            &terms(),
            None,
            "alice",
            7,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn listing_bytes_change_with_price() {
        let a = listing_signing_bytes(
            &descriptor(),
            ListingKind::Service,
            &terms(),
            None,
            "alice",
            7,
        )
        .unwrap();
        let mut t = terms();
        t.price = Some(Price { amount_minor: 6 });
        let b = listing_signing_bytes(&descriptor(), ListingKind::Service, &t, None, "alice", 7)
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn listing_bytes_change_with_artifact() {
        let artifact = ArtifactRef {
            content_hash: "b".repeat(64),
            size_bytes: 10,
            base_model: "m".into(),
            base_model_version: "1".into(),
            chunking: ChunkPlan {
                chunk_size_bytes: 1024,
            },
        };
        let a = listing_signing_bytes(
            &descriptor(),
            ListingKind::Adapter,
            &terms(),
            None,
            "alice",
            7,
        )
        .unwrap();
        let b = listing_signing_bytes(
            &descriptor(),
            ListingKind::Adapter,
            &terms(),
            Some(&artifact),
            "alice",
            7,
        )
        .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn no_field_concatenation_ambiguity() {
        // ("ab", "c") must not collide with ("a", "bc").
        let mut d1 = descriptor();
        d1.version = "ab".into();
        let mut d2 = descriptor();
        d2.version = "a".into();
        let a = listing_signing_bytes(&d1, ListingKind::Service, &terms(), None, "c", 7).unwrap();
        let b = listing_signing_bytes(&d2, ListingKind::Service, &terms(), None, "bc", 7).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn receipt_bytes_deterministic_and_sensitive() {
        let a = receipt_signing_bytes(
            "r", "q", "s", "1.0", "per_call", 1, 5, 5, "USD", "c", "p", 9,
        )
        .unwrap();
        let b = receipt_signing_bytes(
            "r", "q", "s", "1.0", "per_call", 1, 5, 5, "USD", "c", "p", 9,
        )
        .unwrap();
        assert_eq!(a, b);
        let c = receipt_signing_bytes(
            "r", "q", "s", "1.0", "per_call", 2, 5, 10, "USD", "c", "p", 9,
        )
        .unwrap();
        assert_ne!(a, c);
    }
}
