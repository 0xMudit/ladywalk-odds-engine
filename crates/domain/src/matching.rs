//! # Matching
//!
//! Deciding which resting order a new order fills against, and turning that
//! decision into the ledger transaction that takes the money.
//!
//! ## Price agreement, not price crossing
//!
//! This is a fixed-odds exchange, not a limit-order book. The server publishes
//! one price per market at a time, so there is no bid/ask to cross and no
//! better price to wait for. An order fills when the resting book already
//! contains an order on the opposite selection from a different customer at
//! the same published price. A customer who cannot fill at the published price
//! does not get a worse fill — they do not get a fill.
//!
//! That is why [`match_order`] takes the price from the incoming order and
//! requires every maker to carry it, rather than comparing two prices and
//! choosing the better one. A crossing rule would be the correct rule for a
//! different product, and would be a bug here.
//!
//! ## The taker's stake is a ceiling, not a payment
//!
//! A taker arriving with 1000 fills against makers offering 600, 300 and 500,
//! in book order, until its 1000 is used up: 600 + 300, then stop. The 500 stays
//! resting for whoever comes next, and the 100 the taker could not place is
//! never taken from them at all.
//!
//! The mistake this guards against is taking the taker's whole stake and then
//! filling it against every eligible maker. With a 1000 taker and three 1000
//! makers that takes 1000 from the taker and 3000 from the makers, so the hold
//! holds 4000 for a bet whose maximum payout is 2000, and the exchange is
//! quietly financing the imbalance out of its own hold. A maker's stake is a
//! ceiling in exactly the same way: a 1500 maker against a 1000 taker is filled
//! for 1000 and keeps 500 of exposure outstanding, which is a smaller mistake
//! than the reverse because the money never leaves the maker's account.
//!
//! Taking only the committed amount is what makes the hold self-funding. Every
//! minor unit in `hold:<market>` is a minor unit a customer chose to risk, so
//! [`settle_market`](crate::settlement::settle_market) can pay every winner
//! out of the hold without reaching for a platform balance.
//!
//! ## The book is a snapshot
//!
//! Matching is a pure function of the incoming order and a list of resting
//! orders. It does not mutate the book and does not read a clock, so a caller
//! can match against a book, persist the result, and be certain that a retry
//! of the same request against the same book produces the same result. The
//! concurrency control — making sure the book has not changed between reading
//! it and writing the matches — belongs to the application layer, which owns
//! the transaction.

use std::fmt;

use crate::error::DomainError;
use crate::ids::{AccountId, IdempotencyKey, MarketId, OrderId, TransactionRef};
use crate::ledger::{EntryReason, LedgerLeg, LedgerTransaction};
use crate::order::{Order, OrderStatus};

/// One filled order, and the order it filled against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedPair {
    /// The order being filled.
    pub taker_id: OrderId,
    /// The resting order it filled against.
    pub maker_id: OrderId,
    /// The account whose stake is taken.
    pub taker_account_id: AccountId,
    /// The account whose stake is taken.
    pub maker_account_id: AccountId,
    /// The stake in minor units, identical for both sides.
    pub stake_minor_units: i64,
    /// The ledger transaction that holds both stakes.
    pub holding: TransactionRef,
}

impl MatchedPair {
    /// Whether this fill exhausts the taker's stake.
    ///
    /// A taker whose final fill consumes its whole stake is fully committed and
    /// is marked `Matched` outright; one that runs out of book first is marked
    /// `PartiallyFilled` and keeps whatever it could not place.
    #[must_use]
    pub fn exhausts_taker(&self, taker_stake_minor_units: i64) -> bool {
        self.stake_minor_units == taker_stake_minor_units
    }
}

