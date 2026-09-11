#!/usr/bin/env bash
# Configure updates independently of relay startup; usable on existing relays.
set -euo pipefail
CHANNEL="${1:?Usage: setup-relay-updates.sh stable|off [SERVICE] [HTTP_PORT]}"
SERVICE="${2:-enoxian-bootstrap}"
PORT="${3:-36521}"
[[ "$CHANNEL" == stable || "$CHANNEL" == off ]] || { echo 'Expected stable or off'; exit 1; }
[[ "$SERVICE" =~ ^[A-Za-z0-9_-]+$ ]] || exit 1
[[ "$PORT" =~ ^[0-9]+$ ]] && (( PORT >= 1 && PORT <= 65535 )) || exit 1
if [[ "$CHANNEL" == off ]]; then
    systemctl disable --now enoxian-relay-update.timer
    exit 0
fi
command -v python3 >/dev/null
test -x /usr/local/bin/enox
systemctl is-active --quiet "$SERVICE"
systemctl show "$SERVICE" --property=ExecStart --value | grep -q '/usr/local/bin/enox bootstrap serve' || {
    echo 'Upgrade the relay service to /usr/local/bin/enox bootstrap serve before enabling updates.'
    exit 1
}
install -m 755 "$(dirname "$0")/update-relay.py" /usr/local/sbin/enoxian-relay-update
cat > /etc/systemd/system/enoxian-relay-update.service <<EOF
[Unit]
Description=Update Enoxian relay to the latest stable release
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/bin/python3 /usr/local/sbin/enoxian-relay-update --service $SERVICE --port $PORT
TimeoutStartSec=10min
TimeoutStopSec=90s
UMask=0077
EOF
cat > /etc/systemd/system/enoxian-relay-update.timer <<'EOF'
[Unit]
Description=Check for stable Enoxian relay releases daily

[Timer]
OnCalendar=*-*-* 04:00:00 UTC
RandomizedDelaySec=30min
Persistent=true

[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now enoxian-relay-update.timer
echo 'Stable updates enabled: daily at 04:00 UTC, with up to 30 minutes jitter.'
echo 'Logs: journalctl -u enoxian-relay-update.service'
