# Relay stable updates

Enable during setup with `--auto-update stable` (Python 3 and systemd required):

```bash
bash setup-rendezvous.sh --advertise-host relay.example.com --auto-update stable
```

Keep `setup-relay-updates.sh` and `update-relay.py` beside the setup script.
The deployment wrappers copy these automatically when given `--auto-update stable`
(PowerShell: `-AutoUpdate stable`). Omit the option to leave the update policy alone;
use `off` to disable the timer.

For an existing relay, install the timer without restarting it:

```bash
# The optional service name supports installations retaining the legacy unit name.
bash setup-relay-updates.sh stable enoxd-bootstrap 36521
```

The timer checks daily at 04:00 UTC plus up to 30 minutes of random delay, and
catches up after downtime. It follows GitHub's latest stable release, which the
release workflow promotes only after its smoke tests. Unchanged versions and
downgrades do not restart the relay. Network/download/verification failures leave
the current installation running. A version upgrade briefly disconnects peers.

Downloads are checked against the release's SHA256SUMS before executing the
staged binary. The executable is replaced atomically; a failed restart, health
check, or changed peer ID restores the previous binary. No peer keys are changed.
The backup stays at `/usr/local/bin/enox.previous`. Updates never run in the
relay startup command, so a download outage cannot prevent startup.

The updater also keeps itself current: each release ships `update-relay.py`,
listed in SHA256SUMS, and every run installs a changed, verified copy over
`/usr/local/sbin/enoxian-relay-update` before doing anything else, then hands
the run to it. `--check` never replaces it.

An updater installed before this existed cannot fetch its replacement, and one
from before 0.9.0 fails on the new `enox --version` format ("Expected a stable
semantic version") before installing anything. Replace it once by hand; the
timer and service stay as they are:

```bash
scp update-relay.py root@relay:/tmp/enoxian-relay-update.new
ssh root@relay 'install -m 755 /tmp/enoxian-relay-update.new /usr/local/sbin/enoxian-relay-update && systemctl start enoxian-relay-update.service'
```

```bash
systemctl list-timers enoxian-relay-update.timer
journalctl -u enoxian-relay-update.service
systemctl start enoxian-relay-update.service  # check/update immediately
python3 /usr/local/sbin/enoxian-relay-update --service enoxd-bootstrap --check
bash setup-relay-updates.sh off             # disable future checks
python3 test-update-relay.py                # isolated updater regression tests
```

Failures appear as a failed systemd unit and in the journal; configure your server
monitoring to alert on `enoxian-relay-update.service` failures. The timer updates
the binary, not its own scripts or unit definitions; rerun setup to update those.
