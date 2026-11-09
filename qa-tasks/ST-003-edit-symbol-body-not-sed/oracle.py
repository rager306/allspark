"""describe changed, summary byte-identical — checked INSIDE each method body.

The previous version was file-wide: `"describe: retries=" in text` is satisfied by the
string appearing in a COMMENT, and `text.count(original) != 1` is satisfied by the
original format expression surviving in `describe` — the method nobody was asked to
touch (review #38). Measured live: a wrong answer that left `describe` alone, corrupted
`summary`, and put the marker in a comment passed the old oracle.

So each method's body is extracted by brace matching and asserted separately:
  * `Config::describe` carries the `describe: ` form and no longer the original shape;
  * `Config::summary` is byte-identical to the original body and carries no marker.
A text replace of `src/lib.rs` cannot satisfy both at once — which is the task.
"""
import os
import sys

sys.path.insert(0, os.environ.get("QA_SCRIPTS_DIR", os.path.join(os.path.dirname(__file__), "..", "..", "scripts", "qa")))
import qa_oracle

work = os.environ.get("QA_WORK", "")
path = os.path.join(work, "src", "lib.rs")
if not os.path.isfile(path):
    print(f"missing {path}")
    sys.exit(1)
text = open(path, encoding="utf-8").read()

ORIGINAL_BODY = 'format!("retries={} timeout={}", self.retries, self.timeout_ms)'
MARKER = "describe: retries="

problems = []
describe = qa_oracle.fn_bodies(text, "describe")
summary = qa_oracle.fn_bodies(text, "summary")
if len(describe) != 1:
    problems.append(f"expected exactly one `Config::describe` body, found {len(describe)}")
if len(summary) != 1:
    problems.append(f"expected exactly one `Config::summary` body, found {len(summary)}")

if len(describe) == 1:
    body = describe[0]
    if MARKER not in body:
        problems.append("Config::describe does not return the `describe: retries=` form")
    if ORIGINAL_BODY in body:
        problems.append("Config::describe still carries the original (unprefixed) format string")
if len(summary) == 1:
    body = summary[0].strip()
    if body != ORIGINAL_BODY:
        problems.append(
            f"Config::summary's body is not byte-identical to the original: found {body!r}")
    if MARKER in body:
        problems.append("Config::summary was rewritten with the describe marker")

for problem in problems:
    print(problem)
sys.exit(1 if problems else 0)
