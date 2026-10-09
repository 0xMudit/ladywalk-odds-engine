//! # Ports
//!
//! The interfaces the use cases are written against, and the rule that
//! everything crossing them is owned by `domain`.
//!
//! ## Why traits and not concrete types
//!
//! A use case like placing an order needs to read a market, read a book, take a
//! stake, and commit — or do none of it. Expressing that against a real
//! database would mean the interesting logic, the rules about what a valid
//! order is, would only ever be exercisable by starting Postgres. Written
//! against these traits, the same use case runs against an in-memory store in
//! the test suite, so the rules are tested as rules rather than as a side
//! effect of a container being available.
//!
//! ## What the ports deliberately do not have
//!
//! No port takes or returns a row struct. Rows are an adapter's problem — they
//! mirror columns, and columns are allowed to change. What crosses is a
//! [`domain`] type, which means a change to the schema cannot change what the
//! use case does without the compiler saying so.
//!
//! ## Transactions
//!
//! [`Store`] is a unit of work: every repository it hands out reads and writes
//! inside it, and nothing is durable until [`Store::commit`] succeeds. Taking
//! the stake and marking the order matched are two different writes, and
//! separating them is how a customer ends up with a matched order and no money
//! held.
//!
//! ## Why `async fn` in a trait, via `async-trait`
//!
//! The adapters are async because the database is. Native `async fn` in traits
//! is not dyn-compatible, and the repositories are handed out as `dyn` so that
//! a unit of work can expose one per call site. `async-trait` is the cost of
//! that, and it is a crate that only exists to be a macro.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use domain::error::DomainError;
use domain::ids::{AccountId, IdempotencyKey, MarketId, OrderId, QuoteId, TransactionRef};
use domain::ledger::LedgerTransaction;
use domain::money::{Currency, MoneyAmount};
use domain::odds::Odds;
use domain::order::{Order, Selection};
use domain::quote::Quote;
use domain::risk::{CustomerRiskProfile, KillSwitchState};
use domain::settlement::Outcome;
use serde::{Deserialize, Serialize};

use crate::errors::AppError;
use crate::events::StreamEvent;

/// Something went wrong below the use case.
///
/// Deliberately not a [`DomainError`]: a rule violation and an unreachable
/// database are different problems, and an HTTP adapter maps them to different
/// status codes. Conflating them is how an outage turns into a 400.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreError {
    /// The database was unreachable, timed out, or returned an error.
    Backend(String),
    /// A row that the use case required was not there.
    Missing {
        /// What kind of record was expected.
        resource_type: &'static str,
        /// The id that was looked for.
        resource_id: String,
    },
    /// Two writers reached the same row and one had to lose.
    ///
    /// Separate from [`StoreError::Backend`] because the two are handled
    /// differently: a conflict is *expected* under load and the caller's answer
    /// is to try again, whereas a backend error is a fault to be alerted on.
    /// Collapsing them would mean either retrying a genuine outage forever or
    /// failing a legitimately contended request as if the database were down.
    Conflict {
        /// What was contended — a market row, a balance, an idempotency key.
        resource: &'static str,
        /// The backend's own description, for the log.
        detail: String,
    },
}

impl StoreError {
    /// Whether this is our fault rather than the caller's.
    #[must_use]
    pub const fn is_internal(&self) -> bool {
        match self {
            Self::Backend(_) | Self::Conflict { .. } => true,
            Self::Missing { .. } => false,
        }
    }

    /// Whether the same request could succeed if it were simply tried again.
    ///
    /// This is the flag a retry loop reads, and it is the reason a conflict is
    /// its own variant: retrying is the correct response to losing a race and the
    /// wrong response to a dropped connection, and only the first of those two
    /// produces a bounded, useful result.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Conflict { .. })
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(detail) => write!(f, "storage backend error: {detail}"),
            Self::Missing {
                resource_type,
                resource_id,
            } => write!(f, "no such {resource_type}: {resource_id}"),
            Self::Conflict { resource, detail } => {
                write!(f, "concurrent modification of {resource}: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreError {}

/// Where a market is in its own lifecycle.
///
/// Separate from [`domain::order::OrderStatus`], because the two answer
/// different questions: an order's status is about one customer's instruction,
/// while a market's is about whether *anyone* may trade it. They are also driven
/// by different people — a customer cancels an order, an operator resolves a
/// market — and conflating them is how a market ends up accepting orders after
/// the referee has been announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketState {
    /// Accepting orders and matching them.
    Open,
    /// Temporarily not accepting orders. Existing orders are untouched and may
    /// be cancelled by their customers.
    Suspended,
    /// The outcome is known and money is owed, but has not been paid yet.
    ///
    /// Distinct from [`MarketState::Settled`] because the two are separable in
    /// time and one is a customer-visible commitment while the other is an
    /// internal bookkeeping step. A market that has resolved but not settled
    /// is one where a customer is owed money that has not moved, and that is a
    /// different operational alert from a payout that failed.
    Resolved,
    /// Payouts posted and the hold closed.
    Settled,
    /// Held for investigation. No new orders, and settlement is refused until
    /// it is released.
    Quarantined,
}

impl MarketState {
    /// Whether the market accepts new orders.
    #[must_use]
    pub const fn accepts_orders(self) -> bool {
        matches!(self, Self::Open)
    }

    /// Whether a market in this state can still be settled.
    #[must_use]
    pub const fn can_settle(self) -> bool {
        matches!(self, Self::Resolved)
    }
}

/// A market as a use case needs it: its identity, its lifecycle, and the price
/// it is currently publishing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Market {
    /// The market's id.
    pub market_id: MarketId,
    /// Where the market is in its lifecycle.
    pub state: MarketState,
    /// The price every order on this market is filled at, if it has one.
    ///
    /// `None` while the market is being created, and `None` again once it has
    /// been suspended: a market with no price cannot be matched against, and
    /// representing that as a zero price would be a number someone could trade.
    pub published_odds: Option<Odds>,
    /// The currency every stake on this market is in.
    pub currency: Currency,
    /// The outcome, once it is known.
    pub outcome: Option<Outcome>,
}

/// Reading and writing markets.
#[async_trait]
pub trait MarketRepository: Send {
    /// The market, whatever state it is in.
    ///
    /// Returned as one value because a use case that needs the market also
    /// needs its price and its state, and reading them separately is a race
    /// with an administrative change. `Ok(None)` means there is no such market;
    /// a market that exists but is not open is returned with its state, because
    /// "suspended" and "does not exist" call for different responses from a
    /// customer and from an operator.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the market cannot be read.
    async fn find(&self, market_id: &MarketId) -> Result<Option<Market>, StoreError>;

