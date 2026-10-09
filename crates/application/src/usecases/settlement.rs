//! # Settling a market
//!
//! Turning a resolved market into paid winners and a closed hold.
//!
//! ## The whole design is in `domain::settlement`
//!
//! [`settle_market`] is a pure function from (orders, outcome, commission, held)
//! to a list of balanced transactions, and it refuses to produce a settlement
//! that does not reconcile. This module's job is therefore narrow and the
//! discipline is in what it *refuses* to add:
//!
//! - It does not decide who won. `domain` does.
//! - It does not clamp a payout to what the hold can afford. `domain` refuses the
//!   whole market instead, and this module passes that refusal through.
//! - It does not retry a partially-posted settlement, because there is no such
//!   state: the payouts, the order status changes and the market's own state
//!   change are one transaction.
//!
//! ## Why resolution and settlement are separate steps
//!
//! [`resolve_market`] in [`crate::usecases::markets`] moves a market to
//! `Resolved`. This one moves it to `Settled`. Two steps rather than one because
//! the gap between them is where a customer-visible promise lives: "this market
//! has resolved and you are owed money" is true, and reportable, before any
//! money has moved. Collapsing them would mean a market that is stuck waiting
//! for a payout to be indistinguishable from one that has not resolved, and those
//! are different conversations with a customer and with a regulator.
//!
//! The sweep for markets that are resolved but unsettled exists because that gap
//! is a real operational state, and a system that relies on an operator noticing
//! it has no reconciliation at all.
//!
//! ## Why the exposure is what is settled
//!
//! Payouts are computed from [`domain::order::Order::filled_minor_units`], not from the stake.
//! An order that asked for 1000 and filled for 400 has 400 in the hold and 600
//! still in the customer's account; paying on the stake would credit winnings on
//! money nobody ever took from them, and the hold would come up short at exactly
//! the moment a thin market resolves.
//!
//! Note that a partially filled order is `Matched`, not `Pending`:
//! [`domain::order::Order::mark_matched`] moves an order to `Matched` for any fill at all,
//! however small. So there is no state in which an order has a real claim on the
//! hold and is still treated as never having matched — which is what makes
//! `settle_market`'s classification by status sound rather than merely plausible.
//!
//! ## The hold is read back, not computed
//!
//! The reconciliation check is made twice: once against the plan `domain`
//! produced, before anything is written, and once against the balance the ledger
//! holds after the payouts are written. The second one is the one that matters,
//! and it is why [`SettlementSummary::hold_balance_after_minor`] is read from the
//! ledger rather than derived from [`domain::settlement::Settlement::holding_balance_after`]. A
//! number computed from the plan can only ever confirm the plan, and the plan is
//! precisely what a bug in the adapter gets wrong.

use std::sync::Arc;

use domain::ids::MarketId;
use domain::settlement::{OrderSettlement, Outcome, settle_market};

use crate::errors::AppError;
use crate::events::{EventKind, StreamEvent};
use crate::ports::{MarketState, StoreError, with_retry};
use crate::usecases::{Deps, announce, stage};

/// What a settlement did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementSummary {
    /// The market that was settled.
    pub market_id: MarketId,
    /// The outcome it settled at.
    pub outcome: Outcome,
    /// How many orders were paid.
    pub winners: usize,
    /// How many orders lost their stake.
    pub losers: usize,
    /// The total returned to winners, in minor units.
    pub total_paid_minor: i64,
    /// The total commission taken from winners' profits.
    pub total_commission_minor: i64,
    /// The total that entered the hold, in minor units.
    pub total_held_minor: i64,
    /// The hold's balance afterwards, read back from the ledger after the payouts
    /// were written. Must be zero.
    ///
    /// Read rather than computed, which is the point: a value derived from
    /// [`domain::settlement::Settlement::holding_balance_after`] can only ever report what the plan
    /// said, and a plan is exactly what an adapter bug would get wrong. This is
    /// the committed number.
    pub hold_balance_after_minor: i64,
}

impl SettlementSummary {
    /// Whether the hold closed.
    ///
    /// The post-condition the whole module exists to protect: every minor unit
    /// that entered the hold must leave it, as a payout, as commission, or as the
    /// platform's sweep. A settlement that paid the winners and left money behind
    /// has not failed — it has silently stranded a customer's money, which is
    /// strictly worse than an error, because nothing would report it.
    ///
    /// Checked twice on purpose. `domain` refuses to produce a settlement that
    /// does not reconcile, and this asserts the same identity from the other side
    /// — against the balance the ledger actually holds after the writes — so that
    /// a bug in the adapter cannot slip past a check that only ever inspected the
    /// plan it was given.
    #[must_use]
    pub fn reconciled(&self) -> bool {
        self.hold_balance_after_minor == 0
    }
}

