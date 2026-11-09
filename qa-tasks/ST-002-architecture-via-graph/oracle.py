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

# 3. The independent expected sequence: load is evaluated before validate, so its
#    first mention must come first in the answer.
order = ["main", "handle", "load", "validate"]
positions = {}
for name in order:
    match = re.search(rf"\b{name}\b", answer)
    if not match:
        problems.append(f"the final answer does not name `{name}`")
    else:
        positions[name] = match.start()
missing = [n for n in order if n not in positions]
if not missing:
    if positions["load"] > positions["validate"]:
        problems.append(
            "the final answer orders `validate` before `load`; Rust evaluates "
            "validate(repo.load())'s argument first, so load runs first")

for problem in problems:
    print(problem)
sys.exit(1 if problems else 0)
