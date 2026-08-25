# Docker image for an enoxian bootstrap / relay node.
#
# A bootstrap node runs `enox bootstrap serve`: a public rendezvous + circuit
# relay server. It joins no circles and holds no circle PSKs (see docs/concepts/
# security.md), so it needs no persistent secrets beyond its own stable keypair,
# which it generates at ~/.enoxian/bootstrap.key on first run — mount a volume
# there to keep the peer id stable across restarts.
#
# Build:   docker build -t enoxian-bootstrap .
# Run:     docker run -p 36521:36521/udp -p 36521:36521/tcp -p 36522:36522/tcp \
#            -v enoxian-bootstrap:/root/.enoxian enoxian-bootstrap
#
# CI builds this image on every push to main and on pull requests that touch its
# inputs, so it cannot drift away from the CLI unnoticed.

# ── Build stage ──────────────────────────────────────────────────────────────
# Pinned to the same toolchain as CI (see RUST_VERSION in .github/workflows).
FROM rust:1.97-bookworm AS build
WORKDIR /src

# A bootstrap node never serves the WebUI, so skip the npm build entirely and
# hand rust-embed an empty directory. The embed macro only requires that the
# folder exists; `serve` is not reachable from `bootstrap serve` anyway.
ENV ENOXIAN_SKIP_FRONTEND_BUILD=1

COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src ./src
RUN mkdir -p static

RUN cargo build --release --locked --bin enox

# ── Runtime stage ────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/enox /usr/local/bin/enox

# QUIC rendezvous (UDP) and the status endpoint (TCP) share --port; the circuit
# relay listens on --relay-port, which defaults to --port + 1.
EXPOSE 36521/udp
EXPOSE 36521/tcp
EXPOSE 36522/tcp

# Persist the stable bootstrap keypair across restarts by mounting /root/.enoxian.
VOLUME ["/root/.enoxian"]

ENTRYPOINT ["enox", "bootstrap", "serve"]
CMD ["--port", "36521"]
