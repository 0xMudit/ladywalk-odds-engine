//! # Odds
//!
//! Odds are a ratio, and a ratio of money is still money. They are stored here
//! as an `i64` count of ten-thousandths — `2.50` is `25_000` — because the
//! alternative, a `f64`, would make settlement inexact, and an inexact payout
//! is a payout that does not sum to zero.
//!
//! ## Why a fixed scale rather than a general decimal
//!
//! A general decimal type would allow `2.5123456789`. Nothing in a betting
//! market has that many significant digits, and allowing them only widens the
//! space of values that must be range-checked, rounded, and proven to survive a
//! database round trip. The fixed scale gives a total function on a bounded
//! domain, which is a much easier thing to reason about and to test
//! exhaustively.
//!
//! ## The invariant that matters
//!
//! [`Odds`] is always at or above [`Odds::MINIMUM`], which is strictly greater
//! than 1.0. Odds below 1.0 describe a lay rather than a back, and odds at or
//! below zero are meaningless. The invariant is established in [`Odds::new`] and
//! [`Odds::parse`], so an `Odds` value that exists is an `Odds` value that can
//! be paid out.
//!
//! ## Rounding, and why there is no remainder to track
//!
//! [`Odds::payout`] rounds to whole minor units, which can discard a fraction
//! smaller than one pence. That fraction is deliberately **not** reported as a
//! field. It is not money — it is a quantity ten-thousandths the size of the
//! smallest representable amount — and giving it a name in minor units invites
//! it to be posted to the ledger, where it would break the sum-to-zero
//! invariant for the sake of a rounding artefact nobody can spend.
//!
//! What does matter is that the parts of a payout are internally consistent:
//! `profit` is always exactly `returned` minus the stake, computed from the
//! rounded figures rather than independently, so a caller that posts both legs
//! to a ledger cannot disagree with itself.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::money::MoneyAmount;

/// The number of ten-thousandths in 1.0. Two spellings of the same odds
/// therefore compare equal.
const SCALE: i64 = 10_000;

/// An error produced when constructing or applying [`Odds`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OddsError {
    /// The supplied value was not a decimal number.
    NotANumber(String),
    /// The odds were at or below [`Odds::MINIMUM`], or had more than four
    /// decimal places, or overflowed.
    OutOfRange,
}

impl fmt::Display for OddsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber(s) => write!(f, "odds are not a decimal number: {s}"),
            Self::OutOfRange => f.write_str("odds are out of range"),
        }
    }
}

impl std::error::Error for OddsError {}

/// What a winning backer receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payout {
    /// The total returned to the customer, stake included.
    pub returned: MoneyAmount,
    /// The portion of `returned` that is winnings rather than returned stake.
    ///
    /// Always exactly `returned` minus the original stake, so that the two
    /// figures can never contradict each other in a ledger.
    pub profit: MoneyAmount,
    /// The stake that was put at risk.
    pub stake: MoneyAmount,
}

/// Decimal odds, stored as an `i64` count of ten-thousandths.
///
/// `2.50` is stored as `25_000`. The invariant `self.0 >= MINIMUM.0` is
/// established by [`Odds::from_raw`] and [`Odds::parse`], and holds for every
/// value that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Odds(i64);

impl Odds {
    /// The smallest odds that can pay out: 1.01.
    ///
    /// A backer at 1.0 or below cannot profit, so accepting one would mean
    /// accepting an order whose only possible outcome is a loss.
    pub const MINIMUM: Self = Self(10_001);

    /// Construct odds from a count of ten-thousandths.
    ///
    /// # Errors
    ///
    /// Returns [`OddsError::OutOfRange`] if the value is below
    /// [`Odds::MINIMUM`].
    pub const fn from_raw(raw: i64) -> Result<Self, OddsError> {
        if raw < Self::MINIMUM.0 {
            Err(OddsError::OutOfRange)
        } else {
            Ok(Self(raw))
        }
    }

    /// Construct odds from the same count, without a range check.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `raw >= MINIMUM.0`. This exists for
    /// deserialising values that were validated on the way in; every other
    /// caller should use [`Odds::from_raw`].
    #[must_use]
    pub const unsafe fn from_raw_unchecked(raw: i64) -> Self {
        Self(raw)
    }

    /// The value as a count of ten-thousandths.
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Parse odds from a decimal string such as `"2.5"` or `"2.50"`.
    ///
    /// This exists rather than a `FromStr` impl for `f64` so that parsing never
    /// passes through a binary float. The text is read digit by digit and
    /// scaled as an integer, so `"2.5"`, `"2.50"` and `"2.500"` all produce
    /// exactly `25_000` with no rounding step to be wrong about.
    ///
    /// # Errors
    ///
    /// Returns [`OddsError::NotANumber`] if the text is not a decimal number,
    /// and [`OddsError::OutOfRange`] if it has more than four decimal places,
    /// is not greater than 1.0, or overflows.
    pub fn parse(text: &str) -> Result<Self, OddsError> {
        let not_a_number = || OddsError::NotANumber(text.to_string());

        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (text, ""),
        };

