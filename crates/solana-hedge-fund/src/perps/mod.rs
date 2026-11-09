//! Paper perps engine: open / close / reduce positions with leverage, margin,
//! liquidation price, funding accrual, and marked PnL. Unsigned intents only —
//! no signing, no keypairs, no chain submission.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::models::Side;
use crate::venues::OrderIntent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerpsPosition {
    pub symbol: String,
    pub side: Side,
    /// Absolute base quantity.
    pub qty: f64,
    pub entry_price: f64,
    pub leverage: f64,
    /// Isolated margin locked for this position.
    pub margin_usd: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub funding_paid_usd: f64,
    pub liquidation_price: f64,
}

impl PerpsPosition {
    pub fn notional_usd(&self, mark: f64) -> f64 {
        self.qty.abs() * mark
    }

    pub fn mark_to(&mut self, mark: f64) {
        let sign = self.side.sign();
        self.unrealized_pnl = sign * self.qty.abs() * (mark - self.entry_price);
        self.liquidation_price = liq_price(self.side, self.entry_price, self.leverage);
    }
}

/// Isolated-margin approximate liquidation: long -> entry * (1 - 1/lev),
/// short -> entry * (1 + 1/lev). Maintenance buffer ignored for paper.
pub fn liq_price(side: Side, entry: f64, leverage: f64) -> f64 {
    if entry <= 0.0 || leverage < 1.0 {
        return 0.0;
    }
    match side {
        Side::Long => entry * (1.0 - 1.0 / leverage).max(0.0),
        Side::Short => entry * (1.0 + 1.0 / leverage),
    }
}

