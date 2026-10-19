# B.1 independent review

Read AGENTS.md and clean-code skill. Initial read-only review of the five requested files; parent then approved the minimal fix and offline regression coverage. No live requests, builds, or config changes by this reviewer. One synthetic, stdlib-only `build_state` check confirmed the finding below without invoking the bridge.

## Finding: P1 — leaked planted secrets were sent to the live judge (fixed)

`scripts/qa/run_suite.py:1643-1649` passes raw final-answer text, stdout/stderr, and oracle reasons to `judge_task`. `grade_trial` has the planted secret values and detects their appearance (`:1552`, `:1585`), but this merely adds a failure reason. The raw value remains in the evidence. `qa_judge.build_state` does not redact it despite claiming secrets never enter the state. With the new default live/public judge, an RT-008 privacy failure therefore uploads the leaked value to OpenCode. Truncation is not a privacy boundary. This raw evidence path existed before, but the new no-key live default makes it reachable in ordinary judge runs.

### Accepted initial proposed fix (superseded by applied value-based guard)

1. Add `unsafe_evidence: bool = False` after `model` in `judge_task` parameters.
2. After the explicit scripted branch and before `if not api_key`, insert:

```python
    if unsafe_evidence:
        result["reason"] = "judge evidence contains a planted secret"
        return result
```

3. Add this key to `grade_trial`'s transient `_judge_input`:

```python
        "unsafe_evidence": any(r.startswith("secret:") for r in result["reasons"]),
```

4. In the `main` call to `judge_task`, add:

```python
                unsafe_evidence=judge_input.get("unsafe_evidence", False)
```

Do not sanitize only after slicing evidence: the slice could retain a secret prefix while removing its suffix. Skipping a known contaminated live measurement is the smallest safe change. Regression should exercise `grade_trial` with a synthetic planted value and mock the bridge: assert deterministic failure remains, advisory result is `(unscored, None, None)`, and native transport is never called. Include a secret in an early reason outside the six-reason evidence cap.

## Checks that passed inspection

- Numeric zero is preserved; missing/unknown/error measurements stay `None`, including empty summaries.
- No implicit scripted fallback or retries; explicit scripted measurements have separate provenance and do not enter numeric native averages.
- The call occurs once per task, after all trials, using the last trial; transient judge input is removed before manifest persistence.
- Normal judge errors and abstentions do not affect strict/delta deterministic gates.
- Comparison requires matching backend, bundle, model, prompt revision, and rubric hash; wrong response model is unscored.
- Native bridge only selects fork-catalog `opencode` models, fixed native catalog URLs, and public auth. No 9router route or runtime-home configuration read.
- Bridge input is independently capped at 65,536 bytes; timeout clamped; incomplete/failed provider responses cannot become scores. Provider bodies/errors are not printed. Python evidence is bounded.
- Tests assert real statuses, call counts, numeric zero, mismatch exclusions, stdin-only auth, and runner manifests. Integration tests skip explicitly if binary is missing rather than silently passing.

## Upstream truncation provenance (fixed)

The advisory extraction path now retains a visible truncation marker, including process-output tail slicing. Deterministic output-oracle extraction remains unchanged. Tests verify both upstream paths preserve markers into state and `state_truncated=True`. Limits remain unchanged: answer 4,000 chars, stdout 1,200, stderr 800, total state 12,000.

## Clean-code score

**8/10.** Names and provenance boundaries are clear, failure behavior is explicit, and tests target meaningful states. The missing privacy boundary is the substantive gap; add the fail-closed evidence guard and its runner-level regression. Preserve extraction truncation provenance if claiming full evidence reporting. No refactoring for an arbitrary score is warranted.

## Applied fix and verification

- `grade_trial` marks transient evidence unsafe using planted secret values against complete transcript bytes, complete stdout/stderr, and all deterministic reasons. This happens independently of answer/output/reason slices; a secret beyond the six-reason cap is caught.
- Native `judge_task` returns `(unscored, None, None)` before the bridge when unsafe. Explicit scripted behavior remains offline and unchanged.
- Runner regression exercises the real grader, main/manifest and strict gate with mocked trial driving: secret after the answer cap, before the stdout tail, and in an oracle reason after the first six. Native transport is asserted never called; strict stays exit 1 and the deterministic failure is retained. Scripted fixture remains `not_achieved` without a native call.
- Red-first: tests added before fix produced two failing contaminated live subcases (incorrectly scored achieved); `.research/b1-privacy-red.txt`.
- Final: `uv run --no-project python -m unittest discover -s scripts/qa -p 'test*judge.py'`: **23 passed**, no skips. `.research/b1-privacy-green.txt`.
- No live provider calls/cargo builds/config changes or commits by this worker. Source stable after the final test run. Clean-code after fix: **9/10**; no arbitrary refactor needed. Tests intentionally mock process driving for the privacy boundary; real native transport bytes remain covered by the existing bridge tests.
