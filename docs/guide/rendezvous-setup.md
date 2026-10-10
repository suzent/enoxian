# Rendezvous Server Setup

> A Circle with no relays or rendezvous server of its own falls back to the
> project-operated defaults: `relay.enoxian.com` plus Iroh's public relays.
> Running your own server as described here, and pointing Circles at it,
> replaces that default. See
> [privacy and security](../concepts/security.md#relay-and-rendezvous).

Every Circle connects over Iroh. Devices find each other by public key through
an Iroh relay, and traffic goes through that relay until Iroh opens a direct
path. On networks that never allow a direct path (a mobile carrier's symmetric
NAT, for example) traffic stays on the relay.

`enox bootstrap serve` runs two things on one server:

- **HTTP endpoints** on `36521/tcp`: `/version`, `/peer-id`, `/pair` (the dead
  drop `enox link` uses) and `/invite` (sealed short invites). This is the
  "rendezvous server" in Circle config (`rendezvous_addrs`, `--rendezvous`).
  It hosts short invite links, device linking and the upgrade check. It does
  not do peer discovery.
- **An Iroh relay** (with `--iroh-relay`): HTTPS on `443/tcp`, HTTP on
  `80/tcp`, QUIC address discovery on `7842/udp`.

The server holds **no PSK and joins no Circle**. It cannot read Circle content:
the relay forwards end-to-end encrypted QUIC traffic, and short invites are
sealed before they are uploaded.

0.11 and earlier ran a libp2p rendezvous and circuit relay here instead. See
[Upgrading a 0.11 server](#upgrading-a-011-server).

---

## Requirements

- A VPS with a public IP
- A DNS name pointing at it. The relay's TLS certificate is issued for this
  name, so the relay cannot run on a bare IP.
- These ports open, and not used by anything else:

| Port | Used for |
|------|----------|
| `36521/tcp` | HTTP endpoints (`--port`) |
| `443/tcp` | Iroh relay over HTTPS, and the Let's Encrypt TLS-ALPN challenge |
| `80/tcp` | Iroh's captive-portal probe |
| `7842/udp` | QUIC address discovery |

- SSH access from your local machine
- No local Rust toolchain. The deploy script downloads a pre-built binary from
  GitHub Releases by default. Linux release binaries and the Docker image
  include the relay (feature `iroh-relay-server`).

### Recommended locations

| Region | Best choice | Notes |
|--------|-------------|-------|
| UK ↔ China | Singapore or Japan | Singapore for balanced routing; Japan for lower China latency |
| US + Europe | US East / Frankfurt | Standard cloud providers work fine |

Avoid US West Coast for China connectivity — transpacific routing is congested and GFW inspection is heavier on US-origin traffic.

---

## DNS setup

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

Wait until it resolves before starting the relay: Let's Encrypt checks the
name when the server first asks for a certificate.

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
3. Install a systemd service (`enoxian-bootstrap`) running
   `enox bootstrap serve --port 36521 --advertise-host <host> --iroh-relay`,
   with `CAP_NET_BIND_SERVICE` so it can bind 80 and 443
4. Open `36521/tcp`, `80/tcp`, `443/tcp` and `7842/udp` on ufw/firewalld
5. Start the service and print its peer ID

Without `--advertise-host` the server runs the HTTP endpoints only, with no
relay, and only `36521/tcp` is opened.

Output at the end:

```
Bootstrap server running on port 36521

  Peer ID:
12D3KooWrdv...

  To put this relay in invites from your local machine:
    enox invite <circle> --relay https://enox.yourdomain.com
```

On the VPS itself, `setup-rendezvous.sh` does the same with the binary at
`/tmp/enox` (or `BINARY_SRC`):

```bash
bash setup-rendezvous.sh [--port PORT] [--advertise-host HOST] [--auto-update stable|off]
```

### Custom port

`--port` moves the HTTP endpoints. The relay's ports are fixed.

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --port 36521 --advertise-host enox.yourdomain.com
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -Port 36521 -AdvertiseHost enox.yourdomain.com
```

### Automatic updates

Add `--auto-update stable` (PowerShell: `-AutoUpdate stable`) to install a
daily systemd timer that follows the latest stable release. It checks the
download against the release's checksums, restarts the service, and rolls back
if the service fails its health check or its peer ID changes. `off` disables
the timer. See [scripts/rendezvous/README.md](../../scripts/rendezvous/README.md).

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --advertise-host enox.yourdomain.com --auto-update stable
```

The timer replaces the binary only. Rerun setup to change the service file.

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

With `--advertise-host`, `--update` reruns the full setup and rewrites the
service file. Without it, `--update` only replaces the binary and restarts the
service.

### Building manually (no release tag)

If you need to deploy unreleased code, build inside Docker on the VPS:

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --build-on-remote --advertise-host enox.yourdomain.com
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -BuildOnRemote -AdvertiseHost enox.yourdomain.com
```

Requires Docker on the VPS.

You can also cross-compile locally and upload the result:

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --local --advertise-host enox.yourdomain.com
.\scripts\rendezvous\deploy-rendezvous.ps1 user@your-vps -Local -AdvertiseHost enox.yourdomain.com
```

Both build with `--features iroh-relay-server`.

---

## Docker

The repository's `Dockerfile` builds an image with the relay compiled in. Mount
a volume at `/root/.enoxian` to keep the server's key and certificates across
restarts.

```bash
docker build -t enoxian-bootstrap .

# HTTP endpoints only
docker run -p 36521:36521/tcp -v enoxian-bootstrap:/root/.enoxian enoxian-bootstrap

# With the Iroh relay
docker run -p 36521:36521/tcp -p 80:80/tcp -p 443:443/tcp -p 7842:7842/udp \
    -v enoxian-bootstrap:/root/.enoxian \
    enoxian-bootstrap --port 36521 --advertise-host enox.yourdomain.com --iroh-relay
```

---

## Using the server

### Put your relay in invites

```bash
enox invite <circle> --relay https://enox.yourdomain.com
```

The invite carries the relay URL. `enox enter` saves it as the joiner's
`iroh_relays`, and every invite the joiner makes carries it on. Devices with
`iroh_relays` set use only those relays, not enoxian's or Iroh's public ones.

`--relay` changes the invite, not your own device. To move an existing member
onto your relay, set it in that device's Circle config
(`~/.enoxian/circles/<circle-id>/config.toml`) and restart the daemon:

```toml
iroh_relays = ["https://enox.yourdomain.com"]
```

Every member should use the same relays, so that each device's home relay is
one the others can reach.

### Use it for short invites and linking

The HTTP side is selected separately, by host name or IP:

```bash
# Short invite links, and the joiner's upgrade check, use this server
enox invite <circle> --rendezvous enox.yourdomain.com

# Override the rendezvous server when joining
enox enter <invite> --rendezvous enox.yourdomain.com
```

The CLI calls `GET http://<host>:36521/peer-id` to check the server. After a
member joins, the server is saved in their config (`rendezvous_addrs`) and
named in every invite they make. A Circle's running daemon asks it for
`/version` at start and every six hours. To pair devices through your server,
run `enox link --server enox.yourdomain.com` on both devices.

---

## Manual setup (without the script)

If you prefer to set up manually or are not using systemd:

### 1. Copy the binary

Use a Linux release binary, or build one with the relay:

```bash
cargo build --release --bin enox --features iroh-relay-server
scp target/release/enox user@your-vps:/usr/local/bin/enox
```

### 2. Run directly

```bash
enox bootstrap serve --port 36521 --advertise-host enox.yourdomain.com \
    --iroh-relay --acme-contact you@example.com --acme-staging
```

- **Certificates:** issued for `--advertise-host` and cached in
  `~/.enoxian/acme`. `--acme-staging` uses Let's Encrypt's staging directory,
  whose certificates are not trusted but which has no production rate limits.
  Start with it, then drop it once the relay is reachable.
- **Key:** the server generates a stable Ed25519 keypair at
  `~/.enoxian/bootstrap.key` on first run. The peer ID is stable across
  restarts, and the updater checks it — **do not delete this file**.
- **Privileged ports:** binding 80 and 443 needs root or
  `CAP_NET_BIND_SERVICE`.
- **Local testing:** `--iroh-relay-dev-port <port>` serves the relay as plain
  HTTP on that port, with no certificate.

If the relay cannot start (no `--advertise-host`, a port in use, or a binary
built without `iroh-relay-server`), the server exits instead of running
without it.

Startup output:

```
Bootstrap server starting
  PeerID : 12D3KooWrdv...
  HTTP   : http://0.0.0.0:36521/version
```

### 3. Systemd service (manual)

```ini
# /etc/systemd/system/enoxian-bootstrap.service
[Unit]
Description=enoxian Bootstrap Server (HTTP + Iroh relay)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/enox bootstrap serve --port 36521 --advertise-host enox.yourdomain.com --iroh-relay
# The Iroh relay listens on 80 and 443.
AmbientCapabilities=CAP_NET_BIND_SERVICE
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

```bash
# ufw
sudo ufw allow 36521/tcp
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw allow 7842/udp

# firewalld
sudo firewall-cmd --permanent --add-port=36521/tcp
sudo firewall-cmd --permanent --add-port=80/tcp
sudo firewall-cmd --permanent --add-port=443/tcp
sudo firewall-cmd --permanent --add-port=7842/udp
sudo firewall-cmd --reload
```

---

## Verifying the server

```bash
# From anywhere — check the server is reachable and get its peer ID
curl http://your-vps:36521/peer-id
# {"peer_id":"12D3KooWrdv..."}

# Version, capabilities, and the relay it runs
curl http://your-vps:36521/version
# {"version":"<package-version>","min_client_version":null,"iroh_relay":"https://enox.yourdomain.com","capabilities":{"short_invites":true,"device_linking":true}}

# The relay answers over HTTPS with a trusted certificate
curl -I https://enox.yourdomain.com

# Check service status on the VPS
systemctl status enoxian-bootstrap

# Live logs
journalctl -u enoxian-bootstrap -f
```

`iroh_relay` is `null` when the server runs no relay.

On a member device, Settings → Connectivity lists the relays its Circle uses,
and `GET /circles/<id>/api/status` reports them under `p2p.relays`, with the
one it is homed on as `p2p.home_relay`.

---

`GET /version` is public and read-only. It reports the running binary's package
version, not a release lookup or the version of a binary replaced on disk.
Responses use `Cache-Control: no-store` so probes can observe a restarted server.
Check `capabilities.short_invites` for sealed invite storage and
`capabilities.device_linking` for the pairing mailbox instead of guessing from
version numbers. These flags describe supported endpoints, not current storage
capacity or successful end-to-end P2P connectivity. Older relays return 404 for
this endpoint; treat that as unknown capability and retain full-invite fallback.
An upgraded relay must be restarted before the endpoint becomes available.

`min_client_version` is the oldest client this server still serves, or `null`
while every client release is. A running Circle asks its rendezvous servers at
start and every six hours; a client older than the minimum reports
`upgrade_required` in its status and asks its user to run `enox update`. It is
set at build time (`MIN_CLIENT_VERSION` in `src/defaults.rs`) and raised only by
a release that old clients can no longer connect to.

---

## Upgrading a 0.11 server

0.12 clients cannot talk to 0.11 clients, and every device in a Circle must
upgrade. The server changes too:

- It no longer listens on `36521/udp` (QUIC rendezvous) or `36522/tcp`
  (circuit relay). Close them.
- `--relay-port` is accepted and ignored, so an old service file still starts.
  The auto-updater can therefore move a 0.11 server to 0.12 without breaking
  it, but the server then runs the HTTP endpoints only.
- To run the relay, add `--iroh-relay` (with `--advertise-host`), grant
  `CAP_NET_BIND_SERVICE`, and open `80/tcp`, `443/tcp` and `7842/udp`.

The simplest way is to rerun setup with a host name, which rewrites the service
file and the firewall rules:

```bash
./scripts/rendezvous/deploy-rendezvous.sh user@your-vps --update --advertise-host enox.yourdomain.com
```

Then close the old ports:

```bash
sudo ufw delete allow 36521/udp
sudo ufw delete allow 36522/tcp
```

The server keeps its `bootstrap.key`, so its peer ID does not change.

---

## Security

| Property | Status |
|----------|--------|
| Knows Circle content | No |
| Holds a Circle PSK | No |
| Sees member public keys and IP addresses | Yes (relay) |
| Can impersonate members | No (TLS identity is the member's key) |
| Can join a Circle | No (every stream needs a proof derived from the PSK) |
| Traffic encrypted end-to-end | Yes (QUIC, relay forwards ciphertext) |
