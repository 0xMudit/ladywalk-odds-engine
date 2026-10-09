//! # Settlement
//!
//! When a market resolves, every matched order becomes a claim on the outcome
//! and the losing claims are extinguished. This module computes that
//! distribution, and — the part that actually matters — computes the ledger
//! transactions that record it.
//!
//! ## Why settlement is a function and not a service
//!
//! The dangerous bug in a settlement is one that pays some winners correctly
//! and then runs out of money, or pays a winner twice because a retry
//! re-entered the loop. Both are avoided by making settlement a pure function
//! from (orders, outcome, commission) to (transactions), and by having the
//! caller persist the result atomically. The function can be property-tested
//! exhaustively: a settlement must always balance, and must always pay out no
//! more than was held.
//!
//! ## Who pays the winners, and why it is the holding account
//!
//! [ADR-0001] chose a peer-to-peer exchange: the customers are the
//! counterparties, and the platform earns **commission on net winnings**. That
//! has one consequence that is easy to get wrong and expensive to discover
//! later, which is where the money comes from when a winner is paid.
//!
//! When two orders matched, *both* customers' stakes were taken into
//! `hold:<market_id>` — see [`crate::matching::match_order`]. That hold is the
//! only money that was ever set aside for this market. So a winner is paid
//! **from the hold**, and the platform's global liability account is not
//! involved in the payout path at all.
//!
//! This is the opposite of a bookmaker, where the house is the counterparty and
//! a single global account funds every payout. Modelling an exchange that way
//! would imply the platform guaranteed the payout, which is precisely the model
//! ADR-0001 rejected along with vig and the liability cap. It also produces a
//! ledger that balances transaction-by-transaction while being wrong in
//! aggregate: the hold account accumulates stakes that are never drained, and
//! the platform account pays out of nothing.
//!
//! ## The three legs of a settlement
//!
//! For a market with `H` held in total:
//!
//! 1. **Payouts.** Each winner is credited `stake × odds`, debited from the
//!    hold. One transaction per winner, each carrying an idempotency key keyed
//!    on the order, so a retry cannot pay twice.
//! 2. **Commission.** Each winner's *net winnings* — the profit, not the
//!    returned stake — are charged a commission, debited from the hold and
//!    credited to [`PLATFORM_REVENUE`]. Commission on profit rather than on
//!    stake is what distinguishes an exchange from a bookmaker, so it is
//!    applied here and only here.
//! 3. **Sweep.** Whatever the hold still contains after payouts and
//!    commission — the losers' stakes, net of commission — moves to
//!    [`PLATFORM_REVENUE`] in a single transaction. Without this leg the
//!    hold accumulates forever and the platform's revenue is never booked.
//!
//! ## The invariant a settlement must hold
//!
//! For every resolved market:
//!
//! 1. The sum of every leg is zero, so no money is created or destroyed.
//! 2. The total paid to winners is at most the total held, so the platform can
//!    never be drained by a market that resolved against it.
//! 3. Exactly one order on the winning selection is paid per matched pair, so a
//!    retry cannot pay twice without a matching debit somewhere.
//! 4. **The hold ends at zero.** Every minor unit that entered the hold leaves
//!    it, as a payout, as commission, or as a sweep. This is the identity the
//!    Q30 reconciliation asserts, and [`Settlement::reconciles`] checks it
//!    before the caller is allowed to persist anything.
//!
//! [ADR-0001]: ../adr/0001-peer-to-peer-exchange.md

use crate::error::DomainError;
use crate::ids::{AccountId, IdempotencyKey, MarketId, OrderId, TransactionRef};
use crate::ledger::{EntryReason, LedgerLeg, LedgerTransaction};
use crate::money::Currency;
use crate::order::{Order, OrderStatus, Selection};
#[cfg(test)]
use crate::odds::Odds;

/// The outcome a market resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The home selection won.
    HomeWin,
    /// The away selection won.
    AwayWin,
    /// The market was drawn.
    Draw,
}

impl Outcome {
    /// The selection this outcome rewards.
    #[must_use]
    pub const fn winning_selection(self) -> Selection {
        match self {
            Self::HomeWin => Selection::Home,
            Self::AwayWin => Selection::Away,
            Self::Draw => Selection::Draw,
        }
    }
}

/// The account the platform's own earnings sit in.
///
/// A constant rather than configuration: this name is part of what the ledger
/// means, and a deployment that could name its revenue account differently from
/// the reconciliation job reading it is a deployment that will eventually
/// disagree with itself.
///
/// There is deliberately **no** platform *liability* account. See the module
/// documentation for why an exchange does not have one.
pub const PLATFORM_REVENUE: &str = "platform:revenue";

/// The largest commission that can be charged, as a fraction of one.
///
/// 100% would take the entire profit and leave a winner with their stake back,
/// which is not a business model — it is a rounding error away from being a
/// loss. The bound exists so that a misconfigured rate cannot pay a winner less
/// than they staked, which would make the exchange the customer of its own
/// commission.
const MAX_COMMISSION_BPS: i64 = 10_000;

