# JEV (TypeSafe) as a fast decision layer in the allspark fork

**Status:** research/design note (no code landed). **Sources:** local read-only clone
`/tmp/omp-src` (oh-my-pi), live EXA/TypeSafe docs (`docs.typesafe.ai`), the allspark fork
at `/root/prime-agent`.

**One-line thesis.** JEV is a *typed-decision* API (state + questions -> calibrated
answers), not a chat model. It is cheap and fast enough to sit on hot paths where the fork
today either calls a full LLM turn, runs a heuristic, or does nothing. The cleanest
integration is a **new fork-native `judge` layer** (a small `pa-jev` client + a `jev`
AgentTool + internal call sites), wired exactly like `lsp`/`graph`/`edit`: gated by a
settings block (default OFF, unlike the other fork tools), surfaced through
`native_tool_names`, and never on the critical path unless explicitly enabled.

---

## 1. JEV API reference (verified)

### 1.1 Transport

```
POST https://api.typesafe.ai/v1/systemone
Authorization: Bearer <TYPESAFE_API_KEY>
Content-Type: application/json
```

- `GET https://api.typesafe.ai/v1/models` lists account models (returns aliases).
- One JSON round-trip. **No streaming, no session state.** `output_tokens` are reported
  but **not billed**; you pay **input only**.
- Body may be compressed: `Content-Encoding: gzip` | `x-gzip` | `zstd` (one layer; zstd
  window <= 8 MiB). Body cap **32 MiB** (compressed and decompressed).
- Optional header `TypeSafe-Danger-Zone-Long-Context: true` raises context to 262k
  (slower, less accurate; off by default).

### 1.2 Request body

```json
{
  "model": "jev-latest",
  "state": "<string | object | array>",
  "questions": {
    "<id>": { "type": "...", "instructions": "...", "criteria": ... }
  }
}
```

- `state` — the content to judge. A string, or structured JSON (object/array). JEV ingests
  it **once**; every question sees the same state and they run **in parallel**, so a
  request with hundreds of questions costs about the same wall-clock as one question.
- `model` — required. `jev-latest` (alias -> `jev-1.13.0`) or a pinned id
  (`jev-1.13.0`, `jev-1.14.0`); `jev-preview` -> newest. Pin a version if you gate on a
  tuned confidence threshold — aliases move.
- `questions` — a map you key. Keys are **not** sent to the model; answers return under the
  same keys.
- `instructions` may itself be a string, object, or array (break a long question into a
  question field + referenced data fields, backticked by name).

### 1.3 Question primitives (three)

| `type`   | Asks                          | `criteria`                                   | Answer |
|----------|-------------------------------|----------------------------------------------|--------|
| `noul`   | yes/no                        | optional `{ "true": str, "false": str }`     | `noul` (0..1). **No `confidence` field.** |
| `choice` | pick one of a labelled set    | map `option -> rubric str \| null` (max 255) | `choice`, `probabilities` (sums to 1), `confidence` |
| `score`  | position on an ordered rubric | ordered array of 2..10 level descriptions   | `score`, `probabilities`, `confidence` (+ `legend` on some hosts) |

**Gotchas (measured, `WallerChen/jev-measured`):**
- `Score` returns the probability-weighted mean of the rubric **indices** `0..n-1`, **not**
  a 0..1 float (a 4-level rubric can return `2.68`). Recomputing from `probabilities`
  reproduces it to ~±0.02 (probabilities are rounded to 2 decimals).
- `Score.probabilities` is keyed by **index** (`"0"`, `"1"`, ...), never by label; use
  `legend` to map back.
- `noul` answers carry **no `confidence`** — distance from `0.5` is the only gate.
- `confidence` = 1 when mass is concentrated on one outcome, 0 when spread evenly; derived
  from `probabilities` for choice/score only.

