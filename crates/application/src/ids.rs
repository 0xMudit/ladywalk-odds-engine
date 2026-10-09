//! # Identifier generation
//!
//! The one implementation of [`IdGenerator`], and the reason the trait exists at
//! all.
//!
//! ## Why not `newtype` inside the domain
//!
//! `domain` is infrastructure-free, and it is also the crate that defines what an
//! `OrderId` *is*: a validated, non-empty, bounded string. Deciding that new ids
//! are UUIDs is a different question — it is a question about deployment and
//! collision resistance across a fleet of instances, not about the rules of an
//! order. So the domain names the id and this crate supplies it.
//!
//! ## Why the counter for transaction references
//!
//! A `TransactionRef` groups the legs of one business operation, and it is
//! written into an idempotency key and into `ledger_entries`. Making it a UUID
//! would work, and would also make the ledger unreadable: a customer disputing a
//! charge needs to be shown `hold:m1:000042`, not a 32-character hex string they
//! cannot match against anything. A monotonic counter with a readable prefix is
//! chosen for that reason, and the counter is process-local — the reference only
//! has to be unique within a market's ledger, and uniqueness that actually
//! matters is enforced by the database's primary key rather than by the format.

use domain::ids::{AccountId, IdempotencyKey, MarketId, OrderId, QuoteId, TransactionRef};
use uuid::Uuid;

use crate::ports::{IdGenerator, StoreError};

/// Generates ids from UUIDv4 and a per-process counter.
#[derive(Debug)]
pub struct UuidGenerator {
    counter: u64,
    prefix: String,
}

impl UuidGenerator {
    /// A generator whose transaction references start at one.
    ///
    /// `prefix` is prepended to every generated reference, so references read as
    /// `order-entry:hold:m1:000042` — which names both the service that wrote it
    /// and the operation it belongs to.
    #[must_use]
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            counter: 0,
            prefix: prefix.into(),
        }
    }

    /// The next value in the counter, as six digits.
    fn next_seq(&mut self) -> Result<String, StoreError> {
        self.counter = self.counter.saturating_add(1);
        Ok(format!("{:06}", self.counter))
    }

    /// Build an id from a UUID, mapping the (unreachable) validation failure to
    /// a store error.
    fn uuid_id<T, F>(make: F) -> Result<T, StoreError>
    where
        F: FnOnce(String) -> Result<T, domain::ids::InvalidId>,
    {
        let raw = Uuid::new_v4().to_string();
        make(raw).map_err(|e| StoreError::Backend(format!("generated an invalid id: {e}")))
    }
}

impl Default for UuidGenerator {
    fn default() -> Self {
        Self::new("order-entry")
    }
}

impl IdGenerator for UuidGenerator {
    fn new_order_id(&mut self) -> OrderId {
        // `OrderId` validates a UUID it was handed the same way it validates any
        // other string, so this can only fail if the domain's rules and the
        // UUID format disagree — which the fallback handles by producing a valid
        // id rather than panicking on the money path.
        Self::uuid_id(OrderId::new).unwrap_or_else(|_| {
            let _ = self.next_seq();
            OrderId::new(format!("order-{}", Uuid::new_v4())).unwrap_or_else(|_| {
                // Unreachable: the fallback string is a valid identifier. Handled
                // rather than unwrapped because this crate denies `unwrap_used`,
                // and a panic on the order path is a worse outcome than any of
                // the strings above.
                let fallback = format!("{}-{}", self.prefix, self.counter);
                OrderId::new(fallback).unwrap_or(OrderId::new("order").expect("static id"))
            })
        })
    }

    fn new_transaction_ref(&mut self, prefix: &str) -> Result<TransactionRef, StoreError> {
        let seq = self.next_seq()?;
        let candidate = format!("{}:{prefix}:{seq}", self.prefix);
        // A long caller-supplied prefix can push the reference past the domain's
        // length limit. Falling back to the bare sequence rather than failing
        // keeps reference generation infallible in practice, and the reference is
        // still unique within the process.
        TransactionRef::new(candidate.clone())
            .or_else(|_| TransactionRef::new(seq.clone()))
            .map_err(|e| StoreError::Backend(format!("generated an invalid reference: {e}")))
    }

    fn new_quote_id(&mut self) -> Result<QuoteId, StoreError> {
        Self::uuid_id(QuoteId::new)
    }

    fn new_market_id(&mut self) -> Result<MarketId, StoreError> {
        Self::uuid_id(MarketId::new)
    }

    fn new_account_id(&mut self) -> Result<AccountId, StoreError> {
        Self::uuid_id(AccountId::new)
    }

    fn new_idempotency_key(&mut self, suffix: &str) -> Result<IdempotencyKey, StoreError> {
        let candidate = format!("{}:{suffix}", Uuid::new_v4());
        IdempotencyKey::new(candidate.clone())
            .or_else(|_| IdempotencyKey::new(Uuid::new_v4().to_string()))
            .map_err(|e| StoreError::Backend(format!("generated an invalid key: {e}")))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_ids_are_unique() {
        let mut generator = UuidGenerator::default();
        let first = generator.new_order_id();
        let second = generator.new_order_id();
        assert_ne!(first, second, "two orders must not share an id");
    }

    #[test]
    fn transaction_references_are_readable_and_ordered() {
        let mut generator = UuidGenerator::new("order-entry");
        let first = generator
            .new_transaction_ref("hold")
            .expect("generates");
        let second = generator
            .new_transaction_ref("hold")
            .expect("generates");

        assert_eq!(first.as_str(), "order-entry:hold:000001");
        assert_eq!(second.as_str(), "order-entry:hold:000002");
    }

    #[test]
    fn a_prefix_too_long_for_the_domain_limit_still_produces_a_reference() {
        let mut generator = UuidGenerator::new("order-entry");
        let long = "x".repeat(200);
        let reference = generator
            .new_transaction_ref(&long)
            .expect("falls back rather than failing the money path");
        assert!(reference.as_str().len() <= 128);
    }

    #[test]
    fn market_and_account_ids_are_unique() {
        let mut generator = UuidGenerator::default();
        assert_ne!(
            generator.new_market_id().expect("generates"),
            generator.new_market_id().expect("generates")
        );
        assert_ne!(
            generator.new_account_id().expect("generates"),
            generator.new_account_id().expect("generates")
        );
    }
}
