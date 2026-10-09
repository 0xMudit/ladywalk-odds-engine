//! # Use cases
//!
//! The behaviour. Each module here is one thing a caller can ask the exchange
//! to do, written against [`crate::ports`] and against `domain`.
//!
//! ## The shape of every use case
//!
//! They are all built the same way, and deliberately so:
//!
//! ```text
//! deps ──▶ begin a unit of work ──▶ check the cheap things ──▶ do the work
//!        ──▶ write the audit entry ──▶ stage the events ──▶ commit
//!        ──▶ publish what was staged
//! ```
//!
//! Four properties of that sequence are worth naming, because each one is a
//! decision rather than a habit.
//!
//! **The unit of work is a dependency, not a local variable.** Every use case
//! takes a [`StoreFactory`] rather than a `Store`, which is what lets
//! [`crate::ports::with_retry`] re-run the body after losing a race. Two
//! customers placing orders on the same market collide by design, and the loser
//! of that collision is supposed to try again, not to be told the database is
//! unavailable.
//!
//! **The audit entry is written before the commit, in the same transaction as
//! the change.** An audit trail that is written afterwards is an audit trail
//! that is missing exactly the entries that matter, because those are the ones
//! where something failed.
//!
//! **Events are staged before the commit, and published after it.** Staging puts
//! the event in the same transaction as the change it describes, so no subscriber
//! can be told about a match that rolls back and no committed match goes
//! unrecorded. Publishing happens after, because a broadcast is a courtesy to
//! whoever is connected and the log is the record. [`stage`] and [`announce`]
//! exist so that the ordering is a function rather than a convention — the
//! mistake is invisible in a happy-path test and catastrophic in production.
//!
//! **The sequence numbers come from the log, not from the use case.** An event is
//! built with [`crate::events::StreamEvent::seq`] at zero, meaning "not yet
//! written"; [`stage`] returns it with the position the database assigned, and
//! that is what goes out. A use case that published its own hand-built event
//! would announce position zero for everything, and a subscriber could no longer
//! tell whether it had missed anything.
//!
//! ## Where the rules actually are
//!
//! Not here. Whether a stake is legal, what a payout is, and whether a
//! transaction balances are all decided in `domain`, and the use cases exist only
//! to sequence those decisions and to make sure they are all made inside one
//! transaction. A rule that appears in this layer as well as in `domain` is a
//! rule that will eventually be enforced in one place and not the other.

pub mod markets;
pub mod orders;
pub mod quotes;
pub mod reconciliation;
pub mod settlement;
pub mod wallet;

use std::sync::Arc;

use domain::ids::MarketId;
use domain::risk::KillSwitchState;

use crate::errors::AppError;
use crate::events::EventPublisher;
use crate::ids::UuidGenerator;
use crate::ports::{Clock, IdGenerator, QuoteSigner, StoreFactory, StoreError};

/// Everything a use case needs, gathered once.
///
/// A struct rather than six constructor arguments because the argument list of
/// an eight-parameter constructor is unreadable at the call site and impossible
/// to get right in the right order. It is `Clone` so a use case can be handed a
/// copy, and the `Arc`s mean that cloning shares the one connection pool and the
/// one signing key rather than making a second of each.
#[derive(Clone)]
pub struct Deps {
    /// Where units of work come from.
    pub store: Arc<dyn StoreFactory>,
    /// How prices are signed.
    pub signer: Arc<dyn QuoteSigner>,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// Where new ids come from.
    pub ids: Arc<std::sync::Mutex<UuidGenerator>>,
    /// How committed events reach subscribers.
    pub publisher: Arc<dyn EventPublisher>,
    /// How long a quote stays valid, in seconds.
    pub quote_ttl_secs: i64,
    /// The commission charged on net winnings, in basis points.
    pub commission_bps: i64,
    /// The default per-currency exposure limit for a new customer, in minor
    /// units.
    pub default_exposure_limit_minor: i64,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately not derived: the fields are trait objects behind `Arc`,
        // and a derived `Debug` would either not exist or print the vtable.
        f.debug_struct("Deps")
            .field("quote_ttl_secs", &self.quote_ttl_secs)
            .field("commission_bps", &self.commission_bps)
            .field("default_exposure_limit_minor", &self.default_exposure_limit_minor)
            .finish_non_exhaustive()
    }
}

