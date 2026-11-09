# Solana Rust Hedge Fund Harness

**Built on [swarms-rs](https://github.com/The-Swarm-Corporation/swarms-rs)** (The-Swarm-Corporation, Apache-2.0) — a Solana-native, paper-first hedge fund of agents in Rust, mirroring the proven Python [`sol_fund`](https://github.com/Solizardking/ai-hedge-fund) design.

Three desks (Spot, Perps, Arb) evaluate a market snapshot and emit typed votes; a Portfolio Manager takes a weighted consensus with dissent recorded; a Risk Agent holds a veto that overrides any majority. A paper book fills approved intents at live marks with a fee + slippage cost model. The Grok (xAI) layer adds optional narrative — and abstains when it has no key.

## Paper-first doctrine

Live execution is **refused by design**, exactly as in the Python fund:

- Venues emit **unsigned order intents only** (symbol, side, size, leverage, venue). There is no signing code and no keypair anywhere in this crate.
- Every fill is a paper fill against the in-memory book.
- The deterministic risk gate runs before any fill, deny-by-default.
- Grok is advisory narrative. It never changes a desk score, and it returns `not_configured` / abstains when `XAI_API_KEY` is unset — it never fabricates an opinion.

## Architecture — map to sol_fund

| Rust module | Python counterpart | What it does |
| --- | --- | --- |
| `src/data/` | `sol_fund/data/` | Jupiter price, Phoenix perps market list, Helius RPC balance — read-only reqwest clients |
| `src/venues/` | `sol_fund/venues/` | `OrderIntent` + `Venue` types (unsigned intents only) |
| `src/paper/` | `sol_fund/paper/` | Fill engine at provided marks, fee + slippage cost model, positions + equity |
| `src/risk/` | `sol_fund/risk/` | Deterministic gate: max order notional, max leverage, daily loss stop (reduce-only exempt) |
| `src/signals/` | `sol_fund/signals/` | Pure functions: momentum, funding carry, spot-perp basis + funding-differential arb |
| `src/swarm/` | `sol_fund/swarm/` | SpotDesk / PerpsDesk / ArbDesk, RiskAgent veto, PortfolioManager weighted consensus with dissent |
| `src/llm/` | `sol_fund/llm/` | Grok (xAI) client; abstains when unconfigured |
| `src/models.rs` | `sol_fund/models.py` | `Side`, `MarketSnapshot` (spot, perp mark, 24h return, per-venue funding) |

### How swarms-rs is used — and where it isn't

The fund as a whole implements the upstream **`swarms_rs::structs::swarm::Swarm`** trait (`HedgeFundSwarm`): one task in, one serializable consensus out — the right shape for a fund cycle.

The desks deliberately do **not** implement swarms-rs' `Agent` trait. That trait models an LLM agent loop (`run(task) -> String`, planning and long-term-memory hooks); our desks are deterministic evaluators that must emit typed scores and unsigned intents. Dressing them as free-text LLM loops would fabricate narrative as signal, which the fund doctrine forbids. They sit behind a thin local `Desk` trait instead, composed by the `Swarm` implementation. See the comment block in `src/swarm/mod.rs`.

## Environment (names only)

| Name | Used by | Notes |
| --- | --- | --- |
| `HELIUS_RPC_URL` | `data::helius_balance_lamports` | Full RPC URL; the owner keeps any key inside it. Never logged. |
| `XAI_API_KEY` | `llm::GrokClient` | Unset ⇒ Grok abstains (`not_configured`). |
| `GROK_MODEL` | `llm::GrokClient` | Optional, default `grok-4.6`. |

No other secrets exist in this system. Tests need none of these and make no network calls.

## Build & run

```bash
cargo build
cargo test            # fully offline: risk edges, paper PnL, arb math, consensus + veto
cargo run             # one offline paper swarm cycle on fixture data (the 2026-11-08
                      # +432.5% APR Phoenix-vs-Backpack funding-differential tape)
```

The demo prints each desk's vote, the consensus (with dissent), the gated paper fill, and book equity.

## Layout

```
Cargo.toml                     workspace
crates/solana-hedge-fund/
  Cargo.toml                   swarms-rs = "0.3.0" (crates.io); path-dep swap documented inside
  src/{data,venues,paper,risk,signals,swarm,llm}/mod.rs
  src/models.rs, src/lib.rs, src/main.rs
LICENSE                        Apache-2.0 (same license as upstream swarms-rs)
NOTICE                         upstream attribution
```

## Credits

- swarms-rs by The Swarm Corporation — https://github.com/The-Swarm-Corporation/swarms-rs
- Design lineage: the Python `sol_fund` in https://github.com/Solizardking/ai-hedge-fund

## Perps backend (paper)

Branch `perps-backend` adds a Phoenix/Imperial read-only data path, a paper
perps engine (margin, leverage, liq price, funding accrual), and an HTTP API:

```bash
export OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include
cargo build --release
cargo run --release --bin hedge-fund-server   # http://0.0.0.0:18787
# GET  /health /status /markets /positions
# POST /run-cycle   — live Jupiter+Imperial snapshot → desk votes → paper fill
```

`LIVE_TRADING` is never enabled here. Venues still emit unsigned intents only.
