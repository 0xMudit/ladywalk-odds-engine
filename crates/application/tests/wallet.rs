//! Tests for the properties that are easy to state and easy to get wrong.
//!
//! Every test here is about how a use case *sequences* decisions inside one
//! transaction, which is the only thing [`application::testing`] exists to make
//! testable without a database. `domain`'s own rules are tested in `domain`.
//!
//! They are grouped by the failure they catch rather than by the file they
//! exercise, because the useful question about a change to a use case is "what
//! did it stop doing", and that is answered faster from this list than from a
//! file-by-file diff.
//!
//! # Why these and not more
//!
//! Each test names a way the system could be wrong that nothing else would catch:
//! a balance reported from before the money arrived, a claim that survives the
//! rollback of the transaction that made it, a retry that credits twice. A test
//! that only re-asserts what a use case already returns fails when the code is
//! refactored and never when the code is wrong, so there are deliberately none of
//! those here.
//!
//! # A note on starting balances
//!
//! Nothing here seeds a balance directly. `add_customer`'s `minor_units` writes
//! the maintained projection without writing a ledger entry behind it, so a
//! harness seeded that way starts in a state no correct database can be in — a
//! projection that disagrees with the ledger by construction. Two deposits with
//! different keys get to a non-zero balance honestly, and if a test here ever
//! needs a genuine opening balance the fake wants a `credit_customer` that posts
//! a real transaction rather than a number.

use application::prelude::{
    AccountId, Actor, AppError, Currency, DepositRequest, IdempotencyKey, KillSwitchState,
    StoreError,
};
use application::testing::TestHarness;
use application::usecases::reconciliation::{self, ReconciliationReport, Violation};
use application::usecases::wallet::{PLATFORM_DEPOSITS, deposit};

// ── Fixtures ────────────────────────────────────────────────────────────────

/// An account id, for arranging a starting position.
fn account(name: &str) -> AccountId {
    AccountId::new(name).expect("a valid account id in a test fixture")
}

/// The operator allowed to credit an account.
///
/// Deposits are operator-only on purpose: a deposit is a record that money
/// arrived from outside, not a balance the customer may set for themselves.
fn operator() -> Actor {
    Actor::operator(account("acct-operator-1"))
}

/// A deposit of `amount_minor` for `account_id`, safe to retry under `key`.
fn deposit_request(
    account_id: &AccountId,
    amount_minor: i64,
    key: &str,
) -> DepositRequest {
    DepositRequest {
        account_id: account_id.clone(),
        amount_minor,
        currency: Currency::GBP,
        idempotency_key: IdempotencyKey::new(key).expect("a valid idempotency key"),
        external_reference: format!("psp-{key}"),
    }
}

/// A harness with one customer who has no money yet.
fn harness_with_customer() -> (TestHarness, AccountId) {
    let harness = TestHarness::at_epoch();
    let customer = account("acct-customer-1");
    harness
        .store
        .add_customer(&customer, 0, Currency::GBP);
    (harness, customer)
}

/// What the maintained projection says the customer holds.
fn projected(harness: &TestHarness, account_id: &AccountId) -> i64 {
    harness
        .store
        .state()
        .balances
        .get(&(account_id.clone(), Currency::GBP))
        .copied()
        .unwrap_or_default()
}

/// What the ledger itself says the customer holds.
///
/// Computed from the legs rather than read from the projection, because having
/// both is the point and a test that reads one of them twice proves nothing.
fn from_ledger(harness: &TestHarness, account_id: &AccountId) -> i64 {
    harness
        .store
        .state()
        .ledger
        .iter()
        .flat_map(|transaction| &transaction.legs)
        .filter(|leg| leg.account_id == *account_id)
        .map(|leg| leg.amount_minor_units)
        .sum()
}

/// Every account id any leg in the ledger names.
fn accounts_touched(harness: &TestHarness) -> Vec<String> {
    harness
        .store
        .state()
        .ledger
        .iter()
        .flat_map(|transaction| &transaction.legs)
        .map(|leg| leg.account_id.as_str().to_string())
        .collect()
}