impl Deps {
    /// Borrow the id generator for the duration of one call.
    ///
    /// The generator mutates a counter, so it is behind a mutex; taking the lock
    /// inside a helper keeps the lock's scope to the single id allocation rather
    /// than spanning a transaction, which would serialise every concurrent order
    /// behind one mutex for no benefit — the counter is the only shared state.
    pub fn with_ids<T>(&self, f: impl FnOnce(&mut UuidGenerator) -> T) -> T {
        let mut guard = self
            .ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    /// A transaction reference for a named operation.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError::Backend`] if the reference cannot be built.
    pub fn transaction_ref(
        &self,
        prefix: &str,
    ) -> Result<domain::ids::TransactionRef, StoreError> {
        self.with_ids(|generator| generator.new_transaction_ref(prefix))
    }
}

/// The kill switch and the market's own state, read once so that a use case
/// cannot be told two different things by two reads.
///
/// Both are read inside the unit of work, and the market row is locked first, so
/// a market suspended by an operator a millisecond ago is seen as suspended by
/// this request rather than as open.
pub(crate) async fn ensure_trading_permitted(
    store: &mut Box<dyn crate::ports::Store>,
    market_id: &MarketId,
) -> Result<(), AppError> {
    use crate::ports::MarketState;

    // The market row is locked *before* the switch is read, so an operator
    // suspending a market and a customer placing an order are serialised: one
    // of them wins, and the loser acts on the winner's state rather than on
    // a snapshot taken between them. Reading the switch first would leave a
    // window where an order is accepted against a market that was suspended a
    // millisecond earlier.
    let market = store.markets().lock(market_id).await?;
    let kill_switch = store.risk().kill_switch().await?;

    if kill_switch == KillSwitchState::Halted {
        return Err(domain::error::DomainError::BettingHalted.into());
    }
    if market.state != MarketState::Open {
        return Err(domain::error::DomainError::MarketClosed.into());
    }
    Ok(())
}

/// Write events into the open transaction and hand back what was persisted.
///
/// This is the first half of [`announce`], and the two are always used together
/// because the ordering between them is the entire contract:
///
/// 1. Build events with [`crate::events::StreamEvent::seq`] left at `0`, meaning
///    "not yet persisted".
/// 2. `stage` them, which writes them in this transaction and returns them with
///    the positions the database actually assigned.
/// 3. [`crate::ports::Store::commit`].
/// 4. `announce` the returned events.
///
/// Skipping step 2 and publishing a hand-built event with `seq: 0` produces a
/// stream where every message says "I am at position zero", which a subscriber
/// cannot use to detect the gap it is currently in. Skipping step 3 or doing it
/// in the other order produces a stream that announces changes which then roll
/// back. Neither mistake is visible in a unit test that only checks the happy
/// path, which is why the pair is a function here rather than a convention.
///
/// # Errors
///
/// Returns [`AppError::Store`] if the events cannot be written, aborting the
/// surrounding transaction.
pub(crate) async fn stage(
    store: &mut Box<dyn crate::ports::Store>,
    events: Vec<crate::events::StreamEvent>,
) -> Result<Vec<crate::events::StreamEvent>, AppError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    Ok(store.events().append(&events).await?)
}

/// Tell live subscribers about events that are already committed.
///
/// Best effort by design. A committed change is never rolled back because nobody
/// was listening to it — the event is durable in the log, and a subscriber that
/// reconnects reads it from there. The cost of a failure here is delayed delivery,
/// so it is a warning rather than a failed request.
///
/// Takes the slice [`stage`] returned rather than the caller's original events,
/// so that what goes out on the wire is exactly what was written, with a real
/// sequence number on it.
pub(crate) async fn announce(deps: &Deps, events: &[crate::events::StreamEvent]) {
    for event in events {
        if let Err(error) = deps.publisher.publish(event).await {
            tracing::warn!(
                error = %error,
                kind = event.kind_label(),
                seq = event.seq,
                "committed a change but could not publish it; subscribers will \
                 read it from the log on reconnect"
            );
        }
    }
}