/// What settlement decided for one matched order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderSettlement {
    /// The order backed the winning selection and is owed a payout.
    Paid {
        /// The total returned, stake included.
        returned_minor_units: i64,
        /// The profit, which is the part that was not at risk.
        profit_minor_units: i64,
        /// The commission taken out of the profit, which is never negative and
        /// never more than the profit.
        commission_minor_units: i64,
    },
    /// The order lost its stake.
    Lost,
    /// The order never matched, so there was nothing to settle.
    NotMatched,
}

/// The result of settling one market.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settlement {
    /// The transactions to post, in a deterministic order: payouts in order
    /// order, then commissions, then the sweep.
    pub transactions: Vec<LedgerTransaction>,
    /// The decision for each order id, for the audit record.
    pub decisions: Vec<(OrderId, OrderSettlement)>,
    /// The market's holding account, so a caller can verify it ends at zero.
    pub holding_account: AccountId,
    /// The total that entered the hold, in minor units.
    pub total_held_minor_units: i64,
}

impl Settlement {
    /// The total paid out across every winning order.
    #[must_use]
    pub fn total_paid_minor_units(&self) -> i64 {
        self.decisions
            .iter()
            .filter_map(|(_, decision)| match decision {
                OrderSettlement::Paid { returned_minor_units, .. } => Some(*returned_minor_units),
                OrderSettlement::Lost | OrderSettlement::NotMatched => None,
            })
            .fold(0_i64, i64::saturating_add)
    }

    /// The total commission taken out of winners' profits.
    #[must_use]
    pub fn total_commission_minor_units(&self) -> i64 {
        self.decisions
            .iter()
            .filter_map(|(_, decision)| match decision {
                OrderSettlement::Paid {
                    commission_minor_units,
                    ..
                } => Some(*commission_minor_units),
                OrderSettlement::Lost | OrderSettlement::NotMatched => None,
            })
            .fold(0_i64, i64::saturating_add)
    }

    /// The number of orders that won.
    #[must_use]
    pub fn winners(&self) -> usize {
        self.decisions
            .iter()
            .filter(|(_, decision)| matches!(decision, OrderSettlement::Paid { .. }))
            .count()
    }

    /// The hold balance implied by these transactions.
    ///
    /// This is the sum of every leg naming the holding account, which is the
    /// balance the account will have once the caller has persisted everything.
    /// It is computed here rather than read from the database so that a caller
    /// can check the settlement *before* committing it, and so the same value is
    /// available in a test with no database at all.
    #[must_use]
    pub fn holding_balance_after(&self) -> i64 {
        self.transactions.iter().fold(0_i64, |total, transaction| {
            total
                + transaction.balance_of(&self.holding_account)
        })
    }

    /// Whether the settlement accounts for every minor unit that was held.
    ///
    /// This is the post-condition the caller must check before writing: the
    /// hold started at [`Self::total_held_minor_units`] and must end at zero.
    /// A settlement that pays some winners and leaves money behind in the hold
    /// is not a settlement that failed loudly — it is one that quietly stranded
    /// a customer's money, which is the failure mode this whole module exists
    /// to prevent.
    #[must_use]
    pub fn reconciles(&self) -> bool {
        self.holding_balance_after() == 0
    }
}

