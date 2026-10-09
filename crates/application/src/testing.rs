//! # An in-memory store, for testing the rules rather than the wiring
//!
//! Every use case is written against [`crate::ports`], and this module implements
//! all of it in a `BTreeMap`. The point is not to reimplement Postgres. The point
//! is that "the audit entry is written in the same transaction as the change" and
//! "the balance row is locked before it is read" are properties of a *use case*,
//! and a property of a use case can only be tested without a container if the
//! ports can be satisfied without one.
//!
//! ## What is faithful
//!
//! **Transaction boundaries.** [`InMemoryStoreFactory::begin`] snapshots the state
//! and gives the new unit of work its own copy. Writes land in the copy.
//! [`Store::commit`] swaps it in; [`Store::rollback`] throws it away. So a use
//! case that returns early without committing really does leave nothing behind,
//! and a test that asserts "this failed, so nothing changed" is testing the use
//! case and not the fake.
//!
//! **The balance projection.** [`FakeLedgerRepository::post`] updates the
//! maintained `account_balances` the way the production trigger does, so
//! [`reconcile`](crate::usecases::reconciliation) is green by default — and
//! [`InMemoryStoreFactory::set_projected_balance`] lets a test break it on
//! purpose.
//!
//! **Domain refusals.** The order repository calls `Order::mark_matched`,
//! `mark_cancelled` and `mark_settled` and turns a refusal into a
//! [`StoreError::Conflict`], because that is what an adapter does. A fake that
//! wrote the status column directly would let a use case pass a test that
//! production would reject.
//!
//! ## What is not
//!
//! - **No isolation between concurrent transactions.** Each unit of work gets its
//!   own snapshot and they all merge at commit, so two attempts that raced will
//!   both succeed here and one will conflict in production. Contention is
//!   therefore *not* testable here, and the retry loop in
//!   [`crate::ports::with_retry`] is exercised by a hand-rolled store instead
//!   (see [`InMemoryStoreFactory::fail_next`]).
//! - **The audit chain is not cryptographic.** The fake chains by position and
//!   previous hash with a plain FNV-1a, which detects a deleted or reordered row
//!   and nothing more. A fake that pretended to be tamper-evident would let a
//!   test pass that says a tampering test passes.
//! - **No book ordering by time.** [`FakeOrderRepository::pending_book`] sorts by
//!   id, because an `Order` has no timestamp and "oldest first" is not otherwise
//!   reproducible. A test that depends on a particular fill order must therefore
//!   control the ids.
//! - **Money is not checked for negative customer balances.** In production that
//!   is a `CHECK` constraint. Here [`FakeLedgerRepository::post`] is free to drive
//!   an account negative, which is deliberate: [`reconcile`] is supposed to
//!   notice that, and a fake that refused it would make the check unfalsifiable.

// The lints denied crate-wide are right for code that runs in production and
// wrong for a test double's own fixtures. A hardcoded id string in `harness()`
// that somehow failed validation would be a bug in this file, and panicking on it
// is how that bug gets reported rather than how it gets papered over.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use domain::ids::{AccountId, IdempotencyKey, MarketId, OrderId, QuoteId};
use domain::ledger::LedgerTransaction;
use domain::money::Currency;
use domain::odds::Odds;
use domain::order::{Order, OrderStatus};
use domain::quote::Quote;
use domain::risk::{CustomerRiskProfile, KillSwitchState};
use domain::settlement::Outcome;

use crate::events::StreamEvent;
use crate::ids::UuidGenerator;
use crate::ports::{
    AccountRepository, Actor, AuditLog, AuditRecord, BalanceDiscrepancy, Clock, EventLog,
    IdempotencyOutcome, IdempotencyStore, LedgerRepository, Market, MarketRepository, MarketState,
    NewAuditEntry, OrderRepository, QuoteRepository, QuoteSigner, RiskRepository, Store, StoreError,
    StoreFactory, StoredResponse,
};
use crate::signing::HmacSigner;
use crate::usecases::Deps;

/// The hash a chain starts from, standing in for "nothing came before".
pub const GENESIS_HASH: &str = "0000000000000000";

/// Everything the fake database holds.
///
/// Public and cloneable so a test can assert on the committed state directly
/// rather than only through a use case's return value — which is often the only
/// way to check the thing that actually matters, such as whether an audit entry
/// was written at all.
#[derive(Debug, Clone)]
pub struct FakeState {
    /// Accounts that exist and may trade.
    pub accounts: BTreeSet<AccountId>,
    /// The maintained balance projection, `(account, currency) -> minor units`.
    ///
    /// The thing a database trigger maintains in production, and the thing
    /// reconciliation compares against the ledger.
    pub balances: BTreeMap<(AccountId, Currency), i64>,
    /// Per-customer risk configuration.
    pub profiles: BTreeMap<AccountId, CustomerRiskProfile>,
    /// Markets.
    pub markets: BTreeMap<MarketId, Market>,
    /// Orders.
    pub orders: BTreeMap<OrderId, Order>,
    /// Every posted transaction, in the order it was posted.
    pub ledger: Vec<LedgerTransaction>,
    /// Quotes that were issued.
    pub quotes: BTreeMap<QuoteId, Quote>,
    /// Claimed idempotency keys.
    pub idempotency: BTreeMap<IdempotencyKey, Claim>,
    /// The platform-wide switch.
    pub kill_switch: KillSwitchState,
    /// The hash-chained audit trail.
    pub audit: Vec<AuditRecord>,
    /// The durable event log, in the order it was appended.
    ///
    /// Present because [`crate::ports::Store`] now hands out an
    /// [`EventLog`]. A fake without it would let a use case stage its events into
    /// a void, publish them, and pass — which is precisely the bug the
    /// transactional log exists to make impossible, so the fake has to be as
    /// strict as the real one about refusing to answer for events that were never
    /// written.
    pub events: Vec<StreamEvent>,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            accounts: BTreeSet::new(),
            balances: BTreeMap::new(),
            profiles: BTreeMap::new(),
            markets: BTreeMap::new(),
            orders: BTreeMap::new(),
            ledger: Vec::new(),
            quotes: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            // Running, so a test has to halt trading deliberately rather than
            // discovering on the first use case call that everything is refused.
            kill_switch: KillSwitchState::Running,
            audit: Vec::new(),
            events: Vec::new(),
        }
    }
}

