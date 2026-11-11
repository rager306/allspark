#!/usr/bin/env python3
"""A SUCCESSFUL graph call and the call chain in the FINAL ANSWER (review #37).

Three defects made the old oracle pass an agent that never traced anything:

1. it accepted a `graph` ATTEMPT - the invariant layer only reported the tool's name
   and arguments, so a failing call satisfied `must`. The oracle now reads the
   transcript and requires a graph call whose result arrived without an error.
2. `graph.db` existing was the state proof, but a session creates the graph store on
   startup, so the file says "the tool was constructed", not "the graph answered".
3. the `CHAIN-DONE` marker was searched over the WHOLE transcript - and the task's own
   prompt carries it, so it was always there. It is now required in the final answer.

The chain itself is checked against the fixture's evaluation order, deterministically:
`handle` calls `validate(repo.load())`, and Rust evaluates the argument first, so
`load` runs before `validate`. The advisory judge still grades the wording.
"""
import os
import re
import sys

sys.path.insert(0, os.environ.get("QA_SCRIPTS_DIR",
                                  os.path.join(os.path.dirname(__file__), "..", "..", "scripts", "qa")))
import qa_oracle

transcript = os.environ.get("QA_TRANSCRIPT", "")
rows = qa_oracle.load_rows(transcript)
problems = []

if not rows:
    print("no session transcript: the trial produced no evidence to grade")
    sys.exit(1)

# 1. The graph ANSWERED: a call whose result exists and carried no error.
graph_calls = qa_oracle.successful_calls(rows, "graph")
if not graph_calls:
    attempted = [c for c in qa_oracle.tool_calls(rows) if c["name"] == "graph"]
    if attempted:
        results = qa_oracle.tool_results(rows)
        detail = [results.get(c["id"], {}).get("text", "<no result>")[:200]
                  for c in attempted][:2]
        problems.append(f"every graph call failed or never returned: {detail}")
    else:
        problems.append("no graph tool call at all: the chain was not traced with `graph`")

# 2. The ANSWER carries the marker (the prompt carries it too, so the transcript
#    cannot prove anything) and the chain, in evaluation order.
answer = qa_oracle.final_answer(rows)
if "CHAIN-DONE" not in answer:
    problems.append("the final answer is missing CHAIN-DONE")

# 3. The independent expected sequence: the answer must be the ORDERED list the prompt
#    asks for, naming each hop in evaluation order. "Four names somewhere in the text"
#    is not a chain - `validate -> load -> handle -> main` names all four and inverts
#    the answer (review #37's residual).
HOPS = ["main", "handle", "load"]
# `validate` is the call load's value is passed INTO, so it may appear in the answer (the
# reference chain carries it) but the prompt stops at the value 41: its absence is correct
# and its presence must FOLLOW load.
OPTIONAL = ["validate"]
#: The value `Repo::load` returns — the first value the chain carries, and the value the
#: prompt asks the chain to end at.
ANSWER_VALUE = "41"
#: The value `validate` returns. It may appear ONLY after `validate` appears, because a value
#: cannot precede the call that produces it (review #37, residual: `load -> 42` was accepted).
VALIDATE_VALUE = "42"


def hop_is_known(token: str) -> bool:
    """Whether `token` is a hop this fixture can actually contain."""
    return token in HOPS or token in OPTIONAL or bool(re.fullmatch(r"\d+", token))


def hop_name(raw: str) -> str:
    """One `A -> B` token reduced to the name it names: `Repo::load` and
    `repo.load()` are both `load`, `` `main` `` is `main`, `41` stays a value."""
    token = raw.strip().strip("`*_ ").rstrip(".,;:")
    if not token:
        return ""
    head = token[: token.find("(")] if "(" in token else token
    names = re.findall(r"[A-Za-z_][A-Za-z0-9_]*", head)
    # The LAST identifier of a qualified path, the FIRST of a call expression's head
    # (`validate(repo.load())` is `validate`; `repo::Repo::load` is `load`).
    return names[-1] if names else token