/// Settles a resolved market.
#[derive(Debug, Clone)]
pub struct SettleMarket {
    deps: Arc<Deps>,
}

impl SettleMarket {
    /// A use case that settles markets using `deps`.
    #[must_use]
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Pay out `market_id`.
    ///
    /// # Errors
    ///
    /// - [`AppError::Domain`] with `MarketClosed` if the market is not
    ///   `Resolved`. An open market has no outcome, so there is nothing to pay.
    /// - [`AppError::Domain`] with `InsufficientFunds` if the hold cannot cover
    ///   the payouts. The whole market is refused rather than paid in part: a
    ///   half-paid market has some customers made whole and others left waiting
    ///   with nothing in the ledger to explain it.
    /// - [`AppError::Domain`] with `BettingHalted` if the kill switch is
    ///   thrown. Settlement *pays money out*, so the switch that stops money
    ///   moving stops this too — but note that this is a deliberate asymmetry
    ///   with resolution, which is not refused. Halting trading must not be
    ///   read as "stop paying the customers who already won".
    /// - [`AppError::Store`] on any storage failure. Nothing is written unless
    ///   everything is.
    pub async fn execute(
        &self,
        market_id: &MarketId,
    ) -> Result<SettlementSummary, AppError> {
        let deps = Arc::clone(&self.deps);
        let factory = Arc::clone(&deps.store);
        let market_id = market_id.clone();
        with_retry(factory, |factory| {
            let deps = Arc::clone(&deps);
            let market_id = market_id.clone();
            async move {
                let mut store = factory.begin().await?;
                Self::attempt(&deps, &mut store, &market_id).await
            }
        })
        .await
    }

