//! # Ledger
//!
//! The ledger is the source of truth. A balance is a projection of it, not a
//! column that happens to agree with it, and a correction is a new compensating
//! transaction rather than an edit (Q03).
//!
//! ## Double entry
//!
//! Every transaction has at least two legs and its legs sum to exactly zero.
//! That is not a convention the code follows politely; it is checked in
//! [`LedgerTransaction::new`], so a transaction that does not balance cannot be
//! constructed. A single-currency platform has no need for the multi-currency
//! per-currency balancing that a general ledger needs, and pretending to
//! support it here would mean a zero-sum check that could be satisfied within
//! one currency while the whole was unbalanced across two.
//!
//! ## Sign convention
//!
//! A credit is a negative amount and a debit is a positive one, from the
//! perspective of the customer's balance: depositing money debits the customer
//! account and credits the platform liability account. Deriving [`EntryType`]
//! from the sign of the amount, rather than storing both, means the two can
//! never disagree — there is no state in which a `Credit` holds a positive
//! amount.
//!
//! ## Why legs are `i64` and not `MoneyAmount`
//!
//! A leg is a signed count of minor units in a currency the whole transaction
//! shares. Wrapping each leg in its own `MoneyAmount` would repeat the currency
//! on every row of a transaction that by construction has one currency, and
//! would permit the pathological case of a transaction whose legs disagree
//! about what they are denominated in.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{AccountId, IdempotencyKey, TransactionRef};
use crate::money::Currency;

/// Whether a leg increases or decreases the account it belongs to.
///
/// Derived from the sign of the amount, never stored alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    /// Money leaving the account.
    Debit,
    /// Money arriving in the account.
    Credit,
}

impl EntryType {
    /// The type a signed amount implies.
    ///
    /// Zero is [`EntryType::Debit`], which is arbitrary but harmless: a
    /// transaction is rejected for having a zero leg before this is reached.
    #[must_use]
    pub const fn of(minor_units: i64) -> Self {
        if minor_units < 0 {
            Self::Credit
        } else {
            Self::Debit
        }
    }

    /// The sign this type implies.
    #[must_use]
    pub const fn sign(self) -> i64 {
        match self {
            Self::Debit => 1,
            Self::Credit => -1,
        }
    }
}

impl fmt::Display for EntryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Debit => f.write_str("DEBIT"),
            Self::Credit => f.write_str("CREDIT"),
        }
    }
}

/// Why a transaction was written. Recorded on every leg so that a balance can
/// be explained without replaying history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryReason {
    /// Funds moved into the platform.
    Deposit,
    /// Funds moved out of the platform.
    Withdrawal,
    /// A stake was taken from a customer and is now held.
    BetStake,
    /// A held stake was released without a match.
    BetStakeReleased,
    /// A matched order's holding was converted into a payout.
    SettlementPayout,
    /// Commission was taken out of a winner's net winnings.
    ///
    /// Distinct from the payout because it is the platform's revenue and not a
    /// return of the customer's own money, and a balance explained from its
    /// entries has to be able to tell the two apart.
    Commission,
    /// A market's residual holding was swept to platform revenue.
    ///
    /// The losers' stakes, which is money that was taken in and is now booked as
    /// the platform's. Written explicitly rather than left as a residual so that
    /// the holding account provably reaches zero.
    ResidualSweep,
    /// A payout was written back to recover an overpayment.
    SettlementRecovery,
    /// An entry that reverses an earlier transaction.
    Compensation,
}

impl fmt::Display for EntryReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Deposit => "DEPOSIT",
            Self::Withdrawal => "WITHDRAWAL",
            Self::BetStake => "BET_STAKE",
            Self::BetStakeReleased => "BET_STAKE_RELEASED",
            Self::SettlementPayout => "SETTLEMENT_PAYOUT",
            Self::Commission => "COMMISSION",
            Self::ResidualSweep => "RESIDUAL_SWEEP",
            Self::SettlementRecovery => "SETTLEMENT_RECOVERY",
            Self::Compensation => "COMPENSATION",
        };
        f.write_str(text)
    }
}

/// One side of a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerLeg {
    /// The account this leg belongs to.
    pub account_id: AccountId,
    /// The signed amount in minor units. Negative credits the account.
    pub amount_minor_units: i64,
}

