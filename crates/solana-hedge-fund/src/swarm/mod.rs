//! The fund swarm: SpotDesk, PerpsDesk and ArbDesk evaluate a snapshot and
//! emit typed votes; the RiskAgent can veto; the PortfolioManager takes a
//! weighted consensus and records dissent. Mirrors sol_fund/swarm.
//!
//! Why a thin local orchestration layer instead of swarms-rs agents:
//! swarms-rs' `Agent` trait models an LLM agent loop — `run(task) -> String`
//! with planning and long-term-memory hooks. Our desks are DETERMINISTIC
//! evaluators that must emit typed scores and unsigned intents; dressing
//! them as free-text LLM loops would fabricate the one thing the fund
//! doctrine forbids (narrative dressed as signal). So the desks live behind
//! the local `Desk` trait, and the fund as a whole implements the upstream
//! `swarms_rs::structs::swarm::Swarm` trait, which IS the right shape:
//! one task in, one serializable consensus out. The Grok narrative layer
//! (crate::llm) stays optional and advisory, exactly as in sol_fund.

use erased_serde::Serialize as ErasedSerialize;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use swarms_rs::structs::swarm::{Swarm, SwarmError};

use crate::models::{MarketSnapshot, Side};
use crate::risk::{self, GateDecision, RiskLimits};
use crate::signals;
use crate::venues::{OrderIntent, Venue};

/// One desk's typed opinion. `score` in [-1, 1]: positive = long bias.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub desk: String,
    pub symbol: String,
    pub score: f64,
    pub confidence: f64,
    pub rationale: String,
    pub intent: Option<OrderIntent>,
}

pub trait Desk: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate(&self, snapshot: &MarketSnapshot) -> Vote;
}

fn intent_for(symbol: &str, score: f64, size_usd: f64, venue: Venue) -> Option<OrderIntent> {
    if score.abs() < 0.05 {
        return None;
    }
    let side = if score > 0.0 { Side::Long } else { Side::Short };
    Some(OrderIntent::new(symbol, side, size_usd, 2.0, venue))
}

pub struct SpotDesk {
    pub size_usd: f64,
}
impl Desk for SpotDesk {
    fn name(&self) -> &str {
        "SpotDesk"
    }
    fn evaluate(&self, s: &MarketSnapshot) -> Vote {
        let score = signals::momentum_score(s.return_24h_pct);
        Vote {
            desk: self.name().into(),
            symbol: s.symbol.clone(),
            score,
            confidence: 0.6,
            rationale: format!("24h return {:.2}% -> momentum {:.2}", s.return_24h_pct, score),
            intent: intent_for(&s.symbol, score, self.size_usd, Venue::Jupiter),
        }
    }
}

pub struct PerpsDesk {
    pub size_usd: f64,
}
impl Desk for PerpsDesk {
    fn name(&self) -> &str {
        "PerpsDesk"
    }
    fn evaluate(&self, s: &MarketSnapshot) -> Vote {
        let score = signals::funding_carry_score(s.funding_apr_phoenix_pct);
        Vote {
            desk: self.name().into(),
            symbol: s.symbol.clone(),
            score,
            confidence: 0.7,
            rationale: format!(
                "Phoenix funding {:.1}% APR -> carry score {:.2}",
                s.funding_apr_phoenix_pct, score
            ),
            intent: intent_for(&s.symbol, score, self.size_usd, Venue::Phoenix),
        }
    }
}

pub struct ArbDesk {
    pub size_usd: f64,
}
impl Desk for ArbDesk {
    fn name(&self) -> &str {
        "ArbDesk"
    }
    fn evaluate(&self, s: &MarketSnapshot) -> Vote {
        let score = signals::arb_score(s);
        Vote {
            desk: self.name().into(),
            symbol: s.symbol.clone(),
            score,
            confidence: 0.8,
            rationale: format!(
                "basis {:.1} bps, funding diff {:.1} APR pts -> arb score {:.2}",
                s.basis_bps(),
                s.funding_differential_apr(),
                score
            ),
            intent: intent_for(&s.symbol, score, self.size_usd, Venue::Phoenix),
        }
    }
}

