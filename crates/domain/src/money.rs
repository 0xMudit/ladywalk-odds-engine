//! # Money
//!
//! A [`MoneyAmount`] value is an `i64` count of **minor units** (e.g. pence,
//! cents) combined with an explicit [`Currency`]. This representation satisfies
//! Q01: money is never a float anywhere in the system.
//!
//! ## Design decisions
//!
//! - Two fields, so the amount and the unit it is denominated in cannot be
//!   separated. The database stores them as a `BIGINT` and a currency code
//!   rather than a single value, so an amount read from a row is re-wrapped
//!   through [`MoneyAmount::new`] rather than transmuted.
//! - No `From<f64>` / `From<f32>` — the only path in or out of a float is the
//!   explicit, named, audited function [`MoneyAmount::from_float_lossy`], which
//!   is intentionally marked `#[deprecated]` so it cannot be used without a
//!   deliberate `#[allow]` and a written justification.
//! - Arithmetic is **checked** — overflow returns `Err(MoneyError::Overflow)`,
//!   never wraps silently.
//! - Division returns the **quotient and the remainder**, so no minor units are
//!   silently discarded. It rounds half to even.
//!
//! ## Two rounding rules, on purpose
//!
//! [`MoneyAmount::div_i64`] rounds half to even and
//! [`MoneyAmount::from_float_lossy`] rounds half away from zero. They disagree,
//! and each is documented where it is used. The reason for the split is that
//! they answer different questions: a split of a pot is many small divisions
//! whose total should not drift, so it rounds to even; a single conversion of
//! an externally-supplied decimal is one value that a human wrote down, and
//! half away from zero is the rule a human expects when they wrote `0.005`.
//! Neither rule is applied to the other, so the two answers can never meet
//! inside a calculation.
//!
//! ## Non-goals
//!
//! This type does not know exchange rates. A cross-currency operation is
//! always `Err(MoneyError::CurrencyMismatch)`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Currency
// ─────────────────────────────────────────────────────────────────────────────

/// ISO 4217 currency code with its minor-unit exponent.
///
/// The exponent is the number of decimal places in the minor unit:
/// GBP → 2 (pence), JPY → 0, KWD → 3.
///
/// Ordering is by ISO code, so a `(AccountId, Currency)` pair can key the
/// ordered maps the stores are built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Currency {
    /// ISO 4217 three-letter code, e.g. `"GBP"`.
    pub code: [u8; 3],
    /// Minor-unit exponent (decimal places). GBP → 2, JPY → 0.
    pub exponent: u8,
}

impl Currency {
    /// British Pound Sterling — 2 decimal places (pence).
    pub const GBP: Self = Self {
        code: *b"GBP",
        exponent: 2,
    };
    /// United States Dollar — 2 decimal places (cents).
    pub const USD: Self = Self {
        code: *b"USD",
        exponent: 2,
    };
    /// Euro — 2 decimal places (cents).
    pub const EUR: Self = Self {
        code: *b"EUR",
        exponent: 2,
    };

    /// The multiplier from whole units to minor units (10^exponent).
    #[must_use]
    pub fn minor_unit_factor(self) -> i64 {
        10_i64.pow(u32::from(self.exponent))
    }

