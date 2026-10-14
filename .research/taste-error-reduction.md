# Taste admission gate: как снижать ошибки и повторные правки в Prime Agent

## 0. Короткий вывод

**Claim.** Самый полезный импорт из Command Code Taste для Prime — не “добавить ещё одну память” и не верить маркетинговому `taste-1`, а поставить **admission gate между сигналом пользователя и записью будущего поведения**: raw correction/edit/reject → structured candidate → admission/re-score/conflict check → scoped readable harness entry → generation/review uses only admitted entries.

**Почему это снижает ошибки.** Gate уменьшает четыре класса повторных правок: (1) LLM выдумывает предпочтение из тонкого evidence; (2) preference дрейфует, а старая запись не переоценивается; (3) локальное правило случайно становится глобальным; (4) “taste” просто вставляется в prompt и либо игнорируется, либо переобобщается. Это уже было отмечено в предыдущем research: “preference learning separated from memory writing” и fixed train/eval split для проверки taste, а не unit tests (`/tmp/prime-architecture-research/patterns-draft.md:5-8`).

**Для Prime.** Встраивать gate надо в `pa-core` перед `apply_refinement_plan`, а хранить новые поля в уже существующем `HarnessEntry.metadata`, без изменения top-level schema. Ranking должен учитывать admitted taste metadata, но digest должен оставаться читаемым: summary в `content`, машинные поля в `metadata`.

---

## 1. Что уже найдено в соседних research-файлах, чтобы не дублировать

- Previous synthesis уже выделил Command Code Taste как “feedback learning of personal preferences”, где raw behavior проходит через admission/classification, а позже generation/review uses only admitted preferences (`/tmp/prime-architecture-research/harness-competitors.md:7-13`).
- Там же зафиксировано, что Prime уже имеет continual harness/refinement/memories/prompt notes/skills/subagents, поэтому borrowing должен быть минимальным: дисциплинированный loop вокруг существующего refinement, а не новый storage subsystem (`/tmp/prime-architecture-research/harness-competitors.md:13`).
- Existing Prime architecture review уже показал gap: preferences/taste не являются first-class объектами и остаются memories/prompt notes, если агент вручную их так структурирует (`/tmp/prime-architecture-research/prime-architecture.md:156-157`).
- Existing pattern note уже предлагает measured outcome: меньше repeated taste violations, ближе к user decision style, меньше токенов за счёт structured/scoped preferences (`/tmp/prime-architecture-research/prime-patterns.md:141-157`).
- Existing review предупреждает: current harness ranking — lexical substring + IDF, не semantic/outcome-aware (`/tmp/prime-architecture-research/review.md:56-60`, `/tmp/prime-architecture-research/prime-architecture.md:152`).

---

## 2. Command Code Taste: official claims vs independent scrutiny

### 2.1 Official claim: Taste как classifier/admission layer

