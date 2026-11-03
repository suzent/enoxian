#!/usr/bin/env python3
"""Run one task instance under one condition on the three-device containers.

    python harness.py run --instance DIR --condition M2C2 --seed 0 \
        --synth ~/SYNCagent/eval/synthetic --runs runs [--model gemini/gemini-3.5-flash]

One run, start to finish:

1. Fresh devices (`docker compose down -v`, then up), each with the
   mini-swe-agent key copied from this host, the `stakeholder` command, and
   an acp-mini agent: @alice on A, @bob on B, @carol on C.
2. A bare git repo per run under the run directory, mounted at /git, with a
   pre-receive hook that enforces `meta.owners`. Each device clones it.
3. Circles: the team conditions put A, B and C in one Circle; Silent gives
   each device a Circle of its own and a repo of its own; Ref-Full and
   Ref-Split use device A alone.
4. Each agent is started with `enox inbox inject`: its brief, its notes, and
   the condition's coordination text (prompts.py).
5. The harness watches `main`. When the instance's trigger holds (or after
   `fallback.after_turns` turns, or when every agent has gone idle first), it
   injects the change into the entry device's agent only.
6. The run ends once every agent is idle after the change, or a budget limit
   is hit. The final `main` (Silent: each owner's package from its own repo)
   is graded by SYNCagent's `synth.py grade` in a throwaway container with no
   network.

Writes to runs/<experiment>/<model>/<condition>/<instance>/<seed>/:
events.jsonl, final.bundle, result.json, env.json, and devices/ (raw logs).
"""

from __future__ import annotations

import argparse
import datetime as dt
import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
from pathlib import Path

import yaml

import prompts

HERE = Path(__file__).resolve().parent
DEVICES_DIR = HERE.parent / "devices"
COMPOSE = ["docker", "compose", "-f", str(DEVICES_DIR / "compose.yaml")]
RELAY_URL = "http://relay:3340"
RENDEZVOUS = "/dns4/relay/tcp/36521"
STAKEHOLDER_PORT = 36600
HOME = "/home/device"
REPO = f"{HOME}/repo"
LEDGER = "interfaces.md"

CONDITIONS = {
    "M1C1": {"kind": "team", "comm": "M1", "org": "C1"},
    "M1C2": {"kind": "team", "comm": "M1", "org": "C2"},
    "M2C1": {"kind": "team", "comm": "M2", "org": "C1"},
    "M2C2": {"kind": "team", "comm": "M2", "org": "C2"},
    "Silent": {"kind": "silent"},
    "Ref-Full": {"kind": "single", "brief": "merged"},
    "Ref-Split": {"kind": "single", "brief": "split"},
}


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


class Instance:
    def __init__(self, path: Path):
        self.path = path
        self.meta = json.loads((path / "meta.json").read_text())
        self.change = json.loads((path / "hidden" / "change.json").read_text())

    def read(self, rel: str) -> str:
        return (self.path / rel).read_text()

    def stakeholder(self, device: str) -> dict:
        return yaml.safe_load(self.read(f"stakeholders/{device}.yaml"))


# --------------------------------------------------------------------------
# containers


