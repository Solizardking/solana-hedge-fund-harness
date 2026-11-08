//! Demo: one OFFLINE paper swarm cycle on fixture data (the tape that made
//! the Python fund's arb desk famous on 2026-11-08: a +432.5% APR
//! Phoenix-vs-Backpack funding differential), then a gated paper fill.
//! No network, no wallet, no signing.

use solana_hedge_fund::models::MarketSnapshot;
use solana_hedge_fund::paper::PaperBook;
use solana_hedge_fund::risk::{self, GateDecision, RiskLimits};
use solana_hedge_fund::swarm::{
    default_desks, run_cycle, PortfolioManager, RiskAgent,
};
use std::collections::HashMap;

fn main() {
    let snapshot = MarketSnapshot {
        symbol: "SOL".into(),
        spot_price: 89.25,
        perp_mark: 89.42,
        return_24h_pct: 1.8,
        funding_apr_phoenix_pct: 442.5,
        funding_apr_backpack_pct: 10.0,
    };
    println!("== Solana Rust Hedge Fund Harness — offline paper cycle ==");
    println!(
        "SOL spot ${:.2} | perp mark ${:.2} | basis {:.1} bps | funding Phoenix {:.1}% vs Backpack {:.1}% APR",
        snapshot.spot_price,
        snapshot.perp_mark,
        snapshot.basis_bps(),
        snapshot.funding_apr_phoenix_pct,
        snapshot.funding_apr_backpack_pct
    );

    let limits = RiskLimits::default();
    let desks = default_desks(10_000.0);
    let pm = PortfolioManager::default();
    let risk_agent = RiskAgent { limits: limits.clone() };
    let consensus = run_cycle(&snapshot, &desks, &pm, &risk_agent, 0.0);

    for v in &consensus.votes {
        println!("  {:<10} score {:+.2} conf {:.1} — {}", v.desk, v.score, v.confidence, v.rationale);
    }
    println!("net score {:+.3} -> action {:?}", consensus.net_score, consensus.action);
    for d in &consensus.dissent {
        println!("  dissent: {d}");
    }
    if let Some(reason) = &consensus.veto_reason {
        println!("  RISK VETO: {reason}");
    }

    let mut book = PaperBook::new(100_000.0);
    if let Some(intent) = &consensus.approved_intent {
        match risk::evaluate(intent, 0.0, &limits) {
            GateDecision::Allow => {
                let fill = book.fill(intent, snapshot.perp_mark).expect("paper fill");
                println!(
                    "paper fill: {:?} {:.4} {} @ ${:.4} (cost ${:.2})",
                    intent.side, fill.qty, intent.symbol, fill.fill_price, fill.cost_usd
                );
            }
            GateDecision::Deny(reason) => println!("gate denied the approved intent: {reason}"),
        }
    } else {
        println!("no approved intent — nothing filled (paper book stays flat)");
    }
    let marks = HashMap::from([("SOL".to_string(), snapshot.perp_mark)]);
    println!("book equity at mark: ${:.2}", book.equity(&marks));
    println!("Grok layer: {}", solana_hedge_fund::llm::GrokClient::from_env().status());
}
