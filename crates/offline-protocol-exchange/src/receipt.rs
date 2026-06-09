//! Signed usage receipts — the durable settlement claim.
//!
//! A receipt is issued by the **consumer** when a priced invocation completes
//! (it doubles as the debit authorization against the prepaid balance), then
//! counter-signed by the **provider** on receipt (acknowledging delivery and
//! forming the provider's claim at settlement). Either signature alone proves
//! one party's view; the dual-signed form is the strongest claim and what the
//! clearing backend prefers.

use crate::attestation::{ExchangeSigner, ExchangeVerifier};
use crate::canonical::receipt_signing_bytes;
use crate::error::{ExchangeError, ExchangeResult};
use crate::types::{BillingUnit, Terms};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Settlement lifecycle of a locally stored receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    /// Issued/received, not yet submitted to a clearing backend.
    PendingSettlement,
    /// Cleared by a settlement backend (fee applied, provider credited).
    Settled,
    /// Rejected by a settlement backend (kept for audit).
    Rejected,
}

/// A usage receipt for one priced invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReceipt {
    /// Unique receipt id (UUID).
    pub receipt_id: String,
    /// Correlation id of the invocation this receipt settles.
    pub request_id: String,
    /// The invoked listing's service id.
    pub service_id: String,
    /// The invoked listing's version at invocation time.
    pub listing_version: String,
    /// Billing unit the listing was priced in.
    pub unit: BillingUnit,
    /// Number of units consumed.
    pub unit_count: u64,
    /// Price per unit in minor units, from the listing terms.
    pub unit_price_minor: u64,
    /// Total charge in minor units (`unit_count * unit_price_minor`).
    pub total_minor: u64,
    /// Settlement currency identifier.
    pub currency: String,
    /// Consumer's stable identity (OfflineID user id).
    pub consumer_id: String,
    /// Provider's stable identity (OfflineID user id).
    pub provider_id: String,
    /// Milliseconds since epoch at issuance.
    pub issued_at_ms: u64,
    /// Consumer's Ed25519 public key, base64.
    pub consumer_public_key: String,
    /// Consumer's signature over the canonical receipt bytes, base64.
    pub consumer_signature: String,
    /// Provider's Ed25519 public key, base64. Empty until counter-signed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_public_key: String,
    /// Provider's counter-signature, base64. Empty until counter-signed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_signature: String,
}

