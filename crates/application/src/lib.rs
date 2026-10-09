//! # Application
//!
//! The layer that turns a request into a decision, without knowing how the
//! decision is stored or transported.
//!
//! ## The shape of the crate
//!
//! Everything in here is one of three things:
//!
//! * [`ports`] — what the outside world must be able to do for a use case to
//!   work: read a market, write an order, commit a transaction, tell the time.
//! * [`signing`] — the one real implementation of a port that this crate can
//!   supply without an adapter, because a signature needs a secret and a
//!   secret is configuration rather than infrastructure.
//! * [`errors`] — the single error that leaves, split so an HTTP adapter can
//!   answer the only question it really has: whose fault is this?
//!
//! * [`usecases`] — the actual behaviour: place an order, cancel one, settle a
//!   market, move money, reconcile. Each one is written against the ports above
//!   and against `domain`, and each one can be exercised with an in-memory store
//!   so the rules are tested as rules.
//!
//! The open questions in `docs/design/open-questions.md` are still open, and
//! the use cases are written so that answering one is a change in one place
//! rather than a rewrite: everything a question could affect sits behind a port
//! or a domain function, never inlined into a handler.
//!
//! ## Why the ports are `async` and dyn-safe
//!
//! Every use case needs to reach a database, and every test of a use case must
//! not. An `async_trait` keeps the port object-safe, so a test can hand a use
//! case an in-memory store and a production handler can hand it a SQLx one,
//! with no generics in the use case signature and no feature flag that changes
//! what compiles.

#![deny(clippy::pedantic)]
#![deny(clippy::nursery)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![deny(clippy::float_arithmetic, clippy::float_cmp, clippy::float_conversion)]
#![forbid(unsafe_code)]
// Two nursery lints are off crate-wide, and only these two, with a reason each.
//
// `exhaustive_structs` wants every struct to keep at least one field private and
// gain a builder for the rest. This layer is mostly ports and the records they
// move — `Market`, `NewAuditEntry`, `SettlementSummary`, `OrderSummary` and the
// request types. Making those fields private would mean an adapter in another
// crate cannot construct the value it was asked to persist, which is the one job
// it has, and would add a constructor to a struct whose whole point is that its
// fields *are* the contract. The lint is aimed at types designed to be built up
// over time; a record crossing a process boundary is not one.
#![allow(clippy::exhaustive_structs)]
// `missing_const_for_fn` asks for `const fn` on every eligible function. A large
// share of what is eligible here is not eligible in a way the lint can see — a
// function that formats a string, that takes a `MutexGuard`, or that is one arm of
// an `async_trait` is not `const` even when every line in it looks const-shaped.
// Where `const` genuinely is right here it is written, and where it is wrong the
// alternative is a `const fn` that could not be called anyway.
#![allow(clippy::missing_const_for_fn)]

pub mod audit;
pub mod config;
pub mod errors;
pub mod events;
pub mod ids;
pub mod ports;
pub mod signing;
pub mod shutdown;
pub mod telemetry;
pub mod testing;
pub mod usecases;

pub use errors::AppError;

/// Rebuilt from `domain` so an adapter has one import for the vocabulary of
/// this system rather than a `use` per type.
///
/// Only what a caller outside this crate genuinely needs. A use-case function is
/// re-exported under its module path (`markets::resolve_market`) rather than
/// flattened into the prelude, because these modules are not namespaces: two of
/// them already have a `publish`-shaped internal helper, and a flat list would
/// have to invent a disambiguation that reads worse than the module it came from.
pub mod prelude {
    pub use domain::error::DomainError;
    pub use domain::ids::{AccountId, IdempotencyKey, MarketId, OrderId, TransactionRef};
    pub use domain::money::{Currency, MoneyAmount, MoneyError};
    pub use domain::odds::Odds;
    pub use domain::order::{Order, OrderRequest, OrderStatus, Selection};
    pub use domain::quote::Quote;
    pub use domain::risk::{CustomerRiskProfile, KillSwitchState};

    pub use crate::errors::AppError;
    pub use crate::events::{EventKind, Gap, StreamEvent};    pub use crate::ids::UuidGenerator;
    pub use crate::ports::{
        AccountRepository, Actor, AuditLog, AuditRecord, BalanceDiscrepancy, Clock, IdGenerator,
        IdempotencyOutcome, IdempotencyStore, LedgerRepository, Market, MarketRepository,
        MarketState, NewAuditEntry, OrderRepository, OrderSummary, PlaceOrderRequest,
        QuoteRepository, QuoteSigner, Role, RiskRepository, Store, StoreError, StoreFactory,
        StoredResponse, with_retry,
    };
    pub use crate::signing::{HmacSigner, SystemClock};
    pub use crate::usecases::Deps;
    pub use crate::usecases::markets::{SuspendMarket, resolve_market, resume_market, set_kill_switch};
    pub use crate::usecases::orders::{CancelOrder, PlaceOrder};
    pub use crate::usecases::quotes::{QuoteMarket, QuoteRequest, QuotedPrice};
    pub use crate::usecases::reconciliation::{ReconciliationReport, Violation};
    pub use crate::usecases::settlement::{SettleMarket, SettlementSummary, SweepReport};
    pub use crate::usecases::wallet::{
        BalanceRequest, DepositRequest, PLATFORM_DEPOSITS, WalletBalance, WithdrawRequest, balance,
        deposit, withdraw,
    };
}
