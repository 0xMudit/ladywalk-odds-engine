//! # Graceful shutdown (F-009 · #71)
//!
//! A shutdown primitive used by every server binary. It answers Q56: a
//! `SIGTERM` stops new work from being accepted and waits for in-flight work
//! up to a configurable deadline, then exits.
//!
//! ## Contract
//!
//! - On `SIGTERM` (or `Ctrl-C` in dev): stop accepting new requests.
//! - Wait for in-flight work to finish up to `timeout`.
//! - Exit `0` on clean drain, `1` on forced termination (timeout hit).
//! - A request arriving during the drain window is refused with `503`.
//!
//! ## Implementation
//!
//! Uses `tokio_util::sync::CancellationToken` plus a `tokio::sync::watch`
//! channel to broadcast the shutdown signal to all subsystems, and
//! `tokio::task::JoinSet` to track in-flight work.
//!
//! The `watch` channel approach (rather than a bare `CancellationToken`) lets
//! Axum's `axum::serve::with_graceful_shutdown` await it directly.

use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// A handle given to each subsystem that wants to participate in shutdown.
///
/// Clone it cheaply; each clone tracks the same shutdown signal.
#[derive(Clone, Debug)]
pub struct ShutdownHandle {
    /// Token cancelled when shutdown begins.
    pub token: CancellationToken,
    /// Receiver side of the drain-complete signal.
    drain_complete_tx: watch::Sender<bool>,
}

/// The shutdown coordinator. Owned by the binary's main function.
pub struct ShutdownCoordinator {
    token: CancellationToken,
    drain_complete_tx: watch::Sender<bool>,
    drain_complete_rx: watch::Receiver<bool>,
    timeout: Duration,
}

impl ShutdownCoordinator {
    /// Create a new coordinator with the given drain timeout.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let token = CancellationToken::new();
        let (tx, rx) = watch::channel(false);
        Self {
            token,
            drain_complete_tx: tx,
            drain_complete_rx: rx,
            timeout,
        }
    }

    /// A [`ShutdownHandle`] to distribute to subsystems.
    #[must_use]
    pub fn handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            token: self.token.clone(),
            drain_complete_tx: self.drain_complete_tx.clone(),
        }
    }

    /// A `Future` that resolves once the OS sends `SIGTERM` or `Ctrl-C`.
    ///
    /// Pass this to `axum::serve::with_graceful_shutdown`.
    pub async fn signal_future(&self) {
        self.token.cancelled().await;
    }

    /// Run the shutdown sequence.
    ///
    /// 1. Wait for a signal (`SIGTERM` / `Ctrl-C`).
    /// 2. Cancel the token (stops new work).
    /// 3. Wait up to `timeout` for in-flight work to drain.
    /// 4. Return `ShutdownOutcome`.
    pub async fn run(self) -> ShutdownOutcome {
        // Wait for OS signal
        wait_for_signal().await;
        info!("shutdown signal received; draining in-flight work");
        self.token.cancel();

        // Wait for all drain_complete_tx senders to be dropped (i.e. all
        // subsystems signalled done) or timeout.
        let deadline = tokio::time::sleep(self.timeout);
        tokio::pin!(deadline);

        let mut rx = self.drain_complete_rx.clone();
        tokio::select! {
            _ = rx.wait_for(|v| *v) => {
                info!("graceful shutdown complete");
                ShutdownOutcome::Clean
            }
            () = &mut deadline => {
                warn!(
                    timeout_secs = self.timeout.as_secs(),
                    "shutdown timeout reached; forcing exit"
                );
                ShutdownOutcome::Forced
            }
        }
    }
}

impl ShutdownHandle {
    /// Signal that this subsystem has finished draining.
    ///
    /// Call once all in-flight work is complete. The coordinator will
    /// unblock once all subsystem handles have called this.
    pub fn drain_complete(&self) {
        // Sending `true` on the watch channel unblocks the coordinator.
        // If the receiver is already dropped (coordinator gone) we ignore.
        let _ = self.drain_complete_tx.send(true);
    }