/// A claimed idempotency key, and the answer if there is one yet.
#[derive(Debug, Clone)]
pub struct Claim {
    /// Who claimed it.
    pub account_id: AccountId,
    /// For which operation.
    pub endpoint: String,
    /// The stored answer, once the work is done.
    pub response: Option<StoredResponse>,
}

/// Shared state, plus the knobs a test needs.
pub struct InMemoryStoreFactory {
    state: Arc<Mutex<FakeState>>,
    /// Injected failures, consumed one per [`StoreFactory::begin`].
    ///
    /// How contention is tested at all. The fake gives each unit of work its own
    /// snapshot and merges at commit, so a genuine race between two of them
    /// cannot happen here; failing the *next* begin is the closest honest
    /// equivalent, and it is what lets a test assert that a use case retries
    /// rather than surfacing a conflict to a customer.
    pending_failures: Arc<Mutex<Vec<StoreError>>>,
    /// The timestamp written onto every audit entry.
    now_unix: AtomicI64,
}

impl std::fmt::Debug for InMemoryStoreFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryStoreFactory")
            .field("pending_failures", &self.queued_failures())
            .field("now_unix", &self.now())
            .finish_non_exhaustive()
    }
}

impl InMemoryStoreFactory {
    /// An empty database, with a clock reading the Unix epoch.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState::default())),
            pending_failures: Arc::new(Mutex::new(Vec::new())),
            now_unix: AtomicI64::new(0),
        }
    }

    /// Stamp audit entries with `now_unix` from now on.
    ///
    /// A field rather than a clock dependency because [`Store`] has nowhere to
    /// put one, and inventing a clock port for a fake's timestamps would be a
    /// production interface shaped by a test's convenience. Atomic rather than a
    /// plain `i64` only so that a test holding an `Arc` to the factory can reach
    /// it, which is how every use of it is written.
    pub fn set_now(&self, now_unix: i64) {
        self.now_unix.store(now_unix, Ordering::Relaxed);
    }

    /// The instant audit entries are stamped with.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.now_unix.load(Ordering::Relaxed)
    }

    /// Fail the next `begin`, and the one after that, and so on.
    pub fn fail_next(&self, error: StoreError) {
        self.pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(error);
    }

    /// How many injected failures are still queued.
    #[must_use]
    pub fn queued_failures(&self) -> usize {
        self.pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// The committed state.
    #[must_use]
    pub fn state(&self) -> FakeState {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Mutate the committed state directly, outside any unit of work.
    ///
    /// For arranging a starting position — a customer who exists, a market that is
    /// open — and, less happily, for corrupting something so that a check can be
    /// observed failing.
    pub fn edit(&self, f: impl FnOnce(&mut FakeState)) {
        f(&mut self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner));
    }

    /// Force the maintained balance projection to a value, without touching the
    /// ledger.
    ///
    /// The one way to make [`reconcile`](crate::usecases::reconciliation) fail,
    /// since a correct store can never produce that disagreement on its own.
    pub fn set_projected_balance(
        &self,
        account_id: &AccountId,
        currency: Currency,
        minor_units: i64,
    ) {
        self.edit(|state| {
            state
                .balances
                .insert((account_id.clone(), currency), minor_units);
        });
    }

    /// Add a customer with a balance, which is most of what a test needs.
    pub fn add_customer(&self, account_id: &AccountId, minor_units: i64, currency: Currency) {
        self.edit(|state| {
            state.accounts.insert(account_id.clone());
            state.balances.insert((account_id.clone(), currency), minor_units);
            state.profiles.insert(
                account_id.clone(),
                CustomerRiskProfile {
                    exposure_limit_minor_units: i64::MAX,
                },
            );
        });
    }

    /// Add a market.
    pub fn add_market(&self, market: Market) {
        self.edit(|state| {
            state.markets.insert(market.market_id.clone(), market);
        });
    }

    /// Add an order, as though it had already been placed and committed.
    pub fn add_order(&self, order: Order) {
        self.edit(|state| {
            state.orders.insert(order.id.clone(), order);
        });
    }
}

impl Default for InMemoryStoreFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StoreFactory for InMemoryStoreFactory {
    async fn begin(&self) -> Result<Box<dyn Store>, StoreError> {
        let mut pending = self
            .pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = pending.first().cloned() {
            pending.remove(0);
            return Err(error);
        }
        drop(pending);

        let snapshot = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Ok(Box::new(InMemoryStore {
            shared: Arc::clone(&self.state),
            staged: Arc::new(Mutex::new(Some(snapshot))),
            now_unix: self.now(),
            markets: None,
            orders: None,
            ledger: None,
            idempotency: None,
            risk: None,
            quotes: None,
            accounts: None,
            audit: None,
            events: None,
        }))
    }
}

