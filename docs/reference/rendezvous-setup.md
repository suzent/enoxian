# Rendezvous Server Setup

A rendezvous server lets circle members behind NAT find each other without any member having a public IP address. It also acts as a circuit relay — traffic can tunnel through it as a fallback when direct connections fail.

The server holds **no PSK and joins no circle**. It only knows libp2p peer IDs and circle UUIDs. It cannot read any circle content.

---

## Requirements

- A VPS with a public IP and open port `36521` (UDP + TCP)
- SSH access from your local machine
- No local Rust toolchain needed — the deploy script downloads a pre-built binary from GitHub Releases by default

### Recommended locations

| Region | Best choice | Notes |
|--------|-------------|-------|
| UK ↔ China | Singapore or Japan | Singapore for balanced routing; Japan for lower China latency |
| US + Europe | US East / Frankfurt | Standard cloud providers work fine |

Avoid US West Coast for China connectivity — transpacific routing is congested and GFW inspection is heavier on US-origin traffic.

---

## DNS setup (optional but recommended)

A hostname is better than a bare IP for the rendezvous address — if you ever move the server or change providers, you update the DNS record and existing configs keep working without regenerating invites.

### 1. Add an A record

In your DNS provider's control panel, add:

| Type | Name | Value | TTL |
|------|------|-------|-----|
| A | `enox` | `12.34.56.78` | 300 |

This creates `enox.yourdomain.com → 12.34.56.78`. Use a short TTL (300s = 5 min) so changes propagate quickly if you ever need to move the server.

### 2. Verify propagation

```bash
# Should return your VPS IP
nslookup enox.yourdomain.com

# Or
dig +short enox.yourdomain.com
```

### 3. Use the hostname in invites

```bash
enox invite <circle> --rendezvous enox.yourdomain.com
```

The CLI resolves it to `/dns4/enox.yourdomain.com/udp/36521/quic-v1/p2p/<id>` and stores the hostname (not the IP) in config, so DNS changes are picked up automatically on the next daemon restart.

---

## Deployment (one command)

The deploy script downloads the latest pre-built binary from GitHub Releases and installs it — no build tools needed anywhere.

**macOS / Linux:**
```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --advertise-host enox.yourdomain.com
```

**Windows (PowerShell):**
```powershell
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -AdvertiseHost enox.yourdomain.com
```

This will:
1. Download the matching `enoxian-linux-<arch>.tar.gz` release and extract `enox`
2. Create an `enoxian` system user
3. Install a systemd service (`enoxian-bootstrap`)
4. Open port `36521` on ufw/firewalld
5. Start the service and print the server address

Output at the end:

```
✦ Rendezvous server running on port 36521

  Peer ID: 12D3KooWrdv...

  To embed in invites from your local machine:
    enox invite <circle> --rendezvous 12.34.56.78
```

### Custom port

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --port 36521 --relay-port 36522 --advertise-host enox.yourdomain.com
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -Port 36521 -RelayPort 36522 -AdvertiseHost enox.yourdomain.com
```

### Updating after a code change

Tag a new release to trigger the build:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

GitHub Actions builds the binaries automatically. Once the release is published, deploy:

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --update --advertise-host enox.yourdomain.com
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -Update -AdvertiseHost enox.yourdomain.com
```

### Building manually (no release tag)

If you need to deploy unreleased code, build inside Docker on the VPS:

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --build-on-remote
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -BuildOnRemote
```

Requires Docker on the VPS.

You can also cross-compile locally and upload the result:

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --local
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -Local
```

---

## Using the server

Once deployed, pass the hostname or IP to `enox invite` or `enox enter` — the peer ID is resolved automatically:

```bash
# Embed rendezvous in an invite
enox invite <circle> --rendezvous 12.34.56.78

# Or use a hostname
enox invite <circle> --rendezvous enox.yourdomain.com

# Join a circle that has a rendezvous embedded in the invite
enox enter <invite>

# Override the rendezvous server when joining
enox enter <invite> --rendezvous enox.yourdomain.com
```

