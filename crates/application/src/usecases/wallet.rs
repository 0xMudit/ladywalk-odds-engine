//! # The wallet
//!
//! Reading a balance, and moving money into or out of the system.
//!
//! ## Deposits and withdrawals are the platform's own double entry
//!
//! A matched bet is a transfer between two customers, and both legs are already
//! accounted for: the taker is debited, the hold is credited, and the sum is
//! zero without the platform appearing anywhere. That is what makes
//! [ADR-0001]'s exchange model work, and it has a consequence here — a deposit
//! is the one movement that is *not* a transfer between two customers, because
//! the money arrives from outside the ledger.
//!
//! So a deposit needs a counter-account, and the choice is [`PLATFORM_DEPOSITS`]
//! rather than the revenue account. The distinction is not cosmetic:
//!
//! - `platform:deposits` is money the platform **holds on the customer's
//!   behalf**. It is a liability in the accounting sense, and it is what makes
//!   the reconciliation identity `customer balances + holds + revenue +
//!   deposits = 0` hold.
//! - `platform:revenue` is money the platform has **earned**, and it is only
//!   ever credited by settlement.
//!
//! Using the revenue account for deposits would balance the ledger and be
//! completely wrong: a reconciliation would then report the platform as having
//! earned every pound a customer ever deposited. Note that this *is* the
//! liability account [ADR-0001] warns about — the one that must never fund a
//! payout — and it is used here for the opposite purpose. It holds customer money
//! in transit and is drained by withdrawals, never by settlement, which draws
//! only on `hold:<market>`.
//!
//! ## Locking order
//!
//! Every movement here debits one account and credits another, so it needs both
//! balance rows locked before either is read. The two are locked in a fixed
//! order — sorted by account id, not in the order the transaction happens to
//! name them — because two movements that touch the same pair of accounts in
//! opposite orders is the textbook deadlock, and on a wallet it is a deadlock
//! between two customers withdrawing at once.
//!
//! [`lock_both`] is the single place that does this, so that a future use case
//! cannot get it wrong by locking in the natural order.
//!
//! ## What is not here
//!
//! A real deposit is a payment-provider callback, verified against a webhook
//! signature, reconciled against a bank statement, and settled on a schedule.
//! None of that is implemented, and this module does not pretend to. It moves
//! money in response to a *direct authenticated call* from an operator, which is
//! enough to make the ledger correct and the balance projection true, and not
//! enough to be a payment system. The [`EntryReason`] written is `Deposit` and
//! `Withdrawal` rather than something that claims a card was charged.
//!
//! [ADR-0001]: ../../../adr/0001-peer-to-peer-exchange.md

use std::sync::Arc;

use domain::ids::{AccountId, IdempotencyKey};
use domain::ledger::{EntryReason, LedgerLeg, LedgerTransaction};
use domain::money::Currency;
use domain::risk::KillSwitchState;

use crate::errors::AppError;
use crate::events::{EventKind, StreamEvent};
use crate::ports::{Actor, IdGenerator, IdempotencyOutcome, Store, with_retry};
use crate::usecases::Deps;
use crate::usecases::{announce, stage};

/// The account that holds customer money which has arrived but not yet been bet,
/// and which has left but not yet settled on the provider's side.
///
/// See the module documentation for why this is not the revenue account.
pub const PLATFORM_DEPOSITS: &str = "platform:deposits";

/// The counter-account for a movement across the platform boundary.
///
/// # Errors
///
/// Returns [`AppError::Domain`] with `Internal` if the constant is not a valid
/// identifier, which would be a compile-time constant that does not compile —
/// caught here so that a future edit to the constant cannot make it a runtime
/// failure on a money path.
fn deposits_account() -> Result<AccountId, AppError> {
    AccountId::new(PLATFORM_DEPOSITS).map_err(|e| {
        AppError::Domain(domain::error::DomainError::internal_with_context(
            "the deposits account constant is not a valid identifier",
            e,
        ))
    })
}