**Claim from official docs.** Command Code говорит, что `taste-1` не LLM, не генерирует текст и “classifies taste learnings”: real preference/noise, category, confidence (`/tmp/prime-architecture-research/taste-official.txt:104-113`, URL: https://commandcode.ai/docs/taste).

**Claim from official docs.** Symbolic layer allegedly knows category structure, confidence scoring, relations, contradictions; meta loop re-scores old learnings, confidence rises with repeated evidence and falls on contradiction (`/tmp/prime-architecture-research/taste-official.txt:119-124`, URL: https://commandcode.ai/docs/taste).

**Claim from official docs.** Docs name the LLM failure modes directly: LLM fills gaps with generic developer preferences, gives different profiles run-to-run, writes uncalibrated confidence, and starts fresh without prior decisions (`/tmp/prime-architecture-research/taste-official.txt:127-136`, URL: https://commandcode.ai/docs/taste).

**Claim from official docs.** The advertised comparison is: fixed category set prevents arbitrary category invention, same input should produce same decision, calibrated confidence drives high-confidence enforcement / low-confidence hints / conflict flags (`/tmp/prime-architecture-research/taste-official.txt:139-153`, URL: https://commandcode.ai/docs/taste).

**Claim from official docs.** The stated pipeline is four steps: user prompts/accepts/rejects/edits/correction diffs → LLM extracts structured signals → `taste-1` classifies real preference/category/confidence → LLM writes readable markdown → CLI stores `taste.md` (`/tmp/prime-architecture-research/taste-official.txt:155-239`, `/tmp/prime-architecture-research/taste-official.txt:295-307`, URL: https://commandcode.ai/docs/taste).

### 2.2 Official storage/scoping facts

- Taste is plain markdown, editable/lintable/shareable with `npx taste lint` and `npx taste push` (`/tmp/prime-architecture-research/taste-official.txt:368-373`, URL: https://commandcode.ai/docs/taste).
- Settings are project/user scoped: project writes `.commandcode/settings.json`, user writes `~/.commandcode/config.json`; project setting takes precedence over user default (`/tmp/prime-architecture-research/taste-official.txt:383-399`, `/tmp/prime-architecture-research/taste-official.txt:413-436`, URL: https://commandcode.ai/docs/taste).
- Packages are categorized (`cli`, `typescript`, `architecture`, etc.) and have project/global/remote scopes: `.commandcode/taste/`, `~/.commandcode/taste/`, `commandcode.ai/username/taste` (`/tmp/prime-architecture-research/taste-official.txt:509-534`, URL: https://commandcode.ai/docs/taste).
- Project package layout stores `taste.md` plus category-specific `taste.md` files (`/tmp/prime-architecture-research/taste-official.txt:551-596`, URL: https://commandcode.ai/docs/taste).
- CLI supports enable/disable/push/pull/list/lint/open across project/global/remote packages (`/tmp/prime-architecture-research/taste-official.txt:650-699`, URL: https://commandcode.ai/docs/taste/commands).
- Push/pull merge behavior updates learnings whose confidence changed; lint validates confidence values between 0 and 1 (`/tmp/prime-architecture-research/taste-official.txt:783-894`, `/tmp/prime-architecture-research/taste-official.txt:1042-1056`, URL: https://commandcode.ai/docs/taste/commands).

### 2.3 Memory vs Taste in Command Code docs

**Claim from official docs.** Command Code Memory is explicit written instruction in `AGENTS.md`, loaded into every turn; it is distinct from Taste, which is learned style rather than written rules (URL: https://commandcode.ai/docs/memory).  
**Why this matters.** Prime should not treat taste as “just another always-on AGENTS.md note”: it should be admitted, scoped, ranked, and used as a review signal.

**Claim from official docs.** `/import` copies existing setup, while `/learn-taste` learns “the way you work” from past sessions; docs explicitly distinguish copied config from learned style (URL: https://commandcode.ai/docs/import; URL: https://commandcode.ai/docs/reference/cli).

### 2.4 Independent scrutiny: do not copy the marketing claim

**Independent claim.** Safzan Pirani’s teardown says the shipped `command-code@1.58.0` package contains no model id/routing/catalog entry called `taste-1`; according to that article, taste learning is git/session transcript → prompt → ordinary selected model → `.commandcode/taste/taste.md` → later prompt injection (URL: https://cmd.safzan.dev/; gist search result: https://gist.github.com/safzanpirani/5bd7a77ce304edb16f768982bb26bcc0).

**Independent claim.** Heatdrop summarizes the public repo as useful/interesting but says “neuro-symbolic AI” and “taste-1” are branded terms with no technical detail in the README, no benchmarks/comparison data/open weights, and that skeptics should wait for reverse-engineering (URL: https://heatdrop.ai/repo/CommandCodeAI/command-code).

**Independent claim.** AIWiki’s article says the taste-1 description is disputed and summarizes the teardown as finding no `taste-1` model identifier in the package, while also noting the company answered with a “taste-1 vs Jev” post (URL: https://aiwiki.ai/wiki/command_code; official response URL: https://commandcode.ai/blog/taste-1-vs-jev).

**Conclusion for Prime.** Borrow the **mechanism** (admission + scope + confidence + contradiction + review checklist), not the unverifiable product claim “neuro-symbolic RL model”. A transparent deterministic gate is enough for v1.

---

## 3. Механика: как admission gate снижает ошибки

### 3.1 Ошибка: LLM выдумывает preference из слабого evidence

**Claim.** Raw LLM refinement is prone to “preference hallucination”: after one edit it writes “user prefers X” because the text sounds plausible. Command Code docs call this out directly: where evidence is thin, an LLM reaches for what developers usually prefer (`/tmp/prime-architecture-research/taste-official.txt:127-130`).

**Mechanism.** Admission gate forces a non-generative decision before persistence:

1. Candidate must include `signal_kind` (`explicit_user_preference`, `user_correction`, `edit_diff`, `reject`, `accept`, `manual_refine`, etc.).
2. Candidate must include bounded category enum, e.g. `architecture | rust | testing | naming | workflow | cli | docs | ui | other`.
3. Candidate must include `polarity` (`prefer | avoid | require | forbid`) and a target.
4. Candidate must include evidence references, not just rationale prose.
5. Confidence is capped by evidence quality. A single ambiguous edit can become `hint`, not `admitted`.
6. Gate can return `rejected` or `no_learning`.

**Why it reduces repeated fixes.** Unsupported preferences never enter the future prompt/review path. Low-confidence hints may be shown to a reviewer or used as search terms, but not enforced as a rule.

### 3.2 Ошибка: self-rated confidence is not calibrated

**Claim.** Command Code docs say an LLM-written confidence number is just a number in the style of probability, not calibrated to correctness (`/tmp/prime-architecture-research/taste-official.txt:133-146`).

**Mechanism.** In Prime v1, confidence should be computed from evidence, not trusted from the planner:

- explicit user statement (“remember/prefer/never”) can start high (`0.8-0.95`);
- user correction after assistant output starts medium-high (`0.65-0.85`), depending on specificity;
- inferred edit diff without explicit preference starts low/medium (`0.35-0.65`);
- accept/reject without explanation is weak support (`0.1-0.4`);
- contradiction reduces confidence or marks `conflicted`.

These constants should be treated as product defaults to validate, not universal truth. They are the initial “admission policy”, not an ML model.

### 3.3 Ошибка: contradictions create stale or duplicate rules

**Claim.** Official docs frame contradiction handling as a core symbolic function: learnings relate to each other, contradictions are detected, and old learnings are re-scored (`/tmp/prime-architecture-research/taste-official.txt:119-124`, `/tmp/prime-architecture-research/taste-official.txt:149-153`).

**Mechanism.** Admission gate should compare each candidate to existing taste atoms:

- same category + same target + opposite polarity → conflict;
- narrower project rule beats broader global rule only inside matching project scope;
- newer explicit correction can down-score older inferred rule;
- repeated same signal increments support and may promote `hint` → `admitted`;
- unresolved conflict makes both entries `status=conflicted` or keeps one admitted and the other as `hint`, with `contradicts` links.

**Why it reduces repeated fixes.** The agent stops applying an old preference after the user reverses it. It also stops accumulating near-duplicate “rules” that fight each other.

### 3.4 Ошибка: project-specific workaround becomes global preference

**Claim.** Command Code distinguishes project/global scopes in taste package paths and settings (`/tmp/prime-architecture-research/taste-official.txt:383-399`, `/tmp/prime-architecture-research/taste-official.txt:509-534`). Prime already has local/global harness scope policy and previous research says scoping is required to avoid accidental globalized workarounds (`/tmp/prime-architecture-research/patterns-draft.md:6-8`).

**Mechanism.** Gate should require stronger evidence for global:

- `scope=local/session`: current task facts, temporary blocker, current project only.
- `scope=project`: evidence names repo/workspace/path or repeats inside same repo.
- `scope=global`: explicit user says cross-session/global, or repeated evidence across projects/sessions.

**Why it reduces repeated fixes.** A one-off Rust workspace workaround does not later contaminate a TypeScript repo or another user’s preference profile.

### 3.5 Ошибка: taste as prompt injection, not verification

**Claim.** Independent teardown says Command Code’s shipped mechanism writes markdown and pastes it into the later system prompt wrapped as taste (URL: https://cmd.safzan.dev/). Even if useful, this is still mainly conditioning generation.

**Mechanism Prime should prefer.** Treat admitted taste as a **review checklist** in addition to optional prompt context:

- Before generation: retrieve only relevant high-confidence taste entries and show them as “preferences to consider,” not absolute law.
- After generation/diff: run a local checklist over relevant entries: “violated / satisfied / not applicable / conflict”.
- Low-confidence entries can be hints for review, not generation constraints.
- Conflicted entries are surfaced as “ask user / do not enforce”.

**Why it reduces repeated fixes.** The user’s next correction is often a review failure (“you again used X / ignored Y”). A checklist catches that before the user does. Prompt injection alone cannot tell whether the generated answer actually followed the preference.

---

## 4. Scientific analogs and why they support the gate

### 4.1 RLHF / preference learning

**Claim.** InstructGPT/RLHF uses human demonstrations and rankings, trains a preference/reward model, and then optimizes the model with that reward; the paper reports that a 1.3B InstructGPT was preferred to 175B GPT-3 in human evaluation on their prompt distribution (URL: https://arxiv.org/abs/2203.02155).

**Relevance.** Taste admission is a small online analogue: separate the preference signal from the generator. For Prime v1, do not train a model; separate the decision boundary anyway.

### 4.2 Constitutional AI / AI feedback

**Claim.** Constitutional AI uses a list of principles, self-critiques/revisions in supervised learning, then AI preference judgments and RL from AI feedback (RLAIF); the abstract emphasizes control with fewer human labels (URL: https://arxiv.org/abs/2212.08073).

**Relevance.** This supports the review-checklist framing: generation and critique are different steps. Taste should be a critique/admission/checklist layer, not only text stuffed into the prompt.

### 4.3 PAHF / personalization evaluation

**Claim from vault.** PAHF is a continual personalization framework with explicit per-user memory and two feedback channels: pre-action clarification and post-action correction (`/root/vault/knowledge/pahf-framework-for-continual-agent-personalization.md:17-25`, URL: https://arxiv.org/abs/2602.16173).

**Claim from vault.** PAHF’s theoretical note says without post-action feedback there is linear regret under drift, without pre-action feedback regret is linear in ambiguity rate, and both channels produce bounded regret (`/root/vault/knowledge/pahf-framework-for-continual-agent-personalization.md:33-41`).

**Claim from vault/project.** PAHF reports better success rates than no-memory and single-channel baselines in embodied and shopping benchmarks (`/root/vault/knowledge/pahf-framework-for-continual-agent-personalization.md:43-50`, project URL: https://personalized-ai.github.io/).

**Limitations.** The same vault note flags no real-user validation, privacy governance, scalability analysis, adversarial resilience, and noisy feedback handling (`/root/vault/knowledge/pahf-framework-for-continual-agent-personalization.md:65-71`).

**Relevance.** Prime’s taste gate should support both channels:

- pre-action: if relevant taste is ambiguous/conflicted, ask user or downgrade to hint;
- post-action: user corrections update confidence and contradictions.

**Method note.** Jina MCP `search_arxiv` returned 401 in this environment, so the arXiv facts above were checked through Keenable fetch/search plus the local PAHF vault note. The required arXiv URLs are still included.

---

## 5. Code-evidence in Prime: current path and gaps

### 5.1 Current refine path already has a good insertion point

**Code-evidence.** `execute_refinement_with_rows` builds planning state, calls `plan_refinement`, then strips display prefixes (`crates/pa-core/src/session_engine/refine.rs:337-370`). It then resolves target scope/dir and applies the plan under `update_harness_state` (`crates/pa-core/src/session_engine/refine.rs:372-407`).

**Claim.** The taste admission gate should sit **after** `plan = strip_display_prefixes(plan)` and **before** `update_harness_state(... apply_refinement_plan ...)`.

**Why here.** At this point Prime has: proposed edits, requested scope, planning state, refinement history, baseline state, and before applying state. The gate can filter/annotate preference edits without changing the LLM planning seam.

### 5.2 Current auto-refine gate is not enough

**Code-evidence.** `review_auto_refine` is a checkpoint gate that decides whether to run `/refine` at all; it sees trigger, harness state, history, conversation, and returns `shouldRefine` plus instructions (`crates/pa-core/src/refinement/executor.rs:348-386`).

**Claim.** That gate rejects one-off noise at the run level, but it does not classify each candidate preference, assign polarity/confidence, or resolve contradictions. Taste admission is a second, narrower gate over proposed harness edits.

### 5.3 Metadata can hold taste fields without schema break

**Code-evidence.** `HarnessEntry` already has `metadata: serde_json::Map<String, serde_json::Value>` (`crates/pa-core/src/refinement/mod.rs:51-63`). The top-level `HarnessState` schema is stable and entries are deserialized as `HarnessEntry` values (`crates/pa-core/src/refinement/mod.rs:84-91`, `crates/pa-core/src/refinement/mod.rs:175-221`).

**Code-evidence.** `apply_refinement_proposal` preserves `edit.metadata`, falls back to previous entry metadata on update, or uses default metadata (`crates/pa-core/src/refinement/planner.rs:418-455`). It records applied changes and refinement event evidence/outcome (`crates/pa-core/src/refinement/planner.rs:471-490`).

**Claim.** No new top-level schema or new `RefinementKind::Taste` is needed for v1. Use `kind=memory`, `path=taste/<category>` or `path=projects/<repo>/taste/<category>`, readable `content`, and machine fields under `metadata.taste`.

### 5.4 Ranking currently ignores metadata

**Code-evidence.** Harness ranking scores only title/content/path/id: `title`, `content`, and `identifier` are the scored strings (`crates/pa-core/src/refinement/ranking.rs:144-175`). The digest renders title/path/version/content, not metadata (`crates/pa-core/src/refinement/ranking.rs:312-322`). Refinement prompt overview similarly renders content/ref/args, not metadata (`crates/pa-core/src/refinement/executor.rs:41-85`).

**Claim.** If taste facts live only in metadata, they will not be retrieved or shown. Therefore each taste entry needs a short natural-language `content` summary, and `ranking.rs` should optionally include selected `metadata.taste` terms in rank text.

### 5.5 Critical path constraint

**Code-evidence.** Repo rules say user-visible paths must not await network; render path hands HTTP/socket work to background workers and returns immediately (`AGENTS.md:96-100`).

**Claim.** Do not add a remote model/API decision to prompt admission or first paint. Start with deterministic admission during `/refine`/auto-refine. If a model-based gate is added later, run it only on the existing refinement/background path, not on prompt assembly.

---

## 6. Concrete Prime design

### 6.1 Minimal data shape in `HarnessEntry.metadata`

Use existing `metadata` map. Suggested shape:

```json
{
  "taste": {
    "schema": 1,
    "status": "admitted",
    "category": "architecture",
    "polarity": "avoid",
    "target": "single-use helper methods",
    "confidence": 0.82,
    "supportCount": 2,
    "scope": "project",
    "scopeReason": "Observed in prime-agent repo corrections; not claimed cross-project.",
    "evidence": [
      {
        "kind": "user_correction",
        "summary": "User rejected a helper method used once and asked to inline it.",
        "sessionId": "...",
        "messageIds": ["..."],
        "files": ["crates/pa-core/src/..."],
        "observedAt": "2026-10-12T...Z"
      }
    ],
    "contradicts": [],
    "lastScoredAt": "2026-10-12T...Z"
  }
}
```

**Rules.**

- `content` remains human-readable, e.g. “In this project, avoid extracting single-use helpers; inline the code unless reuse is real.”
- `metadata.taste.evidence.summary` must be short. Store message ids/file paths rather than full prompts when possible.
- `polarity` is required because “prefer X” and “avoid X” need different review checks.
- `confidence` is required and must be `0..1`, mirroring Command Code’s linted confidence range (`/tmp/prime-architecture-research/taste-official.txt:1042-1056`).
- `status=hint` entries are not enforced in generation; `status=conflicted` entries are not enforced without user clarification.

### 6.2 Admission algorithm v1

**Input.** Proposed `RefinementEdit`s, planning state, history, requested scope, baseline state.

**Output.** Same proposal shape, but preference edits are annotated or rejected before apply.

Pseudo-policy:

1. Detect candidate preference edits:
   - `metadata.taste` exists, or
   - path starts with `taste/`, or
   - title/content contains explicit “user prefers/avoid/always/never” pattern.
2. Require evidence:
   - if no evidence → reject or convert to `hint` with `confidence <= 0.35`;
   - if explicit user directive → admit can pass with one strong signal;
   - if inferred from edit diff → require repeat or keep as `hint`.
3. Normalize category/polarity/target.
4. Scope check:
   - global requires explicit global instruction or cross-session/cross-project evidence;
   - otherwise force local/project path.
5. Conflict check against existing `metadata.taste` entries:
   - same category/target/opposite polarity → down-score or mark conflict;
   - repeated same signal → increase support and confidence.
6. Apply thresholds:
   - `confidence >= 0.75` and no hard conflict → `admitted`;
   - `0.45 <= confidence < 0.75` → `hint`;
   - `< 0.45`, no evidence, or scope mismatch → `rejected`.
7. Keep rejected edits out of the plan or record them as non-applied planned rows with a clear error/reason.

### 6.3 Where to put code

**`crates/pa-core/src/refinement/taste_admission.rs`**

- Private module under `refinement`, owned by harness/refinement concern.
- Pure functions, no network, no async requirement:
  - `admit_taste_edits(proposal: &mut RefinementProposal, state: &HarnessState, history: &[RefinementResult], requested_scope: HarnessScope) -> TasteAdmissionReport`
  - helper functions for `TasteStatus`, `TastePolarity`, `TasteCategory` can be private enums serialized only into metadata strings.
- Avoid new public API unless tests need `pub(crate)`.

**`crates/pa-core/src/session_engine/refine.rs`**

Insert after line 370:

```rust
plan = strip_display_prefixes(plan);
let taste_report = crate::refinement::admit_taste_edits(
    &mut plan.proposal,
    &planning_state,
    &history,
    requested_scope,
);
```

Then include a compact admission summary in the refinement result rationale/outcome or in non-applied edit errors. This keeps the existing plan → re-read → apply → persist flow intact (`crates/pa-core/src/session_engine/refine.rs:246-255`, `crates/pa-core/src/session_engine/refine.rs:395-407`).

**`crates/pa-core/src/refinement/ranking.rs`**

Add a helper used by both IDF and score:

```rust
fn entry_rank_text(entry: &HarnessEntry) -> String {
    // existing title/content/path/id + selected metadata.taste.category/target/polarity/status
}
```

Then replace direct title/content/identifier checks in `harness_query_term_idf` and `score_harness_entry_for_query` with the helper. Keep digest rendering compact; do not dump full evidence. The current scoring location is `crates/pa-core/src/refinement/ranking.rs:108-175`, and digest rendering is `crates/pa-core/src/refinement/ranking.rs:195-323`.

**Important.** If metadata terms affect ranking, tests must prove metadata cannot outrank direct content wildly. Use small bounded boost only for `status=admitted` and matching scope/category.

### 6.4 How generation/review should consume admitted taste

- Prompt retrieval: include admitted relevant taste as “preferences to consider”, not absolute law.
- Review checklist: after draft/diff, check relevant admitted taste entries and fix violations before final response.
- Low confidence: show as “hint”, not mandatory.
- Conflict: do not enforce; ask user if action depends on it.
- Negative polarity: convert to explicit checks (“Does this introduce X?”) rather than vague style prose.

---

## 7. Evaluation plan: held-out user corrections

**Claim.** Existing research already says tests passing do not verify taste; use held-out tasks/user corrections and blind preference comparisons (`/tmp/prime-architecture-research/PATTERNS.md:21-23`, `/tmp/prime-architecture-research/patterns-draft.md:8`).

### 7.1 Dataset

- Build chronological train/test split from user corrections, rejected outputs, explicit “prefer/avoid” statements, and post-edit diffs.
- Train/admit taste only on the earlier split.
- Test on later tasks where the user corrected the same class of behavior.
- Keep project/repo split where possible to measure scope leakage.

### 7.2 Metrics

- **Repeated correction rate:** same correction class appears again after an admitted taste atom exists.
- **False admission rate:** admitted preference not supported by held-out user behavior.
- **Over-application rate:** agent applies a preference where user rejects it as irrelevant/stale.
- **Scope leakage:** project-specific preference affects unrelated project/global session.
- **Conflict recovery:** turns/corrections needed after user reverses a preference.
- **Manual edit burden:** edit distance or number of user fixes until acceptance.
- **Checklist catch rate:** violations caught by review checklist before user reply.

### 7.3 A/B comparisons

1. Current refine memory only.
2. Refine + metadata admission, no ranking changes.
3. Refine + metadata admission + ranking metadata terms.
4. Prompt-only taste injection vs review-checklist taste.

### 7.4 Red-first tests for code

- Single ambiguous edit does not create admitted global taste.
- Explicit user preference creates admitted local/global entry depending on requested scope.
- Contradictory later correction marks old entry conflicted or lowers confidence.
- Ranking can find admitted taste by metadata category/target, but digest remains compact.
- No network/async call is introduced in prompt assembly or ranking path, matching performance rules (`AGENTS.md:96-100`).

---

## 8. Risks and guardrails

- **Do not make taste a hard law.** Even Command Code docs say `taste-1` can be wrong and low-confidence learning should be hint/conflict, not enforced (`/tmp/prime-architecture-research/taste-official.txt:151-154`).
- **Do not store only metadata.** Current ranking/digest ignores metadata, so human-readable `content` is still needed (`crates/pa-core/src/refinement/ranking.rs:144-175`, `crates/pa-core/src/refinement/ranking.rs:312-322`).
- **Do not add a new top-level schema/kind in v1.** `HarnessEntry.metadata` is enough and avoids schema break (`crates/pa-core/src/refinement/mod.rs:51-63`).
- **Do not run a remote decision model on prompt path.** Repo performance rules forbid awaiting network on user-visible paths (`AGENTS.md:96-100`).
- **Do not claim science you did not implement.** External scrutiny disputes Command Code’s `taste-1` implementation details (URL: https://cmd.safzan.dev/; URL: https://aiwiki.ai/wiki/command_code). Prime can be stronger by being transparent.

---

## 9. Recommended v1 for Prime

1. Add pure deterministic admission in `pa-core/src/refinement/taste_admission.rs`.
2. Call it in `session_engine/refine.rs` after `strip_display_prefixes` and before `apply_refinement_plan`.
3. Store taste atoms as `kind=memory`, `path=taste/<category>` or `projects/prime-agent/taste/<category>`.
4. Use `metadata.taste` for `evidence`, `polarity`, `confidence`, `status`, `scopeReason`, `contradicts`.
5. Update `ranking.rs` to include selected `metadata.taste` terms in score/IDF with a small admitted-only boost.
6. Use admitted taste mainly as a review checklist. Prompt injection is optional and lower priority.
7. Evaluate on held-out user corrections, not only unit tests.