    /// Take a write lock on the market row, and return it.
    ///
    /// This is the whole of the exchange's concurrency control, and the reason
    /// it is a port rather than an implementation detail: **matching must be
    /// serialized per market.** Two takers reading the same book and both
    /// filling against the same maker is the classic double-spend, and the fix
    /// has to be visible in the interface that matching is written against —
    /// if this method did not exist, nothing would stop someone reading a book
    /// and writing fills without holding anything.
    ///
    /// The lock is held until the surrounding [`Store`] commits or rolls back.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Missing`] if there is no such market.
    async fn lock(&mut self, market_id: &MarketId) -> Result<Market, StoreError>;

    /// Publish a price for a market that has none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the market cannot be written.
    async fn publish_odds(&mut self, market_id: &MarketId, odds: Odds) -> Result<(), StoreError>;

    /// Move a market to a new lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the market cannot be written.
    async fn set_state(&mut self, market_id: &MarketId, state: MarketState) -> Result<(), StoreError>;

    /// Record the outcome of a market, moving it to `Resolved` with it.
    ///
    /// The outcome and the state are written by one call rather than a
    /// `set_outcome` plus a `set_state` because they are the same fact: a market
    /// that is `Resolved` without an outcome is a market settlement will refuse,
    /// and a market with an outcome that is not `Resolved` is a market paying out
    /// before anyone said it was over. Two calls would make that state
    /// reachable — and reachable by a crash between them, which is exactly when
    /// it must not be.
    ///
    /// That said out loud: the alternative is not "a smaller transaction", it is
    /// a window in which a market is `Resolved` and has no outcome recorded, and
    /// [`crate::usecases::settlement::SettleMarket`] answers that with an
    /// internal error instead of a payout. This method exists so that the state
    /// is unreachable.
    ///
    /// Refuses a market that is not `Open` or `Suspended`, and refuses a second
    /// outcome, because re-resolving a market is how a customer gets paid twice
    /// for one event.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if the market cannot be resolved, and
    /// [`StoreError::Missing`] if there is no such market.
    async fn resolve(
        &mut self,
        market_id: &MarketId,
        outcome: Outcome,
    ) -> Result<(), StoreError>;

    /// Every market that is currently open, for reconciliation and for the
    /// settlement sweeper.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the markets cannot be read.
    async fn list_by_state(&self, state: MarketState) -> Result<Vec<Market>, StoreError>;
}

/// Reading and writing customers.
#[async_trait]
pub trait AccountRepository: Send {
    /// The customer, if it exists and may trade.
    ///
    /// `Ok(None)` covers both "no such customer" and "this customer is closed",
    /// because a customer who has been closed must not be able to tell the
    /// difference from one who never existed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the account cannot be read.
    async fn find_active(&self, account_id: &AccountId) -> Result<bool, StoreError>;

    /// An account's spendable balance in one currency, in minor units.
    ///
    /// Read from the maintained `account_balances` projection rather than by
    /// summing the ledger, because this is on the path of every order and a
    /// `SUM` over the whole ledger for every order does not scale. The
    /// projection is a trigger-maintained materialisation of the ledger, and
    /// [`LedgerRepository::reconcile_balances`] is the job that proves the two
    /// still agree; a projection read without that job is a cache with no
    /// invalidation story, which is why the reconciliation is a first-class use
    /// case rather than a script someone might forget to run.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the balance cannot be read.
    async fn balance_minor(
        &self,
        account_id: &AccountId,
        currency: Currency,
    ) -> Result<i64, StoreError>;

    /// Lock an account's balance row so that it cannot change under a
    /// transaction that is about to debit it.
    ///
    /// Taken in a deterministic order across every account a transaction
    /// touches, so that two transactions touching the same pair of accounts
    /// cannot deadlock each other by grabbing them in opposite orders.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the row cannot be locked.
    async fn lock_balance(
        &mut self,
        account_id: &AccountId,
        currency: Currency,
    ) -> Result<i64, StoreError>;
}

/// Reading and writing orders.
#[async_trait]
pub trait OrderRepository: Send {
    /// Persist a new order, quoted but not yet matched.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the order cannot be written.
    async fn insert(&mut self, order: &Order) -> Result<(), StoreError>;

    /// Every pending order in a market, oldest first.
    ///
    /// Order matters: matching fills the front of the book, so a snapshot
    /// returned in an arbitrary order would make the same request produce
    /// different fills on different days.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the book cannot be read.
    async fn pending_book(&self, market_id: &MarketId) -> Result<Vec<Order>, StoreError>;

