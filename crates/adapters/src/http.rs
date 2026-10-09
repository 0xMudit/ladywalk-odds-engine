//! HTTP adapter: Axum router and error mapping.
//!
//! This module owns the single mapping from [`domain::error::DomainError`] to
//! HTTP status codes and machine-readable error bodies (F-007 · #69).

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use domain::error::DomainError;
use serde_json::json;

/// Wraps a [`DomainError`] so that Axum can render it.
///
/// This cannot be `impl IntoResponse for DomainError`: `IntoResponse` comes
/// from `axum` and `DomainError` comes from `domain`, so neither the trait nor
/// the type is local to this crate and Rust's orphan rule rejects it. A local
/// newtype is the only way to implement a foreign trait for a foreign type,
/// and it has a second benefit — the mapping from error to response now has a
/// name, so `git grep ErrorResponse` finds the one place it is defined.
#[derive(Debug)]
pub struct ErrorResponse(pub DomainError);

/// Lets a handler propagate a domain error with `?` and have Axum render it,
/// which is what makes the happy path read as `Result<Json<T>, ErrorResponse>`.
impl From<DomainError> for ErrorResponse {
    fn from(error: DomainError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        let DomainError::Internal { .. } = &self.0 else {
            let status = self
                .0
                .http_status()
                .and_then(|s| StatusCode::from_u16(s).ok())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let code = self.0.error_code().unwrap_or("UNKNOWN_ERROR");
            let body = json!({
                "error": {
                    "code":    code,
                    "message": self.0.to_string(),
                }
            });
            return (status, Json(body)).into_response();
        };

        // Never expose internal detail to clients: the status is the only
        // thing the caller learns, and the body is a fixed string.
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "code":    "INTERNAL_ERROR",
                    "message": "An internal error occurred. Please contact support.",
                }
            })),
        )
            .into_response()
    }
}

/// Placeholder router — filled in during EP-05.
pub fn router() -> axum::Router {
    axum::Router::new().route("/health", axum::routing::get(health_handler))
}

async fn health_handler() -> StatusCode {
    StatusCode::OK
}
