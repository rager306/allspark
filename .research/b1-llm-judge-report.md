# B.1 advisory native LLM judge — 2026-10-19

## Implementation and scope

The existing `scripts/qa/qa_judge.py` and `run_suite.py --judge` remain the only
QA judge pipeline. Removed the unregistered `crates/pa-core/src/qa/` draft
(its tests were not compiled). No production public API or dependency added.
The dev-only `crates/pa-ai/examples/qa_judge_request.rs` calls the existing
`pa_ai::complete_simple` transport once. Thus native opencode HTTP/2/v2
behavior stays in pa-ai; Python implements no provider HTTP transport.
The example owns no tools, sessions, agent loop, fallback or retry.

`harness2` measure-after-choice means judge only after the selected agent result,
not a confidence-line parser. Here it runs once per task, after the final trial.
The verdict is JSON `{verdict, reason}`; probabilities/confidence are not asked
for or claimed calibrated. Manifest keeps empty/null legacy confidence fields.

Live errors, malformed output, unknown verdict and unknown model are unscored:
`verdict: null`, `score: null`. A scored `not_achieved` is **0**. Native numeric
means exclude unscored and all scripted fixtures. Scripted is explicit only,
with distinct backend/bundle. A/B comparisons require equal backend, bundle,
model, prompt revision and rubric hash; errors/unknown are excluded, not zeros.
The deterministic exit gate does not read the advisory judge verdict.

Assumption: this QA-only bridge is a developer build artifact, not a new shipped
product command. The task rubric maps to ordinal 0/1/2, not weighted benchmark
accuracy; future multi-criterion scoring needs a separately versioned rubric.

### Explicit build and run

```sh
PA_TS_BINARY=/nonexistent/ts CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo build -p pa-ai --example qa_judge_request
python3 scripts/qa/run_suite.py --tasks RT-007 --lane faux --trials 1 --bin /root/prime-agent/target/debug/prime-agent --strict --judge live --judge-model opencode/deepseek-v4.1-flash-free --judge-timeout 30 --out .research/b1-live-run
```

No hidden auto-build. Missing executable is clearly unscored. `--judge-bin`
selects a prebuilt example; `--judge-model` selects the model. Explicit `public`
auth is passed at request boundary through stdin, never argv/manifests.
The bridge accepts native opencode catalog models only, and requires `public`.
It does not read/change settings or use 9router. No secrets/request auth dumps
are recorded. HTTP metadata includes status and retry-after only. There are no
retries, including 429: the caller can honor retry-after before a future run.

Constants introduced: native default `opencode/deepseek-v4.1-flash-free`, prompt
revision `qa-judge-native-v1`, 512 response-token budget, 65,536-byte input cap,
120-second maximum native timeout. Existing judge evidence slices now report
truncation. Default runner timeout remains 25 seconds. Ordinal score is 0/1/2,
not an accuracy probability. No temperature parameter is required or sent.

## Verifiers

- Red-first original-source run: `.research/b1-regression-red.txt`: 9 tests,
  13 failing assertions/subtests for silent scripted fallback and incompatible
  provenance comparisons. This ran before fixes; no mutation of other workers.
- Additional isolated inversion proof: copied final source under
  `.research/b1-mutation/`, removed provenance/status compatibility guard;
  the current tests fail (see `.research/b1-mutated-provenance-red.txt`).
  Production source and other workers were never mutated.
- `python3 -m unittest discover -s scripts/qa -p test_qa_judge.py -v`: 17 tests
  pass, including 429/no-key/timeout/malformed/unknown, zero preservation,
  provenance, native-only mean, stdin auth, and evidence truncation.
- `python3 -m unittest discover -s scripts/qa -p "test_*judge.py" -v`: 20 tests
  (17 unit + 3 integration). Integration drives the real CLI twice per task,
  then exactly one native-executable seam; mocked 429/invalid/timeout all remain
  unscored and `--strict` still exits 0. A/B on errors excludes the pair.
  See `.research/b1-all-python.txt`.
- Native example build and focused rustfmt check passed. Focused clippy
  `cargo clippy -p pa-ai --example qa_judge_request -- -D warnings` passed
  after fixing two example-only style diagnostics (doc backticks and explicit
  Map default); see `.research/b1-clippy-final.log`.
- No full workspace gate run by this worker (parallel ownership/cargo lock).

## Real live measurement

Exactly **one live request** (budget <=4), through the same run_suite path above.
Agent under test used faux RT-007; judge was real native opencode:

- model: `opencode/deepseek-v4.1-flash-free`
- HTTP 200, verdict achieved, ordinal score 2
- measured provider-request latency **1969 ms**
- reported usage: **746 input + 70 output = 816 tokens**; cost **0**
- deterministic trial pass; runner exit 0

Sanitized result: `.research/b1-live-result.json`; generated complete manifest:
`.research/b1-live-run/20261019-145237/manifest.json`.
No second live request was needed. A live 429/timeout was not induced; their
unscored/gate behavior is proven offline, not represented as live evidence.
No claim about judge quality/calibration follows from one easy sample.

## Limits and next design

This closes a bounded single-judge QA path, not PPC selection or parallel eval.
The current single task selection is the existing final trial; no hidden best-of
or judge-informed retry. Broader multi-model judging should first confirm each
model in the native catalog, then use separate provenance-stratified summaries
and record per-model availability; never average failures as zeros or pool
scripted/live values. GLM was not tested or asserted working. There is no
Typesafe/Jev confidence emulation. TS parity evidence was not applicable to this
fork-only dev QA bridge; no shipped product CLI/wire surface changed.

### Extended testing plan (not executed)

Keep the bounded smoke budget **max 4 live calls** per approved pass. On 429,
stop that provider pass and honor its retry-after before a later approved pass;
no automatic retries. Pin endpoint (`https://opencode.ai/inference/v1` from the
native catalog), exact provider/model reference, prompt/rubric hashes and binary
revision in every future experiment; catalog/endpoint changes require a new
measurement provenance, not a pooled historical delta. Current model is the
confirmed native DeepSeek free entry; future GLM rows require catalog and
availability confirmation first.

Future blind matrix: freeze task evidence after agent selection; anonymize
candidate labels, keep candidate harness identity hidden, and judge the same
frozen deliverable with each confirmed judge model independently. Include known
complete, partial, missing-artifact and insufficient-evidence fixtures. Report
per-model availability, ordinal agreement and disagreements, not a fabricated
calibrated confidence. Any changed endpoint/model/prompt/rubric forms a separate
matrix column. The selector must not see semantic judge results before making
its choice. This is future design, not implementation or performance evidence.

## Independent privacy review follow-up (offline)

The accepted independent P1 is fixed: known contaminated evidence is marked from
complete planted-value checks before any judge slicing and never reaches native
transport. Explicit scripted fixtures and the deterministic strict gate retain
their prior behavior. Advisory answer and process-output extraction now retain
truncation markers without changing deterministic output-oracle text semantics.
No limits/default constants changed; no live requests or cargo builds were added.

Red-first privacy regression: `.research/b1-privacy-red.txt` (two contaminated
live subcases wrongly scored on pre-fix source). Final native project invocation:
`uv run --no-project python -m unittest discover -s scripts/qa -p 'test*judge.py'`
passed **23 tests**, no skips, superseding the earlier20-test count above.
Evidence: `.research/b1-privacy-green.txt`. The added runner coverage verifies
complete transcript/stdout and oracle-reason contamination beyond evidence caps,
bridge never called, strict exit1, scripted behavior preserved, and upstream
truncation provenance. Source is stable after the final test run.
