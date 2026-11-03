# harness

Runs one task instance under one experiment condition on the three-device
containers (`experiments/devices/`), with acp-mini as every device's agent,
and grades the result. The plan is `plan/arr-2027-plan.md` in the enox-dev
Circle; the instance format, run directory and event names follow
`plan/interfaces.md` there.

```bash
cd experiments/harness
../acp-mini/.venv/bin/python harness.py run \
    --instance ~/syn-instances/syn-order-discount-rename-B \
    --condition M2C2 --seed 0 \
    --synth ~/workspace/SYNCagent/eval/synthetic \
    --model gemini/gemini-3.5-flash
```

Instances come from SYNCagent's `synth.py build`; grading is its
`synth.grade`, so `--synth` points at that directory. The provider key is
read from mini-swe-agent's own `.env` (`mini-extra config set GEMINI_API_KEY`)
and copied into each device; it never appears on a command line.

## What one run does

1. **Devices.** `docker compose down -v` and up again: a fresh identity per
   device. Each gets the key, the `stakeholder` command, and an acp-mini agent
   — @alice on A, @bob on B, @carol on C — in push mode, working in a git
   checkout at `/home/device/repo`.
2. **Git.** A bare repo per run under the run directory, mounted at `/git`.
   Its `pre-receive` hook enforces `meta.owners`: a device may push only its
   own package to `main`. Every push, accepted or refused, is logged.
3. **Circles.** The team conditions put A, B and C in one Circle. Silent gives
   each device a Circle and a repo of its own. Ref-Full and Ref-Split run on
   A alone.
4. **Kickoff.** `enox inbox inject` gives each agent its brief, its notes and
   the condition's coordination text (`prompts.py`), privately.
5. **Change.** After every new commit on `main` the harness checks the
   instance's trigger inside device A. When it holds — or after
   `fallback.after_turns` finished turns, or when every agent has gone idle
   first — the change is injected into the entry device's agent only, and
   that device's stakeholder starts answering from `after_change`.
6. **End.** Every agent idle for `--idle-secs` after the change, or the
   instance's wall-clock or turn budget. Then `main` is frozen (the hook
   refuses further pushes), devices stop admitting turns, and running turns
   get `--drain-secs` to finish so their usage is recorded. The final `main` (Silent: each
   owner's package from its own repo) is graded in a throwaway container with
   no network, so agent code never runs on the host.

## Conditions

| Condition | Devices | Told |
|---|---|---|
| M1C1 | A, B, C in one Circle | messages only, fixed-format first line; @alice leads |
| M1C2 | A, B, C in one Circle | messages only; nobody leads |
| M2C1 | A, B, C in one Circle | task board + `interfaces.md` in the shared folder; @alice leads |
| M2C2 | A, B, C in one Circle | task board + `interfaces.md`; nobody leads |
| Silent | A, B, C, separate Circles and repos | nobody is reachable |
| Ref-Full | A | `briefs/merged.md`, owns everything |
| Ref-Split | A | all three briefs and notes, owns everything |

M2C2-W (warden) is not wired up yet.

## Output

`runs/<experiment>/<model>/<condition>/<instance>/<seed>/`:

| File | What |
|---|---|
| `result.json` | `synth.grade` fields plus tokens, cost, turns, wall time, why the run ended, when the change went in |
| `events.jsonl` | every device's `enox watch` log, the harness's own events (`kickoff`, `change_injected`, `stakeholder_qa`, `run_end`) and git pushes, by time |
| `final.bundle` | the graded repository |
| `env.json` | model, enoxian commit and image, SYNCagent commit, netem |
| `devices/` | raw logs: each device's watch log, executions, agent run records |
| `git/` | the integration repo(s) and `pushes.jsonl` |

## Known gaps

- **Spoofable identity.** The hook and the stakeholder trust
  `ENOXIAN_DEVICE_LABEL`, which an agent could override in its own shell.
  Check `events.jsonl` for a device asking or pushing as another.
- **The trigger is per template.** `trigger_holds` knows template 1
  (order-discount) only; other templates inject on the fallbacks.
- **Token budget is not enforced.** `meta.budget.tokens` (600k) is below what
  Ref-Full alone used in the pilot once cached input is counted; it is
  recorded, and the wall-clock and turn budgets are enforced, until the
  budget is settled after calibration.
- **Cascades are capped at 50 turns** by the daemon (`RELAY_TURNS_CEILING`);
  a run whose agents hit it shows `relay budget spent` in the executions.
- **Mentions past the agent queue are dropped.** The daemon queues four
  requests per agent by default; the rest expire with `per-agent queue
  capacity exceeded`. In the first M1C1 run 8 of 61 were dropped, most of them
  to the lead. The harness sets `max_queued_per_agent` (`--agent-queue`,
  default 64), which needs a daemon with enoxian #330; `result.json` reports
  what was still dropped as `requests_dropped`.
- **The stakeholder matches keywords.** A question phrased without one of a
  rule's keywords gets "I don't know." even when the rule covers it.