impl LedgerLeg {
    /// Create a leg.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidInput`] if the amount is zero. A zero leg
    /// moves nothing, so it is a mistake rather than a rounding artefact — and
    /// the rounding in [`crate::odds::Odds::payout`] is deliberate about never
    /// producing one.
    pub fn new(account_id: AccountId, amount_minor_units: i64) -> Result<Self, DomainError> {
        if amount_minor_units == 0 {
            return Err(DomainError::InvalidInput {
                field: "amount_minor_units",
                reason: "a ledger leg must not be zero".to_string(),
            });
        }
        Ok(Self {
            account_id,
            amount_minor_units,
        })
    }

    /// Whether this leg credits the account.
    #[must_use]
    pub fn entry_type(&self) -> EntryType {
        EntryType::of(self.amount_minor_units)
    }

    /// The unsigned magnitude, for display.
    #[must_use]
    pub fn magnitude_minor_units(&self) -> i64 {
        self.amount_minor_units.abs()
    }
}

/// A balanced set of ledger legs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerTransaction {
    /// Groups the legs of one atomic business operation.
    pub transaction_ref: TransactionRef,
    /// The currency every leg is denominated in.
    pub currency: Currency,
    /// Why the transaction was written.
    pub reason: EntryReason,
    /// The request that caused it, for safe replay.
    pub idempotency_key: Option<IdempotencyKey>,
    /// The legs, in posting order.
    pub legs: Vec<LedgerLeg>,
}

impl LedgerTransaction {
    /// Build a transaction, enforcing the invariants.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidInput`] if there are fewer than two legs,
    /// or if the legs do not sum to zero, or if two legs name the same account
    /// and offset each other. The last case is rejected because a net-zero pair
    /// on one account is indistinguishable from a mistake in the amounts while
    /// still satisfying the sum check.
    pub fn new(
        transaction_ref: TransactionRef,
        currency: Currency,
        reason: EntryReason,
        idempotency_key: Option<IdempotencyKey>,
        legs: Vec<LedgerLeg>,
    ) -> Result<Self, DomainError> {
        if legs.len() < 2 {
            return Err(DomainError::InvalidInput {
                field: "legs",
                reason: "a ledger transaction needs at least two legs".to_string(),
            });
        }

        let mut total: i64 = 0;
        for leg in &legs {
            total = total.checked_add(leg.amount_minor_units).ok_or(
                DomainError::InvalidInput {
                    field: "legs",
                    reason: "ledger amounts overflow i64".to_string(),
                },
            )?;
        }

        if total != 0 {
            return Err(DomainError::InvalidInput {
                field: "legs",
                reason: format!("ledger legs must sum to zero, but they sum to {total}"),
            });
        }

        // A pair of legs on the same account that cancel is a bookkeeping
        // accident, not a movement. Catching it here means the sum-to-zero
        // check cannot be satisfied without any money actually moving.
        for (i, a) in legs.iter().enumerate() {
            for b in &legs[i + 1..] {
                if a.account_id == b.account_id && a.amount_minor_units == -b.amount_minor_units {
                    return Err(DomainError::InvalidInput {
                        field: "legs",
                        reason: format!(
                            "legs on account {} cancel each other out",
                            a.account_id.as_str()
                        ),
                    });
                }
            }
        }

        Ok(Self {
            transaction_ref,
            currency,
            reason,
            idempotency_key,
            legs,
        })
    }

    /// The sum of every leg, which is always zero for a valid transaction.
    #[must_use]
    pub fn total_minor_units(&self) -> i64 {
        self.legs
            .iter()
            .fold(0_i64, |acc, leg| acc.saturating_add(leg.amount_minor_units))
    }

    /// The net movement on one account within this transaction.
    #[must_use]
    pub fn balance_of(&self, account_id: &AccountId) -> i64 {
        self.legs
            .iter()
            .filter(|leg| &leg.account_id == account_id)
            .fold(0_i64, |acc, leg| acc.saturating_add(leg.amount_minor_units))
    }

    /// The accounts this transaction touches.
    #[must_use]
    pub fn accounts(&self) -> Vec<AccountId> {
        let mut seen: Vec<AccountId> = Vec::new();
        for leg in &self.legs {
            if !seen.contains(&leg.account_id) {
                seen.push(leg.account_id.clone());
            }
        }
        seen
    }
}

