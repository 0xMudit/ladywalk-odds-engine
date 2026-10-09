//! # Reconciliation
//!
//! Proving the ledger, the balance projection, the audit chain and the market
//! lifecycle still agree with each other.
//!
//! ## Why this is a use case and not a test
//!
//! A test proves the system was correct when it was written. Reconciliation
//! proves it is correct *now*, on a database that has been running for eleven
//! weeks, and the failure it exists to catch is one that no test can: a trigger
//! that stopped firing, a constraint that was dropped and recreated without its
//! check, a migration that ran against a table with rows in it. Those are all
//! facts about a running deployment rather than about the source, so the check
//! has to run against a running deployment.
//!
//! It is a use case rather than a `psql` script for the same reason the
//! reconciliation in [`crate::usecases::settlement`] is not: a script is not
//! something that can be called from a health endpoint, run on a schedule by the
//! binary, or given a severity when it fails.
//!
//! ## What counts as a violation
//!
//! Not every negative balance is a bug. A matched customer's stake is debited at
//! match time, so a customer account is *expected* to be negative while a
//! market runs, and reporting that as a violation would page someone every time
//! a bet is placed. What is a bug is a customer account going negative *when the
//! order that took the money was refused* — and that is enforced by a
//! constraint, not detected here.
//!
//! [`is_permitted_to_be_negative`] is therefore a classification, and it is
//! hardcoded for the same reason `domain::settlement::PLATFORM_REVENUE` is: a
//! deployment that could disagree with the reconciliation job about which
//! accounts may go negative is a deployment that will eventually disagree with
//! itself. Note that it does *not* permit `hold:<market>` to be negative. A hold
//! is fed by debits from customers and drained by payouts; a negative hold means
//! a payout drew on money that was never there, which is the single most
//! expensive thing this report can find.

use std::collections::BTreeSet;

use domain::ids::{AccountId, MarketId};

use crate::errors::AppError;
use crate::ports::{BalanceDiscrepancy, MarketState, is_permitted_to_be_negative};

/// One thing that disagrees with itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// The maintained balance projection and the ledger disagree about an
    /// account's balance.
    ///
    /// The most serious kind, because a balance that is wrong in a database that
    /// is otherwise healthy is a balance that is wrong for every customer and
    /// nothing else looks wrong.
    BalanceDisagreement {
        /// The account in dispute.
        account_id: AccountId,
        /// What the projection holds.
        projected_minor_units: i64,
        /// What the ledger's legs sum to.
        ledger_minor_units: i64,
    },
    /// An account is negative and is not one that may be.
    IllegalNegativeBalance {
        /// The account that should not be negative.
        account_id: AccountId,
        /// How far below zero it is, as a positive number.
        shortfall_minor_units: i64,
    },
    /// The hash-chained audit trail does not verify.
    ///
    /// Reported without the position of the break, because a report that named
    /// it would be a report that an operator could truncate the trail to before.
    /// The chain is only evidence if finding the break is not the same as
    /// choosing to look at it.
    AuditChainBroken,
    /// A market has resolved and not settled, so customers are owed money that
    /// has not moved.
    UnsettledMarket {
        /// The market that is waiting.
        market_id: MarketId,
    },
}

/// What reconciliation found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconciliationReport {
    /// Everything wrong, in the order the checks ran.
    pub violations: Vec<Violation>,
    /// How many accounts were compared.
    pub accounts_compared: usize,
    /// How many audit entries were verified.
    pub audit_entries_verified: usize,
    /// How many resolved markets were checked for settlement.
    pub markets_checked: usize,
}

