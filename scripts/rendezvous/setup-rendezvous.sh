#!/usr/bin/env bash
# Run this on the VPS to install `enox bootstrap serve` as a systemd service.
# The enox binary must be available at /tmp/enox unless BINARY_SRC overrides it.
#
# Usage:
#   bash setup-rendezvous.sh [--port PORT] [--advertise-host HOST] [--auto-update stable|off]
#
# PORT (default 36521) is the HTTP port for /version, /peer-id, /pair and
# /invite. With --advertise-host the server also runs the Iroh relay on 443
# (HTTPS, Let's Encrypt certificate for HOST), 80 (Iroh's captive-portal
# probe) and 7842/udp (QUIC
# address discovery). --relay-port is accepted and ignored: the libp2p circuit
# relay it set is gone.
set -euo pipefail

PORT=36521
ADVERTISE_HOST=""
AUTO_UPDATE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port) PORT="$2"; shift 2 ;;
        --relay-port) shift 2 ;;  # ignored; see above
        --advertise-host) ADVERTISE_HOST="$2"; shift 2 ;;
        --auto-update) AUTO_UPDATE="$2"; shift 2 ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

# Preserve enabled updates when setup migrates a legacy relay unit name.
if [[ -z "$AUTO_UPDATE" ]] && systemctl is-enabled --quiet enoxian-relay-update.timer 2>/dev/null; then
    AUTO_UPDATE=stable
fi

if [[ -n "$AUTO_UPDATE" ]]; then
    [[ "$AUTO_UPDATE" == stable || "$AUTO_UPDATE" == off ]] || { echo 'Expected --auto-update stable|off'; exit 1; }
    test -f "$(dirname "$0")/setup-relay-updates.sh"
    test -f "$(dirname "$0")/update-relay.py"
    command -v python3 >/dev/null
fi

if [[ -n "$ADVERTISE_HOST" && ! "$ADVERTISE_HOST" =~ ^[A-Za-z0-9.-]+$ ]]; then
    echo "Invalid --advertise-host: $ADVERTISE_HOST"
    exit 1
fi

BINARY_SRC="${BINARY_SRC:-/tmp/enox}"
BINARY_DST="/usr/local/bin/enox"
SERVICE_NAME="enoxian-bootstrap"
SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
SERVICE_USER="enoxian"

echo "Setting up the enoxian bootstrap server on port $PORT"

ADVERTISE_ARGS=""
if [[ -n "$ADVERTISE_HOST" ]]; then
    ADVERTISE_ARGS=" --advertise-host $ADVERTISE_HOST --iroh-relay"
    echo "  Running the Iroh relay at https://$ADVERTISE_HOST"
else
    echo "  No --advertise-host: HTTP only, no Iroh relay (it needs a hostname for its certificate)"
fi

# Install binary
if [[ ! -f "$BINARY_SRC" ]]; then
    echo "Error: binary not found at $BINARY_SRC"
    echo "Run deploy-rendezvous.sh from your local machine instead."
    exit 1
fi

systemctl stop "$SERVICE_NAME" 2>/dev/null || true
systemctl disable --now enoxd-bootstrap 2>/dev/null || true
rm -f /etc/systemd/system/enoxd-bootstrap.service /usr/local/bin/enoxd

echo "  Installing binary $BINARY_DST"
cp "$BINARY_SRC" "$BINARY_DST"
chmod +x "$BINARY_DST"

# Create system user
if ! id "$SERVICE_USER" &>/dev/null; then
    echo "  Creating system user '$SERVICE_USER'"
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi

# Create the config directory and give ownership to the service user.
# The bootstrap keypair (~/.enoxian/bootstrap.key) lives here.
enoxian_DIR="/home/$SERVICE_USER/.enoxian"
mkdir -p "$enoxian_DIR"
chown -R "$SERVICE_USER:$SERVICE_USER" "$enoxian_DIR"

# Write systemd service
echo "  Writing $SERVICE_FILE"
cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=enoxian Bootstrap Server (HTTP + Iroh relay)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$BINARY_DST bootstrap serve --port $PORT$ADVERTISE_ARGS
# The Iroh relay listens on 80 and 443.
AmbientCapabilities=CAP_NET_BIND_SERVICE
Restart=always
RestartSec=5
User=$SERVICE_USER
Environment=HOME=/home/$SERVICE_USER
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

# Firewall
PORTS=("$PORT/tcp")
if [[ -n "$ADVERTISE_HOST" ]]; then
    PORTS+=(80/tcp 443/tcp 7842/udp)
fi
echo "  Opening ${PORTS[*]}"
if command -v ufw &>/dev/null; then
    for p in "${PORTS[@]}"; do
        ufw allow "$p" comment "enoxian bootstrap" 2>/dev/null || true
    done
elif command -v firewall-cmd &>/dev/null; then
    for p in "${PORTS[@]}"; do
        firewall-cmd --permanent --add-port="$p" 2>/dev/null || true
    done
    firewall-cmd --reload 2>/dev/null || true
else
    echo "  (no ufw/firewalld found - open ${PORTS[*]} manually)"
fi

# Enable and start
echo "  Enabling and starting service"
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl reset-failed "$SERVICE_NAME" 2>/dev/null || true
systemctl start "$SERVICE_NAME"

sleep 1
if systemctl is-active --quiet "$SERVICE_NAME"; then
    echo ""
    echo "Bootstrap server running on port $PORT"
    echo ""
    echo "  Peer ID:"
    curl -sf "http://localhost:$PORT/peer-id" | grep -o '"peer_id":"[^"]*"' | cut -d'"' -f4 \
        && echo "" || echo "  (starting up - try again in a moment)"
    if [[ -n "$ADVERTISE_HOST" ]]; then
        echo ""
        echo "  To put this relay in invites from your local machine:"
        echo "    enox invite <circle> --relay https://$ADVERTISE_HOST"
    fi
    echo ""
    echo "  Logs: journalctl -u $SERVICE_NAME -f"
else
    echo "Error: service failed to start"
    journalctl -u "$SERVICE_NAME" -n 20 --no-pager
    exit 1
fi

if [[ -n "$AUTO_UPDATE" ]]; then
    bash "$(dirname "$0")/setup-relay-updates.sh" "$AUTO_UPDATE" "$SERVICE_NAME" "$PORT"
fi