    /// Record a fill: the order matched, and `filled` minor units of its stake
    /// are now held against the market.
    ///
    /// Takes the exposure rather than assuming it equals the stake. A taker that
    /// ran out of book is filled for less than it asked for, and writing the
    /// stake here would tell settlement the customer is owed on money nobody
    /// ever took from them — the hold would then be short at exactly the moment
    /// a thin market resolved. See [`Order::filled_minor_units`].
    ///
    /// The adapter loads the order and calls [`Order::mark_matched`], so an
    /// illegal transition — matching something already settled, or filling for
    /// more than the order's own stake — is refused by `domain` rather than
    /// written to a column.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the order cannot be written.
    async fn mark_matched(
        &mut self,
        order_id: &OrderId,
        holding: &TransactionRef,
        filled_minor_units: i64,
    ) -> Result<(), StoreError>;

    /// Cancel an order that never matched.
    ///
    /// Named for the intent rather than given a `set_status(order, new_status)`
    /// signature on purpose. A status setter would let a use case write any
    /// transition it liked, including the illegal ones — and the domain's whole
    /// contribution here is refusing them, so a setter would route around the
    /// only part of the system that is checking. The adapter calls
    /// [`Order::mark_cancelled`], which refuses anything that is not `Pending`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the order cannot be written.
    async fn cancel(&mut self, order_id: &OrderId) -> Result<(), StoreError>;

    /// Move a matched order to its terminal state after settlement.
    ///
    /// The adapter calls [`Order::mark_settled`], which requires the order to be
    /// `Matched`. That requirement is the point: a resting order that never
    /// matched has no claim to settle, and settling it would invent a payout.
    ///
    /// Note what is *not* a parameter: the new status. There is exactly one
    /// status a caller may move a matched order to through this method, so
    /// passing it in would be a parameter that can only be given the wrong value.
    /// A caller that has an unmatched order calls [`OrderRepository::cancel`],
    /// which is a different transition and refuses a matched order — the two are
    /// kept apart because conflating them is how a payout is invented for a
    /// customer who never risked anything.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the order cannot be written.
    async fn settle(&mut self, order_id: &OrderId) -> Result<(), StoreError>;

    /// Every order in a market that carries a matched claim, for settlement.
    ///
    /// "Carries a matched claim" rather than "has some money in the hold",
    /// because the two are not the same set of rows. A taker that ran out of book
    /// is filled for *less* than it asked for, and the unfilled remainder of its
    /// stake is gone from the book rather than resting in it. An implementation
    /// that filtered this on `status = 'matched'` would therefore be right, and
    /// one that filtered it on `filled_minor_units > 0` would also be right, and
    /// the second reads like a generalisation of the first until you notice the
    /// partial fill that is *not* still resting.
    ///
    /// What this must **not** filter on is the original stake. A partially filled
    /// order has [`Order::filled_minor_units`] in the hold, and that figure, not
    /// [`Order::stake_minor_units`], is what the payout is computed from.
    ///
    /// # Deliberately excludes resting orders
    ///
    /// This returns matched orders only, so settlement never sees a `Pending`
    /// order and never pays one. An order that never matched holds nothing and has
    /// no claim; cancelling it is a separate concern, and mixing the two reads
    /// here would mean a settlement transaction quietly voiding orders it has no
    /// reason to know about.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the orders cannot be read.
    async fn matched_orders(&self, market_id: &MarketId) -> Result<Vec<Order>, StoreError>;

    /// Void every order still resting in a market, and report which they were.
    ///
    /// Called when a market settles, for the resting orders that
    /// [`OrderRepository::matched_orders`] deliberately left out. It is a separate
    /// method rather than a widening of that one so the two uses cannot be
    /// confused: a settlement that needed to *see* pending orders to avoid paying
    /// them would be one refactor away from paying them, and the return value is
    /// the only thing that makes the voiding observable to a customer.
    ///
    /// Returning the affected orders rather than a count is the point. An order
    /// voided here is an order a customer is waiting to hear about, so each needs
    /// an `OrderCancelled` event, and a count cannot produce them.
    ///
    /// Already cancelled orders are not affected: this is an update of the
    /// `pending` set, so a row that is already `Cancelled` is not in it. Note that
    /// `Order` records no reason for a cancellation, so "untouched" here means the
    /// status is left alone and not that any explanation a customer was given
    /// survives — there is nowhere for one to be stored. Whether cancellation
    /// reasons need to be is an open question, and it is not this method's job to
    /// answer.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the orders cannot be written.
    async fn cancel_resting(
        &mut self,
        market_id: &MarketId,
    ) -> Result<Vec<Order>, StoreError>;

    /// Every order belonging to a customer, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the orders cannot be read.
    async fn list_for_account(&self, account_id: &AccountId) -> Result<Vec<Order>, StoreError>;

    /// The order, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the order cannot be read.
    async fn find(&self, order_id: &OrderId) -> Result<Option<Order>, StoreError>;
}

/// Writing to and reading from the ledger.
#[async_trait]
pub trait LedgerRepository: Send {
    /// Post a balanced transaction and every one of its legs.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the transaction cannot be written.
    async fn post(&mut self, transaction: &LedgerTransaction) -> Result<(), StoreError>;

    /// An account's signed balance in minor units, summed over every leg.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the balance cannot be computed.
    async fn balance_minor(&self, account_id: &AccountId) -> Result<i64, StoreError>;

    /// Every account whose balance is below zero.
    ///
    /// The ledger is double entry and a transaction can legally take an account
    /// negative — a matched customer's stake is debited at match time, not at
    /// settlement. So this is not automatically a bug, and it is not reported as
    /// one. What *is* a bug is a customer account going negative when it should
    /// have been refused, and [`LedgerRepository::reconcile_balances`] is where
    /// that distinction is made: it compares the maintained projection against
    /// the ledger and against the set of accounts that are *permitted* to be
    /// negative, and reports anything else as a violation.
    ///
    /// Kept as a port method rather than folded into reconciliation so the
    /// reconciliation job can return a list of offenders without having to
    /// return a boolean.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the balances cannot be read.
    async fn negative_accounts(&self) -> Result<Vec<(AccountId, i64)>, StoreError>;

