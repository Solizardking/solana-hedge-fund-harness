//! Deterministic pre-trade risk gate. Pure functions, deny-by-default:
//! any intent that violates a limit — or carries non-finite numbers — is
//! denied with a named reason. Mirrors sol_fund/risk/gate.py.

use serde::{Deserialize, Serialize};

use crate::venues::OrderIntent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimits {
    pub max_order_notional_usd: f64,
    pub max_leverage: f64,
    /// Stop for the day once realized+unrealized PnL reaches this loss (USD, positive number).
    pub daily_loss_stop_usd: f64,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_order_notional_usd: 25_000.0,
            max_leverage: 5.0,
            daily_loss_stop_usd: 500.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GateDecision {
    Allow,
    Deny(String),
}

impl GateDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, GateDecision::Allow)
    }
}

/// Evaluate one intent against the limits and today's PnL (USD, negative = loss).
pub fn evaluate(intent: &OrderIntent, day_pnl_usd: f64, limits: &RiskLimits) -> GateDecision {
    if !intent.size_usd.is_finite() || intent.size_usd <= 0.0 {
        return GateDecision::Deny(format!(
            "invalid size_usd: {}",
            intent.size_usd
        ));
    }
    if !intent.leverage.is_finite() || intent.leverage < 1.0 {
        return GateDecision::Deny(format!(
            "invalid leverage: {}",
            intent.leverage
        ));
    }
    if intent.size_usd > limits.max_order_notional_usd {
        return GateDecision::Deny(format!(
            "notional {} exceeds max order notional {}",
            intent.size_usd, limits.max_order_notional_usd
        ));
    }
    if intent.leverage > limits.max_leverage {
        return GateDecision::Deny(format!(
            "leverage {} exceeds max leverage {}",
            intent.leverage, limits.max_leverage
        ));
    }
    if day_pnl_usd <= -limits.daily_loss_stop_usd && !intent.reduce_only {
        return GateDecision::Deny(format!(
            "daily loss stop hit: day PnL {} <= -{}",
            day_pnl_usd, limits.daily_loss_stop_usd
        ));
    }
    GateDecision::Allow
}



/// Extra perps checks: leverage already gated above; also refuse opens that
/// leave less than `min_liq_distance_frac` to liquidation at the current mark.
pub fn evaluate_perps(
    intent: &OrderIntent,
    mark: f64,
    available_margin_usd: f64,
    min_liq_distance_frac: f64,
    limits: &RiskLimits,
) -> GateDecision {
    match evaluate(intent, 0.0, limits) {
        GateDecision::Deny(r) => return GateDecision::Deny(r),
        GateDecision::Allow => {}
    }
    let margin = intent.margin_usd();
    if !intent.reduce_only && margin > available_margin_usd {
        return GateDecision::Deny(format!(
            "required margin {margin} exceeds available {available_margin_usd}"
        ));
    }
    if mark > 0.0 && intent.leverage >= 1.0 && !intent.reduce_only {
        let liq = crate::perps::liq_price(intent.side, mark, intent.leverage);
        let dist = crate::perps::liq_distance_frac(intent.side, mark, liq);
        if dist < min_liq_distance_frac {
            return GateDecision::Deny(format!(
                "liq distance {dist} < min {min_liq_distance_frac} (liq={liq})"
            ));
        }
    }
    GateDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Side;
    use crate::venues::Venue;

    fn intent(size: f64, lev: f64) -> OrderIntent {
        OrderIntent::new("SOL", Side::Long, size, lev, Venue::Phoenix)
    }

    #[test]
    fn allows_intent_inside_limits() {
        let limits = RiskLimits::default();
        assert!(evaluate(&intent(10_000.0, 2.0), 0.0, &limits).is_allow());
    }

    #[test]
    fn denies_oversized_notional_at_edge() {
        let limits = RiskLimits::default();
        assert!(evaluate(&intent(25_000.0, 2.0), 0.0, &limits).is_allow());
        assert!(!evaluate(&intent(25_000.01, 2.0), 0.0, &limits).is_allow());
    }

    #[test]
    fn denies_excess_leverage() {
        let limits = RiskLimits::default();
        assert!(!evaluate(&intent(1_000.0, 5.01), 0.0, &limits).is_allow());
        assert!(!evaluate(&intent(1_000.0, 0.5), 0.0, &limits).is_allow());
    }

    #[test]
    fn daily_stop_blocks_opens_but_not_reduce_only() {
        let limits = RiskLimits::default();
        assert!(!evaluate(&intent(1_000.0, 2.0), -500.0, &limits).is_allow());
        let mut closing = intent(1_000.0, 2.0);
        closing.reduce_only = true;
        assert!(evaluate(&closing, -600.0, &limits).is_allow());
    }

    #[test]
    fn denies_non_finite_and_empty() {
        let limits = RiskLimits::default();
        assert!(!evaluate(&intent(f64::NAN, 2.0), 0.0, &limits).is_allow());
        assert!(!evaluate(&intent(0.0, 2.0), 0.0, &limits).is_allow());
    }
}
