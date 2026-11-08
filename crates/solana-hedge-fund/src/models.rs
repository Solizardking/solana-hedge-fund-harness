use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Long,
    Short,
}

impl Side {
    pub fn sign(&self) -> f64 {
        match self {
            Side::Long => 1.0,
            Side::Short => -1.0,
        }
    }
}

/// One market snapshot the desks evaluate. Fixtures implement this shape
/// offline; the `data` module fills it from live REST reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub symbol: String,
    pub spot_price: f64,
    pub perp_mark: f64,
    /// 24h spot return in percent (momentum input for the SpotDesk).
    pub return_24h_pct: f64,
    /// Annualized funding rates in percent, per venue (carry inputs).
    pub funding_apr_phoenix_pct: f64,
    pub funding_apr_backpack_pct: f64,
}

impl MarketSnapshot {
    /// Spot-perp basis in basis points: (perp - spot) / spot * 10_000.
    pub fn basis_bps(&self) -> f64 {
        if self.spot_price <= 0.0 {
            return 0.0;
        }
        (self.perp_mark - self.spot_price) / self.spot_price * 10_000.0
    }

    /// Funding differential (Phoenix minus Backpack) in APR percentage points.
    pub fn funding_differential_apr(&self) -> f64 {
        self.funding_apr_phoenix_pct - self.funding_apr_backpack_pct
    }
}