impl UsageReceipt {
    /// Builds and consumer-signs a receipt for a completed priced invocation.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        receipt_id: String,
        request_id: String,
        service_id: String,
        listing_version: String,
        terms: &Terms,
        unit_count: u64,
        consumer_id: String,
        provider_id: String,
        issued_at_ms: u64,
        signer: &dyn ExchangeSigner,
    ) -> ExchangeResult<Self> {
        if unit_count == 0 {
            return Err(ExchangeError::InvalidReceipt(
                "unit_count must be >= 1".into(),
            ));
        }
        let unit_price_minor = terms.unit_price_minor();
        let total_minor = terms.total_for_units(unit_count)?;
        let bytes = receipt_signing_bytes(
            &receipt_id,
            &request_id,
            &service_id,
            &listing_version,
            terms.unit.as_str(),
            unit_count,
            unit_price_minor,
            total_minor,
            &terms.currency,
            &consumer_id,
            &provider_id,
            issued_at_ms,
        )?;
        let public_key = signer.public_key()?;
        let signature = signer.sign(&bytes)?;
        Ok(Self {
            receipt_id,
            request_id,
            service_id,
            listing_version,
            unit: terms.unit,
            unit_count,
            unit_price_minor,
            total_minor,
            currency: terms.currency.clone(),
            consumer_id,
            provider_id,
            issued_at_ms,
            consumer_public_key: BASE64.encode(public_key),
            consumer_signature: BASE64.encode(signature),
            provider_public_key: String::new(),
            provider_signature: String::new(),
        })
    }

    /// The canonical bytes both signatures cover.
    pub fn signing_bytes(&self) -> ExchangeResult<Vec<u8>> {
        receipt_signing_bytes(
            &self.receipt_id,
            &self.request_id,
            &self.service_id,
            &self.listing_version,
            self.unit.as_str(),
            self.unit_count,
            self.unit_price_minor,
            self.total_minor,
            &self.currency,
            &self.consumer_id,
            &self.provider_id,
            self.issued_at_ms,
        )
    }

    /// Verifies the consumer signature. Structural failures count as invalid.
    pub fn verify_consumer_signature(&self, verifier: &dyn ExchangeVerifier) -> bool {
        self.verify_one(
            &self.consumer_public_key,
            &self.consumer_signature,
            verifier,
        )
    }

    /// Verifies the provider counter-signature, when present.
    pub fn verify_provider_signature(&self, verifier: &dyn ExchangeVerifier) -> bool {
        if self.provider_signature.is_empty() {
            return false;
        }
        self.verify_one(
            &self.provider_public_key,
            &self.provider_signature,
            verifier,
        )
    }

    fn verify_one(&self, key_b64: &str, sig_b64: &str, verifier: &dyn ExchangeVerifier) -> bool {
        let Ok(key) = BASE64.decode(key_b64) else {
            return false;
        };
        let Ok(sig) = BASE64.decode(sig_b64) else {
            return false;
        };
        let Ok(bytes) = self.signing_bytes() else {
            return false;
        };
        verifier.verify(&key, &bytes, &sig).unwrap_or(false)
    }

    /// Provider counter-signs the receipt, acknowledging delivery.
    pub fn counter_sign(&mut self, signer: &dyn ExchangeSigner) -> ExchangeResult<()> {
        let bytes = self.signing_bytes()?;
        let public_key = signer.public_key()?;
        let signature = signer.sign(&bytes)?;
        self.provider_public_key = BASE64.encode(public_key);
        self.provider_signature = BASE64.encode(signature);
        Ok(())
    }

    /// Validates a receipt received from a consumer against the published
    /// listing terms it claims to settle. Run on the provider before storing.
    pub fn validate_against_terms(&self, terms: &Terms) -> ExchangeResult<()> {
        if self.unit != terms.unit {
            return Err(ExchangeError::InvalidReceipt(format!(
                "receipt unit {} does not match listing unit {}",
                self.unit.as_str(),
                terms.unit.as_str()
            )));
        }
        if self.unit_price_minor != terms.unit_price_minor() {
            return Err(ExchangeError::InvalidReceipt(format!(
                "receipt unit price {} does not match listing price {}",
                self.unit_price_minor,
                terms.unit_price_minor()
            )));
        }
        if self.currency != terms.currency {
            return Err(ExchangeError::InvalidReceipt(format!(
                "receipt currency {} does not match listing currency {}",
                self.currency, terms.currency
            )));
        }
        let expected_total = terms.total_for_units(self.unit_count)?;
        if self.total_minor != expected_total {
            return Err(ExchangeError::InvalidReceipt(format!(
                "receipt total {} does not match {} units at {} minor",
                self.total_minor, self.unit_count, self.unit_price_minor
            )));
        }
        if self.unit_count == 0 {
            return Err(ExchangeError::InvalidReceipt(
                "unit_count must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

/// A stored receipt with its local settlement status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredReceipt {
    /// The receipt itself.
    pub receipt: UsageReceipt,
    /// Local settlement status.
    pub status: ReceiptStatus,
    /// Whether this node was the consumer (payer) on this receipt.
    pub local_role_consumer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attestation::test_signer::{DalekSigner, DalekVerifier};
    use crate::types::Price;

    fn terms() -> Terms {
        Terms {
            price: Some(Price { amount_minor: 10 }),
            unit: BillingUnit::PerToken,
            currency: "USD".into(),
            max_payload_kb: 64,
        }
    }

    fn issue(signer: &DalekSigner) -> UsageReceipt {
        UsageReceipt::issue(
            "r-1".into(),
            "req-1".into(),
            "llm.summarize".into(),
            "1.0".into(),
            &terms(),
            3,
            "alice".into(),
            "bob".into(),
            1000,
            signer,
        )
        .unwrap()
    }

    #[test]
    fn issue_and_verify_consumer_signature() {
        let signer = DalekSigner::new(1);
        let receipt = issue(&signer);
        assert_eq!(receipt.total_minor, 30);
        assert!(receipt.verify_consumer_signature(&DalekVerifier));
        assert!(!receipt.verify_provider_signature(&DalekVerifier));
    }

    #[test]
    fn counter_sign_and_verify() {
        let consumer = DalekSigner::new(1);
        let provider = DalekSigner::new(2);
        let mut receipt = issue(&consumer);
        receipt.counter_sign(&provider).unwrap();
        assert!(receipt.verify_consumer_signature(&DalekVerifier));
        assert!(receipt.verify_provider_signature(&DalekVerifier));
    }

    #[test]
    fn tampered_total_fails_signature() {
        let signer = DalekSigner::new(1);
        let mut receipt = issue(&signer);
        receipt.total_minor = 1;
        assert!(!receipt.verify_consumer_signature(&DalekVerifier));
    }

    #[test]
    fn validate_against_terms_catches_mismatches() {
        let signer = DalekSigner::new(1);
        let receipt = issue(&signer);
        assert!(receipt.validate_against_terms(&terms()).is_ok());

        let mut cheaper = terms();
        cheaper.price = Some(Price { amount_minor: 5 });
        assert!(receipt.validate_against_terms(&cheaper).is_err());

        let mut other_currency = terms();
        other_currency.currency = "EUR".into();
        assert!(receipt.validate_against_terms(&other_currency).is_err());

        let mut other_unit = terms();
        other_unit.unit = BillingUnit::PerCall;
        assert!(receipt.validate_against_terms(&other_unit).is_err());
    }

    #[test]
    fn zero_units_rejected() {
        let signer = DalekSigner::new(1);
        let result = UsageReceipt::issue(
            "r".into(),
            "q".into(),
            "s".into(),
            "1.0".into(),
            &terms(),
            0,
            "a".into(),
            "b".into(),
            1,
            &signer,
        );
        assert!(matches!(result, Err(ExchangeError::InvalidReceipt(_))));
    }

    #[test]
    fn serde_roundtrip_preserves_signatures() {
        let consumer = DalekSigner::new(1);
        let provider = DalekSigner::new(2);
        let mut receipt = issue(&consumer);
        receipt.counter_sign(&provider).unwrap();
        let json = serde_json::to_string(&receipt).unwrap();
        let parsed: UsageReceipt = serde_json::from_str(&json).unwrap();
        assert!(parsed.verify_consumer_signature(&DalekVerifier));
        assert!(parsed.verify_provider_signature(&DalekVerifier));
    }
}
