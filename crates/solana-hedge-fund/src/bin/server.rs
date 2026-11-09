//! Paper-only hedge fund + perps HTTP server.
//! Env: PORT (default 18787), STARTING_CASH (default 100000), BIND_ADDR (default "::", dual-stack;
//! Railway private networking resolves *.railway.internal over IPv6).
//! Never enables live trading. Never loads keypairs.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
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
    let ip: IpAddr = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(IpAddr::V6(Ipv6Addr::UNSPECIFIED));
    let addr = SocketAddr::new(ip, port);
    tracing::info!(
        "hedge-fund-server paper mode on http://{addr} (live_trading=false)"
    );
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
