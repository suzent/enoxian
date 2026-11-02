# Image for one simulated enoxian device in the multi-device experiments.
#
# Each container runs `enox daemon run` in the foreground with its own HOME, so
# it has its own keypair, Circle configs, logs and ~/enoxian workspace — three
# containers are three separate devices. iproute2 is installed so a container
# can slow its own link with `tc qdisc add dev eth0 root netem delay 200ms`
# (needs cap_add NET_ADMIN, which compose.yaml grants).
#
# The binary is built with the relay feature, so the same image also runs the
# local bootstrap server and plain-HTTP Iroh relay the compose file starts
# (`enox bootstrap serve --iroh-relay --iroh-relay-dev-port ...`).
#
# Build from the repository root:
#   docker compose -f experiments/devices/compose.yaml build

# ── Build stage ──────────────────────────────────────────────────────────────
# Same toolchain as CI and the bootstrap image.
FROM rust:1.94-bookworm AS build
WORKDIR /src

# The experiments drive devices from the CLI, so skip the npm build. A release
# daemon still refuses to start without an embedded index.html; a placeholder
# page stands in for the WebUI.
ENV ENOXIAN_SKIP_FRONTEND_BUILD=1

COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src ./src
RUN mkdir -p static \
 && echo '<!doctype html><title>enoxian device</title><p>WebUI not built into the experiment image; use the enox CLI.' \
      > static/index.html

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked --bin enox --features iroh-relay-server

# ── Runtime stage ────────────────────────────────────────────────────────────
# Ubuntu rather than debian-slim: it matches the DGX hosts the agent CLIs will
# be added for later, and a bookworm-built binary runs on its newer glibc.
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates iproute2 iputils-ping jq curl procps \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/enox /usr/local/bin/enox
COPY experiments/devices/entrypoint.sh /usr/local/bin/device-entrypoint

# HOME is a volume per device; compose mounts a named one at /home/device.
ENV HOME=/home/device
WORKDIR /home/device

ENTRYPOINT ["device-entrypoint"]
