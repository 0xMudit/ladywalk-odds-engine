//! # Market administration
//!
//! The operations an operator performs: opening, suspending, resuming and
//! resolving a market.
//!
//! ## Why these are one module
//!
//! All four are the same shape — an operator, a market, a state change, an audit
//! entry — and splitting them would duplicate that shape four times with four
//! slightly different mistakes. What they share is more important than what
//! differs: **each one locks the market row before changing it.** An operator
//! suspending a market must be serialised against a customer placing an order on
//! it, and the only way to guarantee that is for both to take the same lock.
//!
//! ## Suspension is not cancellation
//!
//! Suspending a market stops new orders. It does not touch existing ones, and it
//! does not return anybody's money: a resting order holds nothing, so there is
//! nothing to return, and a matched order's funds are held against a
//! counterparty in a way that releasing them would be inventing an outcome. A
//! market that is suspended and later resumed finds its book exactly as it was
//! left, which is the property that makes suspension a safe thing to do in a
//! hurry.

use std::sync::Arc;

use domain::ids::MarketId;
use domain::odds::Odds;
use domain::settlement::Outcome;

use crate::errors::AppError;
use crate::events::{EventKind, StreamEvent};
use crate::ports::{Actor, MarketState, with_retry};
use crate::usecases::Deps;
use crate::usecases::{announce, stage};

/// Requires an operator, and returns the actor if they are one.
///
/// # Errors
///
/// Returns [`AppError::Forbidden`] for a customer. Checked in every
/// administrative use case rather than once in a middleware, because the set of
/// operator-only operations is the thing most likely to grow and a route guard
/// is the wrong place to enumerate it.
fn require_operator(actor: &Actor, action: &'static str) -> Result<(), AppError> {
    if actor.is_operator() {
        Ok(())
    } else {
        Err(AppError::Forbidden { action })
    }
}

/// Suspends a market, so it stops accepting orders.
#[derive(Debug, Clone)]
pub struct SuspendMarket {
    deps: Arc<Deps>,
}

impl SuspendMarket {
    /// A use case that suspends markets using `deps`.
    #[must_use]
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Suspend `market_id`, recording who did it and why.
    ///
    /// `reason` is required rather than optional. A suspension with no stated
    /// reason is one that cannot be explained to a customer who asks why their
    /// bet is not being accepted, and the reason is the first thing an operator
    /// needs at 3am.
    ///
    /// # Errors
    ///
    /// - [`AppError::Forbidden`] if `actor` is not an operator.
    /// - [`AppError::NotFound`] if the market does not exist.
    /// - [`AppError::Domain`] with `InvalidInput` if the market is already
    ///   suspended or has already resolved — suspending a settled market is a
    ///   no-op that would misreport the state.
    pub async fn execute(
        &self,
        actor: &Actor,
        market_id: &MarketId,
        reason: &str,
    ) -> Result<(), AppError> {
        require_operator(actor, "suspend a market")?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(domain::error::DomainError::InvalidInput {
                field: "reason",
                reason: "a suspension must state a reason".to_string(),
            }
            .into());
        }
        if reason.len() > MAX_REASON_LEN {
            return Err(domain::error::DomainError::InvalidInput {
                field: "reason",
                reason: format!("a reason may be at most {MAX_REASON_LEN} characters"),
            }
            .into());
        }

        let deps = Arc::clone(&self.deps);
        let factory = Arc::clone(&deps.store);
        let market_id = market_id.clone();
        let reason = reason.to_string();
        let actor = actor.clone();