### 1.4 Response

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "route": { "type": "choice", "choice": "billing",
               "probabilities": { "billing": 0.99, "bug": 0.0, "account": 0.01 },
               "confidence": 0.99 },
    "urgent": { "type": "noul", "noul": 0.95 }
  },
  "usage": { "input_tokens": 296, "output_tokens": 20 }
}
```

- Every answer echoes its question `type`; a missing/refused id is possible — a robust
  client must treat "answer for every requested id" as a postcondition and split/retry
  otherwise.

### 1.5 Errors, limits, retries

| Status | Meaning | Handling |
|--------|---------|----------|
| 400 | malformed body (nesting > 64, NaN/Inf, bad compression) | do not retry; fix request |
| 401 | missing/invalid key | rotate credential (oh-my-pi rotates on 401/403) |
| 403 | permission | do not retry |
| 404 | unknown model | do not retry |
| 413 | body > 32 MiB | shrink state |
| 415 | unsupported/multi-layer `Content-Encoding` | do not retry |
| 422 | body failed validation (e.g. over a question cap) | **split** questions and retry |
| 429 | rate limit (per-org, tokens/s + req/s) | honor `retry-after`, exponential backoff |
| 529 | TypeSafe overloaded | backoff + retry |

`x-typesafe-request-id` response header is the support/dedup handle. Retry policy mirrors
the SDKs: retry `408/429/500/502/503/504`, connection errors, and timeouts, with
exponential backoff + jitter (JS default: initial 500 ms, max 5000 ms, jitter 0.25).

**Question caps are per-adapter, not per-provider.** TypeSafe JEV itself has **no count
cap** (only the 64k per-request / 32k per-question token budgets). Third-party System One
servers do: OpenJEV-style gateways cap at **16**, razorback OpenJEV at **256**, Cloudflare
Clef at **64**. A client should carry a per-endpoint `maxQuestions` and split oversized
requests (oh-my-pi splits into chunks, <= 4 in flight, merging answers and summing usage;
the first failing chunk aborts siblings).

### 1.6 Economics (measured, not vendor arithmetic)

- Price: **$0.042 / 1M input tokens; output free.** $42/Btok.
- Real decision cost across 12 use cases: **$0.0000153–$0.0000304** (364–724 input
  tokens). The widely quoted "~$0.0004/decision" describes a *much larger* state.
- Latency: 458–563 ms raw from a laptop of which ~199 ms is the network floor; **184 ms
  median** US-East via OpenRouter; remainder ~260–375 ms.
- Head-to-head (same fixtures, structured output on): JEV 352 ms / $0.0000188 vs
  mistral-small 1343 ms / $0.0000255 (1.4x) vs gemini-2.5-flash-lite 877 ms / 1.7x vs
  gpt-5-nano 7504 ms / 18x. **A cheap chat model is 1.4–1.7x the cost, not 2 orders of
  magnitude** — the large multiples belong to reasoning models burning output tokens.
- **The only number that moves the bill is your average `state` length.** Questions share
  one state and run in parallel, so batch every independent question into one call; three
  separate calls pay for the state three times.

### 1.7 SDK / dependency choice

Official SDKs: Python (`typesafe_sdk`), JS/TS. Community Rust crates exist:
`typesafe-jev` (sync, `ureq` + rustls, no async runtime; the client jegrep uses),
`typesafe-systemone` (async, `reqwest`), `typesafe-client`, `jev-sdk`. **For the fork we
should not take a third-party crate as a hard dep**: the wire is one JSON POST, the fork
already depends on `reqwest` (`pa-core/Cargo.toml`), and `pa-ai` already has a shared
non-streaming JSON POST helper (`utils_inner/http.rs::post_json`). A ~300-line in-crate
client (types + `reqwest`) keeps the supply chain clean and lets us reuse the fork's auth
registry. This matches the fork's rule: *new fork work in new crates/files, no new deps
where the workspace already covers it.*

---

## 2. How oh-my-pi does it (the pattern to port)

oh-my-pi ships JEV as a first-class **`judge` model role** + **judgment APIs**:

- **Wire dialects** (`packages/ai/src/judgment/`): `typesafe` (`POST {base}/v1/systemone`),
  `openrouter-decisions` (`{base}/decisions`), `workers-ai-systemone`, `openai-decisions`
  (translated both ways). A `NativeJudge` serves all of them; `isJudgmentApi(api)` decides.
- **Provider catalog** (`catalog/.../providers/typesafe.kdl`): provider `typesafe`,
  `default-model "jev-latest"`, `env "TYPESAFE_API_KEY"`, `base-url "https://api.typesafe.ai"`,
  cost `input=0.042 output=0`, `supports-tools false`, `kind "judge"`.
- **Model role** (`config/model-roles.ts`): `judge` is a model-kind role whose pool accepts
  `judge | tiny | chat` models. `judgeRoleChain` resolves the role, and **from the first
  native candidate on, only native candidates remain** — a prompted chat model can never
  stand in for a failed native judgment (it can't reproduce calibrated probabilities).
- **Auth** (`rules/auth/typesafe.kdl`): env `TYPESAFE_API_KEY`, live `/v1/models`
  discovery, `TYPESAFE_BASE_URL` override.
- **Answer cache** (`judgment/cache.ts`): SQLite keyed by
  `(sha256(canonical state), model, question name, type, instruction, criteria)`; a re-ask
  of any question about an identical state is answered locally for free; only unanswered
  questions reach the provider.
- **Telemetry/billing**: every attempt is attributed exactly once (native request or chat
  `onAttempt`), reaching both the session ledger and a `judgment` span.
- **Consumers** (the payoff): `find` semantic grep (`tools/jfind`, cascaded 4-phase judge),
  `auto-thinking` difficulty classifier, `auto-graph` chart planner, `unexpected-stop`
  classifier, `ttsr` judged rules, `stats` frustration classifier, `eval` `judge()` /
  `judge_batch()`.

The fork has **none** of this (`grep -i typesafe|jev|judge` over `crates/` -> nothing but
unrelated words like "judge the staleness"). So the design is greenfield.

---

## 3. Architecture for the fork

### 3.1 Layering (mirror `lsp`/`graph`/`edit`)

```
pa-jev (NEW crate)          wire types + client + question builders + answer parsing
   ^