    /// The currency for an ISO 4217 code.
    ///
    /// The set is closed rather than open. A currency this build does not know
    /// is one it has no exponent for, and guessing an exponent is how a
    /// JPY-denominated amount ends up being treated as if it had two decimal
    /// places — a factor of one hundred, posted to the ledger.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::UnknownCurrency`] for any code this build does not
    /// support. Adding one is a code change, reviewed and tested like any other.
    pub const fn from_code(code: &str) -> Result<Self, MoneyError> {
        // Byte-array patterns match only a three-byte code, so a wrong-length
        // string falls into the catch-all with the unknown codes.
        match code.as_bytes() {
            [b'G', b'B', b'P'] => Ok(Self::GBP),
            [b'U', b'S', b'D'] => Ok(Self::USD),
            [b'E', b'U', b'R'] => Ok(Self::EUR),
            _ => Err(MoneyError::UnknownCurrency),
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: codes are always 3 ASCII bytes, valid UTF-8.
        let s = std::str::from_utf8(&self.code).unwrap_or("???");
        f.write_str(s)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MoneyError
// ─────────────────────────────────────────────────────────────────────────────

/// Errors that can occur during money arithmetic.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MoneyError {
    /// Arithmetic would overflow `i64`.
    Overflow,

    /// Attempted to combine amounts in different currencies without conversion.
    CurrencyMismatch(Currency, Currency),

    /// Division by zero.
    DivisionByZero,

    /// A string could not be parsed as a money amount.
    InvalidFormat(String),

    /// The currency code is not one this build supports.
    UnknownCurrency,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("arithmetic overflow on money amount"),
            Self::CurrencyMismatch(a, b) => {
                write!(f, "currency mismatch: {a} vs {b}")
            }
            Self::DivisionByZero => f.write_str("division by zero"),
            Self::InvalidFormat(s) => write!(f, "invalid money string: {s}"),
            Self::UnknownCurrency => f.write_str("unknown currency code"),
        }
    }
}

impl std::error::Error for MoneyError {}

// ─────────────────────────────────────────────────────────────────────────────
// DivisionResult
// ─────────────────────────────────────────────────────────────────────────────

/// The result of [`Money::div_i64`]: quotient **and** remainder, so no minor
/// units are ever silently discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DivisionResult {
    /// The quotient (rounded with banker's rounding).
    pub quotient: MoneyAmount,
    /// The remainder in minor units (always `0 ≤ remainder < divisor`).
    pub remainder_minor_units: i64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Money
// ─────────────────────────────────────────────────────────────────────────────

/// An amount of money in a specific currency, stored as `i64` minor units.
///
/// # Invariants
///
/// - The inner value is always in minor units (e.g. pence for GBP).
/// - All arithmetic is checked; overflow is `Err`, not wrap.
/// - No implicit float conversion exists.
///
/// # Examples
///
/// ```
/// use domain::money::{Currency, Money};
///
/// let five_pounds = Money::new(500, Currency::GBP); // 500 pence = £5.00
/// let ten_pounds  = Money::new(1000, Currency::GBP);
///
/// let total = (five_pounds + ten_pounds).unwrap();
/// assert_eq!(total.minor_units, 1500);
/// assert_eq!(total.to_string(), "15.00 GBP");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoneyAmount {
    /// The amount in minor units.
    pub minor_units: i64,
    /// The currency.
    pub currency: Currency,
}

impl MoneyAmount {
    /// Create a new `MoneyAmount`.
    #[must_use]
    pub const fn new(minor_units: i64, currency: Currency) -> Self {
        Self {
            minor_units,
            currency,
        }
    }