    /// Every account where the maintained balance projection disagrees with the
    /// ledger, and what each side says.
    ///
    /// The projection is a cache, and this is the assertion that makes caching a
    /// balance defensible. It is a full-table comparison rather than a
    /// per-account one because the interesting failure is not "one account is
    /// wrong" but "the trigger stopped firing", and that is only visible when
    /// every row is compared.
    ///
    /// # Only the disagreements come back
    ///
    /// A full scan returning a row per account would hand the reconciliation job
    /// one entry for every balance in the book to throw all but a handful away,
    /// and on a real exchange that is a lot of data to move for a list that is
    /// empty whenever the system is healthy — which is nearly always. The scan
    /// itself is what must be complete, and that is a property of the adapter's
    /// SQL, not of what it returns.
    ///
    /// The consequence is that **the number of rows returned is not the number of
    /// accounts checked**. A healthy system returns none and has checked all of
    /// them, so a report that counted rows would claim a green run covered zero
    /// accounts. [`LedgerRepository::balance_accounts_compared`] is how a caller
    /// learns the size of the scan.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the comparison cannot be run.
    async fn reconcile_balances(&self) -> Result<Vec<BalanceDiscrepancy>, StoreError>;

    /// How many accounts [`LedgerRepository::reconcile_balances`] compared.
    ///
    /// Separate from that call so the scan can return only its disagreements
    /// without the caller having to guess how wide it was. A `count(*)` over the
    /// same union the scan uses, and expected to agree with it — a caller that
    /// wants to assert the scan was not truncated compares the two.
    ///
    /// `i64` rather than `usize` because it is a SQL row count and converting at
    /// the adapter boundary would be the adapter inventing a guarantee. It is
    /// never negative, and an implementation returning a negative number has a bug.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the count cannot be read.
    async fn balance_accounts_compared(&self) -> Result<i64, StoreError>;
}

/// A place where the maintained balance projection and the ledger disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceDiscrepancy {
    /// The account whose balance is in dispute.
    pub account_id: AccountId,
    /// The balance the projection holds.
    pub projected_minor_units: i64,
    /// The balance the ledger's own legs sum to.
    pub ledger_minor_units: i64,
}

/// The accounts whose balance is allowed to be negative, and why.
///
/// This is a hardcoded classification rather than configuration, for the same
/// reason [`domain::settlement::PLATFORM_REVENUE`] is: these names are part of
/// what the ledger means, and a deployment that could disagree with the
/// reconciliation job about which accounts may go negative is a deployment that
/// will eventually disagree with itself.
///
/// Two accounts are permitted, and it is worth being precise about why each one
/// is a liability rather than a loss:
///
/// - `platform:deposits` is **negative while cash is held outside the ledger**.
///   [`crate::usecases::wallet::deposit`] posts the customer's credit against a
///   debit to this account, because the counter-party to money arriving from
///   outside is not another account inside the ledger. A negative figure here
///   means the platform owes that much to the outside world, which is the
///   healthy state, not a shortfall. Flagging it would raise an alarm on every
///   single deposit, and an alarm that always fires is an alarm nobody reads.
/// - `platform:revenue` is **negative when the house is losing money**, which
///   happens whenever a market pays out more than it collected. That is the
///   business losing a bet, not the ledger being wrong, and it is the only
///   account where a negative balance is a normal consequence of a correct
///   settlement rather than a mirror of money held elsewhere.
///
/// Two accounts are deliberately *not* permitted, and both omissions are the
/// point:
///
/// - A customer's own account is **never** negative. Placing an order is refused
///   if the stake would overdraw it, and a matched stake is paid out of the
///   hold rather than drawn from the customer's account going negative first.
/// - `hold:<market>` is **always** positive while a market runs and zero after it
///   settles. A negative hold means a payout drew on money that was never there,
///   which is the most expensive thing this report can find — so it is reported
///   rather than permitted.
#[must_use]
pub fn is_permitted_to_be_negative(account_id: &AccountId) -> bool {
    let id = account_id.as_str();
    id == domain::settlement::PLATFORM_REVENUE
        || id == crate::usecases::wallet::PLATFORM_DEPOSITS
}

/// Replaying a request that has already been answered.
#[async_trait]
pub trait IdempotencyStore: Send {
    /// The stored answer for a key, if this request has been made before.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the record cannot be read.
    async fn find(&self, key: &IdempotencyKey) -> Result<Option<StoredResponse>, StoreError>;

    /// Remember the answer to a request.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the record cannot be written.
    async fn remember(
        &mut self,
        key: &IdempotencyKey,
        account_id: &AccountId,
        endpoint: &str,
        response: &StoredResponse,
    ) -> Result<(), StoreError>;

    /// Claim a key, or discover that it is already claimed.
    ///
    /// This is the primitive that makes idempotency safe under concurrency, and
    /// it is separate from [`IdempotencyStore::remember`] for a specific reason.
    /// The obvious implementation is find-then-write: read the key, do the work
    /// if absent, write the answer. Two concurrent retries of the same request
    /// both read "absent", both do the work, and both write — which for a
    /// deposit is the customer being credited twice from one tap.
    ///
    /// Claiming first closes that: the insert is the contention point, so exactly
    /// one caller wins it. The winner does the work and calls
    /// [`IdempotencyStore::remember`]; the loser is told the key is
    /// [`IdempotencyOutcome::InProgress`] and must not do the work.
    ///
    /// `InProgress` is deliberately not the same as a replay. A caller that gets
    /// `InProgress` has learned that its request is in flight but not what it
    /// will be answered with, and answering it as a replay would report a result
    /// that does not exist yet.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the claim cannot be written.
    async fn claim(
        &mut self,
        key: &IdempotencyKey,
        account_id: &AccountId,
        endpoint: &str,
    ) -> Result<IdempotencyOutcome, StoreError>;
}

