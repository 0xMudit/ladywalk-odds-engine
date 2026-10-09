//! # Application errors
//!
//! One error type crosses out of the application layer, and the split it makes
//! is the one an HTTP adapter has to make anyway: **did the caller do something
//! wrong, or did we?**
//!
//! Getting that split wrong in the direction of "caller's fault" is how a
//! database outage becomes a wall of 400s and a page nobody is paged for. So
//! [`AppError`] is not a string wrapper: [`AppError::is_client`] is computed
//! from the variant, and a [`StoreError::Backend`] can only become an internal
//! error.
//!
//! ## What the client is told
//!
//! [`AppError::to_client_message`] is the only text that reaches a response
//! body. It is written for a customer, not for a log: "that price has moved,
//! ask again" rather than "StaleQuote". The machine-readable code is separate
//! and stable, so a client can branch on it without parsing prose.

use std::fmt;

use domain::error::DomainError;
use domain::order::OrderError;
use domain::risk::RiskRefusal;

use crate::ports::StoreError;

/// Why a use case could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppError {
    /// The request violated a domain rule.
    Domain(DomainError),
    /// The store could not do its job.
    Store(StoreError),
    /// The request named a resource that does not exist.
    NotFound {
        /// What kind of record was being asked for.
        resource_type: &'static str,
        /// The id that was asked for.
        resource_id: String,
    },
    /// The request was already made, and this is a different request reusing
    /// its key.
    IdempotencyKeyReused {
        /// The key that was reused.
        key: String,
    },
    /// The same request is already running under this key.
    ///
    /// Distinct from a replay on purpose. A replay has an answer to return; this
    /// one does not yet exist, and reporting it as a replay would be a lie the
    /// client cannot detect. It is a 409 with a `Retry-After`, not a 200.
    RequestInProgress,
    /// The caller is not authenticated.
    Unauthenticated,
    /// The caller is authenticated but not permitted to do this.
    Forbidden {
        /// What was attempted, for the log and for the code.
        action: &'static str,
    },
    /// The caller's own limit for this operation is exhausted.
    RateLimited {
        /// Seconds to wait before trying again.
        retry_after_secs: u64,
    },
}

impl AppError {
    /// Whether the caller can fix this by sending something different.
    ///
    /// A backend failure is never the caller's fault, whatever the operation
    /// was, so it is decided here once rather than at each call site. A lost race
    /// is not the caller's fault either, and answers 503 rather than 409: the
    /// client did nothing wrong, and telling it to change its request would be
    /// advice that cannot help.
    #[must_use]
    pub fn is_client(&self) -> bool {
        match self {
            Self::Domain(error) => !error.is_internal(),
            Self::Store(error) => !error.is_internal(),
            Self::NotFound { .. }
            | Self::IdempotencyKeyReused { .. }
            | Self::RequestInProgress
            | Self::Unauthenticated
            | Self::Forbidden { .. }
            | Self::RateLimited { .. } => true,
        }
    }