    /// Checked addition; fails on overflow or currency mismatch.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] or [`MoneyError::CurrencyMismatch`].
    pub fn checked_add(self, rhs: Self) -> Result<Self, MoneyError> {
        if self.currency != rhs.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, rhs.currency));
        }
        let minor_units = self
            .minor_units
            .checked_add(rhs.minor_units)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor_units,
            currency: self.currency,
        })
    }

    /// Checked subtraction; fails on overflow or currency mismatch.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] or [`MoneyError::CurrencyMismatch`].
    pub fn checked_sub(self, rhs: Self) -> Result<Self, MoneyError> {
        if self.currency != rhs.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, rhs.currency));
        }
        let minor_units = self
            .minor_units
            .checked_sub(rhs.minor_units)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor_units,
            currency: self.currency,
        })
    }

    /// Checked multiplication by an integer scalar.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`].
    pub fn checked_mul_i64(self, rhs: i64) -> Result<Self, MoneyError> {
        let minor_units = self
            .minor_units
            .checked_mul(rhs)
            .ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor_units,
            currency: self.currency,
        })
    }

    /// Division with **banker's rounding** (round half to even).
    ///
    /// Returns both the quotient and the remainder so no minor units are
    /// silently discarded.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::DivisionByZero`] or [`MoneyError::Overflow`].
    pub fn div_i64(self, divisor: i64) -> Result<DivisionResult, MoneyError> {
        if divisor == 0 {
            return Err(MoneyError::DivisionByZero);
        }
        let quotient_raw = self
            .minor_units
            .checked_div(divisor)
            .ok_or(MoneyError::Overflow)?;
        let remainder_raw = self
            .minor_units
            .checked_rem(divisor)
            .ok_or(MoneyError::Overflow)?;

        // Banker's rounding: if the remainder is exactly half the divisor,
        // round to even; otherwise standard rounding.
        let twice_remainder = remainder_raw
            .abs()
            .checked_mul(2)
            .ok_or(MoneyError::Overflow)?;
        let divisor_abs = divisor.abs();
        let step_away_from_zero = if quotient_raw >= 0 { 1 } else { -1 };
        let quotient_minor = match twice_remainder.cmp(&divisor_abs) {
            // Exactly halfway — round to even.
            std::cmp::Ordering::Equal => {
                if quotient_raw % 2 == 0 {
                    quotient_raw
                } else {
                    quotient_raw
                        .checked_add(step_away_from_zero)
                        .ok_or(MoneyError::Overflow)?
                }
            }
            // More than half a divisor — round away from zero.
            std::cmp::Ordering::Greater => quotient_raw
                .checked_add(step_away_from_zero)
                .ok_or(MoneyError::Overflow)?,
            // Less than half a divisor — truncate.
            std::cmp::Ordering::Less => quotient_raw,
        };

        Ok(DivisionResult {
            quotient: MoneyAmount {
                minor_units: quotient_minor,
                currency: self.currency,
            },
            remainder_minor_units: remainder_raw.abs(),
        })
    }

    /// Whether this amount is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.minor_units == 0
    }

    /// Whether this amount is strictly positive.
    #[must_use]
    pub fn is_positive(self) -> bool {
        self.minor_units > 0
    }

    /// Whether this amount is strictly negative.
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.minor_units < 0
    }

    /// Negate this amount (checked).
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] when `minor_units == i64::MIN`.
    pub fn checked_neg(self) -> Result<Self, MoneyError> {
        let minor_units = self.minor_units.checked_neg().ok_or(MoneyError::Overflow)?;
        Ok(Self {
            minor_units,
            currency: self.currency,
        })
    }

    /// **Explicitly lossy** conversion from a float. Rounds to the nearest
    /// minor unit, with exact halves rounded away from zero.
    ///
    /// # ⚠ Deprecation
    ///
    /// This function exists only for migration tooling and test fixtures where
    /// a human-entered float must be imported. It MUST NOT be used anywhere in
    /// the money path. Every call site requires an `#[allow(deprecated)]` with
    /// a written justification comment. CI audits for undocumented usages.
    ///
    /// # Panics
    ///
    /// Never panics. `NaN` / `Inf` maps to `Err`.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::InvalidFormat`] if the value is `NaN` or infinite,
    /// and [`MoneyError::Overflow`] if it is outside the `i64` range.
    #[deprecated(
        since = "0.1.0",
        note = "AUDIT REQUIRED: float→money is lossy. Only permitted for test fixtures and migration tooling."
    )]
    // The f64 <-> i64 casts below are the whole point of a function that is
    // already deprecated for being lossy, and they are guarded: the finite
    // check rejects NaN and Inf, and the range check below runs before the
    // truncating cast rather than after it.
    #[allow(clippy::cast_precision_loss)]
    pub fn from_float_lossy(value: f64, currency: Currency) -> Result<Self, MoneyError> {
        if !value.is_finite() {
            return Err(MoneyError::InvalidFormat(format!(
                "non-finite float cannot represent money: {value}"
            )));
        }
        let factor = currency.minor_unit_factor() as f64;
        let scaled = (value * factor).round();
        // Check range before cast
        if scaled > i64::MAX as f64 || scaled < i64::MIN as f64 {
            return Err(MoneyError::Overflow);
        }
        #[allow(clippy::cast_possible_truncation)]
        let minor_units = scaled as i64;
        Ok(Self {
            minor_units,
            currency,
        })
    }
}

