# syntax=docker/dockerfile:1.18.0@sha256:dabfc0969b935b2080555ace70ee69a5261af8a8f1b4df97b9e7fbcf6722eddf

FROM rust:1.98.0-bookworm@sha256:4e4a7e7939c17991ab35f2b8c2e67593980f771d28f6b1254b1850f860fd0c7f AS server-builder
WORKDIR /workspace
ENV RUSTUP_TOOLCHAIN=1.98.0

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --locked -p sparrow-server --bin sparrow-server

FROM oven/bun:1.4.0@sha256:18639686662e5cd8a963ffb967dd130034a2a2d076a52e65dfd4fe18f75cc038 AS app-builder
WORKDIR /workspace/app

COPY app/package.json app/bun.lockb ./
RUN bun install --frozen-lockfile
COPY app ./
RUN bun run build

FROM gcr.io/distroless/cc-debian13:nonroot-amd64@sha256:1d2e87077bb3b12be8622609c5975fed6a3cba63e68fed53209293be10f7022c AS runtime
ARG VCS_REF=unknown

LABEL org.opencontainers.image.source="https://github.com/ponbac/sparrow-tv" \
      org.opencontainers.image.revision="${VCS_REF}"

WORKDIR /srv/sparrow
COPY --from=server-builder /workspace/target/release/sparrow-server /usr/local/bin/sparrow-server
COPY --from=app-builder /workspace/app/dist ./app/dist

USER 65532:65532
# Publishing is explicit in deployment/rehearsal. Dockerfile frontend 1.18.0
# serializes EXPOSE history with a process-local pointer, breaking byte reproduction.
HEALTHCHECK --interval=30s --timeout=3s --start-period=12m --retries=3 \
  CMD ["/usr/local/bin/sparrow-server", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/sparrow-server"]
