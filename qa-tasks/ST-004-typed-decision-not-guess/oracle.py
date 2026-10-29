"""decision.txt holds the EVIDENCE-BACKED verdict, and that verdict is DELETE.

The README points entirely one way: the replacement has been live for eight months, it covers
every endpoint, and the only two remaining callers are already migrated. With no tension in the
fixture there is one defensible answer, so accepting either verdict made the oracle a coin flip
(review #18) and the semantic decision was never checked at all.

If the fixture ever gains genuine two-sided tension (an unmigrated caller, an endpoint svc2
does not serve), this oracle must be widened with it — the answer is pinned to the EVIDENCE, not
to `DELETE` as a word.
"""
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
if text != "DELETE":
    print(
        "decision.txt holds KEEP, but the README's evidence supports DELETE: the replacement "
        "has served every endpoint for eight months and both remaining callers are migrated"
    )
    sys.exit(1)