/// The result of matching one order against a book.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MatchOutcome {
    /// The fills, in book order.
    pub pairs: Vec<MatchedPair>,
    /// The transaction that took every matched stake.
    ///
    /// One transaction for the whole match rather than one per pair: either
    /// every fill is funded or none is, and a partially-funded match is the
    /// worst possible state to have to reconcile.
    pub holding: Option<LedgerTransaction>,
    /// The part of the taker's stake that was filled, in minor units.
    ///
    /// Always equal to the sum of [`MatchedPair::stake_minor_units`] and never
    /// more than the taker's stake. Only the filled part is taken from the
    /// taker's account, so this is the amount the customer has actually risked.
    pub taker_filled_minor_units: i64,
}

impl MatchOutcome {
    /// Whether the order found at least one fill.
    #[must_use]
    pub fn is_matched(&self) -> bool {
        !self.pairs.is_empty()
    }

    /// The number of resting orders that were filled.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.pairs.len()
    }

    /// The total stake committed by the makers that were filled.
    #[must_use]
    pub fn maker_stake_minor_units(&self) -> i64 {
        self.pairs.iter().fold(0_i64, |acc, pair| {
            acc.saturating_add(pair.stake_minor_units)
        })
    }

    /// The part of the taker's own stake that found no counterparty.
    ///
    /// This money is never taken from the customer, so there is nothing for the
    /// caller to release. It is reported so a caller can tell a customer their
    /// order was only partly filled, which is the difference between an order
    /// that worked and one that silently did nothing.
    #[must_use]
    pub fn taker_unfilled_minor_units(&self, taker_stake_minor_units: i64) -> i64 {
        taker_stake_minor_units.saturating_sub(self.taker_filled_minor_units)
    }
}

/// The account customer stakes are held in while a market is running.
///
/// # Errors
///
/// Returns [`DomainError::InvalidInput`] if the prefixed name is longer than an
/// identifier may be. That is reachable — a market id may already be at the
/// length limit — so the result is checked rather than assumed.
pub fn holding_account_id(market_id: &MarketId) -> Result<AccountId, DomainError> {
    AccountId::new(format!("hold:{market_id}")).map_err(|e| DomainError::InvalidInput {
        field: "market_id",
        reason: format!("the market id cannot name a holding account: {e}"),
    })
}

