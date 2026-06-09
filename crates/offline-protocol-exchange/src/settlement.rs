//! Settlement backends: where receipts become money movement.
//!
//! Settlement is **eventual**: receipts are the durable claim, and a backend
//! clears them when connectivity allows. The protocol fee (take-rate) is
//! applied at settlement, never on the mesh. The trait is deliberately
//! synchronous and transport-agnostic — production backends (e.g. the Stripe
//! clearing service) live behind an HTTP client owned by the host app, while
//! CI uses [`MockClearing`].

use crate::error::{ExchangeError, ExchangeResult};
use crate::receipt::UsageReceipt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Outcome of submitting a batch of receipts for clearing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementReport {
    /// Receipt ids that cleared.
    pub settled: Vec<String>,
    /// Receipt ids that were rejected, with reasons.
    pub rejected: Vec<(String, String)>,
    /// Total protocol fees collected across settled receipts, in minor units
    /// keyed by currency.
    pub fees_minor: HashMap<String, u64>,
    /// Net amounts credited to providers, in minor units keyed by currency.
    pub net_to_providers_minor: HashMap<String, u64>,
}

/// A clearing backend that settles signed usage receipts.
pub trait SettlementBackend: Send + Sync {
    /// Stable identifier for logs and receipts (e.g. "mock", "stripe").
    fn backend_id(&self) -> &str;

    /// Protocol take-rate in basis points, applied to each settled receipt.
    fn protocol_fee_bps(&self) -> u32;

    /// Submits receipts for clearing. Receipts the backend cannot verify or
    /// fund are rejected individually — one bad receipt never fails the batch.
    fn submit_receipts(&self, receipts: &[UsageReceipt]) -> ExchangeResult<SettlementReport>;
}

/// In-memory clearing backend for CI and integration tests.
///
/// Verifies receipt structure, applies the configured fee, and accumulates
/// per-identity account balances so tests can assert on money movement.
pub struct MockClearing {
    fee_bps: u32,
    /// (identity, currency) -> net minor units credited.
    accounts: Mutex<HashMap<(String, String), u64>>,
    /// currency -> accumulated protocol fees.
    fees: Mutex<HashMap<String, u64>>,
    /// Receipt ids already settled (idempotency guard).
    settled_ids: Mutex<std::collections::HashSet<String>>,
}

