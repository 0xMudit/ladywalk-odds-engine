//! # Telemetry / tracing (F-006 · #68)
//!
//! Initialises the global tracing subscriber and provides the money-path event
//! macro that enforces the field contract (amount + currency + account +
//! reference always present together).
//!
//! ## Correlation IDs
//!
//! Every request is assigned a `correlation_id` (UUID v4). It is:
//! - Taken from the inbound `X-Correlation-ID` header if present and valid.
//! - Generated fresh otherwise.
//! - Stored in a `tokio::task_local!` so it propagates across `.await` points.
//! - Echoed in the `X-Correlation-ID` response header by the HTTP adapter.
//!
//! ## Output format
//!
//! - `json_logs = true` → Bunyan-compatible JSON, one object per line.
//! - `json_logs = false` → human-readable (dev/test).

use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Task-local correlation ID
// ─────────────────────────────────────────────────────────────────────────────

tokio::task_local! {
    /// The correlation ID for the current async task.
    ///
    /// Set at the request boundary; propagated across `.await` via
    /// `CorrelationId::scope(id, future)`.
    pub static CORRELATION_ID: Uuid;
}

/// Returns the current correlation ID, or a nil UUID if none is set
/// (e.g. in background jobs started without a request context).
#[must_use]
pub fn current_correlation_id() -> Uuid {
    CORRELATION_ID.try_with(|id| *id).unwrap_or(Uuid::nil())
}

/// Run `future` inside a correlation-ID scope.
///
/// All spans created inside `future` will see the correlation ID in
/// `CORRELATION_ID`.
pub async fn with_correlation_id<F, T>(id: Uuid, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CORRELATION_ID.scope(id, future).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Subscriber initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise the global tracing subscriber.
///
/// Call once at binary startup, before spawning any tasks.
///
/// # Panics
///
/// Panics if called more than once in the same process (tracing-subscriber
/// raises an error when the global default is set twice).
pub fn init(log_level: &str, json_logs: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    if json_logs {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE),
            )
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE),
            )
            .init();
    }
}

/// Initialise a test-scoped subscriber that does not conflict with other tests.
///
/// Returns `true` if it was successfully installed, `false` if a subscriber
/// was already installed (caller should not treat that as an error).
#[must_use]
pub fn init_test() -> bool {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("debug"))
        .with_test_writer()
        .try_init()
        .is_ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Money-path event macro
// ─────────────────────────────────────────────────────────────────────────────

/// Emit a structured tracing event for a money-path operation.
///
/// Guarantees that `amount_minor`, `currency`, `account_id`, and `reference`
/// always appear together. No money-path event can be emitted without all
/// four fields — the macro enforces this at the call site.
///
/// # Example
///
/// ```rust,ignore
/// money_event!(
///     level = tracing::Level::INFO,
///     operation = "bet_accepted",
///     amount_minor = 500_i64,
///     currency = "GBP",
///     account_id = "acct-uuid-here",
///     reference = "order-uuid-here",
///     extra = "price=2.50"
/// );
/// ```
#[macro_export]
macro_rules! money_event {
    (
        level       = $level:expr,
        operation   = $op:expr,
        amount_minor = $amount:expr,
        currency    = $currency:expr,
        account_id  = $account:expr,
        reference   = $reference:expr
        $(, $key:ident = $val:expr)*
        $(,)?
    ) => {
        tracing::event!(
            $level,
            // Required money-path fields — always present:
            operation    = $op,
            amount_minor = $amount,
            currency     = $currency,
            account_id   = %$account,
            reference    = %$reference,
            correlation_id = %$crate::telemetry::current_correlation_id(),
            // Optional extra fields:
            $( $key = $val, )*
        );
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn correlation_id_propagates_across_await() {
        let id = Uuid::new_v4();
        let captured = with_correlation_id(id, async {
            // Simulate an await boundary
            tokio::task::yield_now().await;
            current_correlation_id()
        })
        .await;
        assert_eq!(captured, id);
    }

    #[tokio::test]
    async fn no_correlation_id_returns_nil() {
        // Outside a scope, the nil UUID is returned rather than panicking.
        let id = current_correlation_id();
        assert_eq!(id, Uuid::nil());
    }

    #[tokio::test]
    async fn nested_scopes_restore_outer_id() {
        let outer = Uuid::new_v4();
        let inner = Uuid::new_v4();

        let (outer_after, inner_captured) = with_correlation_id(outer, async {
            let inner_captured =
                with_correlation_id(inner, async { current_correlation_id() }).await;
            // After inner scope, outer is restored
            (current_correlation_id(), inner_captured)
        })
        .await;

        assert_eq!(inner_captured, inner);
        assert_eq!(outer_after, outer);
    }
}
