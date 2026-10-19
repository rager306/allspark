"""Routing-table regression tests.

The routing is the product here: a wrong direction (`in` vs `out`) costs the
model a round-trip and teaches it the graph tool "did not work", which is how a
session ended with 132 python_repl calls and one graph call.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from graph import format_call, route  # noqa: E402


def test_caller_question_routes_inward() -> None:
    call = route("who calls load_config")
    assert call["action"] == "trace"
    assert call["direction"] == "in"
    assert call["symbol"] == "load_config"
    assert "why" in call


def test_callee_question_routes_outward() -> None:
    call = route("what does load_config call")
    assert call["action"] == "trace"
    assert call["direction"] == "out"
    assert call["symbol"] == "load_config"


def test_chain_question_walks_both_directions() -> None:
    call = route("is there a chain from main to save")
    assert call["action"] == "trace"
    assert call["direction"] == "both"
    assert call["depth"] > 1


def test_architecture_views_map_by_intent() -> None:
    assert route("show me the cycles")["view"] == "cycles"
    assert route("where is the complexity")["view"] == "hotspots"
    assert route("entry points of the system")["view"] == "entry_points"
    assert route("how is the code organized")["view"] == "structure"


def test_discovery_defaults_to_search() -> None:
    call = route("where do we parse YAML config files")
    assert call["action"] == "search"
    assert call["query"] == "where do we parse YAML config files"


def test_every_call_carries_its_reason() -> None:
    for question in (
        "who calls main",
        "what does parse do",
        "show the layers",
        "where is the retry logic",
    ):
        call = route(question)
        assert call["why"], f"{question!r} routed without a reason: {call}"


def test_format_call_detaches_the_reason() -> None:
    rendered = format_call(route("who calls main"))
    # The why is a comment for the reader; the tool payload must not carry it.
    assert '"why"' not in rendered.split("//")[0]
