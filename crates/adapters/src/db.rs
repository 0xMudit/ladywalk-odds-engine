//! Database adapter: Postgres connection pool and transaction helper.

use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;

/// Build a Postgres connection pool from the given URL.
///
/// # Errors
///
/// Returns a [`sqlx::Error`] if the pool cannot be created or the initial
/// connection cannot be established.
pub async fn create_pool(
    database_url: &str,
    max_connections: u32,
    connect_timeout_secs: u64,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(connect_timeout_secs))
        .connect(database_url)
        .await
}