/// One unit of work against the fake database.
pub struct InMemoryStore {
    shared: Arc<Mutex<FakeState>>,
    staged: Arc<Mutex<Option<FakeState>>>,
    now_unix: i64,
    markets: Option<Box<dyn MarketRepository>>,
    orders: Option<Box<dyn OrderRepository>>,
    ledger: Option<Box<dyn LedgerRepository>>,
    idempotency: Option<Box<dyn IdempotencyStore>>,
    risk: Option<Box<dyn RiskRepository>>,
    quotes: Option<Box<dyn QuoteRepository>>,
    accounts: Option<Box<dyn AccountRepository>>,
    audit: Option<Box<dyn AuditLog>>,
    events: Option<Box<dyn EventLog>>,
}

impl InMemoryStore {
    /// The staged state, or a clear failure if the unit of work is over.
    ///
    /// A repository can only be reached through the `&mut` borrow [`Store`] hands
    /// out, so a `None` here means the store was committed or rolled back and then
    /// used again — a bug in the caller, and one worth a message rather than an
    /// unwrap.
    fn staged_mut(&self) -> MutexGuard<'_, Option<FakeState>> {
        self.staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl Store for InMemoryStore {
    fn markets(&mut self) -> &mut dyn MarketRepository {
        // The `Arc` is cloned into a local first so that the immutable borrow of
        // `self.staged` has ended before `self.markets` is borrowed mutably.
        let staged = Arc::clone(&self.staged);
        &mut **self
            .markets
            .get_or_insert_with(|| Box::new(FakeMarketRepository { staged }))
    }

    fn orders(&mut self) -> &mut dyn OrderRepository {
        let staged = Arc::clone(&self.staged);
        &mut **self
            .orders
            .get_or_insert_with(|| Box::new(FakeOrderRepository { staged }))
    }

    fn ledger(&mut self) -> &mut dyn LedgerRepository {
        let staged = Arc::clone(&self.staged);
        &mut **self
            .ledger
            .get_or_insert_with(|| Box::new(FakeLedgerRepository { staged }))
    }

    fn idempotency(&mut self) -> &mut dyn IdempotencyStore {
        let staged = Arc::clone(&self.staged);
        &mut **self
            .idempotency
            .get_or_insert_with(|| Box::new(FakeIdempotencyStore { staged }))
    }

    fn risk(&mut self) -> &mut dyn RiskRepository {
        let staged = Arc::clone(&self.staged);
        &mut **self
            .risk
            .get_or_insert_with(|| Box::new(FakeRiskRepository { staged }))
    }

    fn quotes(&mut self) -> &mut dyn QuoteRepository {
        let staged = Arc::clone(&self.staged);
        &mut **self
            .quotes
            .get_or_insert_with(|| Box::new(FakeQuoteRepository { staged }))
    }

    fn accounts(&mut self) -> &mut dyn AccountRepository {
        let staged = Arc::clone(&self.staged);
        &mut **self
            .accounts
            .get_or_insert_with(|| Box::new(FakeAccountRepository { staged }))
    }

    fn audit(&mut self) -> &mut dyn AuditLog {
        // Read into a local first: the closure would otherwise capture `self`
        // while `self.audit` is mutably borrowed for the `get_or_insert_with`.
        let now_unix = self.now_unix;
        let staged = Arc::clone(&self.staged);
        &mut **self
            .audit
            .get_or_insert_with(|| Box::new(FakeAuditLog { staged, now_unix }))
    }

    fn events(&mut self) -> &mut dyn EventLog {
        // The log needs both the staged copy and the committed one: events are
        // appended into the transaction being built, but a read of the log has to
        // see everything already committed, including work from a unit of work
        // that finished earlier. The fake is the one place where that distinction
        // is visible; in production both are the same table.
        let staged = Arc::clone(&self.staged);
        let shared = Arc::clone(&self.shared);
        &mut **self
            .events
            .get_or_insert_with(|| Box::new(FakeEventLog { staged, shared }))
    }

    async fn commit(&mut self) -> Result<(), StoreError> {
        let staged = self.staged_mut().take().ok_or_else(|| {
            StoreError::Backend("commit called on a finished unit of work".to_string())
        })?;
        *self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = staged;
        Ok(())
    }

    async fn rollback(&mut self) -> Result<(), StoreError> {
        self.staged_mut().take();
        Ok(())
    }
}

/// Markets, in memory.
struct FakeMarketRepository {
    staged: Arc<Mutex<Option<FakeState>>>,
}

#[async_trait]
impl MarketRepository for FakeMarketRepository {
    async fn find(&self, market_id: &MarketId) -> Result<Option<Market>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard.as_ref().and_then(|s| s.markets.get(market_id).cloned()))
    }

    async fn lock(&mut self, market_id: &MarketId) -> Result<Market, StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        state.markets.get(market_id).cloned().ok_or_else(|| {
            StoreError::Missing {
                resource_type: "market",
                resource_id: market_id.as_str().to_string(),
            }
        })
    }

    async fn publish_odds(&mut self, market_id: &MarketId, odds: Odds) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let market = state.markets.get_mut(market_id).ok_or_else(|| {
            StoreError::Missing {
                resource_type: "market",
                resource_id: market_id.as_str().to_string(),
            }
        })?;
        market.published_odds = Some(odds);
        Ok(())
    }

    async fn set_state(&mut self, market_id: &MarketId, state: MarketState) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let inner = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let market = inner.markets.get_mut(market_id).ok_or_else(|| {
            StoreError::Missing {
                resource_type: "market",
                resource_id: market_id.as_str().to_string(),
            }
        })?;
        market.state = state;
        Ok(())
    }

    async fn resolve(
        &mut self,
        market_id: &MarketId,
        outcome: domain::settlement::Outcome,
    ) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let market = state.markets.get_mut(market_id).ok_or_else(|| {
            StoreError::Missing {
                resource_type: "market",
                resource_id: market_id.as_str().to_string(),
            }
        })?;

        // Both refusals are `Conflict` rather than `Backend`, so `with_retry`
        // treats a lost re-resolution race the way it treats any other one.
        if !matches!(market.state, MarketState::Open | MarketState::Suspended) {
            return Err(StoreError::Conflict {
                resource: "markets",
                detail: format!(
                    "market {} is {:?} and cannot be resolved",
                    market_id.as_str(),
                    market.state
                ),
            });
        }
        if market.outcome.is_some() {
            return Err(StoreError::Conflict {
                resource: "markets",
                detail: format!(
                    "market {} already has an outcome; resolving it again would pay \
                     twice for one event",
                    market_id.as_str()
                ),
            });
        }

        // State and outcome together, because `SettleMarket` requires both and a
        // market that is `Resolved` with no outcome is unpayable.
        market.state = MarketState::Resolved;
        market.outcome = Some(outcome);
        Ok(())
    }

    async fn list_by_state(&self, state: MarketState) -> Result<Vec<Market>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(inner) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(inner
            .markets
            .values()
            .filter(|market| market.state == state)
            .cloned()
            .collect())
    }
}

