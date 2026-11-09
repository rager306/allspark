#!/usr/bin/env python3
"""What a task oracle needs to read state: the session transcript and Rust sources.

Two defect classes made this module necessary (review #36-#39):

* a file-wide substring check cannot tell WHICH method contains a string, so an
  edit to the wrong function passed (ST-003), and
* a marker or a chain found anywhere in the transcript is not an answer: the
  task's own prompt carries the marker, so "the transcript contains it" proved
  nothing (ST-002).

Both need the same two readers: the transcript's tool calls/results and the
final answer, and the bodies of the Rust functions in the fixture. Every reader
here returns data, never a verdict: the oracle decides.

Imported by task oracles through ``QA_SCRIPTS_DIR`` (set by
``scripts/qa/run_suite.py`` when it runs a script oracle). Stdlib only, like the
rest of the suite.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


def load_rows(transcript: str | os.PathLike | None) -> list[dict]:
    """The transcript's JSON rows (a missing/unreadable file is no rows)."""
    if not transcript:
        return []
    try:
        raw = Path(transcript).read_bytes()
    except OSError:
        return []
    rows = []
    for line in raw.split(b"\n"):
        if not line.strip():
            continue
        try:
            row = json.loads(line.decode("utf-8", errors="replace"))
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict):
            rows.append(row)
    return rows


def assistant_texts(rows: list[dict]) -> list[str]:
    """Every assistant text block, oldest first (tool calls are not prose)."""
    texts = []
    for row in rows:
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "assistant":
            continue
        for block in message.get("content") or []:
            if isinstance(block, dict) and block.get("type") == "text":
                text = block.get("text") or ""
                if text.strip():
                    texts.append(text)
    return texts


def final_answer(rows: list[dict]) -> str:
    """The last assistant text: the answer the task asked for."""
    texts = assistant_texts(rows)
    return texts[-1] if texts else ""


def tool_calls(rows: list[dict]) -> list[dict]:
    """Every tool call as {id, name, arguments} (arguments as a JSON string)."""
    calls = []
    for row in rows:
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "assistant":
            continue
        for block in message.get("content") or []:
            if isinstance(block, dict) and block.get("type") == "toolCall":
                calls.append({
                    "id": block.get("id") or "",
                    "name": block.get("name") or "",
                    "arguments": json.dumps(block.get("arguments") or {}, sort_keys=True),
                })
    return calls


def tool_results(rows: list[dict]) -> dict[str, dict]:
    """Tool results keyed by the call id they answer: {is_error, text, details}."""
    results = {}
    for row in rows:
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "toolResult":
            continue
        call_id = message.get("toolCallId") or ""
        if not call_id:
            continue
        text = "\n".join(
            block.get("text") or ""
            for block in message.get("content") or []
            if isinstance(block, dict) and block.get("type") == "text"
        )
        details = message.get("details") or {}
        results[call_id] = {
            "is_error": bool(message.get("isError")),
            "text": text,
            "details": details if isinstance(details, dict) else {},
        }
    return results


def successful_calls(rows: list[dict], name: str) -> list[dict]:
    """Tool calls named ``name`` whose result arrived and carried no error.

    A call whose result never landed is not evidence the tool ran to completion
    (the task's own failure mode is an attempt, not a result).
    """
    results = tool_results(rows)
    return [call for call in tool_calls(rows)
            if call["name"] == name
            and call["id"] in results
            and not results[call["id"]]["is_error"]]


def result_of(rows: list[dict], call: dict) -> dict:
    """The result row of one call ({} when it never arrived)."""
    return tool_results(rows).get(call["id"], {})


def fn_bodies(source: str, name: str) -> list[str]:
    """The brace-matched bodies of every ``fn <name>`` in ``source``.

    Brace matching, not a regex: the fixtures put the same expression in two
    methods on purpose, so the question is always "which body holds this", and a
    regex that spans a function compiles into an answer for one layout only.
    """
    bodies = []
    cursor = 0
    needle = f"fn {name}("
    while (found := source.find(needle, cursor)) >= 0:
        brace = source.find("{", found)
        if brace < 0:
            break
        depth = 0
        for index in range(brace, len(source)):
            char = source[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    bodies.append(source[brace + 1:index])
                    cursor = index + 1
                    break
        else:
            break
    return bodies


def impl_method_body(source: str, type_name: str, method: str) -> str | None:
    """The body of ``type_name::method`` — the one inside ``impl type_name``.

    The fixture's trap is two methods with ONE name in two impls, so the body must be
    resolved by its owning type; a positional pick breaks the moment the file is
    reordered.
    """
    head = f"impl {type_name} "
    start = source.find(head)
    if start < 0:
        head = f"impl {type_name}{{"
        start = source.find(head)
    if start < 0:
        return None
    brace = source.find("{", start)
    if brace < 0:
        return None
    depth = 0
    for index in range(brace, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                bodies = fn_bodies(source[brace:index], method)
                return bodies[0] if bodies else None
    return None


def oracle_import_path() -> str:
    """The directory this module lives in (for an oracle's sys.path)."""
    return os.environ.get("QA_SCRIPTS_DIR") or str(Path(__file__).resolve().parent)
