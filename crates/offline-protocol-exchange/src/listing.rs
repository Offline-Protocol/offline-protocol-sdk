//! Embedding listings in service descriptors and extracting them on discovery.
//!
//! The exchange never changes the service-discovery wire format. A listing is
//! a [`ListingEnvelope`] serialized to JSON and stored in the descriptor's
//! `capabilities` map under [`LISTING_CAPABILITY_KEY`]. Exchange-unaware nodes
//! see a normal descriptor; exchange-aware nodes reconstruct the listing.

use crate::error::{ExchangeError, ExchangeResult};
use crate::types::{
    Listing, ListingEnvelope, ListingKind, LISTING_CAPABILITY_KEY, LISTING_ENVELOPE_VERSION,
    MAX_LISTING_ENVELOPE_BYTES,
};
use offline_protocol_core::ServiceDescriptor;
use std::collections::HashMap;

/// Embeds a listing's envelope into its descriptor's capabilities map,
/// returning the descriptor to register with service discovery.
pub fn embed_listing(listing: &Listing) -> ExchangeResult<ServiceDescriptor> {
    let envelope = ListingEnvelope {
        v: LISTING_ENVELOPE_VERSION,
        kind: listing.kind,
        terms: listing.terms.clone(),
        artifact: listing.artifact.clone(),
        publisher: listing.publisher.clone(),
        attestation: listing.attestation.clone(),
    };
    let json = serde_json::to_string(&envelope)
        .map_err(|e| ExchangeError::Serialization(e.to_string()))?;
    if json.len() > MAX_LISTING_ENVELOPE_BYTES {
        return Err(ExchangeError::InvalidListing(format!(
            "listing envelope is {} bytes, exceeds {} byte limit",
            json.len(),
            MAX_LISTING_ENVELOPE_BYTES
        )));
    }
    let mut descriptor = listing.descriptor.clone();
    descriptor
        .capabilities
        .insert(LISTING_CAPABILITY_KEY.to_string(), json);
    Ok(descriptor)
}

/// Attempts to extract a listing from discovered service metadata.
///
/// Returns `Ok(None)` when the capabilities carry no listing envelope (a
/// plain, exchange-unaware service). Returns `Err` when an envelope is
/// present but malformed or from an unsupported version — callers should
/// treat that listing as untrusted.
pub fn extract_listing(descriptor: &ServiceDescriptor) -> ExchangeResult<Option<Listing>> {
    let Some(json) = descriptor.capabilities.get(LISTING_CAPABILITY_KEY) else {
        return Ok(None);
    };
    if json.len() > MAX_LISTING_ENVELOPE_BYTES {
        return Err(ExchangeError::InvalidEnvelope(format!(
            "envelope is {} bytes, exceeds {} byte limit",
            json.len(),
            MAX_LISTING_ENVELOPE_BYTES
        )));
    }
    let envelope: ListingEnvelope =
        serde_json::from_str(json).map_err(|e| ExchangeError::InvalidEnvelope(e.to_string()))?;
    if envelope.v != LISTING_ENVELOPE_VERSION {
        return Err(ExchangeError::UnsupportedEnvelopeVersion(envelope.v));
    }
    if envelope.kind == ListingKind::Adapter && envelope.artifact.is_none() {
        return Err(ExchangeError::InvalidEnvelope(
            "adapter listing without artifact reference".into(),
        ));
    }
    if envelope.kind == ListingKind::Service && envelope.artifact.is_some() {
        return Err(ExchangeError::InvalidEnvelope(
            "service listing must not carry an artifact reference".into(),
        ));
    }
    if envelope.publisher.trim().is_empty() {
        return Err(ExchangeError::InvalidEnvelope(
            "listing without publisher identity".into(),
        ));
    }
    Ok(Some(Listing {
        descriptor: descriptor.clone(),
        kind: envelope.kind,
        terms: envelope.terms,
        artifact: envelope.artifact,
        publisher: envelope.publisher,
        attestation: envelope.attestation,
    }))
}