/// What claiming an idempotency key established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyOutcome {
    /// This caller claimed the key and is the one that should do the work.
    Claimed,
    /// The key was claimed earlier by the same customer and endpoint, and its
    /// stored answer must be replayed.
    Replay(StoredResponse),
    /// The key was claimed by an *earlier request that is still running*.
    InProgress,
    /// The key belongs to a different customer, or a different operation.
    ///
    /// Reported rather than replayed, because replaying would be the dangerous
    /// answer: a caller who reused a key for a *different* request would be handed
    /// the first request's result, and a client that quietly reuses keys — which
    /// is a common and entirely reasonable bug — would be told its new order
    /// succeeded when nothing was placed.
    ///
    /// Whether a same-customer, same-endpoint retry with a *different body* is
    /// also a reuse is the part still open; see the request-digest column in
    /// `docs/design/open-questions.md`. Until that is decided, this variant
    /// covers the two cases that are unambiguously wrong.
    Reused {
        /// The customer the key was first used by.
        original_account_id: AccountId,
        /// The operation it was first used for.
        original_endpoint: String,
    },
}

/// A response that was already produced, replayed rather than recomputed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResponse {
    /// The status code the original request returned.
    pub status: u16,
    /// The original body, byte for byte.
    pub body: String,
}

impl StoredResponse {
    /// A response to remember.
    #[must_use]
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

/// Signing and checking the server's price.
///
/// A trait rather than an HMAC call so that the use cases can be tested against
/// a stub, and so that rotating to a different scheme is a deployment decision
/// rather than a recompile of every call site.
#[async_trait]
pub trait QuoteSigner: Send + Sync {
    /// The signature over a quote's signing input.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Internal`] if the signer is misconfigured, which
    /// is an operator problem and never the caller's.
    async fn sign(&self, signing_input: &str) -> Result<String, DomainError>;

    /// Whether `signature` is the one this signer would have produced.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Internal`] if the check cannot be performed.
    async fn verify(&self, signing_input: &str, signature: &str) -> Result<bool, DomainError>;
}

/// A customer's risk configuration, and the platform-wide switch.
#[async_trait]
pub trait RiskRepository: Send {
    /// The customer's exposure limit.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the profile cannot be read.
    async fn profile(&self, account_id: &AccountId)
        -> Result<CustomerRiskProfile, StoreError>;

    /// The customer's current exposure in minor units: the sum of their stakes
    /// on orders that are pending or matched and not yet settled.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the exposure cannot be computed.
    async fn exposure_minor(&self, account_id: &AccountId) -> Result<i64, StoreError>;

    /// Whether the platform is accepting money movement at all.
    ///
    /// Read on every use rather than cached, because the answer has to be
    /// correct within seconds of an operator flipping the switch, and a cached
    /// answer is a switch with a propagation delay. The state lives in the same
    /// Postgres the money moves through, which is what makes that promise
    /// keepable: there is no second system to fall out of sync, and a switch
    /// flipped in the same transaction as the request it halts cannot be lost.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the switch cannot be read.
    async fn kill_switch(&self) -> Result<KillSwitchState, StoreError>;

    /// Halt or resume money movement platform-wide.
    ///
    /// Operator-only, and the reason it is a port rather than a column the
    /// operator can `UPDATE` by hand is that the act is itself an auditable
    /// event. Who halted trading and when is the first question asked after an
    /// incident, and it has to be answerable without trusting that nobody
    /// forgot to write it down.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the switch cannot be written.
    async fn set_kill_switch(
        &mut self,
        state: KillSwitchState,
        actor: &Actor,
        reason: &str,
    ) -> Result<(), StoreError>;
}

/// Recording a hash-chained audit entry.
///
/// The hashing itself is not here: [`crate::audit`] owns it, so that the in-memory
/// fake and the Postgres adapter cannot drift apart and start reporting a broken
/// chain on an intact database. What this contract adds is the one thing a shared
/// function cannot supply — the serialisation.
#[async_trait]
pub trait AuditLog: Send {
    /// Append an entry, chaining it to the current head.
    ///
    /// The chain exists so that a later reader can tell an audit trail that is
    /// complete from one that is merely present. A plain append-only table
    /// proves nothing about rows that were never written: an operator with write
    /// access can delete a row and there is no residue. Chaining each entry to the
    /// hash of its predecessor means any deletion or edit breaks every
    /// subsequent link, and the break is detectable without knowing what should
    /// have been in the gap.
    ///
    /// The chain is written inside the caller's transaction, so an audit entry
    /// and the change it describes either both exist or neither does.
    ///
    /// ## Serialising on the head
    ///
    /// An implementation **must** read the current head under a lock that no other
    /// append can pass, hold it until the caller commits, and write the new
    /// position and hash as the new head in that same transaction. This is the one
    /// requirement on an adapter that is easy to get wrong and impossible to see:
    ///
    /// - Reading the head without a lock lets two transactions both see entry *n*
    ///   as the last one. Both chain to it, both write position *n+1*, and one
    ///   silently overwrites the other. The chain still verifies, because the
    ///   surviving row is internally consistent — it has just lost a sibling, and
    ///   the link that would have revealed the loss never got written.
    /// - Taking the head's position from a sequence rather than from the chain lets
    ///   the same thing happen with an extra step: a rolled-back insert consumes a
    ///   number, so the chain is dense in *hashes* but sparse in *positions*, which
    ///   the discontinuity check reports forever.
    ///
    /// So the position and the hash both come from the row being appended, and the
    /// head row is what is locked. In Postgres that is a single-row table locked
    /// `FOR UPDATE`, or `SELECT ... ORDER BY seq DESC LIMIT 1 FOR UPDATE` on the
    /// chain itself; the in-memory fake gets it from the mutex every staged copy
    /// shares.
    ///
    /// ## What this still does not prove
    ///
    /// Chaining detects an edit, an insertion, or a reordering. It does **not**
    /// detect a suffix being deleted, because every surviving link remains valid.
    /// Catching that needs the head hash recorded outside the reach of whoever can
    /// write the table, which is a notary or a published digest rather than
    /// anything a port can require of a single table. [`Self::head_seq`] exists so
    /// an implementation can record the position somewhere durable and let
    /// [`crate::audit::verify_chain`] compare against it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the entry cannot be written. Returns
    /// [`StoreError::Conflict`] if the head is contended, so that
    /// [`with_retry`] re-runs the whole use case rather than leaving the caller to
    /// decide.
    async fn append(&mut self, entry: NewAuditEntry) -> Result<AuditRecord, StoreError>;

