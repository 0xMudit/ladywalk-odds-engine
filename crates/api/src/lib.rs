//! The public, versioned HTTP contract of the exchange.
//!
//! This crate is the boundary a third-party integrator programs against, so it
//! is the one place where a wire format is allowed to be defined and the one
//! place where changing a wire format is a breaking change. It owns request
//! and response shapes, the error envelope, and idempotency keys; it does not
//! own transport, which lives in `adapters`.
//!
//! Keeping the contract separate from `adapters` is what makes the "is this a
//! breaking change?" question answerable. A handler in `adapters` can be
//! rewritten freely, because nothing outside this crate names it. A field
//! added here has to be versioned, because something outside this crate
//! parses it.
//!
//! Nothing is defined here yet. The contract is specified by EP-12, and the
//! surface should be designed once, with its idempotency and rate-limit
//! semantics settled, rather than accreted from whatever the first handler
//! happened to need.

#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