        with_retry(factory, |factory| {
            let deps = Arc::clone(&deps);
            let market_id = market_id.clone();
            let reason = reason.clone();
            let actor = actor.clone();
            async move {
                let mut store = factory.begin().await?;
                let market = store.markets().lock(&market_id).await?;

                if market.state == MarketState::Suspended {
                    return Err(domain::error::DomainError::InvalidInput {
                        field: "market",
                        reason: "this market is already suspended".to_string(),
                    }
                    .into());
                }
                if !market.state.accepts_orders() {
                    return Err(domain::error::DomainError::InvalidInput {
                        field: "market",
                        reason: format!(
                            "a market that is {:?} cannot be suspended",
                            market.state
                        ),
                    }
                    .into());
                }

                store
                    .markets()
                    .set_state(&market_id, MarketState::Suspended)
                    .await?;
                store
                    .audit()
                    .append(crate::ports::NewAuditEntry {
                        action: "market.suspended".to_string(),
                        subject: market_id.as_str().to_string(),
                        actor: actor.account_id.as_str().to_string(),
                        detail: serde_json::json!({ "reason": reason }).to_string(),
                    })
                    .await?;
                let staged = stage(
                    &mut store,
                    vec![StreamEvent {
                        seq: 0,
                        kind: EventKind::MarketSuspended {
                            market_id: market_id.clone(),
                        },
                        occurred_at_unix: deps.clock.now_unix(),
                    }],
                )
                .await?;
                store.commit().await?;

                announce(&deps, &staged).await;
                Ok(())
            }
        })
        .await
    }
}

/// Resolves a market, declaring its outcome.
///
/// # Errors
///
/// - [`AppError::Forbidden`] if the actor is not an operator.
/// - [`AppError::NotFound`] if the market does not exist.
/// - [`AppError::Domain`] with `InvalidInput` if the market is already resolved
///   or settled. Resolving twice is refused rather than treated as a re-affirmation
///   because the first resolution may already have been paid out, and a second
///   one for a different outcome would be money moving twice for one event.
pub async fn resolve_market(
    deps: &Deps,
    actor: &Actor,
    market_id: &MarketId,
    outcome: Outcome,
) -> Result<(), AppError> {
    require_operator(actor, "resolve a market")?;
    let deps_owned = Arc::new(deps.clone());
    let factory = Arc::clone(&deps_owned.store);
    let market_id = market_id.clone();
    let actor = actor.clone();

    with_retry(factory, |factory| {
        let deps = Arc::clone(&deps_owned);
        let market_id = market_id.clone();
        let actor = actor.clone();
        async move {
            let mut store = factory.begin().await?;
            let market = store.markets().lock(&market_id).await?;

            if !matches!(market.state, MarketState::Open | MarketState::Suspended) {
                return Err(domain::error::DomainError::InvalidInput {
                    field: "market",
                    reason: format!(
                        "a market that is {:?} cannot be resolved",
                        market.state
                    ),
                }
                .into());
            }

            // The outcome and the state move together, in one call. Recording the
            // state without the outcome would leave a market that is `Resolved`
            // and unpayable, which settlement reports as an internal error rather
            // than as the customer not being owed anything.
            store
                .markets()
                .resolve(&market_id, outcome)
                .await?;
            store
                .audit()
                .append(crate::ports::NewAuditEntry {
                    action: "market.resolved".to_string(),
                    subject: market_id.as_str().to_string(),
                    actor: actor.account_id.as_str().to_string(),
                    detail: serde_json::json!({ "outcome": format!("{outcome:?}") }).to_string(),
                })
                .await?;
            let staged = stage(
                &mut store,
                vec![StreamEvent {
                    seq: 0,
                    kind: EventKind::MarketResolved {
                        market_id: market_id.clone(),
                        outcome: outcome.into(),
                    },
                    occurred_at_unix: deps.clock.now_unix(),
                }],
            )
            .await?;
            store.commit().await?;

            announce(&deps, &staged).await;
            Ok(())
        }
    })
    .await
}

