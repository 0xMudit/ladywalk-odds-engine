//! # Risk
//!
//! The rules that decide whether a stake may be taken at all: how much a
//! customer may have at risk at once, and whether the platform is accepting
//! money movement.
//!
//! ## The kill switch
//!
//! Q35 asks how the whole platform stops taking money within seconds. The
//! answer here is that every path that takes a stake passes through
//! [`RiskChecker::check`], and the switch is a field on the checker rather than
//! a condition at each call site. A condition at a call site is a condition
//! that one of them will eventually forget; a field on the checker is a
//! condition that cannot be forgotten, because the only way to bypass it is to
//! not use the checker, and there is exactly one place that does that.
//!
//! ## Exposure
//!
//! Exposure is the total a customer stands to lose on open and matched orders.
//! It is checked against a limit before the stake is held, so a customer cannot
//! reach a limit by making several small orders in the same instant: the
//! check reads the already-held exposure, not the exposure this request would
//! have liked.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::money::MoneyAmount;

/// Why a stake was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RiskRefusal {
    /// The platform-wide kill switch is active.
    BettingHalted,
    /// The market is not accepting orders.
    MarketClosed,
    /// Taking this stake would exceed the customer's exposure limit.
    ExposureLimitExceeded {
        /// The limit that would have been breached, in minor units.
        limit_minor_units: i64,
        /// Exposure the customer already has, in minor units.
        current_minor_units: i64,
        /// Exposure this order would add, in minor units.
        requested_minor_units: i64,
    },
}

impl fmt::Display for RiskRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BettingHalted => f.write_str("betting is halted by the platform kill switch"),
            Self::MarketClosed => f.write_str("the market is not accepting orders"),
            Self::ExposureLimitExceeded {
                limit_minor_units,
                current_minor_units,
                requested_minor_units,
            } => write!(
                f,
                "exposure limit exceeded: limit {limit_minor_units}, current {current_minor_units}, requested {requested_minor_units}"
            ),
        }
    }
}

impl std::error::Error for RiskRefusal {}

impl From<RiskRefusal> for DomainError {
    fn from(refusal: RiskRefusal) -> Self {
        match refusal {
            RiskRefusal::BettingHalted => Self::BettingHalted,
            RiskRefusal::MarketClosed => Self::MarketClosed,
            RiskRefusal::ExposureLimitExceeded {
                limit_minor_units,
                current_minor_units,
                requested_minor_units,
            } => Self::ExposureLimitExceeded {
                limit_minor: limit_minor_units,
                current_minor: current_minor_units.saturating_add(requested_minor_units),
            },
        }
    }
}

/// The state of the platform-wide kill switch, as last read from the store.
///
/// Read from outside on every check rather than cached, because a switch whose
/// effect depends on a cache is a switch with a propagation delay, and the
/// number it has to be within is the number of seconds a regulator asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillSwitchState {
    /// Money may move.
    Running,
    /// Money movement is halted platform-wide.
    Halted,
}

/// A customer's risk configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomerRiskProfile {
    /// The most the customer may have at risk across all open positions, in
    /// minor units of the stake currency.
    pub exposure_limit_minor_units: i64,
}

/// Decides whether a stake may be taken.
#[derive(Debug, Clone, Copy)]
pub struct RiskChecker {
    /// Whether money movement is permitted at all.
    pub kill_switch: KillSwitchState,
    /// Whether the market the order is for is open.
    pub market_open: bool,
}

impl RiskChecker {
    /// A checker for a running platform and an open market.
    #[must_use]
    pub const fn running() -> Self {
        Self {
            kill_switch: KillSwitchState::Running,
            market_open: true,
        }
    }

    /// Whether money movement is permitted at all, ignoring any customer.
    #[must_use]
    pub const fn accepts_money_movement(self) -> bool {
        matches!(self.kill_switch, KillSwitchState::Running) && self.market_open
    }

