//! Live read-only data clients (reqwest). These are never called from tests;
//! the desks run on `MarketSnapshot` values from any source, fixtures included.
//!
//! Env names only — values are read at runtime and never logged:
//! - HELIUS_RPC_URL: full Helius RPC URL (key embedded in the URL by the owner)
//! - XAI_API_KEY: used by crate::llm, not here
//!
//! Imperial public REST (read-only): https://api.imperial.space/api/v1
//! Phoenix public REST via Imperial `/phoenix/markets` (and optional perp-api).

use std::collections::HashMap;

use serde_json::{json, Value};

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

const IMPERIAL_V1: &str = "https://api.imperial.space/api/v1";

/// Jupiter Price API (v3 via lite-api host). Accepts mint or symbol key.
pub async fn jupiter_price_usd(id: &str) -> Result<f64, String> {
    let url = format!("https://lite-api.jup.ag/price/v3?ids={id}");
    let v: Value = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    v.get(id)
        .and_then(|p| p.get("usdPrice"))
        .and_then(|p| p.as_f64())
        .ok_or_else(|| format!("no usdPrice for {id} in Jupiter response"))
}

pub async fn jupiter_change_24h(id: &str) -> Result<f64, String> {
    let url = format!("https://lite-api.jup.ag/price/v3?ids={id}");
    let v: Value = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    v.get(id)
        .and_then(|p| p.get("priceChange24h"))
        .and_then(|p| p.as_f64())
        .ok_or_else(|| format!("no priceChange24h for {id}"))
}

/// Phoenix perps market list (public REST via Imperial).
pub async fn phoenix_markets() -> Result<Value, String> {
    imperial_phoenix_markets().await
}

fn extract_mark(row: &Value) -> Option<f64> {
    for key in ["price", "markPrice", "mark_price"] {
        if let Some(p) = row.get(key).and_then(|x| x.as_f64()) {
            return Some(p);
        }
    }
    for venue in ["index", "phoenix", "jupiter", "sunrise"] {
        if let Some(node) = row.get(venue) {
            for key in ["price", "markPrice"] {
                if let Some(p) = node.get(key).and_then(|x| x.as_f64()) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Imperial /mark-prices → symbol → USD mark.
pub async fn imperial_mark_prices() -> Result<HashMap<String, f64>, String> {
    let v: Value = http()
        .get(format!("{IMPERIAL_V1}/mark-prices"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let rows = v
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = HashMap::new();
    for row in rows {
        if let (Some(sym), Some(price)) = (
            row.get("symbol").and_then(|s| s.as_str()).map(|s| s.to_uppercase()),
            extract_mark(&row),
        ) {
            out.insert(sym, price);
        }
    }
    Ok(out)
}

/// Imperial /funding-rates raw map keyed by symbol.
pub async fn imperial_funding_rates() -> Result<HashMap<String, Value>, String> {
    let v: Value = http()
        .get(format!("{IMPERIAL_V1}/funding-rates"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let rows = v
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = HashMap::new();
    for row in rows {
        if let Some(sym) = row.get("symbol").and_then(|s| s.as_str()) {
            out.insert(sym.to_uppercase(), row);
        }
    }
    Ok(out)
}

pub async fn imperial_phoenix_markets() -> Result<Value, String> {
    let v: Value = http()
        .get(format!("{IMPERIAL_V1}/phoenix/markets"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(v)
}

/// Best-effort orderbook / market stats from Phoenix public API.
pub async fn phoenix_market_stats(symbol: &str) -> Result<Value, String> {
    let url = format!("https://perp-api.phoenix.trade/v1/market/{symbol}/stats");
    let resp = http().get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("phoenix stats {}: {}", symbol, resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

/// Helius RPC getBalance for a wallet, in lamports. Requires HELIUS_RPC_URL
/// (falls back to HELIUS_MAINNET_RPC_URL, then public mainnet).
pub async fn helius_balance_lamports(pubkey: &str) -> Result<u64, String> {
    let rpc = std::env::var("HELIUS_RPC_URL")
        .or_else(|_| std::env::var("HELIUS_MAINNET_RPC_URL"))
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".into());
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "getBalance", "params": [pubkey]
    });
    let v: Value = http()
        .post(&rpc)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    v.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|x| x.as_u64())
        .ok_or_else(|| "no result.value in Helius getBalance response".to_string())
}