/// The RiskAgent re-runs the deterministic gate over a proposed intent.
/// Its veto overrides any consensus (see PortfolioManager).
pub struct RiskAgent {
    pub limits: RiskLimits,
}
impl RiskAgent {
    pub fn check(&self, intent: &OrderIntent, day_pnl_usd: f64) -> GateDecision {
        risk::evaluate(intent, day_pnl_usd, &self.limits)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsensusAction {
    Long,
    Short,
    Hold,
    Vetoed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Consensus {
    pub symbol: String,
    pub net_score: f64,
    pub action: ConsensusAction,
    pub votes: Vec<Vote>,
    /// Names + scores of desks that disagreed with the final direction.
    pub dissent: Vec<String>,
    pub veto_reason: Option<String>,
    pub approved_intent: Option<OrderIntent>,
}

pub struct PortfolioManager {
    /// (desk name, weight) — weights need not sum to 1; they are normalized.
    pub weights: Vec<(String, f64)>,
}

impl Default for PortfolioManager {
    fn default() -> Self {
        Self {
            weights: vec![
                ("SpotDesk".into(), 1.0),
                ("PerpsDesk".into(), 1.0),
                ("ArbDesk".into(), 1.5),
            ],
        }
    }
}

impl PortfolioManager {
    pub fn consensus(
        &self,
        votes: &[Vote],
        risk: &RiskAgent,
        day_pnl_usd: f64,
    ) -> Consensus {
        let symbol = votes
            .first()
            .map(|v| v.symbol.clone())
            .unwrap_or_default();
        let mut weighted = 0.0;
        let mut weight_sum = 0.0;
        for v in votes {
            let w = self
                .weights
                .iter()
                .find(|(n, _)| n == &v.desk)
                .map(|(_, w)| *w)
                .unwrap_or(1.0);
            weighted += w * v.score * v.confidence;
            weight_sum += w * v.confidence;
        }
        let net = if weight_sum > 0.0 {
            signals::clamp_score(weighted / weight_sum)
        } else {
            0.0
        };
        let direction = if net > 0.1 {
            1.0
        } else if net < -0.1 {
            -1.0
        } else {
            0.0
        };
        let dissent: Vec<String> = votes
            .iter()
            .filter(|v| direction != 0.0 && v.score * direction < -0.05)
            .map(|v| format!("{} dissented with score {:.2}", v.desk, v.score))
            .collect();

        // The intent of the highest-conviction desk aligned with consensus.
        let candidate = votes
            .iter()
            .filter(|v| direction != 0.0 && v.score * direction > 0.0)
            .max_by(|a, b| {
                (a.score.abs() * a.confidence)
                    .partial_cmp(&(b.score.abs() * b.confidence))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|v| v.intent.clone());

        let (action, approved_intent, veto_reason) = match (direction, candidate) {
            (0.0, _) => (ConsensusAction::Hold, None, None),
            (_, None) => (ConsensusAction::Hold, None, None),
            (_, Some(intent)) => match risk.check(&intent, day_pnl_usd) {
                GateDecision::Allow => {
                    let action = if direction > 0.0 {
                        ConsensusAction::Long
                    } else {
                        ConsensusAction::Short
                    };
                    (action, Some(intent), None)
                }
                GateDecision::Deny(reason) => (ConsensusAction::Vetoed, None, Some(reason)),
            },
        };

        Consensus {
            symbol,
            net_score: net,
            action,
            votes: votes.to_vec(),
            dissent,
            veto_reason,
            approved_intent,
        }
    }
}

/// One full offline cycle: desks vote, manager decides, risk can veto.
pub fn run_cycle(
    snapshot: &MarketSnapshot,
    desks: &[Box<dyn Desk>],
    pm: &PortfolioManager,
    risk: &RiskAgent,
    day_pnl_usd: f64,
) -> Consensus {
    let votes: Vec<Vote> = desks.iter().map(|d| d.evaluate(snapshot)).collect();
    pm.consensus(&votes, risk, day_pnl_usd)
}

pub fn default_desks(size_usd: f64) -> Vec<Box<dyn Desk>> {
    vec![
        Box::new(SpotDesk { size_usd }),
        Box::new(PerpsDesk { size_usd }),
        Box::new(ArbDesk { size_usd }),
    ]
}

/// The fund as an upstream swarms-rs `Swarm`: one task in, the cycle's
/// consensus (a serializable value) out.
pub struct HedgeFundSwarm {
    pub snapshot: MarketSnapshot,
    pub size_usd: f64,
    pub limits: RiskLimits,
    pub day_pnl_usd: f64,
}

impl Swarm for HedgeFundSwarm {
    fn name(&self) -> &str {
        "solana-hedge-fund"
    }

    fn run(&self, _task: String) -> BoxFuture<'_, Result<Box<dyn ErasedSerialize>, SwarmError>> {
        Box::pin(async move {
            let desks = default_desks(self.size_usd);
            let pm = PortfolioManager::default();
            let risk = RiskAgent {
                limits: self.limits.clone(),
            };
            let consensus = run_cycle(&self.snapshot, &desks, &pm, &risk, self.day_pnl_usd);
            let boxed: Box<dyn ErasedSerialize> = Box::new(consensus);
            Ok(boxed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(ret: f64, perp: f64, phx_funding: f64, bp_funding: f64) -> MarketSnapshot {
        MarketSnapshot {
            symbol: "SOL".into(),
            spot_price: 100.0,
            perp_mark: perp,
            return_24h_pct: ret,
            funding_apr_phoenix_pct: phx_funding,
            funding_apr_backpack_pct: bp_funding,
        }
    }

    #[test]
    fn neutral_tape_holds() {
        let desks = default_desks(5_000.0);
        let pm = PortfolioManager::default();
        let risk = RiskAgent {
            limits: RiskLimits::default(),
        };
        let c = run_cycle(&snap(0.1, 100.0, 2.0, 2.0), &desks, &pm, &risk, 0.0);
        assert!(matches!(c.action, ConsensusAction::Hold), "{:?}", c.action);
        assert!(c.approved_intent.is_none());
    }

    #[test]
    fn aligned_desks_reach_long_consensus_with_dissent_recorded() {
        // Momentum strongly up, funding negative (shorts pay longs), perp cheap:
        // all desks lean long except the arb desk leans slightly the other way
        // only via funding diff — here Backpack funding equals Phoenix, so arb
        // follows basis (perp cheap -> long). Use a split tape for dissent:
        // strong momentum + expensive perp.
        let desks = default_desks(5_000.0);
        let pm = PortfolioManager::default();
        let risk = RiskAgent {
            limits: RiskLimits::default(),
        };
        let c = run_cycle(&snap(8.0, 101.0, -60.0, -60.0), &desks, &pm, &risk, 0.0);
        assert!(matches!(c.action, ConsensusAction::Long), "{:?} net={}", c.action, c.net_score);
        assert!(c.approved_intent.is_some());
        assert!(
            c.dissent.iter().any(|d| d.contains("ArbDesk")),
            "dissent should record ArbDesk: {:?}",
            c.dissent
        );
    }

    #[test]
    fn risk_veto_overrides_majority() {
        // Desks unanimously bullish, but the proposed size breaches the gate.
        let desks = default_desks(1_000_000.0);
        let pm = PortfolioManager::default();
        let risk = RiskAgent {
            limits: RiskLimits::default(), // max order $25k
        };
        let c = run_cycle(&snap(8.0, 100.0, -80.0, -80.0), &desks, &pm, &risk, 0.0);
        assert!(matches!(c.action, ConsensusAction::Vetoed), "{:?}", c.action);
        assert!(c.veto_reason.is_some());
        assert!(c.approved_intent.is_none());
    }

    #[test]
    fn swarm_trait_runs() {
        let swarm = HedgeFundSwarm {
            snapshot: snap(0.1, 100.0, 2.0, 2.0),
            size_usd: 1_000.0,
            limits: RiskLimits::default(),
            day_pnl_usd: 0.0,
        };
        assert_eq!(swarm.name(), "solana-hedge-fund");
        let out = futures::executor::block_on(swarm.run("cycle".into())).unwrap();
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("Hold"), "{json}");
    }
}
