//! # Orders
//!
//! An order is a customer's standing instruction: back this selection at these
//! odds for this much money, or take those odds. This module owns the order
//! lifecycle and the rules that decide when a new order may be accepted.
//!
//! ## Why an order can never carry a price the client chose
//!
//! [`OrderRequest`] deliberately has no field for the odds. The customer names
//! a selection and a stake; the server decides the price, and it records that
//! decision as a signed [`crate::quote::Quote`]. An order that arrived already
//! priced would make the whole system a client that trusts its own arithmetic,
//! and no amount of validation afterwards would fix that.
//!
//! ## Lifecycle
//!
//! ```text
//!   Pending ──match──> Matched ──settle──> Settled
//!      │
//!      └──cancel──> Cancelled
//! ```
//!
//! `Pending` and `Matched` are the only states in which an order can still
//! affect a market. `Settled` and `Cancelled` are terminal, and every
//! transition out of a terminal state is rejected by [`Order::transition`]
//! rather than being merely discouraged by convention.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, MarketId, OrderId, TransactionRef};
use crate::money::MoneyAmount;
use crate::odds::Odds;

/// Which side of a selection an order is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// The outcome the customer is backing.
    Home,
    /// The outcome the customer is taking against.
    Away,
    /// The draw, where the market offers one.
    Draw,
}

impl Selection {
    /// The opposite selection, for deciding which resting orders match.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Home => Self::Away,
            Self::Away => Self::Home,
            Self::Draw => Self::Draw,
        }
    }
}

impl fmt::Display for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Home => f.write_str("home"),
            Self::Away => f.write_str("away"),
            Self::Draw => f.write_str("draw"),
        }
    }
}

/// Where an order is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// Accepted, not yet matched, and can still be cancelled.
    Pending,
    /// Matched against a counterparty, and now funds are held.
    Matched,
    /// The market resolved and the payout has been posted.
    Settled,
    /// Withdrawn by the customer or rejected before matching.
    Cancelled,
}

impl OrderStatus {
    /// Whether no further transition is possible from this state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Settled | Self::Cancelled)
    }
}

/// A customer's instruction to back a selection.
///
/// There is no odds field, by design — see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRequest {
    /// The market the order is placed in.
    pub market_id: MarketId,
    /// The selection being backed.
    pub selection: Selection,
    /// The stake in minor units, in the market's currency.
    pub stake: MoneyAmount,
}

impl OrderRequest {
    /// Create a request, rejecting a non-positive stake.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::InvalidStake`] if the stake is zero or negative.
    /// An order of nothing is not a small order, it is a malformed one.
    pub fn new(
        market_id: MarketId,
        selection: Selection,
        stake: MoneyAmount,
    ) -> Result<Self, OrderError> {
        if !stake.is_positive() {
            return Err(OrderError::InvalidStake);
        }
        Ok(Self {
            market_id,
            selection,
            stake,
        })
    }
}

/// A placed order together with everything the engine decided about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// The order's identity.
    pub id: OrderId,
    /// The customer who placed it.
    pub account_id: AccountId,
    /// The market it was placed in.
    pub market_id: MarketId,
    /// The selection backed.
    pub selection: Selection,
    /// The stake, in minor units.
    pub stake_minor_units: i64,
    /// ISO currency of the stake.
    pub currency_code: String,
    /// The price the server decided, once the order has been quoted. `None`
    /// until then, and a request to match a `None` price is an error rather
    /// than a zero-odds fill.
    pub odds: Option<Odds>,
    /// Current lifecycle state.
    pub status: OrderStatus,
    /// The ledger transaction that moved the stake, if funds are held.
    pub holds_funds: Option<TransactionRef>,
    /// The minor units of this order that were actually matched, once matched.
    ///
    /// An order's stake is what it *asked* for; this is what it *got*. They
    /// differ whenever a book is thin — a 1000 taker against a 600 maker is
    /// filled for 600 and keeps 400 of its stake untouched — and settlement pays
    /// out against this number, not the stake. Paying on the stake would credit
    /// a customer winnings on money they were never charged for risking, which
    /// is a hole in the hold that only shows up once a thin market resolves.
    ///
    /// Zero until the order matches. [`Option`] rather than a bare integer so
    /// that `0` and "never matched" cannot be confused: a matched order with no
    /// exposure is a bug, and a plain `i64` would make it indistinguishable from
    /// an order that simply never filled.
    pub filled_minor_units: Option<i64>,
}