/// Accounts and their balances, in memory.
struct FakeAccountRepository {
    staged: Arc<Mutex<Option<FakeState>>>,
}

#[async_trait]
impl AccountRepository for FakeAccountRepository {
    async fn find_active(&self, account_id: &AccountId) -> Result<bool, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard
            .as_ref()
            .is_some_and(|s| s.accounts.contains(account_id)))
    }

    async fn balance_minor(
        &self,
        account_id: &AccountId,
        currency: Currency,
    ) -> Result<i64, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Zero rather than an error for an account that has never held a balance.
        // A missing row and a row holding zero are the same fact to every caller
        // in the system, and the production schema creates the row on first use.
        Ok(guard
            .as_ref()
            .and_then(|s| s.balances.get(&(account_id.clone(), currency)).copied())
            .unwrap_or(0))
    }

    async fn lock_balance(
        &mut self,
        account_id: &AccountId,
        currency: Currency,
    ) -> Result<i64, StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let balance = state
            .balances
            .entry((account_id.clone(), currency))
            .or_insert(0);
        Ok(*balance)
    }
}

/// Orders, in memory.
struct FakeOrderRepository {
    staged: Arc<Mutex<Option<FakeState>>>,
}

/// The error a domain refusal becomes.
///
/// A [`StoreError::Conflict`] rather than a `Backend`, because every one of these
/// refusals means "this row changed under you" or "that transition is not legal
/// from here", and the retry loop should treat it as a lost race rather than as an
/// outage to be paged on.
fn refusal(what: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Conflict {
        resource: "order",
        detail: format!("{what}: {detail}"),
    }
}

#[async_trait]
impl OrderRepository for FakeOrderRepository {
    async fn insert(&mut self, order: &Order) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        if state.orders.contains_key(&order.id) {
            return Err(StoreError::Conflict {
                resource: "order",
                detail: format!("order {} already exists", order.id.as_str()),
            });
        }
        state.orders.insert(order.id.clone(), order.clone());
        Ok(())
    }

    async fn pending_book(&self, market_id: &MarketId) -> Result<Vec<Order>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        // Sorted by id, not by time: an `Order` has no timestamp, and an
        // unsorted map would make "the same request against the same book"
        // produce a different fill on a different run. See the module
        // documentation.
        let mut book: Vec<Order> = state
            .orders
            .values()
            .filter(|order| {
                order.market_id == *market_id && order.status == OrderStatus::Pending
            })
            .cloned()
            .collect();
        book.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(book)
    }

    async fn mark_matched(
        &mut self,
        order_id: &OrderId,
        holding: &domain::ids::TransactionRef,
        filled_minor_units: i64,
    ) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let order = state.orders.get_mut(order_id).ok_or_else(|| {
            StoreError::Missing {
                resource_type: "order",
                resource_id: order_id.as_str().to_string(),
            }
        })?;
        order
            .mark_matched(holding.clone(), filled_minor_units)
            .map_err(|error| refusal("cannot mark matched", error))
    }

    async fn cancel(&mut self, order_id: &OrderId) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let order = state.orders.get_mut(order_id).ok_or_else(|| {
            StoreError::Missing {
                resource_type: "order",
                resource_id: order_id.as_str().to_string(),
            }
        })?;
        order
            .mark_cancelled()
            .map_err(|error| refusal("cannot cancel", error))
    }

    async fn settle(&mut self, order_id: &OrderId) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let order = state.orders.get_mut(order_id).ok_or_else(|| {
            StoreError::Missing {
                resource_type: "order",
                resource_id: order_id.as_str().to_string(),
            }
        })?;
        order
            .mark_settled()
            .map_err(|error| refusal("cannot settle", error))
    }

    async fn matched_orders(&self, market_id: &MarketId) -> Result<Vec<Order>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        // `Matched` is exactly the set of orders carrying a claim on the hold.
        // A partially filled order is `Matched` too — `mark_matched` moves an
        // order there for any fill at all — so nothing with money at risk is
        // excluded here, and nothing with no money at risk is included.
        let mut orders: Vec<Order> = state
            .orders
            .values()
            .filter(|order| {
                order.market_id == *market_id && order.status == OrderStatus::Matched
            })
            .cloned()
            .collect();
        orders.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(orders)
    }

    async fn cancel_resting(&mut self, market_id: &MarketId) -> Result<Vec<Order>, StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;

        // Collect first, then cancel. `mark_cancelled` borrows the order mutably
        // while the id is needed to find it, so the two cannot be interleaved, and
        // collecting up front also means the returned vec is exactly what changed
        // — which is what a test asserts on.
        let resting: Vec<OrderId> = state
            .orders
            .values()
            .filter(|order| order.market_id == *market_id && order.status == OrderStatus::Pending)
            .map(|order| order.id.clone())
            .collect();

        let mut voided = Vec::with_capacity(resting.len());
        for order_id in resting {
            let order = state.orders.get_mut(&order_id).ok_or_else(|| {
                StoreError::Missing {
                    resource_type: "order",
                    resource_id: order_id.as_str().to_string(),
                }
            })?;
            order
                .mark_cancelled()
                .map_err(|error| refusal("cannot cancel a resting order", error))?;
            voided.push(order.clone());
        }

        // The production adapter issues one `UPDATE ... RETURNING`; the fake sorts
        // so a test comparing the list against an expected one is not order-blind.
        voided.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(voided)
    }

    async fn list_for_account(&self, account_id: &AccountId) -> Result<Vec<Order>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let mut orders: Vec<Order> = state
            .orders
            .values()
            .filter(|order| order.account_id == *account_id)
            .cloned()
            .collect();
        // Newest first, by id descending, for the same reason the book is sorted.
        orders.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(orders)
    }

    async fn find(&self, order_id: &OrderId) -> Result<Option<Order>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard
            .as_ref()
            .and_then(|s| s.orders.get(order_id).cloned()))
    }
}

