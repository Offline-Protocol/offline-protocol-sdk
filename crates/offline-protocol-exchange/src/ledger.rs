//! Prepaid balance ledger.
//!
//! The v1 settlement model bounds non-payment risk to a prepaid amount: a
//! consumer funds a balance out-of-band (clearing backend / mock), priced
//! invocations place a **hold** for the worst-case charge before the request
//! is sent, and the hold is **committed** (debited) for the actual charge when
//! the receipt is issued — any remainder returns to the available balance. A
//! failed or expired invocation **releases** the hold in full.
//!
//! Amounts are integer minor units per currency. The ledger is pure state;
//! persistence is a serde snapshot the host stores wherever it likes.

use crate::error::{ExchangeError, ExchangeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Balance for one currency.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    /// Spendable minor units.
    pub available_minor: u64,
    /// Minor units held against in-flight invocations.
    pub held_minor: u64,
}

/// An in-flight hold against the balance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Hold {
    currency: String,
    amount_minor: u64,
}

/// Prepaid balances with two-phase holds, keyed by currency.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrepaidLedger {
    balances: HashMap<String, Balance>,
    /// Active holds keyed by invocation request id.
    holds: HashMap<String, Hold>,
}

impl PrepaidLedger {
    /// Creates an empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Credits the available balance (funding confirmed by the clearing backend).
    pub fn credit(&mut self, currency: &str, amount_minor: u64) -> ExchangeResult<Balance> {
        let balance = self.balances.entry(currency.to_string()).or_default();
        balance.available_minor = balance
            .available_minor
            .checked_add(amount_minor)
            .ok_or_else(|| ExchangeError::AmountOverflow("credit overflows balance".into()))?;
        Ok(balance.clone())
    }

    /// Current balance for a currency (zero when never funded).
    pub fn balance(&self, currency: &str) -> Balance {
        self.balances.get(currency).cloned().unwrap_or_default()
    }

    /// All non-zero balances.
    pub fn balances(&self) -> &HashMap<String, Balance> {
        &self.balances
    }

    /// Places a hold for the worst-case charge of an invocation.
    pub fn hold(
        &mut self,
        request_id: &str,
        currency: &str,
        amount_minor: u64,
    ) -> ExchangeResult<()> {
        if self.holds.contains_key(request_id) {
            return Err(ExchangeError::InvalidReceipt(format!(
                "hold already exists for request {request_id}"
            )));
        }
        let balance = self.balances.entry(currency.to_string()).or_default();
        if balance.available_minor < amount_minor {
            return Err(ExchangeError::InsufficientBalance {
                currency: currency.to_string(),
                needed: amount_minor,
                available: balance.available_minor,
            });
        }
        balance.available_minor -= amount_minor;
        balance.held_minor = balance
            .held_minor
            .checked_add(amount_minor)
            .ok_or_else(|| ExchangeError::AmountOverflow("hold overflows held".into()))?;
        self.holds.insert(
            request_id.to_string(),
            Hold {
                currency: currency.to_string(),
                amount_minor,
            },
        );
        Ok(())
    }

    /// Whether a hold exists for the request.
    pub fn has_hold(&self, request_id: &str) -> bool {
        self.holds.contains_key(request_id)
    }

    /// Commits a hold for the actual charge; the remainder returns to the
    /// available balance. `actual_minor` must not exceed the held amount.
    pub fn commit(&mut self, request_id: &str, actual_minor: u64) -> ExchangeResult<()> {
        let hold = self
            .holds
            .remove(request_id)
            .ok_or_else(|| ExchangeError::UnknownInvocation(request_id.to_string()))?;
        if actual_minor > hold.amount_minor {
            // Never charge beyond the hold — restore it and refuse.
            self.holds.insert(request_id.to_string(), hold);
            return Err(ExchangeError::InvalidReceipt(format!(
                "actual charge {actual_minor} exceeds held amount"
            )));
        }
        let balance = self.balances.entry(hold.currency.clone()).or_default();
        balance.held_minor = balance.held_minor.saturating_sub(hold.amount_minor);
        balance.available_minor = balance
            .available_minor
            .checked_add(hold.amount_minor - actual_minor)
            .ok_or_else(|| ExchangeError::AmountOverflow("commit remainder overflows".into()))?;
        Ok(())
    }

