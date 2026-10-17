#!/usr/bin/env python3
"""Output grading for the Real-Task Deviation Suite (level-2 -> level-3 bridge).

Two graders live here:

  * ``evaluate_output_patterns`` — DETERMINISTIC output oracle. Counts,
    exact formulations and length bounds over the agent's final answer and
    the process outcome. Cheap, stable, CI-safe; it is the only part of this
    module allowed to fail a trial.
  * ``judge_task`` — ONE System One (SubQ Snap) decision per task, asked on a
    task rubric ("was the goal reached / is the right next step named").
    Returns a verdict plus the full probability vector. It never fails a
    trial: it lands in the manifest as ``judge`` and feeds the advisory A/B
    delta, exactly as the level-3 tier in
    ``.research/blackbox-qa-best-practices.md`` prescribes (model-graded
    checks stay out of the blocking gate).

Judge contract (``.research/subq-snap.md`` §2): ``POST /v1/systemone`` with
``{model, state, questions}``; ``choice`` questions carry ``criteria`` (the
option -> rubric map); the answer is ``{choice, probabilities, confidence}``.
One question per call, ``temperature`` is not a parameter of this API (it is
a decision model), ``unknown`` is always an offered option so the judge can
abstain instead of hallucinating.

The API key is read from ``SUBQ_API_KEY`` (or ``--judge-key-file``); it is
never logged, never written to the manifest. The manifest records only the
bundle id and a hash of the rubric text, per the run-manifest discipline.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import urllib.error
import urllib.request

SUBQ_ENDPOINT = "https://api.subq.ai/v1/systemone"
SUBQ_MODEL = "subq-snap-preview"
JUDGE_BUNDLE = "subq-snap-preview"          # what the manifest records
JUDGE_PROMPT_REV = "qa-judge-v1"            # bump on any rubric/prompt change
SCRIPTED_BUNDLE = "scripted-stub"

VERDICT_ORDER = {"not_achieved": 0, "partial": 1, "achieved": 2}
DEFAULT_CRITERIA = {
    "achieved": "the answer completes the task's stated goal with the "
                "concrete artifact/step the task asked for",
    "partial": "the answer moves toward the goal but leaves the required "
               "artifact, number or next step missing or vague",
    "not_achieved": "the answer does not address the goal, is empty, or is "
                    "only a refusal/error echo with no next step",
    "unknown": "the state does not carry enough evidence to decide",
}

# ---------------------------------------------------------------------------
# Final-answer extraction
# ---------------------------------------------------------------------------

def extract_final_answer(rows: list[dict], max_chars: int = 4000) -> str:
    """Last assistant text blocks, in order (tool calls are not prose)."""
    for row in reversed(rows or []):
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "assistant":
            continue
        texts = [b.get("text") or "" for b in (message.get("content") or [])
                 if isinstance(b, dict) and b.get("type") == "text"]
        text = "\n".join(t for t in texts if t).strip()
        if text:
            return text[:max_chars]
    return ""


def extract_tool_transcript(rows: list[dict], max_chars: int = 1200) -> str:
    """A compact "what the agent did" line for the judge's state."""
    lines = []
    for row in rows or []:
        if row.get("type") != "message":
            continue
        message = row.get("message") or {}
        if message.get("role") != "assistant":
            continue
        for block in message.get("content") or []:
            if isinstance(block, dict) and block.get("type") == "toolCall":
                args = json.dumps(block.get("arguments") or {}, sort_keys=True)
                lines.append(f"- {block.get('name')}: {args[:160]}")
    return "\n".join(lines[:20])[:max_chars]


# ---------------------------------------------------------------------------
# Deterministic output patterns (the `output:` block / oracle type `output`)
# ---------------------------------------------------------------------------

PATTERN_DEFAULT_TARGET = "answer"


def _target_text(target: str, answer: str, outcome: str) -> str:
    if target == "outcome":
        return outcome
    if target == "both":
        return answer + "\n" + outcome
    return answer


