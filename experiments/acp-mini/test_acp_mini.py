"""Drive acp_mini.py over stdio the way enoxian does, with a scripted fake model."""

import json
import subprocess
import sys
import time
from pathlib import Path

import pytest

HERE = Path(__file__).parent
END = "cat <<'EOF'\nCOMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT\n{}\nEOF"


class Client:
    def __init__(self, script: Path, state: Path, *extra: str):
        self.proc = subprocess.Popen(
            [sys.executable, str(HERE / "acp_mini.py"), "--model", f"fake:{script}", "--state-dir", str(state), *extra],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
        )
        self.next_id = 0
        self.updates: list[dict] = []

    def send(self, method: str, params: dict, notify: bool = False) -> int | None:
        msg = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notify:
            self.next_id += 1
            msg["id"] = self.next_id
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()
        return msg.get("id")

    def wait(self, request_id: int) -> dict:
        while True:
            line = self.proc.stdout.readline()
            assert line, "agent exited"
            msg = json.loads(line)
            if msg.get("method") == "session/update":
                self.updates.append(msg["params"]["update"])
            elif msg.get("id") == request_id:
                assert "error" not in msg, msg["error"]
                return msg["result"]

    def call(self, method: str, params: dict) -> dict:
        return self.wait(self.send(method, params))

    def kinds(self) -> list[str]:
        return [u["sessionUpdate"] for u in self.updates]

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def script(tmp_path: Path, steps: list[dict]) -> Path:
    path = tmp_path / "script.json"
    path.write_text(json.dumps(steps))
    return path


@pytest.fixture
def work(tmp_path):
    (tmp_path / "work").mkdir()
    return tmp_path / "work"


def prompt(client: Client, sid: str, text: str) -> dict:
    return client.call("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": text}]})


def test_a_turn_runs_commands_and_replies(tmp_path, work):
    steps = [
        {"content": "writing the file", "command": "echo hi > f.txt"},
        {"command": END.format("wrote f.txt")},
    ]
    client = Client(script(tmp_path, steps), tmp_path / "state")
    init = client.call("initialize", {"protocolVersion": 1, "clientCapabilities": {}})
    assert init["protocolVersion"] == 1 and init["agentCapabilities"]["loadSession"] is True
    sid = client.call("session/new", {"cwd": str(work), "mcpServers": []})["sessionId"]

    result = prompt(client, sid, "make f.txt")
    assert (work / "f.txt").read_text() == "hi\n"
    assert result["stopReason"] == "end_turn"
    # Two model calls: 100 + 200 prompt tokens, 10 + 10 completion tokens.
    assert result["usage"]["inputTokens"] == 300 and result["usage"]["totalTokens"] == 320
    replies = [u["content"]["text"] for u in client.updates if u["sessionUpdate"] == "agent_message_chunk"]
    assert replies == ["wrote f.txt"]
    assert "tool_call" in client.kinds() and "tool_call_update" in client.kinds()
    costs = [u["cost"]["amount"] for u in client.updates if u["sessionUpdate"] == "usage_update"]
    # One report per step, and the cumulative total again when the turn ends.
    assert costs == pytest.approx([0.01, 0.02, 0.02])
    client.close()


def test_a_later_prompt_continues_the_conversation_even_in_a_new_process(tmp_path, work):
    steps = [
        {"command": END.format("first")},
        {"command": END.format("second")},
        {"command": END.format("third")},
    ]
    path, state = script(tmp_path, steps), tmp_path / "state"
    client = Client(path, state)
    client.call("initialize", {"protocolVersion": 1})
    sid = client.call("session/new", {"cwd": str(work), "mcpServers": []})["sessionId"]
    prompt(client, sid, "one")
    prompt(client, sid, "two")
    client.close()

    saved = json.loads((state / f"{sid}.json").read_text())
    users = [m["content"] for m in saved["messages"] if m["role"] == "user"]
    assert users[0].startswith("one") and users[1].startswith("two")

    # A new process, as after a daemon restart: load, then carry on.
    client = Client(path, state)
    client.call("initialize", {"protocolVersion": 1})
    client.call("session/load", {"sessionId": sid, "cwd": str(work), "mcpServers": []})
    # The load reports what was already spent, so it is not billed to the next turn.
    assert client.updates[-1]["sessionUpdate"] == "usage_update"
    assert client.updates[-1]["cost"]["amount"] == pytest.approx(0.02)
    result = prompt(client, sid, "three")
    assert result["stopReason"] == "end_turn"
    assert client.updates[-1]["content"]["text"] == "third"
    client.close()


def test_loading_an_unknown_session_fails(tmp_path, work):
    client = Client(script(tmp_path, []), tmp_path / "state")
    client.call("initialize", {"protocolVersion": 1})
    request = client.send("session/load", {"sessionId": "nope", "cwd": str(work), "mcpServers": []})
    while True:
        msg = json.loads(client.proc.stdout.readline())
        if msg.get("id") == request:
            # The code enoxian reads as "session gone", so it starts a new one.
            assert msg["error"]["code"] == -32002
            break
    client.close()


def test_a_turn_ended_by_format_errors_still_reports_its_cost(tmp_path, work):
    steps = [{"command": "true"}] + [{"malformed": True}] * 3
    client = Client(script(tmp_path, steps), tmp_path / "state")
    client.call("initialize", {"protocolVersion": 1})
    sid = client.call("session/new", {"cwd": str(work), "mcpServers": []})["sessionId"]
    assert prompt(client, sid, "go")["stopReason"] == "refusal"
    costs = [u["cost"]["amount"] for u in client.updates if u["sessionUpdate"] == "usage_update"]
    assert costs[-1] == pytest.approx(0.04)
    client.close()


def test_cancel_stops_before_the_next_step(tmp_path, work):
    steps = [{"command": "sleep 1"}, {"command": "touch never"}]
    client = Client(script(tmp_path, steps), tmp_path / "state")
    client.call("initialize", {"protocolVersion": 1})
    sid = client.call("session/new", {"cwd": str(work), "mcpServers": []})["sessionId"]
    request = client.send("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": "go"}]})
    time.sleep(0.5)
    client.send("session/cancel", {"sessionId": sid}, notify=True)
    assert client.wait(request)["stopReason"] == "cancelled"
    assert not (work / "never").exists()
    client.close()


def test_a_turn_stops_at_its_step_limit(tmp_path, work):
    steps = [{"command": "true"}] * 5
    client = Client(script(tmp_path, steps), tmp_path / "state", "--turn-steps", "2")
    client.call("initialize", {"protocolVersion": 1})
    sid = client.call("session/new", {"cwd": str(work), "mcpServers": []})["sessionId"]
    assert prompt(client, sid, "loop")["stopReason"] == "max_turn_requests"
    client.close()
