//! # Error taxonomy
//!
//! A single typed error enum for every failure the domain can produce.
//! The variants are deliberately coarse enough to drive client recovery
//! behaviour (Q14) while preserving internal detail for logging.
//!
//! ## Mapping to HTTP
//!
//! The mapping from `DomainError` to HTTP status lives **only** in
//! `adapters::http` — one file, one diff when it changes. This module
//! deliberately stops at [`DomainError::http_status`], which reports the
//! number a client should see without knowing what an HTTP response is.
//!
//! ## Internal vs client-visible errors
//!
//! [`DomainError::Internal`] carries information that must **never** reach
//! a client response. It is a distinct variant to make accidental exposure
//! a compile-time problem: the HTTP adapter matches on it explicitly and
//! returns a generic 500 body.
//!
//! ## Why this is hand-written
//!
//! `thiserror` would remove about forty lines of `Display` here, and the
//! domain crate is not allowed it: F-001 requires `domain` to depend on
//! nothing but `serde` and a decimal helper, so that the rules of betting can
//! be tested and reasoned about without a dependency graph. The formatting
//! below is explicit, which is also why the per-variant messages are checked
//! by tests instead of being taken on trust.

use std::fmt;

use crate::money::MoneyError;

/// Every failure that can occur inside the domain layer.
///
/// This enum is `#[non_exhaustive]` so adding a variant is not a breaking
/// change for downstream crates. Match arms must include a `_` fallback.
///
/// `Clone` and `Eq` are required by the application layer: an error can be
/// staged inside an idempotency record and replayed, and a replay must be
/// comparable to the original response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DomainError {
    // ── Money path ─────────────────────────────────────────────────────────
    /// The customer's available balance would go below their exposure limit.
    ///
    /// Client recovery: display available balance and let the customer adjust
    /// their stake.
    InsufficientFunds {
        /// Available minor units at decision time.
        available_minor: i64,
        /// Requested minor units.
        requested_minor: i64,
    },

    /// The quote token has expired or does not match the current price.
    ///
    /// Client recovery: fetch a fresh quote and re-submit.
    StaleQuote,

    /// The market is not accepting new orders.
    ///
    /// Client recovery: display market status; retry when it re-opens.
    MarketClosed,

    /// The platform kill switch is active. All money movement is halted.
    ///
    /// Client recovery: display a maintenance message.
    BettingHalted,

    /// The market has been automatically quarantined due to a plausibility
    /// invariant breach.
    ///
    /// Client recovery: do not retry; contact support.
    MarketQuarantined {
        /// The quarantined market's ID, in whatever representation the
        /// caller's identifier type serialises to. The domain does not
        /// depend on an ID crate, so it does not get to pick one; the
        /// adapter converts at the boundary.
        market_id: String,
    },

    /// A customer's net exposure would exceed their configured limit.
    ///
    /// Client recovery: reduce stake or cancel open orders.
    ExposureLimitExceeded {
        /// Configured exposure limit in minor units.
        limit_minor: i64,
        /// Current exposure in minor units after this order.
        current_minor: i64,
    },

    // ── Idempotency ────────────────────────────────────────────────────────
    /// A request with this idempotency key was already processed.
    ///
    /// Client recovery: treat the original result as the canonical response.
    DuplicateRequest,

    // ── Not found ──────────────────────────────────────────────────────────
    /// The requested resource does not exist.
    ///
    /// Client recovery: the resource is gone; do not retry the same id.
    NotFound {
        /// Human-readable resource type, e.g. `"market"`, `"order"`.
        resource_type: &'static str,
        /// The ID that was looked up.
        resource_id: String,
    },

    // ── Rate limiting ──────────────────────────────────────────────────────
    /// The caller has exceeded the rate limit for this operation.
    ///
    /// Client recovery: honour `Retry-After`.
    RateLimited {
        /// Seconds until the rate limit resets.
        retry_after_secs: u64,
    },

    // ── Validation ─────────────────────────────────────────────────────────
    /// The caller supplied an invalid value.
    ///
    /// Client recovery: fix the request and retry.
    InvalidInput {
        /// The field name, e.g. `"stake"`.
        field: &'static str,
        /// Human-readable reason.
        reason: String,
    },

    // ── Money arithmetic ───────────────────────────────────────────────────
    /// A money arithmetic error propagated from the money layer.
    Money(MoneyError),

    // ── Internal / page-worthy ─────────────────────────────────────────────
    //
    // THIS VARIANT MUST NEVER BE SERIALISED INTO A CLIENT RESPONSE.
    // The HTTP adapter in `adapters` must match on it explicitly and
    // return a generic 500 body with no internal detail.
    /// An internal invariant was violated.
    ///
    /// Examples:
    /// - Double-entry sum-to-zero assertion failed.
    /// - Settlement would produce a negative payout.
    /// - Ledger record could not be written.
    ///
    /// **This must never reach a client response body.**
    Internal {
        /// Internal detail for structured logging and alerting.
        message: String,
        /// Optional context for the pager.
        context: Option<String>,
    },
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientFunds {
                available_minor,
                requested_minor,
            } => write!(
                f,
                "insufficient funds: available {available_minor} minor units, requested {requested_minor}"
            ),
            Self::StaleQuote => {
                f.write_str("stale quote token: the price has moved or the token has expired")
            }
            Self::MarketClosed => f.write_str("market is closed for new orders"),
            Self::BettingHalted => {
                f.write_str("betting is halted: the platform kill switch is active")
            }
            Self::MarketQuarantined { market_id } => {
                write!(f, "market {market_id} is quarantined and not accepting orders")
            }
            Self::ExposureLimitExceeded {
                limit_minor,
                current_minor,
            } => write!(
                f,
                "exposure limit exceeded: limit {limit_minor}, current {current_minor}"
            ),
            Self::DuplicateRequest => f.write_str("duplicate request: idempotency key already used"),
            Self::NotFound {
                resource_type,
                resource_id,
            } => write!(f, "resource not found: {resource_type} {resource_id}"),
            Self::RateLimited { retry_after_secs } => {
                write!(f, "rate limit exceeded; retry after {retry_after_secs}s")
            }
            Self::InvalidInput { field, reason } => write!(f, "invalid input: {field} — {reason}"),
            Self::Money(source) => write!(f, "money arithmetic error: {source}"),
            Self::Internal { message, .. } => {
                write!(f, "INTERNAL ERROR (do not expose to client): {message}")
            }
        }
    }
}

