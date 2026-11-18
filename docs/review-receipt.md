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
# one finding, reviewed at eea54033b, still open
id: R1
severity: high            # critical | high | medium | low
confidence: 0.95          # 0.0-1.0; below 0.5 is a question, not a finding
claim: "Two sessions in one project share a plan-item row key"
file: crates/pa-project/src/model.rs
line: 22
quote: |
  id         TEXT PRIMARY KEY,
evidence: |
  The plan item id is the table's primary key, so `P1` is one row per project,
  not per session. Two sessions in one project both build `P1`..., so the second
  snapshot overwrites the first's row and the restored plan is whichever session
  wrote last. Measured: with two recorders over one log the model kept a single
  row after both wrote `P1`.
verdict: confirmed
suggested_fix: "Key the row by (project, session, id), or refuse a second writer.
  Both change the contract, so this is a decision, not an edit."
```

A `rejected` receipt is just as much a result:

```yaml
id: R2
severity: medium
confidence: 0.6
claim: "The plan tool records events even when the tool call is refused"
file: crates/pa-daemon/src/agent_engine/plan_tool.rs
line: 120
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
