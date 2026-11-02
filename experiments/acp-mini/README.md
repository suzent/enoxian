# acp-mini: mini-swe-agent over ACP

The agent every device runs in the multi-device experiments. It wraps
[mini-swe-agent](https://mini-swe-agent.com) (one bash tool, a plain loop) in
the Agent Client Protocol, so enoxian runs it like any other ACP agent:

- a chat mention, a warden assignment or `enox inbox --agent <name> inject`
  becomes a `session/prompt` and wakes it;
- the reply goes back to the Circle (or, for an inject, to the device only);
- the turn's tokens and cost land in `turn_end`, so `enox watch --json --log`
  records them.

Every model family runs the same agent through litellm (`gemini/...`, an
OpenAI-compatible vLLM endpoint for Qwen, `anthropic/...`), so differences
between runs come from the model and the coordination condition, not from each
vendor's CLI.

## Use

The device image (`experiments/devices/Dockerfile`) installs it at
`/opt/acp-mini`. On a device:

```bash
enox agent add mini --driver acp -- \
    /opt/acp-mini/venv/bin/python /opt/acp-mini/acp_mini.py --model gemini/gemini-3.5-flash
enox agent reaction push
```

The model's API key comes from the daemon's environment (`GEMINI_API_KEY`,
`ANTHROPIC_API_KEY`, `OPENAI_API_BASE` + `OPENAI_API_KEY` for vLLM, ...).

| Flag | Default | |
|------|---------|--|
| `--model` | required | litellm model name; `fake:<script.json>` replays a scripted run |
| `--turn-steps` | 100 | model calls allowed in one prompt turn |
| `--command-timeout` | 120 | seconds per bash command |
| `--context-size` | 0 | context window, reported in `usage_update` |
| `--config` | | YAML overriding `agent.system_template`, `instance_template`, `followup_template` |
| `--state-dir` | `~/.acp-mini/sessions` | saved conversations (`$ACP_MINI_HOME`) |

## How a turn works

One ACP session is one conversation. The first prompt is rendered into the
instance template; later prompts are appended with the follow-up template. The
agent runs bash commands until it ends the turn with

```
cat <<'EOF'
COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT
<reply>
EOF
```

and the lines after the marker are its reply. A turn also ends at
`--turn-steps` (`max_turn_requests`), on `session/cancel` before the next step
(`cancelled`), or after repeated format errors (`refusal`).

The conversation is saved after every step, so `session/load` continues it in a
new process (after a daemon restart, say). The load reports the cost spent so
far, so it is not billed to the next turn. Saved messages keep each response's
token usage and the model id the provider reported.

## Test

```bash
cd experiments/acp-mini
uv venv -p 3.12 .venv && uv pip install -p .venv/bin/python -r requirements.txt pytest
.venv/bin/python -m pytest -q
```

The tests drive the adapter over stdio the way enoxian does, with a scripted
fake model. End to end on the three devices: run `experiments/devices/smoke.sh`,
add `mini` on one device with `--model fake:/runs/<script>.json`, then inject
to it and mention it from another device.