impl fmt::Display for MoneyAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let factor = self.currency.minor_unit_factor();
        let whole = self.minor_units / factor;
        let frac = (self.minor_units % factor).abs();
        let exp = usize::from(self.currency.exponent);
        write!(f, "{whole}.{frac:0>exp$} {}", self.currency)
    }
}

impl FromStr for MoneyAmount {
    type Err = MoneyError;

    /// Parse `"12.50 GBP"` or `"1250"` (minor units, currency inferred as GBP).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // Try "AMOUNT CURRENCY" format
        if let Some((amount_str, currency_str)) = s.split_once(' ') {
            let currency = match currency_str.trim() {
                "GBP" => Currency::GBP,
                "USD" => Currency::USD,
                "EUR" => Currency::EUR,
                other => {
                    return Err(MoneyError::InvalidFormat(format!(
                        "unknown currency: {other}"
                    )))
                }
            };
            let factor = currency.minor_unit_factor();
            if let Some((whole_str, frac_str)) = amount_str.split_once('.') {
                let whole: i64 = whole_str
                    .parse()
                    .map_err(|_| MoneyError::InvalidFormat(format!("invalid amount: {s}")))?;
                // Left-pad / truncate fractional part to the exponent width
                let exp = usize::from(currency.exponent);
                let frac_padded = format!("{frac_str:0<exp$}");
                let frac_padded = &frac_padded[..exp.min(frac_padded.len())];
                let frac: i64 = frac_padded
                    .parse()
                    .map_err(|_| MoneyError::InvalidFormat(format!("invalid fraction: {s}")))?;
                let minor_units = whole
                    .checked_mul(factor)
                    .and_then(|w| w.checked_add(if whole >= 0 { frac } else { -frac }))
                    .ok_or(MoneyError::Overflow)?;
                return Ok(Self {
                    minor_units,
                    currency,
                });
            }
            let minor_units: i64 = amount_str
                .parse()
                .map_err(|_| MoneyError::InvalidFormat(format!("invalid amount: {s}")))?;
            return Ok(Self {
                minor_units,
                currency,
            });
        }
        Err(MoneyError::InvalidFormat(format!(
            "expected \"<amount> <CURRENCY>\", got: {s}"
        )))
    }
}

// Convenience type alias — most call sites deal with a MoneyAmount.
/// A [`MoneyAmount`] is the canonical way to pass money with its currency.
pub use MoneyAmount as Money;

// ─────────────────────────────────────────────────────────────────────────────
// Operator overloads (delegate to checked arithmetic, return Result)
// ─────────────────────────────────────────────────────────────────────────────

impl std::ops::Add for MoneyAmount {
    type Output = Result<Self, MoneyError>;
    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs)
    }
}

impl std::ops::Sub for MoneyAmount {
    type Output = Result<Self, MoneyError>;
    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs)
    }
}

