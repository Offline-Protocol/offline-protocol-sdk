//! Attested publish: signing and verifying listings.
//!
//! The exchange does not own key material. The host (the protocol crate, or a
//! test harness) supplies an [`ExchangeSigner`] backed by the node's OfflineID
//! Ed25519 identity key and an [`ExchangeVerifier`] for checking peers'
//! signatures. This keeps the crate free of crypto dependencies and lets CI
//! run with deterministic test signers.

use crate::canonical::listing_signing_bytes;
use crate::error::{ExchangeError, ExchangeResult};
use crate::types::{ArtifactRef, Attestation, AttestationStatus, Listing, ListingKind, Terms};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use offline_protocol_core::ServiceDescriptor;

/// Signs exchange payloads with the node's stable identity key.
pub trait ExchangeSigner: Send + Sync {
    /// The identity public key, raw bytes.
    fn public_key(&self) -> ExchangeResult<Vec<u8>>;
    /// Signs `data`, returning the raw signature bytes.
    fn sign(&self, data: &[u8]) -> ExchangeResult<Vec<u8>>;
}

/// Verifies signatures from peers' identity keys.
pub trait ExchangeVerifier: Send + Sync {
    /// Returns `Ok(true)` when `signature` over `data` verifies under
    /// `public_key`, `Ok(false)` when it does not, and `Err` only for
    /// structural failures (malformed key, backend unavailable).
    fn verify(&self, public_key: &[u8], data: &[u8], signature: &[u8]) -> ExchangeResult<bool>;
}

/// Produces a signed attestation for a listing about to be published.
pub fn attest_listing(
    descriptor: &ServiceDescriptor,
    kind: ListingKind,
    terms: &Terms,
    artifact: Option<&ArtifactRef>,
    publisher: &str,
    signed_at_ms: u64,
    signer: &dyn ExchangeSigner,
) -> ExchangeResult<Attestation> {
    let bytes = listing_signing_bytes(descriptor, kind, terms, artifact, publisher, signed_at_ms)?;
    let public_key = signer.public_key()?;
    let signature = signer.sign(&bytes)?;
    Ok(Attestation {
        public_key: BASE64.encode(public_key),
        signature: BASE64.encode(signature),
        signed_at_ms,
    })
}

/// Verifies a discovered listing's attestation against its canonical bytes.
///
/// Returns [`AttestationStatus::Invalid`] for any decode or verification
/// failure — a malformed attestation is treated the same as a forged one.
pub fn verify_listing(listing: &Listing, verifier: &dyn ExchangeVerifier) -> AttestationStatus {
    let att = &listing.attestation;
    let Ok(public_key) = BASE64.decode(&att.public_key) else {
        return AttestationStatus::Invalid;
    };
    let Ok(signature) = BASE64.decode(&att.signature) else {
        return AttestationStatus::Invalid;
    };
    let Ok(bytes) = listing_signing_bytes(
        &listing.descriptor,
        listing.kind,
        &listing.terms,
        listing.artifact.as_ref(),
        &listing.publisher,
        att.signed_at_ms,
    ) else {
        return AttestationStatus::Invalid;
    };
    match verifier.verify(&public_key, &bytes, &signature) {
        Ok(true) => AttestationStatus::Verified,
        Ok(false) => AttestationStatus::Invalid,
        Err(_) => AttestationStatus::Invalid,
    }
}

/// Decodes the attested public key, for identity pinning.
pub fn attested_public_key(attestation: &Attestation) -> ExchangeResult<Vec<u8>> {
    BASE64
        .decode(&attestation.public_key)
        .map_err(|e| ExchangeError::VerificationFailed(format!("bad public key encoding: {e}")))
}