        if whole.is_empty() && fraction.is_empty() {
            return Err(not_a_number());
        }
        if !whole.bytes().all(|b| b.is_ascii_digit()) {
            return Err(not_a_number());
        }
        if !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return Err(not_a_number());
        }
        if fraction.len() > 4 {
            return Err(OddsError::OutOfRange);
        }

        let whole: i64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| OddsError::OutOfRange)?
        };

        // Pad the fraction out to four digits so "2.5" scales identically to
        // "2.5000". Input longer than four digits was rejected above, so this
        // pad never truncates a digit.
        let padded = format!("{fraction:0<4}");
        let fraction: i64 = padded.parse().map_err(|_| not_a_number())?;

        let scaled = whole
            .checked_mul(SCALE)
            .and_then(|w| w.checked_add(fraction))
            .ok_or(OddsError::OutOfRange)?;

        Self::from_raw(scaled)
    }

    /// What a winning backer receives in total, stake included.
    ///
    /// A stake of £10.00 at 2.50 returns £25.00, of which £15.00 is profit.
    ///
    /// Rounding is to the nearest minor unit, with exact halves away from zero.
    /// A float is not used to do it: that would put a binary approximation back
    /// into the money path, which is the one thing this module exists to
    /// prevent.
    ///
    /// # Errors
    ///
    /// Returns [`OddsError::OutOfRange`] if the result does not fit in `i64`.
    pub fn payout(self, stake: MoneyAmount) -> Result<Payout, OddsError> {
        // Widened to i128 before multiplying. Rounding needs a half-scale
        // addition, so a product that is merely close to the i64 ceiling would
        // overflow during rounding even though the rounded result itself fits —
        // the intermediate is larger than the answer, and treating the
        // intermediate as the bound would refuse payouts the customer is owed.
        let scaled = i128::from(stake.minor_units) * i128::from(self.0);
        let rounded = round_half_away_from_zero(scaled, i128::from(SCALE));
        let rounded = i64::try_from(rounded).map_err(|_| OddsError::OutOfRange)?;

        Ok(Payout {
            returned: MoneyAmount::new(rounded, stake.currency),
            // Derived from the rounded total, not from the unrounded product,
            // so the two figures cannot disagree.
            profit: MoneyAmount::new(rounded - stake.minor_units, stake.currency),
            stake,
        })
    }

    /// What the customer keeps if this bet loses: nothing.
    ///
    /// Named rather than written as `MoneyAmount::new(0, ..)` at the call site
    /// so that a reader of a settlement can see that losing the whole stake is
    /// a modelled outcome and not an oversight.
    #[must_use]
    pub fn loss_if_unbacked(self, stake: MoneyAmount) -> MoneyAmount {
        MoneyAmount::new(0, stake.currency)
    }
}

/// Divide `scaled` by `scale`, rounding to the nearest whole number with exact
/// halves away from zero.
///
/// Takes and returns `i128` because rounding adds half a scale, so the
/// intermediate can exceed the value it produces; computing in `i64` would
/// overflow on a payout whose answer is representable.
const fn round_half_away_from_zero(scaled: i128, scale: i128) -> i128 {
    let half = scale / 2;
    if scaled >= 0 {
        (scaled + half) / scale
    } else {
        -((-scaled + half) / scale)
    }
}

impl fmt::Display for Odds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / SCALE;
        let fraction = self.0 % SCALE;
        write!(f, "{whole}.{fraction:04}")
    }
}

impl FromStr for Odds {
    type Err = OddsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn gbp(minor_units: i64) -> MoneyAmount {
        MoneyAmount::new(minor_units, Currency::GBP)
    }

    #[test]
    fn parsing_is_exact_and_ignores_trailing_zeroes() {
        for text in ["2.5", "2.50", "2.500", "2.5000"] {
            let odds = Odds::parse(text).expect("should parse");
            assert_eq!(odds.raw(), 25_000, "text was {text}");
            assert_eq!(odds.to_string(), "2.5000");
        }
    }

    #[test]
    fn odds_at_or_below_one_are_rejected() {
        assert_eq!(Odds::parse("1.0000"), Err(OddsError::OutOfRange));
        assert_eq!(Odds::parse("0.5000"), Err(OddsError::OutOfRange));
        assert_eq!(Odds::parse("0"), Err(OddsError::OutOfRange));
        assert!(Odds::parse("1.0001").is_ok());
    }

    #[test]
    fn non_numeric_text_is_rejected() {
        for text in ["", ".", "abc", "2.5.5", "2,5", "-2.5", "2.5x", "1e3", "+2.5"] {
            assert!(
                Odds::parse(text).is_err(),
                "text {text:?} should have been rejected"
            );
        }
    }

