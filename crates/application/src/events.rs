//! # Market events
//!
//! What subscribers are told about a market, and the two properties the stream
//! has to hold for it to be useful.
//!
//! ## Events are written before they are published
//!
//! An event is appended to a durable log **inside the same transaction** as the
//! state change it describes, and only published to live subscribers after that
//! transaction commits. The order is not an optimisation, it is the only ordering
//! that is honest:
//!
//! - Publishing first means a subscriber can see an event for a match that then
//!   fails to commit, and has to be told to un-apply it.
//! - Writing first and publishing after means a committed change is never
//!   unsayable. A subscriber that misses the live broadcast still finds the event
//!   in the log and can catch up.
//!
//! The failure mode of the second is a gap, and the failure mode of the first is
//! a lie. Gaps are detectable; a stream that lies about money is not recoverable.
//!
//! ## Sequence numbers come from the database
//!
//! Each event's position is a `BIGSERIAL` assigned by the insert, not a counter
//! held in memory. That matters for more than durability: a subscriber that
//! reconnects and asks "what happened after 4,217?" gets an answer from the same
//! numbering the live stream used, so a gap detected mid-stream and a gap
//! detected after a reconnect are healed by the same mechanism — a resync from
//! the log. A counter in memory would number differently after a restart, and
//! every reconnect would be an unrecoverable discontinuity.
//!
//! Subscribers are told about gaps rather than left to infer them. See
//! [`StreamEvent::Gap`].

use domain::ids::MarketId;
use domain::money::Currency;
use domain::order::Selection;
use domain::settlement::Outcome;
use serde::{Deserialize, Serialize};

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A market was given a price and is accepting orders.
    MarketOpened {
        /// The market's id.
        market_id: MarketId,
        /// The price it is publishing.
        odds_minor_units: i64,
    },
    /// A market stopped accepting orders.
    MarketSuspended {
        /// The market's id.
        market_id: MarketId,
    },
    /// A market started accepting orders again at a price.
    MarketResumed {
        /// The market's id.
        market_id: MarketId,
        /// The price it is publishing.
        odds_minor_units: i64,
    },
    /// A market's outcome became known. Payouts have not been posted yet.
    MarketResolved {
        /// The market's id.
        market_id: MarketId,
        /// What it resolved to.
        outcome: OutcomeWire,
    },
    /// A market's payouts were posted and its hold closed.
    MarketSettled {
        /// The market's id.
        market_id: MarketId,
        /// The total returned to winners.
        total_paid_minor_units: i64,
        /// The total commission taken.
        total_commission_minor_units: i64,
    },
    /// A price was quoted to a customer.
    ///
    /// Its own kind rather than a reuse of [`EventKind::OrderPlaced`], because
    /// an order is not the only thing that moves a market's price forward — a
    /// quote that was never acted on still happened, and a subscriber tracking
    /// demand has to be able to see it.
    QuoteIssued {
        /// The stored id of the quote.
        quote_id: String,
        /// The market it is for.
        market_id: MarketId,
        /// The selection being backed.
        selection: Selection,
        /// The stake the quote is bound to.
        stake_minor_units: i64,
        /// The currency of the stake.
        currency: CurrencyWire,
    },
    /// A customer placed an order.
    OrderPlaced {
        /// The order's id.
        order_id: String,
        /// The market it is in.
        market_id: MarketId,
        /// The selection backed.
        selection: Selection,
        /// The stake.
        stake_minor_units: i64,
        /// The currency of the stake.
        currency: CurrencyWire,
    },
    /// An order filled, in whole or in part.
    OrderFilled {
        /// The order that was filled.
        order_id: String,
        /// The market it is in.
        market_id: MarketId,
        /// The counterparty it filled against.
        counterparty_order_id: String,
        /// How much changed hands.
        stake_minor_units: i64,
    },
    /// An order was cancelled before it matched, and no funds moved.
    OrderCancelled {
        /// The order's id.
        order_id: String,
        /// The market it was in.
        market_id: MarketId,
    },
    /// An order settled, either paid or extinguished.
    OrderSettled {
        /// The order's id.
        order_id: String,
        /// The market it was in.
        market_id: MarketId,
        /// What it was paid, or `0` if it lost.
        returned_minor_units: i64,
        /// Its net profit, which is `0` if it lost.
        profit_minor_units: i64,
    },
    /// A customer moved money in.
    FundsDeposited {
        /// Whose balance changed.
        account_id: String,
        /// How much arrived.
        amount_minor_units: i64,
        /// The currency it arrived in.
        currency: CurrencyWire,
    },
    /// A customer moved money out.
    FundsWithdrawn {
        /// Whose balance changed.
        account_id: String,
        /// How much left.
        amount_minor_units: i64,
        /// The currency it left in.
        currency: CurrencyWire,
    },
    /// Money movement was halted or resumed platform-wide.
    KillSwitchChanged {
        /// Whether money may now move.
        running: bool,
    },
}

