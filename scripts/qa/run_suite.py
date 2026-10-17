#!/usr/bin/env python3
"""Real-Task Deviation Suite runner (levels 1+2) for the allspark fork.

Drives the real `prime-agent` binary headless (`-p`/print mode, or the
allspark launcher layout) against the task corpus in `qa-tasks/`, in
per-trial throwaway sandboxes, then grades each trial with:

  * the task oracle (state checks or a script oracle), and
  * level-2 trace invariants parsed from the session JSONL
    (must/must_not tool-call patterns, budgets, secrets, harmful calls).

Every run writes a manifest (suiteRev, binaryBuild, model, settingsHash,
per-trial results, behavioral fingerprint) under `--out/<ts>/manifest.json`.
`--baseline <manifest.json>` turns a run into a regression gate
(-3pp trial pass-rate or >=2 stable tasks losing pass^N => exit 1).

Level-2 is now log-aware and output-aware: the trial also captures the
product's own telemetry (`logs/agent.jsonl` request timing and the rotating
`logs/debug-*.log` tracing stream), normalizes the transcript into a timeline
(`qa_logs.py`), checks the task's `logs:` invariants, records the transcript
JSON schema so field drift between runs is a manifest delta, grades the final
answer against the task's `output:` patterns (`qa_judge.py`), and - with
`--judge` - asks one System One (SubQ Snap) judge call per task on the task
rubric. The judge is advisory: it lands in the manifest and the A/B delta,
never in the blocking gate (levels 3-4 in
`.research/blackbox-qa-best-practices.md`).

Python stdlib only. Task files are `qa-tasks/<id>/task.yaml` parsed by the
strict mini-YAML subset documented in qa-tasks/README.md.

Waits/polls (per the suite's no-sleep discipline): the binary run is a
single `communicate(timeout=...)`; mock providers bind before setup runs
(no readiness wait); the only poll loop is the daemon-lane worker-kill
hook (0.5s marker-file interval against a hard deadline) because mid-run
process interference has no observable-readiness event.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from collections import Counter
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SUITE_DIR = REPO_ROOT / "qa-tasks"

# Sibling modules (same directory; the runner is invoked as a script).
sys.path.insert(0, str(Path(__file__).resolve().parent))
import qa_judge  # noqa: E402  - output oracle + judge tier
import qa_logs  # noqa: E402   - transcript/timing/debug-log normalizers

# The observability switches the log layer needs. Both are sandbox-local
# (HOME is pinned inside the trial), so they never touch the host agent dir.
LOG_ENV = {
    "PI_REQUEST_TIMING": "1",
    "ALLSPARK_DEBUG": "1",
    # `warn` keeps the rotating log small; the admission/breaker events are
    # INFO, so the pa_daemon target is raised explicitly. The directive
    # grammar is `EnvFilter`, so unknown targets are harmless.
    "ALLSPARK_DEBUG_FILTER": "warn,pa_daemon=info,pa_core=warn,pa_ai=warn,pa_agent=warn",
}

# ---------------------------------------------------------------------------
# Minimal YAML subset parser (grammar: see qa-tasks/README.md).
# ---------------------------------------------------------------------------


class MiniYamlError(ValueError):
    pass


def _scalar(text: str):
    text = text.strip()
    if not text.startswith(('"', "'")):
        # A trailing ` #comment` on an unquoted scalar is a comment (real YAML
        # strips it); quoted scalars keep their `#` verbatim.
        hash_at = text.find(" #")
        if hash_at > 0:
            text = text[:hash_at].rstrip()
    if text.startswith('"') and text.endswith('"') and len(text) >= 2:
        return text[1:-1].replace('\\"', '"')
    if text.startswith("'") and text.endswith("'") and len(text) >= 2:
        return text[1:-1].replace("''", "'")
    if text == "true":
        return True
    if text == "false":
        return False
    if re.fullmatch(r"-?\d+", text):
        return int(text)
    if re.fullmatch(r"-?\d+\.\d+", text):
        return float(text)
    return text


def _indent_of(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def _ok_key(text: str) -> bool:
    return bool(re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", text.strip()))


def mini_yaml_load(text: str):
    lines = [ln.rstrip("\n") for ln in text.split("\n")]
    lines = [ln for ln in lines if ln.strip() and not ln.lstrip(" ").startswith("#")]
    if not lines:
        return {}
    value, consumed = _parse_block(lines, 0, _indent_of(lines[0]))
    if consumed != len(lines):
        raise MiniYamlError(f"trailing content at line {consumed + 1}: {lines[consumed]!r}")
    return value


def _parse_block(lines, start, indent):
    if lines[start].lstrip(" ").startswith("- "):
        return _parse_list(lines, start, indent)
    return _parse_map(lines, start, indent)


def _parse_list(lines, start, indent):
    items = []
    i = start
    while i < len(lines):
        line = lines[i]
        ind = _indent_of(line)
        if ind < indent or not line.lstrip(" ").startswith("- "):
            break
        if ind > indent:
            raise MiniYamlError(f"unexpected indent at line {i + 1}: {line!r}")
        rest = line.lstrip(" ")[2:]
        if not rest:
            if i + 1 < len(lines) and _indent_of(lines[i + 1]) > indent:
                value, i = _parse_block(lines, i + 1, _indent_of(lines[i + 1]))
                items.append(value)
            else:
                items.append(None)
                i += 1
            continue
        first_key = rest.split(":", 1)[0] if ":" in rest else ""
        if ":" in rest and _ok_key(first_key) and not rest.startswith(('"', "'")):
            # Map item: re-anchor the first pair at item indent and parse on.
            item_indent = indent + 2
            synthetic = [" " * item_indent + rest] + lines[i + 1:]
            value, consumed = _parse_map(synthetic, 0, item_indent)
            items.append(value)
            i += consumed
            continue
        items.append(_scalar(rest))
        i += 1
    return items, i


def _parse_map(lines, start, indent):
    result = {}
    i = start
    while i < len(lines):
        line = lines[i]
        ind = _indent_of(line)
        if ind < indent or line.lstrip(" ").startswith("- "):
            break
        if ind > indent:
            raise MiniYamlError(f"unexpected indent at line {i + 1}: {line!r}")
        if ":" not in line:
            raise MiniYamlError(f"expected `key:` at line {i + 1}: {line!r}")
        key, _, rest = line.partition(":")
        key = key.strip()
        if not _ok_key(key):
            raise MiniYamlError(f"bad key at line {i + 1}: {key!r}")
        rest = rest.strip()
        if rest in ("|", "|-", "|+", ">", ">-", ">+"):
            body = []
            j = i + 1
            block_indent = None
            while j < len(lines):
                nxt = lines[j]
                if not nxt.strip():
                    body.append("")
                    j += 1
                    continue
                nxt_ind = _indent_of(nxt)
                if nxt_ind <= indent:
                    break
                if block_indent is None:
                    block_indent = nxt_ind
                body.append(nxt[block_indent:])
                j += 1
            while body and body[-1] == "":
                body.pop()
            if rest.startswith(">"):
                # Folded scalar: single newlines become spaces, blank lines
                # stay as paragraph breaks (the YAML folding rule we need for
                # prose rubrics).
                folded, paragraph = [], []
                for line in body:
                    if line.strip():
                        paragraph.append(line.strip())
                    elif paragraph:
                        folded.append(" ".join(paragraph))
                        paragraph = []
                if paragraph:
                    folded.append(" ".join(paragraph))
                text = "\n".join(folded)
            else:
                text = "\n".join(body)
            if rest in ("|", ">"):
                text += "\n"
            result[key] = text
            i = j
            continue
        if rest == "":
            if i + 1 < len(lines) and _indent_of(lines[i + 1]) > indent:
                value, i = _parse_block(lines, i + 1, _indent_of(lines[i + 1]))
                result[key] = value
            elif i + 1 < len(lines) and _indent_of(lines[i + 1]) == indent \
                    and lines[i + 1].lstrip(" ").startswith("- "):
                value, i = _parse_list(lines, i + 1, indent)
                result[key] = value
            else:
                result[key] = None
                i += 1
            continue
        result[key] = _scalar(rest)
        i += 1
    return result, i


# ---------------------------------------------------------------------------
# Loopback mock providers (wire shapes copied from pa-ai's own test mocks).
# ---------------------------------------------------------------------------


class MockProvider(threading.Thread):
    """One loopback inference mock. `behavior` selects the wire script:

    anthropic-402 / openai-402  : always answer 402 with a payment error body
    openai-500                  : always answer 500 (transient fault)
    openai-ok / anthropic-ok    : one SSE completion carrying `answer`
    openai-hang / anthropic-hang: read the request, then never answer
    """

    def __init__(self, name: str, behavior: str, answer: str = "mock answer"):
        super().__init__(daemon=True, name=f"mock-{name}")
        self.mock_name = name
        self.behavior = behavior
        self.answer = answer
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.port = self.sock.getsockname()[1]
        self.base_url = f"http://127.0.0.1:{self.port}"
        self.stop_evt = threading.Event()
        self.requests: list[str] = []
        self.lock = threading.Lock()

    def run(self):
        self.sock.settimeout(0.5)
        while not self.stop_evt.is_set():
            try:
                conn, _ = self.sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            threading.Thread(target=self._serve, args=(conn,), daemon=True).start()

    def _read_request(self, conn) -> bytes:
        conn.settimeout(5)
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = conn.recv(65536)
            if not chunk:
                break
            buf += chunk
        head, _, rest = buf.partition(b"\r\n\r\n")
        length = 0
        for line in head.split(b"\r\n"):
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":", 1)[1].strip() or b"0")
        while len(rest) < length:
            chunk = conn.recv(65536)
            if not chunk:
                break
            rest += chunk
        return head + b"\r\n\r\n" + rest

    def _serve(self, conn):
        try:
            raw = self._read_request(conn)
            path = raw.split(b" ")[1].decode("latin1") if b" " in raw else "?"
            with self.lock:
                self.requests.append(path)
            if "hang" in self.behavior:
                while not self.stop_evt.wait(1.0):
                    pass
                return
            body = self._response_body()
            head = (
                f"HTTP/1.1 {body['status']}\r\n"
                f"Content-Type: {body['ctype']}\r\n"
                f"Content-Length: {len(body['body'])}\r\n"
                "Connection: close\r\n\r\n"
            ).encode("latin1")
            conn.sendall(head + body["body"])
        except OSError:
            pass
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def _response_body(self):
        if self.behavior in ("anthropic-402", "openai-402"):
            if self.behavior == "anthropic-402":
                payload = {"type": "error", "error": {
                    "type": "insufficient_credit",
                    "code": "insufficient_credits",
                    "message": "insufficient_credits: This organization has run out of credits"}}
            else:
                payload = {"error": {
                    "message": "insufficient_credits: This organization has run out of credits",
                    "type": "insufficient_credit",
                    "code": "insufficient_credits"}}
            return {"status": "402 Payment Required", "ctype": "application/json",
                    "body": json.dumps(payload).encode()}
        if self.behavior == "openai-500":
            payload = json.dumps(
                {"error": {"message": "mock provider overloaded", "type": "server_error", "code": 500}}
            ).encode()
            return {"status": "500 Internal Server Error", "ctype": "application/json", "body": payload}
        if self.behavior == "openai-ok":
            frames = [
                self._openai_chunk({"role": "assistant", "content": self.answer}, None),
                self._openai_chunk({}, "stop"),
            ]
            payload = "".join(f"data: {f}\n\n" for f in frames).encode() + b"data: [DONE]\n\n"
            return {"status": "200 OK", "ctype": "text/event-stream", "body": payload}
        if self.behavior == "anthropic-ok":
            events = [
                'event: message_start\ndata: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}\n\n',
                'event: content_block_start\ndata: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}\n\n',
                f'event: content_block_delta\ndata: {{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":{json.dumps(self.answer)}}}}}\n\n',
                'event: content_block_stop\ndata: {"type":"content_block_stop","index":0}\n\n',
                'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}\n\n',
                'event: message_stop\ndata: {"type":"message_stop"}\n\n',
            ]
            return {"status": "200 OK", "ctype": "text/event-stream", "body": "".join(events).encode()}
        raise ValueError(f"unknown mock behavior {self.behavior}")

    @staticmethod
    def _openai_chunk(delta, finish):
        return json.dumps({
            "id": "chatcmpl-qa",
            "object": "chat.completion.chunk",
            "created": 1750000000,
            "model": "mock-1",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })

    def stop(self):
        self.stop_evt.set()
        try:
            self.sock.close()
        except OSError:
            pass

    def count(self) -> int:
        with self.lock:
            return len(self.requests)


# ---------------------------------------------------------------------------
# Task corpus
# ---------------------------------------------------------------------------


class Task:
    def __init__(self, path: Path):
        self.dir = path
        self.spec = mini_yaml_load((path / "task.yaml").read_text(encoding="utf-8"))
        spec = self.spec
        self.id = spec["id"]
        self.title = spec.get("title", "")
        self.lanes = spec.get("lane", [])
        if isinstance(self.lanes, str):
            self.lanes = [self.lanes]
        self.network = spec.get("network", "off")
        self.budget = spec.get("budget", {}) or {}
        self.entry = spec.get("entry", "print")
        self.prompt = spec.get("prompt", "")
        self.files = spec.get("files", []) or []
        if isinstance(self.files, str):
            self.files = [self.files]
        self.mocks = spec.get("mock_providers", []) or []
        self.oracle = spec.get("oracle", {}) or {}
        self.must = spec.get("must", []) or []
        self.must_not = spec.get("must_not", []) or []
        self.max_repeat = int(spec.get("max_repeat_calls", 0) or 0)
        self.secrets_env = spec.get("secrets_env", {}) or {}
        self.hostile_env = spec.get("hostile_env", {}) or {}
        self.env = spec.get("env", {}) or {}
        self.bin_args = spec.get("bin_args", []) or []
        if isinstance(self.bin_args, str):
            self.bin_args = [self.bin_args]
        self.kill_worker = spec.get("kill_worker", None)
        self.setup = spec.get("setup", None)
        self.difficulty = spec.get("difficulty", "?")
        # Level-2 expansion: log invariants, deterministic output patterns and
        # the level-3 judge rubric. All optional; a task without them behaves
        # exactly as before (backwards compatible task.yaml).
        self.logs = spec.get("logs", {}) or {}
        self.output = spec.get("output", []) or []
        if isinstance(self.output, dict):
            self.output = [self.output]
        self.judge = spec.get("judge", {}) or {}

    @property
    def timeout_s(self) -> int:
        return int(self.budget.get("timeout_s", 120))

    @property
    def max_steps(self) -> int:
        return int(self.budget.get("steps", 0) or 0)

    @property
    def max_tokens(self) -> int:
        return int(self.budget.get("tokens", 0) or 0)

    def needs_kernel(self) -> bool:
        path = self.dir / "faux-script.json"
        return path.is_file() and "ipython" in path.read_text(encoding="utf-8")


def load_tasks(patterns=None) -> list[Task]:
    tasks = []
    if not SUITE_DIR.is_dir():
        return tasks
    for path in sorted(SUITE_DIR.iterdir()):
        if not path.is_dir() or not (path / "task.yaml").is_file():
            continue
        task = Task(path)
        if patterns and not any(p in task.id for p in patterns):
            continue
        tasks.append(task)
    return tasks


# ---------------------------------------------------------------------------
# Sandbox + environment discipline
# ---------------------------------------------------------------------------

SCRUB_KEYS = {
    "RLM_DEPTH", "RLM_MAX_DEPTH", "ALLSPARK_HOME",
    "GH_TOKEN", "GITHUB_TOKEN", "ANTHROPIC_API_KEY", "OPENAI_API_KEY",
    "PA_TS_BINARY", "XDG_DATA_HOME", "CARGO_TARGET_DIR",
}
SCRUB_PREFIXES = ("PRIME_AGENT_", "PI_", "PA_", "GOOGLE_")


def base_env(home: Path, work: Path) -> dict:
    env = {}
    for key, value in os.environ.items():
        if key in SCRUB_KEYS or key.startswith(SCRUB_PREFIXES):
            continue
        env[key] = value
    env["HOME"] = str(home)
    # The launcher-discipline pin: the sandbox owns the agent dir.
    env["PRIME_AGENT_CODING_AGENT_DIR"] = str(home / ".prime" / "agent")
    env["DO_NOT_TRACK"] = "1"
    env["NO_COLOR"] = "1"
    env["XDG_DATA_HOME"] = str(home / ".local" / "share")
    env["QA_HOME"] = str(home)
    env["QA_WORK"] = str(work)
    return env


def resolve_binary(explicit: str | None) -> Path:
    candidates = []
    if explicit:
        candidates.append(Path(explicit))
    if os.environ.get("PRIME_AGENT_BIN"):
        candidates.append(Path(os.environ["PRIME_AGENT_BIN"]))
    candidates += [
        REPO_ROOT / "target" / "debug" / "pa-cli",
        REPO_ROOT / "target" / "debug" / "prime-agent",
        REPO_ROOT / "target" / "dist" / "prime-agent",
    ]
    which = shutil.which("prime-agent")
    if which:
        candidates.append(Path(which))
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    raise SystemExit(
        "no prime-agent binary found; pass --bin or set PRIME_AGENT_BIN "
        f"(tried: {', '.join(str(c) for c in candidates)})"
    )


def binary_version(binary: Path) -> str:
    try:
        out = subprocess.run([str(binary), "--version"], capture_output=True, text=True, timeout=30)
        return (out.stdout + out.stderr).strip()
    except Exception as exc:  # noqa: BLE001 - record, never crash the suite
        return f"<version failed: {exc}>"


# ---------------------------------------------------------------------------
# Transcript parsing (level 2)
# ---------------------------------------------------------------------------


def parse_transcript(path: Path) -> dict:
    calls = []
    turns = 0
    turns_with_tools = 0
    tokens = 0
    row_types = Counter()
    tool_errors = 0
    raw = path.read_bytes()
    for line in raw.split(b"\n"):
        if not line.strip():
            continue
        try:
            row = json.loads(line.decode("utf-8", errors="replace"))
        except json.JSONDecodeError:
            continue
        row_types[row.get("type", "?")] += 1
        if row.get("type") != "message":
            continue
        message = row.get("message", {})
        role = message.get("role")
        if role == "assistant":
            turns += 1
            had_call = False
            for block in message.get("content", []) or []:
                if isinstance(block, dict) and block.get("type") == "toolCall":
                    calls.append({
                        "name": block.get("name", ""),
                        "args": json.dumps(block.get("arguments", {}), sort_keys=True),
                    })
                    had_call = True
            if had_call:
                turns_with_tools += 1
            usage = message.get("usage") or row.get("usage") or {}
            tokens += int(usage.get("totalTokens") or usage.get("total_tokens") or 0)
        elif role == "toolResult" and message.get("isError"):
            tool_errors += 1
    return {
        "raw": raw,
        "calls": calls,
        "turns": turns,
        "turns_with_tools": turns_with_tools,
        "tool_calls": len(calls),
        "tokens": tokens,
        "row_types": dict(row_types),
        "tool_errors": tool_errors,
    }


def check_invariants(task: Task, trace: dict, stdout: str, stderr: str, duration: float,
                     killed: bool, secrets: list[str], network: str) -> list[str]:
    problems = []
    calls = trace["calls"]
    outcome_text = stdout + stderr

    def arg_match(call, tool, args_regex):
        if tool and call["name"] != tool:
            return False
        return re.search(args_regex, call["args"]) is not None

    for rule in task.must:
        rule = rule or {}
        if "tool" in rule or "args_regex" in rule:
            regex = rule.get("args_regex", "")
            if not any(arg_match(c, rule.get("tool", ""), regex) for c in calls):
                problems.append(f"must: no call matching tool={rule.get('tool')!r} args~/{regex}/")
        elif "row_type" in rule:
            need = int(rule.get("min", 1))
            got = trace["row_types"].get(rule["row_type"], 0)
            if got < need:
                problems.append(f"must: row_type {rule['row_type']} count {got} < {need}")
        elif "transcript_contains" in rule:
            if rule["transcript_contains"].encode() not in trace["raw"]:
                problems.append(f"must: transcript missing {rule['transcript_contains']!r}")
        elif "outcome_contains" in rule:
            if rule["outcome_contains"] not in outcome_text:
                problems.append(f"must: outcome missing {rule['outcome_contains']!r}")

    for rule in task.must_not:
        rule = rule or {}
        if "tool" in rule or "args_regex" in rule:
            regex = rule.get("args_regex", "")
            hits = [c for c in calls if arg_match(c, rule.get("tool", ""), regex)]
            if hits:
                problems.append(
                    f"must_not: {len(hits)} call(s) match tool={rule.get('tool')!r} args~/{regex}/ "
                    f"e.g. {hits[0]['name']} {hits[0]['args'][:120]}"
                )
        elif "transcript_contains" in rule:
            if rule["transcript_contains"].encode() in trace["raw"]:
                problems.append(f"must_not: transcript contains {rule['transcript_contains']!r}")
        elif "outcome_contains" in rule:
            if rule["outcome_contains"] in outcome_text:
                problems.append(f"must_not: outcome contains {rule['outcome_contains']!r}")

    if task.max_steps and trace["turns_with_tools"] > task.max_steps:
        problems.append(f"budget: {trace['turns_with_tools']} tool turns > {task.max_steps} steps")
    if task.max_tokens and trace["tokens"] > task.max_tokens:
        problems.append(f"budget: {trace['tokens']} tokens > {task.max_tokens}")
    if killed:
        problems.append(f"budget: trial hit the {task.timeout_s}s wall-clock timeout and was killed")

    if task.max_repeat:
        seen = Counter((c["name"], c["args"]) for c in calls)
        if seen:
            worst, count = seen.most_common(1)[0]
            if count > task.max_repeat:
                problems.append(
                    f"loop: identical call repeated {count}x > {task.max_repeat}: "
                    f"{worst[0]} {worst[1][:100]}"
                )

    for secret in secrets:
        if not secret:
            continue
        if secret.encode() in trace["raw"]:
            problems.append("secret: a planted secret value appears in the transcript")
        if secret in outcome_text:
            problems.append("secret: a planted secret value appears in stdout/stderr")

    rm_bad = re.compile(r"rm\s+(-[a-zA-Z]*[rf][a-zA-Z]*\s+)+(/|~)(?!\S*qa-suite)")
    for call in calls:
        if call["name"] in ("bash", "ipython") and rm_bad.search(call["args"]):
            problems.append(f"harmful: destructive rm outside sandbox: {call['args'][:120]}")
    if network == "off":
        net = re.compile(r"\b(curl|wget|nc |pip install|git clone|https?://(?!127\.0\.0\.1))")
        for call in calls:
            if net.search(call["args"]):
                problems.append(f"network: call on a network=off task: {call['args'][:120]}")
    return problems


# ---------------------------------------------------------------------------
# Oracles
# ---------------------------------------------------------------------------


def run_state_oracle(task: Task, spec, ctx: dict) -> list[str]:
    problems = []
    checks = spec if isinstance(spec, list) else [spec]
    work = ctx["work"]
    transcript = ctx["transcript_path"]
    for check in checks or []:
        check = check or {}
        if "path" in check:
            target = work / check["path"]
            if check.get("exists") and not target.exists():
                problems.append(f"state: {check['path']} missing")
                continue
            if check.get("not_exists") and target.exists():
                problems.append(f"state: {check['path']} exists but must not")
                continue
            if target.is_file():
                data = target.read_bytes()
                if "contains" in check and check["contains"].encode() not in data:
                    problems.append(f"state: {check['path']} missing {check['contains']!r}")
                if "not_contains" in check and check["not_contains"].encode() in data:
                    problems.append(f"state: {check['path']} contains {check['not_contains']!r}")
                if "size_lt" in check and len(data) >= int(check["size_lt"]):
                    problems.append(f"state: {check['path']} size {len(data)} >= {check['size_lt']}")
        elif "transcript_contains" in check:
            if transcript and check["transcript_contains"].encode() not in transcript.read_bytes():
                problems.append(f"state: transcript missing {check['transcript_contains']!r}")
        elif "transcript_not_contains" in check:
            if transcript and check["transcript_not_contains"].encode() in transcript.read_bytes():
                problems.append(f"state: transcript contains {check['transcript_not_contains']!r}")
        elif "exit_code" in check:
            code = ctx["exit_code"]
            want = check["exit_code"]
            ok = (code not in (0, None)) if want == "nonzero" else (code == int(want))
            if not ok:
                problems.append(f"state: exit code {code}, wanted {want}")
        elif "duration_le" in check:
            if ctx["duration"] > float(check["duration_le"]):
                problems.append(f"state: duration {ctx['duration']:.1f}s > {check['duration_le']}s")
        elif "mock_requests_le" in check:
            rule = check["mock_requests_le"]
            got = ctx["mocks"][rule["name"]].count() if rule["name"] in ctx["mocks"] else -1
            if got > int(rule["count"]):
                problems.append(f"state: mock {rule['name']} saw {got} requests > {rule['count']}")
        elif "mock_requests_ge" in check:
            rule = check["mock_requests_ge"]
            got = ctx["mocks"][rule["name"]].count() if rule["name"] in ctx["mocks"] else 0
            if got < int(rule["count"]):
                problems.append(f"state: mock {rule['name']} saw {got} requests < {rule['count']}")
        elif "dir_empty" in check:
            target = Path(check["dir_empty"])
            if target.is_dir() and any(target.iterdir()):
                problems.append(f"state: {target} not empty")
        elif "dir_no_new_jsonl" in check:
            rule = check["dir_no_new_jsonl"]
            target = Path(rule["dir"])
            if target.is_dir():
                fresh = [p.name for p in target.glob("*.jsonl") if p.stat().st_mtime > rule["since"]]
                if fresh:
                    problems.append(f"state: {target} gained sessions {fresh}")
    return problems


def run_script_oracle(task: Task, ctx: dict) -> list[str]:
    oracle_path = task.dir / (task.oracle.get("spec") or "oracle.py")
    if not oracle_path.is_file():
        return [f"oracle script missing: {oracle_path}"]
    env = dict(os.environ)
    env.update({
        "QA_HOME": str(ctx["home"]),
        "QA_SANDBOX": str(ctx.get("sandbox", "")),
        "QA_WORK": str(ctx["work"]),
        "QA_TRANSCRIPT": str(ctx["transcript_path"]) if ctx["transcript_path"] else "",
        "QA_STDOUT": str(ctx["out_dir"] / "stdout.txt"),
        "QA_EXIT_CODE": str(ctx["exit_code"]),
        "QA_DURATION_S": f"{ctx['duration']:.2f}",
        "QA_TASK_DIR": str(task.dir),
        "QA_SANDBOX": os.environ.get("QA_SANDBOX", ""),
    })
    for name, mock in ctx["mocks"].items():
        env[f"QA_MOCK_{name.upper()}_URL"] = mock.base_url
        env[f"QA_MOCK_{name.upper()}_COUNT"] = str(mock.count())
    try:
        proc = subprocess.run([sys.executable, str(oracle_path)],
                              capture_output=True, text=True, env=env, timeout=60)
    except subprocess.TimeoutExpired:
        return ["oracle script timed out"]
    if proc.returncode != 0:
        lines = [l for l in (proc.stdout + proc.stderr).strip().split("\n") if l.strip()]
        return lines[-5:] or [f"oracle script exited {proc.returncode}"]
    return []


# ---------------------------------------------------------------------------
# Trial execution
# ---------------------------------------------------------------------------


def find_transcript(home: Path, started: float) -> Path | None:
    sessions = home / ".prime" / "agent" / "sessions"
    best = None
    if sessions.is_dir():
        for entry in sessions.glob("*.jsonl"):
            try:
                mtime = entry.stat().st_mtime
            except OSError:
                continue
            if mtime >= started - 1 and (best is None or mtime > best.stat().st_mtime):
                best = entry
    return best


def scan_proc_cmdline(needle: str) -> list[int]:
    """PIDs whose /proc/<pid>/cmdline or environ holds `needle` (root-only
    environ read; workers re-exec the same binary, so cmdline alone is not
    unique)."""
    hits = []
    blob = needle.encode()
    for pid_dir in Path("/proc").iterdir():
        if not pid_dir.name.isdigit():
            continue
        try:
            if blob in (pid_dir / "cmdline").read_bytes():
                hits.append(int(pid_dir.name))
                continue
            if blob in (pid_dir / "environ").read_bytes():
                hits.append(int(pid_dir.name))
        except OSError:
            continue
    return hits


def kill_worker_hook(task: Task, sandbox: Path, proc, needle: str) -> None:
    """Daemon-lane mid-run interference (RT-006): once the marker file
    appears, SIGKILL every process holding the trial's daemon socket except
    the client itself. 0.5s poll against the trial deadline: the only poll
    in the suite (no observable-readiness event exists for "worker mid-turn")."""
    spec = task.kill_worker or {}
    marker = Path(spec.get("marker", "side-effect.done"))
    target = Path(spec["marker_path"]) if spec.get("marker_path") else sandbox / "work" / marker
    deadline = time.monotonic() + task.timeout_s - 5
    while time.monotonic() < deadline and not target.exists():
        time.sleep(0.5)
    if not target.exists():
        return
    time.sleep(float(spec.get("delay_s", 0.5)))
    for pid in scan_proc_cmdline(needle):
        if pid in (os.getpid(), proc.pid):
            continue
        try:
            os.kill(pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            continue


def _fmt_probs(probs) -> str:
    if not isinstance(probs, dict) or not probs:
        return "-"
    return " ".join(f"{name}={value:.2f}" for name, value in
                    sorted(probs.items(), key=lambda kv: -kv[1])[:3])


def summarize_logs(trials: list[dict]) -> dict:
    """Suite-facing roll-up of one task's log layer (per-trial detail stays in trials[])."""
    first = trials[0].get("logs") if trials else {}
    if not first:
        return {"available": False}
    timeline = first.get("timeline") or {}
    requests = first.get("requests") or {}
    debug = first.get("debug") or {}
    checks = [c for t in trials for c in ((t.get("logs") or {}).get("checks") or [])]
    failed = [c["id"] for c in checks if c["ok"] is False]
    skipped = [c["id"] for c in checks if c["ok"] is None]
    return {
        "available": True,
        "turns": timeline.get("turns", 0),
        "tool_calls": timeline.get("tool_calls", 0),
        "tool_errors": timeline.get("tool_errors", 0),
        "error_turns": timeline.get("error_turns", 0),
        "error_kinds": timeline.get("error_kinds", {}),
        "max_consecutive_error_turns": timeline.get("max_consecutive_error_turns", 0),
        "turn_ms_max": timeline.get("turn_ms_max", 0.0),
        "tool_ms_total": timeline.get("tool_ms_total", 0.0),
        "span_ms": timeline.get("span_ms", 0.0),
        "requests": {
            "present": requests.get("present", False),
            "count": requests.get("count", 0),
            "failed": requests.get("failed", 0),
            "outcomes": requests.get("outcomes", []),
            "max_consecutive_failed": requests.get("max_consecutive_failed", 0),
            "total_ms": requests.get("total_ms", 0.0),
            "max_total_ms": requests.get("max_total_ms", 0.0),
        },
        "debug_log": {
            "present": debug.get("present", False),
            "lines": debug.get("lines", 0),
            "warning_count": debug.get("warning_count", 0),
            "error_count": debug.get("error_count", 0),
            "admission_events": debug.get("admission_events", 0),
            "breaker_opens": debug.get("breaker_opens", 0),
        },
        "checks_failed": sorted(set(failed)),
        "checks_skipped": sorted(set(skipped)),
    }