impl Order {
    /// Create a pending, unquoted order.
    #[must_use]
    pub fn pending(
        id: OrderId,
        account_id: AccountId,
        request: &OrderRequest,
    ) -> Self {
        Self {
            id: id.clone(),
            account_id,
            market_id: request.market_id.clone(),
            selection: request.selection,
            stake_minor_units: request.stake.minor_units,
            currency_code: request.stake.currency.to_string(),
            odds: None,
            status: OrderStatus::Pending,
            holds_funds: None,
            filled_minor_units: None,
        }
    }

    /// The stake as a typed amount.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::UnknownCurrency`] if the stored currency code is
    /// not one this build knows. Storing the code rather than the struct is
    /// what lets the row be read without a schema change when a currency is
    /// added.
    pub fn stake(&self) -> Result<MoneyAmount, OrderError> {
        let currency = currency_from_code(&self.currency_code)?;
        Ok(MoneyAmount::new(self.stake_minor_units, currency))
    }

    /// The quoted price, or an error if the order has not been quoted.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::NotQuoted`] if no price has been set.
    pub fn quoted_odds(&self) -> Result<Odds, OrderError> {
        self.odds.ok_or(OrderError::NotQuoted)
    }

    /// Apply a server-decided price.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::AlreadyQuoted`] if a price was already set. A
    /// price is decided once, and re-quoting an order would let a customer be
    /// filled at a price chosen after they knew better.
    pub fn apply_quote(&mut self, odds: Odds) -> Result<(), OrderError> {
        if self.odds.is_some() {
            return Err(OrderError::AlreadyQuoted);
        }
        if self.status != OrderStatus::Pending {
            return Err(OrderError::NotPending(self.status));
        }
        self.odds = Some(odds);
        Ok(())
    }

    /// Record that this order matched, holding the given minor units of it.
    ///
    /// `filled` is the amount that found a counterparty, which may be less than
    /// the stake: an order is filled against a book, not against a promise. It
    /// is checked against the stake rather than trusted, because an exposure
    /// larger than the order's own stake would let a customer be charged for
    /// risk they never agreed to.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::NotPending`] if the order is not `Pending`,
    /// [`OrderError::UnfilledMatch`] if `filled` is not positive, or
    /// [`OrderError::Overfill`] if `filled` exceeds the stake.
    pub fn mark_matched(
        &mut self,
        holds_funds: TransactionRef,
        filled: i64,
    ) -> Result<(), OrderError> {
        if self.status != OrderStatus::Pending {
            return Err(OrderError::NotPending(self.status));
        }
        if filled <= 0 {
            return Err(OrderError::UnfilledMatch);
        }
        if filled > self.stake_minor_units {
            return Err(OrderError::Overfill {
                filled,
                stake: self.stake_minor_units,
            });
        }
        self.status = OrderStatus::Matched;
        self.holds_funds = Some(holds_funds);
        self.filled_minor_units = Some(filled);
        Ok(())
    }

    /// The minor units at risk, for settlement.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::UnfilledMatch`] if the order is not matched. An
    /// unmatched order has no exposure, so there is nothing to pay out on.
    pub fn exposure_minor_units(&self) -> Result<i64, OrderError> {
        self.filled_minor_units
            .ok_or(OrderError::UnfilledMatch)
    }

    /// Record that the order was cancelled and its stake released.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::NotCancellable]` if the order is not `Pending`.
    /// A matched order's funds are held against the counterparty, so cancelling
    /// it is a settlement operation and not a withdrawal.
    pub fn mark_cancelled(&mut self) -> Result<(), OrderError> {
        if self.status != OrderStatus::Pending {
            return Err(OrderError::NotCancellable(self.status));
        }
        self.status = OrderStatus::Cancelled;
        self.holds_funds = None;
        Ok(())
    }

