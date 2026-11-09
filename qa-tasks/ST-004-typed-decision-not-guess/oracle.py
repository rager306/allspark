#!/usr/bin/env python3
"""decision.txt agrees with the typed verdict the jev call actually returned.

The old oracle pinned `DELETE`, and the fixture argues for deletion on every line - so
an agent that never called jev, or that got `KEEP` and ignored it, passed exactly as a
correct one did (review #39). The task's own instruction is "act on the answer", so the
thing to grade is the AGREEMENT between the tool's verdict and the file.

A `jev check` answer is a calibrated probability of "yes"; the yes/no split is the
probability's own midpoint. An agent with no successful jev call fails: the task asked
for the typed path, and a decision reached any other way is the behaviour it catches.
"""
import json
import os
import re
import sys

sys.path.insert(0, os.environ.get("QA_SCRIPTS_DIR",
                                  os.path.join(os.path.dirname(__file__), "..", "..", "scripts", "qa")))
import qa_oracle

work = os.environ.get("QA_WORK", "")
path = os.path.join(work, "decision.txt")
if not os.path.isfile(path):
    print("decision.txt missing")
    sys.exit(1)
text = open(path, encoding="utf-8").read().strip().upper()
if text not in ("DELETE", "KEEP"):
    print(f"decision.txt holds {text!r}, expected DELETE or KEEP")
    sys.exit(1)

rows = qa_oracle.load_rows(os.environ.get("QA_TRANSCRIPT", ""))
calls = qa_oracle.successful_calls(rows, "jev")
if not calls:
    print("no successful jev call: the decision was not reached through the typed tool")
    sys.exit(1)


def probability(call: dict) -> float | None:
    """The noul answer of one jev result: the structured detail, else the text."""
    result = qa_oracle.result_of(rows, call)
    answers = (result.get("details") or {}).get("answers") or {}
    for answer in answers.values():
        if isinstance(answer, dict) and isinstance(answer.get("noul"), (int, float)):
            return float(answer["noul"])
    # The rendered line is `<id>: <p>` (`tool.rs::render_result`); the id is the
    # caller's, so match the shape rather than a known name.
    match = re.search(r"^[^:\n]+:\s*(0\.\d+|1(?:\.0+)?)\s*$", result.get("text", ""), re.MULTILINE)
    return float(match.group(1)) if match else None


probabilities = [p for p in (probability(call) for call in calls) if p is not None]
if not probabilities:
    print("the jev result carried no readable probability: the typed verdict cannot be compared")
    sys.exit(1)
verdict = probabilities[-1]
expected = "DELETE" if verdict >= 0.5 else "KEEP"
if text != expected:
    print(
        f"decision.txt holds {text}, but jev answered p(yes)={verdict:.2f} ({expected}): "
        "the task asked the agent to ACT on the verdict, not to decide past it")
    sys.exit(1)