/// An outcome, as it crosses the wire.
///
/// Mirrored rather than re-exported because [`domain::settlement::Outcome`] is
/// not a `serde` type — the domain deliberately has no serialisation contract —
/// and one adapter's guess at a wire representation should not become the one
/// every other adapter is forced to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeWire {
    /// The home selection won.
    HomeWin,
    /// The away selection won.
    AwayWin,
    /// The market was drawn.
    Draw,
}

impl From<OutcomeWire> for Outcome {
    fn from(wire: OutcomeWire) -> Self {
        match wire {
            OutcomeWire::HomeWin => Self::HomeWin,
            OutcomeWire::AwayWin => Self::AwayWin,
            OutcomeWire::Draw => Self::Draw,
        }
    }
}

impl From<Outcome> for OutcomeWire {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::HomeWin => Self::HomeWin,
            Outcome::AwayWin => Self::AwayWin,
            Outcome::Draw => Self::Draw,
        }
    }
}

/// A currency, as it crosses the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CurrencyWire {
    /// Pounds sterling.
    GBP,
    /// United States dollars.
    USD,
    /// Euros.
    EUR,
}

impl From<Currency> for CurrencyWire {
    fn from(currency: Currency) -> Self {
        // The currency set is closed — `Currency::from_code` admits only these
        // three — so the final arm is the euro and nothing else.
        if currency == Currency::GBP {
            Self::GBP
        } else if currency == Currency::USD {
            Self::USD
        } else {
            Self::EUR
        }
    }
}

impl From<CurrencyWire> for Currency {
    fn from(wire: CurrencyWire) -> Self {
        match wire {
            CurrencyWire::GBP => Self::GBP,
            CurrencyWire::USD => Self::USD,
            CurrencyWire::EUR => Self::EUR,
        }
    }
}

/// One thing that happened, and where it sits in the sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEvent {
    /// Its position in the log, starting at one.
    ///
    /// Zero means "not yet persisted". Use cases build events before the
    /// transaction commits and do not know the position until the insert
    /// returns it, so the value is only meaningful once an adapter has written
    /// it — and a subscriber is never handed one that has not been.
    pub seq: i64,
    /// What happened.
    pub kind: EventKind,
    /// When, as a Unix timestamp in seconds.
    pub occurred_at_unix: i64,
}

impl StreamEvent {
    /// A short, stable name for the event, for logs and for metrics.
    ///
    /// Written out rather than derived from `Debug` so that a metric label stays
    /// the same if a field is renamed, and so that renaming a *field* can never
    /// silently rename a *metric*.
    #[must_use]
    pub fn kind_label(&self) -> &'static str {
        match &self.kind {
            EventKind::QuoteIssued { .. } => "quote.issued",
            EventKind::MarketOpened { .. } => "market.opened",
            EventKind::MarketSuspended { .. } => "market.suspended",
            EventKind::MarketResumed { .. } => "market.resumed",
            EventKind::MarketResolved { .. } => "market.resolved",
            EventKind::MarketSettled { .. } => "market.settled",
            EventKind::OrderPlaced { .. } => "order.placed",
            EventKind::OrderFilled { .. } => "order.filled",
            EventKind::OrderCancelled { .. } => "order.cancelled",
            EventKind::OrderSettled { .. } => "order.settled",
            EventKind::FundsDeposited { .. } => "funds.deposited",
            EventKind::FundsWithdrawn { .. } => "funds.withdrawn",
            EventKind::KillSwitchChanged { .. } => "kill_switch.changed",
        }
    }
}

/// A gap in a subscriber's view of the stream.
///
/// Sent instead of silently skipping the missing events. A subscriber that
/// cannot afford to resync — a browser tab that was backgrounded for a minute —
/// is told to refetch, and one that can is told from which sequence number to
/// replay.
///
/// The alternative, quietly dropping the missed events, produces a client that
/// believes it has seen a market move when it has not. For a betting exchange
/// that is the difference between a display that is stale and one that is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    /// The first sequence number the subscriber missed.
    pub after_seq: i64,
    /// The first sequence number it is being sent.
    pub from_seq: i64,
}

/// The delivery end of the event stream.
///
/// A port rather than a `tokio::sync::broadcast` receiver so that the use cases
/// can be tested without a runtime, and so that the fanout mechanism can be
/// replaced — a Postgres `LISTEN`/`NOTIFY` or a Kafka topic for a multi-instance
/// deployment — without touching a single use case.
#[async_trait::async_trait]
pub trait EventPublisher: Send + Sync {
    /// Publish an event that has already been committed.
    ///
    /// Never called before the writing transaction commits, and never carries
    /// state that only exists inside it. A publisher that fails is an outage for
    /// live subscribers but not a correctness problem: the event is durable in
    /// the log, and a reconnecting subscriber reads it from there.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if the event cannot be broadcast.
    async fn publish(&self, event: &StreamEvent) -> Result<(), crate::ports::StoreError>;
}
