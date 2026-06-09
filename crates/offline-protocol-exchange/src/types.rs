//! Core data model for the capability exchange.
//!
//! A [`Listing`] wraps the existing `ServiceDescriptor` with commercial terms,
//! provenance (an OfflineID attestation), and — for adapters — a
//! content-addressed artifact reference. The descriptor itself stays unchanged
//! on the wire: the listing rides inside the descriptor's `capabilities` map
//! under [`LISTING_CAPABILITY_KEY`], so exchange-unaware nodes see a normal
//! service and ignore it.

use crate::error::{ExchangeError, ExchangeResult};
use offline_protocol_core::ServiceDescriptor;
use serde::{Deserialize, Serialize};

/// Version of the listing envelope wire format. Bump when the envelope schema
/// changes incompatibly; older readers must reject newer versions.
pub const LISTING_ENVELOPE_VERSION: u16 = 1;

/// Reserved key in `ServiceDescriptor.capabilities` carrying the serialized
/// [`ListingEnvelope`]. Exchange-unaware discovery consumers ignore this key.
pub const LISTING_CAPABILITY_KEY: &str = "x-op-listing";

/// Maximum serialized envelope size. Keeps listing-bearing descriptors well
/// under the 128 KB service payload limit even with other capability entries.
pub const MAX_LISTING_ENVELOPE_BYTES: usize = 8 * 1024;

/// Reserved request method for pulling an adapter artifact from a provider.
pub const ADAPTER_PULL_METHOD: &str = "exchange.adapter.pull";

/// What kind of capability a listing offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingKind {
    /// A request/response capability another node invokes remotely.
    Service,
    /// A model adapter artifact a consumer pulls to gain a local capability.
    Adapter,
}

impl ListingKind {
    /// Stable string for logging and FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Adapter => "adapter",
        }
    }
}

/// The unit an invocation is billed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingUnit {
    /// One unit per request/response round trip.
    PerCall,
    /// Units declared by the provider per invocation (e.g. tokens generated).
    PerToken,
    /// Units declared by the provider per invocation (elapsed seconds).
    PerSecond,
    /// A single flat charge per invocation regardless of usage.
    Flat,
}

impl BillingUnit {
    /// Whether the provider declares the consumed unit count after responding.
    /// `PerCall` and `Flat` always bill exactly one unit.
    pub fn is_metered(&self) -> bool {
        matches!(self, Self::PerToken | Self::PerSecond)
    }

    /// Stable string for logging and FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PerCall => "per_call",
            Self::PerToken => "per_token",
            Self::PerSecond => "per_second",
            Self::Flat => "flat",
        }
    }
}

/// Price per billing unit, in minor units of the listing currency (e.g. cents
/// for USD, micro-units for stablecoins — the currency id defines the scale).
/// Integer minor units avoid floating-point money errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    /// Amount per unit in minor units.
    pub amount_minor: u64,
}

/// Commercial terms attached to a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terms {
    /// Price per billing unit. `None` means the listing is free to invoke.
    pub price: Option<Price>,
    /// The unit invocations are billed in.
    pub unit: BillingUnit,
    /// Settlement currency identifier (e.g. "USD", "USDC").
    pub currency: String,
    /// Maximum request/response payload the provider accepts, in KB.
    pub max_payload_kb: u32,
}

impl Terms {
    /// Free-to-invoke terms (no price).
    pub fn free() -> Self {
        Self {
            price: None,
            unit: BillingUnit::PerCall,
            currency: String::new(),
            max_payload_kb: 64,
        }
    }

    /// Whether invoking under these terms costs money.
    pub fn is_priced(&self) -> bool {
        self.price.is_some_and(|p| p.amount_minor > 0)
    }

    /// Price per unit in minor units (0 when free).
    pub fn unit_price_minor(&self) -> u64 {
        self.price.map(|p| p.amount_minor).unwrap_or(0)
    }

    /// Total cost for `units` units, checked against overflow.
    pub fn total_for_units(&self, units: u64) -> ExchangeResult<u64> {
        self.unit_price_minor()
            .checked_mul(units)
            .ok_or_else(|| ExchangeError::AmountOverflow(format!("{units} units overflow price")))
    }