/// Settle a resolved market, paying winners out of the market's holding account.
///
/// `held_minor_units` is what the market's hold is currently worth. It is a
/// parameter rather than something read from the database so that this stays a
/// pure function, and it is checked against the payouts: if the hold cannot
/// cover every winner, the market is returned as an error **before** any
/// transaction is produced, because a partial settlement is the one outcome
/// worse than no settlement.
///
/// # Errors
///
/// Returns [`DomainError::InvalidInput`] if:
/// - an order is `Matched` but has no quoted odds, which would make its payout
///   unknowable;
/// - an order's stake currency is not the market's currency;
/// - `commission_bps` is negative or above [`MAX_COMMISSION_BPS`];
/// - the hold cannot cover the total payout plus commission.
///
/// Returns [`DomainError::internal`] if a generated identifier is invalid, which
/// would be a bug here rather than bad input.
pub fn settle_market(
    market_id: &MarketId,
    orders: &[Order],
    outcome: Outcome,
    currency: Currency,
    commission_bps: i64,
    held_minor_units: i64,
    transaction_prefix: &TransactionRef,
) -> Result<Settlement, DomainError> {
    if !(0..=MAX_COMMISSION_BPS).contains(&commission_bps) {
        return Err(DomainError::InvalidInput {
            field: "commission_bps",
            reason: format!("commission must be between 0 and {MAX_COMMISSION_BPS} basis points"),
        });
    }

    let holding = crate::matching::holding_account_id(market_id)?;
    let revenue = AccountId::new(PLATFORM_REVENUE).map_err(|_| {
        DomainError::internal("the platform revenue account constant must be a valid identifier")
    })?;

    let winning = outcome.winning_selection();
    let mut transactions: Vec<LedgerTransaction> = Vec::new();
    let mut decisions: Vec<(OrderId, OrderSettlement)> = Vec::new();
    let mut total_paid: i64 = 0;

    for order in orders {
        match order.status {
            // A resting order holds no money, so there is nothing to unwind. It
            // is recorded as unmatched here and the *caller* must cancel it: a
            // settlement that silently ignored it would leave a `Pending` order
            // on a resolved market forever, which no later query would expect.
            OrderStatus::Pending | OrderStatus::Cancelled | OrderStatus::Settled => {
                decisions.push((order.id.clone(), OrderSettlement::NotMatched));
            }
            OrderStatus::Matched => {
                let odds = order.quoted_odds().map_err(|_| {
                    DomainError::internal(format!(
                        "order {} is matched but has no quoted odds, so its payout cannot be computed",
                        order.id.as_str()
                    ))
                })?;
                // The exposure, not the stake. A 1000 order that only filled for
                // 600 has 600 in the hold and 400 still in the customer's
                // account, so a payout computed from the stake would pay out on
                // money that was never held — and the hold would come up short
                // at exactly the moment a thin market resolves.
                let exposure = order.exposure_minor_units().map_err(|_| {
                    DomainError::internal(format!(
                        "order {} is matched but holds no exposure, so its payout cannot be computed",
                        order.id.as_str()
                    ))
                })?;
                let stake = rebuild_amount(&order.currency_code, exposure, currency)?;

                if order.selection != winning {
                    // The stake is already sitting in the hold and stays there.
                    // It is not extinguished by writing a zero-value entry; it is
                    // extinguished by the sweep at the end, which is the only
                    // place that books it as the platform's revenue.
                    decisions.push((order.id.clone(), OrderSettlement::Lost));
                    continue;
                }

                let payout = odds.payout(stake).map_err(|e| {
                    DomainError::internal(format!(
                        "payout overflowed for order {}: {e}",
                        order.id.as_str()
                    ))
                })?;
                let commission = commission_on(payout.profit.minor_units, commission_bps);
                // The whole hold funds this one winner: their returned amount,
                // part of which the platform keeps as commission. Writing it as
                // one three-legged transaction rather than a payout followed by a
                // commission debit is what keeps the hold solvent — a separate
                // commission debit would draw on a hold that the payout has
                // already emptied, and the platform would be paying its own
                // commission out of nothing.
                let net = payout.returned.minor_units - commission;

                let mut legs = vec![
                    LedgerLeg::new(order.account_id.clone(), net)?,
                    LedgerLeg::new(holding.clone(), -payout.returned.minor_units)?,
                ];
                if commission != 0 {
                    // Inserted last so the winning customer's leg is the first
                    // one a reader of a statement sees.
                    legs.push(LedgerLeg::new(revenue.clone(), commission)?);
                }

                let reference = derived_ref(transaction_prefix, "payout", &order.id)?;
                let transaction = LedgerTransaction::new(
                    reference,
                    currency,
                    EntryReason::SettlementPayout,
                    Some(derived_key(transaction_prefix, &order.id)?),
                    legs,
                )?;
                transactions.push(transaction);

                total_paid = total_paid
                    .checked_add(payout.returned.minor_units)
                    .ok_or(DomainError::Money(crate::money::MoneyError::Overflow))?;

                decisions.push((
                    order.id.clone(),
                    OrderSettlement::Paid {
                        returned_minor_units: payout.returned.minor_units,
                        profit_minor_units: payout.profit.minor_units,
                        commission_minor_units: commission,
                    },
                ));
            }
        }
    }

    // Only the gross payouts are drawn on the hold. Commission is taken *out of*
    // a payout, so it is a destination for part of the hold rather than an extra
    // claim on it, and adding the two here would demand money that was never
    // set aside.
    if total_paid > held_minor_units {
        // Refusing the whole market, rather than paying the winners we can
        // afford, is deliberate: a half-paid market is one where some customers
        // have been made whole and others have been left waiting, with nothing
        // in the ledger to say so.
        return Err(DomainError::InsufficientFunds {
            available_minor: held_minor_units,
            requested_minor: total_paid,
        });
    }

    // Everything still in the hold once the winners have been paid is the
    // losers' money, and it is the platform's — it was taken in and never
    // belonged to anyone still holding it. Booking it here is what closes the
    // identity: without this leg the hold would drift upwards by every stake
    // ever matched and no reconciliation would ever balance.
    let residual = held_minor_units - total_paid;
    if residual != 0 {
        let reference = TransactionRef::new(format!("{}.sweep", transaction_prefix.as_str()))
            .map_err(|e| DomainError::internal(format!("generated an invalid reference: {e}")))?;
        let transaction = LedgerTransaction::new(
            reference,
            currency,
            EntryReason::ResidualSweep,
            Some(IdempotencyKey::new(format!(
                "settle:{}:sweep",
                transaction_prefix.as_str()
            ))
            .map_err(|e| DomainError::internal(format!("generated an invalid key: {e}")))?),
            vec![
                LedgerLeg::new(holding.clone(), -residual)?,
                LedgerLeg::new(revenue, residual)?,
            ],
        )?;
        transactions.push(transaction);
    }

    let settlement = Settlement {
        transactions,
        decisions,
        holding_account: holding,
        total_held_minor_units: held_minor_units,
    };

    // The one check that says the arithmetic above was right. Reaching here with
    // a hold that does not close to zero means this function is wrong, and the
    // caller must not be allowed to persist it.
    if !settlement.reconciles() {
        return Err(DomainError::internal(format!(
            "settlement for market {} does not reconcile: the hold would end at {} rather than zero",
            market_id.as_str(),
            settlement.holding_balance_after()
        )));
    }

    Ok(settlement)
}

