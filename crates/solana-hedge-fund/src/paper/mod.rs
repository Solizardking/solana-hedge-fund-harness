//! Paper fill engine: simulated fills at provided marks with a fee +
//! slippage cost model (the spirit of sol_fund's upstream cost model).
//! No chain, no wallet, no signing — cash and positions live in memory.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::venues::OrderIntent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostModel {
    pub fee_bps: f64,
    pub slippage_bps: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        // 5 bps taker fee + 2 bps slippage, in line with the Python fund's model.
        Self {
            fee_bps: 5.0,
            slippage_bps: 2.0,
        }
    }
}

impl CostModel {
    /// Total cost in USD for a fill of `notional_usd` at `mark`.
    pub fn cost_usd(&self, notional_usd: f64) -> f64 {
        notional_usd * (self.fee_bps + self.slippage_bps) / 10_000.0
    }

    /// Effective fill price after adverse slippage for the given side sign (+1 long, -1 short).
    pub fn fill_price(&self, mark: f64, side_sign: f64) -> f64 {
        mark * (1.0 + side_sign * self.slippage_bps / 10_000.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    /// Signed base quantity: positive long, negative short.
    pub qty: f64,
    pub avg_entry: f64,
}

impl Position {
    pub fn unrealized_pnl(&self, mark: f64) -> f64 {
        self.qty * (mark - self.avg_entry)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub symbol: String,
    pub fill_price: f64,
    pub qty: f64,
    pub cost_usd: f64,
    pub realized_pnl_delta: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperBook {
    pub cash: f64,
    pub realized_pnl: f64,
    pub positions: HashMap<String, Position>,
    pub costs: CostModel,
}

impl PaperBook {
    pub fn new(starting_cash: f64) -> Self {
        Self {
            cash: starting_cash,
            realized_pnl: 0.0,
            positions: HashMap::new(),
            costs: CostModel::default(),
        }
    }

    /// Fill an intent at `mark`. Returns the fill, or an error string when the
    /// intent is malformed. Callers must run the risk gate first — the book
    /// itself does not gate.
    pub fn fill(&mut self, intent: &OrderIntent, mark: f64) -> Result<Fill, String> {
        if mark <= 0.0 || !mark.is_finite() {
            return Err(format!("invalid mark: {mark}"));
        }
        let side_sign = intent.side.sign();
        let price = self.costs.fill_price(mark, side_sign);
        let qty = side_sign * (intent.size_usd / price);
        let cost = self.costs.cost_usd(intent.size_usd);
        self.cash -= cost;

        let mut realized_delta = 0.0;
        let entry = self
            .positions
            .entry(intent.symbol.clone())
            .or_insert(Position {
                symbol: intent.symbol.clone(),
                qty: 0.0,
                avg_entry: price,
            });
        if entry.qty == 0.0 {
            entry.avg_entry = price;
            entry.qty = qty;
        } else if entry.qty.signum() == qty.signum() {
            // Adding: blend the average entry.
            let total = entry.qty + qty;
            entry.avg_entry = (entry.avg_entry * entry.qty + price * qty) / total;
            entry.qty = total;
        } else {
            // Reducing or flipping: realize PnL on the closed portion.
            let closed = qty.abs().min(entry.qty.abs());
            realized_delta = closed * entry.qty.signum() * (price - entry.avg_entry);
            self.realized_pnl += realized_delta;
            entry.qty += qty;
            if entry.qty == 0.0 {
                entry.avg_entry = price;
            } else if entry.qty.signum() == qty.signum() {
                // Flipped through zero: remainder opens at the fill price.
                entry.avg_entry = price;
            }
        }
        let fill = Fill {
            symbol: intent.symbol.clone(),
            fill_price: price,
            qty,
            cost_usd: cost,
            realized_pnl_delta: realized_delta,
        };
        if self
            .positions
            .get(&intent.symbol)
            .map(|p| p.qty == 0.0)
            .unwrap_or(false)
        {
            self.positions.remove(&intent.symbol);
        }
        Ok(fill)
    }

    pub fn unrealized_pnl(&self, marks: &HashMap<String, f64>) -> f64 {
        self.positions
            .values()
            .map(|p| p.unrealized_pnl(*marks.get(&p.symbol).unwrap_or(&p.avg_entry)))
            .sum()
    }

    pub fn equity(&self, marks: &HashMap<String, f64>) -> f64 {
        self.cash + self.realized_pnl + self.unrealized_pnl(marks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Side;
    use crate::venues::Venue;

    fn marks(sol: f64) -> HashMap<String, f64> {
        HashMap::from([("SOL".to_string(), sol)])
    }

    #[test]
    fn long_fill_pnl_and_costs() {
        let mut book = PaperBook::new(100_000.0);
        let intent = OrderIntent::new("SOL", Side::Long, 10_000.0, 2.0, Venue::Phoenix);
        let fill = book.fill(&intent, 100.0).unwrap();
        // Slippage pushes a long's fill above the mark.
        assert!(fill.fill_price > 100.0);
        assert_eq!(fill.cost_usd, 10_000.0 * 7.0 / 10_000.0);
        // Mark at the fill price: flat. Mark +10%: roughly +$1,000 unrealized.
        let pnl = book.unrealized_pnl(&marks(fill.fill_price * 1.10));
        assert!((pnl - 1_000.0).abs() < 1.0, "pnl={pnl}");
        let equity = book.equity(&marks(fill.fill_price * 1.10));
        assert!((equity - (100_000.0 - fill.cost_usd + 1_000.0)).abs() < 1.0);
    }

    #[test]
    fn short_profits_when_mark_falls() {
        let mut book = PaperBook::new(10_000.0);
        let intent = OrderIntent::new("SOL", Side::Short, 5_000.0, 2.0, Venue::Phoenix);
        let fill = book.fill(&intent, 200.0).unwrap();
        assert!(fill.fill_price < 200.0);
        assert!(book.unrealized_pnl(&marks(180.0)) > 400.0);
    }

    #[test]
    fn closing_fill_realizes_pnl() {
        let mut book = PaperBook::new(100_000.0);
        let open = OrderIntent::new("SOL", Side::Long, 10_000.0, 1.0, Venue::Jupiter);
        let f1 = book.fill(&open, 100.0).unwrap();
        let mut close = OrderIntent::new("SOL", Side::Short, 10_000.0, 1.0, Venue::Jupiter);
        close.reduce_only = true;
        // Resize the close to the actual open quantity at the close's own
        // slippage-adjusted fill price, so the position closes exactly.
        close.size_usd = f1.qty.abs() * book.costs.fill_price(110.0, -1.0);
        let f2 = book.fill(&close, 110.0).unwrap();
        assert!(f2.realized_pnl_delta > 900.0, "realized={}", f2.realized_pnl_delta);
        assert!(book.positions.is_empty());
        assert!(book.realized_pnl > 900.0);
    }

    #[test]
    fn rejects_bad_mark() {
        let mut book = PaperBook::new(1_000.0);
        let intent = OrderIntent::new("SOL", Side::Long, 100.0, 1.0, Venue::Phoenix);
        assert!(book.fill(&intent, 0.0).is_err());
        assert!(book.fill(&intent, f64::NAN).is_err());
    }
}