/// The first balance disagreement in a report, as an account id and both figures.
///
/// Reported this way rather than as the whole `BalanceDiscrepancy` so the
/// assertions can be about the fact being claimed — "this account disagrees" —
/// instead of pinning a struct that will grow a currency field one day.
fn first_disagreement(report: &ReconciliationReport) -> Option<(AccountId, i64, i64)> {
    report.violations.iter().find_map(|violation| match violation {
        Violation::BalanceDisagreement {
            account_id,
            projected_minor_units,
            ledger_minor_units,
        } => Some((
            account_id.clone(),
            *projected_minor_units,
            *ledger_minor_units,
        )),
        _ => None,
    })
}

// ── The deposit balance ─────────────────────────────────────────────────────

/// A deposit reports the balance *after* the money arrived.
///
/// The bug this pins is the one a customer complains about: the money lands and
/// the API reports the balance from before it landed. Nothing else would catch
/// it — the ledger is correct either way, only the number handed back is wrong,
/// and it is the only number the customer ever sees.
#[tokio::test]
async fn a_deposit_reports_the_balance_after_the_money_arrived() {
    let (harness, customer) = harness_with_customer();

    let balance = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("a deposit for an active account succeeds");

    assert_eq!(
        balance.available_minor, 2_500,
        "the reported balance must include the deposit rather than stop short of it"
    );
    assert_eq!(balance.currency, Currency::GBP);
    assert_eq!(balance.account_id, customer);
}

/// A second deposit reports the running total, not just the latest amount.
///
/// Separate from the above because it is the arithmetic that regresses rather
/// than the lookup: a deposit onto zero hides a missing addition that a deposit
/// onto an existing balance cannot.
#[tokio::test]
async fn a_second_deposit_reports_the_running_total() {
    let (harness, customer) = harness_with_customer();

    deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("the first deposit succeeds");
    let balance = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 1_000, "deposit-2"),
    )
    .await
    .expect("the second deposit succeeds");

    assert_eq!(
        balance.available_minor, 3_500,
        "the second deposit reported {balance:?}"
    );
}

/// A deposit moves the ledger and the maintained projection together, to the same
/// figure.
///
/// A deposit that moved one and not the other would pass the assertion above and
/// surface in reconciliation weeks later, which is the entire reason the
/// projection exists as a thing worth having.
#[tokio::test]
async fn a_deposit_moves_the_ledger_and_the_projection_together() {
    let (harness, customer) = harness_with_customer();

    deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("the deposit succeeds");

    assert_eq!(projected(&harness, &customer), 2_500, "the projection moved");
    assert_eq!(from_ledger(&harness, &customer), 2_500, "the ledger moved");
}

/// A deposit is a liability movement, not revenue and not money from nowhere.
///
/// Double entry rules out "from nowhere" as long as the transaction balances, so
/// the error worth guarding against is the *counter-account* being something that
/// looks like the platform's own money. That failure passes every balance check
/// and stays invisible until somebody settles a market using customer deposits,
/// which is exactly what the custody account exists to prevent.
#[tokio::test]
async fn a_deposit_is_a_liability_movement_not_revenue() {
    let (harness, customer) = harness_with_customer();

    deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("the deposit succeeds");

    let touched = accounts_touched(&harness);
    assert!(
        touched.iter().any(|id| id.as_str() == PLATFORM_DEPOSITS),
        "the deposit must be debited from the custody account, touched {touched:?}"
    );
    assert!(
        !touched.iter().any(|id| id.contains("revenue")),
        "a deposit is not the platform's revenue, touched {touched:?}"
    );
}

// ── The transaction boundary ────────────────────────────────────────────────

/// A refused deposit leaves nothing behind: no money, no audit entry, and no
/// idempotency claim.
///
/// The claim is the part that is easy to miss and expensive to miss. It is taken
/// before the work, inside the same unit of work, so it rolls back with it. If it
/// did not, a customer's retry of a deposit that failed for a boring reason would
/// be told the first attempt is still running — forever.
#[tokio::test]
async fn a_refused_deposit_leaves_no_money_no_audit_and_no_claim() {
    let harness = TestHarness::at_epoch();
    // Deliberately no `add_customer`, so the account does not exist and the
    // deposit is refused *after* the idempotency key has been claimed.
    let missing = account("acct-does-not-exist");

    let result = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&missing, 2_500, "deposit-1"),
    )
    .await;

    assert!(result.is_err(), "a deposit to a missing account is refused");

    let after = harness.store.state();
    assert!(
        after.ledger.is_empty(),
        "a refused deposit must not leave a transaction: {:?}",
        after.ledger
    );
    assert!(
        after.audit.is_empty(),
        "a refused deposit must not leave an audit entry: {:?}",
        after.audit
    );
    assert!(
        after.idempotency.is_empty(),
        "the claim rolled back with the transaction that made it"
    );
}

