//! # Quotes
//!
//! The server decides the price, and the price is presented to the customer as
//! a short-lived signed token. This module owns *what* a quote says; the
//! signing itself lives in `application`, because signing needs a secret and a
//! secret is infrastructure.
//!
//! ## The problem this solves
//!
//! A customer who is quoted 2.50 and whose order is filled at 1.90 has been
//! robbed, and the natural defence — validate the odds the client sent — does
//! not work, because the client chose the number. So the number the client
//! holds is not a number at all: it is a token whose payload the server signed
//! and whose price the server will honour. The client can read the price out of
//! it for display, but it cannot choose it.
//!
//! ## What a token commits to
//!
//! The payload binds the market, the selection, the odds, the stake, and an
//! expiry. Two consequences follow, and both are deliberate:
//!
//! - A token cannot be replayed onto a different stake, because the stake is
//!   inside the signed bytes.
//! - A token cannot be reused later, because the expiry is inside them and is
//!   checked against the caller's clock, not the client's.
//!
//! ## Signature comparison
//!
//! [`Quote::verify`] compares signatures with [`constant_time_eq`] rather than
//! `==`. A byte-by-byte comparison that returns early leaks how much of a
//! forged signature was correct, which is enough to recover a signature one
//! byte at a time. The comparison is written out rather than pulled from a crate
//! so that the money path keeps its dependency count where the README says it
//! is.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{AccountId, MarketId};
use crate::money::MoneyAmount;
use crate::odds::Odds;
use crate::order::Selection;

/// How long a quote stays valid, in seconds. Long enough for a customer to
/// read the price, short enough that a stale price is not a customer being
/// quoted a market that has since moved.
pub const DEFAULT_TTL_SECS: i64 = 10;

/// A short-lived, server-signed price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    /// The market the price applies to.
    pub market_id: MarketId,
    /// The selection the price applies to.
    pub selection: Selection,
    /// The price.
    pub odds: Odds,
    /// The stake the price was quoted for, in minor units.
    pub stake_minor_units: i64,
    /// The currency the stake is in.
    pub currency_code: String,
    /// When the quote stops being honoured, as a Unix timestamp in seconds.
    pub expires_at_unix: i64,
    /// The account the quote was issued to.
    pub account_id: AccountId,
    /// The server's signature over every other field.
    pub signature: String,
}

/// What was wrong with a quote.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuoteError {
    /// The signature did not match the payload.
    BadSignature,
    /// The quote has expired.
    Expired,
    /// The quote was issued for a different market.
    MarketMismatch,
    /// The quote was issued for a different selection.
    SelectionMismatch,
    /// The quote was issued for a different stake.
    StakeMismatch,
    /// The quote was issued to a different account.
    AccountMismatch,
}

impl fmt::Display for QuoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadSignature => f.write_str("quote signature is not valid"),
            Self::Expired => f.write_str("quote has expired"),
            Self::MarketMismatch => f.write_str("quote was issued for a different market"),
            Self::SelectionMismatch => f.write_str("quote was issued for a different selection"),
            Self::StakeMismatch => f.write_str("quote was issued for a different stake"),
            Self::AccountMismatch => f.write_str("quote was issued to a different account"),
        }
    }
}

impl std::error::Error for QuoteError {}

impl From<QuoteError> for DomainError {
    fn from(error: QuoteError) -> Self {
        match error {
            // A bad signature is reported as a stale quote rather than as an
            // authentication failure, because a client with a forged token has
            // not done anything the client could recover from, and saying
            // "signature invalid" would tell them exactly that.
            QuoteError::BadSignature
            | QuoteError::Expired
            | QuoteError::MarketMismatch
            | QuoteError::SelectionMismatch
            | QuoteError::StakeMismatch
            | QuoteError::AccountMismatch => Self::StaleQuote,
        }
    }
}

