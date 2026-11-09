# Paper-only hedge-fund harness HTTP server (no keys, no signing, live_trading=false).
FROM rust:1-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked --bin hedge-fund-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/hedge-fund-server /usr/local/bin/hedge-fund-server
ENV PORT=18787 RUST_LOG=info
EXPOSE 18787
USER nobody
CMD ["hedge-fund-server"]