class Devices:
    """The compose project: devices a/b/c and the relay."""

    def __init__(self, env: dict):
        self.env = dict(os.environ, **env)

    def compose(self, *args: str, check: bool = True, **kw) -> subprocess.CompletedProcess:
        return subprocess.run([*COMPOSE, *args], env=self.env, check=check, text=True, capture_output=True, **kw)

    def on(self, dev: str, *cmd: str, check: bool = True, env: dict | None = None) -> str:
        flags = [x for k, v in (env or {}).items() for x in ("-e", f"{k}={v}")]
        result = self.compose("exec", "-T", *flags, f"dev-{dev.lower()}", *cmd, check=False)
        if check and result.returncode != 0:
            raise RuntimeError(f"dev-{dev.lower()}: {' '.join(cmd)[:200]} failed:\n{result.stdout}{result.stderr}")
        return result.stdout

    def sh(self, dev: str, script: str, check: bool = True) -> str:
        return self.on(dev, "sh", "-c", script, check=check)

    def detach(self, dev: str, script: str) -> None:
        self.compose("exec", "-d", f"dev-{dev.lower()}", "sh", "-c", script)

    def cp_in(self, dev: str, src: Path, dst: str, mode: str = "644") -> None:
        cid = self.compose("ps", "-q", f"dev-{dev.lower()}").stdout.strip()
        self.sh(dev, f"mkdir -p '{os.path.dirname(dst)}'")
        subprocess.run(["docker", "cp", str(src), f"{cid}:{dst}"], check=True, capture_output=True)
        self.sh(dev, f"chmod {mode} '{dst}'")

    def cp_out(self, dev: str, src: str, dst: Path) -> bool:
        cid = self.compose("ps", "-q", f"dev-{dev.lower()}").stdout.strip()
        return subprocess.run(["docker", "cp", f"{cid}:{src}", str(dst)], capture_output=True).returncode == 0

    def api(self, dev: str, circle_id: str, path: str):
        out = self.sh(dev, f'curl -sf -H "Authorization: Bearer $(cat ~/.enoxian/api.token)" '
                           f'localhost:36521/circles/{circle_id}/api/{path}', check=False)
        try:
            return json.loads(out)
        except json.JSONDecodeError:
            return None

    def wait(self, what: str, secs: float, fn) -> None:
        deadline = time.time() + secs
        while time.time() < deadline:
            try:
                if fn():
                    return
            except Exception:
                pass
            time.sleep(1)
        raise RuntimeError(f"timed out waiting for {what}")


# --------------------------------------------------------------------------
# the stakeholder: answers from the instance's rules, on the host


class Stakeholders:
    def __init__(self, instance: Instance, single: bool, events: "Events"):
        self.instance = instance
        self.single = single
        self.events = events
        self.changed = False
        self.server = None

    def rules(self, device: str) -> tuple[list[dict], str]:
        devices = sorted(self.instance.meta["owners"]) if self.single else [device]
        entry = self.instance.meta["entry_device"]
        rules, fallback = [], "I don't know."
        for d in devices:
            spec = self.instance.stakeholder(d)
            fallback = spec.get("fallback", fallback)
            if self.changed and (self.single or d == entry) and spec.get("after_change"):
                rules = spec["after_change"]["rules"] + rules
            rules += spec.get("rules", [])
        return rules, fallback

    def answer(self, device: str, question: str) -> str:
        rules, fallback = self.rules(device)
        q = question.lower()
        match = next((r for r in rules if any(k.lower() in q for k in r["keywords"])), None)
        answer = match["answer"] if match else fallback
        self.events.add("stakeholder_qa", device=device, payload={
            "question": question, "answer": answer,
            "matched_rule": (match.get("fact") or ",".join(match["keywords"])) if match else None,
        })
        return answer

    def serve(self) -> None:
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
                form = urllib.parse.parse_qs(body)
                device = form.get("device", ["?"])[0]
                answer = owner.answer(device, form.get("question", [""])[0])
                data = answer.encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/plain; charset=utf-8")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("0.0.0.0", STAKEHOLDER_PORT), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def stop(self) -> None:
        if self.server:
            self.server.shutdown()


class Events:
    """Events only the harness sees (change injection, stakeholder Q&A, ...)."""

    def __init__(self, run_id: str):
        self.run_id = run_id
        self.lines: list[dict] = []
        self.lock = threading.Lock()

    def add(self, kind: str, device: str = "harness", payload: dict | None = None) -> None:
        with self.lock:
            self.lines.append({"ts": now(), "run_id": self.run_id, "device": device,
                               "kind": kind, "payload": payload or {}})


# --------------------------------------------------------------------------
# one run


