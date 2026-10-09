//! # order-entry service
//!
//! Thin binary entrypoint. Responsibilities:
//! 1. Load and validate configuration (fail-fast on bad config).
//! 2. Initialise tracing.
//! 3. Start the Axum HTTP server.
//! 4. Coordinate graceful shutdown (SIGTERM → drain → exit 0 or 1).

#![allow(clippy::unwrap_used)] // Binary entrypoint: process::exit on failure is intentional

use std::sync::Arc;
use std::time::Duration;

use application::{config::Settings, shutdown::ShutdownCoordinator, telemetry};
use tracing::info;

#[tokio::main]
async fn main() {
    // ── 1. Load config (fail-fast: exits non-zero listing every missing key) ──
    let settings = Settings::load().unwrap_or_else(|e| {
        // Config errors go to stderr before tracing is up.
        eprintln!("Configuration failed:\n{e}");
        std::process::exit(1);
    });

    // ── 2. Initialise tracing ─────────────────────────────────────────────────
    telemetry::init(
        &settings.observability.log_level,
        settings.observability.json_logs,
    );

    // Log resolved config with secrets redacted.
    info!(config = %settings.redacted_display(), "configuration loaded");

    // ── 3. Shutdown coordinator ───────────────────────────────────────────────
    let timeout = Duration::from_secs(settings.server.shutdown_timeout_secs);
    let coordinator = ShutdownCoordinator::new(timeout);
    let shutdown_handle = coordinator.handle();

    // ── 4. Axum router ────────────────────────────────────────────────────────
    let app = adapters::http::router();

    let addr_str = settings.server.socket_addr();
    let addr: std::net::SocketAddr = addr_str.parse().unwrap_or_else(|e| {
        eprintln!("invalid socket address '{addr_str}': {e}");
        std::process::exit(1);
    });

    info!(addr = %addr, "starting order-entry service");

    let _settings = Arc::new(settings);

    // ── 5. Serve with graceful shutdown ───────────────────────────────────────
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("failed to bind {addr}: {e}");
            std::process::exit(1);
        });

    tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_handle.token.cancelled().await;
                info!("HTTP server: shutdown signal received, draining");
            })
            .await;

        if let Err(e) = result {
            tracing::error!(error = %e, "HTTP server error");
        }
    });

    // ── 6. Wait for OS signal, then coordinate drain ──────────────────────────
    let outcome = coordinator.run().await;
    info!(outcome = ?outcome, "shutdown complete");

    std::process::exit(outcome.exit_code());
}