impl std::ops::Mul<i64> for MoneyAmount {
    type Output = Result<Self, MoneyError>;
    fn mul(self, rhs: i64) -> Self::Output {
        self.checked_mul_i64(rhs)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn gbp(p: i64) -> MoneyAmount {
        MoneyAmount::new(p, Currency::GBP)
    }

    // --- basic properties ---------------------------------------------------

    #[test]
    fn zero_is_identity_for_addition() {
        let five = gbp(500);
        let zero = gbp(0);
        assert_eq!((five + zero).unwrap(), five);
        assert_eq!((zero + five).unwrap(), five);
    }

    #[test]
    fn addition_is_commutative() {
        let a = gbp(300);
        let b = gbp(250);
        assert_eq!((a + b).unwrap(), (b + a).unwrap());
    }

    #[test]
    fn overflow_is_error_not_wrap() {
        let max = MoneyAmount::new(i64::MAX, Currency::GBP);
        let one = gbp(1);
        assert_eq!((max + one).unwrap_err(), MoneyError::Overflow);
    }

    #[test]
    fn currency_mismatch_is_error() {
        let gbp_val = gbp(100);
        let usd_val = MoneyAmount::new(100, Currency::USD);
        assert!(matches!(
            (gbp_val + usd_val).unwrap_err(),
            MoneyError::CurrencyMismatch(_, _)
        ));
    }

    // --- display ------------------------------------------------------------

    #[test]
    fn display_formats_correctly() {
        assert_eq!(gbp(1500).to_string(), "15.00 GBP");
        assert_eq!(gbp(5).to_string(), "0.05 GBP");
        assert_eq!(gbp(-100).to_string(), "-1.00 GBP");
    }

    // --- parsing ------------------------------------------------------------

    #[test]
    fn parse_roundtrip() {
        let m = gbp(1250);
        let s = m.to_string();
        let parsed: MoneyAmount = s.parse().unwrap();
        assert_eq!(parsed, m);
    }

    #[test]
    fn parse_invalid_currency_errors() {
        assert!("10.00 XYZ".parse::<MoneyAmount>().is_err());
    }

    // --- division and banker's rounding ------------------------------------

    #[test]
    fn division_returns_quotient_and_remainder() {
        // 10 pence / 3 = 3 remainder 1
        let result = gbp(10).div_i64(3).unwrap();
        assert_eq!(result.quotient, gbp(3));
        assert_eq!(result.remainder_minor_units, 1);
    }

    #[test]
    fn bankers_rounding_rounds_half_to_even() {
        // 5 / 2 = 2.5 → round to even → 2 (since 2 is even)
        let result = gbp(5).div_i64(2).unwrap();
        assert_eq!(result.quotient.minor_units, 2);

        // 7 / 2 = 3.5 → round to even → 4 (since 4 is even)
        let result = gbp(7).div_i64(2).unwrap();
        assert_eq!(result.quotient.minor_units, 4);
    }

    #[test]
    fn division_by_zero_is_error() {
        assert_eq!(gbp(100).div_i64(0).unwrap_err(), MoneyError::DivisionByZero);
    }

    // --- neg ----------------------------------------------------------------

    #[test]
    fn checked_neg_works() {
        assert_eq!(gbp(100).checked_neg().unwrap(), gbp(-100));
    }

    #[test]
    fn checked_neg_min_overflows() {
        let min = MoneyAmount::new(i64::MIN, Currency::GBP);
        assert_eq!(min.checked_neg().unwrap_err(), MoneyError::Overflow);
    }

    // --- serde --------------------------------------------------------------

    #[test]
    fn serde_roundtrip() {
        let m = gbp(9999);
        let json = serde_json::to_string(&m).unwrap();
        let back: MoneyAmount = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    // --- the float guard ----------------------------------------------------

    /// The deprecated float path is exercised only here, and only to pin the
    /// behaviour that the deprecation exists to discourage.
    #[test]
    #[allow(deprecated)] // Justification: test-only, pinning the behaviour of the deprecated migration helper
    fn from_float_lossy_rounds_half_away_from_zero() {
        // 12.345 * 100 == 1234.5, and f64::round takes exact halves away from
        // zero, so this is 1235 — not the 1234 that banker's rounding gives.
        let m = MoneyAmount::from_float_lossy(12.345, Currency::GBP).unwrap();
        assert_eq!(m.minor_units, 1235);
    }

    #[test]
    #[allow(deprecated)] // Justification: test-only, pinning the behaviour of the deprecated migration helper
    fn from_float_lossy_rejects_nan() {
        assert!(MoneyAmount::from_float_lossy(f64::NAN, Currency::GBP).is_err());
        assert!(MoneyAmount::from_float_lossy(f64::INFINITY, Currency::GBP).is_err());
    }
}
