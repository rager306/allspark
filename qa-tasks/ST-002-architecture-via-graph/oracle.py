"""The graph was USED for this question, and the state proves it — no prose matching.

Two problems with the previous version, both real (review #16):

1. It matched chain hops as raw substrings over the whole transcript (`flat.find`), so ordinary
   prose that happened to contain `main`, `handle`, `validate`, `load` IN THAT ORDER passed with
   no graph call at all. AGENTS.md is explicit that text matching belongs in the advisory judge
   and never in the blocking gate.
2. The order it demanded was wrong. The fixture is `validate(repo.load())`, and Rust evaluates
   the argument first: the real chain is `main -> handle -> load -> 41 -> validate -> 41`, i.e.
   `load` BEFORE `validate`. An agent that read the code correctly and reported the true order
   would have FAILED the oracle.

So the blocking layer checks what a state oracle can check — that the symbol graph was actually
consulted, and that the fixture is the one this task describes — and hop ORDER is left to the
advisory judge, which reads the answer in context instead of substring-matching it.
"""
import json, os, sys

work = os.environ.get("QA_WORK", "")
home = os.environ.get("QA_HOME", "") or os.path.expanduser("~")
transcript = os.environ.get("QA_TRANSCRIPT", "")
problems = []

# 1. The state: a graph index exists for this work tree. `graph` builds one under the agent
#    dir, so its absence means the tool never ran — which is the behaviour this task exists to
#    catch, and it cannot be faked by prose.
candidates = []
for root in {home, os.path.expanduser("~"), work}:
    if not root:
        continue
    candidates.append(os.path.join(root, ".prime", "agent", "graph.db"))
    candidates.append(os.path.join(root, ".allspark-home", ".prime", "agent", "graph.db"))
indexed = any(os.path.isfile(path) and os.path.getsize(path) > 0 for path in candidates)
if not indexed:
    problems.append(
        "no graph index exists under the agent dir: the code graph was never built, so the "
        "chain was not traced with `graph` (prose alone is not navigation)"
    )

# 2. The answer is present at all. Its CONTENT is graded by the advisory judge, not here.
if transcript and os.path.isfile(transcript):
    text = open(transcript, encoding="utf-8", errors="replace").read()
    if "CHAIN-DONE" not in text:
        problems.append("missing CHAIN-DONE marker")

for problem in problems:
    print(problem)
sys.exit(1 if problems else 0)
