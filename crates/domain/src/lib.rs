//! # LadyWalk Domain
//!
//! Pure domain model for the LadyWalk peer-to-peer betting exchange.
//!
//! ## Dependency contract
//!
//! This crate is **infrastructure-free** and depends on `serde` and nothing
//! else. It may not depend on sqlx, redis, axum, tokio, or any HTTP library.
//! Two things enforce that, and both are run in CI:
//!
//! - `tests/dependency_purity.rs` reads this crate's own `Cargo.toml` and
//!   fails if `[dependencies]` names anything but `serde`. It is a test
//!   rather than a script so that a violation is a failing build rather than
//!   a thing somebody has to remember to run.
//! - `scripts/check-domain-deps.sh` resolves the full dependency graph with
//!   `cargo metadata` and fails if `domain` reaches `adapters` or any I/O
//!   crate, directly or transitively.
//!
//! ## Modules
//!
//! | Module | Covers |
//! |--------|--------|
//! | [`error`] | `DomainError`, the one error type that crosses the crate boundary |
//! | [`ids`] | The string-backed identity types every other type refers to |
//! | [`money`] | `MoneyAmount`, `Currency`, checked arithmetic, rounding |
//! | [`odds`] | Fixed-scale decimal odds and their payouts |
//! | [`quote`] | The server-signed price a client presents back |
//! | [`order`] | The order lifecycle and the transitions it permits |
//! | [`matching`] | Which resting order an incoming order fills against |
//! | [`ledger`] | Balanced double-entry transactions |
//! | [`settlement`] | Turning a resolved market into payouts |
//! | [`risk`] | The kill switch and exposure limits |
//!
//! ## How the pieces fit
//!
//! A quote is signed, an order carries the price the server chose, matching
//! takes the stake into the ledger, and settlement pays it back. Each of those
//! is a separate module because each has a failure mode worth naming on its
//! own: a forged quote, an order that changed its price, a match that took the
//! wrong money, a settlement that paid a loser or paid twice.
//!
//! Every public type that crosses a serialisation boundary is marked
//! `#[non_exhaustive]` so adding a variant is not a breaking change for
//! downstream crates that `match` on it.

#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![allow(clippy::module_name_repetitions)]
// A test asserts on the exact value an operation returned, and on the `Err` it
// returned instead, which is what `unwrap` and `expect` are for. The
// no-panic rule exists for code that runs in production, where an `unwrap` is
// a reachable outage, so it is relaxed only under `cfg(test)`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod ids;
pub mod ledger;
pub mod matching;
pub mod money;
pub mod odds;
pub mod order;
pub mod quote;
pub mod risk;
pub mod settlement;
