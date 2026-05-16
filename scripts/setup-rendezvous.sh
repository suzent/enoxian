#!/usr/bin/env bash
# Run this on the VPS to install enochd as a systemd bootstrap service.
# The enochd binary must already be present in the same directory as this script,
# or at /tmp/enochd (deploy-rendezvous.sh puts it there).
#
# Usage:
#   bash setup-rendezvous.sh [--port PORT]
#
# Defaults:
#   PORT=36521
set -euo pipefail

PORT=36521

while [[ $# -gt 0 ]]; do
    case "$1" in
        --port) PORT="$2"; shift 2 ;;
        *) echo "Unknown argument: $1"; exit 1 ;;
    esac
done

BINARY_SRC="${BINARY_SRC:-/tmp/enochd}"
BINARY_DST="/usr/local/bin/enochd"
SERVICE_FILE="/etc/systemd/system/enochd-bootstrap.service"
SERVICE_USER="enochian"

echo "▶ Setting up enochian rendezvous server on port $PORT"

# ── Install binary ────────────────────────────────────────────────────────────
if [[ ! -f "$BINARY_SRC" ]]; then
    echo "Error: binary not found at $BINARY_SRC"
    echo "Run deploy-rendezvous.sh from your local machine instead."
    exit 1
fi

echo "  Installing binary → $BINARY_DST"
cp "$BINARY_SRC" "$BINARY_DST"
chmod +x "$BINARY_DST"

# ── Create system user ────────────────────────────────────────────────────────
if ! id "$SERVICE_USER" &>/dev/null; then
    echo "  Creating system user '$SERVICE_USER'"
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi

# Create the config directory and give ownership to the service user.
# The bootstrap keypair (~/.enochian/bootstrap.key) lives here.
ENOCHIAN_DIR="/home/$SERVICE_USER/.enochian"
mkdir -p "$ENOCHIAN_DIR"
chown -R "$SERVICE_USER:$SERVICE_USER" "$ENOCHIAN_DIR"

# ── Write systemd service ─────────────────────────────────────────────────────
echo "  Writing $SERVICE_FILE"
cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Enochian Bootstrap Server (rendezvous + relay)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$BINARY_DST --bootstrap --port $PORT
Restart=always
RestartSec=5
User=$SERVICE_USER
Environment=HOME=/home/$SERVICE_USER
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

# ── Firewall ──────────────────────────────────────────────────────────────────
echo "  Opening port $PORT (UDP + TCP)"
if command -v ufw &>/dev/null; then
    ufw allow "$PORT/udp" comment "enochian rendezvous QUIC" 2>/dev/null || true
    ufw allow "$PORT/tcp" comment "enochian rendezvous HTTP" 2>/dev/null || true
elif command -v firewall-cmd &>/dev/null; then
    firewall-cmd --permanent --add-port="$PORT/udp" 2>/dev/null || true
    firewall-cmd --permanent --add-port="$PORT/tcp" 2>/dev/null || true
    firewall-cmd --reload 2>/dev/null || true
else
    echo "  (no ufw/firewalld found — open port $PORT/udp and $PORT/tcp manually)"
fi

# ── Enable and start ──────────────────────────────────────────────────────────
echo "  Enabling and starting service"
systemctl daemon-reload
systemctl enable enochd-bootstrap
systemctl stop enochd-bootstrap 2>/dev/null || true
systemctl reset-failed enochd-bootstrap 2>/dev/null || true
systemctl start enochd-bootstrap

sleep 1
if systemctl is-active --quiet enochd-bootstrap; then
    echo ""
    echo "✦ Rendezvous server running on port $PORT"
    echo ""
    echo "  Peer ID:"
    curl -sf "http://localhost:$PORT/peer-id" | grep -o '"peer_id":"[^"]*"' | cut -d'"' -f4 \
        && echo "" || echo "  (starting up — try again in a moment)"
    echo ""
    echo "  To embed in invites from your local machine:"
    echo "    enoch invite <circle> --rendezvous $(curl -sf https://api4.my-ip.io/ip 2>/dev/null || hostname -I | awk '{print $1}')"
    echo ""
    echo "  Logs: journalctl -u enochd-bootstrap -f"
else
    echo "Error: service failed to start"
    journalctl -u enochd-bootstrap -n 20 --no-pager
    exit 1
fi
