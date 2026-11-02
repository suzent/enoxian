#!/bin/sh
# Start one simulated device, or the local bootstrap server and Iroh relay.
#
#   device-entrypoint            run this device's daemon in the foreground
#   device-entrypoint relay      run `enox bootstrap serve` with a dev relay
#   device-entrypoint <cmd...>   run anything else (a shell, a one-off enox call)
set -eu

case "${1:-device}" in
device)
    port="${ENOXIAN_PORT:-36521}"
    # The label is how the device shows up in `enox who` and in run logs. The
    # daemon creates the identity on first start, so a fresh HOME gets one
    # short run to create it; the label is then set before the real start, so
    # the first presence carries it.
    if [ -n "${ENOXIAN_DEVICE_LABEL:-}" ]; then
        if [ ! -f "$HOME/.enoxian/identity.toml" ]; then
            enox daemon run --port "$port" >/dev/null 2>&1 &
            pid=$!
            while [ ! -f "$HOME/.enoxian/identity.toml" ]; do sleep 0.2; done
            kill "$pid" 2>/dev/null || true
            wait "$pid" || true
        fi
        enox identity set-label "$ENOXIAN_DEVICE_LABEL" >/dev/null
    fi
    exec enox daemon run --port "$port"
    ;;
relay)
    exec enox bootstrap serve --port "${BOOTSTRAP_PORT:-36521}" \
        --iroh-relay --iroh-relay-dev-port "${RELAY_PORT:-3340}"
    ;;
*)
    exec "$@"
    ;;
esac