    #[test]
    fn more_than_four_decimal_places_is_rejected() {
        assert_eq!(Odds::parse("2.50000"), Err(OddsError::OutOfRange));
    }

    #[test]
    fn a_fraction_with_no_whole_part_parses() {
        assert!(Odds::parse(".5").is_err(), "0.5 is below the minimum");
        assert!(Odds::parse(".0001").is_err());
    }

    #[test]
    fn payout_of_ten_pounds_at_two_and_a_half_is_twenty_five() {
        let odds = Odds::parse("2.5").expect("valid");
        let payout = odds.payout(gbp(1000)).expect("no overflow");
        assert_eq!(payout.returned, gbp(2500));
        assert_eq!(payout.profit, gbp(1500));
        assert_eq!(payout.stake, gbp(1000));
    }

    #[test]
    fn profit_is_always_returned_minus_stake() {
        // The property that lets a settlement post both figures without the
        // ledger disagreeing with itself.
        for raw in [10_101, 12_345, 25_000, 99_999] {
            let odds = Odds::from_raw(raw).expect("valid");
            for stake in [1, 7, 99, 1000, 123_456] {
                let payout = odds.payout(gbp(stake)).expect("no overflow");
                assert_eq!(
                    payout.profit.minor_units,
                    payout.returned.minor_units - stake,
                    "odds raw {raw}, stake {stake}"
                );
            }
        }
    }

    #[test]
    fn rounding_goes_to_the_nearest_minor_unit() {
        // 7 * 12345 == 86415, / 10000 == 8.6415 -> 9 minor units.
        let odds = Odds::parse("1.2345").expect("valid");
        let payout = odds.payout(gbp(7)).expect("no overflow");
        assert_eq!(payout.returned, gbp(9));
    }

    #[test]
    fn an_exact_half_rounds_away_from_zero() {
        // 1 * 10500 / 10000 == 1.05 -> 1, not 1.05. And 10501 -> 1.0501 -> 1.
        let odds = Odds::from_raw(10_500).expect("valid");
        let payout = odds.payout(gbp(1)).expect("no overflow");
        assert_eq!(payout.returned, gbp(1));

        let odds = Odds::from_raw(10_501).expect("valid");
        let payout = odds.payout(gbp(1)).expect("no overflow");
        assert_eq!(payout.returned, gbp(1));

        // 3 * 10500 / 10000 == 3.15 -> 3.
        let payout = odds.payout(gbp(3)).expect("no overflow");
        assert_eq!(payout.returned, gbp(3));
    }

    #[test]
    fn a_losing_backer_keeps_nothing() {
        let odds = Odds::parse("3.0").expect("valid");
        assert_eq!(odds.loss_if_unbacked(gbp(500)), gbp(0));
    }

    #[test]
    fn payout_of_zero_stake_is_zero() {
        let odds = Odds::parse("2.5").expect("valid");
        let payout = odds.payout(gbp(0)).expect("no overflow");
        assert_eq!(payout.returned, gbp(0));
        assert_eq!(payout.profit, gbp(0));
    }

    #[test]
    fn a_very_large_stake_reports_overflow_rather_than_wrapping() {
        let odds = Odds::parse("1000.0000").expect("valid");
        assert_eq!(odds.payout(gbp(i64::MAX)), Err(OddsError::OutOfRange));
    }

    #[test]
    fn odds_display_round_trips() {
        for text in ["1.0101", "2.5000", "12.3456", "100.0000"] {
            let odds = Odds::parse(text).expect("valid");
            assert_eq!(odds.to_string(), text);
            assert_eq!(Odds::parse(&odds.to_string()), Ok(odds));
        }
    }

    #[test]
    fn rounding_of_a_half_away_from_zero_is_symmetric() {
        // 5/10 == 0.5 -> 1, and -5/10 == -0.5 -> -1. Banker's rounding would
        // give 0 for both.
        assert_eq!(round_half_away_from_zero(5, 10), 1);
        assert_eq!(round_half_away_from_zero(-5, 10), -1);
        assert_eq!(round_half_away_from_zero(0, 10), 0);
        assert_eq!(round_half_away_from_zero(4, 10), 0);
    }

    #[test]
    fn a_payout_whose_answer_fits_but_whose_intermediate_does_not_is_still_paid() {
        // A stake near the i64 ceiling at 1.01 has a product far beyond i64,
        // while the answer — the stake itself, plus about one percent of it —
        // is representable. Refusing it would refuse a real payout to guard an
        // intermediate that the widened arithmetic no longer has to worry
        // about.
        let odds = Odds::from_raw(10_100).expect("valid");
        let stake = i64::MAX / 10_100;
        let payout = odds.payout(gbp(stake)).expect("no overflow");

        let expected = (i128::from(stake) * 10_100 + 5_000) / 10_000;
        assert_eq!(i128::from(payout.returned.minor_units), expected);
        assert!(payout.returned.minor_units > stake);
    }
}