impl Quote {
    /// Build an unsigned quote. [`application`] signs it.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidInput`] if the stake is not positive, or
    /// if `now_unix + ttl_secs` overflows, which would produce a quote that is
    /// already expired.
    pub fn unsigned(
        market_id: MarketId,
        selection: Selection,
        odds: Odds,
        stake: MoneyAmount,
        account_id: AccountId,
        now_unix: i64,
        ttl_secs: i64,
    ) -> Result<Self, DomainError> {
        if !stake.is_positive() {
            return Err(DomainError::InvalidInput {
                field: "stake",
                reason: "stake must be greater than zero".to_string(),
            });
        }
        let expires_at_unix = now_unix.checked_add(ttl_secs).ok_or(DomainError::InvalidInput {
            field: "ttl_secs",
            reason: "the quote expiry is not representable".to_string(),
        })?;
        Ok(Self {
            market_id,
            selection,
            odds,
            stake_minor_units: stake.minor_units,
            currency_code: stake.currency.to_string(),
            expires_at_unix,
            account_id,
            signature: String::new(),
        })
    }

    /// The bytes a signature is computed over: every field except the
    /// signature itself.
    ///
    /// Field order is fixed and a separator that cannot appear in a currency
    /// code or an ISO timestamp is used between them, so that no two distinct
    /// payloads can produce the same signing input.
    #[must_use]
    pub fn signing_input(&self) -> String {
        format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
            self.market_id.as_str(),
            self.selection,
            self.odds.raw(),
            self.stake_minor_units,
            self.currency_code,
            self.expires_at_unix,
            self.account_id.as_str(),
        )
    }

    /// The stake as a typed amount.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidInput`] if the stored currency code is not
    /// one this build knows.
    pub fn stake(&self) -> Result<MoneyAmount, DomainError> {
        let currency = crate::money::Currency::from_code(&self.currency_code).map_err(|_| {
            DomainError::InvalidInput {
                field: "currency",
                reason: format!("unknown currency code: {}", self.currency_code),
            }
        })?;
        Ok(MoneyAmount::new(self.stake_minor_units, currency))
    }

    /// Whether the quote is still valid at `now_unix`, ignoring its signature.
    #[must_use]
    pub fn is_fresh_at(&self, now_unix: i64) -> bool {
        now_unix < self.expires_at_unix
    }

    /// Check the signature and the freshness, without checking the order it
    /// will be applied to.
    ///
    /// # Errors
    ///
    /// Returns [`QuoteError::BadSignature`] if the signature does not match,
    /// and [`QuoteError::Expired`] if `now_unix` is at or past the expiry.
    pub fn verify(&self, expected_signature: &str, now_unix: i64) -> Result<(), QuoteError> {
        if !constant_time_eq(self.signature.as_bytes(), expected_signature.as_bytes()) {
            return Err(QuoteError::BadSignature);
        }
        if !self.is_fresh_at(now_unix) {
            return Err(QuoteError::Expired);
        }
        Ok(())
    }

    /// Check that a quote may be used for a specific order.
    ///
    /// Every field the token signed is compared, not just the market: a token
    /// is a promise about one order, and honouring it for a different order is
    /// the same as letting the client choose the price.
    ///
    /// # Errors
    ///
    /// Returns the specific [`QuoteError`] for the first mismatch, or
    /// [`QuoteError::BadSignature`] if the signature is wrong.
    pub fn verify_for(
        &self,
        expected_signature: &str,
        now_unix: i64,
        market_id: &MarketId,
        selection: Selection,
        stake: MoneyAmount,
        account_id: &AccountId,
    ) -> Result<(), QuoteError> {
        self.verify(expected_signature, now_unix)?;
        if self.market_id != *market_id {
            return Err(QuoteError::MarketMismatch);
        }
        if self.selection != selection {
            return Err(QuoteError::SelectionMismatch);
        }
        if self.stake_minor_units != stake.minor_units || self.currency_code != stake.currency.to_string()
        {
            return Err(QuoteError::StakeMismatch);
        }
        if self.account_id != *account_id {
            return Err(QuoteError::AccountMismatch);
        }
        Ok(())
    }

    /// The signature a token needs in order to be honoured, for an error
    /// message or a log line that must not be actionable.
    #[must_use]
    pub fn signature(&self) -> &str {
        &self.signature
    }
}

