//! Venues emit UNSIGNED order intents only. There is deliberately no code
//! here that builds, signs, or submits a transaction — mirroring sol_fund,
//! where venues build unsigned transactions and signing stays with the
//! owner's wallet under a scoped policy. Live execution is refused by design.

use serde::{Deserialize, Serialize};

use crate::models::Side;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Venue {
    Phoenix,
    Jupiter,
    Dflow,
    Backpack,
}

/// An order the fund would like to place — a pure data intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderIntent {
    pub symbol: String,
    pub side: Side,
    /// Position size in USD notional.
    pub size_usd: f64,
    pub leverage: f64,
    pub venue: Venue,
    pub reduce_only: bool,
}

impl OrderIntent {
    pub fn new(symbol: &str, side: Side, size_usd: f64, leverage: f64, venue: Venue) -> Self {
        Self {
            symbol: symbol.to_string(),
            side,
            size_usd,
            leverage,
            venue,
            reduce_only: false,
        }
    }

    /// Collateral (margin) the intent requires at its leverage.
    pub fn margin_usd(&self) -> f64 {
        if self.leverage <= 0.0 {
            return self.size_usd;
        }
        self.size_usd / self.leverage
    }
}