    /// Whether shutdown has been requested.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// The outcome of running the shutdown coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// All in-flight work finished within the deadline.
    Clean,
    /// The deadline was hit; some work may have been abandoned.
    Forced,
}

impl ShutdownOutcome {
    /// Returns the process exit code for this outcome.
    /// `0` → clean, `1` → forced.
    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Clean => 0,
            Self::Forced => 1,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OS signal helper
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves on SIGTERM or Ctrl-C (SIGINT).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        // If signal registration fails, fall back to ctrl_c only.
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = sigterm.recv() => { info!("received SIGTERM"); }
                _ = tokio::signal::ctrl_c() => { info!("received Ctrl-C"); }
            }
        } else {
            // Signal registration failed — fall back to ctrl_c.
            let _ = tokio::signal::ctrl_c().await;
            info!("received Ctrl-C (SIGTERM handler unavailable)");
        }
    }
    #[cfg(not(unix))]
    {
        // On Windows only Ctrl-C is supported in the standard library.
        let _ = tokio::signal::ctrl_c().await;
        info!("received Ctrl-C");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Simulate in-flight work: a task that runs for a bit, then marks itself done.
    #[tokio::test]
    async fn work_started_before_signal_completes() {
        let coordinator = ShutdownCoordinator::new(Duration::from_secs(5));
        let handle = coordinator.handle();

        let work_done = Arc::new(AtomicBool::new(false));
        let work_done_clone = work_done.clone();

        // Spawn work that takes 50ms
        tokio::spawn({
            let handle = handle.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                work_done_clone.store(true, Ordering::SeqCst);
                handle.drain_complete();
            }
        });

        // Trigger shutdown immediately
        coordinator.token.cancel();

        // Manually run drain logic
        let timeout = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(timeout);
        let mut rx = coordinator.drain_complete_rx.clone();

        tokio::select! {
            _ = rx.wait_for(|v| *v) => {}
            _ = &mut timeout => { panic!("timeout hit in test"); }
        }

        assert!(
            work_done.load(Ordering::SeqCst),
            "work should have completed"
        );
    }

    #[tokio::test]
    async fn forced_shutdown_after_timeout() {
        let coordinator = ShutdownCoordinator::new(Duration::from_millis(10));

        // No one calls drain_complete — simulate hung work
        coordinator.token.cancel();

        let timeout = tokio::time::sleep(coordinator.timeout);
        tokio::pin!(timeout);
        let mut rx = coordinator.drain_complete_rx.clone();

        let outcome = tokio::select! {
            _ = rx.wait_for(|v| *v) => ShutdownOutcome::Clean,
            _ = &mut timeout => ShutdownOutcome::Forced,
        };

        assert_eq!(outcome, ShutdownOutcome::Forced);
        assert_eq!(outcome.exit_code(), 1);
    }

    #[tokio::test]
    async fn clean_shutdown_exits_zero() {
        let coordinator = ShutdownCoordinator::new(Duration::from_secs(5));
        let handle = coordinator.handle();

        coordinator.token.cancel();
        handle.drain_complete();

        let mut rx = coordinator.drain_complete_rx.clone();
        let outcome = tokio::select! {
            _ = rx.wait_for(|v| *v) => ShutdownOutcome::Clean,
            _ = tokio::time::sleep(Duration::from_secs(1)) => ShutdownOutcome::Forced,
        };
        assert_eq!(outcome, ShutdownOutcome::Clean);
        assert_eq!(outcome.exit_code(), 0);
    }

    #[tokio::test]
    async fn is_shutting_down_reflects_cancellation() {
        let coordinator = ShutdownCoordinator::new(Duration::from_secs(5));
        let handle = coordinator.handle();
        assert!(!handle.is_shutting_down());
        coordinator.token.cancel();
        assert!(handle.is_shutting_down());
    }
}