/// Match an incoming order against a book of resting orders.
///
/// The incoming order's stake is a ceiling: it is filled against eligible makers
/// in book order until either the stake is exhausted or the book runs out, and
/// only the filled part is taken from its account. See the module
/// documentation for why.
///
/// # Errors
///
/// Returns [`DomainError::InvalidInput`] if the incoming order is not
/// `Pending`, or [`DomainError::StaleQuote`] if it is not quoted at the
/// market's published price. Returns [`crate::money::MoneyError::Overflow`]
/// wrapped in [`DomainError::Money`] if the running maker total overflows.
pub fn match_order(
    incoming: &Order,
    published_odds: crate::odds::Odds,
    book: &[Order],
    holding_ref: &TransactionRef,
) -> Result<MatchOutcome, DomainError> {
    if incoming.status != OrderStatus::Pending {
        return Err(DomainError::InvalidInput {
            field: "order",
            reason: "only a pending order can be matched".to_string(),
        });
    }
    if incoming.quoted_odds().is_err() {
        return Err(DomainError::StaleQuote);
    }
    if incoming.quoted_odds()? != published_odds {
        return Err(DomainError::StaleQuote);
    }

    let holding = holding_account_id(&incoming.market_id)?;
    let taker_stake = incoming.stake_minor_units;
    let mut remaining_taker = taker_stake;
    let mut pairs: Vec<MatchedPair> = Vec::new();
    let mut filled_makers: Vec<OrderId> = Vec::new();
    // Accumulated only to prove the total cannot overflow. It is bounded above by
    // the taker's stake by construction, so the checked add can only fail if the
    // taker's own stake was already the maximum.
    let mut maker_total: i64 = 0;
    let mut maker_legs: Vec<LedgerLeg> = Vec::new();

    for maker in book {
        if remaining_taker == 0 {
            break;
        }
        if !incoming.is_matchable_by(maker) {
            continue;
        }
        // Only fill against orders quoted at the published price. A maker that
        // has not been quoted yet, or was quoted at a stale price, is not
        // available to fill even though it is otherwise eligible.
        if maker.quoted_odds().ok() != Some(published_odds) {
            continue;
        }
        // A book that lists the same order twice must not fill it twice. The
        // caller is responsible for a consistent snapshot, but a duplicate
        // would take a customer's money a second time, and that is worth
        // defending against here rather than trusting.
        if filled_makers.contains(&maker.id) {
            continue;
        }

        // The fill is the smaller of what the taker still has and what the
        // maker offered. Neither side is committed to more than its own stake.
        let stake = remaining_taker.min(maker.stake_minor_units);
        if stake <= 0 {
            continue;
        }
        remaining_taker -= stake;

        maker_total = maker_total
            .checked_add(stake)
            .ok_or(DomainError::Money(crate::money::MoneyError::Overflow))?;
        filled_makers.push(maker.id.clone());

        maker_legs.push(LedgerLeg::new(maker.account_id.clone(), -stake)?);
        maker_legs.push(LedgerLeg::new(holding.clone(), stake)?);

        pairs.push(MatchedPair {
            taker_id: incoming.id.clone(),
            maker_id: maker.id.clone(),
            taker_account_id: incoming.account_id.clone(),
            maker_account_id: maker.account_id.clone(),
            stake_minor_units: stake,
            holding: holding_ref.clone(),
        });
    }

    if pairs.is_empty() {
        return Ok(MatchOutcome::default());
    }

    // Only the filled part of the taker's stake is taken, and it is taken once
    // however many makers it was spread across. Every leg is the taker or a
    // maker moving money into the holding account, so the sum is zero by
    // construction and the checked-zero rule in `LedgerTransaction::new` has
    // nothing left to catch.
    let taker_committed = taker_stake - remaining_taker;
    debug_assert_eq!(
        taker_committed, maker_total,
        "the taker's committed stake and the makers' must be equal, or the hold is not self-funding"
    );

    let mut legs = Vec::with_capacity(maker_legs.len() + 2);
    legs.push(LedgerLeg::new(incoming.account_id.clone(), -taker_committed)?);
    legs.push(LedgerLeg::new(holding.clone(), taker_committed)?);
    legs.extend(maker_legs);

    let currency = incoming.stake()?.currency;

    let idempotency = IdempotencyKey::new(format!(
        "match:{}:{}",
        holding_ref.as_str(),
        incoming.id.as_str()
    ))
    .map_err(|e| DomainError::internal(format!("generated an invalid idempotency key: {e}")))?;

    let transaction = LedgerTransaction::new(
        holding_ref.clone(),
        currency,
        EntryReason::BetStake,
        Some(idempotency),
        legs,
    )?;

    Ok(MatchOutcome {
        pairs,
        holding: Some(transaction),
        taker_filled_minor_units: taker_committed,
    })
}

