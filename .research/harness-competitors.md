# Harness competitor research: Grok Build, DeepSeek Harness, Command Code

Дата: 2026-10-12. Ограничения: ничего не устанавливал, не запускал, репозиторий Prime не менял, secrets не читал. Источники — официальные docs, официальные GitHub репозитории и публичные product pages. Для Command Code, где исходники CLI закрыты, claims ниже отделены от подтверждённых фактов документации.

## 0. Короткий вывод

1. **“Grok Code” как harness — неверное имя.** Официальный harness называется **Grok Build** (`grok`). Отдельно есть модель **`grok-code-fast-1`** — это модель для agentic coding, не coding-agent runtime.
2. **DeepSeek Harness** — открытый all-plugin harness на Cordis. Сильная идея: не “core + плагины”, а почти всё как плагины: model adapters, tools, session log, agent loop, jobs, subagents, compaction, PTC, terminal, MCP.
3. **Command Code CLI** — закрытый продукт, но docs дают полезные product mechanisms: tool-call repair, token-efficient read/shell tools, background jobs без polling, taste-learning из accept/reject/edit, sessions/checkpoints/rewind, compaction/recovery.
4. Для Prime Agent Rust самый полезный learning не “добавить ещё MCP/subagents/memory” — это уже есть. High impact для coding: **(a) measured tool-output efficiency + delta/monitor semantics**, **(b) repair/recovery layer for model/tool boundary**, **(c) structured coding feedback loop from accept/reject/edit into reviewable project memory/skills**.

---

## 1. Canonical identity, source status, license

| Product | Canonical project/docs | Open/closed | License / commit |
|---|---|---:|---|
| **Grok Build** (`grok`) | Docs: https://docs.x.ai/build/overview ; repo: https://github.com/xai-org/grok-build | Open source Rust CLI/TUI synced from SpaceXAI monorepo | Apache-2.0. GitHub HEAD inspected via API: `2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8` on `main`; repo `SOURCE_REV`: `559751fdcec02d413e4c57c8832ab275e4f44980`. |
| **`grok-code-fast-1`** | xAI news: https://x.ai/news/grok-code-fast-1 | Model, not harness | No harness license. Product/model claims only. |
| **DeepSeek Harness** (`dsh`) | Site: https://www.deepseek.com/en/harness/ ; repo: https://github.com/deepseek-ai/deepseek-harness ; docs: https://deepseek-harness.github.io/deepseek-harness/ | Open source TypeScript/Node + desktop/web architecture | MIT. GitHub HEAD inspected via API: `d743267388641bc76f17c45ce8b4c231aed1d32c` on `master`. Developer preview; docs warn compatibility-breaking changes. |
| **Command Code CLI** (`cmd`, `command-code`) | Site/docs: https://commandcode.ai/ and https://commandcode.ai/docs | Closed source product docs | No source commit. Treat metrics such as “99%+ cache hit”, TEF curves, and “10x/5x” as product claims unless independently reproduced. |

---

## 2. Grok Build / “Grok Code” disambiguation

### Official identity

Official docs call the product **Grok Build**, “a powerful and extensible coding agent” usable through an interactive TUI, headlessly, or via ACP: https://docs.x.ai/build/overview. The open repo README says Grok Build is SpaceXAI’s terminal-based AI coding agent; it edits files, runs shell commands, searches web, manages long-running tasks, and can run headless/CI or ACP.

The **model** page/news for `grok-code-fast-1` says it is a fast/economical reasoning model optimized for agentic coding, with common tools like grep, terminal, file editing, and prompt-cache optimizations. That is model capability/serving, not a harness architecture: https://x.ai/news/grok-code-fast-1.

### Source-backed features