// ── The retry loop ──────────────────────────────────────────────────────────

/// A conflict is retried and the retry commits exactly once.
///
/// This is what the owned-factory signature exists for. Given a borrowed store,
/// the body could only re-run against the transaction that had just lost the
/// race, which in PostgreSQL is already aborted — so the retry would fail
/// identically and the customer would be shown a conflict that was never going to
/// clear. The one-commit assertion is the other half of the same property: a loop
/// that retried *after* committing would credit twice.
#[tokio::test]
async fn a_conflict_is_retried_and_the_retry_commits_once() {
    let (harness, customer) = harness_with_customer();

    harness.store.fail_next(StoreError::Conflict {
        resource: "account_balances",
        detail: "another transaction updated this row".to_string(),
    });

    let balance = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("a conflict is contention, not a failure");

    assert_eq!(balance.available_minor, 2_500);
    assert_eq!(
        harness.store.queued_failures(),
        0,
        "exactly one injected failure was consumed"
    );
    assert_eq!(
        harness.store.state().ledger.len(),
        1,
        "the retried attempt committed once, not twice"
    );
    assert_eq!(projected(&harness, &customer), 2_500, "and credited once");
}

/// A failure that is not contention is surfaced rather than retried.
///
/// A backend outage retried five times is five times the load on something that is
/// already failing, and it turns a fast honest error into a slow dishonest one.
/// Two failures are queued and one is left over precisely so that "was it
/// retried?" is observable without instrumenting the loop.
#[tokio::test]
async fn a_non_conflict_failure_is_not_retried() {
    let (harness, customer) = harness_with_customer();

    harness
        .store
        .fail_next(StoreError::Backend("the connection pool is exhausted".to_string()));
    harness
        .store
        .fail_next(StoreError::Backend("the connection pool is exhausted".to_string()));

    let result = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await;

    assert!(result.is_err(), "the outage is reported to the caller");
    assert_eq!(
        harness.store.queued_failures(),
        1,
        "a non-conflict must not be retried"
    );
    assert!(
        harness.store.state().ledger.is_empty(),
        "nothing was written"
    );
}

// ── Idempotency ─────────────────────────────────────────────────────────────

/// A replayed deposit returns the first answer, unchanged.
///
/// The response is stored precisely so that the provider's retry is safe. Crediting
/// twice is the loud version of this bug; answering with a different balance than
/// the first time is the quiet one, where the customer is simply told something
/// untrue. Asserting the two answers are equal covers both — equality is the
/// property, and which of them it broke is a detail.
#[tokio::test]
async fn a_replayed_deposit_returns_the_first_answer_unchanged() {
    let (harness, customer) = harness_with_customer();
    let request = deposit_request(&customer, 2_500, "deposit-1");

    let first = deposit(&harness.deps, &operator(), &request)
        .await
        .expect("the first deposit succeeds");
    let second = deposit(&harness.deps, &operator(), &request)
        .await
        .expect("the replay is answered, not refused");

    assert_eq!(
        first, second,
        "the replay returns the original response"
    );
    assert_eq!(
        second.available_minor, 2_500,
        "and does not report the post-replay balance"
    );
    assert_eq!(
        harness.store.state().ledger.len(),
        1,
        "the replay posted no second transaction"
    );
}