    /// One attempt, inside its own unit of work.
    async fn attempt(
        deps: &Deps,
        store: &mut Box<dyn crate::ports::Store>,
        market_id: &MarketId,
    ) -> Result<SettlementSummary, AppError> {
        // The market row is locked for the whole of settlement. Two concurrent
        // attempts would each read the same hold, each compute a full payout, and
        // each try to post it — and the second would find the hold already
        // emptied, which by then is a `InsufficientFunds` error reported against
        // a market that has already been paid. The lock makes the second attempt
        // wait and then find the market `Settled`.
        let market = store.markets().lock(market_id).await?;

        if market.state == MarketState::Settled {
            return Err(domain::error::DomainError::InvalidInput {
                field: "market",
                reason: "this market has already been settled".to_string(),
            }
            .into());
        }
        if market.state == MarketState::Quarantined {
            // A quarantined market is not merely un-settled, it is under
            // investigation, and the distinction matters to whoever has to act on
            // it. `MarketClosed` would say "this market is over" and send an
            // operator looking for a result rather than for a bug.
            return Err(AppError::Domain(
                domain::error::DomainError::MarketQuarantined {
                    market_id: market_id.as_str().to_string(),
                },
            ));
        }
        if !market.state.can_settle() {
            return Err(AppError::Domain(
                domain::error::DomainError::MarketClosed,
            ));
        }

        // Settlement pays money out, so the kill switch applies. Reading it inside
        // the transaction means an operator who halts trading cannot have a
        // settlement commit in the same instant.
        if store.risk().kill_switch().await? == domain::risk::KillSwitchState::Halted {
            return Err(AppError::Domain(
                domain::error::DomainError::BettingHalted,
            ));
        }

        let outcome = market.outcome.ok_or_else(|| {
            AppError::Domain(domain::error::DomainError::internal_with_context(
                "a resolved market has no recorded outcome",
                market_id.as_str(),
            ))
        })?;

        // Read every order the market ever matched. Not just the winners: the
        // losers are needed so that their orders can be marked settled, and a
        // market with losers still open would have a book that no later query
        // expects to find.
        //
        // Resting orders are *not* read here — see
        // [`OrderRepository::matched_orders`] for why a settlement must not be
        // able to see a `Pending` order — and they are voided separately below.
        let orders = store.orders().matched_orders(market_id).await?;
        if orders.is_empty() {
            return Err(domain::error::DomainError::InvalidInput {
                field: "market",
                reason: "this market has no matched orders to settle".to_string(),
            }
            .into());
        }

        // The hold's current value, from the ledger, as the source of truth. Not
        // the sum of the orders' exposures: if those two ever disagree, that is a
        // bug to be investigated, and using the ledger means settlement pays out
        // what is actually there rather than what the order rows claim.
        let holding = domain::matching::holding_account_id(market_id)?;
        let held = store.ledger().balance_minor(&holding).await?;
        if held <= 0 {
            return Err(domain::error::DomainError::InvalidInput {
                field: "market",
                reason: format!(
                    "the hold for this market is empty ({held} minor units); there is \
                     nothing to settle and the order rows disagree with the ledger"
                ),
            }
            .into());
        }

        let prefix = deps.transaction_ref(&format!("settle:{}", market_id.as_str()))?;
        let settlement = settle_market(
            market_id,
            &orders,
            outcome,
            market.currency,
            deps.commission_bps,
            held,
            &prefix,
        )?;

        // Assert `domain`'s own post-condition before anything is written. It is
        // checked again from the other side after the writes, but checking the
        // plan first means a settlement that does not account for the whole hold
        // costs nothing at all — there is no rollback to reason about, because
        // nothing was posted.
        if !settlement.reconciles() {
            return Err(AppError::Store(StoreError::Backend(format!(
                "settlement of market {} does not account for the {} minor units it was \
                 given: the plan leaves {} in the hold",
                market_id.as_str(),
                held,
                settlement.holding_balance_after()
            ))));
        }

        // ── Write it ───────────────────────────────────────────────────────
        // One transaction holds every payout, every commission, the sweep, every
        // order's new status, the market's new state, and the audit entry. A
        // failure at any point leaves the market exactly as it was, and a retry
        // recomputes the same thing from the same ledger.
        for transaction in &settlement.transactions {
            store.ledger().post(transaction).await?;
        }

        let mut winners = 0_usize;
        let mut losers = 0_usize;
        let mut events = Vec::new();

        for (order_id, decision) in &settlement.decisions {
            match decision {
                OrderSettlement::Paid {
                    returned_minor_units,
                    profit_minor_units,
                    ..
                } => {
                    winners += 1;
                    store.orders().settle(order_id).await?;
                    events.push(StreamEvent {
                        seq: 0,
                        kind: EventKind::OrderSettled {
                            order_id: order_id.as_str().to_string(),
                            market_id: market_id.clone(),
                            returned_minor_units: *returned_minor_units,
                            profit_minor_units: *profit_minor_units,
                        },
                        occurred_at_unix: deps.clock.now_unix(),
                    });
                }
                OrderSettlement::Lost => {
                    losers += 1;
                    store.orders().settle(order_id).await?;
                }
                // Unreachable, and deliberately not made reachable.
                //
                // `orders` came from `matched_orders`, so every order in
                // `settlement.decisions` is `Matched` and `domain` classified it
                // as `Paid` or `Lost`. This arm exists so that the refusal is
                // visible rather than silent: if a future adapter widens
                // `matched_orders` to include resting orders, `domain` will
                // hand back `NotMatched` decisions and settlement would try to
                // settle orders that hold nothing — inventing payouts.
                //
                // It errors instead of cancelling, because cancelling belongs to
                // `cancel_resting` below, which does it in bulk and reports what
                // it voided. An error is the loudest thing this code can do, and
                // it fails the whole transaction, which is the correct outcome.
                OrderSettlement::NotMatched => {
                    return Err(domain::error::DomainError::internal_with_context(
                        "settlement received an order that never matched",
                        format!(
                            "order {order_id} in market {} was classified NotMatched, which \
                             matched_orders should never return",
                            market_id.as_str()
                        ),
                    )
                    .into());
                }
            }
        }

        // Void whatever is still resting. Not a cleanup detail: an order left
        // `Pending` on a market that has `Settled` tells a customer their order
        // is still waiting to be matched, when in fact the market is closed and
        // never will be. That is a record that misinforms rather than one that is
        // merely untidy, so it is voided in the same transaction that closes the
        // market.
        //
        // Already-cancelled orders are untouched — the port updates the `pending`
        // set only — so an order a customer withdrew themselves is left exactly
        // as they left it.
        let voided = store.orders().cancel_resting(market_id).await?;
        for order in &voided {
            events.push(StreamEvent {
                seq: 0,
                kind: EventKind::OrderCancelled {
                    order_id: order.id.as_str().to_string(),
                    market_id: market_id.clone(),
                },
                occurred_at_unix: deps.clock.now_unix(),
            });
        }

        store
            .markets()
            .set_state(market_id, MarketState::Settled)
            .await?;

        // ── Read the hold back ─────────────────────────────────────────────
        // `settlement.holding_balance_after()` is arithmetic over the plan, and a
        // plan is what an adapter bug would corrupt. This is the balance the
        // ledger actually holds now that the payouts are written, and it is read
        // inside the transaction so a settlement is never committed with money
        // stranded and the check written down afterwards.
        let hold_balance_after = store.ledger().balance_minor(&holding).await?;
        if hold_balance_after != 0 {
            return Err(AppError::Store(StoreError::Backend(format!(
                "settlement of market {} would leave {hold_balance_after} minor units in \
                 the hold; the payouts posted do not account for everything that entered it",
                market_id.as_str()
            ))));
        }

        let summary = SettlementSummary {
            market_id: market_id.clone(),
            outcome,
            winners,
            losers,
            total_paid_minor: settlement.total_paid_minor_units(),
            total_commission_minor: settlement.total_commission_minor_units(),
            total_held_minor: held,
            hold_balance_after_minor: hold_balance_after,
        };

        store
            .audit()
            .append(crate::ports::NewAuditEntry {
                action: "market.settled".to_string(),
                subject: market_id.as_str().to_string(),
                actor: "system".to_string(),
                detail: serde_json::json!({
                    "outcome": format!("{outcome:?}"),
                    "winners": winners,
                    "losers": losers,
                    "total_held_minor": held,
                    "total_paid_minor": summary.total_paid_minor,
                    "total_commission_minor": summary.total_commission_minor,
                    "commission_bps": deps.commission_bps,
                    "reconciled": summary.reconciled(),
                })
                .to_string(),
            })
            .await?;
        // The market-level event joins the per-order ones in the same append, so a
        // subscriber reading the log sees the whole settlement as one contiguous
        // run rather than as a batch of order events followed by an unrelated
        // summary.
        events.push(StreamEvent {
            seq: 0,
            kind: EventKind::MarketSettled {
                market_id: market_id.clone(),
                total_paid_minor_units: summary.total_paid_minor,
                total_commission_minor_units: summary.total_commission_minor,
            },
            occurred_at_unix: deps.clock.now_unix(),
        });
        let staged = stage(store, events).await?;
        store.commit().await?;

        // ── Publish ────────────────────────────────────────────────────────
        // Past the commit, so nothing is announced for a settlement that rolled
        // back, and carrying the positions the log assigned.
        announce(deps, &staged).await;

        Ok(summary)
    }
}

