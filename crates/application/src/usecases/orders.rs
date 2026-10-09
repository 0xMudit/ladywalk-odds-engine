//! # Placing and cancelling orders
//!
//! The exchange's main path.
//!
//! ## Money moves when an order matches, not when it rests
//!
//! This is the single most consequential decision in the codebase, so it is
//! worth being explicit about what it rules out.
//!
//! A resting order takes no money. There is no "hold on placement", no
//! pre-authorisation, and therefore no reservation to release, no expiry sweep
//! for unclaimed holds, and no possibility of a customer's balance being
//! debited for a bet that never happened. An order that rests for six hours and
//! is cancelled has moved nothing, and there is no path by which it could.
//!
//! The cost of that choice is that affordability cannot be checked once, at
//! placement: the set of makers that can actually pay has to be worked out at
//! match time. That is the [`affordable_makers`] filter below, and it is why
//! matching filters the book rather than filling from its head.
//!
//! ## The order of operations
//!
//! 1. Claim the idempotency key, so a retry cannot place two orders.
//! 2. Lock the market row. Everything below is serialised per market.
//! 3. Check the kill switch and the market's state.
//! 4. Verify the quote: signature, expiry, and that it is bound to *this*
//!    market, selection, stake and account.
//! 5. Check the customer's risk profile and current exposure.
//! 6. Read the book, and match against the makers who can afford their side.
//! 7. Post one balanced holding transaction for the whole match.
//! 8. Record the fills, the audit entry, and the stored response.
//! 9. Stage the events, then commit.
//! 10. Publish what was staged.
//!
//! Steps 7 and 8 are the reason the transaction exists. An order marked matched
//! with no holding is a customer who has a bet at odds and no money at risk, and
//! an order with a holding and no match record is money nobody can reconcile.
//! They are written in the same unit of work or the system is in a state that no
//! later query can repair.
//!
//! Step 9 puts the events in that same unit of work, for the same reason: a
//! committed fill that nothing was told about is a price that moves without a
//! subscriber hearing it, and a subscriber that misses a broadcast cannot tell
//! the difference between a quiet market and a lost event.
//!
//! ## Why the quote is re-verified rather than trusted
//!
//! The customer could edit the odds in the request. The signature is what makes
//! the price the server's own, and re-checking it here — rather than only when it
//! was issued — is what stops a captured-and-replayed request from being honoured
//! at a price that has since moved, or for a stake the customer never intended.

use std::sync::Arc;

use domain::ids::{AccountId, OrderId};
use domain::matching::{MatchedPair, match_order};
use domain::order::{Order, OrderRequest};
use domain::quote::Quote;
use domain::risk::RiskChecker;

use crate::errors::AppError;
use crate::events::{EventKind, StreamEvent};
use crate::ports::{
    Actor, IdGenerator, IdempotencyOutcome, OrderSummary, PlaceOrderRequest, QuoteSigner, Store,
    StoreError, StoredResponse, with_retry,
};
use crate::usecases::Deps;
use crate::usecases::{announce, stage};

/// Places an order, matching it against the book.
///
/// See the module documentation for the sequence and for why each step is where
/// it is.
#[derive(Debug, Clone)]
pub struct PlaceOrder {
    deps: Arc<Deps>,
}

impl PlaceOrder {
    /// A use case that places orders using `deps`.
    #[must_use]
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Place `request`, or replay the answer to a request that already placed it.
    ///
    /// # Errors
    ///
    /// - [`AppError::Domain`] with `StaleQuote` if the quote is expired,
    ///   unsigned, forged, or bound to a different market, selection, stake or
    ///   account.
    /// - [`AppError::Domain`] with `BettingHalted` or `MarketClosed` if the
    ///   platform or the market is not accepting orders.
    /// - [`AppError::Domain`] with `ExposureLimitExceeded` if the stake would
    ///   put the customer over their limit.
    /// - [`AppError::Domain`] with `InsufficientFunds` if the customer cannot
    ///   pay for any part of the stake. Note that this is decided against the
    ///   *fillable* stake, not the requested one: a customer with 500 who asks to
    ///   stake 1000 against a book that can only fill 400 is not overdrawing.
    /// - [`AppError::IdempotencyKeyReused`] if the key was used for a different
    ///   request, which is a client bug worth surfacing rather than replaying.
    /// - [`AppError::RequestInProgress`] if the same request is still running.
    /// - [`AppError::Store`] on any storage failure.
    pub async fn execute(&self, request: PlaceOrderRequest) -> Result<OrderSummary, AppError> {
        let deps = Arc::clone(&self.deps);
        let factory = Arc::clone(&deps.store);
        let outcome = with_retry(factory, |factory| {
            let request = request.clone();
            let deps = Arc::clone(&deps);
            async move {
                let mut store = factory.begin().await?;
                Self::attempt(&deps, &mut store, &request).await
            }
        })
        .await?;

        // Past the retry loop, so this is after a commit that actually happened.
        // `announce` logs rather than fails: the events are already durable, and a
        // subscriber that was not listening will read them from the log.
        announce(&deps, &outcome.events).await;

        Ok(outcome.summary)
    }