/// Distance from mark to liquidation as a fraction of mark (0..). Higher = safer.
pub fn liq_distance_frac(side: Side, mark: f64, liq: f64) -> f64 {
    if mark <= 0.0 {
        return 0.0;
    }
    match side {
        Side::Long => ((mark - liq) / mark).max(0.0),
        Side::Short => ((liq - mark) / mark).max(0.0),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerpsFill {
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    pub fill_price: f64,
    pub leverage: f64,
    pub margin_usd: f64,
    pub fee_usd: f64,
    pub realized_pnl_delta: f64,
    pub reduce_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerpsBook {
    pub cash: f64,
    pub positions: HashMap<String, PerpsPosition>,
    pub fee_bps: f64,
    pub funding_interval_hours: f64,
}

impl PerpsBook {
    pub fn new(starting_cash: f64) -> Self {
        Self {
            cash: starting_cash,
            positions: HashMap::new(),
            fee_bps: 5.0,
            funding_interval_hours: 1.0,
        }
    }

    pub fn equity(&self) -> f64 {
        // Cash already had margin deducted on open; add margin back so equity
        // reflects locked collateral + uPnL.
        self.cash
            + self
                .positions
                .values()
                .map(|p| p.margin_usd + p.unrealized_pnl)
                .sum::<f64>()
    }

    pub fn positions_vec(&self) -> Vec<PerpsPosition> {
        self.positions.values().cloned().collect()
    }

    /// Apply hourly funding. Positive rate = longs pay shorts.
    /// `funding_rate_per_hour` is a fraction (e.g. 0.0001 = 0.01%/hr).
    pub fn accrue_funding(&mut self, symbol: &str, mark: f64, funding_rate_per_hour: f64) {
        if let Some(pos) = self.positions.get_mut(symbol) {
            let notional = pos.qty.abs() * mark;
            let payment = notional * funding_rate_per_hour * self.funding_interval_hours;
            // Longs pay when rate > 0; shorts receive.
            let signed = pos.side.sign() * payment;
            pos.funding_paid_usd += signed;
            pos.realized_pnl -= signed;
            self.cash -= signed;
            pos.mark_to(mark);
        }
    }

    /// Open / add / reduce / close from an unsigned intent at `mark`.
    pub fn apply_intent(&mut self, intent: &OrderIntent, mark: f64) -> Result<PerpsFill, String> {
        if mark <= 0.0 || !mark.is_finite() {
            return Err(format!("invalid mark: {mark}"));
        }
        if intent.leverage < 1.0 || !intent.leverage.is_finite() {
            return Err(format!("invalid leverage: {}", intent.leverage));
        }
        let fee = intent.size_usd * self.fee_bps / 10_000.0;
        let qty = intent.size_usd / mark;
        let margin = intent.margin_usd();

        if let Some(existing) = self.positions.get(&intent.symbol).cloned() {
            let same_side = existing.side == intent.side;
            if intent.reduce_only || !same_side {
                return self.reduce_or_flip(&intent.symbol, intent.side, qty, mark, fee, intent.reduce_only);
            }
            // Add to position.
            let pos = self.positions.get_mut(&intent.symbol).unwrap();
            let total_qty = pos.qty + qty;
            pos.entry_price = (pos.entry_price * pos.qty + mark * qty) / total_qty;
            pos.qty = total_qty;
            pos.leverage = intent.leverage;
            pos.margin_usd += margin;
            self.cash -= margin + fee;
            pos.mark_to(mark);
            return Ok(PerpsFill {
                symbol: intent.symbol.clone(),
                side: intent.side,
                qty,
                fill_price: mark,
                leverage: intent.leverage,
                margin_usd: margin,
                fee_usd: fee,
                realized_pnl_delta: 0.0,
                reduce_only: false,
            });
        }

        if intent.reduce_only {
            return Err("no position to reduce".into());
        }
        if self.cash < margin + fee {
            return Err(format!(
                "insufficient cash: need {:.2}, have {:.2}",
                margin + fee,
                self.cash
            ));
        }
        self.cash -= margin + fee;
        let mut pos = PerpsPosition {
            symbol: intent.symbol.clone(),
            side: intent.side,
            qty,
            entry_price: mark,
            leverage: intent.leverage,
            margin_usd: margin,
            unrealized_pnl: 0.0,
            realized_pnl: 0.0,
            funding_paid_usd: 0.0,
            liquidation_price: liq_price(intent.side, mark, intent.leverage),
        };
        pos.mark_to(mark);
        self.positions.insert(intent.symbol.clone(), pos);
        Ok(PerpsFill {
            symbol: intent.symbol.clone(),
            side: intent.side,
            qty,
            fill_price: mark,
            leverage: intent.leverage,
            margin_usd: margin,
            fee_usd: fee,
            realized_pnl_delta: 0.0,
            reduce_only: false,
        })
    }

    fn reduce_or_flip(
        &mut self,
        symbol: &str,
        close_side: Side,
        qty: f64,
        mark: f64,
        fee: f64,
        reduce_only: bool,
    ) -> Result<PerpsFill, String> {
        let pos = self
            .positions
            .get(symbol)
            .cloned()
            .ok_or_else(|| "no position".to_string())?;
        // Closing side should oppose the open side.
        if pos.side == close_side && reduce_only {
            return Err("reduce_only close side matches open side".into());
        }
        let closing_side = if pos.side == close_side {
            // Flip path when not reduce_only: treat as reducing opposite first.
            match pos.side {
                Side::Long => Side::Short,
                Side::Short => Side::Long,
            }
        } else {
            close_side
        };
        let _ = closing_side;
        let close_qty = qty.min(pos.qty);
        let sign = pos.side.sign();
        let realized = sign * close_qty * (mark - pos.entry_price);
        let margin_release = pos.margin_usd * (close_qty / pos.qty);
        self.cash += margin_release + realized - fee;

        let remaining = pos.qty - close_qty;
        if remaining <= 1e-12 {
            self.positions.remove(symbol);
        } else {
            let p = self.positions.get_mut(symbol).unwrap();
            p.qty = remaining;
            p.margin_usd -= margin_release;
            p.realized_pnl += realized;
            p.mark_to(mark);
        }

        // If intent qty exceeded position and not reduce_only, open remainder opposite.
        let leftover = qty - close_qty;
        if leftover > 1e-12 && !reduce_only {
            let open_side = close_side;
            let open_notional = leftover * mark;
            let lev = pos.leverage.max(1.0);
            let margin = open_notional / lev;
            if self.cash < margin {
                return Err("insufficient cash to flip remainder".into());
            }
            self.cash -= margin;
            let mut np = PerpsPosition {
                symbol: symbol.to_string(),
                side: open_side,
                qty: leftover,
                entry_price: mark,
                leverage: lev,
                margin_usd: margin,
                unrealized_pnl: 0.0,
                realized_pnl: 0.0,
                funding_paid_usd: 0.0,
                liquidation_price: liq_price(open_side, mark, lev),
            };
            np.mark_to(mark);
            self.positions.insert(symbol.to_string(), np);
        }

        Ok(PerpsFill {
            symbol: symbol.to_string(),
            side: close_side,
            qty: close_qty,
            fill_price: mark,
            leverage: pos.leverage,
            margin_usd: margin_release,
            fee_usd: fee,
            realized_pnl_delta: realized,
            reduce_only,
        })
    }

    pub fn mark_all(&mut self, marks: &HashMap<String, f64>) {
        for (sym, pos) in self.positions.iter_mut() {
            if let Some(m) = marks.get(sym) {
                pos.mark_to(*m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::Venue;

    #[test]
    fn open_long_and_mark() {
        let mut book = PerpsBook::new(10_000.0);
        let intent = OrderIntent::new("SOL", Side::Long, 1_000.0, 5.0, Venue::Phoenix);
        let fill = book.apply_intent(&intent, 100.0).unwrap();
        assert!((fill.qty - 10.0).abs() < 1e-9);
        assert!((fill.margin_usd - 200.0).abs() < 1e-9);
        let pos = book.positions.get("SOL").unwrap();
        assert!((pos.liquidation_price - 80.0).abs() < 1e-9);
        book.mark_all(&HashMap::from([("SOL".into(), 110.0)]));
        let pos = book.positions.get("SOL").unwrap();
        assert!((pos.unrealized_pnl - 100.0).abs() < 1e-6);
    }

    #[test]
    fn funding_longs_pay() {
        let mut book = PerpsBook::new(10_000.0);
        let intent = OrderIntent::new("SOL", Side::Long, 1_000.0, 2.0, Venue::Imperial);
        book.apply_intent(&intent, 100.0).unwrap();
        let cash_before = book.cash;
        book.accrue_funding("SOL", 100.0, 0.001); // 0.1%/hr
        assert!(book.cash < cash_before);
    }

    #[test]
    fn close_realizes_pnl() {
        let mut book = PerpsBook::new(10_000.0);
        let open = book
            .apply_intent(
                &OrderIntent::new("SOL", Side::Long, 1_000.0, 2.0, Venue::Phoenix),
                100.0,
            )
            .unwrap();
        // Size the reduce to the open qty at the new mark so the book closes flat.
        let mut close = OrderIntent::new(
            "SOL",
            Side::Short,
            open.qty * 110.0,
            2.0,
            Venue::Phoenix,
        );
        close.reduce_only = true;
        let fill = book.apply_intent(&close, 110.0).unwrap();
        assert!(fill.realized_pnl_delta > 90.0);
        assert!(book.positions.is_empty());
    }

    #[test]
    fn liq_distance_long() {
        let d = liq_distance_frac(Side::Long, 100.0, 80.0);
        assert!((d - 0.2).abs() < 1e-9);
    }
}