class Run:
    def __init__(self, args, instance: Instance, condition: str):
        self.args = args
        self.instance = instance
        self.name = condition
        self.condition = CONDITIONS[condition]
        self.kind = self.condition["kind"]
        model_dir = args.model.split("/")[-1]
        self.run_id = f"{args.experiment}/{model_dir}/{condition}/{instance.meta['instance_id']}/{args.seed}"
        self.dir = (args.runs / args.experiment / model_dir / condition / instance.meta["instance_id"] / str(args.seed)).resolve()
        self.events = Events(self.run_id)
        self.devices_used = ["A"] if self.kind == "single" else ["A", "B", "C"]
        self.circles: dict[str, str] = {}
        self.ledgers: dict[str, str] = {}
        self.names: dict[str, str] = {}
        self.change_at: float | None = None
        self.started = time.time()

    # -- setup

    def setup(self) -> None:
        if self.dir.exists():
            shutil.rmtree(self.dir)
        (self.dir / "git").mkdir(parents=True)
        (self.dir / "mnt").mkdir()  # the compose file's /runs mount; nothing goes in it
        (self.dir / "devices").mkdir()
        self.git_setup()
        # Each device sees only its own integration repo at /git: one shared
        # repo for the team and single conditions, one per device for Silent.
        mounts = {f"RUN_GIT_DIR_{d}": str(self.dir / "git" / (d if self.kind == "silent" else ""))
                  for d in ("A", "B", "C")}
        self.dc = Devices({**mounts, "RUNS_DIR": str(self.dir / "mnt")})

        log(f"{self.run_id}: fresh devices")
        self.dc.compose("down", "-v", "--remove-orphans", check=False)
        self.dc.compose("up", "-d")
        for d in self.devices_used:
            self.dc.wait(f"dev-{d.lower()} daemon", 90, lambda d=d: self.dc.compose(
                "exec", "-T", f"dev-{d.lower()}", "enox", "circles", check=False).returncode == 0)

        if self.kind == "team":
            self.team_circle()
        else:
            for d in self.devices_used:
                self.names[d] = f"solo-{d.lower()}"
                self.circles[d] = self.init_circle(d, self.names[d], relay=False)
        for d in self.devices_used:
            self.device_setup(d)

    def git_setup(self) -> None:
        owners = self.instance.meta["owners"]
        repos = {d: "origin.git" for d in self.devices_used}
        if self.kind == "silent":
            repos = {d: f"{d}/origin.git" for d in self.devices_used}
        for d, rel in repos.items():
            bare = self.dir / "git" / rel
            if bare.exists():
                continue
            bare.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["git", "clone", "-q", "--bare", str(self.instance.path / "repo.bundle"), str(bare)], check=True)
            subprocess.run(["git", "--git-dir", str(bare), "branch", "-M", "main"], check=False, capture_output=True)
            shutil.copy(HERE / "pre-receive", bare / "hooks" / "pre-receive")
            (bare / "hooks" / "pre-receive").chmod(0o755)
            allowed = {"A": "*"} if self.kind == "single" else ({d: owners[d]} if self.kind == "silent" else owners)
            (bare.parent / "owners.json").write_text(json.dumps(allowed))
        self.repos = repos

    def repo_rel(self, d: str) -> str:
        """Device d's integration repo, relative to the run's git directory."""
        return self.repos[d]

    def init_circle(self, d: str, name: str, relay: bool) -> str:
        out = self.dc.on(d, "enox", "init", "--name", name)
        circle_id = next(line.split()[-1] for line in out.splitlines() if "circle-id" in line)
        self.dc.wait(f"circle {name} loaded", 60, lambda: self.dc.api(d, circle_id, "status") is not None)
        if relay:
            self.dc.sh(d, f"sed -i -e 's#^iroh_relays = .*#iroh_relays = [\"{RELAY_URL}\"]#' "
                          f"-e 's#^rendezvous_addrs = .*#rendezvous_addrs = [\"{RENDEZVOUS}\"]#' "
                          f"~/.enoxian/circles/{circle_id}/config.toml")
            self.dc.compose("restart", f"dev-{d.lower()}")
            self.dc.wait("restart", 60, lambda: self.dc.api(d, circle_id, "status") is not None)
        return circle_id

    def team_circle(self) -> None:
        log("A creates the Circle, B and C join")
        cid = self.init_circle("A", "exp", relay=True)
        self.dc.wait("A's direct address", 60, lambda: "/ip4/" in self.dc.on("A", "enox", "invite", "exp"))
        for d in ("B", "C"):
            invite = next(w for w in self.dc.on("A", "enox", "invite", "exp").split() if w.startswith("enoxian://"))
            self.dc.on(d, "enox", "enter", invite)
        self.circles = {d: cid for d in ("A", "B", "C")}
        self.names = {d: "exp" for d in ("A", "B", "C")}

        def meshed():
            for d in ("A", "B", "C"):
                who = self.dc.api(d, cid, "who") or []
                if sum(1 for w in who if w.get("status") == "online") < 3:
                    return False
                if sum(1 for w in who if w.get("connections")) < 2:
                    return False
            return True
        self.dc.wait("three devices connected", 180, meshed)
        if self.args.netem_c:
            self.dc.on("C", "tc", "qdisc", "replace", "dev", "eth0", "root", "netem", *self.args.netem_c.split())

    def device_setup(self, d: str) -> None:
        name = prompts.AGENTS[d]
        cid = self.circles[d]
        self.dc.cp_in(d, self.args.key_file, f"{HOME}/.config/mini-swe-agent/.env", mode="600")
        self.dc.cp_in(d, HERE / "stakeholder", "/usr/local/bin/stakeholder", mode="755")
        system = prompts.SYSTEM_TEAM if self.kind == "team" else prompts.SYSTEM_ALONE
        cfg = Path(tempfile.mkstemp(suffix=".yaml")[1])
        cfg.write_text(yaml.safe_dump({"agent": {"system_template": system}}))
        self.dc.cp_in(d, cfg, f"{HOME}/acp-mini.yaml")
        cfg.unlink()

        self.dc.sh(d, f"git config --global --add safe.directory '*' && "
                      f"printf '__pycache__/\\n*.pyc\\n.pytest_cache/\\n' > {HOME}/.gitignore && "
                      f"git config --global core.excludesFile {HOME}/.gitignore && "
                      f"git config --global user.name {name} && git config --global user.email {name}@device-{d.lower()} && "
                      f"git clone -q /git/origin.git {REPO}")
        self.dc.on(d, "enox", "agent", "add", name, "--driver", "acp", "--working-dir", REPO, "--",
                   "/opt/acp-mini/venv/bin/python", "/opt/acp-mini/acp_mini.py", "--model", self.args.model,
                   "--config", f"{HOME}/acp-mini.yaml", "--turn-steps", str(self.args.turn_steps))
        self.dc.on(d, "sh", "-c", "yes | enox agent reaction push")
        # Follow-ups without a mention would route by who spoke last, not by
        # the protocol; the cascade cap is raised to the daemon's ceiling.
        self.dc.sh(d, "sed -i -e 's/^max_relay_turns = .*/max_relay_turns = 50/' "
                      "-e 's/^engagement_window_secs = .*/engagement_window_secs = 0/' "
                      "-e '/^max_queued_per_agent = /d' ~/.enoxian/agents.toml && "
                      f"sed -i '1i max_queued_per_agent = {self.args.agent_queue}' ~/.enoxian/agents.toml")
        if f"@{name}" not in self.dc.on(d, "enox", "agent", "list"):
            raise RuntimeError(f"dev-{d.lower()}: agents.toml no longer lists @{name}")
        self.dc.detach(d, f"enox --circle {self.names[d]} watch --json --log {HOME}/run.jsonl --run-id '{self.run_id}' "
                          f">/dev/null 2>>{HOME}/watch.err")
        if self.kind == "team" and self.condition["comm"] == "M2":
            ws = self.dc.sh(d, f"enox --circle {self.names[d]} status --json | jq -r '.workspace_dir // empty'", check=False).strip()
            self.ledgers[d] = f"{ws or HOME + '/enoxian/exp'}/{LEDGER}"
            if d == "A":
                self.dc.sh(d, f"printf '# Interface record\\n' > {self.ledgers[d]}")

    # -- running

    def inject(self, d: str, text: str) -> None:
        for attempt in range(5):
            try:
                self.dc.on(d, "enox", "inbox", "--circle", self.names[d], "--agent", prompts.AGENTS[d], "inject", text)
                return
            except RuntimeError:
                if attempt == 4:
                    raise
                time.sleep(3)

    def runs_on(self, d: str) -> list[dict]:
        """Every execution entry for this device's agent, newest first, following the cursor."""
        runs, cursor = [], None
        while True:
            page = self.dc.api(d, self.circles[d], "chat/executions?limit=200"
                               + (f"&before={cursor}" if cursor else "")) or {}
            runs += page.get("runs", [])
            cursor = page.get("next_cursor")
            if not cursor:
                break
        return [r for r in runs if r.get("agent_id") == prompts.AGENTS[d]]

    @staticmethod
    def ran(r: dict) -> bool:
        """Whether an entry is a turn that ran. Requests dropped before they
        started (queue full, relay budget spent) are entries too, `expired` or
        `cancelled` with no usage."""
        return str(r.get("status", "")).lower() in ("completed", "failed", "interrupted") or bool(r.get("usage"))

    def busy(self) -> bool:
        for d in self.devices_used:
            for r in self.runs_on(d):
                if str(r.get("status", "")).lower() in ("pending", "running"):
                    return True
        return False

    def finished_turns(self) -> int:
        return sum(1 for d in self.devices_used for r in self.runs_on(d)
                   if str(r.get("status", "")).lower() not in ("pending", "running") and self.ran(r))

    def freeze(self) -> None:
        """A fixed cutoff: no more pushes, no more turns admitted, then wait for
        the turns already running so their usage is recorded."""
        for flag in (self.dir / "git").rglob("owners.json"):
            (flag.parent / "frozen").touch()
        for d in self.devices_used:
            self.dc.on(d, "enox", "agent", "reaction", "pull", check=False)
        deadline = time.time() + self.args.drain_secs
        while self.busy() and time.time() < deadline:
            time.sleep(self.args.poll)
        still = sum(1 for d in self.devices_used for r in self.runs_on(d)
                    if str(r.get("status", "")).lower() == "running")
        self.events.add("frozen", payload={"still_running": still})
        if still:
            log(f"{still} turn(s) still running after {self.args.drain_secs:.0f}s; their usage is not counted")

    def main_head(self) -> str | None:
        heads = []
        for rel in sorted(set(self.repos.values())):
            out = subprocess.run(["git", "--git-dir", str(self.dir / "git" / rel), "rev-parse", "main"],
                                 capture_output=True, text=True)
            heads.append(out.stdout.strip())
        return ",".join(heads)

    def trigger_holds(self) -> bool:
        """The instance's trigger, on the integrated main (Silent: each owner's
        package from its own repo), in a throwaway container with no network so
        agent code never runs on the host and no device sees the others' repos."""
        if self.instance.meta["template_id"] != "order-discount":
            return False
        extract = " && ".join(
            f"git --git-dir=/git/{self.repo_rel(d)} archive main {pkg} | tar -x"
            for d, pkg in self.instance.meta["owners"].items()
        ) if self.kind == "silent" else f"git --git-dir=/git/{self.repo_rel('A')} archive main | tar -x"
        script = (
            "git config --global --add safe.directory '*' && mkdir /tmp/t && cd /tmp/t && "
            f"git --git-dir=/git/{self.repo_rel('A')} archive main | tar -x && {extract} && "
            # A call, not a mention: the stub's docstring already names price_order.
            "grep -Eq 'price_order[[:space:]]*\\(' svc/orders.py && timeout 20 python3 -c \""
            "from core.pricing import price_order\n"
            "try:\n    price_order('10.00', '2.00', '0.1')\n"
            "except NotImplementedError:\n    raise SystemExit(1)\n"
            "except Exception:\n    pass\""
        )
        return subprocess.run([
            "docker", "run", "--rm", "--network", "none", "--entrypoint", "sh",
            "-v", f"{self.dir / 'git'}:/git:ro", "enoxian-device:dev", "-c", script,
        ], capture_output=True).returncode == 0

    def inject_change(self, reason: str) -> None:
        entry = "A" if self.kind == "single" else self.instance.meta["entry_device"]
        log(f"change → {entry} ({reason})")
        self.stakeholders.changed = True
        self.inject(entry, prompts.CHANGE.format(message=self.instance.change["message"]))
        self.change_at = time.time()
        self.events.add("change_injected", payload={"change_type": self.instance.meta["change_type"],
                                                     "entry_device": entry, "reason": reason})

    def run(self) -> str:
        self.stakeholders = Stakeholders(self.instance, self.kind == "single", self.events)
        self.stakeholders.serve()
        budget = self.instance.meta["budget"]
        log("kickoff")
        self.started = time.time()
        for d in self.devices_used:
            self.inject(d, prompts.kickoff(self.condition, d, self.instance, self.ledgers.get(d, LEDGER)))
            self.events.add("kickoff", device=d, payload={"agent": prompts.AGENTS[d]})

        fallback = self.instance.change["trigger"].get("fallback", {}).get("after_turns")
        last_head, idle_since, end = None, None, None
        while end is None:
            time.sleep(self.args.poll)
            elapsed = time.time() - self.started
            turns = self.finished_turns()
            head = self.main_head()
            if self.change_at is None:
                if head != last_head and self.trigger_holds():
                    self.inject_change("trigger")
                elif fallback and turns >= fallback:
                    self.inject_change(f"fallback: {turns} turns")
            last_head = head

            if self.busy():
                idle_since = None
            else:
                idle_since = idle_since or time.time()
                if time.time() - idle_since >= self.args.idle_secs:
                    if self.change_at is None:
                        self.inject_change("everyone idle before the trigger")
                        idle_since = None
                    elif time.time() - self.change_at > self.args.idle_secs:
                        end = "idle after the change"
            if elapsed > budget["wall_s"]:
                end = "wall-clock budget"
            elif turns >= budget["turns"]:
                end = "turn budget"
        log(f"end: {end}")
        self.freeze()
        self.events.add("run_end", payload={"reason": end, "turns": self.finished_turns()})
        self.stakeholders.stop()
        return end

    # -- collecting

    def collect(self, end: str) -> dict:
        wall = time.time() - self.started
        executions = {d: self.runs_on(d) for d in self.devices_used}
        ran = {d: [r for r in runs if self.ran(r)] for d, runs in executions.items()}
        raw = self.dir / "devices"
        for d in self.devices_used:
            self.dc.cp_out(d, f"{HOME}/run.jsonl", raw / f"{d}.jsonl")
            (raw / f"{d}-executions.json").write_text(json.dumps(executions[d], indent=1))
            self.dc.cp_out(d, f"{HOME}/.enoxian/circles/{self.circles[d]}/agent-runs", raw / f"{d}-agent-runs")
        self.dc.compose("stop")

        final = self.dir / "final"
        subprocess.run(["git", "clone", "-q", str(self.dir / "git" / self.repos["A"]), str(final)], check=True)
        if self.kind == "silent":
            for d, pkg in self.instance.meta["owners"].items():
                tmp = Path(tempfile.mkdtemp())
                subprocess.run(["git", "clone", "-q", str(self.dir / "git" / self.repos[d]), str(tmp / "r")], check=True)
                shutil.rmtree(final / pkg)
                shutil.copytree(tmp / "r" / pkg, final / pkg)
                shutil.rmtree(tmp)
            subprocess.run(["git", "-C", str(final), "add", "-A"], check=True)
            subprocess.run(["git", "-C", str(final), "-c", "user.name=harness", "-c", "user.email=harness@local",
                            "commit", "-q", "--allow-empty", "-m", "integrate each owner's package"], check=True)
        subprocess.run(["git", "-C", str(final), "bundle", "create", "-q", str(self.dir / "final.bundle"), "HEAD"],
                       check=True, capture_output=True)

        result = self.grade(final)
        usage = [r.get("usage") or {} for runs in executions.values() for r in runs]
        tokens = {k: sum((u.get("tokens") or {}).get(k, 0) or 0 for u in usage) for k in ("input", "output", "cached_read", "total")}
        result.update({
            "condition": self.name,
            "tokens": tokens,
            "tokens_total": tokens["total"],
            "cost": sum(((u.get("cost") or {}).get("amount") or 0) for u in usage),
            "turns": sum(len(r) for r in ran.values()),
            "turns_by_device": {d: len(r) for d, r in ran.items()},
            "requests_dropped": {d: sum(1 for r in runs if not self.ran(r)) for d, runs in executions.items()},
            "wall_s": wall,
            "end": end,
            "change_injected_s": (self.change_at - self.started) if self.change_at else None,
        })
        (self.dir / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        self.write_events()
        self.write_env()
        shutil.rmtree(final)
        return result

    def grade(self, final: Path) -> dict:
        """synth.grade in a throwaway container: no network, agent code stays off the host."""
        synth = self.args.synth.resolve()
        out = subprocess.run([
            "docker", "run", "--rm", "--network", "none", "--entrypoint", "python3",
            "-v", f"{final}:/solution", "-v", f"{synth}:/synth:ro", "-v", f"{self.instance.path}:/instance:ro",
            "enoxian-device:dev", "-c",
            "import sys, json; sys.path.insert(0, '/synth'); import synth; "
            "from pathlib import Path; print(json.dumps(synth.grade(Path('/solution'), Path('/instance'))))",
        ], capture_output=True, text=True)
        if out.returncode != 0:
            raise RuntimeError(f"grading failed:\n{out.stdout}{out.stderr}")
        return json.loads(out.stdout.strip().splitlines()[-1])

    def write_events(self) -> None:
        lines = list(self.events.lines)
        for d in self.devices_used:
            f = self.dir / "devices" / f"{d}.jsonl"
            if f.exists():
                for raw in f.read_text().splitlines():
                    if raw.strip():
                        lines.append(json.loads(raw))
        for pushes in (self.dir / "git").rglob("pushes.jsonl"):
            for raw in pushes.read_text().splitlines():
                lines.append({**json.loads(raw), "run_id": self.run_id})
        lines.sort(key=lambda e: e.get("ts", ""))
        with (self.dir / "events.jsonl").open("w") as f:
            for e in lines:
                f.write(json.dumps(e, ensure_ascii=False) + "\n")

    def write_env(self) -> None:
        def git(*a, cwd=HERE):
            return subprocess.run(["git", *a], cwd=cwd, capture_output=True, text=True).stdout.strip()
        (self.dir / "env.json").write_text(json.dumps({
            "date": now(),
            "model": self.args.model,
            "agent": "acp-mini (mini-swe-agent)",
            "turn_steps": self.args.turn_steps,
            "enoxian_commit": git("rev-parse", "HEAD"),
            "enoxian_dirty": bool(git("status", "--porcelain", "--", ".")),
            "image": subprocess.run(["docker", "image", "inspect", "-f", "{{.Id}}", "enoxian-device:dev"],
                                    capture_output=True, text=True).stdout.strip(),
            "synth_commit": git("rev-parse", "HEAD", cwd=self.args.synth),
            "netem_c": self.args.netem_c,
            "agent_queue": self.args.agent_queue,
        }, indent=2) + "\n")


def default_key_file() -> Path:
    if sys.platform == "darwin":
        return Path.home() / "Library/Application Support/mini-swe-agent/.env"
    return Path.home() / ".config/mini-swe-agent/.env"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run", help="run one instance under one condition")
    p.add_argument("--instance", type=Path, required=True)
    p.add_argument("--condition", choices=sorted(CONDITIONS), required=True)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--synth", type=Path, required=True, help="SYNCagent's eval/synthetic directory (for grading)")
    p.add_argument("--runs", type=Path, default=Path("runs"))
    p.add_argument("--experiment", default="pilot")
    p.add_argument("--model", default="gemini/gemini-3.5-flash")
    p.add_argument("--turn-steps", type=int, default=80, help="model calls per agent turn")
    p.add_argument("--key-file", type=Path, default=default_key_file(),
                   help="mini-swe-agent .env holding the provider key, copied into each device")
    p.add_argument("--netem-c", default="", help='tc netem arguments for C, e.g. "delay 200ms"')
    p.add_argument("--poll", type=float, default=5.0)
    p.add_argument("--idle-secs", type=float, default=45.0, help="how long everyone must be idle to count as idle")
    p.add_argument("--agent-queue", type=int, default=64,
                   help="max_queued_per_agent on every device (enoxian #330; older daemons ignore it and queue 4)")
    p.add_argument("--drain-secs", type=float, default=300.0, help="how long to let running turns finish once the run ends")
    p.add_argument("--keep-devices", action="store_true", help="leave the containers running afterwards")
    args = parser.parse_args()

    if not args.key_file.exists():
        parser.error(f"no key file at {args.key_file}")
    instance = Instance(args.instance.resolve())
    run = Run(args, instance, args.condition)
    run.setup()
    end = run.run()
    result = run.collect(end)
    if not args.keep_devices:
        run.dc.compose("down", "-v", check=False)
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
