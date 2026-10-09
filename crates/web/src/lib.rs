//! The server-rendered betting surface.
//!
//! Betting is a high-stakes interface, and the rule it has to obey is that a
//! price a user is shown is always a price the server has confirmed. A
//! server-rendered page is the cheapest way to honour that: the bytes the
//! browser receives already contain the state the backend committed, so there
//! is no window in which a stale client-side store can show a stale price.
//!
//! This crate is therefore a presentation layer over `application`, not a
//! second application layer. It renders, and it must not decide: a page that
//! needs a new number asks a use case for that number. It is deliberately
//! separate from `api` because the two have opposite compatibility
//! requirements — a browser can be forced to reload a page, whereas an
//! integrator's integration cannot — and because merging "what the browser
//! sees" with "what integrators parse" would make both harder to change.
//!
//! The client-side application, where one exists, is a TypeScript package
//! outside this Cargo workspace and is tracked by EP-11. What lives here is
//! the server-rendered page and the assets it serves.
//!
//! Nothing is defined here yet. The surface is specified by EP-11, and it
//! should be designed with the responsible-gambling surfaces settled up
//! front, because those constrain the layout rather than being added to it
//! later.

#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