/// The ledger, in memory.
struct FakeLedgerRepository {
    staged: Arc<Mutex<Option<FakeState>>>,
}

#[async_trait]
impl LedgerRepository for FakeLedgerRepository {
    async fn post(&mut self, transaction: &LedgerTransaction) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;

        // Idempotency by key, the way the production unique index does it. Without
        // this, a test that retried a post would silently double it and the
        // reconciliation would report a balance nobody can explain.
        if let Some(key) = &transaction.idempotency_key {
            let already_posted = state
                .ledger
                .iter()
                .any(|existing| existing.idempotency_key.as_ref() == Some(key));
            if already_posted {
                return Ok(());
            }
        }

        // Refuse an unbalanced transaction here, exactly as the domain and the
        // database constraint both would. A fake that accepted one would let a
        // use case pass a test that creates money out of nothing.
        if transaction.total_minor_units() != 0 {
            return Err(StoreError::Backend(format!(
                "a ledger transaction must balance to zero, this one is off by {}",
                transaction.total_minor_units()
            )));
        }

        for leg in &transaction.legs {
            let balance = state
                .balances
                .entry((leg.account_id.clone(), transaction.currency))
                .or_insert(0);
            *balance = balance.saturating_add(leg.amount_minor_units);
        }
        state.ledger.push(transaction.clone());
        Ok(())
    }

    async fn balance_minor(&self, account_id: &AccountId) -> Result<i64, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(0);
        };
        Ok(ledger_balance(state, account_id))
    }

    async fn negative_accounts(&self) -> Result<Vec<(AccountId, i64)>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        Ok(ledger_balances(state)
            .into_iter()
            .filter(|(_, balance)| *balance < 0)
            .collect())
    }

    async fn reconcile_balances(&self) -> Result<Vec<BalanceDiscrepancy>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(Vec::new());
        };

        // Every account the ledger mentions, plus every account the projection
        // mentions. Comparing only the intersection would hide a row the trigger
        // failed to create, which is the failure this check exists for.
        let mut accounts: BTreeSet<AccountId> = ledger_balances(state).into_keys().collect();
        accounts.extend(
            state
                .balances
                .iter()
                .map(|((account_id, _currency), _balance)| account_id.clone()),
        );

        let mut discrepancies = Vec::new();
        for account_id in accounts {
            let from_ledger = ledger_balance(state, &account_id);
            let from_projection: i64 = state
                .balances
                .iter()
                .filter(|((id, _), _)| *id == account_id)
                .map(|(_, balance)| *balance)
                .sum();
            if from_ledger != from_projection {
                discrepancies.push(BalanceDiscrepancy {
                    account_id,
                    projected_minor_units: from_projection,
                    ledger_minor_units: from_ledger,
                });
            }
        }
        Ok(discrepancies)
    }

    async fn balance_accounts_compared(&self) -> Result<i64, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(0);
        };
        // The same union `reconcile_balances` iterates, so a test that breaks the
        // projection and then asserts on `accounts_compared` sees the real scan
        // size rather than a number that happens to be 0 because nothing
        // disagreed.
        let mut accounts: BTreeSet<AccountId> = ledger_balances(state).into_keys().collect();
        accounts.extend(
            state
                .balances
                .iter()
                .map(|((account_id, _currency), _balance)| account_id.clone()),
        );
        Ok(i64::try_from(accounts.len()).unwrap_or(i64::MAX))
    }
}

/// What the ledger says an account holds, in every currency.
fn ledger_balances(state: &FakeState) -> BTreeMap<AccountId, i64> {
    let mut balances: BTreeMap<AccountId, i64> = BTreeMap::new();
    for transaction in &state.ledger {
        for leg in &transaction.legs {
            let total = balances.entry(leg.account_id.clone()).or_insert(0);
            *total = total.saturating_add(leg.amount_minor_units);
        }
    }
    balances
}

/// What the ledger says one account holds.
fn ledger_balance(state: &FakeState, account_id: &AccountId) -> i64 {
    state
        .ledger
        .iter()
        .map(|transaction| transaction.balance_of(account_id))
        .fold(0_i64, i64::saturating_add)
}

/// Replay protection, in memory.
struct FakeIdempotencyStore {
    staged: Arc<Mutex<Option<FakeState>>>,
}

