# Review receipts

A review finding is a claim. A claim without a quote, a location, and a
verdict cannot be checked by anyone who was not in the session, and a review
whose findings cannot be checked is an opinion with a severity label.

Both examples below are real receipts from this repository: one confirmed and
fixed, one rejected. The rejected one matters as much — a round that produces
only confirmations was not adversarial enough to trust.

This is the contract for one finding. It is deliberately small: a receipt that
is expensive to write gets skipped, and a skipped receipt is worse than no
process, because it looks like coverage.

## The receipt

```yaml
# one finding, found at 3e56c9f7a and fixed at a later commit
id: R1
severity: high            # critical | high | medium | low
confidence: 0.9           # 0.0-1.0; below 0.5 is a question, not a finding
claim: "A project plan restored from the store bypasses the plan invariants"
file: crates/pa-daemon/src/plan_restore.rs
line: 110
quote: |
  Some(PlanState { items })
evidence: |
  `update_plan` rejects duplicate ids and a second `in_progress` item in
  `PlanState::validate_items`. The restore path builds `PlanState` directly, so
  a stored plan an older build wrote — or a hand-edited log — installs a plan
  the tool would have refused.
verdict: confirmed
suggested_fix: "Validate in `plan_for_start` and adopt nothing when it fails."
```

A `rejected` receipt is just as much a result:

```yaml
id: R2
severity: medium
confidence: 0.6
claim: "The plan tool records events even when the tool call is refused"
file: crates/pa-daemon/src/agent_engine/plan_tool.rs
line: 118
quote: |
  let state = self.store.replace_from_tool(items);
evidence: |
  I expected the recorder to run before the tool's own validation. It does not:
  `items` is validated above this line and the call returns the error first.
verdict: rejected
suggested_fix: "None; the ordering is already correct."
```

Fields, and what each one is for:

| Field | Why it exists |
|---|---|
| `id` | Findings get re-checked in a later round; an id is what a round refers to |
| `severity` | Orders the work; `critical` means the product is wrong for a user |
| `confidence` | Separates "I read this and it is true" from "this looks suspicious" |
| `claim` | One sentence, checkable, no narrative |
| `file` + `line` | The reviewer's location, verified against the tree at the reviewed commit |
| `quote` | The text the claim is about, copied verbatim, or the claim is unverified |
| `evidence` | What the reviewer did to check it: a command, a read, a test |
| `verdict` | `confirmed` (checked and true), `rejected` (checked and false), `unresolved` (could not check) |
| `suggested_fix` | The smallest change that closes it |

## Rules

- **A finding needs a quote.** A claim with no quoted text is `unresolved`,
  whatever the reviewer believed. Line numbers drift; the quote is what can be
  re-found.
- **`unresolved` is a real verdict, and it never counts as clean.** A round
  with unresolved findings has not converged.
- **`rejected` stays in the report.** A finding that was checked and refuted is
  the evidence that the round was adversarial; deleting it hides that the review
  happened.
- **Confidence is calibrated, not decorative.** Below `0.5` the finding states
  a question. Reporting it as a defect costs the operator the time to disprove
  it.
- **The reviewer states the commit it reviewed.** A receipt against a moved tree
  is a claim about code that is no longer there.

## Convergence

A review round is clean when every finding is `confirmed` and fixed, or
`rejected` with its refutation recorded. `unresolved` findings mean the round
did not converge and must be either resolved or explicitly carried forward with
the reason.

The cap is three rounds on one change. A fourth round is not a review, it is a
rewrite with a reviewer attached: stop and state what does not converge and why.

## Where a receipt lives

- In a pull request: the PR body, one table row per finding.
- In a run with a durable directory: the round's report, one receipt per
  finding, alongside the diff that was reviewed.
- Never in a place that a clone does not get: a review nobody can re-read is a
  log nobody reads.

## What this does not cover

- **Style.** A formatter decides it; a receipt about indentation spends the
  budget that a contract break needed.
- **Preference.** "I would have written this differently" is not a finding. It
  becomes one when it names the cost: a reader misled, a case unhandled, a test
  that cannot fail.
- **Speculation about intent.** The claim is about the code, not about why
  someone wrote it.
