set shell := ["bash", "-euo", "pipefail", "-c"]

check: check-rust check-app

check-rust:
    cargo fmt --all --check
    cargo check --workspace --all-targets --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --all-targets --locked

check-app:
    cd app && bun install --frozen-lockfile
    cd app && bun run lint
    cd app && bun run test
    cd app && bun run build