impl fmt::Display for LedgerTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "txn {} {} with {} legs",
            self.transaction_ref.as_str(),
            self.reason,
            self.legs.len()
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn account(name: &str) -> AccountId {
        AccountId::new(name).expect("valid")
    }

    fn txn(legs: Vec<LedgerLeg>) -> Result<LedgerTransaction, DomainError> {
        LedgerTransaction::new(
            TransactionRef::new("t1").expect("valid"),
            Currency::GBP,
            EntryReason::Deposit,
            None,
            legs,
        )
    }

    #[test]
    fn a_balanced_pair_is_accepted() {
        let t = txn(vec![
            LedgerLeg::new(account("customer"), 1000).expect("non-zero"),
            LedgerLeg::new(account("platform"), -1000).expect("non-zero"),
        ])
        .expect("balanced");
        assert_eq!(t.total_minor_units(), 0);
        assert_eq!(t.accounts().len(), 2);
    }

    #[test]
    fn an_unbalanced_transaction_is_refused() {
        let result = txn(vec![
            LedgerLeg::new(account("customer"), 1000).expect("non-zero"),
            LedgerLeg::new(account("platform"), -900).expect("non-zero"),
        ]);
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn a_single_leg_is_refused() {
        let result = txn(vec![LedgerLeg::new(account("customer"), 1000).expect("non-zero")]);
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn a_zero_leg_is_refused() {
        assert!(LedgerLeg::new(account("customer"), 0).is_err());
    }

    #[test]
    fn a_three_way_balance_is_accepted() {
        let t = txn(vec![
            LedgerLeg::new(account("a"), 1000).expect("non-zero"),
            LedgerLeg::new(account("b"), -600).expect("non-zero"),
            LedgerLeg::new(account("c"), -400).expect("non-zero"),
        ])
        .expect("balanced");
        assert_eq!(t.balance_of(&account("a")), 1000);
        assert_eq!(t.balance_of(&account("b")), -600);
        assert_eq!(t.balance_of(&account("c")), -400);
    }

    #[test]
    fn two_legs_on_one_account_cancelling_are_refused() {
        // Sums to zero, so the sum check alone would pass it.
        let result = txn(vec![
            LedgerLeg::new(account("a"), 1000).expect("non-zero"),
            LedgerLeg::new(account("a"), -1000).expect("non-zero"),
        ]);
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn two_legs_on_one_account_that_do_not_cancel_are_refused_by_the_sum() {
        let result = txn(vec![
            LedgerLeg::new(account("a"), 1000).expect("non-zero"),
            LedgerLeg::new(account("a"), 500).expect("non-zero"),
            LedgerLeg::new(account("b"), -1500).expect("non-zero"),
        ]);
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn overflow_in_the_sum_is_an_error_not_a_wrap() {
        let result = txn(vec![
            LedgerLeg::new(account("a"), i64::MAX).expect("non-zero"),
            LedgerLeg::new(account("b"), i64::MIN).expect("non-zero"),
            LedgerLeg::new(account("c"), 1).expect("non-zero"),
        ]);
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn entry_type_follows_the_sign_of_the_amount() {
        let debit = LedgerLeg::new(account("a"), 100).expect("non-zero");
        let credit = LedgerLeg::new(account("a"), -100).expect("non-zero");
        assert_eq!(debit.entry_type(), EntryType::Debit);
        assert_eq!(credit.entry_type(), EntryType::Credit);
        assert_eq!(EntryType::Debit.sign(), 1);
        assert_eq!(EntryType::Credit.sign(), -1);
        assert_eq!(debit.magnitude_minor_units(), credit.magnitude_minor_units());
    }

    #[test]
    fn accounts_are_reported_once_each() {
        let t = txn(vec![
            LedgerLeg::new(account("a"), 1000).expect("non-zero"),
            LedgerLeg::new(account("a"), -400).expect("non-zero"),
            LedgerLeg::new(account("b"), -600).expect("non-zero"),
        ])
        .expect("balanced");
        assert_eq!(t.accounts(), vec![account("a"), account("b")]);
        assert_eq!(t.balance_of(&account("a")), 600);
    }

    #[test]
    fn the_balance_of_an_absent_account_is_zero() {
        let t = txn(vec![
            LedgerLeg::new(account("a"), 1000).expect("non-zero"),
            LedgerLeg::new(account("b"), -1000).expect("non-zero"),
        ])
        .expect("balanced");
        assert_eq!(t.balance_of(&account("nobody")), 0);
    }
}
