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
if worker_body is None or backup_body is None:
    problems.append("the fixture does not carry both `Worker::run` and `Backup::run`")
else:
    if "self.id * 10" not in worker_body:
        problems.append("Worker::run was not changed to return `self.id * 10`")
    if "self.id * 2" in worker_body:
        problems.append("Worker::run still carries the original `self.id * 2`")
    if "self.id * 3" not in backup_body:
        problems.append("Backup::run was modified; the task asked to leave it untouched")

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
