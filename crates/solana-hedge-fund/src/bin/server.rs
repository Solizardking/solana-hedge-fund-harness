//! Paper-only hedge fund + perps HTTP server.
//! Env: PORT (default 8787), STARTING_CASH (default 100000).
//! Never enables live trading. Never loads keypairs.

use std::net::SocketAddr;
use std::sync::Arc;

use solana_hedge_fund::api::{self, AppState, FundState};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(18787);
    let cash: f64 = std::env::var("STARTING_CASH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000.0);

    let state = AppState {
        inner: Arc::new(Mutex::new(FundState::new(cash))),
    };
    let app = api::router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(
        "hedge-fund-server paper mode on http://{addr} (live_trading=false)"
    );
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