/// Lock both balance rows, in a deterministic order.
///
/// Sorted by account id rather than locked in the order the caller names them.
/// Two customers withdrawing at once, from each other's side of a pair, would
/// otherwise each hold the row the other wants and neither would proceed; sorting
/// makes one of them win and the other wait, which is the outcome a user
/// experiences as "it was busy for a moment" rather than as a hung request.
///
/// Returns the two locked balances in the caller's own order, so the caller does
/// not have to remember which sorted position was theirs.
async fn lock_both(
    store: &mut Box<dyn Store>,
    first: &AccountId,
    second: &AccountId,
    currency: Currency,
) -> Result<(i64, i64), AppError> {
    let (low, high) = if first.as_str() <= second.as_str() {
        (first, second)
    } else {
        (second, first)
    };

    let low_balance = store.accounts().lock_balance(low, currency).await?;
    let high_balance = store.accounts().lock_balance(high, currency).await?;

    if low.as_str() == first.as_str() {
        Ok((low_balance, high_balance))
    } else {
        Ok((high_balance, low_balance))
    }
}

/// A customer's spendable balance.
///
/// `Serialize` and `Deserialize` because this exact struct is what is stored
/// against the idempotency key: the bytes a replay returns are the bytes the
/// first caller was given, so a retry of a deposit cannot be answered with a
/// different balance than the original deposit produced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalletBalance {
    /// Whose balance this is.
    pub account_id: AccountId,
    /// The currency.
    pub currency: Currency,
    /// How much is available, in minor units.
    pub available_minor: i64,
}

impl WalletBalance {
    /// The balance as it would be stored against an idempotency key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Domain`] with `Internal` if serialisation fails, which
    /// for a struct of integers cannot happen; it is a `Result` because a `?
    /// that could not fail is a `?` a future field could make fail, and the
    /// failure mode of a silent fallback is a replayed balance of zero.
    fn to_stored_body(&self) -> Result<String, AppError> {
        serde_json::to_string(self).map_err(|e| {
            AppError::Domain(domain::error::DomainError::internal_with_context(
                "could not serialise a wallet balance",
                e,
            ))
        })
    }
}

/// Read a stored balance back.
///
/// # Errors
///
/// Returns [`AppError::Domain`] with `Internal` if the body will not parse. That
/// means the response format changed under a record which is meant to be
/// replayed forever, so the honest answer is a loud failure: a zero balance
/// returned as a success would tell a customer they have no money.
fn decode_balance(body: &str) -> Result<WalletBalance, AppError> {
    serde_json::from_str(body).map_err(|e| {
        AppError::Domain(domain::error::DomainError::internal_with_context(
            "a stored idempotent response for a wallet movement could not be read back, \
             which means the response format changed under a persisted record",
            e,
        ))
    })
}

/// A customer's spendable balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceRequest {
    /// Whose balance to read.
    pub account_id: AccountId,
    /// In which currency.
    pub currency: Currency,
}

/// Read a balance.
///
/// # Errors
///
/// - [`AppError::NotFound`] if the account does not exist or is closed.
/// - [`AppError::Store`] if the balance cannot be read.
pub async fn balance(deps: &Deps, request: &BalanceRequest) -> Result<WalletBalance, AppError> {
    let mut store = deps.store.begin().await?;
    if !store.accounts().find_active(&request.account_id).await? {
        return Err(AppError::NotFound {
            resource_type: "account",
            resource_id: request.account_id.as_str().to_string(),
        });
    }
    let available_minor = store
        .accounts()
        .balance_minor(&request.account_id, request.currency)
        .await?;
    store.rollback().await?;
    Ok(WalletBalance {
        account_id: request.account_id.clone(),
        currency: request.currency,
        available_minor,
    })
}

