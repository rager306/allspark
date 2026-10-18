"""decision.txt exists and holds a decided verdict."""
import os, sys

work = os.environ.get("QA_WORK", "")
path = os.path.join(work, "decision.txt")
if not os.path.isfile(path):
    print("decision.txt missing")
    sys.exit(1)
text = open(path, encoding="utf-8").read().strip().upper()
if text not in ("DELETE", "KEEP"):
    print(f"decision.txt holds {text!r}, expected DELETE or KEEP")
    sys.exit(1)