- **TUI/headless/ACP**: docs show `grok -p`, JSON/streaming JSON output, and ACP use; CLI ref includes `grok agent stdio`, sessions, exports, MCP, plugins.
- **Custom models**: config in `~/.grok/config.toml`; `grok inspect` shows discovered config, instructions, skills, plugins, hooks, MCP servers.
- **Rules/context**: Grok loads `AGENTS.md` and compatible names (`CLAUDE.md`, `.grok/rules/*.md`, `.claude/rules/`, `.cursor/rules/`) in directory order. Docs note files are loaded in full and shorter instructions work better.
- **Subagents**: independent child sessions with their own context, summary returned to parent; built-in types include `general-purpose`, `explore` read-only/search-only, `plan` no-shell/no-edit. Docs: https://docs.x.ai/build/features/subagents.
- **Worktrees**: isolated repo copies under `~/.grok/worktrees/...`; subagents can request worktree isolation. Docs: https://docs.x.ai/build/features/worktrees.
- **Sessions/recovery**: every conversation saved under `~/.grok/sessions/` with prompts/responses/tool calls/file snapshots. `/resume`, `--resume`, `--continue`; `/fork`; `/rewind` restores files to rewind points. Docs: https://docs.x.ai/build/features/sessions and repo user guide `17-sessions.md`.
- **Sandbox**: Landlock on Linux, Seatbelt on macOS; profiles `workspace`, `devbox`, `read-only`, `strict`; off by default. Docs: https://docs.x.ai/build/features/sandbox.
- **Hooks/MCP/plugins**: hooks can be shell or HTTP lifecycle handlers, project hooks require trust; MCP tools namespaced `<server>__<tool>` and managed via `grok mcp`.
- **Memory**: repo user guide says cross-session memory is experimental and disabled by default. It has global/workspace scopes, Markdown topics, observations after turns, and `/dream` consolidation.
- **Background tasks**: background terminal commands return IDs, completion wakes parent, output fetched later; can kill tasks/subagents.

### Notes for Prime

Prime already has persistent sessions, subagents, MCP, AGENTS.md, heartbeats, persistent Python REPL/PTC. Grok’s strongest concrete deltas are **worktree-first subagent isolation**, **rewind/file snapshot UX**, and **inspectable discovery** (`grok inspect`) that summarizes rules/skills/plugins/hooks/MCP for the current directory.

---

## 3. DeepSeek Harness

### Official identity

DeepSeek’s site says DeepSeek Harness is public preview/open source and “Everything is a plugin”: https://www.deepseek.com/en/harness/. The GitHub README says `dsh` is an open-source agent harness built on Cordis, with developer preview warnings and breaking changes expected.

The architecture doc is the best canonical source: `docs/architecture.md` says plugins contribute services, typed events, and reversible effects to a shared context. “Every part of the product is a plugin, including the model adapter, the tool registry, the session log, and the agent loop itself.” Profiles (`web`, `headless`, `sdk`, `sdk-minimal`, `acp`) are ordered plugin bundles plus patch layers.

### Source-backed features

- **All-plugin composition**: profiles + bundles + patches. `dsh-base` includes model adapters, tools, persistence, sandbox/approval policy, settings, credentials, telemetry. This is a strong architecture boundary, not just extension points.
- **Session model**: `packages/core/session/README.md` says sessions are append-only event logs deriving model history; compaction hides superseded entries without deleting them; persistence layers subscribe to `session/event`.
- **Agent model**: `packages/core/agent/README.md` exposes live agent handles to create/resume, follow-up/steer, inject context, cancel, and wait idle.
- **Subagents**: `packages/subagent/README.md` supports fresh in-process child, history-seeded child, and out-of-process child backed by ACP, Codex, Claude Code, or another Harness runtime. Tools can message adjacent agents, interrupt, and list child status.
- **PTC runtime**: `packages/ptc-runtime/README.md` says a model writes one program that calls host functions as async calls; runs in fresh Node process under sandbox, plus experimental Python backend. Each run starts stateless; failures return as results.
- **Persistent terminal**: `packages/terminal/README.md` keeps interactive shell/REPL sessions alive across tool calls, including cwd/env/running child processes. Sessions are process-local and do not survive restart.
- **Shell/jobs**: shell can run foreground or background commands; jobs are owner-fenced and completion is delivered to owning agent in-session instead of polled (`packages/jobs/README.md`).
- **Context/compaction**: context plugins add durable model-visible context (instructions, file mentions, session snapshots, time/tmux). Compaction package condenses older history automatically, trims oversized tool outputs first, and replaces unsupported images with placeholders.
- **Loop guards**: `packages/guard/README.md` has repeat-tool reminder and timeout policy; both ship in base bundle.
- **Workflow**: `packages/workflow/README.md` lets an agent run model-authored orchestration scripts that fan out subagents through shared PTC Node runtime.
- **Session query/export**: `packages/session-query/README.md` gives exact logs, filters, relationship traces, full-text SQLite FTS5 search, and session ZIP export.
- **Feedback**: `packages/feedback/README.md` stores session remarks and per-message ratings as product signals; explicitly not model input.
- **Safety**: `SAFETY.md` says it is not audited, not secure/production-ready, can execute model-generated code and load plugins, and sandbox/approvals do not guarantee isolation.