/// One idempotency key cannot be replayed for a different account.
///
/// Without this, a client that reuses a key across two customers hands the second
/// one the first one's balance. `IdempotencyOutcome::Reused` exists for exactly
/// this, and the request digest that would complete it does not exist yet — so
/// this is the floor, not the ceiling, and the test says so rather than implying
/// the problem is solved.
#[tokio::test]
async fn one_idempotency_key_cannot_be_replayed_for_another_account() {
    let harness = TestHarness::at_epoch();
    let first_customer = account("acct-customer-1");
    let second_customer = account("acct-customer-2");
    harness
        .store
        .add_customer(&first_customer, 0, Currency::GBP);
    harness
        .store
        .add_customer(&second_customer, 0, Currency::GBP);

    deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&first_customer, 2_500, "deposit-1"),
    )
    .await
    .expect("the first deposit succeeds");

    let result = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&second_customer, 2_500, "deposit-1"),
    )
    .await;

    assert!(
        matches!(result, Err(AppError::IdempotencyKeyReused { .. })),
        "reusing a key across accounts is refused, got {result:?}"
    );
    assert_eq!(
        harness.store.state().ledger.len(),
        1,
        "the refused reuse moved no money"
    );
    assert_eq!(
        projected(&harness, &second_customer),
        0,
        "and credited the second customer nothing"
    );
}

// ── The kill switch ─────────────────────────────────────────────────────────

/// A deposit is refused while the kill switch is thrown.
///
/// The switch exists to stop money moving. A deposit accepted during a halt is
/// money that has arrived into a system which cannot pay it back out, and the
/// customer finds that out at the worst possible moment.
#[tokio::test]
async fn a_deposit_is_refused_while_the_kill_switch_is_thrown() {
    let (harness, customer) = harness_with_customer();
    harness
        .store
        .edit(|state| state.kill_switch = KillSwitchState::Halted);

    let result = deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await;

    assert!(result.is_err(), "a deposit during a halt is refused");
    assert!(
        harness.store.state().ledger.is_empty(),
        "a refused deposit moves nothing"
    );
}

// ── Reconciliation ──────────────────────────────────────────────────────────

/// A healthy system says how much it checked, not merely that nothing broke.
///
/// `reconcile_balances` returns only the rows that disagree, so a green run
/// returns none of them. A report that counted those rows would say "reconciled
/// 0 accounts", which reads as *nothing was verified* — the exact opposite of
/// what a green result must mean for this job to be worth scheduling at all.
#[tokio::test]
async fn a_healthy_reconciliation_reports_what_it_actually_checked() {
    let (harness, customer) = harness_with_customer();

    deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("the deposit succeeds");

    let report = reconciliation::run(&harness.deps)
        .await
        .expect("reconciliation runs");

    assert!(
        report.accounts_compared >= 2,
        "the customer and the custody account were both compared, got {}",
        report.accounts_compared
    );
    assert_eq!(
        report.audit_entries_verified, 1,
        "the deposit wrote exactly one audit entry"
    );
    assert_eq!(
        first_disagreement(&report),
        None,
        "a deposit that moved both sides has nothing to disagree about: {:?}",
        report.violations
    );
    // The deposits account is necessarily negative after a deposit — it is money
    // the platform owes, not money it holds — so a report that flagged it would be
    // telling an operator the custody account is overdrawn, which is the alarm
    // that trains people to ignore alarms.
    assert!(
        !report.violations.iter().any(|violation| matches!(
            violation,
            Violation::IllegalNegativeBalance { .. }
        )),
        "a liability account is permitted to be negative: {:?}",
        report.violations
    );
}

/// A broken projection is reported, with both figures so the reader can tell them
/// apart.
///
/// The projection is a cache a trigger maintains, and "the trigger stopped firing"
/// is the failure worth catching — invisible unless every row is compared, and
/// impossible to test unless a fake can break the projection on purpose.
/// `set_projected_balance` is that affordance, and it is the only way to reach it.
#[tokio::test]
async fn a_broken_projection_is_reported_with_both_figures() {
    let (harness, customer) = harness_with_customer();

    deposit(
        &harness.deps,
        &operator(),
        &deposit_request(&customer, 2_500, "deposit-1"),
    )
    .await
    .expect("the deposit succeeds");

    harness
        .store
        .set_projected_balance(&customer, Currency::GBP, 999);

    let report = reconciliation::run(&harness.deps)
        .await
        .expect("reconciliation runs");

    assert_eq!(
        first_disagreement(&report),
        Some((customer, 999, 2_500)),
        "both figures are reported: {:?}",
        report.violations
    );
    assert!(!report.is_healthy(), "and the report says so");
}