impl ReconciliationReport {
    /// Whether everything reconciled.
    ///
    /// A boolean rather than a number because the caller of a health check can
    /// only act on pass or fail; a "severity" that nobody has agreed how to
    /// weight is a field that will be ignored.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.violations.is_empty()
    }

    /// A one-line summary for a log or a metric label.
    ///
    /// Healthy first, because that is the line an operator scans a dashboard
    /// for. Unhealthy reports name the *kinds* of violation and not the affected
    /// accounts: this string is a metric label, and a label that grows with the
    /// number of broken accounts is a label with unbounded cardinality.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.is_healthy() {
            return format!(
                "reconciled {} accounts, {} audit entries, {} markets",
                self.accounts_compared, self.audit_entries_verified, self.markets_checked
            );
        }
        let mut kinds: BTreeSet<&str> = BTreeSet::new();
        for violation in &self.violations {
            kinds.insert(match violation {
                Violation::BalanceDisagreement { .. } => "balance_disagreement",
                Violation::IllegalNegativeBalance { .. } => "illegal_negative_balance",
                Violation::AuditChainBroken => "audit_chain_broken",
                Violation::UnsettledMarket { .. } => "unsettled_market",
            });
        }
        format!(
            "{} violations across {} ({} accounts compared, {} audit entries, {} markets)",
            self.violations.len(),
            kinds.into_iter().collect::<Vec<_>>().join(", "),
            self.accounts_compared,
            self.audit_entries_verified,
            self.markets_checked
        )
    }
}

/// Check the four things that are supposed to agree, and report what does not.
///
/// Read-only, and rolled back rather than committed: nothing here writes, and a
/// reconciliation that held a transaction open while it ran would be blocking
/// the writes it is meant to be watching.
///
/// # Errors
///
/// Returns [`AppError::Store`] if any of the four checks cannot be run. A check
/// that could not be performed is *not* reported as a pass, because the whole
/// value of this job is that a green result means something was actually
/// verified.
pub async fn run(deps: &crate::usecases::Deps) -> Result<ReconciliationReport, AppError> {
    let mut report = ReconciliationReport::default();
    let mut store = deps.store.begin().await?;

    // ── The projection against the ledger ─────────────────────────────────
    // The full-table comparison, not a spot check. The interesting failure is
    // not "one account is wrong" but "the trigger stopped firing", and that is
    // only visible when every row is compared.
    //
    // The scan size comes from its own query rather than from the length of the
    // result. `reconcile_balances` returns only the rows that disagree, so a
    // system where every balance reconciles returns none — and counting those
    // would report that a green run compared no accounts at all, which is the
    // opposite of what "nothing disagreed" has to mean for this job to be worth
    // running.
    report.accounts_compared = usize::try_from(store.ledger().balance_accounts_compared().await?)
        .map_err(|_| {
            AppError::Domain(domain::error::DomainError::internal_with_context(
                "the ledger reported more accounts than can be counted",
                "the account count did not fit in usize",
            ))
        })?;

    for BalanceDiscrepancy {
        account_id,
        projected_minor_units,
        ledger_minor_units,
    } in store.ledger().reconcile_balances().await?
    {
        if projected_minor_units != ledger_minor_units {
            report.violations.push(Violation::BalanceDisagreement {
                account_id,
                projected_minor_units,
                ledger_minor_units,
            });
        }
    }

    // ── Accounts that may not be negative ────────────────────────────────
    for (account_id, balance) in store.ledger().negative_accounts().await? {
        if balance < 0 && !is_permitted_to_be_negative(&account_id) {
            report.violations.push(Violation::IllegalNegativeBalance {
                account_id,
                // Negated because a shortfall is a positive quantity. A customer
                // reading "shortfall: -400" has to do arithmetic to work out
                // that they are 400 short.
                shortfall_minor_units: balance.saturating_neg(),
            });
        }
    }

    // ── The audit chain ──────────────────────────────────────────────────
    // Read the chain to count what was verified as well as to confirm it, so
    // that a green report says how much was checked. "0 entries verified" is a
    // different thing from "no violations", and only the first is reassuring.
    let chain = store.audit().read_chain().await?;
    report.audit_entries_verified = chain.len();
    if !store.audit().verify_chain().await? {
        report.violations.push(Violation::AuditChainBroken);
    }

    // ── Markets owed a payout ────────────────────────────────────────────
    let resolved = store.markets().list_by_state(MarketState::Resolved).await?;
    report.markets_checked = resolved.len();
    for market in resolved {
        report.violations.push(Violation::UnsettledMarket {
            market_id: market.market_id,
        });
    }

    store.rollback().await?;
    Ok(report)
}
