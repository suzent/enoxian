"""acp-mini: mini-swe-agent behind the Agent Client Protocol.

enoxian runs agents over ACP: a mention or an `enox inbox inject` becomes a
`session/prompt`, the agent's text comes back to the Circle as its reply, and
the turn's tokens and cost land in `turn_end`. This wraps mini-swe-agent, whose
only tool is bash, so every model family in an experiment runs the same small
agent instead of each vendor's own CLI.

    enox agent add mini --driver acp -- \\
        /path/to/.venv/bin/python /path/to/acp_mini.py --model gemini/gemini-3.5-flash

One ACP session is one conversation. Each prompt is appended to it and the agent
runs until it ends the turn by printing the end marker (the lines after it are
its reply), or hits the per-turn step limit, or is cancelled. Sessions are saved
after every step so `session/load` can pick a conversation up in a new process.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import threading
import time
import uuid
from pathlib import Path
from typing import Any

# stdout is the ACP stream: mini-swe-agent's startup banner would corrupt it.
os.environ.setdefault("MSWEA_SILENT_STARTUP", "1")

import yaml
from acp import (
    PROTOCOL_VERSION,
    Agent,
    InitializeResponse,
    LoadSessionResponse,
    NewSessionResponse,
    PromptResponse,
    run_agent,
)
from acp.helpers import (
    start_tool_call,
    text_block,
    tool_content,
    update_agent_message_text,
    update_agent_thought_text,
    update_tool_call,
)
from acp.schema import AgentCapabilities, Cost, Implementation, UsageUpdate, Usage
from minisweagent.agents.default import AgentConfig, DefaultAgent
from minisweagent.environments.local import LocalEnvironment
from minisweagent.exceptions import FormatError, LimitsExceeded, Submitted

VERSION = "0.1.0"
END_MARKER = "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT"

SYSTEM_TEMPLATE = """\
You are an agent working on one device of an enoxian Circle: several people and
agents, each on their own machine, who share a live folder, a task board and a
chat. You act only through the bash tool. Run `enox --help` to see how to read
the chat, claim tasks, and post messages.
"""

INSTANCE_TEMPLATE = """\
{{task}}

Work in {{cwd}} with the bash tool. Each command runs in a new subshell, so `cd`
and environment variables do not carry over; prefix commands as needed.

When you have done what this message needs, end your turn with exactly this
command and nothing else in it; the lines after the first become your reply in
the chat:

cat <<'EOF'
COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT
<your reply>
EOF
"""

FOLLOWUP_TEMPLATE = """\
{{task}}

End your turn the same way as before: the end marker in a heredoc, then your reply.
"""


class Turn(DefaultAgent):
    """A DefaultAgent that keeps one conversation across several prompts."""

    def __init__(self, model, env, *, on_step=None, **kwargs):
        super().__init__(model, env, config_class=SessionConfig, **kwargs)
        self.on_step = on_step or (lambda **_: None)
        self.stop = threading.Event()

    def run_turn(self, text: str) -> tuple[str, str, dict]:
        """Run one prompt to its end. Returns (stop reason, reply, usage summed over the turn)."""
        self.stop.clear()
        self._start_time = time.time()
        first = not self.messages
        self.extra_template_vars |= {"task": text}
        if first:
            self.add_messages(
                self.model.format_message(role="system", content=self._render_template(self.config.system_template)),
                self.model.format_message(role="user", content=self._render_template(self.config.instance_template)),
            )
        else:
            self.add_messages(
                self.model.format_message(role="user", content=self._render_template(self.config.followup_template))
            )
        usage = _zero_usage()
        limit = self.n_calls + self.config.turn_step_limit if self.config.turn_step_limit else 0
        format_errors = 0
        while True:
            if self.stop.is_set():
                return "cancelled", "", usage
            if limit and self.n_calls >= limit:
                return "max_turn_requests", "", usage
            try:
                message = self.query()
            except FormatError as e:
                self.cost += e.messages[0].get("extra", {}).get("cost", 0.0)
                _add_usage(usage, e.messages[0].get("extra", {}).get("response"))
                self.add_messages(*e.messages)
                format_errors += 1
                if 0 < self.config.max_consecutive_format_errors <= format_errors:
                    return "refusal", "", usage
                continue
            except LimitsExceeded:
                return "max_turn_requests", "", usage
            format_errors = 0
            _add_usage(usage, message.get("extra", {}).get("response"))
            _slim(message)
            reply = self.execute_actions(message)
            if reply is not None:
                return "end_turn", reply, usage

    def execute_actions(self, message: dict) -> str | None:
        """Run the message's commands; returns the reply if one of them ended the turn."""
        if message.get("content"):
            self.on_step(thought=message["content"])
        outputs, reply = [], None
        for action in message.get("extra", {}).get("actions", []):
            call_id = action.get("tool_call_id") or uuid.uuid4().hex
            self.on_step(started=(call_id, action.get("command", "")))
            try:
                output = self.env.execute(action)
            except Submitted as done:
                reply = done.messages[0]["content"].strip()
                output = {"output": "Turn ended; your reply was posted.", "returncode": 0, "exception_info": ""}
            self.on_step(finished=(call_id, output))
            outputs.append(output)
            if reply is not None:
                break
        self.add_messages(*self.model.format_observation_messages(message, outputs, self.get_template_vars()))
        self.on_step(cost=self.cost, context=_context_tokens(message))
        return reply