    /// Validates the terms for publish.
    pub fn validate(&self) -> ExchangeResult<()> {
        if self.is_priced() && self.currency.trim().is_empty() {
            return Err(ExchangeError::InvalidListing(
                "priced terms require a currency".into(),
            ));
        }
        if self.max_payload_kb == 0 {
            return Err(ExchangeError::InvalidListing(
                "max_payload_kb must be > 0".into(),
            ));
        }
        Ok(())
    }
}

/// How an adapter artifact is split for transfer. The metadata (this struct)
/// gossips with the listing over any transport; the weights themselves move
/// via the file-transfer path, which DORS routes to a high-bandwidth
/// transport (WiFi Direct / Internet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkPlan {
    /// Preferred chunk size for the bulk transfer, in bytes.
    pub chunk_size_bytes: u32,
}

/// Content-addressed reference to an adapter artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// SHA-256 of the artifact bytes, lowercase hex. Verified on arrival;
    /// a mismatch rejects the artifact.
    pub content_hash: String,
    /// Exact artifact size in bytes.
    pub size_bytes: u64,
    /// The base model this adapter is welded to.
    pub base_model: String,
    /// Version of the base model.
    pub base_model_version: String,
    /// Transfer chunking metadata.
    pub chunking: ChunkPlan,
}

impl ArtifactRef {
    /// Validates the reference for publish.
    pub fn validate(&self) -> ExchangeResult<()> {
        if self.content_hash.len() != 64
            || !self.content_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ExchangeError::InvalidListing(
                "content_hash must be 64 lowercase hex chars (SHA-256)".into(),
            ));
        }
        if self.content_hash.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(ExchangeError::InvalidListing(
                "content_hash must be lowercase hex".into(),
            ));
        }
        if self.size_bytes == 0 {
            return Err(ExchangeError::InvalidListing(
                "artifact size_bytes must be > 0".into(),
            ));
        }
        if self.base_model.trim().is_empty() {
            return Err(ExchangeError::InvalidListing(
                "artifact base_model is required".into(),
            ));
        }
        Ok(())
    }
}

/// Publisher signature over the listing contents (and the artifact hash for
/// adapters). Provenance is mandatory: consumers verify before invoking a
/// paid service or loading an adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// Publisher's Ed25519 public key, base64.
    pub public_key: String,
    /// Ed25519 signature over the canonical listing bytes, base64.
    pub signature: String,
    /// Milliseconds since epoch when the attestation was produced.
    pub signed_at_ms: u64,
}

/// Outcome of verifying a listing's attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationStatus {
    /// Signature verified against the canonical listing bytes.
    Verified,
    /// Signature present but failed verification (or publisher key changed).
    Invalid,
    /// No attestation present (legacy/non-exchange descriptor).
    Unsigned,
}

impl AttestationStatus {
    /// Stable string for logging and FFI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Invalid => "invalid",
            Self::Unsigned => "unsigned",
        }
    }
}

/// Wire form of a listing, embedded in `ServiceDescriptor.capabilities` under
/// [`LISTING_CAPABILITY_KEY`]. The descriptor's own `service_id` and `version`
/// stay authoritative; the envelope carries only the exchange extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingEnvelope {
    /// Envelope schema version. See [`LISTING_ENVELOPE_VERSION`].
    pub v: u16,
    /// What kind of capability this is.
    pub kind: ListingKind,
    /// Commercial terms.
    pub terms: Terms,
    /// Artifact reference; present iff `kind == Adapter`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRef>,
    /// Stable publisher identity (OfflineID user id).
    pub publisher: String,
    /// Publisher signature and claims.
    pub attestation: Attestation,
}

/// A full listing as surfaced to applications: the unchanged descriptor plus
/// the exchange extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listing {
    /// The underlying service descriptor (service_id, version, capabilities).
    pub descriptor: ServiceDescriptor,
    /// What kind of capability this is.
    pub kind: ListingKind,
    /// Commercial terms.
    pub terms: Terms,
    /// Artifact reference; present iff `kind == Adapter`.
    pub artifact: Option<ArtifactRef>,
    /// Stable publisher identity (OfflineID user id).
    pub publisher: String,
    /// Publisher signature and claims.
    pub attestation: Attestation,
}