    /// Read the chain back, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the chain cannot be read.
    async fn read_chain(&self) -> Result<Vec<AuditRecord>, StoreError>;

    /// The position of the last entry, or `0` if the chain is empty.
    ///
    /// Recorded separately so a truncated chain is detectable. The obvious
    /// alternative — deriving the count from the rows — cannot tell a short chain
    /// from a cut one, which is the whole question.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the head cannot be read.
    async fn head_seq(&self) -> Result<i64, StoreError>;

    /// Verify that the chain is intact, using the recorded head.
    ///
    /// Recomputes every link rather than trusting the stored hashes, so a row
    /// edited *in place* — which is possible for a privileged session, and is
    /// exactly the case the chain exists to catch — is detected rather than
    /// inherited. Delegates to [`crate::audit::verify_chain`], so the answer means
    /// the same thing here as it does in a test.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the chain cannot be read or recomputed.
    async fn verify_chain(&self) -> Result<bool, StoreError>;
}

/// An audit entry as the use case describes it, before the chain hashes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditEntry {
    /// What happened, e.g. `"order.placed"`.
    pub action: String,
    /// What it happened to, e.g. an order id.
    pub subject: String,
    /// Who did it, or `"system"` for scheduled work.
    pub actor: String,
    /// The free-form detail, as a JSON object so that it can be queried later.
    pub detail: String,
}

/// An audit entry as stored, with its position and hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// Its position in the chain, starting at one.
    pub seq: i64,
    /// What happened.
    pub action: String,
    /// What it happened to.
    pub subject: String,
    /// Who did it.
    pub actor: String,
    /// The detail, as a JSON object.
    pub detail: String,
    /// When, as a Unix timestamp in seconds.
    pub occurred_at_unix: i64,
    /// The hash of the previous entry, or the genesis constant for the first.
    pub previous_hash: String,
    /// This entry's hash: over its own fields and the previous one.
    pub hash: String,
}

/// The durable, append-only log of everything that has happened.
///
/// This is the half of the event stream that outlives the process. A live
/// subscriber connected at the wrong moment, a browser tab that was closed, and
/// a support agent reconstructing what a customer's balance was doing three weeks
/// ago are all answered from here, and all of them are answered *identically*
/// because the positions come from the same numbering the live stream used.
///
/// ## Why append takes the transaction
///
/// [`EventLog::append`] is a method on the unit of work rather than a free
/// function taking a pool, so an event is written in the same transaction as the
/// state change it describes. That is the whole reason the stream can be trusted:
/// there is no window in which money has moved and the event saying so does not
/// exist, and no window in which an event exists for a change that rolled back.
///
/// ## Why `append` returns events
///
/// Because the position is assigned by the insert, not chosen by the caller, a
/// use case cannot know an event's sequence number before it writes it. Handing
/// back the persisted events is what lets the caller publish exactly what was
/// written, with a real number on it, rather than publishing a guess. The
/// `seq: 0` on an event built by a use case is therefore not a value to send
/// anywhere — see [`crate::events::StreamEvent::seq`].
#[async_trait]
pub trait EventLog: Send {
    /// Append events in the order given, and return them as persisted.
    ///
    /// A slice rather than one event at a time because a single use case often
    /// produces several — a settlement writes one per order plus a market-level
    /// one — and one round trip is cheaper than five.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the rows cannot be written, which aborts the
    /// surrounding transaction. Failing to write an event is not something a use
    /// case can work around: the alternative is a committed change nobody was
    /// told about.
    async fn append(&mut self, events: &[StreamEvent]) -> Result<Vec<StreamEvent>, StoreError>;