class SessionConfig(AgentConfig):
    followup_template: str = FOLLOWUP_TEMPLATE
    turn_step_limit: int = 100
    """Model calls allowed in one prompt turn; 0 for no limit."""


def _zero_usage() -> dict:
    return {"input": 0, "output": 0, "cached_read": 0, "thought": 0, "model": None}


def _add_usage(total: dict, response: Any) -> None:
    """Add one litellm response's `usage` to the turn's running total."""
    if not isinstance(response, dict):
        return
    usage = response.get("usage") or {}
    total["input"] += usage.get("prompt_tokens") or 0
    total["output"] += usage.get("completion_tokens") or 0
    total["cached_read"] += (usage.get("prompt_tokens_details") or {}).get("cached_tokens") or 0
    total["thought"] += (usage.get("completion_tokens_details") or {}).get("reasoning_tokens") or 0
    total["model"] = response.get("model") or total["model"]


def _context_tokens(message: dict) -> int:
    response = message.get("extra", {}).get("usage") or {}
    return response.get("prompt_tokens") or 0


def _slim(message: dict) -> None:
    """Keep a response's usage and model id, not the whole raw response, in the saved history."""
    extra = message.get("extra", {})
    response = extra.pop("response", None)
    if isinstance(response, dict):
        extra["usage"] = response.get("usage")
        extra["model"] = response.get("model")


class MiniAgent(Agent):
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.state_dir = Path(args.state_dir).expanduser()
        self.state_dir.mkdir(parents=True, exist_ok=True)
        self.templates = yaml.safe_load(Path(args.config).read_text()).get("agent", {}) if args.config else {}
        self.sessions: dict[str, Turn] = {}
        self.conn = None

    def on_connect(self, conn) -> None:
        self.conn = conn

    async def initialize(self, protocol_version: int, **kwargs: Any) -> InitializeResponse:
        return InitializeResponse(
            protocol_version=PROTOCOL_VERSION,
            agent_capabilities=AgentCapabilities(load_session=True),
            agent_info=Implementation(name="acp-mini", title="mini-swe-agent", version=VERSION),
            auth_methods=[],
        )

    async def new_session(self, cwd: str, **kwargs: Any) -> NewSessionResponse:
        session_id = uuid.uuid4().hex
        self.sessions[session_id] = self._turn(session_id, cwd)
        self._save(session_id)
        return NewSessionResponse(session_id=session_id)

    async def load_session(self, cwd: str, session_id: str, **kwargs: Any) -> LoadSessionResponse | None:
        path = self._path(session_id)
        if not path.is_file():
            raise FileNotFoundError(f"no saved session {session_id}")
        saved = json.loads(path.read_text())
        turn = self._turn(session_id, cwd)
        turn.messages, turn.cost, turn.n_calls = saved["messages"], saved["cost"], saved["n_calls"]
        self.sessions[session_id] = turn
        # Earlier turns' spend, so the client does not count it as the next turn's.
        await self._send(session_id, self._usage_update(turn.cost, saved.get("context", 0)))
        return LoadSessionResponse()

    async def prompt(self, session_id: str, prompt: list, **kwargs: Any) -> PromptResponse:
        turn = self.sessions[session_id]
        text = "\n\n".join(block.text for block in prompt if getattr(block, "type", None) == "text")
        loop = asyncio.get_running_loop()

        def on_step(**event):
            update = None
            if "thought" in event:
                update = update_agent_thought_text(event["thought"])
            elif "started" in event:
                call_id, command = event["started"]
                update = start_tool_call(call_id, command[:200], kind="execute", status="in_progress",
                                         raw_input={"command": command})
            elif "finished" in event:
                call_id, output = event["finished"]
                text_out = (output.get("output") or "")[-4000:]
                update = update_tool_call(
                    call_id, status="completed" if output.get("returncode") == 0 else "failed",
                    content=[tool_content(text_block(text_out))], raw_output={"returncode": output.get("returncode")},
                )
            elif "cost" in event:
                self._save(session_id, context=event["context"])
                update = self._usage_update(event["cost"], event["context"])
            if update is not None:
                asyncio.run_coroutine_threadsafe(self._send(session_id, update), loop).result()

        turn.on_step = on_step
        try:
            stop_reason, reply, usage = await asyncio.to_thread(turn.run_turn, text)
        finally:
            self._save(session_id)
        if reply:
            await self._send(session_id, update_agent_message_text(reply))
        total = usage["input"] + usage["output"]
        return PromptResponse(
            stop_reason=stop_reason,
            usage=Usage(
                input_tokens=usage["input"], output_tokens=usage["output"], total_tokens=total,
                cached_read_tokens=usage["cached_read"], thought_tokens=usage["thought"],
                field_meta={"model": usage["model"]},
            ) if total else None,
        )

    async def cancel(self, session_id: str, **kwargs: Any) -> None:
        if session_id in self.sessions:
            self.sessions[session_id].stop.set()

    def _turn(self, session_id: str, cwd: str) -> Turn:
        model = make_model(self.args.model)
        env = LocalEnvironment(cwd=cwd, timeout=self.args.command_timeout,
                               env={"PAGER": "cat", "MANPAGER": "cat", "GIT_PAGER": "cat"})
        return Turn(
            model, env,
            system_template=self.templates.get("system_template", SYSTEM_TEMPLATE),
            instance_template=self.templates.get("instance_template", INSTANCE_TEMPLATE),
            followup_template=self.templates.get("followup_template", FOLLOWUP_TEMPLATE),
            turn_step_limit=self.args.turn_steps,
            step_limit=0,
            cost_limit=0,
        )

    def _usage_update(self, cost: float, context: int) -> UsageUpdate:
        return UsageUpdate(session_update="usage_update", used=context, size=self.args.context_size,
                           cost=Cost(amount=cost, currency="USD"))

    async def _send(self, session_id: str, update) -> None:
        await self.conn.session_update(session_id=session_id, update=update)

    def _path(self, session_id: str) -> Path:
        return self.state_dir / f"{session_id}.json"

    def _save(self, session_id: str, context: int = 0) -> None:
        turn = self.sessions[session_id]
        path = self._path(session_id)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps({
            "model": self.args.model, "cost": turn.cost, "n_calls": turn.n_calls,
            "context": context, "messages": turn.messages,
        }, default=str))
        tmp.replace(path)