    /// One attempt, inside its own unit of work.
    async fn attempt(
        deps: &Deps,
        store: &mut Box<dyn Store>,
        request: &PlaceOrderRequest,
    ) -> Result<AttemptOutcome, AppError> {
        // ── 1. Idempotency ──────────────────────────────────────────────────
        // Claimed first, so a retry that arrives while the original is still
        // running is told so rather than racing it into a second order.
        let claim = store
            .idempotency()
            .claim(&request.idempotency_key, &actor_for(request), "POST /v1/orders")
            .await?;
        match claim {
            // The stored body is returned byte for byte rather than recomputed,
            // so a retry sees the same answer the first caller did. Parsing it
            // back into a summary is what lets the caller above this layer be
            // typed rather than handed a string.
            IdempotencyOutcome::Replay(response) => {
                let summary = serde_json::from_str(&response.body).map_err(|e| {
                    AppError::Domain(domain::error::DomainError::internal_with_context(
                        "a stored idempotent response could not be read back",
                        e,
                    ))
                })?;
                return Ok(AttemptOutcome {
                    summary,
                    events: Vec::new(),
                });
            }
            IdempotencyOutcome::InProgress => return Err(AppError::RequestInProgress),
            IdempotencyOutcome::Reused {
                original_account_id,
                original_endpoint,
            } => {
                // Logged, and reported as a distinct code. A client that reuses a
                // key for a different request is entitled to be told so: silently
                // returning the first order's summary would tell it its new order
                // was placed when nothing was.
                tracing::warn!(
                    idempotency_key = %request.idempotency_key.as_str(),
                    attempted_by = %actor_for(request).as_str(),
                    original_owner = %original_account_id.as_str(),
                    original_endpoint,
                    "idempotency key reused by a different customer or operation"
                );
                return Err(AppError::IdempotencyKeyReused {
                    key: request.idempotency_key.as_str().to_string(),
                });
            }
            IdempotencyOutcome::Claimed => {}
        }

        let (summary, outcome_events) = Self::place(deps, store, request).await?;

        // ── 8. Store the answer, then commit ────────────────────────────────
        // The response is written inside the same transaction as the order. If
        // the commit fails, both are gone; if it succeeds, a retry reads back a
        // response that is guaranteed to describe committed state.
        let response = StoredResponse::new(201, serde_json::to_string(&summary).map_err(|e| {
            AppError::Domain(domain::error::DomainError::internal_with_context(
                "could not serialise the order summary",
                e,
            ))
        })?);
        store
            .idempotency()
            .remember(
                &request.idempotency_key,
                &actor_for(request),
                "POST /v1/orders",
                &response,
            )
            .await?;

        // Step 9 of the sequence in the module documentation, and the only step
        // that has to happen *before* the commit: the events are written in this
        // transaction so that an order cannot exist without the market having been
        // told about it, and they come back with the positions the database gave
        // them so that `execute` publishes real sequence numbers.
        let staged = stage(store, outcome_events).await?;

        store.commit().await?;

        Ok(AttemptOutcome {
            summary,
            events: staged,
        })
    }

