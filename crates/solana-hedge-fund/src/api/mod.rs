//! Small axum HTTP API for the paper hedge-fund + perps backend.
//! Endpoints: /health, /status, /markets, /positions, /run-cycle.
//! No signing. LIVE_TRADING is never enabled here.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use crate::data;
use crate::models::MarketSnapshot;
use crate::perps::PerpsBook;
use crate::risk::{self, GateDecision, RiskLimits};
use crate::swarm::{default_desks, run_cycle, Consensus, PortfolioManager, RiskAgent};
use crate::venues::OrderIntent;

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Mutex<FundState>>,
}

pub struct FundState {
    pub perps: PerpsBook,
    pub limits: RiskLimits,
    pub last_consensus: Option<Consensus>,
    pub last_snapshot: Option<MarketSnapshot>,
    pub last_fill: Option<Value>,
    pub day_pnl_usd: f64,
    pub cycle_count: u64,
    pub started_at: std::time::Instant,
}

impl FundState {
    pub fn new(starting_cash: f64) -> Self {
        Self {
            perps: PerpsBook::new(starting_cash),
            limits: RiskLimits::default(),
            last_consensus: None,
            last_snapshot: None,
            last_fill: None,
            day_pnl_usd: 0.0,
            cycle_count: 0,
            started_at: std::time::Instant::now(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub service: &'static str,
    pub mode: &'static str,
    pub live_trading: bool,
    pub uptime_sec: u64,
    pub cycle_count: u64,
    pub cash: f64,
    pub equity: f64,
    pub day_pnl_usd: f64,
    pub positions: usize,
    pub last_action: Option<String>,
    pub grok: &'static str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/markets", get(markets))
        .route("/positions", get(positions))
        .route("/run-cycle", post(run_cycle_handler))
        .route("/cycle", post(run_cycle_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "solana-hedge-fund-perps",
        "live_trading": false,
        "paper": true
    }))
}

async fn status(State(st): State<AppState>) -> Json<StatusResponse> {
    let g = st.inner.lock().await;
    Json(StatusResponse {
        service: "solana-hedge-fund-perps",
        mode: "paper",
        live_trading: false,
        uptime_sec: g.started_at.elapsed().as_secs(),
        cycle_count: g.cycle_count,
        cash: g.perps.cash,
        equity: g.perps.equity(),
        day_pnl_usd: g.day_pnl_usd,
        positions: g.perps.positions.len(),
        last_action: g
            .last_consensus
            .as_ref()
            .map(|c| format!("{:?}", c.action)),
        grok: crate::llm::GrokClient::from_env().status(),
    })
}

async fn markets() -> Json<Value> {
    match build_live_snapshot().await {
        Ok((snap, extra)) => Json(json!({
            "ok": true,
            "snapshot": snap,
            "extra": extra,
            "source": "jupiter+imperial+phoenix"
        })),
        Err(e) => Json(json!({ "ok": false, "error": e })),
    }
}

async fn positions(State(st): State<AppState>) -> Json<Value> {
    let g = st.inner.lock().await;
    Json(json!({
        "ok": true,
        "cash": g.perps.cash,
        "equity": g.perps.equity(),
        "positions": g.perps.positions_vec(),
        "last_fill": g.last_fill,
        "last_consensus": g.last_consensus,
    }))
}

#[derive(Debug, Deserialize, Default)]
pub struct CycleRequest {
    /// Optional override snapshot for offline/demo cycles.
    pub snapshot: Option<MarketSnapshot>,
    pub size_usd: Option<f64>,
}

async fn run_cycle_handler(
    State(st): State<AppState>,
    body: Option<Json<CycleRequest>>,
) -> Json<Value> {
    let req = body.map(|j| j.0).unwrap_or_default();
    let size = req.size_usd.unwrap_or(5_000.0);

    let (snapshot, market_meta) = if let Some(s) = req.snapshot {
        (s, json!({ "source": "request" }))
    } else {
        match build_live_snapshot().await {
            Ok((s, meta)) => (s, meta),
            Err(e) => {
                return Json(json!({ "ok": false, "error": e }));
            }
        }
    };

    let desks = default_desks(size);
    let pm = PortfolioManager::default();
    let mut g = st.inner.lock().await;
    let risk_agent = RiskAgent {
        limits: g.limits.clone(),
    };
    let consensus = run_cycle(&snapshot, &desks, &pm, &risk_agent, g.day_pnl_usd);

    let mut fill_json = Value::Null;
    let mut risk_verdict = json!({ "decision": "no_intent" });

    if let Some(intent) = consensus.approved_intent.clone() {
        // Extra perps risk: margin + liq distance.
        let perps_check = evaluate_perps_risk(&intent, snapshot.perp_mark, &g);
        risk_verdict = json!(perps_check);
        if perps_check.get("allow").and_then(|v| v.as_bool()) == Some(true) {
            match risk::evaluate(&intent, g.day_pnl_usd, &g.limits) {
                GateDecision::Allow => match g.perps.apply_intent(&intent, snapshot.perp_mark) {
                    Ok(fill) => {
                        // Accrue one hour of funding at Phoenix hourly rate if present.
                        if let Some(rate) = market_meta
                            .get("funding_rate_per_hour")
                            .and_then(|v| v.as_f64())
                        {
                            g.perps
                                .accrue_funding(&snapshot.symbol, snapshot.perp_mark, rate);
                        }
                        let mut marks = HashMap::new();
                        marks.insert(snapshot.symbol.clone(), snapshot.perp_mark);
                        g.perps.mark_all(&marks);
                        g.day_pnl_usd = g.perps.equity() - 100_000.0;
                        fill_json = serde_json::to_value(&fill).unwrap_or(Value::Null);
                        g.last_fill = Some(fill_json.clone());
                    }
                    Err(e) => {
                        risk_verdict = json!({ "allow": false, "reason": e });
                    }
                },
                GateDecision::Deny(reason) => {
                    risk_verdict = json!({ "allow": false, "reason": reason });
                }
            }
        }
    }

    g.cycle_count += 1;
    g.last_snapshot = Some(snapshot.clone());
    g.last_consensus = Some(consensus.clone());

    Json(json!({
        "ok": true,
        "live_trading": false,
        "paper": true,
        "cycle": g.cycle_count,
        "snapshot": snapshot,
        "market_meta": market_meta,
        "votes": consensus.votes,
        "net_score": consensus.net_score,
        "action": format!("{:?}", consensus.action),
        "dissent": consensus.dissent,
        "veto_reason": consensus.veto_reason,
        "approved_intent": consensus.approved_intent,
        "risk": risk_verdict,
        "fill": fill_json,
        "positions": g.perps.positions_vec(),
        "cash": g.perps.cash,
        "equity": g.perps.equity(),
        "day_pnl_usd": g.day_pnl_usd,
        "grok": crate::llm::GrokClient::from_env().status(),
    }))
}

fn evaluate_perps_risk(intent: &OrderIntent, mark: f64, g: &FundState) -> Value {
    let margin = intent.margin_usd();
    if margin > g.perps.cash {
        return json!({ "allow": false, "reason": format!("margin {:.2} > cash {:.2}", margin, g.perps.cash) });
    }
    let liq = crate::perps::liq_price(intent.side, mark, intent.leverage);
    let dist = crate::perps::liq_distance_frac(intent.side, mark, liq);
    if dist < 0.05 && !intent.reduce_only {
        return json!({
            "allow": false,
            "reason": format!("liq distance {:.2}% below 5% floor", dist * 100.0),
            "liquidation_price": liq,
            "liq_distance_frac": dist
        });
    }
    json!({
        "allow": true,
        "margin_usd": margin,
        "liquidation_price": liq,
        "liq_distance_frac": dist
    })
}

/// Build a MarketSnapshot from Jupiter spot + Imperial mark/funding (read-only).
pub async fn build_live_snapshot() -> Result<(MarketSnapshot, Value), String> {
    let spot = data::jupiter_price_usd("So11111111111111111111111111111111111111112").await?;
    let marks = data::imperial_mark_prices().await.unwrap_or_default();
    let funding = data::imperial_funding_rates().await.unwrap_or_default();
    let phoenix_markets = data::imperial_phoenix_markets().await.unwrap_or_else(|_| json!([]));

    let perp_mark = marks
        .get("SOL")
        .copied()
        .unwrap_or(spot);
    let phx_hourly = funding
        .get("SOL")
        .and_then(|v| v.get("phoenix"))
        .and_then(|v| v.get("longFundingRatePerHourPercent"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    // Convert hourly % to APR %: * 24 * 365
    let phx_apr = phx_hourly * 24.0 * 365.0;
    let bp_hourly = funding
        .get("SOL")
        .and_then(|v| v.get("backpack").or_else(|| v.get("sunrise")))
        .and_then(|v| v.get("longFundingRatePerHourPercent"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let bp_apr = bp_hourly * 24.0 * 365.0;

    // Approximate 24h return from Jupiter priceChange24h when available.
    let ret = data::jupiter_change_24h("So11111111111111111111111111111111111111112")
        .await
        .unwrap_or(0.0);

    let snap = MarketSnapshot {
        symbol: "SOL".into(),
        spot_price: spot,
        perp_mark,
        return_24h_pct: ret,
        funding_apr_phoenix_pct: phx_apr,
        funding_apr_backpack_pct: bp_apr,
    };
    let meta = json!({
        "marks": marks,
        "funding": funding.get("SOL"),
        "funding_rate_per_hour": phx_hourly / 100.0, // percent -> fraction
        "phoenix_markets_count": phoenix_markets.as_array().map(|a| a.len()).unwrap_or(0),
        "source": "live"
    });
    Ok((snap, meta))
}