pa-core::jev (NEW module)   Judge trait/role-chain resolver + JevTool (AgentTool)
   ^
pa-core::session_engine     create_session wiring (gate + push + native_tool_names)
   ^
internal call sites         edit guards, provider routing, cache anomalies, login check
```

**New crate `crates/pa-jev`** (`#![forbid(unsafe_code)]`, pedantic-clean, like the others):
- `Question::{Noul,Choice,Score}` builders and `Answer::{Noul,Choice,Score}` parsed types
  (serde), field-for-field with the API reference so a misspelt field is a compile error.
- `JevClient` over `reqwest`: `systemone(state, questions, opts) -> Result<JudgmentResult>`,
  with `maxQuestions` chunking (<= 4 in flight), retry (`408/429/5xx` + connection/timeout,
  `retry-after`-aware, jittered backoff), 401/403 rotation hook, `x-typesafe-request-id`
  capture, and `Usage { input_tokens, output_tokens, cost }` where cost =
  `input_tokens/1e6 * 0.042`.
- A pure `decode` + a `Judge` trait so a *prompted fallback* (an ordinary chat model) can
  implement the same interface for degraded mode — but the role chain must prefer native
  (see 3.3).
- Optional: a tiny local answer cache (`rusqlite` is already in the workspace via
  `pa-graph`; or reuse `pa-graph`'s store) keyed as oh-my-pi does. Ship it in phase 2.

**`pa-core::jev`** module:
- `resolve_judge(settings, registry) -> Option<Arc<dyn Judge>>`: reads a `jev` settings
  block + `judge` role/`auxiliaryModel`-style selector, resolves the provider credential
  through the existing `ModelRegistry::get_api_key_and_headers` / `AuthStorage`, and
  returns a client. `None` when disabled/unconfigured (callers degrade).
- `JevTool` implementing `AgentTool` (name `jev`, label `Judge`), actions:
  `ask` (state + questions), `classify` (a one-question sugar), `route`, `check`.
  Markers: none of `CanEdit`; add a new `ToolMarker::DoesNotRequireActiveProject`-style
  marker or reuse existing ones. `parameters()` = JSON schema; `execute()` runs the client
  off the async executor (`tokio::task::spawn_blocking` only if sync; with `reqwest` it is
  already async).

### 3.2 Where it plugs into the session engine

`crates/pa-core/src/session_engine/engine.rs` (`create_session`, ~line 213):
- Capture gates before `settings` moves into the loader (exactly like `lsp_enabled` /
  `graph_enabled` at lines 284–291): `let jev_enabled = settings.get_jev_enabled();` plus
  `jev_default_model`, `jev_base_url`, `jev_max_questions`.
- After the `edit` push (line ~560), push the `jev` tool when enabled and record it in
  `native_tool_names` so it surfaces on `get_session_stats.nativeTools` (already wired:
  `pa-daemon/src/worker/commands.rs:464`).
- The engine already holds `native_tool_names` and the profile filter runs afterward
  (lines 563–571), so `tool_profile` contexts/modes can include/exclude `jev` for free.

`crates/pa-core/src/settings/types.rs`: add a fork-only `JevSettings` struct
(`enabled: Option<bool>` defaulting **OFF**, `model`, `baseUrl`, `maxQuestions`,
`confidenceThreshold`, `cacheEnabled`) and register `jev` in the `KNOWN_FIELDS` list in
`settings/load.rs`. Add getters in `settings/manager.rs` next to `get_lsp_enabled`.
(`lsp`/`graph`/`edit` default ON; **JEV should default OFF** — it costs money per call and
is a third-party endpoint.)

`crates/pa-ai/src/models_fork.rs`: if we want JEV to be selectable as a *model* (for the
`judge` role and for `models.json` overrides) rather than only a tool, add a `typesafe`
provider entry (`api: "typesafe"`, base `https://api.typesafe.ai`, model `jev-latest`,
cost `(0.042, 0.0, 0.0, 0.0)`) to `FORK_RAW_MODELS`, add `"typesafe"` to `fork_providers()`,
and add `TYPESAFE_API_KEY` to `env_api_keys.rs`. **Caveat:** `pa-ai` has no judgment `Provider`
in its `Provider` registry (`registry.rs`), so a `typesafe` model would fail `stream()` —
see 3.3.

### 3.3 Native judgment vs. "just another model"

Two viable shapes:

- **(A) Tool + internal layer (recommended first).** JEV is an `AgentTool` + a Rust `Judge`
  used by internal call sites. No change to the chat streaming path. Lowest risk; matches
  the fork's "new files, no upstream edits" rule.
- **(B) A real `judge` model kind.** Requires: a `Model.kind`/`judge` concept (the fork's
  `Model` has no `kind` field today — `pa-types/src/ai/mod.rs`), a `typesafe` judgment
  `Provider`, and a `modelRoles.judge` resolver. This is the faithful oh-my-pi port but is a
  much larger change and touches upstream-shaped code. Defer to phase 3.

Start with (A); keep the `Judge` trait shaped so (B) can slot in later (the trait is the
seam). If we do (B), **replicate oh-my-pi's rule**: once a native judge is in the role
chain, never fall through to a prompted model for that judgment.

---

## 4. Concrete integration points (file -> function)

### 4a. Muse login verification -> JEV checks the credential/key shape before commit
- `crates/pa-ai/src/oauth/muse.rs::login_muse` / `mint_api_key` already validates the minted
  key shape (`LLM|` prefix, `is_subs_active`). JEV does **not** replace that (it's a
  network/credential check, not a judgment). The JEV-shaped use is: when the login panel
  is about to persist a credential, ask one `noul` — *"Does this error text indicate an
  account/entitlement problem rather than a transient network failure?"* — to classify a
  failed login into `retry | reauth | entitlement` for the panel's guidance. Low value;
  **optional**.

### 4b. Edit hybrid guards -> JEV validates edits (seen-lines / stale-tag)
- `crates/pa-core/src/tools/edit_hybrid.rs::EditHybridTool::run` (~line 176) and
  `pa-edit/src/session.rs` (seen-lines at 364–396, stale-tag handling). The guards today are
  deterministic (tag mismatch -> error; no-op loop -> escalate). **JEV adds a *semantic*
  layer, not a replacement:** after a successful apply, optionally ask a `choice`
  (`looks_correct | suspicious | clearly_wrong`) or a `score` over "does the diff do what
  the edit intended?" and surface a warning in the tool result text (the `outcome_result`
  path already appends `WARNING: parse regression ...`). This is a *post-hoc reviewer*, so
  it must be **off the critical path** and never block a write. Gate by
  `jev.editReview.enabled`.