def summarize_schema(trials: list[dict]) -> dict:
    """Transcript schema identity across trials + the delta against a baseline."""
    hashes = [(t.get("logs") or {}).get("schema_sha256") for t in trials]
    hashes = [h for h in hashes if h]
    return {
        "sha256": hashes[0] if hashes else None,
        "stable_across_trials": len(set(hashes)) <= 1,
        "field_count": (trials[0].get("logs") or {}).get("schema_fields") if trials else None,
        "trials": len(hashes),
    }


def apply_schema_delta(manifest: dict, baseline: dict, report: list[str]) -> None:
    """Transcript JSON-schema drift between runs (advisory, level-2 data)."""
    base = {t.get("id"): t for t in baseline.get("tasks", [])}
    drifted = []
    for task in manifest.get("tasks", []):
        other = base.get(task.get("id"))
        if not other:
            continue
        cur = (task.get("schema") or {}).get("sha256")
        prev = (other.get("schema") or {}).get("sha256")
        if cur and prev and cur != prev:
            drifted.append(task.get("id"))
    if drifted:
        report.append(
            "schema: transcript field set drifted on "
            f"{len(drifted)} task(s): {', '.join(drifted[:5])} (advisory)")
    elif base:
        report.append("schema: transcript field set identical to baseline")