    /// The part of placing an order that is about the exchange's rules.
    ///
    /// Returns the summary and the events, which the caller stages and then
    /// publishes once it has committed. The events are built here but not sent,
    /// because publishing is only correct after the commit that made them true.
    async fn place(
        deps: &Deps,
        store: &mut Box<dyn Store>,
        request: &PlaceOrderRequest,
    ) -> Result<(OrderSummary, Vec<StreamEvent>), AppError> {
        // ── 2 & 3. Lock the market, then check that trading is permitted ────
        crate::usecases::ensure_trading_permitted(store, &request.market_id).await?;
        let market = store
            .markets()
            .lock(&request.market_id)
            .await?;
        let published_odds = market.published_odds.ok_or(AppError::Domain(
            domain::error::DomainError::MarketClosed,
        ))?;

        if market.currency != request.stake.currency {
            return Err(domain::error::DomainError::InvalidInput {
                field: "currency",
                reason: format!(
                    "this market trades in {} and the stake is in {}",
                    market.currency, request.stake.currency
                ),
            }
            .into());
        }

        // ── 4. Re-verify the quote ─────────────────────────────────────────
        Self::verify_quote(deps, &request.quote, request, published_odds.raw()).await?;

        // ── 5. Risk ────────────────────────────────────────────────────────
        let profile = store
            .risk()
            .profile(&Actor::customer(request.account_id.clone()).account_id)
            .await?;
        let exposure = store
            .risk()
            .exposure_minor(&Actor::customer(request.account_id.clone()).account_id)
            .await?;
        let checker = RiskChecker {
            kill_switch: store.risk().kill_switch().await?,
            market_open: true,
        };
        checker.check(profile, exposure, request.stake)?;

        // ── 6. Match ───────────────────────────────────────────────────────
        let order = Order::pending(
            order_id_for(deps)?,
            request.account_id.clone(),
            &OrderRequest::new(
                request.market_id.clone(),
                request.selection,
                request.stake,
            )?,
        );
        let mut quoted = order;
        quoted.apply_quote(published_odds)?;

        let book = store.orders().pending_book(&request.market_id).await?;
        let book = Self::affordable_makers(store, book, &request.account_id).await?;
        let holding_ref = deps.transaction_ref(&format!("hold:{}", request.market_id.as_str()))?;

        let outcome = match_order(&quoted, published_odds, &book, &holding_ref)?;
        let filled_minor = outcome.taker_filled_minor_units;

        // ── 7. Post the holding, record the fills ───────────────────────────
        if let Some(transaction) = &outcome.holding {
            store.ledger().post(transaction).await?;
        }
        store.orders().insert(&quoted).await?;

        let mut events = vec![StreamEvent {
            seq: 0,
            kind: EventKind::OrderPlaced {
                order_id: quoted.id.as_str().to_string(),
                market_id: quoted.market_id.clone(),
                selection: quoted.selection,
                stake_minor_units: quoted.stake_minor_units,
                currency: market.currency.into(),
            },
            occurred_at_unix: deps.clock.now_unix(),
        }];

        for pair in &outcome.pairs {
            store
                .orders()
                .mark_matched(&pair.taker_id, &pair.holding, pair.stake_minor_units)
                .await?;
            events.push(fill_event(deps, &quoted, pair, true));
        }
        for pair in &outcome.pairs {
            store
                .orders()
                .mark_matched(&pair.maker_id, &pair.holding, pair.stake_minor_units)
                .await?;
            events.push(fill_event(deps, &quoted, pair, false));
        }

        if filled_minor > 0 {
            let holding = outcome
                .holding
                .as_ref()
                .map(|t| t.transaction_ref.clone())
                .ok_or_else(|| {
                    AppError::Domain(domain::error::DomainError::internal(
                        "a match was reported with no holding transaction",
                    ))
                })?;
            store
                .orders()
                .mark_matched(&quoted.id, &holding, filled_minor)
                .await?;
        } else {
            // Unfilled, so the order rests with no money taken. It is cancelled
            // rather than left pending: a resting order on a fixed-odds book
            // takes no hold, so there is nothing to release, and a `Pending` order
            // on a one-price market would sit there looking tradeable until the
            // market resolved. The cancellation is what tells the customer their
            // stake was not taken, and the ledger says the same thing by having
            // no entry for it.
            store.orders().cancel(&quoted.id).await?;
        }

        let balance = store
            .accounts()
            .balance_minor(&request.account_id, market.currency)
            .await?;

        store
            .audit()
            .append(crate::ports::NewAuditEntry {
                action: "order.placed".to_string(),
                subject: quoted.id.as_str().to_string(),
                actor: request.account_id.as_str().to_string(),
                detail: serde_json::json!({
                    "market_id": request.market_id.as_str(),
                    "selection": request.selection.to_string(),
                    "stake_minor_units": request.stake.minor_units,
                    "filled_minor_units": filled_minor,
                    "counterparties": outcome.depth(),
                    "odds_minor_units": published_odds.raw(),
                    "balance_after_minor": balance,
                })
                .to_string(),
            })
            .await?;

        Ok((
            OrderSummary {
                order_id: quoted.id.clone(),
                odds: published_odds,
                matched: outcome.is_matched(),
                fills: outcome.depth(),
                filled_minor_units: filled_minor,
                stake_minor_units: request.stake.minor_units,
                balance_minor: balance,
            },
            events,
        ))
    }