### Notes for Prime

Prime’s Rust crate ownership rules already avoid “god modules”; DeepSeek suggests a more systematic version: make major seams service-like and reversible/configurable. The caution is high: DeepSeek is huge, TS/Node, pre-stable, and pluginizing too much in Prime could add abstraction cost and weaken Rust crate boundaries.

---

## 4. Command Code CLI

### Official identity and closed-source caveat

Command Code positions itself as a coding agent for open models, with tool-call repairs, token-efficient read/shell tools, “99%+ cache hit rates”, taste learning, memory, agents, skills, MCP, hooks, mods, sessions, worktrees, headless, ACP, cloud/background. Sources: https://commandcode.ai/ and https://commandcode.ai/docs.

Because CLI source is not available in the docs, the mechanisms below are **product/documentation claims**, not independently verified implementation facts.

### Source-backed docs/features

- **Memory**: `AGENTS.md` tiers: user `~/.commandcode/AGENTS.md`, project `<project>/AGENTS.md` or `.commandcode/AGENTS.md`, subdirectory-level `AGENTS.md`. Loaded in order with source path headings. Source: https://commandcode.ai/docs/memory.
- **Taste learning**: docs claim every accept/reject/edit is a signal. `taste-1` is described as a classifier, not an LLM; LLMs handle text before/after. Taste is plain Markdown, can be linted/opened/pushed/pulled, has project/global/remote packages. Source: https://commandcode.ai/docs/taste. Claims like “10x faster / 5x fewer bugs” are marketing until measured.
- **Subagents**: custom agents are Markdown files. A subagent has its own context window, tools, prompt, and optional model; several can run concurrently; built-ins include General/Explore/Plan. Source: https://commandcode.ai/docs/agents.
- **Tools**: reference lists file ops, grep/glob, shell/background output, monitors, kill, todo/task ledger, scheduling/wakeups, web, sub-agents, plan/worktree tools. Source: https://commandcode.ai/docs/reference/tools.
- **Background tasks**: docs emphasize background shell, `shell_output`/`task_output`, task ledger, monitors, scheduled wakeups where “the agent never polls”, delta reads (`monitor_events`), background subagents, loops. Source: https://commandcode.ai/docs/background-tasks.
- **Context/compaction/recovery**: docs describe `/context`, auto-compaction, tiers 1–2 tool-call trims, tier 3 summarization, original log never rewritten, automatic recovery/model switches, and “Prompt too long never fails the turn.” Source: https://commandcode.ai/docs/context.
- **Sessions/checkpoints**: sessions persist, resume/branch/tree, checkpoints and rewind, restore verification before changing working tree. Source: https://commandcode.ai/docs/sessions.
- **Worktrees**: managed Git worktrees, `.worktreeinclude` for ignored files, model-driven `enter_worktree`/`exit_worktree`, custom isolation backends. Source: https://commandcode.ai/docs/worktrees.
- **Harness engineering: tool-call repair**: docs claim four common failure modes, validate-then-repair, shape vs relational invariant repair, and around 1M repaired tool calls per 1T tokens. Source: https://commandcode.ai/docs/harness-engineering/tool-call-repairs.
- **Harness engineering: read/shell tools**: docs claim token-efficiency-frontier comparisons. Read tool: 37 capabilities, ~347k/~353k savable tokens per 1M read traffic (~98%). Shell tool: 23 capabilities, ~300k/~306k savable tokens per 1M shell traffic (~98%). These are product-modeled estimates, not independent benchmarks. Sources: https://commandcode.ai/docs/harness-engineering/read-tool and https://commandcode.ai/docs/harness-engineering/shell-tool.

