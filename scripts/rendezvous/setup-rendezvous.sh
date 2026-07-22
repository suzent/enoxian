#!/usr/bin/env bash
# Run this on the VPS to install enoxd as a systemd bootstrap service.
# The enoxd binary must already be present in the same directory as this script,
# or at /tmp/enoxd (deploy-rendezvous.sh puts it there).
#
# Usage:
#   bash setup-rendezvous.sh [--port PORT] [--relay-port PORT] [--advertise-host HOST]
#
# Defaults:
#   PORT=36521
#   RELAY_PORT=PORT+1
set -euo pipefail

PORT=36521
RELAY_PORT=""
ADVERTISE_HOST=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port) PORT="$2"; shift 2 ;;
        --relay-port) RELAY_PORT="$2"; shift 2 ;;
        --advertise-host) ADVERTISE_HOST="$2"; shift 2 ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

if [[ -z "$RELAY_PORT" ]]; then
    RELAY_PORT=$((PORT + 1))
fi

if [[ -n "$ADVERTISE_HOST" && ! "$ADVERTISE_HOST" =~ ^[A-Za-z0-9.-]+$ ]]; then
    echo "Invalid --advertise-host: $ADVERTISE_HOST"
    exit 1
fi

BINARY_SRC="${BINARY_SRC:-/tmp/enoxd}"
BINARY_DST="/usr/local/bin/enoxd"
SERVICE_FILE="/etc/systemd/system/enoxd-bootstrap.service"
SERVICE_USER="enoxian"

echo "Setting up enoxian rendezvous server on port $PORT and relay port $RELAY_PORT"

ADVERTISE_ARGS=""
if [[ -n "$ADVERTISE_HOST" ]]; then
    ADVERTISE_ARGS=" --advertise-host $ADVERTISE_HOST"
    echo "  Advertising public hostname $ADVERTISE_HOST"
fi

# Install binary
if [[ ! -f "$BINARY_SRC" ]]; then
    echo "Error: binary not found at $BINARY_SRC"
    echo "Run deploy-rendezvous.sh from your local machine instead."
    exit 1
fi

systemctl stop enoxd-bootstrap 2>/dev/null || true

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
Description=enoxian Bootstrap Server (rendezvous + relay)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$BINARY_DST --bootstrap --port $PORT --relay-port $RELAY_PORT$ADVERTISE_ARGS
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
echo "  Opening port $PORT (UDP + TCP) and $RELAY_PORT/tcp"
if command -v ufw &>/dev/null; then
    ufw allow "$PORT/udp" comment "enoxian rendezvous QUIC" 2>/dev/null || true
    ufw allow "$PORT/tcp" comment "enoxian rendezvous HTTP" 2>/dev/null || true
    ufw allow "$RELAY_PORT/tcp" comment "enoxian circuit relay" 2>/dev/null || true
elif command -v firewall-cmd &>/dev/null; then
    firewall-cmd --permanent --add-port="$PORT/udp" 2>/dev/null || true
    firewall-cmd --permanent --add-port="$PORT/tcp" 2>/dev/null || true
    firewall-cmd --permanent --add-port="$RELAY_PORT/tcp" 2>/dev/null || true
    firewall-cmd --reload 2>/dev/null || true
else
    echo "  (no ufw/firewalld found - open $PORT/udp, $PORT/tcp, and $RELAY_PORT/tcp manually)"
fi

# Enable and start
echo "  Enabling and starting service"
systemctl daemon-reload
systemctl enable enoxd-bootstrap
systemctl reset-failed enoxd-bootstrap 2>/dev/null || true
systemctl start enoxd-bootstrap

sleep 1
if systemctl is-active --quiet enoxd-bootstrap; then
    echo ""
    echo "Rendezvous server running on port $PORT; relay on TCP $RELAY_PORT"
    echo ""
    echo "  Peer ID:"
    curl -sf "http://localhost:$PORT/peer-id" | grep -o '"peer_id":"[^"]*"' | cut -d'"' -f4 \
        && echo "" || echo "  (starting up 鈥?try again in a moment)"
    echo ""
    echo "  To embed in invites from your local machine:"
    echo "    enox invite <circle> --rendezvous $(curl -sf https://api4.my-ip.io/ip 2>/dev/null || hostname -I | awk '{print $1}')"
    echo ""
    echo "  Logs: journalctl -u enoxd-bootstrap -f"
else
    echo "Error: service failed to start"
    journalctl -u enoxd-bootstrap -n 20 --no-pager
    exit 1
fi
