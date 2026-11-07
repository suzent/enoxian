# The published relay image: ghcr.io/suzent/enoxian-bootstrap.
#
# The release workflow builds this from the static musl binaries it has
# already built, smoke-tested and attested, so the image runs exactly the `enox`
# that the release archives carry (release version string, Iroh relay compiled
# in) instead of a second compile of the same source. The repository's root
# `Dockerfile` is the one that builds from source.
#
# Build context: one directory per platform holding that platform's `enox`:
#
#   linux-amd64/enox   from enoxian-linux-x86_64.tar.gz
#   linux-arm64/enox   from enoxian-linux-aarch64.tar.gz
#
# Nothing here RUNs inside the image, so a multi-platform build needs no
# emulation.

FROM alpine:3.22

ARG TARGETARCH
COPY --chmod=755 linux-${TARGETARCH}/enox /usr/local/bin/enox

# The HTTP endpoints listen on --port. The Iroh relay, when turned on, takes
# 443 (HTTPS; the certificate is issued over TLS-ALPN-01), 80 (Iroh's
# captive-portal probe) and 7842/udp (QUIC address discovery).
EXPOSE 36521/tcp
EXPOSE 80/tcp
EXPOSE 443/tcp
EXPOSE 7842/udp

# The server's stable keypair, its Let's Encrypt certificate and the sealed
# short-invite blobs all live here. Without a volume each restart gets a new
# peer id, asks Let's Encrypt for a new certificate (which is rate limited) and
# breaks every outstanding short invite.
VOLUME ["/root/.enoxian"]

# `/version` is the cheapest endpoint the server has. Set ENOXIAN_HEALTH_PORT
# when --port is not 36521. BusyBox wget comes with the base image.
ENV ENOXIAN_HEALTH_PORT=36521
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD wget -q -O /dev/null "http://127.0.0.1:${ENOXIAN_HEALTH_PORT}/version" || exit 1

ENTRYPOINT ["enox", "bootstrap", "serve"]
CMD ["--port", "36521"]