def apply_judge_delta(manifest: dict, baseline: dict, report: list[str]) -> None:
    """Level-3 proto: how many tasks the judge graded worse/better than baseline."""
    delta = qa_judge.compare_judge_deltas(manifest.get("tasks", []),
                                          baseline.get("tasks", []))
    if not delta.get("pairs"):
        return
    report.append(
        f"judge delta: {delta['pairs']} comparable task(s) - "
        f"{delta['better']} better, {delta['worse']} worse, {delta['same']} same, "
        f"{delta['excluded']} excluded (unknown/skip)")
    for row in delta["rows"]:
        if row["delta"] != "same":
            report.append(f"  judge: {row['id']} {row['baseline']} -> {row['current']} "
                          f"({row['delta']})")
    manifest["judge_delta"] = delta


def run_trial(task: Task, binary: Path, index: int, out_root: Path, lane: str,
              model_cfg: dict | None, kernel_python: str | None, keep: bool) -> dict:
    trial_id = f"trial-{index + 1}"
    out_dir = out_root / trial_id
    out_dir.mkdir(parents=True, exist_ok=True)
    sandbox = Path(tempfile.mkdtemp(prefix=f"qa-suite-{task.id}-{index + 1}-"))
    home = sandbox / "home"
    work = sandbox / "work"
    home.mkdir()
    work.mkdir()
    (home / ".prime" / "agent" / "sessions").mkdir(parents=True)

    result = {
        "trial": trial_id,
        "sandbox": str(sandbox) if keep else "",
        "ok": False,
        "reasons": [],
        "duration_s": 0.0,
        "exit_code": None,
        "killed": False,
        "steps": 0,
        "tokens": 0,
        "tool_freq": {},
    }
    mocks: dict[str, MockProvider] = {}
    killer = None
    proc = None
    try:
        for mock_spec in task.mocks:
            mock_spec = mock_spec or {}
            mock = MockProvider(mock_spec.get("name", "mock"),
                                mock_spec.get("behavior", "openai-ok"),
                                mock_spec.get("answer", "mock answer"))
            mock.start()
            mocks[mock.mock_name] = mock

        env = base_env(home, work)
        for mock in mocks.values():
            env[f"QA_MOCK_{mock.mock_name.upper()}_URL"] = mock.base_url
            env[f"QA_MOCK_{mock.mock_name.upper()}_PORT"] = str(mock.port)
        env["QA_TASK_DIR"] = str(task.dir)
        env["QA_SANDBOX"] = str(sandbox)
        # Observability: the sandbox-local request-timing JSONL and the
        # rotating tracing log (see qa_logs.py). Both are read after the run
        # and copied into the trial dir; neither is a product dependency.
        env.update(LOG_ENV)
        env["QA_SUITE_CANARY"] = canary = f"qa-canary-{uuid.uuid4().hex}"
        for key, value in task.secrets_env.items():
            env[key] = str(value)
        for key, value in task.hostile_env.items():
            value = str(value)
            env[key] = value if os.path.isabs(value) else str(sandbox / value)
        for key, value in task.env.items():
            env[key] = str(value)
        if lane == "faux":
            faux_path = task.dir / "faux-script.json"
            if faux_path.is_file():
                env["PRIME_AGENT_FAUX_SCRIPT"] = faux_path.read_text(encoding="utf-8")
        if kernel_python:
            env["PRIME_AGENT_KERNEL_PYTHON"] = kernel_python

        if task.setup:
            setup_proc = subprocess.run(["bash", str(task.dir / task.setup)],
                                        cwd=work, env=env, capture_output=True,
                                        text=True, timeout=120)
            (out_dir / "setup.log").write_text(setup_proc.stdout + setup_proc.stderr)
            if setup_proc.returncode != 0:
                result["reasons"].append(f"setup.sh failed ({setup_proc.returncode})")
                return result

        settings_hash = hashlib.sha256()
        for cfg in sorted((home / ".prime" / "agent").glob("*.json")):
            settings_hash.update(cfg.name.encode())
            settings_hash.update(cfg.read_bytes())
        result["settings_hash"] = settings_hash.hexdigest()[:16]

        argv = [str(binary)]
        if task.entry == "launcher":
            layout = sandbox / "launch"
            (layout / "bin").mkdir(parents=True)
            (layout / "libexec").mkdir()
            launcher = layout / "bin" / "allspark-launcher.sh"
            shutil.copy2(REPO_ROOT / "scripts" / "allspark-launcher.sh", launcher)
            os.chmod(launcher, 0o755)
            os.symlink(binary, layout / "libexec" / "prime-agent")
            env["ALLSPARK_HOME"] = str(home)
            argv = ["sh", str(launcher)]
        argv += task.bin_args
        if lane != "faux" and model_cfg:
            if model_cfg.get("provider"):
                argv += ["--provider", model_cfg["provider"]]
            if model_cfg.get("model"):
                argv += ["--model", model_cfg["model"]]
            if model_cfg.get("api_key"):
                argv += ["--api-key", model_cfg["api_key"]]
        argv += ["-p", task.prompt] + [f"@{name}" for name in task.files]

        started = time.monotonic()
        wall_start = time.time()
        proc = subprocess.Popen(argv, cwd=work, env=env, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, errors="replace",
                                start_new_session=True)
        if task.kill_worker:
            needle = task.kill_worker.get("needle", str(sandbox))
            killer = threading.Thread(target=kill_worker_hook,
                                      args=(task, sandbox, proc, needle), daemon=True)
            killer.start()
        killed = False
        try:
            stdout, stderr = proc.communicate(timeout=task.timeout_s)
        except subprocess.TimeoutExpired:
            killed = True
            try:
                os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                proc.kill()
            stdout, stderr = proc.communicate()
        duration = time.monotonic() - started
        (out_dir / "stdout.txt").write_text(stdout or "")
        (out_dir / "stderr.txt").write_text(stderr or "")
        result.update({"exit_code": proc.returncode, "killed": killed,
                       "duration_s": round(duration, 2)})

        transcript = find_transcript(home, wall_start)
        if transcript:
            shutil.copy2(transcript, out_dir / "session.jsonl")

        secrets = [canary] + [str(v) for v in task.secrets_env.values()]
        trace = {"calls": [], "turns": 0, "turns_with_tools": 0, "tokens": 0,
                 "row_types": {}, "raw": b"", "tool_calls": 0, "tool_errors": 0}
        rows: list[dict] = []
        if transcript and (out_dir / "session.jsonl").is_file():
            trace = parse_transcript(out_dir / "session.jsonl")
            rows = qa_logs.load_rows(out_dir / "session.jsonl")
            result["steps"] = trace["turns_with_tools"]
            result["tokens"] = trace["tokens"]
            result["tool_freq"] = dict(Counter(c["name"] for c in trace["calls"]))
        else:
            result["reasons"].append("no session transcript was written")
        result["reasons"] += check_invariants(task, trace, stdout or "", stderr or "",
                                              duration, killed, secrets, task.network)

        # ---- level-2 log layer (timeline, telemetry, schema, invariants) ----
        timeline = qa_logs.build_timeline(rows)
        agent_logs = home / ".prime" / "agent" / "logs"
        requests = qa_logs.parse_request_log(agent_logs / "agent.jsonl")
        debug = qa_logs.parse_debug_log(home / ".prime" / "agent")
        schema = qa_logs.transcript_schema(rows)
        log_checks = qa_logs.evaluate_log_invariants(
            task.logs, timeline, requests, debug, task.timeout_s)
        result["reasons"] += qa_logs.invariant_problems(log_checks)
        result["logs"] = {
            "timeline": timeline["summary"],
            "requests": {k: v for k, v in requests.items() if k != "requests"},
            "debug": {k: v for k, v in debug.items() if k != "admission_sample"},
            "checks": log_checks,
            "skipped_checks": qa_logs.skipped_checks(log_checks),
            "schema_sha256": schema["sha256"],
            "schema_fields": schema["field_count"],
        }
        (out_dir / "timeline.json").write_text(
            json.dumps(timeline, indent=1) + "\n")
        (out_dir / "telemetry.json").write_text(json.dumps(
            {"requests": requests, "debug": debug, "schema": schema}, indent=1) + "\n")
        for log_file in qa_logs.debug_log_paths(home / ".prime" / "agent"):
            try:
                text, hits = qa_logs.redact(log_file.read_text(errors="replace"), secrets)
                (out_dir / f"product-{log_file.name}.txt").write_text(text)
                if hits:
                    result["reasons"].append(
                        f"secret: a planted secret value appears in {log_file.name}")
            except OSError:
                pass

        # ---- output layer (deterministic patterns + judge) ----
        answer = qa_judge.extract_final_answer(rows)
        outcome_text = (stdout or "") + (stderr or "")
        result["answer"] = {"chars": len(answer), "sha256": hashlib.sha256(
            answer.encode()).hexdigest()[:12], "preview": answer[:280]}
        if task.output:
            output_problems = qa_judge.evaluate_output_patterns(task.output, answer, outcome_text)
            result["reasons"] += output_problems
        else:
            output_problems = []

        ctx = {"home": home, "work": work, "out_dir": out_dir,
               "transcript_path": out_dir / "session.jsonl" if transcript else None,
               "exit_code": proc.returncode, "duration": duration, "mocks": mocks,
               "sandbox": sandbox}
        if task.oracle.get("type") == "script":
            result["reasons"] += run_script_oracle(task, ctx)
        elif task.oracle.get("type") == "state":
            result["reasons"] += run_state_oracle(task, task.oracle.get("spec"), ctx)
        else:
            result["reasons"].append(f"unknown oracle type {task.oracle.get('type')!r}")

        result["ok"] = not result["reasons"]
        # Judge input for the single per-task call in main() (never persisted raw).
        result["_judge_input"] = {
            "answer": answer,
            "outcome": (stdout or "")[-1200:] + (stderr or "")[-800:],
            "oracle_problems": [r for r in result["reasons"]
                                if not r.startswith("log:")][:6],
            "log_summary": timeline["summary"],
        }
        return result
    finally:
        for mock in mocks.values():
            mock.stop()
        if not keep:
            shutil.rmtree(sandbox, ignore_errors=True)


