"""First-contact instruction injection: a tool that teaches itself on the run
that produced findings.

The pattern (ADR-0001 Part B, "metaprompt"): a reader who has never seen this
tool meets it for the first time in a RED run. That run's output must carry
three things besides the findings - what each finding means, what the fix
workflow is, and where the contract lives - because after a failed gate nobody
goes looking for a README.

Injected deterministically (no marker-file state to lose):
  * every red `ci` run  -> the how-to-read block + the fix workflow,
  * `testgov learn`     -> the full scenario card on demand.
"""

RED_GUIDE = """
HOW TO READ THIS
----------------
Each finding is one broken row of the test contract
(doc/testing/test-matrix.yaml).
  [HIGH] = the gate fails; a row's oracle is missing, ignored, or contradicts
           its row.
  (check/category) names the violated rule; file:line is where to act.

FIX WORKFLOW (the row, not the finding, is the unit of work)
  1. Open the row:    testgov explain <row-id>
  2. Read its kill_mutation - the edit that must make the oracle red.
  3. Make the oracle exist (write the test), or re-scope the row if the
     invariant moved.
  4. Re-run:          testgov ci
Red-first is the acceptance test: an oracle that never failed once proves
nothing.

WHERE THE CONTRACT LIVES
  doc/testing/test-matrix.yaml   the rows (one per invariant)
  doc/adr/0001-*.md              the ADR: oracle taxonomy, planes, this workflow
  scripts/testgov/fixtures/      one fixture per check; selftest proves it can
                                 fail
"""

SCENARIO_CARD = """
SCENARIOS (what you can do with this tool)
------------------------------------------
  ci                     gate run: every HIGH finding across all rows
    --changed            only rows whose cited code changed (pre-commit)
    --format json        machine-readable (CI annotation pipelines)
  list                   rows and their status (gap/killed/blocked/...)
  explain <row-id>       one row, full detail, with its matrix line
  map                    rows by plane/layer/axis - a map, not a score
  selftest               prove every check can fail (must stay green)
  learn                  this card

TYPICAL SESSIONS
  * "The gate is red and I have never seen this tool":
        testgov ci             read the guide under the findings
  * "I changed crates/pa-ai and want to know which rows care":
        testgov ci --changed
  * "What does the contract require for the model picker?"
        testgov explain UI-picker-nonempty
  * "Are we strong or weak on identity?"
        testgov map
  * "A check just told me to fix the fixture - did I break the checker?"
        testgov selftest
"""


def red_guide() -> str:
    """The block appended to a red `ci` run."""
    return RED_GUIDE


def scenario_card() -> str:
    """The full scenario card (`testgov learn`)."""
    return SCENARIO_CARD