/// Settle every market that has resolved but not yet been paid.
///
/// Exists because the gap between resolving and settling is a real state, and
/// relying on an operator to notice it is not a control. Returns how many markets
/// were settled and, separately, which ones failed and why — a sweeper that
/// stops at the first failure would leave the rest of the queue unprocessed
/// precisely when something is broken, and a sweeper that ignored failures would
/// report success while customers waited.
///
/// # Errors
///
/// Returns [`AppError::Store`] only if the *list* of pending markets cannot be
/// read. A failure to settle one market is collected into the returned report
/// rather than propagated, because the other markets are unaffected.
pub async fn sweep_unsettled(
    deps: &Deps,
    settle: &SettleMarket,
) -> Result<SweepReport, AppError> {
    let mut report = SweepReport::default();

    // Read the queue in its own short transaction. Holding one transaction open
    // across a whole sweep would pin a connection for as long as the slowest
    // market takes, and the queue is not changing underneath us in a way that
    // matters: each settlement re-checks the market's state under its own lock.
    let mut store = deps.store.begin().await?;
    let resolved = store.markets().list_by_state(MarketState::Resolved).await?;
    store.rollback().await?;
    report.considered = resolved.len();

    for market in resolved {
        match settle.execute(&market.market_id).await {
            Ok(summary) => {
                report.settled.push(summary);
            }
            Err(error) => {
                tracing::error!(
                    market_id = %market.market_id.as_str(),
                    error = %error,
                    "failed to settle a resolved market"
                );
                report.failed.push((market.market_id.clone(), error.to_string()));
            }
        }
    }
    Ok(report)
}

/// What a sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// How many resolved markets were found.
    pub considered: usize,
    /// The ones that were paid.
    pub settled: Vec<SettlementSummary>,
    /// The ones that were not, and why.
    pub failed: Vec<(MarketId, String)>,
}
