# Three devices in containers

Three enoxian devices (A, B, C) on one host, each in its own container with its
own HOME, plus a local bootstrap server and plain-HTTP Iroh relay, so a
multi-device run needs nothing outside the host: no public relay, no second
machine.

| Service | What it is |
|---------|------------|
| `relay` | `enox bootstrap serve --iroh-relay --iroh-relay-dev-port 3340`: short invites on `relay:36521`, Iroh relay on `http://relay:3340` |
| `dev-a`, `dev-b`, `dev-c` | `enox daemon run`, labelled `A`/`B`/`C` (`ENOXIAN_DEVICE_LABEL`), HOME on volume `home-a`/`-b`/`-c`, `NET_ADMIN` for `tc` |

A separate HOME per device matters: `ENOXIAN_HOME` only moves `~/.enoxian`,
while daemon logs and the default `~/enoxian/<circle>` workspace follow the
home directory.

## Run

From the repository root:

```bash
docker compose -f experiments/devices/compose.yaml build
experiments/devices/smoke.sh
```

`smoke.sh` starts from fresh volumes and checks, failing on the first miss:

1. A runs `enox init`, and its Circle is pointed at the compose relay and
   bootstrap server (`enox init` has no relay flags, so the script edits
   `iroh_relays` and `rendezvous_addrs` in the Circle config).
2. B and C each `enox enter` with their own short invite from A.
3. All three see each other online, each connected to the other two, and the
   route of every connection is printed (`lan <ip:port>` is direct over the
   docker network, `relay http://relay:3340/` is through the relay).
4. `enox watch --json --log` runs on every device, writing
   `runs/<run id>-<a|b|c>.jsonl`.
5. A creates a task, B claims it, C's claim is refused, B finishes it; every
   device shows it done.
6. B and C claim one task at the same moment; every device must agree on one
   holder. When both claims land before either syncs, the loser's log gets a
   `task_claim_lost` line.

Variants:

```bash
FORCE_RELAY_C=1 experiments/devices/smoke.sh          # C uses the relay only
NETEM_C="delay 200ms" experiments/devices/smoke.sh    # slow C's link
```

By hand:

```bash
cd experiments/devices
docker compose up -d
docker compose exec dev-a enox --circle exp who
docker compose exec -e ENOXIAN_AGENT_ID=agent-b dev-b enox --circle exp claim <full task id>
docker compose exec dev-c tc qdisc replace dev eth0 root netem delay 200ms 20ms
docker compose exec dev-c tc qdisc del dev eth0 root
docker compose down -v      # forget every identity and Circle
```

## Notes

- **netem needs the host kernel module.** A container cannot load it; on the
  host run `sudo modprobe sch_netem` once. Stock Ubuntu kernels ship it. If it
  is missing, `tc` prints `Specified qdisc kind is unknown`.
- `tc` on `eth0` shapes what a container sends. Delay on C alone adds that
  delay to every A↔C and B↔C round trip, in one direction.
- `enox claim` and `enox done` need the full task id; `enox tasks` shows eight
  characters. Use `enox --json --circle exp tasks` to get it.
- Set `ENOXIAN_AGENT_ID` per call so claims and run logs name the agent.
  Without it every device claims as `cli` (a claim from another device is
  still refused, but every holder reads `cli`).
- The image has no WebUI and no agent CLIs yet; it is the base to add them to.