### Notes for Prime

The most useful Command Code learning is not the exact product numbers. It is the engineering posture: treat read/shell tools as measured token products; repair common tool-call errors at the harness boundary; capture coding preference feedback as explicit, reviewable artifacts instead of dumping all “memory” into opaque context.

---

## 5. Cross-product comparison by requested axis

| Axis | Grok Build | DeepSeek Harness | Command Code |
|---|---|---|---|
| Orchestration | Subagents, forks, worktrees, headless/ACP | In-process/out-of-process subagents, workflow scripts, profiles/bundles | Custom agents, background agents, task ledger, loops |
| Terminal/tools | Background terminal, sandbox, kill, output fetch | Shell + jobs + persistent terminal + PTC runtime | Shell/read tools with TEF claims, monitors/delta reads/no polling |
| Context/cache | AGENTS/rules, sessions, experimental memory; model news claims >90% cache hits for `grok-code-fast-1` partners | Context plugins, compaction, session references, append-only events | `/context`, tool trims before summarization, “99%+ cache hit” product claim |
| Learning/memory | Experimental cross-session Markdown topics + `/dream`; rules compatibility | Feedback stored but not model input; pluginized context/memory can be built | Taste learning from accept/reject/edit; Markdown packages, push/pull/lint |
| Eval/recovery | Sessions, rewind, sandbox, file snapshots | Guards, event log replay/query, compaction without deletion, safety docs | Tool repair, prompt-too-long recovery, checkpoint/rewind verification, TEF docs |
| Licensing | Apache-2.0 open Rust | MIT open TS; pre-stable | Closed source product docs |

---

## 6. Recommendations for Prime Agent Rust

### R1 — Build a measured “coding tool efficiency” lane for read/shell/Python outputs

**What to learn:** Command Code’s read/shell docs and DeepSeek/Grok background-task semantics converge on the same point: coding agents spend huge tokens on tool I/O. Prime already has `bash()` handles, persistent Python, PTC, subagents, MCP, compaction. Add a focused measurement and product lane around tool output shape.

**Concrete mechanism:**
- Add golden/tool-output benchmarks for common coding tasks: read file ranges, search hits, test logs, background process deltas, Python REPL outputs.
- Standardize **delta reads** for long-running commands and monitors: completion notices should include concise status; later reads should return only new output unless full output requested.
- Add structured summaries for test/log output: exit code, command, duration, changed since last read, failing test names, bounded tail, artifact path for full logs.
- Keep raw logs on disk and reference paths in model context.

**Cost:** Medium. Mostly pa-core/daemon/tool surface plus tests and fixtures; no new model provider dependency.

**Risk:** Medium. Too much summarization can hide evidence. Mitigate with raw log path, explicit “truncated/summarized” fields, and tests that ensure no false success.

**Metrics:** tokens per successful coding task; tool-output tokens per turn; number of repeated re-reads; time-to-first-useful-failure from test logs; % turns where model asks for same output again.

### R2 — Add a harness-boundary repair/recovery layer with red-first fixtures

**What to learn:** Command Code’s validate-then-repair and DeepSeek’s loop guards target high-frequency harness failures. Prime should repair only narrow, evidence-backed failures at the boundary, not make model behavior opaque.

**Concrete mechanism:**
- Define repair classes for tool calls and structured outputs: missing required field when inferable, wrong enum casing, JSON-with-preamble, wrong path quoting, duplicate/repeated identical tool call.
- Validate first, then repair, then revalidate. Log the original, repair kind, and final value in the session event stream.
- Add loop guard signals: repeated same tool call with same args; repeated failed command without changed premise; prompt-too-long recovery path that trims tool outputs before full compaction.
- Use red-first corpora: captured malformed tool calls from real sessions; expected repaired value or expected refusal.