/// Reconstructs a descriptor from the fields a `ServiceDiscovered` event
/// carries (the event flattens the descriptor into id/version/capabilities).
pub fn descriptor_from_discovery(
    service_id: &str,
    version: &str,
    capabilities: HashMap<String, String>,
) -> ExchangeResult<ServiceDescriptor> {
    let service_id = offline_protocol_core::ServiceId::new(service_id)
        .map_err(|e| ExchangeError::InvalidEnvelope(format!("bad service id: {e}")))?;
    Ok(ServiceDescriptor {
        service_id,
        version: version.to_string(),
        capabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Attestation, Terms};
    use offline_protocol_core::ServiceId;

    fn listing() -> Listing {
        Listing {
            descriptor: ServiceDescriptor {
                service_id: ServiceId::new("echo.v1").unwrap(),
                version: "1.0".into(),
                capabilities: HashMap::from([("format".into(), "json".into())]),
            },
            kind: ListingKind::Service,
            terms: Terms::free(),
            artifact: None,
            publisher: "alice".into(),
            attestation: Attestation {
                public_key: "cGs=".into(),
                signature: "c2ln".into(),
                signed_at_ms: 1,
            },
        }
    }

    #[test]
    fn embed_extract_roundtrip() {
        let original = listing();
        let descriptor = embed_listing(&original).unwrap();
        // Original capability entries are preserved alongside the envelope.
        assert_eq!(descriptor.capabilities.get("format").unwrap(), "json");
        assert!(descriptor.capabilities.contains_key(LISTING_CAPABILITY_KEY));

        let extracted = extract_listing(&descriptor).unwrap().unwrap();
        assert_eq!(extracted.kind, original.kind);
        assert_eq!(extracted.terms, original.terms);
        assert_eq!(extracted.publisher, original.publisher);
        assert_eq!(extracted.attestation, original.attestation);
    }

    #[test]
    fn plain_descriptor_extracts_none() {
        let descriptor = ServiceDescriptor {
            service_id: ServiceId::new("plain").unwrap(),
            version: "1.0".into(),
            capabilities: HashMap::new(),
        };
        assert!(extract_listing(&descriptor).unwrap().is_none());
    }

    #[test]
    fn malformed_envelope_errors() {
        let descriptor = ServiceDescriptor {
            service_id: ServiceId::new("bad").unwrap(),
            version: "1.0".into(),
            capabilities: HashMap::from([(LISTING_CAPABILITY_KEY.into(), "{not json".into())]),
        };
        assert!(matches!(
            extract_listing(&descriptor),
            Err(ExchangeError::InvalidEnvelope(_))
        ));
    }

    #[test]
    fn future_version_rejected() {
        let mut l = listing();
        let descriptor = embed_listing(&l).unwrap();
        let json = descriptor
            .capabilities
            .get(LISTING_CAPABILITY_KEY)
            .unwrap()
            .replace("\"v\":1", "\"v\":999");
        l.descriptor
            .capabilities
            .insert(LISTING_CAPABILITY_KEY.into(), json);
        assert!(matches!(
            extract_listing(&l.descriptor),
            Err(ExchangeError::UnsupportedEnvelopeVersion(999))
        ));
    }

    #[test]
    fn adapter_without_artifact_rejected() {
        let mut l = listing();
        l.kind = ListingKind::Adapter;
        let descriptor = embed_listing(&l).unwrap();
        assert!(matches!(
            extract_listing(&descriptor),
            Err(ExchangeError::InvalidEnvelope(_))
        ));
    }

    #[test]
    fn oversized_envelope_rejected_at_embed() {
        let mut l = listing();
        l.publisher = "x".repeat(MAX_LISTING_ENVELOPE_BYTES);
        assert!(matches!(
            embed_listing(&l),
            Err(ExchangeError::InvalidListing(_))
        ));
    }
}
