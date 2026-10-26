FROM rust:1.99-slim-bookworm@sha256:86234e857a413004348b471d11a4ebe1aa92d2eae2b6b143cac68fd1ac3c48be AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --locked --bin ownsearch

FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8

ARG OWNSEARCH_VERSION=0.1.0
LABEL org.opencontainers.image.title="OwnSearch" \
      org.opencontainers.image.description="Quality-ranked public SearXNG search for MCP clients" \
      org.opencontainers.image.source="https://github.com/devops-infinity/ownsearch" \
      org.opencontainers.image.version="${OWNSEARCH_VERSION}" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home ownsearch

COPY --from=builder /build/target/release/ownsearch /usr/local/bin/ownsearch

USER ownsearch
ENV OWNSEARCH_STATE_DIR=/home/ownsearch/.local/state/ownsearch \
    OWNSEARCH_LEDGER_DIR=/home/ownsearch/.local/share/ownsearch/ledger \
    OWNSEARCH_BIND=0.0.0.0:8774

EXPOSE 8774
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD curl -fsS http://127.0.0.1:8774/healthz || exit 1

ENTRYPOINT ["ownsearch"]
CMD ["serve", "--http"]