    /// Record that the order has been settled.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::NotSettlable`] unless the order is `Matched`. An
    /// order that never matched has nothing to settle, and settling one would
    /// invent a payout.
    pub fn mark_settled(&mut self) -> Result<(), OrderError> {
        if self.status != OrderStatus::Matched {
            return Err(OrderError::NotSettlable(self.status));
        }
        self.status = OrderStatus::Settled;
        Ok(())
    }

    /// Whether a new order on the opposite selection could match this one.
    ///
    /// Price is deliberately not compared here. In a fixed-odds exchange the
    /// server sets one price for a market at a time, so matching is decided by
    /// price agreement, not by crossing; [`crate::matching`] holds the rule.
    #[must_use]
    pub fn is_matchable_by(&self, other: &Order) -> bool {
        self.status == OrderStatus::Pending
            && other.status == OrderStatus::Pending
            && self.market_id == other.market_id
            && self.selection == other.selection.opposite()
            && self.account_id != other.account_id
    }
}

/// Errors produced by the order lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OrderError {
    /// The stake was zero or negative.
    InvalidStake,
    /// The order has no price yet.
    NotQuoted,
    /// A price was already applied.
    AlreadyQuoted,
    /// The transition is not available from the current status.
    NotPending(OrderStatus),
    /// The order is not `Pending`, so it cannot be cancelled.
    NotCancellable(OrderStatus),
    /// The order is not `Matched`, so there is nothing to settle.
    NotSettlable(OrderStatus),
    /// The order is matched but holds no exposure, so there is nothing to
    /// settle it against.
    UnfilledMatch,
    /// A fill was recorded for more than the order's own stake.
    ///
    /// `filled` is what was recorded, `stake` what the order could absorb.
    Overfill {
        /// The total filled amount recorded for the order.
        filled: i64,
        /// The stake the order was placed with.
        stake: i64,
    },
    /// The stored currency code is not known to this build.
    UnknownCurrency(String),
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStake => f.write_str("stake must be greater than zero"),
            Self::NotQuoted => f.write_str("order has not been quoted"),
            Self::AlreadyQuoted => f.write_str("order has already been quoted"),
            Self::NotPending(status) => write!(f, "order is not pending (it is {status:?})"),
            Self::NotCancellable(status) => {
                write!(f, "order cannot be cancelled from {status:?}")
            }
            Self::NotSettlable(status) => write!(f, "order cannot be settled from {status:?}"),
            Self::UnfilledMatch => {
                f.write_str("order is not matched, so it has no exposure to settle")
            }
            Self::Overfill { filled, stake } => write!(
                f,
                "a fill of {filled} minor units exceeds the stake of {stake}"
            ),
            Self::UnknownCurrency(code) => write!(f, "unknown currency code: {code}"),
        }
    }
}

impl std::error::Error for OrderError {}

impl From<OrderError> for crate::error::DomainError {
    fn from(error: OrderError) -> Self {
        match error {
            OrderError::InvalidStake => Self::InvalidInput {
                field: "stake",
                reason: "stake must be greater than zero".to_string(),
            },
            OrderError::NotQuoted | OrderError::AlreadyQuoted => Self::StaleQuote,
            OrderError::NotPending(OrderStatus::Cancelled)
            | OrderError::NotCancellable(OrderStatus::Cancelled) => Self::InvalidInput {
                field: "order",
                reason: "order was cancelled".to_string(),
            },
            OrderError::NotPending(_) | OrderError::NotCancellable(_) => Self::InvalidInput {
                field: "order",
                reason: "order is no longer pending".to_string(),
            },
            OrderError::NotSettlable(_) => Self::InvalidInput {
                field: "order",
                reason: "order cannot be settled in its current state".to_string(),
            },
            OrderError::UnfilledMatch | OrderError::Overfill { .. } => Self::InvalidInput {
                field: "order",
                reason: "order has no valid matched exposure".to_string(),
            },
            OrderError::UnknownCurrency(code) => Self::InvalidInput {
                field: "currency",
                reason: format!("unknown currency code: {code}"),
            },
        }
    }
}

