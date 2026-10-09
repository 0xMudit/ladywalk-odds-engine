//! # LadyWalk Adapters
//!
//! Infrastructure adapters: Postgres (via sqlx), HTTP handlers (via Axum),
//! Redis. This crate wires the domain and application layers to I/O.
//!
//! ## Dependency rules
//!
//! - `adapters` may depend on `domain` and `application`.
//! - `domain` and `application` must **never** depend on `adapters`.
//! - CI enforces this via `cargo metadata` (see `scripts/check-domain-deps.sh`).

#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![allow(clippy::module_name_repetitions)]

pub mod db;
pub mod http;