    /// Drop makers who cannot pay their side of a fill.
    ///
    /// A maker is only filtered out if it could never pay for *any* part of its
    /// own stake. The test is deliberately `balance >= 1` rather than
    /// `balance >= stake`: a maker with 400 who offered 1000 is still worth
    /// filling for 400, and excluding it would be the exchange throwing away a
    /// real match because a customer could not honour a number they never
    /// reached.
    ///
    /// This is a *pre-filter*, not a guarantee. It removes makers who are
    /// certainly unaffordable so that a fill attempt is not made against them;
    /// the authoritative check is the `account_balances` constraint on the
    /// insert, which refuses a debit that would go negative even if this filter
    /// were stale or a balance changed underneath it. Two mechanisms, because a
    /// pre-filter that is wrong in the *permissive* direction is caught by the
    /// constraint and one that is wrong in the *restrictive* direction silently
    /// costs customers their matches.
    async fn affordable_makers(
        store: &mut Box<dyn Store>,
        book: Vec<Order>,
        taker: &AccountId,
    ) -> Result<Vec<Order>, AppError> {
        let mut affordable = Vec::with_capacity(book.len());
        for maker in book {
            // Self-matching is already refused by `is_matchable_by`, but checking
            // here too means a book that somehow contains the taker cannot make
            // them pay themselves.
            if maker.account_id == *taker {
                continue;
            }
            let currency = maker.stake()?.currency;
            let balance = store.accounts().balance_minor(&maker.account_id, currency).await?;
            if balance > 0 {
                affordable.push(maker);
            } else {
                tracing::info!(
                    order_id = %maker.id.as_str(),
                    account_id = %maker.account_id.as_str(),
                    balance_minor = balance,
                    "skipping a maker who cannot fund a fill"
                );
            }
        }
        Ok(affordable)
    }

    /// Re-verify the quote against everything it claims to be about.
    async fn verify_quote(
        deps: &Deps,
        quote: &Quote,
        request: &PlaceOrderRequest,
        published_odds_raw: i64,
    ) -> Result<(), AppError> {
        // The signature first: everything after it trusts the payload, and
        // without it a customer could quote a price the server never offered.
        if !deps
            .signer
            .verify(&quote.signing_input(), &quote.signature)
            .await?
        {
            return Err(AppError::Domain(domain::error::DomainError::StaleQuote));
        }

        let now = deps.clock.now_unix();
        if quote.expires_at_unix <= now {
            return Err(AppError::Domain(domain::error::DomainError::StaleQuote));
        }

        // The four bindings. Each is checked separately rather than by
        // comparing a whole struct, so that a failure names which one it was:
        // a customer whose tab was open for two minutes and whose stake was
        // edited should be told the price moved, not that their account is
        // wrong.
        if quote.market_id != request.market_id {
            return Err(domain::error::DomainError::InvalidInput {
                field: "quote",
                reason: "the quote was issued for a different market".to_string(),
            }
            .into());
        }
        if quote.selection != request.selection {
            return Err(domain::error::DomainError::InvalidInput {
                field: "quote",
                reason: "the quote was issued for a different selection".to_string(),
            }
            .into());
        }
        if quote.stake_minor_units != request.stake.minor_units
            || quote.currency_code != request.stake.currency.to_string()
        {
            return Err(domain::error::DomainError::StaleQuote.into());
        }
        if quote.account_id != request.account_id {
            return Err(domain::error::DomainError::InvalidInput {
                field: "quote",
                reason: "the quote was issued to a different customer".to_string(),
            }
            .into());
        }
        // The price must also still be the price the market is publishing. A
        // valid, unexpired signature over a stale price is still a stale price.
        if quote.odds.raw() != published_odds_raw {
            return Err(AppError::Domain(domain::error::DomainError::StaleQuote));
        }
        Ok(())
    }
}

/// Cancels an order that has not matched.
#[derive(Debug, Clone)]
pub struct CancelOrder {
    deps: Arc<Deps>,
}