    /// The stable, machine-readable code for this failure.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            // A `DomainError::Internal` has no code of its own, precisely so that
            // it cannot be distinguished by serialisation. The generic code here
            // is what a client sees for every server-side failure.
            Self::Domain(error) => error.error_code().unwrap_or("INTERNAL"),
            Self::Store(StoreError::Missing { .. }) => "NOT_FOUND",
            Self::Store(StoreError::Backend(_) | StoreError::Conflict { .. }) => "INTERNAL",
            Self::NotFound { .. } => "NOT_FOUND",
            Self::IdempotencyKeyReused { .. } => "IDEMPOTENCY_KEY_REUSED",
            Self::RequestInProgress => "REQUEST_IN_PROGRESS",
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::Forbidden { .. } => "FORBIDDEN",
            Self::RateLimited { .. } => "RATE_LIMITED",
        }
    }

    /// The HTTP status this failure should be returned as.
    ///
    /// Lives here rather than in the HTTP adapter so that the mapping is
    /// decided by the layer that knows what the error *means*. The adapter's job
    /// is to render, not to interpret; a `match` in the adapter is a second
    /// place for the same decision to be made, and the two drift.
    #[must_use]
    pub fn http_status(&self) -> u16 {
        match self {
            // `DomainError::http_status` returns `None` for `Internal`, which
            // becomes a 500 rather than the 400 this arm would otherwise default
            // to. An internal failure reported as a client error is the exact
            // misclassification this type exists to prevent.
            Self::Domain(error) => error.http_status().unwrap_or(500),
            Self::Store(StoreError::Missing { .. }) => 404,
            // A conflict is a 503: the request was fine and the server was busy.
            Self::Store(StoreError::Backend(_) | StoreError::Conflict { .. }) => 503,
            Self::NotFound { .. } => 404,
            Self::IdempotencyKeyReused { .. } | Self::RequestInProgress => 409,
            Self::Unauthenticated => 401,
            Self::Forbidden { .. } => 403,
            Self::RateLimited { .. } => 429,
        }
    }

    /// The text a customer may be shown.
    ///
    /// Deliberately never includes a [`DomainError::Internal`] message: those
    /// name the invariant that broke, which is for the log and not the screen.
    /// A backend failure gets the same generic text as an internal one, so that
    /// the two are indistinguishable from outside.
    #[must_use]
    pub fn to_client_message(&self) -> String {
        match self {
            Self::Domain(error) => {
                if error.is_internal() {
                    Self::internal_message()
                } else {
                    error.to_client_message()
                }
            }
            // A backend failure and a lost race both get the generic text. The
            // difference between them is in the code, which an operator reads
            // from the log; the customer is told the same thing either way, which
            // is correct because nothing they can do affects the outcome.
            Self::Store(StoreError::Backend(_)) | Self::Store(StoreError::Conflict { .. }) => {
                Self::internal_message()
            }
            // The id is in the code and in the log. Echoing it in prose is a
            // free confirmation that a guess was close, so it is not echoed.
            Self::Store(StoreError::Missing { resource_type, .. })
            | Self::NotFound { resource_type, .. } => format!("No such {resource_type}."),
            // The key is echoed here because it is the caller's own value, chosen
            // by them, and telling them which key was reused is the only way they
            // can find the request that is stuck.
            Self::IdempotencyKeyReused { key } => {
                format!("That request key has already been used: {key}")
            }
            Self::RequestInProgress => {
                "That request is still being processed. Try again in a moment.".to_string()
            }
            Self::Unauthenticated => "Sign in to continue.".to_string(),
            // The action is named so an operator reading a support ticket can
            // see what was attempted; it names no customer, no market and no
            // account, so it leaks nothing the caller did not already know.
            Self::Forbidden { action } => {
                format!("Your account is not permitted to {action}.")
            }
            Self::RateLimited { retry_after_secs } => {
                format!("Too many requests. Try again in {retry_after_secs}s.")
            }
        }
    }

    /// The one message every server-side failure produces.
    fn internal_message() -> String {
        "Something went wrong on our side. Nothing has changed.".to_string()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => write!(f, "{error}"),
            Self::Store(error) => write!(f, "{error}"),
            Self::NotFound {
                resource_type,
                resource_id,
            } => write!(f, "not found: {resource_type} {resource_id}"),
            Self::IdempotencyKeyReused { key } => {
                write!(f, "idempotency key reused with a different request: {key}")
            }
            Self::RequestInProgress => {
                f.write_str("a request with this idempotency key is still in progress")
            }
            Self::Unauthenticated => f.write_str("caller is not authenticated"),
            Self::Forbidden { action } => {
                write!(f, "caller is not permitted to {action}")
            }
            Self::RateLimited { retry_after_secs } => {
                write!(f, "caller exceeded their rate limit; retry after {retry_after_secs}s")
            }
        }
    }
}

impl std::error::Error for AppError {}

impl From<DomainError> for AppError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<RiskRefusal> for AppError {
    fn from(refusal: RiskRefusal) -> Self {
        Self::Domain(DomainError::from(refusal))
    }
}

impl From<OrderError> for AppError {
    fn from(error: OrderError) -> Self {
        Self::Domain(DomainError::InvalidInput {
            field: "order",
            reason: error.to_string(),
        })
    }
}