/// Reopens a suspended market at a new price.
///
/// # Errors
///
/// - [`AppError::Forbidden`] if the actor is not an operator.
/// - [`AppError::NotFound`] if the market does not exist.
/// - [`AppError::Domain`] with `InvalidInput` if the market is not suspended.
pub async fn resume_market(
    deps: &Deps,
    actor: &Actor,
    market_id: &MarketId,
    odds: Odds,
) -> Result<(), AppError> {
    require_operator(actor, "resume a market")?;
    let deps_owned = Arc::new(deps.clone());
    let factory = Arc::clone(&deps_owned.store);
    let market_id = market_id.clone();
    let actor = actor.clone();

    with_retry(factory, |factory| {
        let deps = Arc::clone(&deps_owned);
        let market_id = market_id.clone();
        let actor = actor.clone();
        async move {
            let mut store = factory.begin().await?;
            let market = store.markets().lock(&market_id).await?;

            if market.state != MarketState::Suspended {
                return Err(domain::error::DomainError::InvalidInput {
                    field: "market",
                    reason: format!("a market that is {:?} is not suspended", market.state),
                }
                .into());
            }

            // The price is required on resume rather than restored, because a
            // market that was suspended precisely because its price was wrong
            // must not come back at it. Requiring the operator to state the new
            // price is the point at which they are forced to decide it.
            store.markets().publish_odds(&market_id, odds).await?;
            store
                .markets()
                .set_state(&market_id, MarketState::Open)
                .await?;
            store
                .audit()
                .append(crate::ports::NewAuditEntry {
                    action: "market.resumed".to_string(),
                    subject: market_id.as_str().to_string(),
                    actor: actor.account_id.as_str().to_string(),
                    detail: serde_json::json!({ "odds_minor_units": odds.raw() }).to_string(),
                })
                .await?;
            let staged = stage(
                &mut store,
                vec![StreamEvent {
                    seq: 0,
                    kind: EventKind::MarketResumed {
                        market_id: market_id.clone(),
                        odds_minor_units: odds.raw(),
                    },
                    occurred_at_unix: deps.clock.now_unix(),
                }],
            )
            .await?;
            store.commit().await?;

            announce(&deps, &staged).await;
            Ok(())
        }
    })
    .await
}

/// Halts or resumes money movement platform-wide.
///
/// Wrapped in [`with_retry`] even though it touches no market row, because it
/// writes the same `platform_settings` row every other use case *reads* while
/// holding a lock. An operator throwing the switch during a burst of order
/// placement is exactly when that row is most contended, and a switch that fails
/// with a serialization error at the moment it is most needed is the worst
/// possible time for it to.
///
/// # Errors
///
/// - [`AppError::Forbidden`] if the actor is not an operator.
/// - [`AppError::Domain`] with `InvalidInput` if the reason is blank.
pub async fn set_kill_switch(
    deps: &Deps,
    actor: &Actor,
    running: bool,
    reason: &str,
) -> Result<(), AppError> {
    require_operator(actor, "change the kill switch")?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(domain::error::DomainError::InvalidInput {
            field: "reason",
            reason: "changing the kill switch must state a reason".to_string(),
        }
        .into());
    }
    if reason.len() > MAX_REASON_LEN {
        return Err(domain::error::DomainError::InvalidInput {
            field: "reason",
            reason: format!("a reason may be at most {MAX_REASON_LEN} characters"),
        }
        .into());
    }

    let state = if running {
        domain::risk::KillSwitchState::Running
    } else {
        domain::risk::KillSwitchState::Halted
    };
    let reason = reason.to_string();
    let actor = actor.clone();
    let factory = Arc::clone(&deps.store);

    with_retry(factory, |factory| {
        let deps = deps.clone();
        let reason = reason.clone();
        let actor = actor.clone();
        async move {
            let mut store = factory.begin().await?;
            store
                .risk()
                .set_kill_switch(state, &actor, &reason)
                .await?;
            // Recorded here rather than inside `set_kill_switch` because the port
            // only promises to write the setting; who threw it is a separate fact
            // with a separate reader, and a port that wrote it internally could not
            // be implemented by an adapter whose audit table is in the same
            // transaction as everything else.
            store
                .audit()
                .append(crate::ports::NewAuditEntry {
                    action: "platform.kill_switch_changed".to_string(),
                    subject: "platform".to_string(),
                    actor: actor.account_id.as_str().to_string(),
                    detail: serde_json::json!({ "running": running, "reason": reason })
                        .to_string(),
                })
                .await?;
            let staged = stage(
                &mut store,
                vec![StreamEvent {
                    seq: 0,
                    kind: EventKind::KillSwitchChanged { running },
                    occurred_at_unix: deps.clock.now_unix(),
                }],
            )
            .await?;
            store.commit().await?;

            announce(&deps, &staged).await;
            Ok(())
        }
    })
    .await
}

/// The longest reason an operator may record, in bytes.
///
/// Bounded because it is written into the hash-chained audit trail, and an
/// unbounded field in a chain is a field whose earlier hashes stop being
/// computable if the storage policy ever truncates it.
const MAX_REASON_LEN: usize = 512;