#[async_trait]
impl IdempotencyStore for FakeIdempotencyStore {
    async fn find(&self, key: &IdempotencyKey) -> Result<Option<StoredResponse>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard
            .as_ref()
            .and_then(|s| s.idempotency.get(key).and_then(|c| c.response.clone())))
    }

    async fn remember(
        &mut self,
        key: &IdempotencyKey,
        account_id: &AccountId,
        endpoint: &str,
        response: &StoredResponse,
    ) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        let claim = state.idempotency.entry(key.clone()).or_insert(Claim {
            account_id: account_id.clone(),
            endpoint: endpoint.to_string(),
            response: None,
        });
        if claim.account_id != *account_id || claim.endpoint != endpoint {
            return Err(StoreError::Conflict {
                resource: "idempotency key",
                detail: format!("key {key} belongs to another request"),
            });
        }
        claim.response = Some(response.clone());
        Ok(())
    }

    async fn claim(
        &mut self,
        key: &IdempotencyKey,
        account_id: &AccountId,
        endpoint: &str,
    ) -> Result<IdempotencyOutcome, StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        match state.idempotency.get(key).cloned() {
            None => {
                state.idempotency.insert(
                    key.clone(),
                    Claim {
                        account_id: account_id.clone(),
                        endpoint: endpoint.to_string(),
                        response: None,
                    },
                );
                Ok(IdempotencyOutcome::Claimed)
            }
            // A key claimed by a different customer or a different operation is a
            // client bug, and replaying it would be worse than failing.
            Some(existing)
                if existing.account_id != *account_id || existing.endpoint != endpoint =>
            {
                Ok(IdempotencyOutcome::Reused {
                    original_account_id: existing.account_id,
                    original_endpoint: existing.endpoint,
                })
            }
            Some(existing) => match existing.response {
                Some(response) => Ok(IdempotencyOutcome::Replay(response)),
                None => Ok(IdempotencyOutcome::InProgress),
            },
        }
    }
}

/// Risk configuration and the kill switch, in memory.
struct FakeRiskRepository {
    staged: Arc<Mutex<Option<FakeState>>>,
}

#[async_trait]
impl RiskRepository for FakeRiskRepository {
    async fn profile(
        &self,
        account_id: &AccountId,
    ) -> Result<CustomerRiskProfile, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard
            .as_ref()
            .and_then(|s| s.profiles.get(account_id).copied())
            .unwrap_or(CustomerRiskProfile {
                // No limit, rather than a refusal. A missing profile is a
                // configuration gap, and the use case under test is about
                // something else; a test that cares sets one explicitly.
                exposure_limit_minor_units: i64::MAX,
            }))
    }

    async fn exposure_minor(&self, account_id: &AccountId) -> Result<i64, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.as_ref() else {
            return Ok(0);
        };
        Ok(state
            .orders
            .values()
            .filter(|order| {
                order.account_id == *account_id
                    && matches!(order.status, OrderStatus::Pending | OrderStatus::Matched)
            })
            .filter_map(|order| order.filled_minor_units)
            .fold(0_i64, i64::saturating_add))
    }

    async fn kill_switch(&self) -> Result<KillSwitchState, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard
            .as_ref()
            .map_or(KillSwitchState::Running, |s| s.kill_switch))
    }

    async fn set_kill_switch(
        &mut self,
        state: KillSwitchState,
        _actor: &Actor,
        _reason: &str,
    ) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let inner = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        inner.kill_switch = state;
        Ok(())
    }
}

/// Quotes, in memory.
struct FakeQuoteRepository {
    staged: Arc<Mutex<Option<FakeState>>>,
}

#[async_trait]
impl QuoteRepository for FakeQuoteRepository {
    async fn insert(&mut self, id: &QuoteId, quote: &Quote) -> Result<(), StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;
        state.quotes.insert(id.clone(), quote.clone());
        Ok(())
    }
}

/// The durable event log, in memory.
///
/// Holds both the staged and the committed state because the two jobs are
/// different: an append belongs to the transaction being built and must vanish on
/// rollback, while a read is a subscriber catching up and has to see everything
/// already committed. In production both are the same table inside the same
/// transaction, and that is the property the fake is approximating — not the
/// two-lock arrangement, which is a consequence of the fake's copy-on-begin
/// model rather than a thing worth reproducing.
struct FakeEventLog {
    /// The transaction being built.
    staged: Arc<Mutex<Option<FakeState>>>,
    /// Everything that has been committed.
    shared: Arc<Mutex<FakeState>>,
}

#[async_trait]
impl EventLog for FakeEventLog {
    async fn append(&mut self, events: &[StreamEvent]) -> Result<Vec<StreamEvent>, StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;