/// Resolve a stored currency code to a [`Currency`].
///
/// # Errors
///
/// Returns [`OrderError::UnknownCurrency`] for any code this build does not
/// know. A new currency is a code change, which is the point: adding one
/// should not be something a deploy can do by accident.
fn currency_from_code(code: &str) -> Result<crate::money::Currency, OrderError> {
    crate::money::Currency::from_code(code)
        .map_err(|_| OrderError::UnknownCurrency(code.to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn ids() -> (OrderId, AccountId, MarketId) {
        (
            OrderId::new("o1").expect("valid"),
            AccountId::new("a1").expect("valid"),
            MarketId::new("m1").expect("valid"),
        )
    }

    fn request(market: MarketId, selection: Selection, minor: i64) -> OrderRequest {
        OrderRequest::new(market, selection, MoneyAmount::new(minor, Currency::GBP))
            .expect("positive stake")
    }

    fn order() -> (Order, AccountId, MarketId) {
        let (id, account, market) = ids();
        let req = request(market.clone(), Selection::Home, 1000);
        (Order::pending(id, account.clone(), &req), account, market)
    }

    #[test]
    fn a_request_needs_a_positive_stake() {
        let market = MarketId::new("m1").expect("valid");
        assert_eq!(
            OrderRequest::new(
                market.clone(),
                Selection::Home,
                MoneyAmount::new(0, Currency::GBP)
            ),
            Err(OrderError::InvalidStake)
        );
        assert_eq!(
            OrderRequest::new(market, Selection::Home, MoneyAmount::new(-1, Currency::GBP)),
            Err(OrderError::InvalidStake)
        );
    }

    #[test]
    fn an_order_is_pending_and_unquoted_when_created() {
        let (o, _, _) = order();
        assert_eq!(o.status, OrderStatus::Pending);
        assert_eq!(o.quoted_odds(), Err(OrderError::NotQuoted));
        assert_eq!(o.stake().expect("GBP is known"), MoneyAmount::new(1000, Currency::GBP));
    }

    #[test]
    fn quoting_is_allowed_once_and_only_while_pending() {
        let (mut o, _, _) = order();
        let odds = Odds::parse("2.5").expect("valid");
        assert_eq!(o.apply_quote(odds), Ok(()));
        assert_eq!(o.quoted_odds(), Ok(odds));
        assert_eq!(o.apply_quote(odds), Err(OrderError::AlreadyQuoted));

        o.mark_cancelled().expect("pending is cancellable");
        assert_eq!(o.apply_quote(odds), Err(OrderError::NotPending(OrderStatus::Cancelled)));
    }

    #[test]
    fn matching_records_the_exposure_and_the_holding() {
        let (mut o, _, _) = order();
        let holds = TransactionRef::new("t1").expect("valid");
        o.apply_quote(Odds::parse("2.5").expect("valid")).expect("quoted");
        assert_eq!(o.mark_matched(holds.clone(), 1000), Ok(()));
        assert_eq!(o.status, OrderStatus::Matched);
        assert_eq!(o.holds_funds, Some(holds));
        assert_eq!(o.exposure_minor_units(), Ok(1000));
    }

    #[test]
    fn a_partially_filled_order_records_only_its_exposure() {
        // The stake is what the customer asked for; the exposure is what found a
        // counterparty. Settlement must pay on the second, and conflating them
        // would pay winnings on 400 that were never taken from the account.
        let (mut o, _, _) = order();
        let holds = TransactionRef::new("t1").expect("valid");
        o.mark_matched(holds, 600).expect("a fill below the stake is valid");

        assert_eq!(o.status, OrderStatus::Matched);
        assert_eq!(o.stake_minor_units, 1000, "the stake is unchanged");
        assert_eq!(o.exposure_minor_units(), Ok(600));
    }

    #[test]
    fn a_fill_beyond_the_stake_is_refused() {
        let (mut o, _, _) = order();
        let holds = TransactionRef::new("t1").expect("valid");
        assert_eq!(
            o.mark_matched(holds, 1001),
            Err(OrderError::Overfill {
                filled: 1001,
                stake: 1000
            })
        );
        assert_eq!(o.status, OrderStatus::Pending);
    }

    #[test]
    fn a_fill_of_nothing_is_refused() {
        // A match with no money against it is not a small match; it is a bug in
        // whatever produced it, and it would settle as a Matched order with no
        // exposure to pay out on.
        let (mut o, _, _) = order();
        let holds = TransactionRef::new("t1").expect("valid");
        assert_eq!(o.mark_matched(holds.clone(), 0), Err(OrderError::UnfilledMatch));
        assert_eq!(o.status, OrderStatus::Pending);
        assert_eq!(o.exposure_minor_units(), Err(OrderError::UnfilledMatch));
    }

    #[test]
    fn an_unmatched_order_has_no_exposure() {
        let (o, _, _) = order();
        assert_eq!(o.filled_minor_units, None);
        assert_eq!(o.exposure_minor_units(), Err(OrderError::UnfilledMatch));
    }

    #[test]
    fn only_pending_orders_can_be_cancelled() {
        let (mut o, _, _) = order();
        o.mark_cancelled().expect("pending is cancellable");
        assert_eq!(o.status, OrderStatus::Cancelled);
        assert!(o.status.is_terminal());
        assert_eq!(o.mark_cancelled(), Err(OrderError::NotCancellable(OrderStatus::Cancelled)));
    }

    #[test]
    fn a_matched_order_cannot_be_cancelled() {
        let (mut o, _, _) = order();
        o.mark_matched(TransactionRef::new("t1").expect("valid"), 1000)
            .expect("pending can match");
        assert_eq!(
            o.mark_cancelled(),
            Err(OrderError::NotCancellable(OrderStatus::Matched))
        );
    }

    #[test]
    fn only_matched_orders_can_be_settled() {
        let (mut o, _, _) = order();
        assert_eq!(
            o.mark_settled(),
            Err(OrderError::NotSettlable(OrderStatus::Pending))
        );

        o.mark_matched(TransactionRef::new("t1").expect("valid"), 1000)
            .expect("pending can match");
        assert_eq!(o.mark_settled(), Ok(()));
        assert!(o.status.is_terminal());
    }

    #[test]
    fn a_settled_order_is_final() {
        let (mut o, _, _) = order();
        o.mark_matched(TransactionRef::new("t1").expect("valid"), 1000)
            .expect("pending can match");
        o.mark_settled().expect("matched can settle");
        assert_eq!(
            o.mark_settled(),
            Err(OrderError::NotSettlable(OrderStatus::Settled))
        );
    }

    #[test]
    fn matching_requires_the_same_market_the_opposite_selection_and_a_different_customer() {
        let (_, a1, market) = ids();
        let a2 = AccountId::new("a2").expect("valid");
        let home = Order::pending(
            OrderId::new("o1").expect("valid"),
            a1.clone(),
            &request(market.clone(), Selection::Home, 1000),
        );
        let other_market = MarketId::new("m2").expect("valid");
        let away_same_market = Order::pending(
            OrderId::new("o2").expect("valid"),
            a2.clone(),
            &request(market.clone(), Selection::Away, 1000),
        );
        let away_other_market = Order::pending(
            OrderId::new("o3").expect("valid"),
            a2.clone(),
            &request(other_market, Selection::Away, 1000),
        );
        let home_same_customer = Order::pending(
            OrderId::new("o4").expect("valid"),
            a1,
            &request(market, Selection::Away, 1000),
        );

        assert!(home.is_matchable_by(&away_same_market));
        assert!(!home.is_matchable_by(&away_other_market));
        assert!(!home.is_matchable_by(&home_same_customer));
    }

    #[test]
    fn opposite_selections_are_symmetric_and_draw_is_its_own_opposite() {
        assert_eq!(Selection::Home.opposite(), Selection::Away);
        assert_eq!(Selection::Away.opposite(), Selection::Home);
        assert_eq!(Selection::Draw.opposite(), Selection::Draw);
    }

    #[test]
    fn an_unknown_currency_code_is_reported_not_guessed() {
        let (mut o, _, _) = order();
        o.currency_code = "XYZ".to_string();
        assert_eq!(
            o.stake(),
            Err(OrderError::UnknownCurrency("XYZ".to_string()))
        );
    }

    #[test]
    fn terminal_states_are_cancelled_and_settled_only() {
        assert!(!OrderStatus::Pending.is_terminal());
        assert!(!OrderStatus::Matched.is_terminal());
        assert!(OrderStatus::Settled.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
    }
}