/// A request to credit an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositRequest {
    /// Who is paying.
    pub account_id: AccountId,
    /// How much arrived.
    pub amount_minor: i64,
    /// The currency it arrived in.
    pub currency: Currency,
    /// The key that makes a retry of this deposit safe.
    ///
    /// Required rather than generated, because the thing that must be safe to
    /// retry is *the provider's* request, and a key generated here would change
    /// on every attempt and therefore deduplicate nothing.
    pub idempotency_key: IdempotencyKey,
    /// A free-form reference for the caller to record, e.g. a provider id.
    pub external_reference: String,
}

/// Credit an account from the platform's deposits account.
///
/// # Errors
///
/// - [`AppError::Forbidden`] if a *customer* is calling this. Crediting an
///   account is not something a customer can do to themselves.
/// - [`AppError::Domain`] with `InvalidInput` if the amount is not positive.
/// - [`AppError::Domain`] with `BettingHalted` if the kill switch is thrown. A
///   deposit during a halt is refused: the switch exists to stop money moving,
///   and a deposit that is accepted and then unusable helps nobody.
/// - [`AppError::NotFound`] if the account does not exist.
/// - [`AppError::RequestInProgress`] if the same deposit is still running.
/// - [`AppError::Store`] on any storage failure.
pub async fn deposit(
    deps: &Deps,
    actor: &Actor,
    request: &DepositRequest,
) -> Result<WalletBalance, AppError> {
    if !actor.is_operator() {
        return Err(AppError::Forbidden {
            action: "credit an account",
        });
    }
    if request.amount_minor <= 0 {
        return Err(domain::error::DomainError::InvalidInput {
            field: "amount_minor",
            reason: "a deposit must be for a positive amount".to_string(),
        }
        .into());
    }

    let owned = Arc::new(deps.clone());
    let factory = Arc::clone(&owned.store);
    let request = request.clone();
    let actor = actor.clone();

    with_retry(factory, |factory| {
        let deps = Arc::clone(&owned);
        let request = request.clone();
        let actor = actor.clone();
        async move {
            let mut store = factory.begin().await?;
            match store
                .idempotency()
                .claim(
                    &request.idempotency_key,
                    // Scoped to the account being credited, not the operator
                    // making the deposit: a key belongs to the customer whose
                    // balance it protects, so an operator retrying the same
                    // deposit replays, while the same key for a different
                    // customer is a reuse the server refuses.
                    &request.account_id,
                    "POST /v1/wallet/deposit",
                )
                .await?
            {
                IdempotencyOutcome::Replay(response) => {
                    return decode_balance(&response.body);
                }
                IdempotencyOutcome::InProgress => return Err(AppError::RequestInProgress),
                IdempotencyOutcome::Reused { .. } => {
                    return Err(AppError::IdempotencyKeyReused {
                        key: request.idempotency_key.as_str().to_string(),
                    });
                }
                IdempotencyOutcome::Claimed => {}
            }

            if store.risk().kill_switch().await? == KillSwitchState::Halted {
                return Err(AppError::Domain(
                    domain::error::DomainError::BettingHalted,
                ));
            }
            if !store.accounts().find_active(&request.account_id).await? {
                return Err(AppError::NotFound {
                    resource_type: "account",
                    resource_id: request.account_id.as_str().to_string(),
                });
            }

            let counter = deposits_account()?;

            // Both rows locked before either balance is read. The counter's own
            // balance is discarded: a deposit credits the customer and debits
            // the liability account, and neither side may move underneath the
            // other.
            let (available_minor, _counter) =
                lock_both(&mut store, &request.account_id, &counter, request.currency).await?;

            let reference = deps
                .with_ids(|generator| generator.new_transaction_ref("deposit"))
                .map_err(AppError::Store)?;
            let transaction = LedgerTransaction::new(
                reference,
                request.currency,
                EntryReason::Deposit,
                Some(request.idempotency_key.clone()),
                vec![
                    LedgerLeg::new(counter, -request.amount_minor)?,
                    LedgerLeg::new(request.account_id.clone(), request.amount_minor)?,
                ],
            )?;
            store.ledger().post(&transaction).await?;

            // The reported balance is the one *after* the credit. Reporting the
            // pre-deposit figure would make a customer's first deposit look
            // like it had not landed, and would put a wrong number into an audit
            // entry that is supposed to be a record of the transaction just made.
            let result = WalletBalance {
                account_id: request.account_id.clone(),
                currency: request.currency,
                available_minor: available_minor
                    .checked_add(request.amount_minor)
                    .ok_or_else(|| {
                        AppError::Domain(domain::error::DomainError::internal_with_context(
                            "a deposit overflowed the customer's balance",
                            "the balance is already i64::MAX",
                        ))
                    })?,
            };

            store
                .audit()
                .append(crate::ports::NewAuditEntry {
                    action: "wallet.deposited".to_string(),
                    subject: request.account_id.as_str().to_string(),
                    actor: actor.account_id.as_str().to_string(),
                    detail: serde_json::json!({
                        "amount_minor": request.amount_minor,
                        "currency": request.currency.to_string(),
                        "balance_after_minor": result.available_minor,
                        "external_reference": request.external_reference,
                    })
                    .to_string(),
                })
                .await?;
            remember(
                &mut store,
                &request.idempotency_key,
                // The same account the key was claimed under: claim and remember
                // must agree, or a legitimate deposit cannot record its answer.
                &request.account_id,
                "POST /v1/wallet/deposit",
                &result,
            )
            .await?;
            let staged = stage(
                &mut store,
                vec![StreamEvent {
                    seq: 0,
                    kind: EventKind::FundsDeposited {
                        account_id: request.account_id.as_str().to_string(),
                        amount_minor_units: request.amount_minor,
                        currency: request.currency.into(),
                    },
                    occurred_at_unix: deps.clock.now_unix(),
                }],
            )
            .await?;
            store.commit().await?;

            announce(&deps, &staged).await;
            Ok(result)
        }
    })
    .await
}

