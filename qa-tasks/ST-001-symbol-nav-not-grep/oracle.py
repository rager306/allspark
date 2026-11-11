#!/usr/bin/env python3
"""Each `run` method's caller, from the ANSWER, and the edit inside the right body.

Two things were wrong with the old check (review #36):

1. the prompt said "only ONE of them is actually reachable", and the fixture calls BOTH
   - an accurate answer therefore contradicted the prompt, and a text search for
   `spawn_worker` / `unrelated` anywhere in the answer passed a SWAPPED mapping. The
   prompt now states both are called, and the oracle requires the structured mapping
   line by line.
2. the state rules were file-wide (`contains "self.id * 10"`), so the marker could land
   in the wrong body. Each method's body is extracted and checked separately.

The fixture's comments used to spell out the correct mapping (`called from
spawn_worker`); they no longer do, so the answer has to come from a symbol-aware lookup.
"""
import os
import re
import sys

sys.path.insert(0, os.environ.get("QA_SCRIPTS_DIR",
                                  os.path.join(os.path.dirname(__file__), "..", "..", "scripts", "qa")))
import qa_oracle

work = os.environ.get("QA_WORK", "")
path = os.path.join(work, "src", "lib.rs")
if not os.path.isfile(path):
    print(f"missing {path}")
    sys.exit(1)
source = open(path, encoding="utf-8").read()

# ── the edit, inside the right body ────────────────────────────────────────
problems = []
worker_body = qa_oracle.impl_method_body(source, "Worker", "run")
backup_body = qa_oracle.impl_method_body(source, "Backup", "run")
def returned_expression(body: str) -> str:
    """The expression a `-> u32` body RETURNS: its tail, or the argument of a bare `return`."""
    lines = [line for line in body.splitlines() if line.strip() and line.strip() != "}"]
    if not lines:
        return ""
    tail = lines[-1]
    if "return " in tail:
        tail = tail.split("return ", 1)[1]
    return tail


if worker_body is None or backup_body is None:
    problems.append("the fixture does not carry both `Worker::run` and `Backup::run`")
else:
    # The RETURNED value, not any occurrence in the body (review #36, residual): a marker in
    # a dead branch (`if false { return self.id * 10; }`) beside a live `self.id * 4` passed
    # the presence/absence pair while the method returned the wrong number.
    worker_returned = returned_expression(worker_body)
    if "self.id * 10" not in worker_returned:
        problems.append(
            "Worker::run does not RETURN `self.id * 10`; a copy in a dead branch is not the "
            f"change (returns {worker_returned.strip()!r})")
    if "self.id * 2" in worker_body:
        problems.append("Worker::run still carries the original `self.id * 2`")
    backup_returned = returned_expression(backup_body)
    if "self.id * 3" not in backup_returned:
        problems.append(
            "Backup::run's RETURNED value changed; the task asked to leave it untouched "
            f"(returns {backup_returned.strip()!r})")

# ── the answer: a structured mapping, not two names somewhere in the text ──
rows = qa_oracle.load_rows(os.environ.get("QA_TRANSCRIPT", ""))
answer = qa_oracle.final_answer(rows)
if "NAV-DONE" not in answer:
    problems.append("the final answer is missing NAV-DONE")

mapping = {}
for line in answer.splitlines():
    match = re.match(r"\s*(Worker|Backup)::run\s*(?:<-|->|:)\s*([A-Za-z_][A-Za-z0-9_]*)", line)
    if match:
        mapping[match.group(1)] = match.group(2)

if mapping.get("Worker") is None or mapping.get("Backup") is None:
    problems.append(
        "the final answer does not report both call sites as `Worker::run <- <caller>` "
        f"and `Backup::run <- <caller>` lines (found {mapping or 'none'})")
else:
    if mapping["Worker"] != "spawn_worker":
        problems.append(f"Worker::run's caller is reported as {mapping['Worker']!r}, not spawn_worker")
    if mapping["Backup"] != "unrelated":
        problems.append(f"Backup::run's caller is reported as {mapping['Backup']!r}, not unrelated")

for problem in problems:
    print(problem)
sys.exit(1 if problems else 0)