- **Do not** put JEV inside the deterministic guard decision — a probabilistic judge must
  never be the reason a write is denied or allowed. Guard = deterministic; JEV = advisory
  annotation only.

### 4c. Provider failover -> JEV routing (which provider for this task)
- `crates/pa-core/src/session_engine/provider_failover.rs::run_turn_with_provider_failover`
  and `crates/pa-core/src/models/resolver.rs::failover_candidates`. Today the order is pure
  catalog order. A JEV `choice` over the candidate providers ("Which of these providers is
  most likely to serve this request given the failure signature?") is **not** a good fit for
  the retry loop: the loop must be deterministic, synchronous, and offline-safe, and the
  failure classes (`payment_required`, `unsupported_tool`, context overflow) are already
  handled by pure predicates. **Recommendation: do not inject JEV into failover.** If a
  "smart routing" feature is wanted, make it a *pre-turn* advisory in the `model`/`effort`
  UI, not in the failover path.
- A genuinely useful JEV call here: classify an ambiguous provider error **text** into
  `retry | reroute | permanent` for errors the string predicates miss (unknown gateways).
  That is a fallback *inside* `error_classify` (`session_engine/error_classify.rs`), gated
  and best-effort. Still optional.

### 4d. `/cache` statistics -> JEV classifies cache anomalies
- `crates/pa-tui/src/info_commands/cache.rs::cache_rows` renders the `get_cache_stats`
  payload; `crates/pa-daemon/src/state_getters.rs::handle_get_cache_stats` builds it. A JEV
  `choice`/`noul` over the per-model rows ("Is this cache-hit rate anomalous for this
  provider/model?") can add a one-line annotation ("cache reads collapsed after the model
  switch"). Nice-to-have; the data is tiny and the heuristic (`hit% < X and input > Y`) is
  cheap. **Optional, low priority.** Keep it as an annotation, never a rewrite of the
  numbers.

### 4e. Native tools -> JEV as arbiter for lsp/edit/graph selection
This is the **highest-value** internal use and mirrors oh-my-pi's `find`/`auto-thinking`:
- **Tool-selection arbiter**: the fork already ships `lsp` (symbols/find_symbol/references/
  rename/diagnostics), `graph` (search/trace/architecture/coverage/changes), and `edit`
  (modes). A JEV `choice` over `{lsp.find_symbol, graph.search, bash.grep, ...}` given the
  user's natural-language intent gives the model a routing hint in the system prompt or a
  `jev.route` tool result. oh-my-pi's `find` proves the pattern: lexical scan -> JEV judges
  candidates -> verbatim verification, with the judge used only as a routing signal.
- **`find`-style semantic search over the graph**: `pa-graph` already has FTS5/BM25/RRF/MMR.
  Add a JEV re-rank pass over the top-K candidates (a `noul` "does this symbol/passage
  answer the query?"), exactly like oh-my-pi's cascade. This is where JEV's speed and price
  pay off: K candidates judged in one request.
- **Symbolic-edit arbiter**: given a requested change, a JEV `choice` over
  `{replace_symbol_body, safe_delete, patch, sloppy}` picks the safest `pa-edit` mode. The
  edit engine already exposes the mode enum; JEV only *suggests* the `mode` argument.

### 4f. (Bonus, high value) auto-thinking / difficulty classifier
The fork has no per-prompt difficulty classifier. oh-my-pi's `auto-thinking/classifier.ts`
asks one `choice` (`trivial|moderate|hard` -> `low|high|xhigh`) to pick the thinking level.
Porting this to the fork (a `jev` call at turn admission that sets `thinking_level`) is a
small, self-contained, high-leverage feature — and it is exactly the kind of "fast deciding
layer" the task asks for.

### 4g. (Bonus) `eval` `judge()` bridge
oh-my-pi exposes `judge(state, questions)` to the Python REPL. The fork's kernel
(`crates/pa-core/src/kernel/`) could register a `judge` host request so the Python RLM layer
can make typed decisions inline. High value for the fork's Python-orchestration identity;
medium effort.

---

## 5. Tests (what to cover)

**`pa-jev` unit tests (no network):**
- Wire encode/decode round-trip for all three primitives against the doc fixtures
  (including `Score.score` = weighted index, `probabilities` keyed by index, `noul` with no
  `confidence`).
- `Usage` cost = `input_tokens/1e6 * 0.042`; output never billed.
- Chunking: a request with `maxQuestions=16` and 40 questions splits into 3, merges answers,
  sums usage; a failing chunk aborts siblings.
- Retry: 429 with `retry-after` honored; 5xx retried; 400/404/422 not retried (422 triggers
  a split, not a plain retry).
- Error mapping: each status -> the right error variant; `x-typesafe-request-id` captured.
- 401/403 rotation hook fires once.
- Missing answer for a requested id -> typed error, not a panic.

**`pa-core::jev` tests:**
- `resolve_judge` returns `None` when disabled / no key; returns a client when
  `TYPESAFE_API_KEY` or a stored credential exists (use the same `AuthStorage` in-memory
  fixtures the auth tests use).
- Role chain prefers native over a prompted fallback (if (B) is built).
- Answer cache: an identical re-ask is served locally; only unanswered questions go out.

**Session-engine tests:**
- `create_session` pushes `jev` only when `jev.enabled`; `native_tool_names` includes it;
  the `tool_profile` filter can exclude it.
- Default-OFF: a session with no `jev` setting has no `jev` tool and makes zero network
  calls.

**Integration (gated, network-optional):**
- A `JevTool::execute` end-to-end against a mock HTTP server (the fork already has reqwest
  test servers) asserting the exact request body and a parsed answer.
- A live smoke test behind `TYPESAFE_API_KEY`, skipped when unset (mirror `pa-lsp`'s
  `live_smoke.rs` posture).
- Call-site tests: edit-review annotation appears when enabled and never blocks a write;
  `find`-style re-rank orders candidates by judged probability.

Follow the merge gate in `Makefile::check` (`fmt --check`, `clippy -D warnings`, `test
--workspace`, release build).

---

## 6. Economics — when JEV is worth it vs an LLM

| Situation | Verdict |
|-----------|---------|
| One decision per turn, small state (hundreds of tokens) | **JEV wins**: ~$0.00002 and ~200–350 ms vs a chat turn's seconds and 1.4–18x cost. |
| Batch of independent questions about one state | **JEV strongly wins**: parallel, state billed once, N answers ≈ one call. |
| Ambiguous text classification / routing / guardrail | **JEV wins** — calibrated `confidence` you can threshold on. |
| Needs prose, reasoning, or tool calls | **LLM** — JEV generates no text and supports no tools. |
| Very low volume (a few decisions/day) | **Either** — the saving is cents/month; choose on latency/reliability, not price. |
| Needs to be offline / deterministic | **Neither** — keep the deterministic guard; JEV is advisory only. |
| Cost is dominated by `state` length | Watch it: the bill scales *only* with state+questions. Truncate state; never send a whole file when a sketch answers the question (oh-my-pi sketches to ~384 bytes/card). |

**Rule of thumb for the fork:** put JEV where the fork today (a) burns a full LLM turn on a
bounded decision (difficulty, routing, "is this an unexpected stop"), or (b) applies a crude
heuristic (cache anomaly, edit correctness). Do **not** put it where the decision must be
deterministic (failover, edit guards) or where prose is required. Default OFF; enable per
feature.

---

## 7. Recommended phasing

1. **Phase 1 — client + tool.** `pa-jev` crate (types, client, chunking, retry), `pa-core::jev`
   (`Judge` trait, `resolve_judge`, `JevTool`), settings (`jev.enabled` default OFF),
   `create_session` wiring + `native_tool_names`, tests. No internal call sites yet.
2. **Phase 2 — one high-value consumer.** Port the **auto-thinking difficulty classifier**
   (4f) and/or the **`find`-style graph re-rank** (4e). Add the answer cache.
3. **Phase 3 — optional model role.** `typesafe` provider in `models_fork.rs`, a judgment
   `Provider`, `modelRoles.judge`; then the edit-review annotation (4b) and cache anomaly
   (4d).
4. **Deliberately skip:** JEV inside provider failover (4c) and inside edit guards — keep
   those deterministic.

**Dependencies:** `reqwest` (already in `pa-core`), `serde`/`serde_json` (already), the
workspace `AuthStorage` for the key. No third-party JEV SDK. `TYPESAFE_API_KEY` added to
`env_api_keys.rs` if the model-role path (B) is built.
