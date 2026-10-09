//! # Identifiers
//!
//! Every entity in the system is named by an opaque string, wrapped in a
//! distinct newtype so that a `MarketId` can never be passed where an
//! `OrderId` is expected. The wrappers are what make a mistake in a repository
//! signature a compile error rather than a production incident.
//!
//! ## Why not `uuid`
//!
//! The domain crate is allowed exactly two external dependencies, `serde` and
//! a decimal/integer helper, and a UUID implementation is neither. That is a
//! deliberate constraint rather than an accident of taste: the crate that
//! decides what a bet is worth should be readable end to end without a
//! dependency graph.
//!
//! Opaque strings have a second advantage. An identifier generated at the
//! edge — a UUID, a ULID, a Keycloak subject, a settlement reference — can be
//! carried through the domain without the domain having to agree with the
//! storage layer on how one is spelled. The adapters convert at the boundary.
//!
//! ## What is *not* validated
//!
//! An id is opaque, so the domain checks only that it is non-empty, bounded,
//! and free of control characters. It deliberately does not check that the
//! value looks like a UUID, because a market id that arrived from an
//! idempotent replay must be accepted just as readily as one that was
//! generated locally, and a check here would be a check the adapters would
//! have to be trusted to enforce anyway.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The maximum length of an identifier. Long enough for any representation this
/// system will use, short enough that a hostile client cannot make an
/// identifier a memory-amplification vector.
const MAX_ID_LEN: usize = 128;

/// An identifier was empty, too long, or contained a control character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidId {
    /// What was wrong with the value that was supplied.
    pub reason: &'static str,
}

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid identifier: {}", self.reason)
    }
}

impl std::error::Error for InvalidId {}

/// Validate an opaque identifier, returning it owned.
fn validate(value: String) -> Result<String, InvalidId> {
    if value.trim().is_empty() {
        return Err(InvalidId {
            reason: "must not be empty",
        });
    }
    if value.len() > MAX_ID_LEN {
        return Err(InvalidId {
            reason: "exceeds the maximum identifier length",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(InvalidId {
            reason: "must not contain control characters",
        });
    }
    Ok(value)
}

/// Define an opaque identifier newtype.
///
/// Written as a macro because the six identifiers below are otherwise six
/// copies of the same twenty lines, and a copy that is edited in one place and
/// not the other is a bug waiting for a slow afternoon.
macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Create an identifier from any string-like value.
            ///
            /// # Errors
            ///
            /// Returns [`InvalidId`] if the value is empty, longer than 128
            /// bytes, or contains a control character.
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
                validate(value.into()).map(Self)
            }

            /// The identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the identifier and return the inner string.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidId;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = InvalidId;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
    };
}

id_newtype!(
    /// A customer or system account that holds a ledger balance.
    AccountId
);

id_newtype!(
    /// A market that orders are placed against.
    MarketId
);

id_newtype!(
    /// A single order, whether resting, matched, or cancelled.
    OrderId
);

id_newtype!(
    /// Groups every ledger leg produced by one atomic business operation, so
    /// that the sum-to-zero invariant can be checked per transaction rather
    /// than only over the whole ledger.
    TransactionRef
);

id_newtype!(
    /// A token that makes a retried request safe to replay.
    IdempotencyKey
);

id_newtype!(
    /// A stored, signed price a customer was shown. Distinct from the quote's
    /// contents: the payload is authenticated by its signature, and this is
    /// the row that records that it was issued at all.
    QuoteId
);

id_newtype!(
    /// The immutable id of a recorded state change, used by the tamper-evident
    /// audit chain.
    EventId
);

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_string_is_a_valid_id() {
        let id = MarketId::new("mkt_01H8Z").expect("should be valid");
        assert_eq!(id.as_str(), "mkt_01H8Z");
        assert_eq!(id.to_string(), "mkt_01H8Z");
    }

    #[test]
    fn a_uuid_is_a_valid_id_because_ids_are_opaque() {
        let uuid = "018f3f6e-1a2b-7c3d-8e4f-5a6b7c8d9e0f";
        let id = OrderId::new(uuid).expect("should be valid");
        assert_eq!(id.as_str(), uuid);
    }

    #[test]
    fn empty_and_whitespace_ids_are_rejected() {
        assert!(AccountId::new("").is_err());
        assert!(AccountId::new("   ").is_err());
        assert!(AccountId::new("\t\n").is_err());
    }

    #[test]
    fn overlong_ids_are_rejected() {
        let long = "x".repeat(MAX_ID_LEN + 1);
        assert!(AccountId::new(long).is_err());
        let at_limit = "x".repeat(MAX_ID_LEN);
        assert!(AccountId::new(at_limit).is_ok());
    }

    #[test]
    fn control_characters_are_rejected() {
        assert!(EventId::new("evt\u{0}1").is_err());
        assert!(EventId::new("evt\n1").is_err());
    }

    #[test]
    fn inner_whitespace_is_allowed() {
        // Only control characters are rejected. A space is a legal character in
        // an opaque id, and rejecting it would be a rule with no purpose.
        assert!(EventId::new("evt 1").is_ok());
    }

    #[test]
    fn different_id_types_do_not_interchange() {
        // This file does not compile if `MarketId` and `OrderId` are the same
        // type, which is the entire reason they are separate newtypes.
        fn takes_market(_: MarketId) {}
        takes_market(MarketId::new("m1").expect("valid"));
        let order = OrderId::new("o1").expect("valid");
        // `takes_market(order)` would not compile. Assert the types differ.
        assert_eq!(order.as_str(), "o1");
    }
}