        let mut appended = Vec::with_capacity(events.len());
        for event in events {
            // Positioned relative to what this transaction has already staged,
            // because the sequence is assigned by the insert and two events in one
            // append have to come out consecutive.
            let seq = i64::try_from(state.events.len() + 1).map_err(|_| {
                StoreError::Backend("the event log is longer than i64 can count".to_string())
            })?;
            let mut persisted = event.clone();
            persisted.seq = seq;
            state.events.push(persisted.clone());
            appended.push(persisted);
        }
        Ok(appended)
    }

    async fn read_after(&self, after_seq: i64, limit: i64) -> Result<Vec<StreamEvent>, StoreError> {
        let guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if limit <= 0 {
            return Ok(Vec::new());
        }
        // `try_from` rather than `as`: a limit above `i64::MAX` is nonsense input
        // and a saturating conversion would silently return a log-sized page.
        let limit = usize::try_from(limit).map_err(|_| {
            StoreError::Backend("the event log read limit is larger than usize".to_string())
        })?;
        Ok(guard
            .events
            .iter()
            .filter(|event| event.seq > after_seq)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn latest_seq(&self) -> Result<i64, StoreError> {
        let guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The last element, not the count: the two are the same only if positions
        // are dense, and a log that someone truncated is exactly the case where
        // assuming they are gives a subscriber a silently wrong resume point.
        Ok(guard.events.last().map_or(0, |event| event.seq))
    }
}

/// The audit trail, in memory.
struct FakeAuditLog {
    staged: Arc<Mutex<Option<FakeState>>>,
    /// The timestamp to stamp on entries appended through this unit of work.
    ///
    /// Copied in at construction rather than read from a clock, so every entry in
    /// one transaction carries the same instant — which is what the production
    /// implementation does too, and what makes a test's expected hash stable.
    now_unix: i64,
}

#[async_trait]
impl AuditLog for FakeAuditLog {
    async fn append(&mut self, entry: NewAuditEntry) -> Result<AuditRecord, StoreError> {
        let mut guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = guard
            .as_mut()
            .ok_or_else(|| StoreError::Backend("the unit of work is closed".to_string()))?;

        // Position and hash both come from the chain being staged, which is what
        // the mutex stands in for the head lock the port requires: no other append
        // can observe a head between this read and the commit.
        let seq = i64::try_from(state.audit.len() + 1).map_err(|_| {
            StoreError::Backend("the audit chain is longer than i64 can count".to_string())
        })?;
        let previous_hash = state
            .audit
            .last()
            .map_or_else(|| GENESIS_HASH.to_string(), |last| last.hash.clone());
        // Hashed before the entry is destructured below, so the hash covers every
        // field the caller supplied. Computing it afterwards would cover only the
        // part that had not been moved.
        let hash = crate::audit::entry_hash(seq, &previous_hash, self.now_unix, &entry);
        let record = AuditRecord {
            seq,
            action: entry.action,
            subject: entry.subject,
            actor: entry.actor,
            detail: entry.detail,
            occurred_at_unix: self.now_unix,
            previous_hash,
            hash,
        };
        state.audit.push(record.clone());
        Ok(record)
    }

    async fn read_chain(&self) -> Result<Vec<AuditRecord>, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard.as_ref().map(|s| s.audit.clone()).unwrap_or_default())
    }

    async fn head_seq(&self) -> Result<i64, StoreError> {
        let guard = self
            .staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(guard
            .as_ref()
            .and_then(|state| state.audit.last())
            .map_or(0, |record| record.seq))
    }

    async fn verify_chain(&self) -> Result<bool, StoreError> {
        let chain = self.read_chain().await?;
        // The same call the Postgres adapter makes, so a test that asserts an
        // intact chain is asserting the property production will check — not a
        // reimplementation of it that happens to agree today.
        let head = self.head_seq().await?;
        Ok(crate::audit::verify_chain(&chain, Some(head)).is_none())
    }
}

/// A clock that only moves when a test tells it to.
///
/// Defaults to the epoch rather than to a real time: a default-constructed
/// `SharedClock` reading a plausible current timestamp would make tests pass or
/// fail depending on the day they ran, and a test that only passes on Tuesdays is
/// not a test.
#[derive(Debug, Clone, Default)]
pub struct FixedClock(i64);

impl FixedClock {
    /// A clock reading `now`.
    #[must_use]
    pub const fn at(now: i64) -> Self {
        Self(now)
    }

    /// A clock reading zero, which is the Unix epoch and therefore obviously not
    /// a real time.
    #[must_use]
    pub const fn epoch() -> Self {
        Self(0)
    }

    /// Move the clock.
    pub const fn advance(&mut self, seconds: i64) {
        self.0 += seconds;
    }
}

impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        self.0
    }
}

/// A [`FixedClock`] a test can still move after the harness has taken its clock.
///
/// `TestHarness` keeps its clock as an `Arc<Mutex<FixedClock>>` so a test can
/// advance time mid-scenario, which means the value it hands to [`Deps`] cannot
/// be the bare `FixedClock` behind the lock. This is the bridge, and it exists
/// for that reason rather than because shared clock access needs its own type in
/// production.
#[derive(Debug, Clone, Default)]
pub struct SharedClock(Arc<Mutex<FixedClock>>);

impl SharedClock {
    /// A shared clock reading `now`.
    #[must_use]
    pub fn at(now: i64) -> Self {
        Self(Arc::new(Mutex::new(FixedClock::at(now))))
    }

    /// Move the clock forward.
    pub fn advance(&self, seconds: i64) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .advance(seconds);
    }
}

impl Clock for SharedClock {
    fn now_unix(&self) -> i64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .now_unix()
    }
}

/// A signer that produces a deterministic, checkable signature.
#[derive(Debug, Clone, Default)]
pub struct TestSigner {
    prefix: String,
}

impl TestSigner {
    /// A signer whose signatures are prefixed with `prefix`.
    #[must_use]
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

#[async_trait]
impl QuoteSigner for TestSigner {
    async fn sign(&self, signing_input: &str) -> Result<String, domain::error::DomainError> {
        Ok(format!("{}:{}", self.prefix, fake_digest(signing_input)))
    }

    async fn verify(
        &self,
        signing_input: &str,
        signature: &str,
    ) -> Result<bool, domain::error::DomainError> {
        Ok(signature == self.sign(signing_input).await?)
    }
}

/// A cheap, deterministic digest.
///
/// The same FNV-1a as [`chain_hash`], reached separately so that a test-only
/// signer never borrows the audit trail's hashing path: if a change to the audit
/// chain also changed what `TestSigner` emits, every signature test would move
/// with it and stop catching anything.
fn fake_digest(material: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in material.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// An in-memory [`EventPublisher`] that keeps what it was given.
#[derive(Debug, Clone, Default)]
pub struct RecordingPublisher {
    published: Arc<Mutex<Vec<StreamEvent>>>,
}

impl RecordingPublisher {
    /// A publisher with nothing recorded yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything published, in order.
    #[must_use]
    pub fn published(&self) -> Vec<StreamEvent> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How many events were published.
    #[must_use]
    pub fn count(&self) -> usize {
        self.published()
            .len()
    }
}

#[async_trait]
impl crate::events::EventPublisher for RecordingPublisher {
    async fn publish(&self, event: &StreamEvent) -> Result<(), crate::ports::StoreError> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.clone());
        Ok(())
    }
}