/// Compare two byte strings without leaking where they first differ.
///
/// The length is compared first, which does leak the length of the expected
/// signature. That is acceptable: the expected value is this system's own
/// output, so its length is not a secret the attacker does not already have.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (x, y) in a.iter().zip(b) {
        difference |= x ^ y;
    }
    difference == 0
}

impl fmt::Display for Quote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} at {} for {} (expires {})",
            self.selection,
            self.market_id,
            self.odds,
            self.stake_minor_units,
            self.expires_at_unix
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    const SECRET_SIGNATURE: &str = "server-computed-signature";

    fn quote(stake: i64) -> Quote {
        let mut q = Quote::unsigned(
            MarketId::new("m1").expect("valid"),
            Selection::Home,
            Odds::parse("2.5").expect("valid"),
            MoneyAmount::new(stake, Currency::GBP),
            AccountId::new("a1").expect("valid"),
            1_000,
            DEFAULT_TTL_SECS,
        )
        .expect("valid quote");
        q.signature = SECRET_SIGNATURE.to_string();
        q
    }

    #[test]
    fn a_signed_unexpired_quote_verifies() {
        let q = quote(1000);
        assert_eq!(q.verify(SECRET_SIGNATURE, 1_000), Ok(()));
        assert!(q.is_fresh_at(1_005));
    }

    #[test]
    fn a_quote_expires_exactly_at_its_expiry() {
        let q = quote(1000);
        assert!(q.is_fresh_at(1_009));
        assert!(!q.is_fresh_at(1_010), "expires_at is exclusive");
        assert_eq!(q.verify(SECRET_SIGNATURE, 1_010), Err(QuoteError::Expired));
    }

    #[test]
    fn a_forged_signature_is_rejected() {
        let mut q = quote(1000);
        q.signature = "attacker-guess".to_string();
        assert_eq!(q.verify(SECRET_SIGNATURE, 1_000), Err(QuoteError::BadSignature));
    }

    #[test]
    fn an_empty_signature_cannot_verify_against_a_real_one() {
        let mut q = quote(1000);
        q.signature.clear();
        assert_eq!(q.verify("", 1_000), Ok(()), "empty matches empty");
        assert_eq!(q.verify(SECRET_SIGNATURE, 1_000), Err(QuoteError::BadSignature));
    }

    #[test]
    fn tampering_with_any_signed_field_fails_verification() {
        // Each field below is covered by signing_input. If one were not, the
        // token would be replayable with a different value.
        let base = quote(1000);

        let mut market = base.clone();
        market.market_id = MarketId::new("m2").expect("valid");
        assert_ne!(market.signing_input(), base.signing_input());

        let mut odds = base.clone();
        odds.odds = Odds::parse("9.9").expect("valid");
        assert_ne!(odds.signing_input(), base.signing_input());

        let mut stake = base.clone();
        stake.stake_minor_units = 1;
        assert_ne!(stake.signing_input(), base.signing_input());

        let mut expiry = base.clone();
        expiry.expires_at_unix = 999_999;
        assert_ne!(expiry.signing_input(), base.signing_input());

        let mut account = base.clone();
        account.account_id = AccountId::new("a2").expect("valid");
        assert_ne!(account.signing_input(), base.signing_input());
    }

    #[test]
    fn the_separator_cannot_be_smuggled_into_a_signed_field() {
        // The separator only makes the signing input unambiguous if no field can
        // contain it, so the guarantee this test checks is the one in `ids`:
        // a control character cannot be part of a validated identifier. Without
        // it, an attacker could move the first character of the account into
        // the market field and produce a second token with the same signing
        // input.
        assert!(
            MarketId::new("a\x1fGBP").is_err(),
            "an identifier containing the separator must be rejected"
        );
        assert!(AccountId::new("a\x1fGBP").is_err());
    }

    #[test]
    fn moving_text_across_a_field_boundary_changes_the_signing_input() {
        // Two payloads whose fields concatenate to the same string without
        // separators. With the separator present they are different inputs, so
        // one signature cannot be replayed as the other.
        let a = Quote {
            market_id: MarketId::new("ab").expect("valid"),
            account_id: AccountId::new("c").expect("valid"),
            ..quote(1)
        };
        let b = Quote {
            market_id: MarketId::new("a").expect("valid"),
            account_id: AccountId::new("bc").expect("valid"),
            ..quote(1)
        };
        assert_ne!(a.signing_input(), b.signing_input());
    }

    #[test]
    fn a_quote_cannot_be_replayed_onto_another_stake() {
        let q = quote(1000);
        let result = q.verify_for(
            SECRET_SIGNATURE,
            1_000,
            &MarketId::new("m1").expect("valid"),
            Selection::Home,
            MoneyAmount::new(5_000, Currency::GBP),
            &AccountId::new("a1").expect("valid"),
        );
        assert_eq!(result, Err(QuoteError::StakeMismatch));
    }

    #[test]
    fn a_quote_cannot_be_replayed_onto_another_market_or_selection() {
        let q = quote(1000);
        assert_eq!(
            q.verify_for(
                SECRET_SIGNATURE,
                1_000,
                &MarketId::new("m2").expect("valid"),
                Selection::Home,
                MoneyAmount::new(1000, Currency::GBP),
                &AccountId::new("a1").expect("valid"),
            ),
            Err(QuoteError::MarketMismatch)
        );
        assert_eq!(
            q.verify_for(
                SECRET_SIGNATURE,
                1_000,
                &MarketId::new("m1").expect("valid"),
                Selection::Away,
                MoneyAmount::new(1000, Currency::GBP),
                &AccountId::new("a1").expect("valid"),
            ),
            Err(QuoteError::SelectionMismatch)
        );
    }

    #[test]
    fn a_quote_cannot_be_replayed_onto_another_customer() {
        let q = quote(1000);
        assert_eq!(
            q.verify_for(
                SECRET_SIGNATURE,
                1_000,
                &MarketId::new("m1").expect("valid"),
                Selection::Home,
                MoneyAmount::new(1000, Currency::GBP),
                &AccountId::new("a2").expect("valid"),
            ),
            Err(QuoteError::AccountMismatch)
        );
    }

    #[test]
    fn a_quote_with_a_non_positive_stake_is_refused() {
        let result = Quote::unsigned(
            MarketId::new("m1").expect("valid"),
            Selection::Home,
            Odds::parse("2.5").expect("valid"),
            MoneyAmount::new(0, Currency::GBP),
            AccountId::new("a1").expect("valid"),
            1_000,
            DEFAULT_TTL_SECS,
        );
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn an_expiry_that_cannot_be_represented_is_refused_rather_than_wrapping() {
        // Wrapping would produce an expiry in the distant past, and the quote
        // would be born expired — or, worse, born valid for centuries.
        let result = Quote::unsigned(
            MarketId::new("m1").expect("valid"),
            Selection::Home,
            Odds::parse("2.5").expect("valid"),
            MoneyAmount::new(1_000, Currency::GBP),
            AccountId::new("a1").expect("valid"),
            i64::MAX,
            DEFAULT_TTL_SECS,
        );
        assert!(matches!(result, Err(DomainError::InvalidInput { .. })));
    }

    #[test]
    fn every_quote_failure_is_reported_to_the_client_as_a_stale_quote() {
        // Telling a caller *which* field mismatched would tell them how to
        // adjust a token, which is the beginning of price manipulation.
        for error in [
            QuoteError::BadSignature,
            QuoteError::Expired,
            QuoteError::MarketMismatch,
            QuoteError::SelectionMismatch,
            QuoteError::StakeMismatch,
            QuoteError::AccountMismatch,
        ] {
            assert!(matches!(
                DomainError::from(error),
                DomainError::StaleQuote
            ));
        }
    }

    #[test]
    fn constant_time_eq_matches_the_stdlib() {
        for (a, b) in [("", ""), ("a", "a"), ("a", "b"), ("ab", "ab"), ("ab", "abc")] {
            assert_eq!(
                constant_time_eq(a.as_bytes(), b.as_bytes()),
                a == b,
                "mismatch for {a:?} vs {b:?}"
            );
        }
    }
}
