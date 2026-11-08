//! Live read-only data clients (reqwest). These are never called from tests;
//! the desks run on `MarketSnapshot` values from any source, fixtures included.
//!
//! Env names only — values are read at runtime and never logged:
//! - HELIUS_RPC_URL: full Helius RPC URL (key embedded in the URL by the owner)
//! - XAI_API_KEY: used by crate::llm, not here

use serde_json::{json, Value};

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

/// Jupiter Price API (v3 via lite-api host, falls back shape-tolerant parsing).
pub async fn jupiter_price_usd(symbol: &str) -> Result<f64, String> {
    let url = format!("https://lite-api.jup.ag/price/v3?ids={symbol}");
    let v: Value = http()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    v.get(symbol)
        .and_then(|p| p.get("usdPrice"))
        .and_then(|p| p.as_f64())
        .ok_or_else(|| format!("no usdPrice for {symbol} in Jupiter response"))
}

/// Phoenix perps market list (public REST).
pub async fn phoenix_markets() -> Result<Value, String> {
    let v: Value = http()
        .get("https://perp-api.phoenix.trade/markets")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(v)
}

/// Helius RPC getBalance for a wallet, in lamports. Requires HELIUS_RPC_URL.
pub async fn helius_balance_lamports(pubkey: &str) -> Result<u64, String> {
    let rpc = std::env::var("HELIUS_RPC_URL")
        .map_err(|_| "HELIUS_RPC_URL not configured".to_string())?;
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