def make_model(name: str):
    """`fake:<path>` replays a scripted run (for tests); anything else goes to litellm."""
    if name.startswith("fake:"):
        return FakeModel(Path(name[5:]))
    from minisweagent.models.litellm_model import LitellmModel

    return LitellmModel(model_name=name, cost_tracking=os.getenv("MSWEA_COST_TRACKING", "ignore_errors"))


class FakeModel:
    """Answers from a JSON list of `{"content": ..., "command": ...}` steps, in order.

    The position is kept next to the script, so a reloaded session continues the
    script where the previous process left off.
    """

    def __init__(self, script: Path):
        self.script = script
        self.steps = json.loads(script.read_text())
        self.cursor = script.with_suffix(".pos")

    def query(self, messages: list[dict], **kwargs) -> dict:
        i = int(self.cursor.read_text()) if self.cursor.exists() else 0
        self.cursor.write_text(str(i + 1))
        step = self.steps[i]
        call_id = f"call_{i}"
        return {
            "role": "assistant",
            "content": step.get("content", ""),
            "tool_calls": [{"id": call_id, "type": "function",
                            "function": {"name": "bash", "arguments": json.dumps({"command": step["command"]})}}],
            "extra": {
                "actions": [{"command": step["command"], "tool_call_id": call_id}],
                "cost": step.get("cost", 0.01),
                "response": {"model": "fake", "usage": {"prompt_tokens": 100 * (i + 1), "completion_tokens": 10}},
            },
        }

    def format_message(self, **kwargs) -> dict:
        return kwargs

    def format_observation_messages(self, message: dict, outputs: list[dict], template_vars: dict | None = None):
        from minisweagent.models.utils.actions_toolcall import format_toolcall_observation_messages

        return format_toolcall_observation_messages(
            actions=message["extra"]["actions"], outputs=outputs,
            observation_template="<returncode>{{output.returncode}}</returncode>\n{{output.output}}",
        )

    def get_template_vars(self, **kwargs) -> dict:
        return {}

    def serialize(self) -> dict:
        return {}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--model", required=True, help="litellm model name, e.g. gemini/gemini-3.5-flash")
    parser.add_argument("--config", help="YAML with agent.system_template / instance_template / followup_template")
    parser.add_argument("--turn-steps", type=int, default=100, help="model calls per prompt turn (0: no limit)")
    parser.add_argument("--command-timeout", type=int, default=120, help="seconds per bash command")
    parser.add_argument("--context-size", type=int, default=0, help="model context window, for usage_update")
    parser.add_argument("--state-dir", default=os.getenv("ACP_MINI_HOME", "~/.acp-mini/sessions"))
    args = parser.parse_args()
    asyncio.run(run_agent(MiniAgent(args)))


if __name__ == "__main__":
    main()
