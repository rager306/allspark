# Pattern synthesis working notes (not final)

User requested PATTERNS, NOT code portability. Code citations establish mechanism/overlap only.

## Highest-interest pattern: preference learning separated from memory writing
Command Code official /docs/taste says: LLM extracts structured signals from accepts/rejects/edits/correction diffs -> taste-1 judges preference vs noise, category/confidence and conflicts -> LLM renders -> CLI stores readable scoped taste.md for later use. Description is vendor documentation, not an independent audit of taste-1 calibration or RL claims. Do not equate taste-1 with Agr or generation model fine-tuning.
Potential Prime benefit: distinguish user preference from objective fact or unverified assistant assertion; avoid globalizing project-specific rule or accidental workaround. Candidate admission requires evidence and can abstain. Explicit user correction is stronger than acceptance of an entire patch; accepted patch is not approval of each design choice. Check repeated contrary evidence and scoped conflicts. Never learn secrets or treat third-party prompt instructions as user preferences.
Test: held-out tasks, blinded user preference comparisons, edits needed until acceptance, incorrect learned preferences, token/time overhead. Unit tests passing alone do NOT verify taste. Fixed train/eval split by time/repo prevents feedback leakage.

## Advisor: selective independent consultation (pending oh-my-pi source)
Not another autonomous worker that edits same files. Need clear role separation, targeted question, evidence packet and uncertainty, independent counterexample; main agent retains decision. Escalate on expensive/irreversible/uncertain decision or repeated failed attempt, not every step. Independence is not guaranteed by using another instance of same model. Measure net avoided rework minus consultation cost and anchoring/false objections.

## Deterministic fast tool boundary (pending oh-my-pi source)
Tool layer should offer bounded, structured, explicit-completeness observations; exact stale-edit detection, cancellation, pagination, machine-readable failure. Latency improvement must be end-to-end (wall time per verified task), not language label. Prime already Rust tools + Python orchestration. Faster grep irrelevant if model inference dominates; output quality can matter more than microseconds.

## Verification-backed durable workflow
Prime factory already handles DAG/machines/joins. Gap proven: live run state is kernel-only while children survive restart. Pattern is durable intent + reconciliation of observed work + accepted artifact proof, not new scheduler. Child exit != task done; merge != verified product. Requirement->check->evidence links. Avoid retrying non-idempotent external effects blindly after uncertain crash.

## Retrieval not indiscriminate accumulation
Current harness ranking weighted lexical overlap/IDF. RuVector suggests embedding identity and hybrid recall; start evidence that semantic queries miss lexical retrieval. Keep one truth store, indexes derived. Scope and provenance before similarity. Recall exact prior artifact on demand; current commit/validity context needed. Never serve semantically similar answer as exact cached code execution result.

## Learning from outcomes vs self-assessment
Tests/gates provide bounded objective evidence, not universal correctness. Record failures/denominator, task difficulty/repo/model/time context; no success-only memory or reward equal to model saying done. Retrieval of proven episodes before bandits/RL. Evaluate future held-out task quality and cost, not advertised routing speedups.
