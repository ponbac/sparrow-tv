FROM lukemathwalker/cargo-chef:latest-rust-1.82.0 AS chef
WORKDIR /app
RUN apt update && apt install lld clang -y

FROM chef AS planner
COPY . .
# Compute a lock-like file for our project
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build our project dependencies, not our application!
RUN cargo chef cook --release --recipe-path recipe.json
# Up to this point, if our dependency tree stays the same,
# all layers should be cached.
COPY . .
# Build our project
RUN cargo build --release --bin sparrow-tv

FROM oven/bun:1.1.33 AS bun
WORKDIR /app
COPY /app .
RUN bun install --frozen-lockfile
RUN bun run build

FROM ubuntu:22.04 AS runtime
WORKDIR /app
RUN apt-get update -y \
    && apt-get install -y --no-install-recommends openssl ca-certificates \
    && apt-get autoremove -y \
    && apt-get clean -y \
    && rm -rf /var/lib/apt/lists/*
# Copy necessary files from builder
COPY --from=builder /app/target/release/sparrow-tv sparrow-tv
COPY --from=bun /app/dist app/dist
ENTRYPOINT ["./sparrow-tv"]
