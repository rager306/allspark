"""Route an architectural question to the right graph action.

The native `graph` tool executes; this skill decides *which* action answers the
question, and composes the exact call for the model to issue. It exists because
the routing is the hard part: "who calls load_config" is `trace` with
`direction: in`, but "what does load_config touch" is `direction: out`, and
picking wrong costs a round-trip and teaches the model the tool "did not work".
"""

from __future__ import annotations

import json


def route(question: str) -> dict:
    """Map a plain-language question to a ready-to-issue graph tool call.

    Returns a dict with the action and parameters, formatted the way the native
    tool expects them. The caller issues the call; this function never executes
    anything.

    Args:
        question: The question in plain language: "who calls load_config",
            "what breaks if I change the Payment flow", "how is the code
            organized", "where is the code that parses YAML".

    Returns:
        {"action": ..., ...params} — pass it straight to the `graph` tool.
        Includes a "why" note describing the routing, so the choice is
        auditable and the caller can disagree and re-route.
    """
    action, params, why = _route(question)
    call = {"action": action, **params}
    call["why"] = why
    return call


def _route(question: str) -> tuple[str, dict, str]:
    q = question.lower().strip()

    # Caller questions: who reaches this symbol.
    for marker in ("who calls", "what calls", "callers of", "who uses", "what uses"):
        if marker in q:
            symbol = _subject_after(q, marker)
            return (
                "trace",
                {"symbol": symbol, "direction": "in"},
                f"`{symbol}`: incoming callers. `direction: in` walks the edges "
                "that reach it; read the returned file:line and jump only if a "
                "callsite needs context.",
            )

    # Callee questions: what this symbol reaches.
    for marker in ("what does", "callees of", "what touches"):
        if marker in q:
            symbol = _subject_after(q, marker)
            return (
                "trace",
                {"symbol": symbol, "direction": "out"},
                f"`{symbol}`: outgoing calls. `direction: out` is the blast "
                "radius view — what breaks if this symbol changes.",
            )

    # A chain between two known points.
    if " -> " in question or " -> " in q or "chain from" in q:
        return (
            "trace",
            {"symbol": _first_symbol(q), "direction": "both", "depth": 6},
            "a chain question: start from the first named symbol, walk both "
            "directions, and intersect with the second name.",
        )

    # Architecture shapes.
    view_map = {
        "cycle": "cycles",
        "layer": "layers",
        "cluster": "clusters",
        "hotspot": "hotspots",
        "complex": "hotspots",
        "entry point": "entry_points",
        "structure": "structure",
        "organized": "structure",
        "architecture": "overview",
        "overview": "overview",
    }
    for needle, view in view_map.items():
        if needle in q:
            return (
                "architecture",
                {"view": view},
                f"`{needle}` maps to the `{view}` architecture view.",
            )

    # Default: find symbols by meaning. The search is hybrid (BM25 + graph
    # degree), so a plain-language description beats a regex.
    return (
        "search",
        {"query": question},
        "no structural cue matched, so this is a discovery question: search by "
        "meaning, then `trace` the hits if relationships surface.",
    )


def _subject_after(q: str, marker: str) -> str:
    rest = q.split(marker, 1)[1]
    words = [w.strip("?,.'\"()") for w in rest.split()]
    return words[0] if words else rest.strip()


def _first_symbol(q: str) -> str:
    for token in q.replace("->", " ").split():
        cleaned = token.strip("?,.'\"()")
        if cleaned and cleaned[0].isalpha() and cleaned not in ("from", "to", "the"):
            return cleaned
    return q.split()[0] if q.split() else q


def format_call(call: dict) -> str:
    """Render a routed call as the JSON a `graph` tool invocation needs."""
    why = call.pop("why", "")
    body = json.dumps(call, indent=2)
    return f"{body}\n// {why}" if why else body