impl MockClearing {
    /// Creates a mock backend with the given take-rate in basis points.
    pub fn new(fee_bps: u32) -> Self {
        Self {
            fee_bps,
            accounts: Mutex::new(HashMap::new()),
            fees: Mutex::new(HashMap::new()),
            settled_ids: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Net minor units credited to an identity in a currency.
    pub fn account_balance(&self, identity: &str, currency: &str) -> u64 {
        self.accounts
            .lock()
            .map(|a| {
                a.get(&(identity.to_string(), currency.to_string()))
                    .copied()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// Accumulated protocol fees in a currency.
    pub fn collected_fees(&self, currency: &str) -> u64 {
        self.fees
            .lock()
            .map(|f| f.get(currency).copied().unwrap_or(0))
            .unwrap_or(0)
    }
}

/// Computes the protocol fee for a settled amount, rounding the fee up so
/// dust never escapes the take-rate. Returns `(fee, net_to_provider)`.
pub fn split_fee(total_minor: u64, fee_bps: u32) -> ExchangeResult<(u64, u64)> {
    let fee_bps = u64::from(fee_bps.min(10_000));
    let product = (total_minor as u128) * (fee_bps as u128);
    // Ceiling division without `div_ceil` (stable only since 1.73; MSRV is 1.70).
    let fee = ((product + 9_999) / 10_000) as u64;
    let net = total_minor
        .checked_sub(fee)
        .ok_or_else(|| ExchangeError::AmountOverflow("fee exceeds total".into()))?;
    Ok((fee, net))
}

impl SettlementBackend for MockClearing {
    fn backend_id(&self) -> &str {
        "mock"
    }

    fn protocol_fee_bps(&self) -> u32 {
        self.fee_bps
    }

    fn submit_receipts(&self, receipts: &[UsageReceipt]) -> ExchangeResult<SettlementReport> {
        let mut report = SettlementReport::default();
        let mut accounts = self
            .accounts
            .lock()
            .map_err(|_| ExchangeError::Settlement("accounts lock poisoned".into()))?;
        let mut fees = self
            .fees
            .lock()
            .map_err(|_| ExchangeError::Settlement("fees lock poisoned".into()))?;
        let mut settled_ids = self
            .settled_ids
            .lock()
            .map_err(|_| ExchangeError::Settlement("settled lock poisoned".into()))?;

        for receipt in receipts {
            if settled_ids.contains(&receipt.receipt_id) {
                // Idempotent: re-submission of a settled receipt is a no-op success.
                report.settled.push(receipt.receipt_id.clone());
                continue;
            }
            if receipt.consumer_signature.is_empty() {
                report.rejected.push((
                    receipt.receipt_id.clone(),
                    "missing consumer signature".into(),
                ));
                continue;
            }
            if receipt.total_minor == 0 {
                report
                    .rejected
                    .push((receipt.receipt_id.clone(), "zero-value receipt".into()));
                continue;
            }
            let (fee, net) = match split_fee(receipt.total_minor, self.fee_bps) {
                Ok(split) => split,
                Err(e) => {
                    report
                        .rejected
                        .push((receipt.receipt_id.clone(), e.to_string()));
                    continue;
                }
            };
            *accounts
                .entry((receipt.provider_id.clone(), receipt.currency.clone()))
                .or_default() += net;
            *fees.entry(receipt.currency.clone()).or_default() += fee;
            *report
                .fees_minor
                .entry(receipt.currency.clone())
                .or_default() += fee;
            *report
                .net_to_providers_minor
                .entry(receipt.currency.clone())
                .or_default() += net;
            settled_ids.insert(receipt.receipt_id.clone());
            report.settled.push(receipt.receipt_id.clone());
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attestation::test_signer::DalekSigner;
    use crate::types::{BillingUnit, Price, Terms};

    fn receipt(id: &str, total_units: u64) -> UsageReceipt {
        let terms = Terms {
            price: Some(Price { amount_minor: 100 }),
            unit: BillingUnit::PerCall,
            currency: "USD".into(),
            max_payload_kb: 64,
        };
        UsageReceipt::issue(
            id.into(),
            format!("req-{id}"),
            "svc".into(),
            "1.0".into(),
            &terms,
            total_units,
            "alice".into(),
            "bob".into(),
            1,
            &DalekSigner::new(1),
        )
        .unwrap()
    }

    #[test]
    fn split_fee_rounds_up() {
        // 2.5% of 99 = 2.475 → fee 3, net 96.
        assert_eq!(split_fee(99, 250).unwrap(), (3, 96));
        assert_eq!(split_fee(0, 250).unwrap(), (0, 0));
        assert_eq!(split_fee(100, 0).unwrap(), (0, 100));
        // Cap at 100%.
        assert_eq!(split_fee(100, 20_000).unwrap(), (100, 0));
    }

    #[test]
    fn mock_clearing_settles_and_applies_fee() {
        let backend = MockClearing::new(250); // 2.5%
        let report = backend.submit_receipts(&[receipt("r1", 4)]).unwrap();
        assert_eq!(report.settled, vec!["r1".to_string()]);
        assert!(report.rejected.is_empty());
        // 400 total, 2.5% = 10 fee, 390 net.
        assert_eq!(backend.collected_fees("USD"), 10);
        assert_eq!(backend.account_balance("bob", "USD"), 390);
    }

    #[test]
    fn mock_clearing_is_idempotent() {
        let backend = MockClearing::new(100);
        let r = receipt("r1", 1);
        backend.submit_receipts(std::slice::from_ref(&r)).unwrap();
        backend.submit_receipts(std::slice::from_ref(&r)).unwrap();
        // Settled once, credited once.
        assert_eq!(backend.account_balance("bob", "USD"), 99);
    }

    #[test]
    fn unsigned_receipt_rejected_individually() {
        let backend = MockClearing::new(100);
        let mut bad = receipt("r-bad", 1);
        bad.consumer_signature = String::new();
        let good = receipt("r-good", 1);
        let report = backend.submit_receipts(&[bad, good]).unwrap();
        assert_eq!(report.settled, vec!["r-good".to_string()]);
        assert_eq!(report.rejected.len(), 1);
    }
}