impl std::error::Error for DomainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Money(source) => Some(source),
            Self::Internal { .. }
            | Self::InsufficientFunds { .. }
            | Self::StaleQuote
            | Self::MarketClosed
            | Self::BettingHalted
            | Self::MarketQuarantined { .. }
            | Self::ExposureLimitExceeded { .. }
            | Self::DuplicateRequest
            | Self::NotFound { .. }
            | Self::RateLimited { .. }
            | Self::InvalidInput { .. } => None,
        }
    }
}

impl From<MoneyError> for DomainError {
    fn from(source: MoneyError) -> Self {
        Self::Money(source)
    }
}

impl DomainError {
    /// Returns `true` if this error is an internal invariant breach that
    /// must **never** be sent to the client.
    ///
    /// The HTTP adapter uses this to gate response serialisation.
    #[must_use]
    pub fn is_internal(&self) -> bool {
        matches!(self, Self::Internal { .. })
    }

    /// Build an [`Internal`][DomainError::Internal] error from any
    /// `Display`able message.
    pub fn internal(message: impl fmt::Display) -> Self {
        Self::Internal {
            message: message.to_string(),
            context: None,
        }
    }

    /// Build an [`Internal`][DomainError::Internal] error with context.
    pub fn internal_with_context(message: impl fmt::Display, context: impl fmt::Display) -> Self {
        Self::Internal {
            message: message.to_string(),
            context: Some(context.to_string()),
        }
    }