    /// Check a proposed stake against the switch, the market, and the limit.
    ///
    /// # Errors
    ///
    /// Returns the specific [`RiskRefusal`] that stopped the stake. The
    /// refusals are distinguished rather than collapsed into one error because
    /// the customer's recovery differs: a halted platform means "try later", a
    /// closed market means "this market is over", and a breached limit means
    /// "reduce the stake".
    pub fn check(
        &self,
        profile: CustomerRiskProfile,
        current_exposure_minor_units: i64,
        stake: MoneyAmount,
    ) -> Result<(), RiskRefusal> {
        if self.kill_switch == KillSwitchState::Halted {
            return Err(RiskRefusal::BettingHalted);
        }
        if !self.market_open {
            return Err(RiskRefusal::MarketClosed);
        }

        let requested = stake.minor_units;
        if !stake.is_positive() {
            return Err(RiskRefusal::ExposureLimitExceeded {
                limit_minor_units: profile.exposure_limit_minor_units,
                current_minor_units: current_exposure_minor_units,
                requested_minor_units: requested,
            });
        }

        let after = current_exposure_minor_units.saturating_add(requested);
        if after > profile.exposure_limit_minor_units {
            return Err(RiskRefusal::ExposureLimitExceeded {
                limit_minor_units: profile.exposure_limit_minor_units,
                current_minor_units: current_exposure_minor_units,
                requested_minor_units: requested,
            });
        }

        Ok(())
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

    fn profile(limit: i64) -> CustomerRiskProfile {
        CustomerRiskProfile {
            exposure_limit_minor_units: limit,
        }
    }

    #[test]
    fn a_stake_under_the_limit_is_accepted() {
        let checker = RiskChecker::running();
        assert_eq!(checker.check(profile(10_000), 0, gbp(5_000)), Ok(()));
        assert_eq!(checker.check(profile(10_000), 5_000, gbp(5_000)), Ok(()));
    }

    #[test]
    fn a_stake_that_reaches_the_limit_exactly_is_accepted() {
        let checker = RiskChecker::running();
        assert_eq!(checker.check(profile(10_000), 5_000, gbp(5_000)), Ok(()));
    }

    #[test]
    fn a_stake_one_minor_unit_over_the_limit_is_refused() {
        let checker = RiskChecker::running();
        assert_eq!(
            checker.check(profile(10_000), 5_000, gbp(5_001)),
            Err(RiskRefusal::ExposureLimitExceeded {
                limit_minor_units: 10_000,
                current_minor_units: 5_000,
                requested_minor_units: 5_001,
            })
        );
    }

    #[test]
    fn the_kill_switch_stops_everything_regardless_of_the_limit() {
        let checker = RiskChecker {
            kill_switch: KillSwitchState::Halted,
            market_open: true,
        };
        assert_eq!(checker.check(profile(i64::MAX), 0, gbp(1)), Err(RiskRefusal::BettingHalted));
        assert!(!checker.accepts_money_movement());
    }

    #[test]
    fn the_kill_switch_wins_over_a_closed_market() {
        // The order of the checks is itself a decision: "the platform is
        // halted" is the more urgent fact to report.
        let checker = RiskChecker {
            kill_switch: KillSwitchState::Halted,
            market_open: false,
        };
        assert_eq!(checker.check(profile(i64::MAX), 0, gbp(1)), Err(RiskRefusal::BettingHalted));
    }

    #[test]
    fn a_closed_market_stops_orders() {
        let checker = RiskChecker {
            kill_switch: KillSwitchState::Running,
            market_open: false,
        };
        assert_eq!(
            checker.check(profile(i64::MAX), 0, gbp(1)),
            Err(RiskRefusal::MarketClosed)
        );
        assert!(!checker.accepts_money_movement());
    }

    #[test]
    fn a_non_positive_stake_is_refused() {
        let checker = RiskChecker::running();
        assert!(checker.check(profile(10_000), 0, gbp(0)).is_err());
        assert!(checker.check(profile(10_000), 0, gbp(-1)).is_err());
    }

    #[test]
    fn each_refusal_maps_to_its_own_domain_error() {
        assert!(matches!(
            DomainError::from(RiskRefusal::BettingHalted),
            DomainError::BettingHalted
        ));
        assert!(matches!(
            DomainError::from(RiskRefusal::MarketClosed),
            DomainError::MarketClosed
        ));
        let mapped = DomainError::from(RiskRefusal::ExposureLimitExceeded {
            limit_minor_units: 1_000,
            current_minor_units: 800,
            requested_minor_units: 500,
        });
        assert!(matches!(
            mapped,
            DomainError::ExposureLimitExceeded {
                limit_minor: 1_000,
                current_minor: 1_300
            }
        ));
    }

    #[test]
    fn a_breached_limit_reports_the_post_order_exposure() {
        // A customer seeing "you would be at 1300 with a limit of 1000" can act
        // on it; one seeing "current 800" cannot, because the number that
        // matters is the one they would reach.
        let checker = RiskChecker::running();
        let refusal = checker
            .check(profile(1_000), 800, gbp(500))
            .expect_err("should be refused");
        let DomainError::ExposureLimitExceeded { current_minor, .. } =
            DomainError::from(refusal)
        else {
            panic!("expected an exposure limit error");
        };
        assert_eq!(current_minor, 1_300);
    }

    #[test]
    fn exposure_saturates_rather_than_overflowing() {
        let checker = RiskChecker::running();
        let refusal = checker
            .check(profile(i64::MAX), i64::MAX - 1, gbp(1_000))
            .expect_err("should be refused");
        assert!(matches!(refusal, RiskRefusal::ExposureLimitExceeded { .. }));
    }
}
