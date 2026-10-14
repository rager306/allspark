# RuVector SONA patterns for Prime Agent (source-focused)

Scope: read-only review of `/root/vendor-source/ruvector/crates/sona` plus exact codebase-memory-mcp snippets. This is **pattern research**, not code to port.

## What SONA really updates

SONA updates local adapter/state, not an external API LLM:

- `MicroLoRA` owns local `down_proj`, `up_proj`, `grad_down`, `grad_up`, and `update_count` (`crates/sona/src/lora.rs:23-43`). `accumulate_gradient` adds `signal.gradient_estimate[i] * quality` into `grad_up`; `apply_accumulated` averages by `update_count` and mutates `up_proj` (`lora.rs:192-229`).
- The instant loop records the trajectory, creates `LearningSignal::from_trajectory`, then `try_write()`s MicroLoRA and accumulates. It auto-flushes after `flush_threshold = 100` signals (`instant.rs:36-43`, `103-145`).
- `BaseLoRA` is also local. Background learning computes one vector from extracted patterns and adds it into each local BaseLoRA layer's `up_proj` (`background.rs:180-227`). This is not provider fine-tuning.
- `SonaEngine.apply_micro_lora` only transforms local vectors through the local coordinator's MicroLoRA (`engine.rs:69-77`). There is no evidence that SONA can alter weights of CodeBuddy/Muse/Genspark/OpenAI/Anthropic models behind API calls.

The gradient itself is heuristic. `LearningSignal::estimate_gradient` uses REINFORCE-style `(reward - baseline) * activation`, normalizes it, and falls back to raw `reward * activation` for single-step or equal-reward trajectories (`types.rs:68-128`). That is useful as a router/ranker/control signal, but it is not a true gradient through a remote LLM.

## Learning loop evidence

- **Instant loop:** per trajectory, buffer + MicroLoRA gradient accumulation + auto flush at 100 signals (`loops/instant.rs:103-145`).
- **Background loop:** adds trajectories to ReasoningBank, extracts K-means++ patterns, computes quality-weighted centroid gradients, applies EWC constraints, updates Fisher, then mutates BaseLoRA (`loops/background.rs:110-178`, `180-227`).
- **Pattern clustering / quality feedback:** ReasoningBank builds an embedding from query + reward-weighted activations (`reasoning_bank.rs:87-148`), runs K-means++ (`151-225`), keeps clusters above `min_cluster_size` and `quality_threshold`, and stores `avg_quality`/`cluster_size`.
- **Consolidation:** docs claim three loops including “Loop C (Deep): Weekly dream consolidation and full EWC++ update” (`loops/mod.rs:3-6`). In code, consolidation exists as helpers (`ReasoningBank::consolidate`, `EwcPlusPlus::consolidate_all_tasks`) but I did not find a scheduled Loop C module in `crates/sona/src/loops`; this looks partly conceptual/not wired in the core loop.
- **Darwin guard pattern:** `Guard::screen` rejects non-finite/out-of-bounds/degenerate/judge-veto cases (`darwin_guard.rs:82-102`), `best_accepted` excludes rejected candidates rather than turning them into zero reward (`109-118`), and holdout contamination helpers detect/filter train/eval overlap (`148-201`).

MCP note: codebase-memory-mcp was useful for exact symbol snippets, but some graph traces can cross into duplicate example modules. I treated exact snippets plus file imports/source as authoritative.

## Important implementation caveat: EWC may no-op in the normal background path

Confirmed for the default config:

- Coordinator creates EWC with `param_count = hidden_dim * base_lora_rank * 2` (`coordinator.rs:47-50`). Defaults are `hidden_dim=256`, `embedding_dim=256`, `base_lora_rank=8` (`types.rs:407-413`), so `param_count = 4096`.
- Background `compute_pattern_gradients` returns a vector with `dim = patterns[0].centroid.len()` (`background.rs:180-206`). Pattern centroids come from ReasoningBank embeddings of `embedding_dim`, normally 256.
- `EwcPlusPlus::apply_constraints` and `update_fisher` return/do nothing when `gradients.len() != param_count` (`ewc.rs:110-124`, `215-246`).

So with defaults, background gradients are 256 wide while EWC expects 4096. The EWC step is effectively unchanged/no-op in that normal path. This does not prove EWC is absent everywhere; it does mean the advertised anti-forgetting protection is not actually constraining the default background update unless dimensions are aligned or gradients are expanded to adapter parameter shape.

## Patterns Prime Agent can reuse with CodeBuddy/Muse/Genspark APIs

Prime Agent cannot update remote API model weights. The current fork catalog exposes CodeBuddy, Muse, and Genspark as API providers (`crates/pa-ai/src/models_fork.rs:9-22`); CodeBuddy/Genspark use OpenAI-completions-shaped proxies, Muse pins `openai-responses`; keys come from env vars such as `CODEBUDDY_API_KEY`, `MUSE_API_KEY`, `GENSPARK_API_KEY`/`GSK_API_KEY` (`env_api_keys.rs:6-45`). What is available is **local adaptation around calls**:

1. **Local routing adapter.** Treat MicroLoRA-like state as a fast local transform over task/session embeddings used to choose provider/model/thinking/tool policy, not as an LLM weight update.
2. **Quality-weighted trajectory memory.** Store trajectories with explicit reward sources: user approval, verifier pass/fail, latency/cost, tool success, and parent/child handoff outcome. Cluster successful trajectories and retrieve similar patterns before routing.
3. **Two-speed learning.** Apply immediate small updates to session-local routing preferences; consolidate slowly into durable policy/memory only after enough evidence and holdout checks.
4. **Immutable verifier gate.** Use a Darwin-guard style screen: invalid output is excluded from learning, not assigned zero and kept in the candidate pool. Keep verifier/judge read-only and separate from learner state.
5. **Holdout / contamination discipline.** For eval-driven prompt/router changes, reserve tasks that the learner cannot train on. Reject “lift” if holdout IDs leaked into training.

## Risks and constraints

- **Cold start:** permissive early clustering can crystallize noise. SONA itself lowered defaults to allow early patterns; Prime should avoid durable updates until enough independent wins accumulate.
- **False rewards:** user silence, self-rated quality, or a passing syntactic test can reward the wrong behavior. Prefer external/verifier signals and keep negative/invalid cases excluded from advantage calculations.
- **Adapter dimension mismatch:** the SONA EWC mismatch above is a concrete warning: local learners need one shape/source of truth for embeddings, adapter parameters, and constraints.
- **API limits:** CodeBuddy/Muse/Genspark can be selected, prompted, and called; they cannot expose internal activations or accept backprop/LoRA updates through the API. Any “learning” must be in Prime Agent’s router, memory, prompt selection, tool policy, or local embedding/ranking layers.
- **Stub/partial wiring:** Core SONA has real MicroLoRA/BaseLoRA mutation and ReasoningBank clustering, but the deep weekly Loop C appears documented more than wired in `crates/sona/src/loops`, and EWC is not effective on the default background gradient path.