**Cost:** Medium to high, depending on breadth. Start with 3–4 high-frequency classes only.

**Risk:** High if overbroad; repairs can mask model/provider bugs or create surprising behavior. Keep repair rules explicit, reversible in logs, and gated by tests.

**Metrics:** repair rate per 1T/1M tool calls; % repaired calls that later succeed; regression rate where repair changes intended args; repeated-tool-loop aborts; prompt-too-long turn recovery success.

### R3 — Turn coding feedback into reviewable project skill/memory candidates

**What to learn:** Command Code’s Taste is closed/product-claimed, but the user-facing idea is valuable: accept/reject/edit signals become portable, reviewable preferences. Prime already has continual harness memory/refinement; make it coding-specific and safer.

**Concrete mechanism:**
- Capture lightweight local signals: user accepts patch, reverts edit, edits agent output, asks for same style correction, marks answer useful/not useful.
- Generate **candidate** memories/skills, not automatic prompt changes. Example: “In this repo, prefer `edit` with exact strings over rewriting files” or “Tests use `make check` before merge.”
- Store candidates as Markdown diffs in session artifacts or a review queue; user/agent can promote them to local harness memory or AGENTS.md.
- Keep it project-scoped first; global only on explicit promotion.

**Cost:** Low to medium. Prime’s continual harness already has memories/skills/refinement history; need event capture and review UX.

**Risk:** Medium. Bad feedback inference can pollute context. Mitigate with explicit review, small entries, provenance links to session turns, and expiry/rollback.

**Metrics:** accepted candidate rate; repeated user corrections reduced; memory hit usefulness; context tokens spent on memory vs task success; rollback/delete rate.

### Optional R4 — Worktree-backed subagent isolation as a safety/product upgrade

**What to learn:** Grok and Command Code both present worktrees as core parallel-agent safety. Prime has subagents but should make isolated worktrees a first-class option for implementation/risky edits.

**Cost:** Medium/high in daemon/session lifecycle and cleanup UX.

**Risk:** Medium: gitignored files, large repos, cleanup, merge conflicts. Use explicit opt-in first.

**Metrics:** parallel implementation success rate; conflict rate; user restores/aborts; leaked/untracked worktree cleanup count.

---

## 7. What not to copy blindly

- Do not copy DeepSeek’s “everything is a plugin” wholesale into Prime Rust. Prime’s crate ownership/layering is already a guardrail; over-pluginization can create runtime complexity and weaker compile-time boundaries.
- Do not copy Command Code’s product metrics as requirements. Reproduce local benchmarks first.
- Do not equate `grok-code-fast-1` model behavior with Grok Build harness behavior. Prime should evaluate models through its own harness and compare harness effects separately.
- Do not add another generic memory system. Prime’s durable harness memory exists; the gap is **feedback capture + review + measurement**.

## 8. Source URLs

- Grok Build docs: https://docs.x.ai/build/overview
- Grok Build repo: https://github.com/xai-org/grok-build
- Grok Build subagents/worktrees/sessions/sandbox/hooks/MCP/headless docs: `https://docs.x.ai/build/features/...`
- Grok model news (`grok-code-fast-1`): https://x.ai/news/grok-code-fast-1
- DeepSeek Harness site: https://www.deepseek.com/en/harness/
- DeepSeek Harness repo: https://github.com/deepseek-ai/deepseek-harness
- DeepSeek architecture raw doc: https://raw.githubusercontent.com/deepseek-ai/deepseek-harness/master/docs/architecture.md
- DeepSeek package READMEs: `packages/core/session`, `packages/core/agent`, `packages/subagent`, `packages/ptc-runtime`, `packages/terminal`, `packages/jobs`, `packages/compaction`, `packages/context`, `packages/guard`, `packages/workflow`, `packages/session-query`, `packages/feedback`.
- Command Code docs: https://commandcode.ai/docs
- Command Code memory/taste/agents/tools/background/context/sessions/worktrees docs: `https://commandcode.ai/docs/...`
- Command Code harness engineering docs: https://commandcode.ai/docs/harness-engineering , `/tool-call-repairs`, `/read-tool`, `/shell-tool`.
