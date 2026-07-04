# Docker image for an enoxian bootstrap / relay node.
#
# A bootstrap node runs `enoxd --bootstrap`: a public rendezvous + circuit relay
# server. It joins no circles and holds no circle PSKs (see docs/concepts/
# security.md), so it needs no persistent secrets beyond its own stable keypair,
# which it generates at ~/.enoxian/bootstrap.key on first run — mount a volume
# there to keep the peer id stable across restarts.
#
# Build:   docker build -t enoxian-bootstrap .
# Run:     docker run -p 36521:36521/tcp -p 36521:36521/udp \
#            -v enoxian-bootstrap:/root/.enoxian enoxian-bootstrap

# ── Build stage ──────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS build
WORKDIR /src

# The bootstrap binary does not serve the frontend, so we skip the npm build by
# not providing frontend/static; build.rs only builds the frontend when the
# frontend/ dir is present in a release build. Copy just what cargo needs.
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src ./src

# Build only the daemon binary, release.
RUN cargo build --release --bin enoxd

# ── Runtime stage ────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/enoxd /usr/local/bin/enoxd

# TCP (relay) and UDP (QUIC rendezvous) on the same port.
EXPOSE 36521/tcp
EXPOSE 36521/udp

# Persist the stable bootstrap keypair across restarts by mounting /root/.enoxian.
VOLUME ["/root/.enoxian"]

ENTRYPOINT ["enoxd", "--bootstrap"]
CMD ["--port", "36521"]