#[cfg(test)]
pub(crate) mod test_signer {
    //! Real Ed25519 signer/verifier for tests, backed by ed25519-dalek.

    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};

    pub struct DalekSigner {
        key: SigningKey,
    }

    impl DalekSigner {
        pub fn new(seed: u8) -> Self {
            Self {
                key: SigningKey::from_bytes(&[seed; 32]),
            }
        }
    }

    impl ExchangeSigner for DalekSigner {
        fn public_key(&self) -> ExchangeResult<Vec<u8>> {
            Ok(self.key.verifying_key().to_bytes().to_vec())
        }

        fn sign(&self, data: &[u8]) -> ExchangeResult<Vec<u8>> {
            Ok(self.key.sign(data).to_bytes().to_vec())
        }
    }

    pub struct DalekVerifier;

    impl ExchangeVerifier for DalekVerifier {
        fn verify(&self, public_key: &[u8], data: &[u8], signature: &[u8]) -> ExchangeResult<bool> {
            let key_bytes: [u8; 32] = public_key
                .try_into()
                .map_err(|_| ExchangeError::VerificationFailed("bad key length".into()))?;
            let key = VerifyingKey::from_bytes(&key_bytes)
                .map_err(|e| ExchangeError::VerificationFailed(e.to_string()))?;
            let sig_bytes: [u8; 64] = signature
                .try_into()
                .map_err(|_| ExchangeError::VerificationFailed("bad signature length".into()))?;
            let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
            Ok(key.verify(data, &sig).is_ok())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_signer::{DalekSigner, DalekVerifier};
    use super::*;
    use crate::types::Price;
    use offline_protocol_core::ServiceId;
    use std::collections::HashMap;

    fn listing(signer: &DalekSigner) -> Listing {
        let descriptor = ServiceDescriptor {
            service_id: ServiceId::new("weather.v1").unwrap(),
            version: "1.0".into(),
            capabilities: HashMap::new(),
        };
        let terms = Terms {
            price: Some(Price { amount_minor: 5 }),
            unit: crate::types::BillingUnit::PerCall,
            currency: "USD".into(),
            max_payload_kb: 64,
        };
        let attestation = attest_listing(
            &descriptor,
            ListingKind::Service,
            &terms,
            None,
            "alice",
            42,
            signer,
        )
        .unwrap();
        Listing {
            descriptor,
            kind: ListingKind::Service,
            terms,
            artifact: None,
            publisher: "alice".into(),
            attestation,
        }
    }

    #[test]
    fn attest_then_verify_roundtrip() {
        let signer = DalekSigner::new(1);
        let listing = listing(&signer);
        assert_eq!(
            verify_listing(&listing, &DalekVerifier),
            AttestationStatus::Verified
        );
    }

    #[test]
    fn tampered_terms_fail_verification() {
        let signer = DalekSigner::new(1);
        let mut listing = listing(&signer);
        listing.terms.price = Some(Price { amount_minor: 1 });
        assert_eq!(
            verify_listing(&listing, &DalekVerifier),
            AttestationStatus::Invalid
        );
    }

    #[test]
    fn tampered_publisher_fails_verification() {
        let signer = DalekSigner::new(1);
        let mut listing = listing(&signer);
        listing.publisher = "mallory".into();
        assert_eq!(
            verify_listing(&listing, &DalekVerifier),
            AttestationStatus::Invalid
        );
    }

    #[test]
    fn malformed_signature_is_invalid_not_error() {
        let signer = DalekSigner::new(1);
        let mut listing = listing(&signer);
        listing.attestation.signature = "!!!not-base64!!!".into();
        assert_eq!(
            verify_listing(&listing, &DalekVerifier),
            AttestationStatus::Invalid
        );
    }

    #[test]
    fn wrong_key_fails_verification() {
        let signer = DalekSigner::new(1);
        let other = DalekSigner::new(2);
        let mut listing = listing(&signer);
        listing.attestation.public_key = BASE64.encode(other.public_key().unwrap());
        assert_eq!(
            verify_listing(&listing, &DalekVerifier),
            AttestationStatus::Invalid
        );
    }
}