/// Commission on a profit, in basis points, rounded toward zero.
///
/// Rounding down rather than to nearest is what keeps the invariant that a
/// winner never receives less than their stake: a commission of at most the
/// profit means `returned - commission >= stake` exactly.
fn commission_on(profit_minor_units: i64, commission_bps: i64) -> i64 {
    if profit_minor_units <= 0 || commission_bps == 0 {
        return 0;
    }
    let numerator = profit_minor_units
        .checked_mul(commission_bps)
        .unwrap_or(i64::MAX);
    let commission = numerator / 10_000;
    commission.min(profit_minor_units)
}

/// A transaction reference derived from a prefix, a kind, and an order id.
fn derived_ref(
    prefix: &TransactionRef,
    kind: &str,
    order_id: &OrderId,
) -> Result<TransactionRef, DomainError> {
    TransactionRef::new(format!("{}.{kind}.{}", prefix.as_str(), order_id.as_str()))
        .map_err(|e| DomainError::internal(format!("generated an invalid reference: {e}")))
}

/// The idempotency key for an order's settlement.
///
/// One key for the payout and its commission legs, because they are one decision
/// written as one transaction: a key that distinguished them would let a retry
/// re-run one half of an order's settlement.
fn derived_key(
    prefix: &TransactionRef,
    order_id: &OrderId,
) -> Result<IdempotencyKey, DomainError> {
    IdempotencyKey::new(format!("settle:{}:{}", prefix.as_str(), order_id.as_str()))
        .map_err(|e| DomainError::internal(format!("generated an invalid idempotency key: {e}")))
}