impl fmt::Display for MatchedPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} filled against {} for {} minor units",
            self.taker_id.as_str(),
            self.maker_id.as_str(),
            self.stake_minor_units
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use crate::money::MoneyAmount;
    use crate::odds::Odds;
    use crate::order::Selection;

    fn odds(text: &str) -> Odds {
        Odds::parse(text).expect("valid odds")
    }

    fn order(id: &str, account: &str, selection: Selection, stake: i64, price: &str) -> Order {
        let market = MarketId::new("m1").expect("valid");
        let request = crate::order::OrderRequest::new(
            market,
            selection,
            MoneyAmount::new(stake, Currency::GBP),
        )
        .expect("positive stake");
        let mut o = Order::pending(OrderId::new(id).expect("valid"), AccountId::new(account).expect("valid"), &request);
        o.apply_quote(odds(price)).expect("pending order can be quoted");
        o
    }

    fn match_it(incoming: &Order, book: &[Order]) -> Result<MatchOutcome, DomainError> {
        match_order(
            incoming,
            incoming.quoted_odds()?,
            book,
            &TransactionRef::new("hold-1").expect("valid"),
        )
    }

    #[test]
    fn an_order_fills_against_the_opposite_selection() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 1000, "2.0");
        let outcome = match_it(&taker, &[maker]).expect("should match");

        assert!(outcome.is_matched());
        assert_eq!(outcome.depth(), 1);
        assert_eq!(outcome.pairs[0].maker_id.as_str(), "m1");
        assert_eq!(outcome.pairs[0].stake_minor_units, 1000);
    }

    #[test]
    fn the_maker_may_have_a_different_stake() {
        // The fill is the smaller of the two. A 1000 taker against a 400 maker
        // is filled for 400 and keeps 600 of its stake in its own account —
        // taking the whole 1000 would be charging the customer for risk it never
        // found a counterparty for.
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 400, "2.0");
        let outcome = match_it(&taker, &[maker]).expect("should match");

        assert_eq!(outcome.pairs[0].stake_minor_units, 400);
        assert_eq!(outcome.taker_filled_minor_units, 400);
        assert_eq!(outcome.taker_unfilled_minor_units(1000), 600);
    }

    #[test]
    fn a_taker_is_never_charged_for_more_than_it_filled() {
        // The whole hold funds every payout, so the taker's debit must equal the
        // total it actually matched. Anything larger is the exchange putting its
        // own money behind a customer's over-commitment.
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 400, "2.0");
        let outcome = match_it(&taker, &[maker]).expect("should match");
        let transaction = outcome.holding.expect("a holding was written");

        assert_eq!(
            transaction.balance_of(&AccountId::new("alice").expect("valid")),
            -400
        );
        assert_eq!(
            transaction.balance_of(&AccountId::new("bob").expect("valid")),
            -400
        );
        assert_eq!(
            transaction.balance_of(
                &holding_account_id(&MarketId::new("m1").expect("valid")).expect("short market id")
            ),
            800
        );
    }

    #[test]
    fn the_holding_transaction_balances_and_records_its_reason() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 1000, "2.0");
        let outcome = match_it(&taker, &[maker]).expect("should match");
        let transaction = outcome.holding.expect("a holding was written");

        assert_eq!(transaction.total_minor_units(), 0);
        assert_eq!(transaction.balance_of(&AccountId::new("alice").expect("valid")), -1000);
        assert_eq!(transaction.balance_of(&AccountId::new("bob").expect("valid")), -1000);
        assert_eq!(
            transaction.balance_of(
                &holding_account_id(&MarketId::new("m1").expect("valid")).expect("short market id")
            ),
            2_000
        );
        assert_eq!(transaction.reason, EntryReason::BetStake);
    }

    #[test]
    fn an_order_cannot_fill_against_its_own_account() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "alice", Selection::Away, 1000, "2.0");
        assert!(!match_it(&taker, &[maker]).expect("no error").is_matched());
    }

    #[test]
    fn an_order_cannot_fill_against_the_same_selection() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Home, 1000, "2.0");
        assert!(!match_it(&taker, &[maker]).expect("no error").is_matched());
    }

    #[test]
    fn an_order_cannot_fill_against_another_market() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let mut maker = order("m1", "bob", Selection::Away, 1000, "2.0");
        maker.market_id = MarketId::new("m2").expect("valid");
        assert!(!match_it(&taker, &[maker]).expect("no error").is_matched());
    }

    #[test]
    fn a_maker_at_a_different_price_is_not_filled() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 1000, "3.0");
        let outcome = match_it(&taker, &[maker]).expect("no error");
        assert!(!outcome.is_matched());
        assert!(outcome.holding.is_none());
    }

    #[test]
    fn an_unquoted_maker_is_not_filled() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let mut maker = order("m1", "bob", Selection::Away, 1000, "2.0");
        maker.odds = None;
        assert!(!match_it(&taker, &[maker]).expect("no error").is_matched());
    }

    #[test]
    fn an_unquoted_taker_is_a_stale_quote() {
        let mut taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        taker.odds = None;
        assert!(matches!(
            match_order(
                &taker,
                odds("2.0"),
                &[],
                &TransactionRef::new("hold-1").expect("valid")
            ),
            Err(DomainError::StaleQuote)
        ));
    }

    #[test]
    fn a_taker_at_a_stale_price_is_a_stale_quote() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        assert!(matches!(
            match_order(
                &taker,
                odds("2.5"),
                &[],
                &TransactionRef::new("hold-1").expect("valid")
            ),
            Err(DomainError::StaleQuote)
        ));
    }

    #[test]
    fn a_matched_maker_is_not_filled_again() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let mut maker = order("m1", "bob", Selection::Away, 1000, "2.0");
        maker
            .mark_matched(TransactionRef::new("h").expect("valid"), 1000)
            .expect("pending can match");
        assert!(!match_it(&taker, &[maker]).expect("no error").is_matched());
    }

    #[test]
    fn a_maker_is_filled_at_most_once_even_if_the_book_lists_it_twice() {
        // A book assembled from two queries, or joined against a table twice,
        // can list the same order more than once. Filling it twice would take a
        // customer's money a second time against a stake they never committed.
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 1000, "2.0");
        let duplicated = vec![maker.clone(), maker];

        let outcome = match_it(&taker, &duplicated).expect("should match");
        assert_eq!(outcome.depth(), 1);
        let transaction = outcome.holding.expect("a holding was written");
        assert_eq!(transaction.total_minor_units(), 0);
        assert_eq!(transaction.balance_of(&AccountId::new("bob").expect("valid")), -1000);
    }

    #[test]
    fn a_taker_spreads_its_stake_across_several_makers_in_book_order() {
        // 1000 filled against 400 then 350: 650 committed, 350 left over, and
        // carol untouched. The hold holds 1300, which is exactly what the two
        // matched pairs put there — no more.
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let book = vec![
            order("m1", "bob", Selection::Away, 400, "2.0"),
            order("m2", "carol", Selection::Away, 350, "2.0"),
            order("m3", "dave", Selection::Away, 250, "2.0"),
        ];
        let outcome = match_it(&taker, &book).expect("should match");

        assert_eq!(outcome.depth(), 2);
        assert_eq!(outcome.maker_stake_minor_units(), 750);
        assert_eq!(outcome.taker_filled_minor_units, 750);
        assert_eq!(outcome.taker_unfilled_minor_units(1000), 250);

        // Fills in book order, and never reaching past the taker's stake.
        assert_eq!(outcome.pairs[0].maker_id.as_str(), "m1");
        assert_eq!(outcome.pairs[0].stake_minor_units, 400);
        assert_eq!(outcome.pairs[1].maker_id.as_str(), "m2");
        assert_eq!(outcome.pairs[1].stake_minor_units, 350);

        let transaction = outcome.holding.expect("a holding was written");
        assert_eq!(transaction.total_minor_units(), 0);
        assert_eq!(transaction.balance_of(&AccountId::new("alice").expect("valid")), -750);
        assert_eq!(transaction.balance_of(&AccountId::new("dave").expect("valid")), 0);
        assert_eq!(
            transaction.balance_of(
                &holding_account_id(&MarketId::new("m1").expect("valid")).expect("short market id")
            ),
            1_500
        );
        // Two legs for the taker, then a debit and a credit per fill.
        assert_eq!(transaction.legs.len(), 6);
    }

    #[test]
    fn a_taker_stops_at_the_maker_that_exhausts_its_stake() {
        // The bug this guards against: taking the taker's whole 1000 and then
        // filling it against every eligible maker takes 2500 into a hold for a
        // bet that can pay at most 2000.
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let book = vec![
            order("m1", "bob", Selection::Away, 1000, "2.0"),
            order("m2", "carol", Selection::Away, 1000, "2.0"),
            order("m3", "dave", Selection::Away, 1000, "2.0"),
        ];
        let outcome = match_it(&taker, &book).expect("should match");

        assert_eq!(outcome.depth(), 1, "only enough book to fill the taker once");
        assert_eq!(outcome.taker_filled_minor_units, 1000);
        assert_eq!(outcome.taker_unfilled_minor_units(1000), 0);
        assert_eq!(outcome.maker_stake_minor_units(), 1_000);

        let transaction = outcome.holding.expect("a holding was written");
        assert_eq!(transaction.total_minor_units(), 0);
        assert_eq!(transaction.balance_of(&AccountId::new("carol").expect("valid")), 0);
        assert_eq!(
            transaction.balance_of(
                &holding_account_id(&MarketId::new("m1").expect("valid")).expect("short market id")
            ),
            2_000
        );
    }

    #[test]
    fn a_maker_larger_than_the_taker_is_filled_only_for_the_takers_stake() {
        // The maker's remaining 500 never leaves its account, so the hold cannot
        // be short-changed by it later.
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let maker = order("m1", "bob", Selection::Away, 1500, "2.0");
        let outcome = match_it(&taker, &[maker]).expect("should match");

        assert_eq!(outcome.pairs[0].stake_minor_units, 1000);
        let transaction = outcome.holding.expect("a holding was written");
        assert_eq!(transaction.balance_of(&AccountId::new("bob").expect("valid")), -1000);
        assert_eq!(
            transaction.balance_of(
                &holding_account_id(&MarketId::new("m1").expect("valid")).expect("short market id")
            ),
            2_000
        );
    }

    #[test]
    fn a_draw_maker_is_only_fillable_by_another_draw_order() {
        let draw = order("d1", "alice", Selection::Draw, 1000, "3.0");
        let other_draw = order("d2", "bob", Selection::Draw, 1000, "3.0");
        let home = order("h1", "carol", Selection::Home, 1000, "3.0");
        assert!(match_it(&draw, &[other_draw.clone()]).expect("no error").is_matched());
        assert!(!match_it(&draw, &[home]).expect("no error").is_matched());
    }

    #[test]
    fn an_empty_book_matches_nothing_and_writes_nothing() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let outcome = match_it(&taker, &[]).expect("no error");
        assert_eq!(outcome, MatchOutcome::default());
        assert!(outcome.holding.is_none());
    }

    #[test]
    fn the_taker_itself_in_the_book_is_ignored() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let outcome = match_it(&taker, std::slice::from_ref(&taker)).expect("no error");
        assert!(!outcome.is_matched());
    }

    #[test]
    fn a_taker_at_the_stake_limit_fills_cleanly_rather_than_overflowing() {
        // A taker at i64::MAX filling two maximal makers is now bounded by the
        // taker's own stake, so it stops after the first fill instead of
        // accumulating an overflowing total. The old rule — fill every eligible
        // maker — would have needed an overflow guard; the ceiling makes that
        // unreachable, and this pins the behaviour that replaced it.
        let taker = order("t1", "alice", Selection::Home, i64::MAX, "2.0");
        let book = vec![
            order("m1", "bob", Selection::Away, i64::MAX, "2.0"),
            order("m2", "carol", Selection::Away, 1, "2.0"),
        ];
        let outcome = match_it(&taker, &book).expect("no overflow");

        assert_eq!(outcome.depth(), 1);
        assert_eq!(outcome.pairs[0].maker_id.as_str(), "m1");
        assert_eq!(outcome.taker_filled_minor_units, i64::MAX);
        assert_eq!(outcome.maker_stake_minor_units(), i64::MAX);
        let transaction = outcome.holding.expect("a holding was written");
        assert_eq!(transaction.total_minor_units(), 0);
    }

    #[test]
    fn a_maker_at_the_stake_limit_contributes_only_what_the_taker_has_left() {
        // The other direction: a maximal maker against a small taker is filled
        // for the taker's stake, and the maker's own figure never reaches the
        // arithmetic.
        let taker = order("t1", "alice", Selection::Home, 1, "2.0");
        let book = vec![order("m1", "bob", Selection::Away, i64::MAX, "2.0")];
        let outcome = match_it(&taker, &book).expect("should match");

        assert_eq!(outcome.pairs[0].stake_minor_units, 1);
        assert_eq!(outcome.taker_filled_minor_units, 1);
        assert_eq!(outcome.taker_unfilled_minor_units(1), 0);
        let transaction = outcome.holding.expect("a holding was written");
        assert_eq!(transaction.total_minor_units(), 0);
        assert_eq!(transaction.balance_of(&AccountId::new("bob").expect("valid")), -1);
    }

    #[test]
    fn holding_accounts_are_scoped_to_their_market() {
        let a = holding_account_id(&MarketId::new("m1").expect("valid")).expect("short id");
        let b = holding_account_id(&MarketId::new("m2").expect("valid")).expect("short id");
        assert_ne!(a, b);
        assert_eq!(a.as_str(), "hold:m1");
    }

    #[test]
    fn a_market_id_at_the_length_limit_cannot_name_a_holding_account() {
        // A prefixed name over the identifier limit is refused rather than
        // truncated, because two markets whose ids share the first 123
        // characters would otherwise share a holding account.
        let long = "m".repeat(128);
        let market = MarketId::new(long).expect("128 bytes is within the limit");
        assert!(holding_account_id(&market).is_err());
    }

    #[test]
    fn the_holding_is_idempotent_per_taker() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let book = vec![order("m1", "bob", Selection::Away, 1000, "2.0")];
        let first = match_it(&taker, &book).expect("should match");
        let second = match_it(&taker, &book).expect("should match");
        assert_eq!(first.holding, second.holding);
        let key = first
            .holding
            .as_ref()
            .and_then(|t| t.idempotency_key.clone())
            .expect("the holding carries a key");
        assert!(key.as_str().starts_with("match:hold-1:t1"));
    }

    #[test]
    fn matching_is_deterministic_for_a_fixed_book() {
        let taker = order("t1", "alice", Selection::Home, 1000, "2.0");
        let book = vec![
            order("m1", "bob", Selection::Away, 1000, "2.0"),
            order("m2", "carol", Selection::Away, 500, "2.0"),
        ];
        assert_eq!(
            match_it(&taker, &book).expect("should match"),
            match_it(&taker, &book).expect("should match")
        );
    }

    #[test]
    fn the_hold_holds_exactly_twice_the_matched_stake() {
        // The invariant settlement depends on: every matched pair puts the same
        // amount in from both sides, so the hold is twice the exposure and can
        // never be short. Checked across books that mix full and partial fills,
        // because a partial fill is where this breaks if it breaks anywhere.
        for (taker_stake, maker_stakes) in [
            (1_000_i64, vec![1_000_i64]),
            (1_000, vec![400, 350, 250]),
            (1_000, vec![5_000]),
            (3, vec![1, 1, 1]),
        ] {
            let taker = order("t1", "alice", Selection::Home, taker_stake, "2.0");
            let book: Vec<Order> = maker_stakes
                .iter()
                .enumerate()
                .map(|(i, stake)| {
                    order(
                        &format!("m{i}"),
                        &format!("maker{i}"),
                        Selection::Away,
                        *stake,
                        "2.0",
                    )
                })
                .collect();

            let outcome = match_it(&taker, &book).expect("should match");
            let matched = outcome.maker_stake_minor_units();
            let transaction = outcome.holding.expect("a holding was written");
            let hold = transaction.balance_of(
                &holding_account_id(&MarketId::new("m1").expect("valid")).expect("short id"),
            );

            assert_eq!(
                outcome.taker_filled_minor_units, matched,
                "the taker's committed stake must equal the makers'"
            );
            assert!(
                matched <= taker_stake,
                "a taker of {taker_stake} was committed {matched}"
            );
            assert_eq!(hold, 2 * matched, "the hold must hold both sides and no more");
            assert_eq!(transaction.total_minor_units(), 0);
        }
    }
}
