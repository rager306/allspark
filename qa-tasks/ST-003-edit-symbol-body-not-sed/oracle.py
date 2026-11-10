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

def without_comments(body: str) -> str:
    """The body's CODE: line and block comments removed, string literals kept whole.

    A body-wide `in` check cannot tell a statement from a comment, so
    `// describe: retries=` beside a wrong return satisfied it (review #38's residual).
    The scan is a small state machine rather than a regex because `//` inside a string
    literal (`"http://…"`) must not start a comment.
    """
    out = []
    index = 0
    while index < len(body):
        char = body[index]
        rest = body[index:]
        if char == '"':
            end = index + 1
            while end < len(body):
                if body[end] == "\\":
                    end += 2
                    continue
                if body[end] == '"':
                    end += 1
                    break
                end += 1
            out.append(body[index:end])
            index = end
        elif rest.startswith("//"):
            newline = body.find("\n", index)
            index = len(body) if newline < 0 else newline
        elif rest.startswith("/*"):
            close = body.find("*/", index + 2)
            index = len(body) if close < 0 else close + 2
        else:
            out.append(char)
            index += 1
    return "".join(out)


def string_literals(code: str) -> list[str]:
    """The contents of every `"…"` literal in code (escapes kept as written)."""
    literals = []
    index = 0
    while index < len(code):
        if code[index] != '"':
            index += 1
            continue
        end = index + 1
        while end < len(code):
            if code[end] == "\\":
                end += 2
                continue
            if code[end] == '"':
                break
            end += 1
        literals.append(code[index + 1:end])
        index = end + 1
    return literals


problems = []
describe = qa_oracle.fn_bodies(text, "describe")
summary = qa_oracle.fn_bodies(text, "summary")
if len(describe) != 1:
    problems.append(f"expected exactly one `Config::describe` body, found {len(describe)}")
if len(summary) != 1:
    problems.append(f"expected exactly one `Config::summary` body, found {len(summary)}")

if len(describe) == 1:
    body = describe[0]
    code = without_comments(body)
    # The RETURNED text, not a marker anywhere in the body: the prefix must be inside a
    # string literal, so a comment that names it cannot stand in for the change.
    if not any(MARKER in literal for literal in string_literals(code)):
        problems.append(
            "Config::describe does not return the `describe: retries=` form "
            "(no string literal in its body carries the prefix)")
    if ORIGINAL_BODY in code:
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