def evaluate_output_patterns(rules, answer: str, outcome: str) -> list[str]:
    """Check ``output:`` rules; returns human-readable problems.

    Rule shapes (all optional keys combine with AND):
      {regex, min, max, target}      occurrence bounds
      {contains, target}             literal substring required
      {not_contains, target}         literal substring forbidden
      {equals_any: [...], target}    the target text (stripped) is one of them
      {first_line_regex, target}     the first non-empty line matches
      {min_chars, max_chars, target} length bounds
      {all_of: [rule...]}, {any_of: [rule...]}  nesting
    """
    problems: list[str] = []
    for rule in rules or []:
        if not isinstance(rule, dict):
            continue
        label = rule.get("id") or ""
        target = rule.get("target", PATTERN_DEFAULT_TARGET)
        text = _target_text(target, answer, outcome)
        tag = f"output{f':{label}' if label else ''}"

        if "all_of" in rule:
            problems += [f"{tag} all_of -> {p}" for p in
                         evaluate_output_patterns(rule["all_of"], answer, outcome)]
        if "any_of" in rule:
            sub = [evaluate_output_patterns([r], answer, outcome) for r in rule["any_of"]]
            if sub and all(s for s in sub):
                problems.append(f"{tag} any_of: none of {len(sub)} alternatives matched")
        if "regex" in rule:
            hits = len(re.findall(rule["regex"], text, re.MULTILINE))
            # A bounds-only rule (max present, min absent) means absence:
            # the presence default of 1 must not turn `max: 0` into a
            # contradiction.
            low = int(rule.get("min", 0 if "max" in rule else 1))
            high = rule.get("max")
            if hits < low:
                problems.append(f"{tag} regex /{rule['regex']}/ matched {hits} < {low}")
            elif high is not None and hits > int(high):
                problems.append(f"{tag} regex /{rule['regex']}/ matched {hits} > {high}")
        if "contains" in rule and rule["contains"] not in text:
            problems.append(f"{tag} {target} missing {rule['contains']!r}")
        if "not_contains" in rule and rule["not_contains"] in text:
            problems.append(f"{tag} {target} contains forbidden {rule['not_contains']!r}")
        if "equals_any" in rule:
            stripped = text.strip()
            if stripped not in [str(v) for v in rule["equals_any"]]:
                problems.append(f"{tag} {target} is not one of {rule['equals_any']!r}")
        if "first_line_regex" in rule:
            first = next((l for l in text.splitlines() if l.strip()), "")
            if not re.search(rule["first_line_regex"], first):
                problems.append(f"{tag} first line {first[:80]!r} !~ /{rule['first_line_regex']}/")
        if "min_chars" in rule and len(text.strip()) < int(rule["min_chars"]):
            problems.append(f"{tag} {target} {len(text.strip())} chars < {rule['min_chars']}")
        if "max_chars" in rule and len(text.strip()) > int(rule["max_chars"]):
            problems.append(f"{tag} {target} {len(text.strip())} chars > {rule['max_chars']}")
    return problems


# ---------------------------------------------------------------------------
# Judge: state construction + live SubQ call + scripted stub
# ---------------------------------------------------------------------------

def build_state(task, answer: str, outcome: str, oracle_problems: list[str],
                log_summary: dict | None, max_chars: int = 12000) -> str:
    """The judge's ``state``: what was asked, what came back, what the harness saw.

    Deliberately evidence-shaped, not persuasive: the task prompt, the
    deterministic verdicts, the answer, and (when the task asks about the
    process) a compact log summary. No secret values ever enter this string.
    """
    parts = [
        "## TASK",
        (task.title or task.id),
        "### PROMPT",
        (task.prompt or "").strip()[:2000],
    ]
    if oracle_problems:
        parts += ["### DETERMINISTIC CHECKS THAT FAILED", *oracle_problems[:6]]
    else:
        parts += ["### DETERMINISTIC CHECKS", "all deterministic checks passed"]
    parts += ["### AGENT FINAL ANSWER", (answer or "<no final answer text>").strip()[:4000]]
    if outcome.strip():
        parts += ["### PROCESS OUTPUT (tail)", outcome.strip()[-1500:]]
    if log_summary:
        parts += ["### RUN TELEMETRY", json.dumps(log_summary, sort_keys=True)[:1200]]
    return "\n".join(parts)[:max_chars]


