//! Pure signal functions shared by the desks (mirrors sol_fund/signals).

use crate::models::MarketSnapshot;

/// Clamp a raw score into [-1, 1].
pub fn clamp_score(x: f64) -> f64 {
    x.clamp(-1.0, 1.0)
}

/// Spot momentum score from the 24h return: ±8% maps to ∓/±1.
pub fn momentum_score(return_24h_pct: f64) -> f64 {
    clamp_score(return_24h_pct / 8.0)
}

/// Funding-carry score for Phoenix: rich positive funding means longs pay
/// shorts, so the carry trade is SHORT (negative score = short bias).
/// 100% APR maps to full conviction.
pub fn funding_carry_score(funding_apr_pct: f64) -> f64 {
    clamp_score(-funding_apr_pct / 100.0)
}

/// Arb score combining spot-perp basis and the cross-venue funding
/// differential. Basis is paid by perp longs when positive (score shorts the
/// perp); a funding differential pays whichever venue is cheaper to be long.
/// Both inputs are squashed so the arb desk speaks the same [-1, 1] language.
pub fn arb_score(snapshot: &MarketSnapshot) -> f64 {
    let basis_component = clamp_score(-snapshot.basis_bps() / 50.0);
    let funding_component = clamp_score(-snapshot.funding_differential_apr() / 100.0);
    clamp_score(0.6 * basis_component + 0.4 * funding_component)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(spot: f64, perp: f64, ret: f64, phx_funding: f64, bp_funding: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: "SOL".into(),
            spot_price: spot,
            perp_mark: perp,
            return_24h_pct: ret,
            funding_apr_phoenix_pct: phx_funding,
            funding_apr_backpack_pct: bp_funding,
        }
    }

    #[test]
    fn basis_bps_math() {
        let s = snap(100.0, 100.25, 0.0, 0.0, 0.0);
        assert!((s.basis_bps() - 25.0).abs() < 1e-9);
        let s2 = snap(100.0, 99.5, 0.0, 0.0, 0.0);
        assert!((s2.basis_bps() + 50.0).abs() < 1e-9);
    }

    #[test]
    fn funding_differential_math() {
        let s = snap(100.0, 100.0, 0.0, 432.5, 10.0);
        assert!((s.funding_differential_apr() - 422.5).abs() < 1e-9);
    }

    #[test]
    fn momentum_and_carry_signs() {
        assert!(momentum_score(8.0) > 0.99);
        assert!(momentum_score(-4.0) < -0.49);
        // Rich funding -> carry score is negative (short bias).
        assert!(funding_carry_score(100.0) < -0.99);
        assert!(funding_carry_score(-50.0) > 0.49);
    }

    #[test]
    fn arb_score_prefers_short_when_basis_and_funding_rich() {
        // Perp 1% rich to spot and Phoenix funding far above Backpack.
        let s = snap(100.0, 101.0, 0.0, 120.0, 5.0);
        assert!(arb_score(&s) < -0.9, "score={}", arb_score(&s));
        // Flat market: no arb.
        let flat = snap(100.0, 100.0, 0.0, 10.0, 10.0);
        assert!(arb_score(&flat).abs() < 1e-9);
    }
}
