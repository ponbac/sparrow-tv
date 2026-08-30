set shell := ["bash", "-euo", "pipefail", "-c"]

check: check-rust check-app

check-rust:
    cargo fmt --all --check
    cargo check --workspace --all-targets --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings -D clippy::debug_assert_with_mut_call
    cargo test --workspace --all-targets --locked

check-app:
    cd app && bun install --frozen-lockfile
    cd app && bun run lint
    cd app && bun run test
    cd app && bun run build

container-repro revision output:
    bash scripts/verify-container-reproducibility.sh "{{revision}}" "{{output}}"

container-rehearse image revision manifest environment_file=".env.local":
    bash scripts/rehearse-hosted-container.sh "{{image}}" "{{revision}}" "{{manifest}}" "{{environment_file}}"
