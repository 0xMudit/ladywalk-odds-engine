//! # Quoting a market
//!
//! Turning "how much would I win if I backed this?" into a signed, expiring
//! price.
//!
//! ## Why this is a use case and not a lookup
//!
//! A quote is not a field on a market. It is a statement about one customer, one
//! stake and one moment, signed by the server so that the customer can prove
//! later that they were offered it. That makes it four things at once — a read
//! of the market, a decision about whether this customer may be quoted at all, a
//! signature, and a record that the price was published.
//!
//! The record matters more than it looks. A price that was offered and never
//! recorded cannot be defended against a complaint that the customer was
//! misled, and the alternative — reconstructing what a price *was* from the
//! orders placed at it — fails the moment the market's price moves, which is
//! exactly when somebody asks.

use std::sync::Arc;

use domain::ids::{AccountId, MarketId, QuoteId};
use domain::money::MoneyAmount;
use domain::order::Selection;
use domain::quote::Quote;

use crate::errors::AppError;
use crate::events::{EventKind, StreamEvent};
use crate::ports::{IdGenerator, Store, with_retry};
use crate::usecases::Deps;
use crate::usecases::{announce, stage};

/// What a customer asked to be quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteRequest {
    /// Who is asking.
    pub account_id: AccountId,
    /// Which market.
    pub market_id: MarketId,
    /// Which selection they want to back.
    pub selection: Selection,
    /// How much they intend to stake, which the quote is bound to.
    pub stake: MoneyAmount,
}

/// A signed price, and the id it was stored under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotedPrice {
    /// The stored id of this quote.
    pub quote_id: QuoteId,
    /// The signed, expiring price.
    pub quote: Quote,
}

/// Issues a signed price for a market.
#[derive(Debug, Clone)]
pub struct QuoteMarket {
    deps: Arc<Deps>,
}

impl QuoteMarket {
    /// A use case that quotes from `deps`.
    #[must_use]
    pub fn new(deps: Arc<Deps>) -> Self {
        Self { deps }
    }

    /// Quote `request`, and record that the price was published.
    ///
    /// # Errors
    ///
    /// - [`AppError::NotFound`] if the market does not exist.
    /// - [`AppError::Domain`] with `MarketClosed` if the market is not open, or
    ///   with `InvalidInput` if the stake is not positive. A customer is not
    ///   quoted a price for a market that is suspended, because a quote is a
    ///   promise and a promise about a closed market is one that cannot be kept.
    /// - [`AppError::Domain`] with `BettingHalted` if the kill switch is thrown:
    ///   quoting is the first step of trading, and continuing to offer prices
    ///   while money movement is halted is how a halted platform still looks
    ///   open to its customers.
    /// - [`AppError::Domain`] with `StaleQuote` if the market has no published
    ///   price, which is the same failure as an unsigned price and is reported
    ///   identically.
    /// - [`AppError::Store`] if the quote cannot be stored.
    pub async fn execute(
        &self,
        request: QuoteRequest,
    ) -> Result<QuotedPrice, AppError> {
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

        // Published after the commit, so a subscriber never sees a price that
        // was rolled back. A failure here loses a live notification, not a
        // record: the quote is already in the database.
        announce(&deps, &outcome.events).await;

        Ok(outcome.price)
    }

    /// One attempt, inside its own unit of work.
    async fn attempt(
        deps: &Deps,
        store: &mut Box<dyn Store>,
        request: &QuoteRequest,
    ) -> Result<AttemptOutcome, AppError> {
        if !request.stake.is_positive() {
            return Err(domain::error::DomainError::InvalidInput {
                field: "stake",
                reason: "a quote must be for a positive stake".to_string(),
            }
            .into());
        }

        // Both checks, in the same order as every other use case, and inside the
        // transaction — so a market suspended between the customer's page load
        // and this request is not quoted.
        crate::usecases::ensure_trading_permitted(store, &request.market_id).await?;

        let market = store
            .markets()
            .find(&request.market_id)
            .await?
            .ok_or(AppError::NotFound {
                resource_type: "market",
                resource_id: request.market_id.as_str().to_string(),
            })?;

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

        // A market with no published price cannot be quoted, and a zero price
        // would be worse than no answer: it looks like a 100% certain outcome.
        let odds = market.published_odds.ok_or_else(|| {
            AppError::Domain(domain::error::DomainError::MarketClosed)
        })?;

        let now = deps.clock.now_unix();
        let mut quote = Quote::unsigned(
            request.market_id.clone(),
            request.selection,
            odds,
            request.stake,
            request.account_id.clone(),
            now,
            deps.quote_ttl_secs,
        )?;
        let signature = deps.signer.sign(&quote.signing_input()).await?;
        quote.signature = signature;

        let quote_id = deps.with_ids(|generator| generator.new_quote_id())?;
        store.quotes().insert(&quote_id, &quote).await?;

        // The sequence number is assigned by the log's insert, so it starts at
        // zero here and is replaced by whatever `stage` returns below. Carrying it
        // on the struct rather than returning it separately keeps the publish call
        // identical for every use case.
        let event = StreamEvent {
            seq: 0,
            kind: EventKind::QuoteIssued {
                quote_id: quote_id.as_str().to_string(),
                market_id: request.market_id.clone(),
                selection: request.selection,
                stake_minor_units: request.stake.minor_units,
                currency: request.stake.currency.into(),
            },
            occurred_at_unix: now,
        };

        store.audit().append(
            crate::ports::NewAuditEntry {
                action: "quote.issued".to_string(),
                subject: quote_id.as_str().to_string(),
                actor: request.account_id.as_str().to_string(),
                detail: serde_json::json!({
                    "market_id": request.market_id.as_str(),
                    "selection": request.selection.to_string(),
                    "stake_minor_units": request.stake.minor_units,
                    "odds_minor_units": odds.raw(),
                    "currency": request.stake.currency.to_string(),
                    "expires_at_unix": quote.expires_at_unix,
                })
                .to_string(),
            },
        )
        .await?;

        let staged = stage(store, vec![event]).await?;

        store.commit().await?;

        Ok(AttemptOutcome {
            price: QuotedPrice { quote_id, quote },
            events: staged,
        })
    }
}

/// What one attempt produced.
struct AttemptOutcome {
    /// The price.
    price: QuotedPrice,
    /// The events, staged inside the transaction and to be published now that it
    /// has committed.
    events: Vec<StreamEvent>,
}