def rubric_of(task) -> dict:
    spec = getattr(task, "judge", None) or {}
    criteria = spec.get("criteria") or {}
    if isinstance(criteria, list):
        criteria = {str(item): None for item in criteria}
    if not criteria:
        criteria = dict(DEFAULT_CRITERIA)
    elif "unknown" not in criteria:
        criteria = {**criteria, "unknown": DEFAULT_CRITERIA["unknown"]}
    return {
        "question": spec.get("question")
        or "Did the agent achieve the task's stated goal?",
        "criteria": criteria,
        "model": spec.get("model") or SUBQ_MODEL,
    }


def rubric_hash(rubric: dict) -> str:
    return hashlib.sha256(json.dumps(rubric, sort_keys=True).encode()).hexdigest()[:12]


def load_api_key(explicit: str | None = None, key_file: str | None = None) -> tuple[str | None, str]:
    """Resolve the judge key: explicit -> env -> file. Returns (key, source)."""
    if explicit:
        return explicit, "flag"
    env = os.environ.get("SUBQ_API_KEY", "").strip()
    if env:
        return env, "env:SUBQ_API_KEY"
    if key_file:
        try:
            value = open(key_file, "r", encoding="utf-8").read().strip()
            if value:
                return value, f"file:{key_file}"
        except OSError:
            pass
    return None, "absent"