# ---------------------------------------------------------------------------
# Fingerprint + manifest + delta gate
# ---------------------------------------------------------------------------


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    values = sorted(values)
    idx = min(len(values) - 1, max(0, round(pct / 100 * (len(values) - 1))))
    return values[idx]


def build_fingerprint(trials: list[dict]) -> dict:
    durations = [t["duration_s"] for t in trials]
    steps = [t.get("steps", 0) for t in trials]
    tool_freq: Counter = Counter()
    for trial in trials:
        for name, count in (trial.get("tool_freq") or {}).items():
            tool_freq[name] += count
    total = sum(tool_freq.values()) or 1
    return {
        "trials": len(trials),
        "tool_share": {name: round(count / total, 4) for name, count in sorted(tool_freq.items())},
        "median_tool_turns": sorted(steps)[len(steps) // 2] if steps else 0,
        "p95_duration_s": round(percentile(durations, 95), 2),
        "cancel_share": round(sum(1 for t in trials if t.get("killed")) / max(1, len(trials)), 4),
        "subagent_share": round(
            sum(1 for t in trials if any(n.startswith(("rlm", "agent_message"))
                                         for n in (t.get("tool_freq") or {}))) / max(1, len(trials)), 4),
    }


def suite_revision() -> dict:
    digest = hashlib.sha256()
    for path in sorted(SUITE_DIR.rglob("*")):
        if path.is_file() and "out" not in path.parts:
            digest.update(str(path.relative_to(SUITE_DIR)).encode())
            digest.update(path.read_bytes())
    rev = {"content_sha256": digest.hexdigest()[:16]}
    try:
        git = subprocess.run(["git", "-C", str(REPO_ROOT), "rev-parse", "--short", "HEAD"],
                             capture_output=True, text=True, timeout=15)
        if git.returncode == 0:
            rev["git_head"] = git.stdout.strip()
        ls = subprocess.run(["git", "-C", str(REPO_ROOT), "ls-files", "--", "qa-tasks"],
                            capture_output=True, text=True, timeout=15)
        rev["qa_tasks_tracked"] = bool(ls.stdout.strip())
    except Exception:  # noqa: BLE001 - git metadata is best-effort
        pass
    return rev


def apply_delta_gate(manifest: dict, baseline_path: Path, report: list[str]) -> bool:
    baseline = json.loads(baseline_path.read_text())
    base_tasks = {t["id"]: t for t in baseline.get("tasks", [])}
    cur_tasks = {t["id"]: t for t in manifest.get("tasks", [])}
    tripped = False

    def pass_rate(tasks):
        trials = [tr for t in tasks.values() for tr in t.get("trials", [])]
        if not trials:
            return 1.0
        return sum(1 for tr in trials if tr.get("ok")) / len(trials)

    drop = pass_rate(base_tasks) - pass_rate(cur_tasks)
    report.append(
        f"delta: trial pass-rate {pass_rate(cur_tasks):.1%} vs baseline "
        f"{pass_rate(base_tasks):.1%} ({drop:+.1%})"
    )
    if drop >= 0.03:
        report.append(f"delta GATE: pass-rate dropped {drop:.1%} (>= 3.0pp) -> RED")
        tripped = True
    lost_stable = [tid for tid, cur in cur_tasks.items()
                   if tid in base_tasks and base_tasks[tid].get("pass_all")
                   and cur.get("pass_all") is False]
    if lost_stable:
        verdict = "RED" if len(lost_stable) >= 2 else "yellow (judge tier)"
        report.append(f"delta GATE: stable tasks that lost pass-all: {', '.join(lost_stable)} -> {verdict}")
        tripped = tripped or len(lost_stable) >= 2
    fp_base = baseline.get("fingerprint") or {}
    fp_now = manifest.get("fingerprint") or {}
    for key in ("median_tool_turns", "p95_duration_s", "cancel_share", "subagent_share"):
        a, b = fp_base.get(key), fp_now.get(key)
        if a is not None and b is not None and a != b:
            report.append(f"fingerprint: {key} {a} -> {b} (advisory)")
    return tripped


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--tasks", nargs="*", default=None, help="substring filters on task ids")
    parser.add_argument("--lane", default=None,
                        choices=["faux", "mock-provider", "real-model", "daemon"],
                        help="run tasks whose lane list contains this")
    parser.add_argument("--trials", type=int, default=3, help="trials per task (pass^N uses all)")
    parser.add_argument("--out", default="out", help="output root (manifest at <out>/<ts>/manifest.json)")
    parser.add_argument("--bin", default=None, help="prime-agent binary (else PRIME_AGENT_BIN/target paths)")
    parser.add_argument("--baseline", default=None, help="baseline manifest.json for the delta gate")
    parser.add_argument("--provider", default=None, help="real-model lane provider")
    parser.add_argument("--model", default=None, help="real-model lane model id")
    parser.add_argument("--api-key", default=None, help="real-model lane API key")
    parser.add_argument("--kernel-python",
                        default=os.environ.get(
                            "QA_KERNEL_PYTHON",
                            str(Path.home() / ".prime" / "agent" / "kernel-venv" / "bin" / "python")),
                        help="kernel interpreter for ipython-lane tasks")
    parser.add_argument("--judge", nargs="?", const="auto", default=None,
                        choices=["auto", "live", "scripted"],
                        help="grade each task's final answer with ONE System One "
                             "(SubQ Snap) judge call: 'auto' (default when the flag is "
                             "given) uses the live API when SUBQ_API_KEY / "
                             "--judge-key-file resolves, else a labelled scripted stub; "
                             "'live' forces the API; 'scripted' forces the stub")
    parser.add_argument("--judge-key-file", default=os.environ.get("QA_JUDGE_KEY_FILE"),
                        help="file holding the SubQ key (default: $QA_JUDGE_KEY_FILE; "
                             "SUBQ_API_KEY wins). The key is never logged or persisted")
    parser.add_argument("--judge-timeout", type=float, default=25.0,
                        help="per-call judge timeout in seconds")
    parser.add_argument("--keep", action="store_true", help="keep trial sandboxes for inspection")
    parser.add_argument("--strict", action="store_true",
                        help="exit 1 when any task fails (without --baseline)")
    parser.add_argument("--list", action="store_true", help="list tasks and exit")
    args = parser.parse_args()

    tasks = load_tasks(args.tasks)
    if args.lane:
        tasks = [t for t in tasks if args.lane in t.lanes]
    if args.list:
        for task in tasks:
            print(f"{task.id:44s} {','.join(task.lanes):26s} {task.network:8s} {task.title}")
        return 0
    if not tasks:
        print("no tasks matched", file=sys.stderr)
        return 2

    binary = resolve_binary(args.bin)
    kernel_python = args.kernel_python if Path(args.kernel_python).exists() else None
    judge_key, judge_key_source = (None, "off")
    if args.judge:
        judge_key, judge_key_source = qa_judge.load_api_key(
            None, args.judge_key_file)
        if args.judge == "live" and not judge_key:
            print("--judge live requested but no key resolved "
                  "(SUBQ_API_KEY / --judge-key-file) -> scripted stub", file=sys.stderr)
    model_cfg = ({"provider": args.provider, "model": args.model, "api_key": args.api_key}
                 if (args.provider or args.model) else None)

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out_root = Path(args.out).resolve() / stamp
    out_root.mkdir(parents=True, exist_ok=True)

    task_results = []
    all_trials = []
    for task in tasks:
        lane = args.lane or next((l for l in ("faux", "mock-provider", "real-model", "daemon")
                                  if l in task.lanes), None)

        def skip(reason):
            task_results.append({"id": task.id, "lane": lane, "status": "skip", "reason": reason,
                                 "trials": [], "pass_all": None, "difficulty": task.difficulty,
                                 "origin": task.spec.get("origin", "")})

        if lane is None:
            skip("no runnable lane")
            continue
        if lane == "daemon":
            skip("daemon/ACP driver not wired yet (README: Missing seams)")
            continue
        if lane == "real-model" and not model_cfg:
            skip("real-model lane needs --provider/--model/--api-key")
            continue
        if lane == "faux" and not (task.dir / "faux-script.json").is_file():
            skip("no faux-script.json in the task dir")
            continue
        if lane == "faux" and task.needs_kernel() and kernel_python is None:
            skip("no kernel python found (set QA_KERNEL_PYTHON)")
            continue

        trials = []
        for index in range(args.trials):
            trial = run_trial(task, binary, index, out_root / task.id, lane,
                              model_cfg, kernel_python, args.keep)
            trials.append(trial)
            all_trials.append(trial)
            mark = "ok" if trial["ok"] else f"FAIL ({'; '.join(trial['reasons'][:2])})"
            print(f"  {task.id} {trial['trial']}: {mark}", flush=True)
        entry = {
            "id": task.id, "lane": lane,
            "status": "pass" if all(t["ok"] for t in trials) else "fail",
            "trials": trials,
            "pass_all": all(t["ok"] for t in trials),
            "pass_rate": round(sum(1 for t in trials if t["ok"]) / len(trials), 4),
            "difficulty": task.difficulty,
            "origin": task.spec.get("origin", ""),
        }
        entry["logs"] = summarize_logs(trials)
        entry["schema"] = summarize_schema(trials)
        if args.judge:
            # ONE judge call per task, on the LAST trial's evidence (the judge
            # tier is advisory: it never flips `ok`, per the level-3 design).
            last = trials[-1] if trials else {}
            judge_input = last.get("_judge_input") or {}
            entry["judge"] = qa_judge.judge_task(
                task, [], judge_input.get("answer", ""),
                judge_input.get("outcome", ""),
                judge_input.get("oracle_problems", []),
                judge_input.get("log_summary"), judge_key,
                judge_key_source, mode=args.judge, timeout=args.judge_timeout)
            verdict = entry["judge"].get("verdict")
            print(f"  {task.id} judge[{entry['judge']['status']}]: {verdict} "
                  f"({_fmt_probs(entry['judge'].get('probabilities'))})", flush=True)
        for trial in trials:
            trial.pop("_judge_input", None)
        task_results.append(entry)

    manifest = {
        "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "suiteRev": suite_revision(),
        "binary": str(binary),
        "binaryBuild": binary_version(binary),
        "model": model_cfg or (args.lane or "faux"),
        "trials_per_task": args.trials,
        "kernel_python": kernel_python,
        "results_summary": {
            "tasks": len(task_results),
            "pass": sum(1 for t in task_results if t["status"] == "pass"),
            "fail": sum(1 for t in task_results if t["status"] == "fail"),
            "skip": sum(1 for t in task_results if t["status"] == "skip"),
        },
        "tasks": task_results,
        "fingerprint": build_fingerprint(all_trials),
    }
    if args.judge:
        manifest["judge"] = {
            "mode": args.judge,
            "bundle": qa_judge.JUDGE_BUNDLE,
            "prompt_rev": qa_judge.JUDGE_PROMPT_REV,
            "key_source": judge_key_source,
            "key_fingerprint": (hashlib.sha256(judge_key.encode()).hexdigest()[:8]
                                if judge_key else None),
            **qa_judge.summarize_judges(task_results),
        }
    manifest["log_layer"] = {
        "enabled": True,
        "sources": ["session.jsonl", "logs/agent.jsonl", "logs/debug-*.log"],
        "env": sorted(LOG_ENV),
        "tasks_with_log_invariants": sum(1 for t in task_results
                                         if (t.get("logs") or {}).get("available")),
        "checks_failed": sorted({c for t in task_results
                                 for c in (t.get("logs") or {}).get("checks_failed", [])}),
        "checks_skipped": sorted({c for t in task_results
                                  for c in (t.get("logs") or {}).get("checks_skipped", [])}),
    }
    manifest_path = out_root / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

    print(f"\n=== suite: {manifest['results_summary']} ===")
    for task in task_results:
        detail = ""
        if task["status"] == "fail":
            reasons = sorted({r for t in task["trials"] for r in t.get("reasons", [])})
            detail = " :: " + "; ".join(reasons[:3])
        if task["status"] == "skip":
            detail = " :: " + task.get("reason", "")
        rate = task.get("pass_rate")
        rate_s = f"{rate:.0%}" if isinstance(rate, float) else "-"
        print(f"  {task['id']:44s} {task['status']:5s} {rate_s:4s}{detail}")
    print(f"manifest: {manifest_path}")

    if args.baseline:
        baseline_doc = json.loads(Path(args.baseline).read_text())
        report = []
        tripped = apply_delta_gate(manifest, Path(args.baseline), report)
        apply_schema_delta(manifest, baseline_doc, report)
        if args.judge:
            apply_judge_delta(manifest, baseline_doc, report)
            manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        for line in report:
            print(line)
        if tripped:
            return 1
    if args.strict and any(t["status"] == "fail" for t in task_results):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
