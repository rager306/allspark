"""describe changed; summary byte-identical."""
import os, sys

work = os.environ.get("QA_WORK", "")
path = os.path.join(work, "src", "lib.rs")
if not os.path.isfile(path):
    print(f"missing {path}")
    sys.exit(1)
text = open(path, encoding="utf-8").read()
problems = []
if "describe: retries=" not in text:
    problems.append("Config::describe was not updated to the `describe: ` form")
# The summary must still carry the ORIGINAL format string.
original = 'format!("retries={} timeout={}", self.retries, self.timeout_ms)'
if text.count(original) != 1:
    problems.append(
        f"the original body must survive exactly once (summary), found {text.count(original)}")
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