/// A request to debit an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawRequest {
    /// Whose balance to take from.
    pub account_id: AccountId,
    /// How much to pay out.
    pub amount_minor: i64,
    /// The currency to pay in.
    pub currency: Currency,
    /// The key that makes a retry safe.
    pub idempotency_key: IdempotencyKey,
    /// Where the money is going, for the audit trail and for a human to check.
    ///
    /// Required. A withdrawal with no recorded destination is money that left a
    /// betting platform and cannot be traced to anywhere, which is the single
    /// most useful thing an investigator needs and the easiest thing to omit.
    pub destination: String,
}

/// Debit an account into the platform's deposits account.
///
/// # Errors
///
/// - [`AppError::Forbidden`] if `request.account_id` is not the account the
///   request was authenticated as.
/// - [`AppError::Domain`] with `InvalidInput` if the amount is not positive or
///   the destination is blank.
/// - [`AppError::Domain`] with `BettingHalted` if the kill switch is thrown.
/// - [`AppError::Domain`] with `InsufficientFunds` if the balance is short. The
///   `account_balances` constraint is the authority on this; the check here
///   exists so the customer gets a clear message rather than a constraint
///   violation reported as an internal error.
/// - [`AppError::NotFound`] if the account does not exist.
/// - [`AppError::RequestInProgress`] if the same withdrawal is still running.
/// - [`AppError::Store`] on any storage failure.
pub async fn withdraw(
    deps: &Deps,
    authenticated: &AccountId,
    request: &WithdrawRequest,
) -> Result<WalletBalance, AppError> {
    if request.amount_minor <= 0 {
        return Err(domain::error::DomainError::InvalidInput {
            field: "amount_minor",
            reason: "a withdrawal must be for a positive amount".to_string(),
        }
        .into());
    }
    if request.destination.trim().is_empty() {
        return Err(domain::error::DomainError::InvalidInput {
            field: "destination",
            reason: "a withdrawal must record where the money is going".to_string(),
        }
        .into());
    }
    if &request.account_id != authenticated {
        return Err(AppError::Forbidden {
            action: "withdraw from another account",
        });
    }

    let owned = Arc::new(deps.clone());
    let factory = Arc::clone(&owned.store);
    let request = request.clone();
    let account_id = authenticated.clone();

    with_retry(factory, |factory| {
        let deps = Arc::clone(&owned);
        let request = request.clone();
        let account_id = account_id.clone();
        async move {
            let mut store = factory.begin().await?;
            match store
                .idempotency()
                .claim(
                    &request.idempotency_key,
                    &account_id,
                    "POST /v1/wallet/withdraw",
                )
                .await?
            {
                IdempotencyOutcome::Replay(response) => {
                    return decode_balance(&response.body);
                }
                IdempotencyOutcome::InProgress => return Err(AppError::RequestInProgress),
                IdempotencyOutcome::Reused { .. } => {
                    return Err(AppError::IdempotencyKeyReused {
                        key: request.idempotency_key.as_str().to_string(),
                    });
                }
                IdempotencyOutcome::Claimed => {}
            }

            if store.risk().kill_switch().await? == KillSwitchState::Halted {
                return Err(AppError::Domain(
                    domain::error::DomainError::BettingHalted,
                ));
            }
            if !store.accounts().find_active(&request.account_id).await? {
                return Err(AppError::NotFound {
                    resource_type: "account",
                    resource_id: request.account_id.as_str().to_string(),
                });
            }

            let counter = deposits_account()?;
            let (available_minor, _counter) =
                lock_both(&mut store, &request.account_id, &counter, request.currency).await?;

            if available_minor < request.amount_minor {
                return Err(AppError::Domain(
                    domain::error::DomainError::InsufficientFunds {
                        available_minor,
                        requested_minor: request.amount_minor,
                    },
                ));
            }

            let reference = deps
                .with_ids(|generator| generator.new_transaction_ref("withdraw"))
                .map_err(AppError::Store)?;
            let transaction = LedgerTransaction::new(
                reference,
                request.currency,
                EntryReason::Withdrawal,
                Some(request.idempotency_key.clone()),
                vec![
                    LedgerLeg::new(request.account_id.clone(), -request.amount_minor)?,
                    LedgerLeg::new(counter, request.amount_minor)?,
                ],
            )?;
            store.ledger().post(&transaction).await?;

            let result = WalletBalance {
                account_id: request.account_id.clone(),
                currency: request.currency,
                available_minor: available_minor - request.amount_minor,
            };

            store
                .audit()
                .append(crate::ports::NewAuditEntry {
                    action: "wallet.withdrawn".to_string(),
                    subject: request.account_id.as_str().to_string(),
                    actor: account_id.as_str().to_string(),
                    detail: serde_json::json!({
                        "amount_minor": request.amount_minor,
                        "currency": request.currency.to_string(),
                        "balance_after_minor": result.available_minor,
                        "destination": request.destination,
                    })
                    .to_string(),
                })
                .await?;
            remember(
                &mut store,
                &request.idempotency_key,
                &account_id,
                "POST /v1/wallet/withdraw",
                &result,
            )
            .await?;
            let staged = stage(
                &mut store,
                vec![StreamEvent {
                    seq: 0,
                    kind: EventKind::FundsWithdrawn {
                        account_id: request.account_id.as_str().to_string(),
                        amount_minor_units: request.amount_minor,
                        currency: request.currency.into(),
                    },
                    occurred_at_unix: deps.clock.now_unix(),
                }],
            )
            .await?;
            store.commit().await?;

            announce(&deps, &staged).await;
            Ok(result)
        }
    })
    .await
}

/// Store a response against a claimed key.
async fn remember(
    store: &mut Box<dyn Store>,
    key: &IdempotencyKey,
    account_id: &AccountId,
    endpoint: &str,
    result: &WalletBalance,
) -> Result<(), AppError> {
    let body = result.to_stored_body()?;
    store
        .idempotency()
        .remember(
            key,
            account_id,
            endpoint,
            &crate::ports::StoredResponse::new(200, body),
        )
        .await?;
    Ok(())
}