impl CancelOrder {
    /// A use case that cancels orders using `deps`.
    #[must_use]
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Cancel `order_id` on behalf of `actor`.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the order does not exist.
    /// - [`AppError::Forbidden`] if the order belongs to someone else. Checked
    ///   here rather than by the route, because a customer reading another
    ///   customer's order is an authorization failure and the route does not
    ///   know whose order it is.
    /// - [`AppError::Domain`] with `InvalidInput` if the order has already
    ///   matched. A matched order's funds are held against a counterparty, so
    ///   withdrawing it is a settlement operation and not a cancellation.
    pub async fn execute(
        &self,
        actor: &Actor,
        order_id: &OrderId,
    ) -> Result<(), AppError> {
        let deps = Arc::clone(&self.deps);
        let factory = Arc::clone(&deps.store);
        let order_id = order_id.clone();
        let staged = with_retry(factory, |factory| {
            let deps = Arc::clone(&deps);
            let order_id = order_id.clone();
            let actor = actor.clone();
            async move {
                let mut store = factory.begin().await?;
                let order = store
                    .orders()
                    .find(&order_id)
                    .await?
                    .ok_or(AppError::NotFound {
                        resource_type: "order",
                        resource_id: order_id.as_str().to_string(),
                    })?;

                if order.account_id != actor.account_id && !actor.is_operator() {
                    return Err(AppError::Forbidden { action: "cancel this order" });
                }

                // `cancel` loads the order and calls `Order::mark_cancelled`,
                // which refuses anything already matched. A matched order's funds
                // are held against a counterparty, so withdrawing it is a
                // settlement operation and not a cancellation, and the refusal
                // comes from `domain` rather than from a check here that a
                // future use case could forget.
                store.orders().cancel(&order_id).await?;

                store
                    .audit()
                    .append(crate::ports::NewAuditEntry {
                        action: "order.cancelled".to_string(),
                        subject: order.id.as_str().to_string(),
                        actor: actor.account_id.as_str().to_string(),
                        detail: serde_json::json!({
                            "market_id": order.market_id.as_str(),
                            "stake_minor_units": order.stake_minor_units,
                        })
                        .to_string(),
                    })
                    .await?;
                let staged = stage(
                    &mut store,
                    vec![StreamEvent {
                        seq: 0,
                        kind: EventKind::OrderCancelled {
                            order_id: order.id.as_str().to_string(),
                            market_id: order.market_id.clone(),
                        },
                        occurred_at_unix: deps.clock.now_unix(),
                    }],
                )
                .await?;
                store.commit().await?;

                Ok(staged)
            }
        })
        .await?;

        announce(&deps, &staged).await;
        Ok(())
    }
}

/// What one attempt produced.
struct AttemptOutcome {
    /// The order summary to return, or to replay.
    summary: OrderSummary,
    /// The events, staged inside the transaction and to be published now that it
    /// has committed.
    events: Vec<StreamEvent>,
}

/// The account a request is made on behalf of.
///
/// `PlaceOrderRequest` carries no actor, because an order is always placed for
/// its own account; authorisation of *who may place for whom* is the caller's
/// job and is not a thing the order itself should be able to express.
fn actor_for(request: &PlaceOrderRequest) -> AccountId {
    request.account_id.clone()
}

/// The event for one side of a fill.
fn fill_event(deps: &Deps, taker: &Order, pair: &MatchedPair, is_taker: bool) -> StreamEvent {
    let (order_id, counterparty) = if is_taker {
        (&pair.taker_id, &pair.maker_id)
    } else {
        (&pair.maker_id, &pair.taker_id)
    };
    StreamEvent {
        seq: 0,
        kind: EventKind::OrderFilled {
            order_id: order_id.as_str().to_string(),
            market_id: taker.market_id.clone(),
            counterparty_order_id: counterparty.as_str().to_string(),
            stake_minor_units: pair.stake_minor_units,
        },
        occurred_at_unix: deps.clock.now_unix(),
    }
}

/// A new order id.
///
/// # Errors
///
/// Propagates [`StoreError::Backend`] if no valid id can be generated.
fn order_id_for(deps: &Deps) -> Result<OrderId, AppError> {
    Ok(deps.with_ids(|generator| generator.new_order_id()))
}

/// Whether a quote is still valid at `now_unix`.
///
/// Exposed so that the HTTP layer can reject an expired quote at the edge with a
/// message that says so, rather than letting it reach the money path and come
/// back as a generic failure the customer cannot act on.
#[must_use]
pub fn quote_is_current(quote: &Quote, now_unix: i64) -> bool {
    quote.expires_at_unix > now_unix
}

/// Whether this error means the caller should be told their price moved.
///
/// The HTTP layer uses it to choose between "ask again" and "we have a problem",
/// which is the only distinction a customer can act on.
#[must_use]
pub fn is_stale_price(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Domain(domain::error::DomainError::StaleQuote)
    )
}

/// Whether a store error is a lost race rather than an outage.
#[must_use]
pub fn is_contention(error: &StoreError) -> bool {
    error.is_retryable()
}

/// A signer, named so the type is used in this module's signatures.
pub type OrderSigner = Arc<dyn QuoteSigner>;