def chain_lines(text: str) -> list[list[str]]:
    """Every line carrying `->`, as its list of hop names."""
    return [
        [hop_name(part) for part in line.split("->")]
        for line in text.splitlines()
        if "->" in line
    ]


def hop_positions(tokens: list[str]) -> list[int] | None:
    """The index of each required hop, when all appear in this order; else None."""
    positions = []
    cursor = 0
    for hop in HOPS:
        while cursor < len(tokens) and tokens[cursor] != hop:
            cursor += 1
        if cursor == len(tokens):
            return None
        positions.append(cursor)
        cursor += 1
    return positions


def chain_problem() -> str | None:
    """Why the answer is not the chain, or None when a line IS the chain."""
    if not chain_lines(answer):
        return (
            "the final answer carries no ordered `A -> B -> C` chain: naming every "
            "function in prose is not the list the task asked for")
    misplaced_value = None
    misplaced_optional = None
    for tokens in chain_lines(answer):
        positions = hop_positions(tokens)
        if positions is None:
            continue
        # The chain is the SUBSET of these names, in this order — not a subsequence of
        # arbitrary hops (review #37, residual). `main -> bogus -> handle -> nonsense ->
        # load -> 41` used to pass: every required hop appeared in order and the invented
        # ones were simply skipped. The prompt asks for the functions the CODE has, so a
        # name that is not one of them means the agent reported a chain it made up.
        invented = [token for token in tokens if not hop_is_known(token)]
        if invented:
            return (
                "the final answer names functions the fixture does not have "
                f"({', '.join(invented)}): the chain is main -> handle -> load, optionally "
                "through validate")
        # A numeric token is a VALUE the call chain returns, and only two exist in this
        # fixture: 41 (what `Repo::load` returns) and 42 (what `validate` returns). `-> 999`
        # is not the chain the code has, however well ordered its names are (review #37,
        # residual). Both legitimate forms stay valid — `load -> 41` and the longer
        # `load -> validate -> 42` that omits 41.
        for number in (token for token in tokens if re.fullmatch(r"\d+", token)):
            if number not in (ANSWER_VALUE, VALIDATE_VALUE):
                return (
                    f"the final answer carries the value {number}, which the fixture never "
                    f"produces: the chain returns {ANSWER_VALUE} from load and "
                    f"{VALIDATE_VALUE} from validate")
        numbers = [token for token in tokens if re.fullmatch(r"\d+", token)]
        # 42 is `validate`'s value: without a `validate` token it has no producer in this
        # chain, and naming it after `load` would claim a call that did not happen.
        if VALIDATE_VALUE in numbers and "validate" not in tokens:
            return (
                f"the final answer carries {VALIDATE_VALUE}, which only `validate` returns, "
                "but the chain names no `validate` call")
        # The prompt stops at 41. Reporting the chain PAST it (`... -> validate -> 42`) is a
        # different question's answer: it names a value the task never asked for, and an
        # oracle that accepts it grades whichever ending the agent happened to write
        # (review #37's residual). The answer must END at ANSWER_VALUE.
        if numbers and numbers[-1] != ANSWER_VALUE:
            return (
                f"the final answer ends at the value {numbers[-1]}, not {ANSWER_VALUE}: the "
                f"prompt asks for the chain to {ANSWER_VALUE}, which is what load returns")
        if numbers and tokens.index(numbers[0]) < positions[2]:
            misplaced_value = tokens
            continue
        if "validate" in tokens and tokens.index("validate") < positions[2]:
            misplaced_optional = tokens
            continue
        return None
    if misplaced_value is not None:
        return (
            "the final answer places a value before `load` returns it: "
            f"{' -> '.join(misplaced_value)}")
    if misplaced_optional is not None:
        return (
            "the final answer orders `validate` before `load`; Rust evaluates "
            "validate(repo.load())'s argument first, so `load` runs first")
    return (
        "the final answer's ordered list does not name `main`, `handle`, `load` in "
        "evaluation order: the chain from main runs through handle's call to "
        f"Repo::load ({' -> '.join(HOPS)})")


problem = chain_problem()
if problem is not None:
    problems.append(problem)

for problem in problems:
    print(problem)
sys.exit(1 if problems else 0)