The CLI calls `GET http://<host>:36521/peer-id`, gets the peer ID, and constructs the full multiaddr automatically. **After the first member joins, the rendezvous address is saved in their config and auto-embedded in every invite they generate** — no one else needs to type it.

---

## Manual setup (without the script)

If you prefer to set up manually or are not using systemd:

### 1. Copy the binary

```bash
cargo build --release --bin enox
scp target/release/enox user@your-vps:/usr/local/bin/enox
```

### 2. Run directly

```bash
enox bootstrap serve --port 36521 --relay-port 36522 --advertise-host enox.yourdomain.com
```

The server generates a stable Ed25519 keypair at `~/.enoxian/bootstrap.key` on first run. The peer ID is stable across restarts — **do not delete this file**.

Startup output:

```
Bootstrap server starting
  PeerID : 12D3KooWrdv...
  HTTP   : http://0.0.0.0:36521/peer-id
  Relay  : tcp/0.0.0.0:36522
Bootstrap listening on /ip4/0.0.0.0/udp/36521/quic-v1
  Rendezvous address for circle members:
    /ip4/0.0.0.0/udp/36521/quic-v1/p2p/12D3KooWrdv...
Bootstrap listening on /ip4/0.0.0.0/tcp/36522
  Relay address for circle members:
    /ip4/0.0.0.0/tcp/36522/p2p/12D3KooWrdv...
```

### 3. Systemd service (manual)

```ini
# /etc/systemd/system/enoxian-bootstrap.service
[Unit]
Description=enoxian Bootstrap Server (rendezvous + relay)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/enox bootstrap serve --port 36521 --relay-port 36522 --advertise-host enox.yourdomain.com
Restart=always
RestartSec=5
User=enoxian
Environment=HOME=/home/enoxian
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now enoxian-bootstrap
```

### 4. Firewall

Open `36521/udp` for QUIC rendezvous, `36521/tcp` for the HTTP peer-id
endpoint, and `36522/tcp` for circuit relay:

```bash
# ufw
sudo ufw allow 36521/udp
sudo ufw allow 36521/tcp
sudo ufw allow 36522/tcp

# firewalld
sudo firewall-cmd --permanent --add-port=36521/udp
sudo firewall-cmd --permanent --add-port=36521/tcp
sudo firewall-cmd --permanent --add-port=36522/tcp
sudo firewall-cmd --reload
```

---

## Verifying the server

```bash
# From anywhere — check the server is reachable and get its peer ID
curl http://your-vps:36521/peer-id
# {"peer_id":"12D3KooWrdv..."}

# Check service status on the VPS
systemctl status enoxian-bootstrap

# Live logs
journalctl -u enoxian-bootstrap -f
```

---

## How it works

1. Member A starts their daemon — it dials the rendezvous server over QUIC and registers under their circle UUID namespace (TTL = 2h, refreshed every hour).
2. Member B joins via an invite with the rendezvous address embedded, dials the server, and discovers Member A's peer ID and addresses.
3. Member B dials Member A directly over PSK-TCP. If direct connection succeeds, the rendezvous server is no longer in the data path.
4. If direct connection fails (strict NAT on both sides), traffic routes through the relay built into the bootstrap server on TCP `36522`.

The bootstrap server is used for discovery and, when direct dialing fails, as a
circuit relay fallback. When a direct path exists, ongoing sync traffic flows
directly between members; otherwise the relay forwards Noise-protected circuit
traffic without joining the circle.

---

## Security

| Property | Status |
|----------|--------|
| Knows circle content | No |
| Knows circle member peer IDs | Yes (registration) |
| Knows circle UUID | Yes (used as namespace) |
| Can impersonate members | No (Noise peer identity) |
| Traffic encrypted end-to-end | Yes (Noise, relay forwards ciphertext) |