impl Listing {
    /// The listing's service id as a string slice.
    pub fn service_id(&self) -> &str {
        self.descriptor.service_id.as_str()
    }
}

/// Filter for discovery results surfaced to applications.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListingFilter {
    /// Only listings of this kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ListingKind>,
    /// `Some(true)` = only free listings, `Some(false)` = only paid listings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free: Option<bool>,
    /// Only this service id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
}

impl ListingFilter {
    /// Whether a listing passes this filter.
    pub fn matches(&self, listing: &Listing) -> bool {
        if let Some(kind) = self.kind {
            if listing.kind != kind {
                return false;
            }
        }
        if let Some(free) = self.free {
            if listing.terms.is_priced() == free {
                return false;
            }
        }
        if let Some(ref sid) = self.service_id {
            if listing.service_id() != sid {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> ArtifactRef {
        ArtifactRef {
            content_hash: "a".repeat(64),
            size_bytes: 1024,
            base_model: "gemma-3-1b".into(),
            base_model_version: "1.0".into(),
            chunking: ChunkPlan {
                chunk_size_bytes: 65536,
            },
        }
    }

    #[test]
    fn terms_free_is_not_priced() {
        assert!(!Terms::free().is_priced());
        assert_eq!(Terms::free().unit_price_minor(), 0);
    }

    #[test]
    fn terms_priced_total() {
        let terms = Terms {
            price: Some(Price { amount_minor: 25 }),
            unit: BillingUnit::PerToken,
            currency: "USD".into(),
            max_payload_kb: 64,
        };
        assert!(terms.is_priced());
        assert_eq!(terms.total_for_units(4).unwrap(), 100);
    }

    #[test]
    fn terms_total_overflow_rejected() {
        let terms = Terms {
            price: Some(Price {
                amount_minor: u64::MAX,
            }),
            unit: BillingUnit::PerCall,
            currency: "USD".into(),
            max_payload_kb: 64,
        };
        assert!(matches!(
            terms.total_for_units(2),
            Err(ExchangeError::AmountOverflow(_))
        ));
    }

    #[test]
    fn terms_priced_requires_currency() {
        let terms = Terms {
            price: Some(Price { amount_minor: 1 }),
            unit: BillingUnit::PerCall,
            currency: "  ".into(),
            max_payload_kb: 64,
        };
        assert!(matches!(
            terms.validate(),
            Err(ExchangeError::InvalidListing(_))
        ));
    }

    #[test]
    fn zero_price_counts_as_free() {
        let terms = Terms {
            price: Some(Price { amount_minor: 0 }),
            unit: BillingUnit::PerCall,
            currency: "USD".into(),
            max_payload_kb: 64,
        };
        assert!(!terms.is_priced());
    }

    #[test]
    fn artifact_validation() {
        assert!(artifact().validate().is_ok());

        let mut bad = artifact();
        bad.content_hash = "xyz".into();
        assert!(bad.validate().is_err());

        let mut bad = artifact();
        bad.content_hash = "A".repeat(64);
        assert!(bad.validate().is_err());

        let mut bad = artifact();
        bad.size_bytes = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn billing_unit_metered() {
        assert!(!BillingUnit::PerCall.is_metered());
        assert!(!BillingUnit::Flat.is_metered());
        assert!(BillingUnit::PerToken.is_metered());
        assert!(BillingUnit::PerSecond.is_metered());
    }

    #[test]
    fn envelope_serde_roundtrip() {
        let env = ListingEnvelope {
            v: LISTING_ENVELOPE_VERSION,
            kind: ListingKind::Adapter,
            terms: Terms::free(),
            artifact: Some(artifact()),
            publisher: "alice".into(),
            attestation: Attestation {
                public_key: "cGs=".into(),
                signature: "c2ln".into(),
                signed_at_ms: 1,
            },
        };
        let json = serde_json::to_string(&env).unwrap();
        let parsed: ListingEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, parsed);
    }
}