    /// Read the log after a position, oldest first.
    ///
    /// This is the catch-up path. A subscriber that noticed it missed events
    /// hands over the last position it saw and gets everything since, so a
    /// reconnect and a mid-stream gap heal through the same code.
    ///
    /// `limit` bounds the response, because a subscriber that was away for a
    /// weekend can be sent millions of events. A caller that gets fewer rows than
    /// it expected asks again from the last position it received rather than
    /// holding the whole log in memory.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the log cannot be read.
    async fn read_after(
        &self,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<StreamEvent>, StoreError>;

    /// The most recent position in the log, or `0` if it is empty.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the log cannot be read.
    async fn latest_seq(&self) -> Result<i64, StoreError>;
}

/// Everything one request's work happens inside.
///
/// Every repository it hands out reads and writes inside the same database
/// transaction, and nothing is durable until [`Store::commit`] succeeds. Taking
/// the stake and marking the order matched are two different writes, and
/// separating them is how a customer ends up with a matched order and no money
/// held.
///
/// The methods take `&mut self` rather than consuming the store, because that
/// is what `async-trait` supports for a dyn-compatible trait. The
/// exactly-once guarantee comes from the caller instead: a use case takes
/// `Box<dyn Store>` by value, so the only two things it can do with it are
/// commit and roll back, and it cannot do either twice.
#[async_trait]
pub trait Store: Send {
    /// Markets.
    fn markets(&mut self) -> &mut dyn MarketRepository;
    /// Orders.
    fn orders(&mut self) -> &mut dyn OrderRepository;
    /// The ledger.
    fn ledger(&mut self) -> &mut dyn LedgerRepository;
    /// Replay protection.
    fn idempotency(&mut self) -> &mut dyn IdempotencyStore;
    /// Risk configuration and the kill switch.
    fn risk(&mut self) -> &mut dyn RiskRepository;
    /// Stored quotes.
    fn quotes(&mut self) -> &mut dyn QuoteRepository;
    /// Customers and their balances.
    fn accounts(&mut self) -> &mut dyn AccountRepository;
    /// The hash-chained audit trail.
    fn audit(&mut self) -> &mut dyn AuditLog;
    /// The durable event log.
    fn events(&mut self) -> &mut dyn EventLog;

    /// Make everything written in this store durable.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the transaction cannot be committed, in which
    /// case nothing written in it is visible.
    async fn commit(&mut self) -> Result<(), StoreError>;

    /// Discard everything written in this store.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the rollback itself fails, which means the
    /// transaction is left for the database to abort when the connection drops.
    async fn rollback(&mut self) -> Result<(), StoreError>;
}

/// Starting a unit of work.
///
/// Split out from [`Store`] so that a use case can be handed the factory rather
/// than a store. A use case that took a `Box<dyn Store>` would work, but a use
/// case that takes a `StoreFactory` can retry: on [`StoreError::Conflict`] it
/// starts a *new* unit of work and tries again, which is the only correct
/// response to losing a race. Passing a store in by value gives the use case no
/// way to get a second one, and a use case that cannot retry a conflict will
/// eventually return an error to a customer for a reason that was never real.
#[async_trait]
pub trait StoreFactory: Send + Sync {
    /// Begin a new unit of work.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if no connection can be obtained. This is
    /// a genuine outage rather than contention, and a caller should not retry it
    /// indefinitely.
    async fn begin(&self) -> Result<Box<dyn Store>, StoreError>;
}

/// How many times a use case will re-run its body after losing a race.
///
/// A bound, not a `while true`: under sustained contention an unbounded retry
/// loop is a request that never returns, which is worse for the system than the
/// conflict it was trying to avoid. Each attempt is a fresh transaction, so
/// progress is made on the way to the bound or the conflict was not transient.
pub const MAX_CONFLICT_RETRIES: usize = 5;

/// Run a unit of work, retrying it if it loses a race.
///
/// The retry is the whole reason this is a function. Contention is not an error
/// condition in a design that serializes on the market row: two customers
/// placing orders on the same popular market *will* collide, and for one of them
/// the correct outcome is simply to try again a few milliseconds later. Lifting
/// that loop out of every use case means none of them can forget it, and there is
/// exactly one place where the bound on it is written down.
///
/// ## Why the factory is moved into the body rather than borrowed
///
/// The obvious signature hands the body a `&mut Box<dyn Store>` and saves the
/// caller from calling [`StoreFactory::begin`] itself. It cannot be written. A
/// body that awaits while holding that borrow returns a future whose type
/// mentions the borrow's lifetime, and `FnMut` would have to name one concrete
/// future type for *every* lifetime the borrow might have — which is exactly
/// what `Fn` sugar means by a higher-ranked bound. The alternative, boxing the
/// body future, pushes the cost of the same lifetime problem onto every call
/// site as a `Box::pin`.
///
/// Passing the factory by value sidesteps it: the body owns an `Arc`, so the
/// future it returns has a single type no matter what, and the guarantee the
/// signature exists to provide is kept. There is no store in scope to reuse —
/// the only thing the body can reach is a factory, and the only thing it can do
/// with that is begin something new. A retry that reuses the transaction it just
/// lost the race in is not a retry, and in PostgreSQL it cannot even be
/// attempted: a transaction that has failed a statement is already aborted, so a
/// second try inside it fails identically and the request eventually returns the
/// conflict the loop was written to avoid.
///
/// Each attempt gets a fresh transaction, and it is the body's job to
/// [`Store::commit`]. A store dropped without committing discards its
/// transaction, which is the correct reading for a body that returns early — a
/// replayed idempotent response, or a validation failure — because there is
/// nothing to keep.
///
/// # Errors
///
/// Returns the body's error, after at most
/// [`MAX_CONFLICT_RETRIES`] further attempts. Returns [`AppError::Store`] if a
/// later attempt cannot even open a unit of work, which is an outage rather than
/// contention and is not worth retrying on top of a conflict.
pub async fn with_retry<T, F, Fut>(factory: Arc<dyn StoreFactory>, mut work: F) -> Result<T, AppError>
where
    F: FnMut(Arc<dyn StoreFactory>) -> Fut,
    Fut: std::future::Future<Output = Result<T, AppError>> + Send,
{
    let mut attempt = 0_usize;
    loop {
        // Cloned per attempt, and moved into the body, so each attempt has a
        // factory it can own rather than one it has to borrow.
        let store = Arc::clone(&factory);
        match work(store).await {
            Err(error) if is_conflict(&error) && attempt < MAX_CONFLICT_RETRIES => {
                attempt += 1;
                tracing::debug!(
                    attempt,
                    max_attempts = MAX_CONFLICT_RETRIES,
                    "lost a race; retrying in a new transaction"
                );
            }
            outcome => return outcome,
        }
    }
}

/// Whether an application error is a lost race rather than a failure.
///
/// Reads the error rather than inspecting the store, so a use case that wraps a
/// conflict in another error type is still retried. The alternative — returning
/// the store error and having the caller inspect it — is how a retry loop ends
/// up retrying the wrong thing.
#[must_use]
pub fn is_conflict(error: &AppError) -> bool {
    matches!(error, AppError::Store(StoreError::Conflict { .. }))
}

/// Where a request's time comes from.
///
/// Injected rather than read from the system clock so that quote expiry is
/// testable at an exact instant instead of by sleeping.
///
/// `Send + Sync` because it is held behind an `Arc` in [`crate::usecases::Deps`]
/// and shared by every in-flight request, and a clock that cannot cross a thread
/// boundary is a clock that only works in a single-threaded test.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch.
    fn now_unix(&self) -> i64;
}