/// Everything a use case needs, wired to the fakes above.
pub struct TestHarness {
    /// The store factory to hand to [`Deps`].
    pub store: Arc<InMemoryStoreFactory>,
    /// The clock, so a test can move time deliberately.
    pub clock: SharedClock,
    /// The publisher, so a test can assert on what was announced.
    pub publisher: Arc<RecordingPublisher>,
    /// The assembled dependencies.
    pub deps: Arc<Deps>,
}

impl std::fmt::Debug for TestHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestHarness")
            .field("store", &self.store)
            .field("publisher", &self.publisher.count())
            .finish_non_exhaustive()
    }
}

impl TestHarness {
    /// A harness with a real HMAC signer and a clock reading the given instant.
    ///
    /// The signer is the production one on purpose. A test double that produced
    /// its own signature format would pass even if [`Quote::signing_input`]
    /// changed underneath it, and the signature is the one thing standing between
    /// a customer and a price the server never offered.
    ///
    /// The key is a constant, and the comment on it is the point: a signature
    /// checked against a key the test chose is a signature checked against
    /// nothing. It also keeps the fixtures reproducible, which a random key
    /// would not. [`HmacSigner`] refuses a key under 32 bytes, so the literal is
    /// padded past that rather than being a passphrase that happens to be short.
    #[must_use]
    pub fn new(now_unix: i64) -> Self {
        let store = Arc::new(InMemoryStoreFactory::new());
        store.set_now(now_unix);
        let clock = SharedClock::at(now_unix);
        let publisher = Arc::new(RecordingPublisher::new());
        let signer = HmacSigner::new(b"ladywalk-test-signing-key-not-a-secret")
            .expect("a fixed key of at least 32 bytes");
        let deps = Arc::new(Deps {
            store: Arc::clone(&store) as Arc<dyn StoreFactory>,
            signer: Arc::new(signer),
            clock: Arc::new(clock.clone()),
            ids: Arc::new(Mutex::new(UuidGenerator::new("id"))),
            publisher: Arc::clone(&publisher) as Arc<dyn crate::events::EventPublisher>,
            quote_ttl_secs: 30,
            // Zero commission, so a test about the exchange's own behaviour does
            // not have to reason about the platform taking a cut. A test about
            // commission sets it.
            commission_bps: 0,
            default_exposure_limit_minor: 1_000_000,
        });
        Self {
            store,
            clock,
            publisher,
            deps,
        }
    }

    /// A harness whose clock is at the epoch, for tests that do not care.
    #[must_use]
    pub fn at_epoch() -> Self {
        Self::new(0)
    }

    /// Move the harness's clock, and the audit timestamps it drives with it.
    ///
    /// Both clocks move together, and they have to. Audit entries are stamped
    /// inside the store's transaction — they are part of the durable record, not a
    /// side effect of the use case — so the fake stamps them from its own counter
    /// rather than from `deps.clock`. Advancing only the harness clock would give
    /// a test an event at T+60 next to an audit entry at T+0, which is precisely
    /// the kind of thing a real clock skew report exists to catch and precisely
    /// the kind of thing a test should never manufacture for itself.
    pub fn advance(&self, seconds: i64) {
        self.clock.advance(seconds);
        self.store.set_now(self.now());
    }

    /// The clock's current reading.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.clock.now_unix()
    }

    /// A market in the given state, for arranging a starting position.
    ///
    /// Panics if asked for a state the production system cannot reach, which is
    /// the point: a `Resolved` market with no outcome is representable in this
    /// struct's two independent fields, but [`MarketRepository::resolve`] always
    /// writes them together, so no real database has ever held one. Building one
    /// here would produce a test that fails somewhere inside settlement with an
    /// internal error, a long way from the line that arranged it. Use
    /// [`Self::resolved_market`] for that state instead.
    #[must_use]
    pub fn market(market_id: &str, state: MarketState, odds: Option<Odds>) -> Market {
        assert!(
            !matches!(state, MarketState::Resolved | MarketState::Settled),
            "a resolved or settled market needs an outcome, which this fixture has \
             nowhere to put; use TestHarness::resolved_market"
        );
        Market {
            market_id: MarketId::new(market_id).expect("a valid market id in a test fixture"),
            state,
            published_odds: odds,
            currency: Currency::GBP,
            outcome: None,
        }
    }

    /// A market that has resolved to `outcome` and is waiting to be settled.
    ///
    /// The state settlement actually consumes, and the one no other fixture can
    /// build. Resolution and settlement are separate on purpose — a resolved
    /// market is a customer-visible promise that money is owed, and it is
    /// operationally different from a market whose payouts have posted — so the
    /// boundary is worth arranging deliberately rather than by side effect.
    #[must_use]
    pub fn resolved_market(
        market_id: &str,
        outcome: Outcome,
        odds: Option<Odds>,
    ) -> Market {
        Market {
            market_id: MarketId::new(market_id).expect("a valid market id in a test fixture"),
            state: MarketState::Resolved,
            published_odds: odds,
            currency: Currency::GBP,
            outcome: Some(outcome),
        }
    }

    /// The committed state.
    #[must_use]
    pub fn state(&self) -> FakeState {
        self.store.state()
    }
}
