#!/usr/bin/env python3
"""Log analysis for the Real-Task Deviation Suite (level-2 expansion).

Three observability sources, one normalizer each, all stdlib-only and all
derived from artifacts the product already writes inside the trial sandbox:

  * ``session.jsonl``        the append-only transcript (always present)
  * ``logs/agent.jsonl``     request-timing JSONL (``PI_REQUEST_TIMING=1``)
  * ``logs/debug-*.log``     the fork's rotating tracing log
                             (``ALLSPARK_DEBUG=1`` + ``ALLSPARK_DEBUG_FILTER``)

The transcript is normalized into a TIMELINE of events (turn start/end, tool
calls with durations, provider errors, compaction rows) exactly like the
``AgentActivityIndex`` pattern from ``.research/omp-debugger.md`` §6: one
normalizer, offline reading, no live hooks. The timeline is the substrate for
the log invariants (error storms, retry budgets, turn durations) and for the
transcript-schema stability delta carried in the run manifest.

Nothing here asserts on prose: the invariants are counts, durations and
schema paths. Anything that needs judgement belongs to the judge tier
(``qa_judge.py``).
"""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter
from pathlib import Path

# ---------------------------------------------------------------------------
# Time helpers
# ---------------------------------------------------------------------------

_ISO_RE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?Z$")


def iso_to_ms(text: str) -> int | None:
    """Parse the transcript's ISO-8601 UTC stamp to epoch ms (no tz database)."""
    match = _ISO_RE.match((text or "").strip())
    if not match:
        return None
    year, month, day, hour, minute, second, frac = match.groups()
    import calendar

    base = calendar.timegm((int(year), int(month), int(day),
                            int(hour), int(minute), int(second), 0, 0, 0))
    millis = int((frac or "0")[:3].ljust(3, "0"))
    return base * 1000 + millis


def _row_ms(row: dict) -> int | None:
    """Row time: the message-level epoch ms wins, the row ISO stamp is the fallback."""
    message = row.get("message")
    if isinstance(message, dict):
        stamp = message.get("timestamp")
        if isinstance(stamp, (int, float)):
            return int(stamp)
    return iso_to_ms(row.get("timestamp", ""))


# ---------------------------------------------------------------------------
# Transcript -> timeline
# ---------------------------------------------------------------------------

def load_rows(path: Path) -> list[dict]:
    rows = []
    if not path or not Path(path).is_file():
        return rows
    for line in Path(path).read_bytes().split(b"\n"):
        if not line.strip():
            continue
        try:
            row = json.loads(line.decode("utf-8", errors="replace"))
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def _digest(value) -> str:
    blob = json.dumps(value, sort_keys=True, default=str).encode()
    return hashlib.sha256(blob).hexdigest()[:8]


def _blocks(message: dict) -> list[dict]:
    content = message.get("content")
    return [b for b in content if isinstance(b, dict)] if isinstance(content, list) else []


def _text_len(blocks: list[dict]) -> int:
    return sum(len(b.get("text") or "") for b in blocks if b.get("type") == "text")