    /// Re-keys a hold from a reservation id to the final request id, once the
    /// invocation has been sent and its correlation id is known.
    pub fn rebind_hold(&mut self, old_id: &str, new_id: &str) -> ExchangeResult<()> {
        if self.holds.contains_key(new_id) {
            return Err(ExchangeError::InvalidReceipt(format!(
                "hold already exists for request {new_id}"
            )));
        }
        let hold = self
            .holds
            .remove(old_id)
            .ok_or_else(|| ExchangeError::UnknownInvocation(old_id.to_string()))?;
        self.holds.insert(new_id.to_string(), hold);
        Ok(())
    }

    /// Releases a hold in full (invocation failed or expired).
    pub fn release(&mut self, request_id: &str) -> ExchangeResult<()> {
        let hold = self
            .holds
            .remove(request_id)
            .ok_or_else(|| ExchangeError::UnknownInvocation(request_id.to_string()))?;
        let balance = self.balances.entry(hold.currency.clone()).or_default();
        balance.held_minor = balance.held_minor.saturating_sub(hold.amount_minor);
        balance.available_minor = balance
            .available_minor
            .checked_add(hold.amount_minor)
            .ok_or_else(|| ExchangeError::AmountOverflow("release overflows balance".into()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_and_balance() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 500).unwrap();
        ledger.credit("USD", 250).unwrap();
        assert_eq!(ledger.balance("USD").available_minor, 750);
        assert_eq!(ledger.balance("USD").held_minor, 0);
        assert_eq!(ledger.balance("EUR").available_minor, 0);
    }

    #[test]
    fn hold_commit_partial_returns_remainder() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 100).unwrap();
        ledger.hold("req-1", "USD", 60).unwrap();
        assert_eq!(ledger.balance("USD").available_minor, 40);
        assert_eq!(ledger.balance("USD").held_minor, 60);

        ledger.commit("req-1", 25).unwrap();
        assert_eq!(ledger.balance("USD").available_minor, 75);
        assert_eq!(ledger.balance("USD").held_minor, 0);
    }

    #[test]
    fn hold_release_restores_full_amount() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 100).unwrap();
        ledger.hold("req-1", "USD", 60).unwrap();
        ledger.release("req-1").unwrap();
        assert_eq!(ledger.balance("USD").available_minor, 100);
        assert_eq!(ledger.balance("USD").held_minor, 0);
    }

    #[test]
    fn insufficient_balance_refused() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 10).unwrap();
        let err = ledger.hold("req-1", "USD", 11).unwrap_err();
        assert!(matches!(err, ExchangeError::InsufficientBalance { .. }));
        // Balance untouched after refusal.
        assert_eq!(ledger.balance("USD").available_minor, 10);
    }

    #[test]
    fn duplicate_hold_refused() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 100).unwrap();
        ledger.hold("req-1", "USD", 10).unwrap();
        assert!(ledger.hold("req-1", "USD", 10).is_err());
    }

    #[test]
    fn commit_above_hold_refused_and_hold_survives() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 100).unwrap();
        ledger.hold("req-1", "USD", 30).unwrap();
        assert!(ledger.commit("req-1", 31).is_err());
        // Hold must still be intact for a correct retry.
        assert!(ledger.has_hold("req-1"));
        ledger.commit("req-1", 30).unwrap();
        assert_eq!(ledger.balance("USD").available_minor, 70);
    }

    #[test]
    fn commit_unknown_request_errors() {
        let mut ledger = PrepaidLedger::new();
        assert!(matches!(
            ledger.commit("nope", 1),
            Err(ExchangeError::UnknownInvocation(_))
        ));
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut ledger = PrepaidLedger::new();
        ledger.credit("USD", 100).unwrap();
        ledger.hold("req-1", "USD", 30).unwrap();
        let json = serde_json::to_string(&ledger).unwrap();
        let mut restored: PrepaidLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.balance("USD").held_minor, 30);
        restored.release("req-1").unwrap();
        assert_eq!(restored.balance("USD").available_minor, 100);
    }
}