def call_systemone(api_key: str, model: str, state: str, question: str,
                   criteria: dict, timeout: float = 20.0) -> dict:
    """One Decisions-API call. Raises on transport/API failure (caller degrades)."""
    body = {
        "model": model,
        "state": state,
        "questions": {
            "goal": {
                "type": "choice",
                "instructions": question,
                "criteria": {name: (desc or None) for name, desc in criteria.items()},
            }
        },
    }
    request = urllib.request.Request(
        SUBQ_ENDPOINT,
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        payload = json.loads(response.read().decode("utf-8"))
    answer = (payload.get("answers") or {}).get("goal") or {}
    return {
        "choice": answer.get("choice"),
        "probabilities": answer.get("probabilities") or {},
        "confidence": answer.get("confidence"),
        "usage": payload.get("usage") or {},
        "model": payload.get("model"),
    }


def scripted_verdict(answer: str, oracle_problems: list[str]) -> dict:
    """Deterministic stand-in judge: deterministic verdict first, prose second.

    Used only when no key is available (or ``--judge scripted``), so the
    pipeline, manifest shape and A/B delta can be exercised offline. It is
    labelled ``scripted-stub`` in the manifest and never presented as a model
    verdict.
    """
    answer = (answer or "").strip()
    if oracle_problems:
        choice, probs = "not_achieved", {"achieved": 0.05, "partial": 0.15,
                                         "not_achieved": 0.75, "unknown": 0.05}
    elif not answer:
        choice, probs = "unknown", {"achieved": 0.0, "partial": 0.0,
                                    "not_achieved": 0.0, "unknown": 1.0}
    elif len(answer) < 12:
        choice, probs = "partial", {"achieved": 0.2, "partial": 0.6,
                                    "not_achieved": 0.15, "unknown": 0.05}
    else:
        choice, probs = "achieved", {"achieved": 0.8, "partial": 0.15,
                                     "not_achieved": 0.03, "unknown": 0.02}
    return {"choice": choice, "probabilities": probs,
            "confidence": max(probs.values()) if probs else None,
            "usage": {"input_tokens": 0, "output_tokens": 0}}


def judge_task(task, rows, answer: str, outcome: str, oracle_problems: list[str],
               log_summary: dict | None, api_key: str | None, key_source: str,
               mode: str = "auto", timeout: float = 20.0) -> dict:
    """One judge call per task. Never raises: failures become ``status: skipped``."""
    rubric = rubric_of(task)
    state = build_state(task, answer, outcome, oracle_problems, log_summary)
    result = {
        "status": "skipped",
        "bundle": JUDGE_BUNDLE,
        "prompt_rev": JUDGE_PROMPT_REV,
        "rubric_sha256": rubric_hash(rubric),
        "question": rubric["question"],
        "criteria": sorted(rubric["criteria"]),
        "state_chars": len(state),
        "key_source": key_source,
        "verdict": None,
        "probabilities": {},
        "confidence": None,
    }
    want_live = mode in ("auto", "live")
    if want_live and api_key:
        try:
            response = call_systemone(api_key, rubric["model"], state,
                                      rubric["question"], rubric["criteria"], timeout)
            result.update({
                "status": "live",
                "verdict": response.get("choice"),
                "probabilities": response.get("probabilities"),
                "confidence": response.get("confidence"),
                "usage": response.get("usage"),
                "model": response.get("model") or rubric["model"],
            })
            return result
        except urllib.error.HTTPError as exc:
            result.update({"status": "skipped", "reason": f"http {exc.code}"})
        except Exception as exc:  # noqa: BLE001 - the judge must never break a run
            result.update({"status": "skipped", "reason": f"{type(exc).__name__}: {exc}"[:160]})
    elif want_live:
        result["reason"] = "no SUBQ_API_KEY (and no --judge-key-file); falling back to scripted"

    stub = scripted_verdict(answer, oracle_problems)
    result.update({
        "status": "scripted",
        "bundle": SCRIPTED_BUNDLE,
        "verdict": stub["choice"],
        "probabilities": stub["probabilities"],
        "confidence": stub["confidence"],
    })
    return result


# ---------------------------------------------------------------------------
# A/B delta (proto level 3)
# ---------------------------------------------------------------------------

def verdict_rank(verdict: str | None) -> int | None:
    return VERDICT_ORDER.get(str(verdict or "").lower())


def compare_judge_deltas(current_tasks: list[dict], baseline_tasks: list[dict]) -> dict:
    """Per-task judge deltas current vs baseline (worse / better / same / n-a).

    Only comparable pairs count: both sides must carry a rankable verdict
    (``unknown`` and skips are excluded, as the level-3 design requires).
    """
    base_by_id = {t.get("id"): t for t in baseline_tasks or []}
    rows, better, worse, same, excluded = [], 0, 0, 0, 0
    for task in current_tasks:
        base = base_by_id.get(task.get("id"))
        if not base:
            continue
        cur_v = (task.get("judge") or {}).get("verdict")
        base_v = (base.get("judge") or {}).get("verdict")
        cur_rank, base_rank = verdict_rank(cur_v), verdict_rank(base_v)
        if cur_rank is None or base_rank is None:
            excluded += 1
            rows.append({"id": task.get("id"), "baseline": base_v, "current": cur_v,
                         "delta": "n/a"})
            continue
        if cur_rank > base_rank:
            delta = "better"
            better += 1
        elif cur_rank < base_rank:
            delta = "worse"
            worse += 1
        else:
            delta = "same"
            same += 1
        rows.append({"id": task.get("id"), "baseline": base_v, "current": cur_v,
                     "delta": delta})
    return {
        "pairs": len(rows),
        "better": better,
        "worse": worse,
        "same": same,
        "excluded": excluded,
        "rows": rows,
    }


def summarize_judges(task_results: list[dict]) -> dict:
    """Suite-level judge roll-up for the manifest."""
    by_verdict: dict[str, int] = {}
    by_status: dict[str, int] = {}
    for task in task_results:
        judge = task.get("judge") or {}
        status = judge.get("status") or "none"
        by_status[status] = by_status.get(status, 0) + 1
        verdict = judge.get("verdict")
        if verdict:
            by_verdict[verdict] = by_verdict.get(verdict, 0) + 1
    return {"by_status": by_status, "by_verdict": by_verdict,
            "bundle": JUDGE_BUNDLE, "prompt_rev": JUDGE_PROMPT_REV}