def build_timeline(rows: list[dict]) -> dict:
    """Normalize transcript rows into ordered events plus a summary.

    Events carry a millisecond offset from the first row so a trial reads as a
    timeline (``t`` = ms since session start). Tool durations come from the
    toolCall -> toolResult message stamps; a missing pair leaves ``ms`` null
    rather than guessing.
    """
    base = None
    for row in rows:
        stamp = _row_ms(row)
        if stamp is not None:
            base = stamp
            break
    base = base or 0

    events: list[dict] = []
    pending_turns: list[tuple[int, int]] = []
    tool_opens: dict[str, dict] = {}
    turn = 0
    turn_start_ms: int | None = None
    open_calls: list[dict] = []
    error_turns: list[int] = []
    consecutive = 0
    max_consecutive = 0
    turn_durations: list[float] = []
    tool_durations: list[float] = []
    provider_errors: Counter = Counter()

    def push(event_kind: str, at: int | None, **fields):
        event = {"kind": event_kind, "t": None if at is None else at - base}
        event.update(fields)
        events.append(event)
        return event

    for row in rows:
        stamp = _row_ms(row)
        rtype = row.get("type")
        if rtype == "session":
            push("session_start", stamp, version=row.get("version"),
                 rlm_depth=row.get("rlmDepth", 0))
            continue
        if rtype == "model_change":
            push("model_change", stamp, provider=row.get("provider"),
                 model=row.get("modelId"))
            continue
        if rtype in ("thinking_level_change", "service_tier_change"):
            push(rtype, stamp,
                 value=row.get("thinkingLevel") or row.get("serviceTier"))
            continue
        if rtype == "compaction":
            push("compaction", stamp,
                 tokens_before=row.get("tokensBefore"),
                 summary_chars=len(str(row.get("summary") or "")))
            continue
        if rtype == "custom_message":
            push("custom_message", stamp, custom_type=row.get("customType"),
                 chars=len(str(row.get("content") or "")))
            continue
        if rtype != "message":
            push(f"row:{rtype}", stamp)
            continue

        message = row.get("message") or {}
        role = message.get("role")
        blocks = _blocks(message)

        if role == "user":
            push("user_message", stamp, chars=_text_len(blocks), blocks=len(blocks))
            continue
        if role == "toolResult":
            tool_name = message.get("toolName") or "?"
            call_id = message.get("toolCallId") or ""
            opened = tool_opens.pop(call_id, None)
            duration = None
            if opened and opened.get("_at") is not None and stamp is not None:
                duration = float(stamp - opened["_at"])
                tool_durations.append(duration)
            push("tool_result", stamp, tool=tool_name, is_error=bool(message.get("isError")),
                 chars=_text_len(blocks), ms=duration)
            continue
        if role == "compactionSummary":
            push("compaction_summary", stamp, chars=_text_len(blocks))
            continue
        if role != "assistant":
            push(f"message:{role}", stamp)
            continue

        # Assistant turn: one model response (may carry tool calls).
        turn += 1
        turn_start_ms = stamp
        calls = [b for b in blocks if b.get("type") == "toolCall"]
        stop = message.get("stopReason")
        diagnostics = message.get("diagnostics") or []
        push("turn_start", stamp, turn=turn, blocks=len(blocks),
             text_chars=_text_len(blocks), tool_calls=len(calls))

        for call in calls:
            entry = push("tool_call", stamp, turn=turn, tool=call.get("name") or "?",
                         args_digest=_digest(call.get("arguments") or {}),
                         args_preview=json.dumps(call.get("arguments") or {},
                                                 sort_keys=True)[:120])
            entry["_at"] = stamp
            tool_opens[call.get("id") or _digest(entry["args_preview"])] = entry
            open_calls.append(entry)

        if stop == "error" or message.get("errorMessage"):
            kind = "unknown"
            for diag in diagnostics:
                if isinstance(diag, dict):
                    details = diag.get("details") or {}
                    kind = details.get("kind") or diag.get("type") or kind
                    break
            provider_errors[kind] += 1
            push("error", stamp, turn=turn, scope="provider", kind=kind,
                 message=(message.get("errorMessage") or "")[:240])
            consecutive += 1
            max_consecutive = max(max_consecutive, consecutive)
            error_turns.append(turn)
        else:
            consecutive = 0

        usage = message.get("usage") or {}
        # A turn's cost is model latency PLUS the tool round trip that follows
        # it, so its duration is closed by the NEXT assistant row (or the last
        # row of the session), not by its own timestamp.
        if turn_start_ms is not None:
            pending_turns.append((turn, turn_start_ms))
        push("turn_end", stamp, turn=turn, stop=stop,
             tokens=int(usage.get("totalTokens") or 0))
        turn_start_ms = None

    # Close turn durations: each turn ends when the next turn starts, and the
    # last one at the final transcript row (model latency + tool round trip).
    last_ms = None
    for row in reversed(rows):
        last_ms = _row_ms(row)
        if last_ms is not None:
            break
    for index, (turn_no, start) in enumerate(pending_turns):
        end = (pending_turns[index + 1][1] if index + 1 < len(pending_turns)
               else (last_ms if last_ms is not None else start))
        duration = float(max(0, end - start))
        turn_durations.append(duration)
        for event in events:
            if event["kind"] == "turn_end" and event.get("turn") == turn_no:
                event["ms"] = duration
                break

    for event in events:
        event.pop("_at", None)

    summary = {
        "rows": len(rows),
        "events": len(events),
        "turns": turn,
        "tool_calls": sum(1 for e in events if e["kind"] == "tool_call"),
        "tool_errors": sum(1 for e in events if e["kind"] == "tool_result" and e["is_error"]),
        "error_turns": len(error_turns),
        "error_kinds": dict(provider_errors),
        "max_consecutive_error_turns": max_consecutive,
        "compaction_rows": sum(1 for e in events if e["kind"] == "compaction"),
        "turn_ms_max": round(max(turn_durations), 1) if turn_durations else 0.0,
        "turn_ms_median": round(sorted(turn_durations)[len(turn_durations) // 2], 1)
        if turn_durations else 0.0,
        "tool_ms_max": round(max(tool_durations), 1) if tool_durations else 0.0,
        "tool_ms_total": round(sum(tool_durations), 1) if tool_durations else 0.0,
        "span_ms": round(max((e["t"] or 0) for e in events), 1) if events else 0.0,
    }
    return {"events": events, "summary": summary}


# ---------------------------------------------------------------------------
# Transcript schema (field-set stability between runs)
# ---------------------------------------------------------------------------

def _walk_schema(value, prefix: str, out: set, depth: int = 0):
    if depth > 6:
        return
    if isinstance(value, dict):
        for key, child in value.items():
            path = f"{prefix}.{key}" if prefix else str(key)
            out.add(path)
            _walk_schema(child, path, out, depth + 1)
    elif isinstance(value, list):
        path = f"{prefix}[]"
        out.add(path)
        for item in value[:3]:
            _walk_schema(item, path, out, depth + 1)
    else:
        out.add(f"{prefix}:{type(value).__name__}")


def transcript_schema(rows: list[dict], max_fields: int = 600) -> dict:
    """Union of JSON paths across rows: the transcript's wire shape.

    A field that disappears between two runs of the same task is a contract
    drift signal (a row lost ``usage.totalTokens``, a diagnostic lost its
    ``details.kind``) that no state oracle would notice.
    """
    fields: set[str] = set()
    per_type: dict[str, set] = {}
    for row in rows:
        _walk_schema(row, "", fields)
        rtype = str(row.get("type", "?"))
        _walk_schema(row, "", per_type.setdefault(rtype, set()))
    ordered = sorted(fields)[:max_fields]
    digest = hashlib.sha256("\n".join(ordered).encode()).hexdigest()[:16]
    return {
        "sha256": digest,
        "field_count": len(fields),
        "fields": ordered,
        "truncated": len(fields) > max_fields,
        "row_types": {key: len(value) for key, value in sorted(per_type.items())},
    }


def compare_schemas(current: dict, baseline: dict) -> dict:
    """Field-set delta: missing fields (regression) vs added fields (informational)."""
    if not current or not baseline:
        return {"comparable": False}
    cur = set(current.get("fields") or [])
    base = set(baseline.get("fields") or [])
    missing = sorted(base - cur)
    added = sorted(cur - base)
    return {
        "comparable": True,
        "missing": missing[:40],
        "missing_count": len(missing),
        "added": added[:40],
        "added_count": len(added),
        "stable": not missing,
    }


# ---------------------------------------------------------------------------
# Request-timing JSONL (logs/agent.jsonl)
# ---------------------------------------------------------------------------

def parse_request_log(path: Path) -> dict:
    """One record per provider request from the request-timing log.

    ``stream-done`` rows carry the outcome (``done``/``failed``), the stop
    reason, the error message and the phase split, so retry ladders are
    visible without guessing from the transcript (a retried request leaves a
    single assistant row).
    """
    rows = load_rows(path) if path else []
    requests: list[dict] = []
    current: dict | None = None
    for row in rows:
        phase = row.get("phase")
        seq = row.get("requestSeq")
        if phase == "prompt-built":
            current = {"seq": seq, "phases": {}, "ts": row.get("ts")}
            requests.append(current)
            continue
        if current is None or current.get("seq") != seq:
            current = {"seq": seq, "phases": {}, "ts": row.get("ts")}
            requests.append(current)
        current["phases"][phase] = row.get("phaseMs")
        if phase == "stream-done":
            current.update({
                "outcome": row.get("outcome"),
                "stop": row.get("stopReason"),
                "error": (row.get("errorMessage") or "")[:240],
                "total_ms": row.get("totalMs"),
                "request_bytes": row.get("requestBytes"),
                "context_entries": row.get("contextEntries"),
                "usage": row.get("usage"),
            })
            current = None

    outcomes = [r.get("outcome") or "?" for r in requests]
    failures = [r for r in requests if r.get("outcome") == "failed"]
    consecutive = peak = 0
    for request in requests:
        if request.get("outcome") == "failed":
            consecutive += 1
            peak = max(peak, consecutive)
        else:
            consecutive = 0
    first_token = [r["phases"].get("first-byte") for r in requests
                   if isinstance(r.get("phases"), dict) and r["phases"].get("first-byte") is not None]
    return {
        "present": bool(rows),
        "count": len(requests),
        "failed": len(failures),
        "outcomes": outcomes,
        "max_consecutive_failed": peak,
        "total_ms": round(sum(float(r.get("total_ms") or 0) for r in requests), 1),
        "max_total_ms": round(max([float(r.get("total_ms") or 0) for r in requests] or [0]), 1),
        "first_byte_ms_max": round(max([float(v) for v in first_token] or [0]), 1),
        "errors": [r.get("error") for r in failures][:5],
        "requests": requests[:40],
    }


# ---------------------------------------------------------------------------
# Rotating debug log (logs/debug-<component>.log) — admission / breaker events
# ---------------------------------------------------------------------------

_TRACE_LINE = re.compile(
    r"^(?P<ts>\d{4}-\d\d-\d\dT[\d:.]+Z)\s+"
    r"(?P<level>TRACE|DEBUG|INFO|WARN|ERROR)\s+"
    r"(?P<target>[A-Za-z0-9_:]+):\s*(?P<rest>.*)$"
)
_FIELD = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=(\"[^\"]*\"|\S+)")

ADMISSION_PREFIX = "provider admission:"

ADMISSION_KINDS = (
    ("breaker opened", "breaker_open"),
    ("breaker re-opened", "breaker_reopen"),
    ("breaker half-open", "breaker_half_open"),
    ("breaker closed", "breaker_close"),
    ("circuit open", "acquire_refused"),
    ("stream wait budget expired", "unadmitted_stream"),
    ("spawn wait budget expired", "unadmitted_spawn"),
    ("spawn dispatch", "spawn_dispatch"),
    ("queue", "queue"),
    ("admit", "admit"),
)


def debug_log_paths(agent_dir: Path) -> list[Path]:
    logs = Path(agent_dir) / "logs"
    if not logs.is_dir():
        return []
    return sorted(p for p in logs.glob("debug-*.log*") if p.is_file())


def parse_debug_log(agent_dir: Path, limit_bytes: int = 8 * 1024 * 1024) -> dict:
    """Parse the fork's tracing log: level counts, warnings, admission events.

    The subscriber writes ``<ts> <LEVEL> <target>: <message> <k>=<v>`` lines
    (``pa-daemon/src/debug_log.rs``). Only the admission/breaker messages are
    interpreted; everything else is counted by level and target so an error
    storm in the product's own log is visible next to the transcript's.
    """
    paths = debug_log_paths(agent_dir)
    if not paths:
        return {"present": False, "reason": "no debug-*.log (ALLSPARK_DEBUG unset or no events)"}
    levels: Counter = Counter()
    targets: Counter = Counter()
    warnings: list[dict] = []
    admission: list[dict] = []
    kinds: Counter = Counter()
    lines = 0
    for path in paths:
        try:
            data = path.read_bytes()[-limit_bytes:]
        except OSError:
            continue
        for raw in data.decode("utf-8", errors="replace").split("\n"):
            if not raw.strip():
                continue
            lines += 1
            match = _TRACE_LINE.match(raw)
            if not match:
                levels["UNPARSED"] += 1
                continue
            level = match.group("level")
            target = match.group("target")
            rest = match.group("rest")
            levels[level] += 1
            targets[target] += 1
            fields = {key: value.strip('"') for key, value in _FIELD.findall(rest)}
            message = rest.split(" {")[0]
            if level in ("WARN", "ERROR"):
                message = _FIELD.sub("", rest).strip()
                warnings.append({"level": level, "target": target,
                                 "message": message[:200],
                                 "fields": {k: v for k, v in list(fields.items())[:6]}})
            if ADMISSION_PREFIX in rest:
                kind = "other"
                for needle, name in ADMISSION_KINDS:
                    if needle in rest:
                        kind = name
                        break
                kinds[kind] += 1
                admission.append({"kind": kind, "level": level,
                                  "provider": fields.get("provider"),
                                  "fields": {k: v for k, v in list(fields.items())[:6]}})
    return {
        "present": True,
        "files": [p.name for p in paths],
        "lines": lines,
        "levels": dict(levels),
        "top_targets": dict(targets.most_common(6)),
        "warnings": warnings[:20],
        "warning_count": len(warnings),
        "error_count": levels.get("ERROR", 0),
        "admission_events": len(admission),
        "admission_kinds": dict(kinds),
        "breaker_opens": kinds.get("breaker_open", 0) + kinds.get("breaker_reopen", 0),
        "breaker_closes": kinds.get("breaker_close", 0),
        "acquire_refusals": kinds.get("acquire_refused", 0),
        "admission_sample": admission[:10],
    }


# ---------------------------------------------------------------------------
# Log invariants
# ---------------------------------------------------------------------------

# Defaults apply when a task has a `logs:` block but omits a key. The two
# storm bounds are ON by default (a suite-wide "no error storm" invariant);
# the budget bounds are opt-in per task because only the task knows its shape.
LOG_DEFAULT_LIMITS = {
    "max_consecutive_error_turns": 5,
    "max_consecutive_failed_requests": 6,
    "breaker_closed": False,        # opt-in: no breaker opens on a green task
}


def _limit(spec: dict, key: str):
    """A numeric limit, or None when unset.

    ``0`` is a real limit (zero occurrences allowed), so the usual
    ``or 0``/``if value`` idiom would silently disable it; the default table
    only marks a check "off" with ``0`` for the bounds whose sensible default
    is unbounded.
    """
    raw = spec.get(key)
    if raw is None or raw == "":
        return None
    try:
        value = int(raw)
    except (TypeError, ValueError):
        return None
    return value


def evaluate_log_invariants(spec: dict | None, timeline: dict, requests: dict,
                            debug: dict, budget_timeout_s: int) -> list[dict]:
    """Check the ``logs:`` block of a task against the parsed sources.

    Every check returns ``{id, ok, detail, source}``; ``source`` names the
    artifact the number came from so a failing check points at evidence.
    Unavailable sources yield ``ok: null`` (skipped), never a silent pass.
    """
    # The storm bounds are suite-wide (like the canary rule): every task gets
    # them unless it overrides them. Budget bounds stay opt-in.
    spec = {**LOG_DEFAULT_LIMITS, **(spec or {})}
    summary = timeline.get("summary") or {}
    checks: list[dict] = []

    def add(check_id, ok, detail, source):
        checks.append({"id": check_id, "ok": ok, "detail": detail, "source": source})

    limit = _limit(spec, "max_consecutive_error_turns")
    if limit is not None:
        got = summary.get("max_consecutive_error_turns", 0)
        add("error_storm_turns", got <= limit,
            f"{got} consecutive error turns (limit {limit})", "session.jsonl")

    limit = _limit(spec, "max_consecutive_failed_requests")
    if limit is not None:
        if requests.get("present"):
            got = requests.get("max_consecutive_failed", 0)
            add("error_storm_requests", got <= limit,
                f"{got} consecutive failed provider requests (limit {limit})",
                "logs/agent.jsonl")
        else:
            add("error_storm_requests", None,
                "request-timing log absent (PI_REQUEST_TIMING off?)", "logs/agent.jsonl")

    limit = _limit(spec, "max_requests")
    if limit is not None:
        if requests.get("present"):
            got = requests.get("count", 0)
            add("retry_budget_requests", got <= limit,
                f"{got} provider requests (limit {limit}, outcomes {requests.get('outcomes')})",
                "logs/agent.jsonl")
        else:
            add("retry_budget_requests", None,
                "request-timing log absent; retry budget not observable", "logs/agent.jsonl")

    limit = _limit(spec, "max_turn_ms")
    if limit is not None:
        got = summary.get("turn_ms_max", 0.0)
        add("turn_duration", got <= limit,
            f"slowest turn {got}ms (limit {limit}ms)", "session.jsonl")

    limit = _limit(spec, "max_total_request_ms")
    if limit is not None:
        if requests.get("present"):
            got = requests.get("total_ms", 0.0)
            add("request_time_total", got <= limit,
                f"{got}ms across all provider requests (limit {limit}ms)", "logs/agent.jsonl")
        else:
            add("request_time_total", None, "request-timing log absent", "logs/agent.jsonl")

    if spec.get("breaker_closed"):
        if debug.get("present") and debug.get("admission_events"):
            opens = debug.get("breaker_opens", 0)
            add("breaker_closed", opens == 0,
                f"{opens} breaker open(s), {debug.get('breaker_closes', 0)} close(s), "
                f"{debug.get('admission_events')} admission event(s)", "logs/debug-*.log")
        else:
            add("breaker_closed", None,
                "no provider-admission events in the debug log "
                "(print lane does not run the daemon admission registry)", "logs/debug-*.log")

    limit = _limit(spec, "max_warn_lines")
    if limit is not None:
        if debug.get("present"):
            got = debug.get("warning_count", 0)
            add("warn_storm", got <= limit,
                f"{got} WARN/ERROR lines in the product log (limit {limit})", "logs/debug-*.log")
        else:
            add("warn_storm", None, "no debug log to scan", "logs/debug-*.log")

    if budget_timeout_s and summary.get("span_ms"):
        add("trial_span_within_timeout", summary["span_ms"] <= budget_timeout_s * 1000,
            f"transcript span {summary['span_ms']}ms vs {budget_timeout_s}s budget",
            "session.jsonl")

    return checks


def invariant_problems(checks: list[dict], prefix: str = "log") -> list[str]:
    """Turn failing checks into the runner's reason strings."""
    return [f"{prefix}: {c['id']} {c['detail']}" for c in checks if c["ok"] is False]


def skipped_checks(checks: list[dict]) -> list[str]:
    return [c["id"] for c in checks if c["ok"] is None]


def redact(text: str, secrets: list[str]) -> tuple[str, int]:
    """Strip planted secret values from copied evidence files."""
    hits = 0
    for secret in secrets:
        if secret and secret in text:
            hits += text.count(secret)
            text = text.replace(secret, "<redacted>")
    return text, hits