/// Where the ids on new records come from.
///
/// Every method that can produce an invalid identifier returns a
/// [`StoreError::Backend`] rather than panicking, because these are called from
/// the middle of a transaction on the money path. A panic there is a 500 with a
/// half-written unit of work, and an error there is a clean rollback.
pub trait IdGenerator {
    /// An id for a new order.
    fn new_order_id(&mut self) -> OrderId;

    /// A reference grouping the legs of one transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if no valid reference can be built from
    /// `prefix`.
    fn new_transaction_ref(&mut self, prefix: &str) -> Result<TransactionRef, StoreError>;

    /// An id for a stored quote.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if no valid id can be generated.
    fn new_quote_id(&mut self) -> Result<QuoteId, StoreError>;

    /// An id for a new market.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if no valid id can be generated.
    fn new_market_id(&mut self) -> Result<MarketId, StoreError>;

    /// An id for a new customer account.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if no valid id can be generated.
    fn new_account_id(&mut self) -> Result<AccountId, StoreError>;

    /// A key for a request whose retry must be safe.
    ///
    /// `suffix` names the operation, so a key read in a log is recognisable
    /// without a database lookup.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Backend`] if no valid key can be generated.
    fn new_idempotency_key(&mut self, suffix: &str) -> Result<IdempotencyKey, StoreError>;
}

/// Storing a quote that was issued, for audit.
#[async_trait]
pub trait QuoteRepository: Send {
    /// Record a quote as issued.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the quote cannot be written.
    async fn insert(&mut self, id: &QuoteId, quote: &Quote) -> Result<(), StoreError>;
}

/// What a customer is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// A customer: may place, cancel and read their own orders.
    Customer,
    /// An operator: may suspend markets and resolve them.
    Operator,
}

/// The caller, as far as a use case is concerned.
///
/// Authorization is a *use case* concern rather than a middleware one, and the
/// reason is that middleware cannot know the subject. A route guard can check
/// that a token is valid; it cannot check that this operator is allowed to
/// resolve *this* market, or that this customer is reading their own orders
/// rather than someone else's. Putting the role here means the check happens
/// where the subject is known, and a use case that forgot to check would not
/// compile without a role to compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// The account acting.
    pub account_id: AccountId,
    /// What this caller may do.
    pub role: Role,
}

impl Actor {
    /// A customer acting on their own behalf.
    #[must_use]
    pub const fn customer(account_id: AccountId) -> Self {
        Self {
            account_id,
            role: Role::Customer,
        }
    }

    /// An operator acting on the platform's behalf.
    #[must_use]
    pub const fn operator(account_id: AccountId) -> Self {
        Self {
            account_id,
            role: Role::Operator,
        }
    }

    /// Whether this actor may act as an operator.
    #[must_use]
    pub const fn is_operator(&self) -> bool {
        matches!(self.role, Role::Operator)
    }
}

/// What a caller asked for, before any rule has been applied to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrderRequest {
    /// The account the order is for.
    ///
    /// A field rather than an [`Actor`] parameter on the use case, because
    /// placing an order for someone else is not a thing this exchange offers: the
    /// route authenticates a caller, and the account on the request must be that
    /// caller's own. Keeping the actor out of the request and the account in it
    /// is what makes that check expressible — an adapter compares the two, and
    /// there is no field for a role to arrive in a customer-controlled body.
    pub account_id: AccountId,
    /// The market to bet on.
    pub market_id: MarketId,
    /// The outcome being backed.
    pub selection: Selection,
    /// How much to stake.
    pub stake: MoneyAmount,
    /// The signed price the customer was shown.
    pub quote: Quote,
    /// The key that makes a retry of this request safe.
    pub idempotency_key: IdempotencyKey,
}

/// A short summary of a placed order, safe to return to the caller.
///
/// `Serialize` because this exact struct is what gets stored against the
/// idempotency key: the bytes a replay returns are the bytes the first caller
/// was given, so a replay cannot be a subtly different response to the same
/// request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSummary {
    /// The order's id.
    pub order_id: OrderId,
    /// The price it rested or filled at.
    pub odds: Odds,
    /// Whether it found a fill.
    pub matched: bool,
    /// How many resting orders it filled against.
    pub fills: usize,
    /// How much of the stake actually found a counterparty, in minor units.
    pub filled_minor_units: i64,
    /// The stake that was asked for, in minor units.
    pub stake_minor_units: i64,
    /// The customer's balance after the stake was taken.
    pub balance_minor: i64,
}