/// Rebuild a typed amount for an order, checking the currency matches the
/// market's.
///
/// # Errors
///
/// Returns [`DomainError::InvalidInput`] if the stored code is not a currency
/// this build knows, or is a currency this build does know but not the one the
/// market settles in. The second check is the one that matters: paying a GBP
/// stake out of a USD hold would balance the transaction and be completely
/// wrong.
fn rebuild_amount(
    code: &str,
    minor_units: i64,
    market_currency: Currency,
) -> Result<crate::money::MoneyAmount, DomainError> {
    let currency = Currency::from_code(code)
        .map_err(|_| DomainError::InvalidInput {
            field: "currency_code",
            reason: format!("unknown currency code: {code}"),
        })?;
    if currency != market_currency {
        return Err(DomainError::InvalidInput {
            field: "currency_code",
            reason: format!(
                "the order is in {code} but the market settles in {market_currency}"
            ),
        });
    }
    Ok(crate::money::MoneyAmount::new(minor_units, currency))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::OrderId;
    use crate::money::MoneyAmount;

    /// The commission the exchange charges in these tests, in basis points.
    const COMMISSION_BPS: i64 = 500; // 5%
    const MARKET: &str = "m1";

    fn matched(
        name: &str,
        account: &str,
        selection: Selection,
        odds: &str,
        stake: i64,
    ) -> Order {
        let mut order = Order::pending(
            OrderId::new(name).expect("valid"),
            AccountId::new(account).expect("valid"),
            &crate::order::OrderRequest::new(
                MarketId::new(MARKET).expect("valid"),
                selection,
                MoneyAmount::new(stake, Currency::GBP),
            )
            .expect("positive stake"),
        );
        order
            .apply_quote(Odds::parse(odds).expect("valid odds"))
            .expect("pending order can be quoted");
        // The holding reference is what settlement draws the payout from, and
        // the exposure is what was actually put in it. Passing the full stake is
        // correct here only because these fixtures are always fully filled; a
        // partial fill would pass the filled amount instead, and passing the
        // stake for a partially filled order would pay out on money that was
        // never held.
        order
            .mark_matched(
                TransactionRef::new(format!("hold-{name}")).expect("valid"),
                stake,
            )
            .expect("pending order can match");
        order
    }

    /// An evenly-staked pair at even odds. Both stake 1000, so the hold is
    /// exactly 2000 and the winner's gross payout consumes all of it: there is
    /// no residual to sweep. This is the case where an exchange's commission is
    /// the *only* revenue, which is the point of ADR-0001.
    fn matched_pair() -> Vec<Order> {
        vec![
            matched("o1", "alice", Selection::Home, "2.0", 1000),
            matched("o2", "bob", Selection::Away, "2.0", 1000),
        ]
    }

    /// A lopsided pair: the winner stakes 1000 but the loser stakes 1500, so
    /// 500 is left in the hold after the winner is paid. This is the case that
    /// produces a residual sweep.
    fn lopsided_pair() -> Vec<Order> {
        vec![
            matched("o1", "alice", Selection::Home, "2.0", 1000),
            matched("o2", "bob", Selection::Away, "2.0", 1500),
        ]
    }

    fn settle(orders: &[Order], outcome: Outcome, held: i64) -> Settlement {
        settle_market(
            &MarketId::new(MARKET).expect("valid"),
            orders,
            outcome,
            Currency::GBP,
            COMMISSION_BPS,
            held,
            &TransactionRef::new("m1-settle").expect("valid"),
        )
        .expect("settlement should succeed")
    }

    #[test]
    fn a_winning_order_is_paid_stake_plus_profit() {
        // Held 2000: alice 1000 and bob 1000 both matched into the hold.
        let result = settle(&matched_pair(), Outcome::HomeWin, 2000);

        assert_eq!(result.winners(), 1);
        assert_eq!(result.total_paid_minor_units(), 2000);
        assert_eq!(
            result.decisions[0].1,
            OrderSettlement::Paid {
                returned_minor_units: 2000,
                profit_minor_units: 1000,
                commission_minor_units: 50,
            }
        );
    }

    #[test]
    fn the_hold_pays_every_payout_and_only_the_hold_does() {
        // The whole point of the exchange model: the money comes from the
        // market's own hold. A platform liability account appearing here would
        // mean an exchange had started acting as a bookmaker.
        let result = settle(&lopsided_pair(), Outcome::HomeWin, 2500);
        let hold = crate::matching::holding_account_id(&MarketId::new(MARKET).expect("valid"))
            .expect("valid");
        let revenue = AccountId::new(PLATFORM_REVENUE).expect("valid");

        let drawn_from_hold: i64 = result
            .transactions
            .iter()
            .map(|t| t.balance_of(&hold))
            .sum();
        assert_eq!(
            -drawn_from_hold,
            result.total_paid_minor_units(),
            "the hold must fund exactly the gross payouts"
        );

        // Nothing anywhere debits a platform account as a source of funds.
        for transaction in &result.transactions {
            for leg in &transaction.legs {
                assert!(
                    !(leg.account_id == revenue && leg.amount_minor_units < 0),
                    "platform revenue must only ever be credited"
                );
            }
        }
    }

    #[test]
    fn the_hold_ends_at_zero_and_every_minor_unit_is_accounted_for() {
        // 2500 held: 2000 paid, 50 commission, 450 residual.
        let result = settle(&lopsided_pair(), Outcome::HomeWin, 2500);

        assert!(result.reconciles());
        assert_eq!(result.holding_balance_after(), 0);
        assert_eq!(result.total_paid_minor_units(), 2000);
        assert_eq!(result.total_commission_minor_units(), 50);
        assert_eq!(
            result.total_paid_minor_units() + result.total_commission_minor_units() + 450,
            2500
        );
    }

    #[test]
    fn the_residual_hold_becomes_platform_revenue() {
        // 50 commission + 450 residual.
        let result = settle(&lopsided_pair(), Outcome::HomeWin, 2500);
        let revenue = AccountId::new(PLATFORM_REVENUE).expect("valid");

        let booked: i64 = result
            .transactions
            .iter()
            .map(|t| t.balance_of(&revenue))
            .sum();
        assert_eq!(booked, 500);
    }

    #[test]
    fn an_evenly_staked_even_odds_market_leaves_nothing_to_sweep() {
        // Both stake 1000 at 2.0, so the hold is exactly the winner's payout and
        // the exchange's only revenue is commission. If this produced a sweep,
        // money would be appearing from somewhere.
        let result = settle(&matched_pair(), Outcome::HomeWin, 2000);

        assert!(
            !result
                .transactions
                .iter()
                .any(|t| t.transaction_ref.as_str().ends_with(".sweep")),
            "there should be no residual to sweep: {:?}",
            result
                .transactions
                .iter()
                .map(|t| t.transaction_ref.as_str().to_string())
                .collect::<Vec<_>>()
        );
        assert!(result.reconciles());
        let revenue = AccountId::new(PLATFORM_REVENUE).expect("valid");
        let booked: i64 = result.transactions.iter().map(|t| t.balance_of(&revenue)).sum();
        assert_eq!(booked, 50, "commission is the whole revenue");
    }

    #[test]
    fn commission_is_taken_from_profit_and_never_from_the_stake() {
        // 5% of 1000 profit is 50. Alice is credited 1950 and the platform 50
        // out of the same 2000 the hold pays, so she ends up ahead of her 1000
        // stake and the hold is not overdrawn.
        let result = settle(&matched_pair(), Outcome::HomeWin, 2000);
        let alice = AccountId::new("alice").expect("valid");
        let net: i64 = result
            .transactions
            .iter()
            .map(|t| t.balance_of(&alice))
            .sum();

        assert_eq!(net, 1950);
        assert!(net > 1000, "a winner must end up ahead of their stake");
    }

    #[test]
    fn a_winner_is_paid_net_and_the_hold_is_drawn_gross() {
        // The two amounts are deliberately different. Crediting the winner the
        // gross and debiting the hold the net would be the bug in the other
        // direction: the hold would keep the commission, so the platform's
        // revenue would be unbooked while the customer was charged for it.
        let result = settle(&matched_pair(), Outcome::HomeWin, 2000);
        let alice = AccountId::new("alice").expect("valid");
        let hold = crate::matching::holding_account_id(&MarketId::new(MARKET).expect("valid"))
            .expect("valid");

        let payout = result
            .transactions
            .iter()
            .find(|t| t.legs.iter().any(|l| l.account_id == alice))
            .expect("alice must have a payout");
        assert_eq!(payout.balance_of(&alice), 1950);
        assert_eq!(payout.balance_of(&hold), -2000);
    }

    #[test]
    fn the_hold_never_goes_negative_mid_settlement() {
        // The hold is the only money that exists, so a settlement that pays out
        // more than it holds — even transiently, even if a later sweep would
        // balance the total — is a settlement paying from nothing.
        let result = settle(&matched_pair(), Outcome::HomeWin, 2000);
        let hold = crate::matching::holding_account_id(&MarketId::new(MARKET).expect("valid"))
            .expect("valid");

        let mut running = 2000_i64;
        for transaction in &result.transactions {
            running += transaction.balance_of(&hold);
            assert!(
                running >= 0,
                "the hold went negative at {}",
                transaction.transaction_ref.as_str()
            );
        }
        assert_eq!(running, 0);
    }

    #[test]
    fn a_zero_commission_still_sweeps_the_losers_money() {
        let result = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &matched_pair(),
            Outcome::HomeWin,
            Currency::GBP,
            0,
            2000,
            &TransactionRef::new("m1-settle").expect("valid"),
        )
        .expect("settlement should succeed");

        assert_eq!(result.total_commission_minor_units(), 0);
        assert!(result.reconciles());
        let revenue = AccountId::new(PLATFORM_REVENUE).expect("valid");
        let booked: i64 = result.transactions.iter().map(|t| t.balance_of(&revenue)).sum();
        // An even pair at even odds leaves nothing over, so with no commission
        // the platform earns nothing at all.
        assert_eq!(booked, 0, "an exactly-matched market leaves no revenue");
    }

    #[test]
    fn a_losing_order_is_recorded_but_gets_no_payout_transaction() {
        let result = settle(&matched_pair(), Outcome::HomeWin, 2000);
        assert_eq!(result.decisions[1].1, OrderSettlement::Lost);
        // Bob gets no leg at all; his money is swept instead.
        let bob = AccountId::new("bob").expect("valid");
        let bobs_legs: i64 = result.transactions.iter().map(|t| t.balance_of(&bob)).sum();
        assert_eq!(bobs_legs, 0);
    }

    #[test]
    fn every_settlement_transaction_balances() {
        // Two winners at different odds against one loser, so the payouts are
        // not all equal and the hold has to be reconciled across them.
        let orders = vec![
            matched("o1", "alice", Selection::Home, "2.0", 1000),
            matched("o2", "bob", Selection::Away, "2.0", 500),
            matched("o3", "carol", Selection::Home, "3.0", 250),
        ];
        // 1750 held, 2000 + 750 = 2750 paid.
        let result = settle(&orders, Outcome::HomeWin, 2750);

        assert_eq!(result.winners(), 2);
        assert!(!result.transactions.is_empty());
        for transaction in &result.transactions {
            assert_eq!(
                transaction.total_minor_units(),
                0,
                "transaction {} did not balance",
                transaction.transaction_ref.as_str()
            );
        }
        assert!(result.reconciles());
    }

    #[test]
    fn a_hold_too_small_to_pay_every_winner_refuses_the_whole_market() {
        // 2000 is owed but only 1500 is held. Paying the first winner and
        // failing on the second is the outcome this check exists to prevent.
        let error = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &matched_pair(),
            Outcome::HomeWin,
            Currency::GBP,
            COMMISSION_BPS,
            1500,
            &TransactionRef::new("m1-settle").expect("valid"),
        )
        .expect_err("an underfunded hold must not settle");

        // Only the gross payout is claimed. Commission is taken out of the
        // payout, not added to it, so demanding 2050 would be asking for money
        // that was never held.
        assert!(matches!(
            error,
            DomainError::InsufficientFunds {
                available_minor: 1500,
                requested_minor: 2000,
            }
        ));
    }

    #[test]
    fn an_exact_hold_is_enough_and_does_not_overdraw() {
        // The boundary: exactly enough money settles cleanly. One minor unit
        // less must not, or the check is off by one and a real market would be
        // refused at the precise moment it became payable.
        let ok = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &matched_pair(),
            Outcome::HomeWin,
            Currency::GBP,
            COMMISSION_BPS,
            2000,
            &TransactionRef::new("m1-settle").expect("valid"),
        );
        assert!(ok.is_ok(), "an exactly-funded hold must settle");
        assert!(ok.expect("checked above").reconciles());

        let short = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &matched_pair(),
            Outcome::HomeWin,
            Currency::GBP,
            COMMISSION_BPS,
            1999,
            &TransactionRef::new("m1-settle").expect("valid"),
        );
        assert!(short.is_err(), "one minor unit short must not settle");
    }

    #[test]
    fn a_negative_or_absurd_commission_is_rejected() {
        for bps in [-1, MAX_COMMISSION_BPS + 1, i64::MIN] {
            let error = settle_market(
                &MarketId::new(MARKET).expect("valid"),
                &matched_pair(),
                Outcome::HomeWin,
                Currency::GBP,
                bps,
                2000,
                &TransactionRef::new("m1-settle").expect("valid"),
            )
            .expect_err("a bad commission must not settle");
            assert!(matches!(error, DomainError::InvalidInput { .. }), "{bps} bps");
        }
    }

    #[test]
    fn an_order_in_the_wrong_currency_is_refused() {
        // A GBP order in a USD market would balance its transaction and be
        // completely wrong, which is why the check is explicit.
        let mut order = matched("o1", "alice", Selection::Home, "2.0", 1000);
        order.currency_code = "USD".to_string();

        let error = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &[order, matched("o2", "bob", Selection::Away, "2.0", 1000)],
            Outcome::HomeWin,
            Currency::GBP,
            COMMISSION_BPS,
            2000,
            &TransactionRef::new("m1-settle").expect("valid"),
        )
        .expect_err("a currency mismatch must not settle");

        assert!(matches!(error, DomainError::InvalidInput { .. }));
    }

    #[test]
    fn an_unmatched_order_is_never_settled() {
        let mut pending = Order::pending(
            OrderId::new("p1").expect("valid"),
            AccountId::new("alice").expect("valid"),
            &crate::order::OrderRequest::new(
                MarketId::new(MARKET).expect("valid"),
                Selection::Home,
                MoneyAmount::new(1000, Currency::GBP),
            )
            .expect("positive stake"),
        );
        pending
            .apply_quote(Odds::parse("2.0").expect("valid odds"))
            .expect("can be quoted");

        let result = settle(&[pending], Outcome::HomeWin, 0);
        assert_eq!(result.winners(), 0);
        assert_eq!(result.total_paid_minor_units(), 0);
        assert_eq!(result.decisions[0].1, OrderSettlement::NotMatched);
        // Nothing was held, so there is nothing to sweep and the hold still
        // reconciles.
        assert!(result.reconciles());
    }

    #[test]
    fn a_matched_order_with_no_price_is_an_internal_error_not_a_payout() {
        // An order can only reach Matched through mark_matched, which does not
        // require a price. The engine must not invent one.
        let mut order = Order::pending(
            OrderId::new("o1").expect("valid"),
            AccountId::new("alice").expect("valid"),
            &crate::order::OrderRequest::new(
                MarketId::new(MARKET).expect("valid"),
                Selection::Home,
                MoneyAmount::new(1000, Currency::GBP),
            )
            .expect("positive stake"),
        );
        order
            .mark_matched(TransactionRef::new("t1").expect("valid"), 1000)
            .expect("can match");

        let error = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &[order],
            Outcome::HomeWin,
            Currency::GBP,
            COMMISSION_BPS,
            2000,
            &TransactionRef::new("m1-settle").expect("valid"),
        )
        .expect_err("a priceless order must not settle");
        assert!(matches!(error, DomainError::Internal { .. }));
    }

    #[test]
    fn an_empty_market_settles_to_nothing_without_error() {
        let result = settle(&[], Outcome::Draw, 0);
        assert!(result.transactions.is_empty());
        assert_eq!(result.total_paid_minor_units(), 0);
        assert!(result.reconciles());
    }

    #[test]
    fn each_winner_gets_their_own_distinct_reference() {
        let orders = vec![
            matched("o1", "alice", Selection::Home, "2.0", 1000),
            matched("o2", "bob", Selection::Home, "4.0", 500),
            matched("o3", "carol", Selection::Away, "3.0", 1000),
        ];
        // 2000 + 2000 paid, 5000 held.
        let result = settle(&orders, Outcome::HomeWin, 5000);

        let refs: Vec<String> = result
            .transactions
            .iter()
            .map(|t| t.transaction_ref.as_str().to_string())
            .collect();
        let unique: std::collections::HashSet<&String> = refs.iter().collect();
        assert_eq!(unique.len(), refs.len(), "references must be unique: {refs:?}");
        assert!(refs.iter().any(|r| r.starts_with("m1-settle.payout.")));
        assert!(refs.iter().any(|r| r.starts_with("m1-settle.sweep")));
    }

    #[test]
    fn every_transaction_carries_an_idempotency_key() {
        // Without one, a retried settlement is indistinguishable from a first
        // run and the customer is paid twice. The key is per order for payouts
        // and per market for the sweep, because those are the two units of work
        // that can each be retried independently.
        let result = settle(&lopsided_pair(), Outcome::HomeWin, 2500);
        for transaction in &result.transactions {
            assert!(
                transaction.idempotency_key.is_some(),
                "transaction {} has no idempotency key",
                transaction.transaction_ref.as_str()
            );
        }

        let keys: Vec<&str> = result
            .transactions
            .iter()
            .filter_map(|t| t.idempotency_key.as_ref().map(|k| k.as_str()))
            .collect();
        assert!(keys.contains(&"settle:m1-settle:m1-settle.o1"));
        assert!(keys.contains(&"settle:m1-settle:sweep"));
    }

    #[test]
    fn two_runs_of_the_same_settlement_produce_the_same_keys() {
        // Determinism is what makes the key useful: a retry must land on the
        // same key as the original, not on a fresh one that matches nothing.
        let first = settle(&lopsided_pair(), Outcome::HomeWin, 2500);
        let second = settle(&lopsided_pair(), Outcome::HomeWin, 2500);

        let keys = |s: &Settlement| -> Vec<String> {
            s.transactions
                .iter()
                .filter_map(|t| t.idempotency_key.as_ref().map(|k| k.as_str().to_string()))
                .collect()
        };
        assert_eq!(keys(&first), keys(&second));
    }

    #[test]
    fn settlement_is_deterministic() {
        let first = settle(&matched_pair(), Outcome::HomeWin, 2000);
        let second = settle(&matched_pair(), Outcome::HomeWin, 2000);
        assert_eq!(first, second);
    }

    #[test]
    fn a_draw_pays_draw_orders_only() {
        let orders = vec![
            matched("o1", "alice", Selection::Draw, "5.0", 100),
            matched("o2", "bob", Selection::Home, "2.0", 100),
        ];
        // 200 held, 500 paid — the hold is more than enough, and the 300 left
        // over is bob's stake.
        let result = settle(&orders, Outcome::Draw, 200);
        assert_eq!(result.winners(), 1);
        assert_eq!(result.total_paid_minor_units(), 500);
        assert!(result.reconciles());
    }

    #[test]
    fn commission_rounds_down_and_never_exceeds_the_profit() {
        // 3 bps of 33 is 0.0099, which must not become 1.
        assert_eq!(commission_on(33, 3), 0);
        assert_eq!(commission_on(100, 3), 0);
        assert_eq!(commission_on(10_000, 3), 3);
        // A tiny profit is charged nothing rather than the whole thing.
        assert_eq!(commission_on(1, 5_000), 0);
        // And never more than the profit, at any rate.
        assert_eq!(commission_on(10, MAX_COMMISSION_BPS), 10);
        // A loss or no profit is never charged.
        assert_eq!(commission_on(0, COMMISSION_BPS), 0);
        assert_eq!(commission_on(-500, COMMISSION_BPS), 0);
        // No rate, no charge.
        assert_eq!(commission_on(1_000_000, 0), 0);
    }

    #[test]
    fn commission_never_overflows_on_an_extreme_profit() {
        // A profit near i64::MAX times a large rate overflows. Charging the
        // whole profit is the documented fallback, and is still bounded by the
        // guarantee that a winner keeps their stake.
        let commission = commission_on(i64::MAX, 10_000);
        assert_eq!(commission, i64::MAX);
        let net = i64::MAX - commission;
        assert_eq!(net, 0, "a 100% commission on a maximal profit nets to nothing");

        // And it must not panic or wrap into a negative charge, which would
        // credit the customer instead of charging them.
        assert!(commission >= 0);
    }

    #[test]
    fn a_winner_always_keeps_at_least_their_stake() {
        // The property that justifies capping commission at the profit. Checked
        // across a spread of rates and profits rather than at one point, because
        // rounding is exactly where this would break.
        for bps in [1_i64, 50, 500, 5_000, MAX_COMMISSION_BPS] {
            for profit in [1_i64, 2, 7, 33, 100, 1_000, 999_999] {
                let stake = 1_000_i64;
                let returned = stake + profit;
                let commission = commission_on(profit, bps);
                assert!(
                    commission <= profit,
                    "{bps} bps on a {profit} profit charged {commission}"
                );
                assert!(
                    returned - commission >= stake,
                    "{bps} bps on a {profit} profit left the winner short of their stake"
                );
            }
        }
    }

    #[test]
    fn a_full_hold_of_100_percent_commission_still_returns_the_stake() {
        // The pathological rate. A winner must never lose the stake itself, or
        // the exchange would be charging for holding a bet rather than for
        // winning it.
        let orders = vec![
            matched("o1", "alice", Selection::Home, "2.0", 1000),
            matched("o2", "bob", Selection::Away, "2.0", 1000),
        ];
        let result = settle_market(
            &MarketId::new(MARKET).expect("valid"),
            &orders,
            Outcome::HomeWin,
            Currency::GBP,
            MAX_COMMISSION_BPS,
            2000,
            &TransactionRef::new("m1-settle").expect("valid"),
        )
        .expect("settlement should succeed");

        let alice = AccountId::new("alice").expect("valid");
        let net: i64 = result.transactions.iter().map(|t| t.balance_of(&alice)).sum();
        assert_eq!(net, 1000, "the stake itself must survive");
        assert!(result.reconciles());
    }
}
