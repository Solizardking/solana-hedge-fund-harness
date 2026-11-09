//! Solana Hedge Fund Harness — a paper-first, Solana-native fund of agents
//! built on swarms-rs (The-Swarm-Corporation), mirroring the Python `sol_fund`
//! package in ~/workspace/ai-hedge-fund.
//!
//! Doctrine (inherited from sol_fund):
//! - Paper / dry-run first. Live execution is refused by design: venues emit
//!   UNSIGNED order intents only; there is no signing code and no keypair
//!   anywhere in this crate.
//! - Deterministic risk gate in front of every fill; the RiskAgent holds a
//!   veto that overrides swarm consensus.
//! - The Grok (xAI) layer is advisory narrative only and ABSTAINS when
//!   XAI_API_KEY is not configured. It never changes a score.

pub mod api;
pub mod data;
pub mod llm;
pub mod models;
pub mod paper;
pub mod perps;
pub mod risk;
pub mod signals;
pub mod swarm;
pub mod venues;