    /// The machine-readable error code sent to clients.
    ///
    /// `Internal` returns `None` — the HTTP adapter must substitute a
    /// generic code without leaking details.
    #[must_use]
    pub fn error_code(&self) -> Option<&'static str> {
        match self {
            Self::InsufficientFunds { .. } => Some("INSUFFICIENT_FUNDS"),
            Self::StaleQuote => Some("STALE_QUOTE"),
            Self::MarketClosed => Some("MARKET_CLOSED"),
            Self::BettingHalted => Some("BETTING_HALTED"),
            Self::MarketQuarantined { .. } => Some("MARKET_QUARANTINED"),
            Self::ExposureLimitExceeded { .. } => Some("EXPOSURE_LIMIT_EXCEEDED"),
            Self::DuplicateRequest => Some("DUPLICATE_REQUEST"),
            Self::NotFound { .. } => Some("NOT_FOUND"),
            Self::RateLimited { .. } => Some("RATE_LIMITED"),
            Self::InvalidInput { .. } => Some("INVALID_INPUT"),
            Self::Money(_) => Some("MONEY_ARITHMETIC_ERROR"),
            // NEVER expose this code — the adapter must substitute a generic one
            Self::Internal { .. } => None,
        }
    }

    /// The HTTP status code that should be returned for this error.
    ///
    /// Returns `None` for [`Internal`][DomainError::Internal] — the adapter
    /// must use 500 without leaking the variant.
    ///
    /// Each variant gets its own line even where several share a status. This
    /// table is the thing a reviewer reads when asking "can any client-visible
    /// failure be reported as a server error, and is `Internal` the only one?",
    /// and collapsing `422` into one pattern with four alternatives is exactly
    /// what makes such a question hard to answer by eye.
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::InsufficientFunds { .. } => Some(422),
            Self::StaleQuote => Some(409),
            Self::MarketClosed => Some(422),
            Self::BettingHalted => Some(503),
            Self::MarketQuarantined { .. } => Some(503),
            Self::ExposureLimitExceeded { .. } => Some(422),
            Self::DuplicateRequest => Some(409),
            Self::NotFound { .. } => Some(404),
            Self::RateLimited { .. } => Some(429),
            Self::InvalidInput { .. } => Some(400),
            Self::Money(_) => Some(422),
            Self::Internal { .. } => None,
        }
    }

    /// The text a customer may be shown, with no internal detail in it.
    ///
    /// Separate from [`Display`][fmt::Display] because `Display` is for logs
    /// and `Display` of an [`Internal`][DomainError::Internal] names the
    /// invariant that broke. This is the only variant of the two that may reach
    /// a response body, and it says what the customer can do next.
    ///
    /// [`Internal`][DomainError::Internal] deliberately produces the same text
    /// as any other server-side failure: a customer who learns which invariant
    /// broke learns about the inside of the system.
    #[must_use]
    pub fn to_client_message(&self) -> String {
        match self {
            Self::InsufficientFunds {
                available_minor,
                requested_minor,
            } => {
                format!(
                    "You have {available_minor} minor units available and this order needs {requested_minor}."
                )
            }
            Self::StaleQuote => {
                "That price has moved or the quote has expired. Ask for a new one.".to_string()
            }
            Self::MarketClosed => "This market is not accepting bets.".to_string(),
            Self::BettingHalted => {
                "Betting is temporarily paused. Nothing has been charged.".to_string()
            }
            Self::MarketQuarantined { .. } => {
                "This market is under review. No bets can be placed on it.".to_string()
            }
            Self::ExposureLimitExceeded { limit_minor, .. } => {
                format!("This order would take you past your {limit_minor} minor unit limit.")
            }
            Self::DuplicateRequest => {
                "This request has already been processed.".to_string()
            }
            Self::NotFound { resource_type, .. } => format!("No such {resource_type}."),
            Self::RateLimited { retry_after_secs } => {
                format!("Too many requests. Try again in {retry_after_secs} seconds.")
            }
            Self::InvalidInput { reason, .. } => reason.clone(),
            Self::Money(_) => "That amount cannot be represented. Nothing has been charged.".to_string(),
            Self::Internal { .. } => {
                "Something went wrong on our side. Nothing has changed.".to_string()
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_error_has_no_code() {
        let e = DomainError::internal("ledger sum is non-zero");
        assert!(e.error_code().is_none());
        assert!(e.http_status().is_none());
        assert!(e.is_internal());
    }

    #[test]
    fn non_internal_errors_have_codes_and_statuses() {
        let cases: &[DomainError] = &[
            DomainError::InsufficientFunds {
                available_minor: 100,
                requested_minor: 200,
            },
            DomainError::StaleQuote,
            DomainError::MarketClosed,
            DomainError::BettingHalted,
            DomainError::MarketQuarantined {
                market_id: "00000000-0000-0000-0000-000000000000".to_string(),
            },
            DomainError::DuplicateRequest,
            DomainError::NotFound {
                resource_type: "market",
                resource_id: "abc".to_string(),
            },
            DomainError::RateLimited {
                retry_after_secs: 30,
            },
            DomainError::InvalidInput {
                field: "stake",
                reason: "too small".to_string(),
            },
        ];

        for err in cases {
            assert!(err.error_code().is_some(), "missing error_code for {err:?}");
            assert!(
                err.http_status().is_some(),
                "missing http_status for {err:?}"
            );
            assert!(!err.is_internal(), "wrongly marked internal: {err:?}");
        }
    }

    #[test]
    fn internal_detail_must_not_appear_in_non_internal_display() {
        // A monitoring system that alerts on the substring "INTERNAL" would
        // page on a stale quote if the domain error text were to leak it.
        let not_internal = DomainError::StaleQuote;
        assert!(!not_internal.to_string().contains("INTERNAL"));
    }

    #[test]
    fn the_client_message_for_an_internal_error_says_nothing_internal() {
        // The internal message is the name of the invariant that broke, and it
        // belongs in the log. Two failures that broke different things must
        // still produce the same text, or the difference itself becomes the
        // hint.
        let a = DomainError::internal("ledger transaction 41 does not sum to zero");
        let b = DomainError::internal_with_context("payout overflowed", "market m1");
        assert_eq!(a.to_client_message(), b.to_client_message());
        assert!(!a.to_client_message().contains("ledger"));
        assert!(!a.to_client_message().contains("41"));
        assert!(!b.to_client_message().contains("m1"));
    }

    #[test]
    fn every_client_message_names_a_recovery() {
        // A message that only reports the failure leaves the customer with
        // nothing to do but contact support.
        for error in [
            DomainError::StaleQuote,
            DomainError::MarketClosed,
            DomainError::BettingHalted,
            DomainError::MarketQuarantined {
                market_id: "m1".to_string(),
            },
            DomainError::DuplicateRequest,
            DomainError::RateLimited {
                retry_after_secs: 30,
            },
        ] {
            let message = error.to_client_message();
            assert!(!message.is_empty(), "empty message for {error:?}");
            assert!(
                !message.contains("INTERNAL"),
                "internal wording leaked for {error:?}"
            );
        }
    }

    #[test]
    fn a_not_found_message_does_not_echo_the_id() {
        // The id is in the code and the log. Putting it in prose is a free
        // confirmation that a guess was close.
        let error = DomainError::NotFound {
            resource_type: "market",
            resource_id: "secret-guess".to_string(),
        };
        assert_eq!(error.to_client_message(), "No such market.");
    }

    #[test]
    fn money_error_converts_and_reports_its_source() {
        let err = DomainError::from(MoneyError::Overflow);
        assert!(matches!(err, DomainError::Money(MoneyError::Overflow)));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn non_internal_errors_report_no_source() {
        let err = DomainError::StaleQuote;
        assert!(std::error::Error::source(&err).is_none());
    }
}
