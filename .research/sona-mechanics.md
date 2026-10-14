# SONA mechanics → паттерны для Prime

Scope: read-only analysis of `/root/vendor-source/ruvector`, mainly `crates/sona`. Existing `/tmp/prime-architecture-research/*.md` already covered high-level SONA ideas; this file adds exact mechanics, mismatch checks, wiring, and Prime-useful patterns. MCP note: schemas for `codebase-memory-mcp` and `gitnexus` were discovered before MCP lookups; no build, install, benchmark, network upload, or repo mutation was run.

## 0. Что полезно Prime: короткие находки

### 1) Быстрая локальная адаптация должна быть **локальным policy/router adapter**, не «обучением API-модели»
- **Механизм:** `MicroLoRA` хранит локальные `down_proj/up_proj`, `grad_up`, `update_count`; forward добавляет локальный low-rank delta к вектору (`crates/sona/src/lora.rs:23-43`, `crates/sona/src/lora.rs:85-107`, `crates/sona/src/engine.rs:68-77`).
- **Режет:** ошибки повторного выбора модели/инструмента и повторные объясняющие промпты; оценка для Prime: 5-15% меньше route/tool mistakes и 5-10% токенов на повторное планирование, если reward берётся из verifier/user outcome, не из self-rating.
- **Куда в Prime:** `pa-core` router/tool-policy layer + `harness` outcome memory; не пытаться обновлять веса CodeBuddy/Muse/Genspark, а обновлять локальные предпочтения модели, tool budget и retrieval policy.

### 2) Учиться надо от **trajectory + external reward**, но SONA-градиент эвристический
- **Механизм:** `LearningSignal::estimate_gradient` считает baseline `avg(reward)` и `Σ(reward-baseline)*activation`, нормирует L2; при нулевой норме fallback — `Σ reward*activation` (`crates/sona/src/types.rs:68-128`).
- **Режет:** «молчаливое отсутствие обучения» на single-step feedback и equal-reward траекториях; оценка для Prime: 10-30% меньше повторных неудачных тактик в задачах с явным verifier/pass-fail, но 0%/вред при reward из самопохвалы.
- **Куда в Prime:** `pa-core` verifier outcomes, `harness` refinement evidence, tool-call telemetry; хранить source of reward (`test_passed`, `user_accepted`, `review_rejected`, `timeout`, `api_error`).

### 3) Flush-batching — полезный контроль шума и записи, но надо делать explicit flush на завершении задачи
- **Механизм:** `InstantLoopConfig.flush_threshold=100`; каждый trajectory сначала кладётся в buffer, потом `LearningSignal` аккумулируется, а при `pending>=100` вызывается `flush_internal` → `MicroLoRA.apply_accumulated(lr)` (`crates/sona/src/loops/instant.rs:21-32`, `crates/sona/src/loops/instant.rs:102-147`).
- **Режет:** шум от каждого отдельного tool/result и лишнюю запись state; оценка для Prime: 5-20% меньше churn в harness memories/skills, если делать batch promotion и final flush на task close.
- **Куда в Prime:** `harness` refinement queue + `pa-core` session-end/task-end hooks; auto-promotion только пачкой, но user-visible important feedback flush immediately.

### 4) ReasoningBank — это pattern memory для **до-планирования**, а не «рассуждающий мозг»
- **Механизм:** embedding = query + reward-weighted activations, затем деление на `total_reward + 1` и L2-нормировка; negative rewards в embedding игнорируются через `reward.max(0.0)` (`crates/sona/src/reasoning_bank.rs:108-147`).
- **Режет:** повторное чтение тех же файлов, повторный поиск похожих решений, лишние subagent prompts; оценка для Prime: 10-25% токенов на задачах с повторяющимися repo/tools patterns.
- **Куда в Prime:** `pa-core` retrieval-before-plan, `harness` memory ranking, kernel/tool advisor; хранить короткие cited snippets + outcome, не длинные chain-of-thought.

### 5) K-means++ в SONA фактически deterministic farthest-first; для Prime важнее diversity cap, чем математика k-means
- **Механизм:** первый centroid всегда trajectory `0`; следующие выбираются не случайно по D²-probability, а как максимальная нормированная distance; затем обычный nearest-centroid k-means до no-change/convergence (`crates/sona/src/reasoning_bank.rs:227-337`).
- **Режет:** «10 похожих воспоминаний забивают контекст»; оценка для Prime: 10-30% меньше retrieval token waste при diversity-by-cluster top-k.
- **Куда в Prime:** `harness` search/rerank + `pa-core` context packer; кластеризовать/дедуплицировать memories before injection.

### 6) EWC++ как идея полезен, но core SONA EWC фактически no-op в обычном background path
- **Механизм:** core создаёт `param_count = hidden_dim * base_lora_rank * 2`, а background gradient имеет длину `pattern.centroid.len() == embedding_dim`; при несовпадении `apply_constraints`, `update_fisher`, `detect_task_boundary` возвращают no-op/false (`crates/sona/src/loops/coordinator.rs:47-50`, `crates/sona/src/loops/background.rs:180-206`, `crates/sona/src/ewc.rs:110-124`, `crates/sona/src/ewc.rs:146-171`, `crates/sona/src/ewc.rs:215-246`).
- **Режет:** сейчас в SONA — 0%; для Prime pattern — shape-gated protected eval slices могут резать regression/forgetting на 10-25% только если verifier действительно блокирует promotion.
- **Куда в Prime:** `pa-core` evaluation gates + `harness` protected memories/tests; перед любым «learning» вводить invariant: reward-vector shape == protected-state shape.

### 7) Background BaseLoRA в core SONA тоже фактически не влияет на forward по умолчанию
- **Механизм:** `BaseLoRA::new` инициализирует `down_proj` нулями; background update пишет только в `layer.up_proj`, а forward сначала умножает input на нулевой `down_proj`, поэтому delta остаётся нулевой (`crates/sona/src/lora.rs:336-349`, `crates/sona/src/lora.rs:353-380`, `crates/sona/src/loops/background.rs:208-227`).
- **Режет:** для Prime это warning: не добавлять «умные» слои, которые красиво обновляются, но не подключены; оценка потерь при таком bug — 100% background-learning value.
- **Куда в Prime:** `pa-core` tools/kernel feature flags + tests that prove user-visible effect; любой learner должен иметь red-first test: same input before/after verified reward changes a later decision.

### 8) Loop C в `crates/sona` задекларирован, но не как core scheduler/module
- **Механизм:** `loops/mod.rs` перечисляет Loop C, но exports только `background`, `coordinator`, `instant`; consolidation helpers есть в `ReasoningBank`/`EwcPlusPlus`, но core coordinator вызывает только Loop A/B (`crates/sona/src/loops/mod.rs:1-14`, `crates/sona/src/loops/coordinator.rs:79-115`, `crates/sona/src/reasoning_bank.rs:447-480`, `crates/sona/src/ewc.rs:279-312`).
- **Режет:** для Prime — риск «документированного, но не вызываемого» smart behavior; оценка: 0% benefit until wired.
- **Куда в Prime:** `pa-core` scheduler/cron and `harness` compaction/refinement; every smart loop needs explicit caller list and telemetry event.

### 9) StalenessWindow — полезный маленький паттерн для drift, но auto-tuner не встроен в engine
- **Механизм:** `weight(age)=1` до `k1`, `exp(-lambda*(age-k1))` до `k2`, потом `0`; `StalenessWindow` хранит `(value, clock)`, evicts stale, возвращает weighted mean (`crates/sona/src/auto_tuner.rs:24-63`, `crates/sona/src/auto_tuner.rs:65-147`). Example `(1+1)-ES` мутирует `lr`, `initial_lambda`, `fisher_ema_decay`, `boundary_threshold`, принимает candidate если recent score лучше (`crates/sona/examples/darwin_autotuner.rs:55-101`, `crates/sona/examples/darwin_autotuner.rs:185-223`).
- **Режет:** stale routing/tool policy после смены repo/user/workload; оценка для Prime: 5-15% меньше wrong defaults после drift, но только при bounded recent metrics.
- **Куда в Prime:** `pa-core` model/tool router, `harness` memory decay, kernel tool timeout/cost policy.

### 10) Training/export code — mostly useful artifact pipeline, not proof of better model quality
- **Механизм:** `TrainingPipeline` feeds examples into SONA trajectories and `tick/force_learn`; validation just sums expected `example.quality`; exports write JSONL/custom safetensors/scripts and HuggingFace upload shells out to git/curl (`crates/sona/src/training/pipeline.rs:421-459`, `crates/sona/src/training/pipeline.rs:563-621`, `crates/sona/src/export/mod.rs:102-171`, `crates/sona/src/export/huggingface_hub.rs:107-181`, `crates/sona/src/export/huggingface_hub.rs:184-235`).
- **Режет:** Prime can use this pattern for reproducible outcome datasets, not for claiming quality; estimate: 10-20% less audit/debug time if every adaptive decision exports evidence.
- **Куда в Prime:** `pa-core` telemetry/export, `harness` evidence bundles, not UI critical path; no blocking git/curl/network on render/tool path.

---

## 1. LoRA math: exact forward/accumulate/apply

### 1.1 MicroLoRA structure and dimensions

`MicroLoRA` owns `down_proj`, `up_proj`, `rank`, `hidden_dim`, `grad_down`, `grad_up`, `update_count`, and `scale` (`crates/sona/src/lora.rs:23-43`). It asserts rank in `1..=2`, initializes `down_proj` length `hidden_dim * rank` with deterministic small non-zero values, initializes `up_proj` length `rank * hidden_dim` to zeros, and sets `scale = 1/sqrt(rank)` (`crates/sona/src/lora.rs:54-80`). Its `param_count()` is `len(down)+len(up)` (`crates/sona/src/lora.rs:249-252`).

Forward formula for input/output vectors of length `hidden_dim` (`crates/sona/src/lora.rs:85-107`):

```text
intermediate[r] = Σ_i input[i] * down_proj[r * hidden_dim + i]
output[i]      += scale * Σ_r intermediate[r] * up_proj[r * hidden_dim + i]
scale           = 1 / sqrt(rank)
```

The AVX2 path implements the same down projection and residual add; scalar fallback is used otherwise (`crates/sona/src/lora.rs:110-188`). `SonaEngine.apply_micro_lora` just `try_read()`s the coordinator's MicroLoRA and calls this forward, so the effect is local vector transformation only (`crates/sona/src/engine.rs:68-77`).

**Accumulate/apply.** `MicroLoRA.accumulate_gradient` ignores signals whose `gradient_estimate.len() != hidden_dim`; otherwise for each rank row and hidden coordinate it adds `signal.gradient_estimate[i] * signal.quality_score` into `grad_up[r * hidden_dim + i]` and increments `update_count` (`crates/sona/src/lora.rs:192-210`). `grad_down` is not updated by this method (`crates/sona/src/lora.rs:192-210`). `apply_accumulated(lr)` returns if `update_count==0`, sets `scale_lr = lr / update_count`, mutates `up_proj += grad_up * scale_lr`, clears both gradient buffers, and resets `update_count=0` (`crates/sona/src/lora.rs:213-229`).

**Prime pattern.** This is a cheap local residual adapter over router/task embeddings. Use it for local policy deltas; do not claim remote model fine-tuning.

### 1.2 BaseLoRA and LoRALayer structure

`BaseLoRA` owns `layers: Vec<LoRALayer>`, `rank`, `hidden_dim`, and `alpha`; each `LoRALayer` owns `down_proj`, `up_proj`, `layer_idx` (`crates/sona/src/lora.rs:312-331`). `BaseLoRA::new(hidden_dim, rank, num_layers)` creates every layer with `down_proj = vec![0.0; hidden_dim * rank]`, `up_proj = vec![0.0; rank * hidden_dim]`, and `alpha = rank` (`crates/sona/src/lora.rs:334-350`).

Forward formula for a layer (`crates/sona/src/lora.rs:353-380`):

```text
scale           = alpha / rank        # default 1.0 because alpha=rank
intermediate[r] = Σ_i input[i] * layer.down_proj[r * hidden_dim + i]
output[i]      += scale * Σ_r intermediate[r] * layer.up_proj[r * hidden_dim + i]
```

`BaseLoRA.param_count()` returns `num_layers * (hidden_dim*rank + rank*hidden_dim)` (`crates/sona/src/lora.rs:411-414`). `BaseLoRA.merge_into` claims `W' = W + scale * (down @ up)` for `[hidden_dim x hidden_dim]`, but indexes `down_proj[i * rank + r]` while forward indexes `down_proj[r * hidden_dim + i]`; that is a layout mismatch between merge and forward (`crates/sona/src/lora.rs:392-401`, `crates/sona/src/lora.rs:362-379`).

### 1.3 BaseLoRA background update is effectively no-op unless `down_proj` is non-zero elsewhere

`BackgroundLoop.compute_pattern_gradients` makes one vector of length `patterns[0].centroid.len()` by quality-and-size-weighted averaging of pattern centroids (`crates/sona/src/loops/background.rs:180-206`). `update_base_lora` then splits that vector across `num_layers` with `per_layer = gradients.len() / num_layers` and mutates only `layer.up_proj[i] += grad * base_lora_lr` for each segment (`crates/sona/src/loops/background.rs:208-227`). Because `BaseLoRA::new` zeros `down_proj` and no core background method mutates `down_proj`, `forward_layer` multiplies input by zeros before using the updated `up_proj` (`crates/sona/src/lora.rs:336-349`, `crates/sona/src/lora.rs:353-380`, `crates/sona/src/loops/background.rs:208-227`).

For the default core config, gradient length is 256 and `num_layers=12`, so `per_layer=21`; at most 252 `up_proj` values are touched, while total BaseLoRA `up_proj` values are `12 * 8 * 256 = 24_576`, and all `down_proj` values remain zero (`crates/sona/src/types.rs:406-418`, `crates/sona/src/loops/coordinator.rs:53-57`, `crates/sona/src/loops/background.rs:216-224`). The user-visible BaseLoRA delta therefore remains zero in default core background learning.

---

## 2. LearningSignal::estimate_gradient

`LearningSignal::from_trajectory` stores the trajectory's query embedding, the `estimate_gradient` result, final quality, timestamp, trajectory id, step count, and model route (`crates/sona/src/types.rs:38-55`).

Exact formula (`crates/sona/src/types.rs:68-128`):

```text
if steps empty:
    gradient = query_embedding
else:
    baseline = average(step.reward)
    gradient[i] = Σ_steps (step.reward - baseline) * step.activations[i]
    if ||gradient||₂ > 1e-8:
        gradient /= ||gradient||₂
    else:
        fallback[i] = Σ_steps step.reward * step.activations[i]
        if ||fallback||₂ > 1e-8:
            fallback /= ||fallback||₂
        else:
            gradient = zero vector
```

The fallback exists because one-step or equal-reward trajectories make every `reward - baseline` zero; the code comment says the fallback fixes that zero-gradient case at the cost of higher variance (`crates/sona/src/types.rs:101-108`). `scaled_gradient()` is just `gradient_estimate * quality_score` (`crates/sona/src/types.rs:130-136`).

**Prime pattern:** keep the baseline/fallback shape, but label reward provenance. A passing unit test and user acceptance should not share one undifferentiated reward field.

---

## 3. `flush_threshold = 100`: semantics

`InstantLoopConfig::default()` sets `flush_threshold: 100`; converting from `SonaConfig` also hardcodes `flush_threshold: 100` instead of reading it from `SonaConfig` (`crates/sona/src/loops/instant.rs:25-43`). On each completed trajectory, `InstantLoop.on_trajectory` records it to the trajectory buffer, builds a `LearningSignal`, then tries a non-blocking write lock on MicroLoRA (`crates/sona/src/loops/instant.rs:102-126`). If `try_write()` fails, the trajectory remains buffered but no signal is accumulated in that call (`crates/sona/src/loops/instant.rs:103-126`). If the lock succeeds, it accumulates the gradient, increments `pending_signals`, and auto-flushes when `pending >= flush_threshold` (`crates/sona/src/loops/instant.rs:113-125`).

Manual `flush()` also uses `try_write()` and calls `flush_internal` if the lock is available (`crates/sona/src/loops/instant.rs:129-147`). `flush_internal` calls `MicroLoRA.apply_accumulated(micro_lora_lr)`, resets `pending_signals` to 0, and increments flush/update metrics (`crates/sona/src/loops/instant.rs:136-147`). `SonaEngine.flush()` delegates to `LoopCoordinator.flush_instant()` (`crates/sona/src/engine.rs:115-118`, `crates/sona/src/loops/coordinator.rs:113-115`).

**Prime pattern:** use the same two modes: low-noise periodic batching plus explicit flush at user-visible task boundaries.

---

## 4. ReasoningBank mechanics and presets

### 4.1 PatternConfig actual thresholds

`PatternConfig::default()` in core is permissive: `k_clusters=5`, `embedding_dim=256`, `max_iterations=100`, `convergence_threshold=0.001`, `min_cluster_size=1`, `max_trajectories=10000`, `quality_threshold=0.05` (`crates/sona/src/reasoning_bank.rs:28-42`). The comment says this was relaxed from earlier defaults so patterns crystallize with fewer trajectories (`crates/sona/src/reasoning_bank.rs:30-33`).

Important mismatch: `LoopCoordinator::with_config` passes `embedding_dim` and `k_clusters` from `SonaConfig` into `PatternConfig`, but leaves `min_cluster_size` and `quality_threshold` at `PatternConfig::default()` (`crates/sona/src/loops/coordinator.rs:40-45`). So `SonaConfig.quality_threshold` values in `types.rs` do not control core ReasoningBank extraction unless another caller manually changes the bank.

### 4.2 Embedding formula

`ReasoningBank.add_trajectory` computes and stores an embedding plus final quality, then enforces `max_trajectories` by draining oldest entries (`crates/sona/src/reasoning_bank.rs:86-105`). `compute_embedding` does:

```text
embedding[0..query_len] = query_embedding[0..query_len]
for each step:
    weight = max(step.reward, 0)
    total_reward += weight
    embedding[i] += step.activations[i] * weight
if total_reward > 0:
    embedding[i] /= total_reward + 1.0
if ||embedding||₂ > 1e-8:
    embedding /= ||embedding||₂
```

This is implemented at `crates/sona/src/reasoning_bank.rs:108-147`. The `+1` is explicitly for the query contribution (`crates/sona/src/reasoning_bank.rs:132-135`). Negative step rewards do not subtract from the embedding because the weight is `step.reward.max(0.0)` (`crates/sona/src/reasoning_bank.rs:121-128`).

### 4.3 K-means++ details

`extract_patterns()` returns empty if there are no trajectories, sets `k = min(k_clusters, trajectory_count)`, initializes centroids with `kmeans_plus_plus_init`, runs k-means, then filters clusters by `min_cluster_size` and `avg_quality >= quality_threshold` (`crates/sona/src/reasoning_bank.rs:151-224`). Cluster `total_weight` is `Σ member.quality`, `avg_quality = total_weight / cluster_size`, and saved `LearnedPattern` stores centroid, cluster size, total weight, avg quality, timestamps, access count, and `PatternType::General` (`crates/sona/src/reasoning_bank.rs:184-214`).

The initialization is deterministic, not random: first centroid is trajectory `0`; subsequent centroids compute squared distance to nearest existing centroid, normalize distances to probabilities, then choose the **highest** normalized distance (`crates/sona/src/reasoning_bank.rs:227-273`). K-means assigns each trajectory to nearest centroid by squared distance, recomputes unweighted mean centroids, and stops when no assignment changes or `max_shift < convergence_threshold` (`crates/sona/src/reasoning_bank.rs:276-337`). `find_similar` sorts stored patterns by cosine similarity and returns top-k (`crates/sona/src/reasoning_bank.rs:348-359`, `crates/sona/src/types.rs:351-365`).

### 4.4 `SonaConfig` presets in `types.rs`

The core config fields are `hidden_dim`, `embedding_dim`, `micro_lora_rank`, `base_lora_rank`, `micro_lora_lr`, `base_lora_lr`, `ewc_lambda`, `pattern_clusters`, `trajectory_capacity`, `background_interval_ms`, `quality_threshold`, `enable_simd` (`crates/sona/src/types.rs:369-396`). The full core presets are:

| Preset / alias | Values | Source |
| --- | --- | --- |
| Default / Balanced | hidden=256, embedding=256, micro_rank=2, base_rank=8, micro_lr=0.002, base_lr=0.0001, ewc_lambda=2000, clusters=100, capacity=10000, background=3600000ms, quality_threshold=0.15, simd=true | `crates/sona/src/types.rs:398-420`; Balanced maps to default in `TrainingTemplate::from_preset` at `crates/sona/src/training/templates.rs:245-247` |
| Fast / max_throughput / Production | hidden=256, embedding=256, micro_rank=2, base_rank=4, micro_lr=0.0005, base_lr=0.0001, ewc_lambda=2000, clusters=100, capacity=5000, background=7200000ms, quality_threshold=0.4, simd=true | `crates/sona/src/types.rs:423-440`; Production maps to max_throughput at `crates/sona/src/training/templates.rs:249-252` |
| MaxQuality | hidden=256, embedding=256, micro_rank=2, base_rank=16, micro_lr=0.002, base_lr=0.001, ewc_lambda=2000, clusters=100, capacity=20000, background=1800000ms, quality_threshold=0.2, simd=true | `crates/sona/src/types.rs:442-458`; TemplatePreset::MaxQuality uses it at `crates/sona/src/training/templates.rs:254-258` |
| Edge deployment | hidden=256, embedding=256, micro_rank=1, base_rank=4, micro_lr=0.001, base_lr=0.0001, ewc_lambda=1000, clusters=50, capacity=200, background=3600000ms, quality_threshold=0.5, simd=true | `crates/sona/src/types.rs:460-475` |
| Batch processing | hidden=256, embedding=256, micro_rank=2, base_rank=8, micro_lr=0.001, base_lr=0.0001, ewc_lambda=2000, clusters=100, capacity=10000, background=3600000ms, quality_threshold=0.3, simd=true | `crates/sona/src/types.rs:478-493` |
| Ephemeral federated agent | hidden=256, embedding=256, micro_rank=2, base_rank=4, micro_lr=0.002, base_lr=0.0001, ewc_lambda=1000, clusters=50, capacity=500, background=60000ms, quality_threshold=0.3, simd=true | `crates/sona/src/types.rs:496-514` |
| Federated coordinator | hidden=256, embedding=256, micro_rank=2, base_rank=16, micro_lr=0.001, base_lr=0.0005, ewc_lambda=2000, clusters=200, capacity=50000, background=300000ms, quality_threshold=0.4, simd=true | `crates/sona/src/types.rs:517-535` |

The comments claiming rank-2 speed, +55.3% quality, 2.3x search speed, and EWC lambda optimality are **claims**, not verified by this read-only pass (`crates/sona/src/types.rs:400-414`).

---

## 5. EWC++ exact behavior and mismatch check

### 5.1 EWC++ formulas

`EwcConfig::default()` sets `param_count=1000`, `max_tasks=10`, `initial_lambda=2000`, `min_lambda=100`, `max_lambda=15000`, `fisher_ema_decay=0.999`, `boundary_threshold=2.0`, `gradient_history_size=100` (`crates/sona/src/ewc.rs:12-48`). `LoopCoordinator` overrides only `param_count = hidden_dim * base_lora_rank * 2` and `initial_lambda = SonaConfig.ewc_lambda` (`crates/sona/src/loops/coordinator.rs:47-50`).

`update_fisher(gradients)` returns immediately if `gradients.len() != param_count`; otherwise it applies online Fisher `F_t = decay * F_{t-1} + (1 - decay) * g^2`, updates gradient stats with Welford-style running mean/variance, and increments `samples_seen` (`crates/sona/src/ewc.rs:109-144`). In core background, those gradients come from `compute_pattern_gradients`, then `apply_constraints`, then `update_fisher(&constrained_gradients)` (`crates/sona/src/loops/background.rs:139-166`).

`detect_task_boundary(gradients)` returns false until `samples_seen >= 50` and length matches `param_count`; when active it computes average absolute z-score against running mean/variance and returns `avg_z > boundary_threshold` (`crates/sona/src/ewc.rs:146-171`). `start_new_task()` stores current Fisher/current weights as a `TaskFisher`, keeps a circular buffer of max tasks, resets current Fisher/stats/samples, increments task id, then calls `adapt_lambda()` (`crates/sona/src/ewc.rs:174-200`). `adapt_lambda()` sets `lambda = initial_lambda * (1 + 0.1 * task_count)`, clamped to `[min_lambda, max_lambda]` (`crates/sona/src/ewc.rs:202-213`).

`apply_constraints(gradients)` returns unchanged if length mismatches; otherwise each remembered task reduces each gradient by `1/(1 + lambda * fisher_i * importance)`, and current Fisher applies a 0.1 weight (`crates/sona/src/ewc.rs:215-246`). `consolidate_all_tasks()` weighted-averages remembered Fisher diagonals by task importance, replaces all task memory with one consolidated task, and uses current weights as the consolidated optimal weights (`crates/sona/src/ewc.rs:279-312`). `set_optimal_weights` exists but is not called by core background code; it is only in tests/examples in the core crate (`crates/sona/src/ewc.rs:272-276`, `crates/sona/src/ewc.rs:443-447`, `crates/sona/examples/darwin_autotuner.rs:129`).

### 5.2 Mismatch numbers for all core presets

Background gradient length is the first pattern centroid length (`crates/sona/src/loops/background.rs:180-186`), and centroid length comes from `PatternConfig.embedding_dim` (`crates/sona/src/reasoning_bank.rs:108-147`, `crates/sona/src/loops/coordinator.rs:40-45`). In every core preset, `embedding_dim=256` (`crates/sona/src/types.rs:406-535`). EWC `param_count` is `hidden_dim * base_lora_rank * 2` (`crates/sona/src/loops/coordinator.rs:47-50`). Therefore:

| Preset | base rank | EWC param_count | background gradient len | Result |
| --- | ---: | ---: | ---: | --- |
| Default / Balanced | 8 | `256*8*2 = 4096` | 256 | `apply_constraints` unchanged, `detect_task_boundary=false`, `update_fisher` returns (`crates/sona/src/types.rs:406-418`, `crates/sona/src/ewc.rs:110-124`, `crates/sona/src/ewc.rs:146-171`, `crates/sona/src/ewc.rs:215-246`) |
| Fast / max_throughput | 4 | `256*4*2 = 2048` | 256 | same no-op (`crates/sona/src/types.rs:425-438`) |
| MaxQuality | 16 | `256*16*2 = 8192` | 256 | same no-op (`crates/sona/src/types.rs:443-456`) |
| Edge deployment | 4 | 2048 | 256 | same no-op (`crates/sona/src/types.rs:461-474`) |
| Batch processing | 8 | 4096 | 256 | same no-op (`crates/sona/src/types.rs:479-492`) |
| Ephemeral | 4 | 2048 | 256 | same no-op (`crates/sona/src/types.rs:500-513`) |
| Federated coordinator | 16 | 8192 | 256 | same no-op (`crates/sona/src/types.rs:521-534`) |

So the specific mismatch `param_count=hidden*rank*2` vs `gradient=embedding_dim` is confirmed for ranks 4/8/16. In the normal core background path, Fisher does not update, task boundaries do not trigger, and EWC constraints do not change gradients.

---

## 6. `auto_tuner.rs`: what it tunes and what is only example code

The reusable module only defines `StalenessSchedule` and `StalenessWindow`; it does not mutate `SonaConfig` by itself (`crates/sona/src/auto_tuner.rs:24-147`). The schedule formula is documented and implemented as:

```text
w(age) = 1                       if age <= k1
       = exp(-lambda*(age-k1))   if k1 < age <= k2
       = 0                       if age > k2
```

The default is `k1=16`, `k2=64`, `lambda=0.08` (`crates/sona/src/auto_tuner.rs:35-63`). `StalenessWindow.push(value)` stores `(value, clock)`, increments the clock, evicts over-capacity samples and samples with `age > k2`; `weighted_mean()` returns `Σ w*v / Σ w` or `None` if empty/all-stale (`crates/sona/src/auto_tuner.rs:77-147`).

The example `darwin_autotuner.rs` tunes a separate `Genome` with `lr`, `initial_lambda`, `fisher_ema_decay`, and `boundary_threshold`, not the whole `SonaConfig` (`crates/sona/examples/darwin_autotuner.rs:55-101`). It first finds a best static config for regime A, then uses a staleness window and `(1+1)-ES` probes every `RETUNE_EVERY=2` epochs; a candidate is accepted when its recent-regime probe beats incumbent recent weighted mean (`crates/sona/examples/darwin_autotuner.rs:26-29`, `crates/sona/examples/darwin_autotuner.rs:150-169`, `crates/sona/examples/darwin_autotuner.rs:185-223`). Its printed “online tuner is X% better” is a synthetic example output, not a verified production benchmark in this pass (`crates/sona/examples/darwin_autotuner.rs:227-240`).

---

## 7. Coordinator, `tick()`, `force_learn()`, `flush()`, and Loop C

`LoopCoordinator::with_config` wires ReasoningBank, EWC, 12-layer BaseLoRA, InstantLoop, and BackgroundLoop; instant/background are enabled by default (`crates/sona/src/loops/coordinator.rs:39-77`). `on_inference` only calls InstantLoop when enabled (`crates/sona/src/loops/coordinator.rs:79-83`). `maybe_run_background` checks `background.should_run()`, drains instant trajectories, and runs background with `force=false` only if there are trajectories (`crates/sona/src/loops/coordinator.rs:91-105`). `force_background` drains trajectories and runs background with `force=true`, bypassing the minimum trajectory check (`crates/sona/src/loops/coordinator.rs:107-110`, `crates/sona/src/loops/background.rs:107-116`). `flush_instant` delegates to InstantLoop flush (`crates/sona/src/loops/coordinator.rs:113-115`).

`SonaEngine.tick()` delegates to `maybe_run_background()` and returns a status string only if a background cycle ran (`crates/sona/src/engine.rs:90-104`). `SonaEngine.force_learn()` delegates to `force_background()` and returns a string summary (`crates/sona/src/engine.rs:106-113`). `SonaEngine.flush()` delegates to `flush_instant()` (`crates/sona/src/engine.rs:115-118`). NAPI exposes `tick`, `force_learn`, and `flush` as direct wrappers (`crates/sona/src/napi.rs:102-120`). WASM exposes `runInstantCycle`, `tick`, and `forceLearn`; `learn_from_feedback` also calls `engine.flush()` immediately after submitting a feedback trajectory (`crates/sona/src/wasm.rs:209-235`, `crates/sona/src/wasm.rs:277-320`).

Loop C status: `loops/mod.rs` documents three loops including “Loop C (Deep)”, but the module exports only `background`, `coordinator`, and `instant` (`crates/sona/src/loops/mod.rs:1-14`). The core coordinator only calls instant/background methods (`crates/sona/src/loops/coordinator.rs:79-115`). Pattern consolidation and EWC consolidation exist as helpers, but no core `deep` module or scheduler calls them (`crates/sona/src/reasoning_bank.rs:447-480`, `crates/sona/src/ewc.rs:279-312`). Some other crates add their own consolidation calls, for example rvAgent middleware calls `reasoning_bank.consolidate(0.95)`, `ewc.consolidate_all_tasks()`, `engine.flush()`, and `engine.force_learn()` during session consolidation (`crates/rvAgent/rvagent-middleware/src/sona.rs:458-477`).

---

## 8. Training and export: working code vs scaffold

### 8.1 Training pipeline

`TrainingPipeline::train()` preprocesses examples, runs epochs, optionally validates, optionally calls `engine.force_learn()` for pattern extraction, and returns metrics (`crates/sona/src/training/pipeline.rs:421-459`). `train_batch_range()` converts each `TrainingExample` into a SONA trajectory with embedding, route/context, activations, attention, reward, and quality, then calls `engine.end_trajectory`; after the batch it calls `engine.tick()` (`crates/sona/src/training/pipeline.rs:563-601`). Validation applies MicroLoRA but then simply accumulates the expected `example.quality`; the comment says a real scenario would evaluate model output (`crates/sona/src/training/pipeline.rs:604-621`).

Verdict: this is working SONA-state feeding code, not full model training. For Prime, it is closer to a replay/evaluation harness than a trainer.

### 8.2 Federated training

`EphemeralAgent` owns a local `SonaEngine`, collects `TrajectoryExport`s, records trajectories into the engine, can `force_learn`, and exports state after forcing local learning (`crates/sona/src/training/federated.rs:73-98`, `crates/sona/src/training/federated.rs:132-164`, `crates/sona/src/training/federated.rs:185-187`, `crates/sona/src/training/federated.rs:241-259`). `FederatedCoordinator` owns a master `SonaEngine`, rejects trajectories below `quality_threshold=0.4`, replays accepted trajectories into the master engine, records contributions, and auto-consolidates every `consolidation_interval=50` agent contributions via `force_learn()` (`crates/sona/src/training/federated.rs:276-308`, `crates/sona/src/training/federated.rs:348-412`).

Verdict: the aggregation/replay path is real in-memory code. Its learning impact inherits the core MicroLoRA/ReasoningBank behavior and the BaseLoRA/EWC no-op caveats above.

### 8.3 Export, HuggingFace, SafeTensors, pretrain package

`HuggingFaceExporter` can export LoRA safetensors, patterns JSONL, preference pairs JSONL, push to Hub, or export all with adapter config and README (`crates/sona/src/export/mod.rs:80-171`). `SafeTensorsExporter` writes `adapter_model.safetensors` via a custom serializer: it exports MicroLoRA A/B and clones each BaseLoRA layer weights into q/k/v/o projection keys (`crates/sona/src/export/safetensors.rs:25-68`, `crates/sona/src/export/safetensors.rs:70-183`, `crates/sona/src/export/safetensors.rs:185-260`). `engine.export_lora_state()` exports MicroLoRA weights and all BaseLoRA layer weights; BaseLoRA down weights are whatever the engine has, which are zero by default (`crates/sona/src/engine.rs:162-196`, `crates/sona/src/lora.rs:336-349`).

Dataset export writes pattern records filtered by `min_quality_threshold`, builds preference pairs by sorting quality trajectories, and builds distillation targets from routing decisions (`crates/sona/src/export/dataset.rs:26-79`, `crates/sona/src/export/dataset.rs:82-155`, `crates/sona/src/export/dataset.rs:157-205`). But `get_quality_trajectories()` uses patterns as proxy query/response embeddings and `get_routing_decisions()` uses one-logit `[avg_quality]`; both are simplified exports from ReasoningBank patterns, not full original LLM examples (`crates/sona/src/engine.rs:199-239`).

`HuggingFaceHub` uses local temp dirs, git clone/add/commit/push, and curl for repo creation (`crates/sona/src/export/huggingface_hub.rs:41-103`, `crates/sona/src/export/huggingface_hub.rs:107-181`, `crates/sona/src/export/huggingface_hub.rs:184-235`). `PretrainPipeline.export_package()` writes export artifacts plus `train.py`, `pretrain_config.json`, `requirements.txt`, and accelerate config; its generated script includes benchmark/quality numbers as **claims** (`crates/sona/src/export/pretrain.rs:269-315`, `crates/sona/src/export/pretrain.rs:318-330`).

Verdict: export is working file-generation/upload code, but the resulting artifacts can encode default-zero BaseLoRA down weights and simplified pattern-derived datasets. Prime should use the export pattern for audit evidence, not as proof of model quality.

---

## 9. SONA copies / wrappers and version skew

Concise inventory of code-bearing SONA copies/wrappers in the RuVector checkout:

| Path | What it is | Key skew / difference |
| --- | --- | --- |
| `crates/sona` | Main Rust crate `ruvector-sona` version `0.2.1`, with optional `wasm`, `napi`, and `serde-support` features (`crates/sona/Cargo.toml:1-33`). | Current core; permissive ReasoningBank defaults `k=5`, `min_cluster_size=1`, `quality_threshold=0.05` (`crates/sona/src/reasoning_bank.rs:28-42`). |
| `examples/ruvLLM/src/sona` | Older source copy of core-like engine/loops/lora/ewc/reasoning_bank. | Default `SonaConfig.quality_threshold=0.3`, not core `0.15`; PatternConfig old `k=100`, `min_cluster_size=5`, `quality_threshold=0.3`, not core `5/1/0.05` (`examples/ruvLLM/src/sona/types.rs:360-380`, `examples/ruvLLM/src/sona/reasoning_bank.rs:28-40`). |
| `examples/edge-net/src/ai/sona` | Edge/WASM-oriented source copy plus a simpler `SonaLearner`. | Default hidden/embedding 128, base rank 4, capacity 500, p2p sharing flag true; also a separate simplified `SonaLearner` with `DeepConsolidator` fields but only `record_trajectory`/count in this module (`examples/edge-net/src/ai/sona/types.rs:372-393`, `examples/edge-net/src/ai/sona/mod.rs:12-23`, `examples/edge-net/src/ai/sona/mod.rs:131-167`). |
| `crates/sona/src/wasm.rs` and `examples/edge-full/pkg/sona` | WASM wrapper source plus generated JS/DTS/wasm artifact. | WASM adds JS-facing trajectory handles, `learn_from_feedback`, immediate flush, federated agent/coordinator wrappers (`crates/sona/src/wasm.rs:43-56`, `crates/sona/src/wasm.rs:209-235`, `crates/sona/src/wasm.rs:430-572`, `crates/sona/src/wasm.rs:574-710`); generated package exposes `WasmSonaEngine` (`examples/edge-full/pkg/sona/ruvector_sona.js:707`, `examples/edge-full/pkg/sona/ruvector_sona.d.ts:211`). |
| `crates/sona/src/napi.rs`, `crates/sona/src/napi_simple.rs`, `npm/packages/sona` | NAPI source plus npm wrapper package. | npm package is version `0.1.8`, optional native packages pinned `0.1.5`, while Rust crate is `0.2.1`; d.ts/default docs still say micro rank 1, ewc 1000, clusters 50, threshold 0.5 (`npm/packages/sona/package.json:1-4`, `npm/packages/sona/package.json:74-80`, `npm/packages/sona/index.d.ts:6-31`, `crates/sona/Cargo.toml:1-3`). |
| `crates/ruvllm/src/sona` | Runtime/pretraining integration around `ruvector_sona`, not the same full copy. | Its default integration config uses embedding_dim 768, `ewc_lambda=0.1`, and has explicit `LearningLoop::Deep`; it maps into core `SonaConfig` and creates a core `SonaEngine` (`crates/ruvllm/src/sona/integration.rs:76-103`, `crates/ruvllm/src/sona/integration.rs:155-184`). |
| `crates/ruvector-dag/src/sona` | DAG-specific SONA implementation. | Uses `DagSonaEngine`, `DagReasoningBank`, DAG embeddings, and pre/post-query APIs; not the core `SonaEngine` (`crates/ruvector-dag/src/sona/mod.rs:1-13`, `crates/ruvector-dag/src/sona/engine.rs:12-80`). |
| `crates/ruvector-postgres/src/sona` | PostgreSQL wrapper over core `ruvector_sona`. | Keeps one core `SonaEngine` per table/dimension and exposes learn/apply/stats operators (`crates/ruvector-postgres/src/sona/mod.rs:1-18`, `crates/ruvector-postgres/src/sona/mod.rs:45-73`, `crates/ruvector-postgres/src/sona/operators.rs:34-115`, `crates/ruvector-postgres/src/sona/operators.rs:123-155`). |
| `crates/rvf/rvf-adapters/sona` | RVF storage adapter for trajectories, replay, patterns. | It is storage/schema code, not LoRA/EWC engine math (`crates/rvf/rvf-adapters/sona/src/lib.rs:1-18`, `crates/rvf/rvf-adapters/sona/src/config.rs:5-26`). |
| `.claude/agents/sona`, `examples/ruvLLM/docs/SONA` | Agent/doc material. | Docs/specs only; not authoritative for core runtime wiring (`crates/sona/src/loops/mod.rs:1-14` is a concrete example where docs mention Loop C but core modules do not). |

Prime lesson: do not port by copying a SONA directory. Pick small patterns with tests: trajectory outcome schema, diversity retrieval, staleness-weighted tuning, and shape-gated promotion.

---

## 10. Real callers of `SonaEngine` outside `crates/sona`

MCP `codebase-memory-mcp.search_code(pattern="SonaEngine::")` and local line search both show real non-test callers. Main ones:

- `crates/rvAgent/rvagent-middleware/src/sona.rs` constructs a feature-gated core `SonaEngine::with_config`, separately creates EWC/ReasoningBank, calls `engine.tick()` in background learning, and calls `engine.flush()` + `engine.force_learn()` during session consolidation (`crates/rvAgent/rvagent-middleware/src/sona.rs:233-245`, `crates/rvAgent/rvagent-middleware/src/sona.rs:255-273`, `crates/rvAgent/rvagent-middleware/src/sona.rs:409`, `crates/rvAgent/rvagent-middleware/src/sona.rs:458-477`).
- `crates/ruvllm/src/sona/integration.rs` creates an `Arc<RwLock<SonaEngine>>` from its own integration config and separately creates EWC/ReasoningBank (`crates/ruvllm/src/sona/integration.rs:126-150`, `crates/ruvllm/src/sona/integration.rs:155-190`).
- `crates/ruvllm/src/sona/ruvltra_pretrain.rs` creates a core `SonaEngine::with_config` and later returns it via `into_engine` (`crates/ruvllm/src/sona/ruvltra_pretrain.rs:380-404`, `crates/ruvllm/src/sona/ruvltra_pretrain.rs:737-739`).
- `crates/ruvector-postgres/src/sona` creates one core `SonaEngine` per table/dimension, learns by `begin_trajectory`/`end_trajectory`, applies `apply_micro_lora`, and exposes stats (`crates/ruvector-postgres/src/sona/mod.rs:45-73`, `crates/ruvector-postgres/src/sona/operators.rs:96-115`, `crates/ruvector-postgres/src/sona/operators.rs:123-155`, `crates/ruvector-postgres/src/sona/operators.rs:167-220`).
- `crates/mcp-brain-server/src/routes.rs` initializes shared `sona::SonaEngine::new(128)` in server state; routes later call `force_learn` according to line search (`crates/mcp-brain-server/src/routes.rs:78-92`, `crates/mcp-brain-server/src/routes.rs:461`, `crates/mcp-brain-server/src/routes.rs:539`).
- `crates/prime-radiant/src/sona_tuning/tuner.rs` constructs a core `SonaEngine`, applies MicroLoRA in threshold adjustment, and separately consolidates its own EWC (`crates/prime-radiant/src/sona_tuning/tuner.rs:151-175`, `crates/prime-radiant/src/sona_tuning/tuner.rs:345-348`, `crates/prime-radiant/src/sona_tuning/tuner.rs:358-370`).
- `examples/ruvLLM/src/bin/export.rs` creates a demo engine and generates demo trajectories before export (`examples/ruvLLM/src/bin/export.rs:81-110`).
- `.claude/intelligence/storage.js` optionally initializes native `@ruvector/sona` `SonaEngine(256)` and calls `forceLearn()` via a JS wrapper (`.claude/intelligence/storage.js:150-178`, `.claude/intelligence/storage.js:318-320`, `.claude/intelligence/storage.js:464`).

Graph caveat: exact `trace_path` for `crates.sona.src.engine.SonaEngine.with_config` mainly returned in-crate/test callers because some cross-crate calls resolve through imports/wrappers; text search plus file windows above are the safer caller evidence.

---

## 11. Prime implementation recommendations

1. Start with **OutcomeRecord** in `pa-core`: task embedding/hash, model/tool route, verifier result, user outcome, cost/tokens/latency, source citations. SONA's trajectory structs show the minimal fields (`crates/sona/src/types.rs:139-211`).
2. Add **retrieval-before-plan**: top-k successful and failed patterns, diversity-clustered, injected as untrusted evidence. SONA's `find_similar` is simple but enough as a shape (`crates/sona/src/reasoning_bank.rs:348-359`).
3. Add **batch promotion** to harness: pending signals collect, flush on task completion or threshold. SONA's `flush_threshold=100` is a usable default shape, not necessarily the right Prime constant (`crates/sona/src/loops/instant.rs:25-43`).
4. Add **shape/effect tests** before any learner: verify a rewarded outcome changes a later route/tool decision, and verify protected cases do not regress. SONA's EWC/BaseLoRA no-ops are the negative example (`crates/sona/src/loops/coordinator.rs:47-50`, `crates/sona/src/loops/background.rs:208-227`, `crates/sona/src/lora.rs:336-380`).
5. Add **staleness-weighted policy scores** for model/tool routing; do not average six-month old outcomes with this week's workload. SONA's `StalenessWindow` is the cleanest reusable component (`crates/sona/src/auto_tuner.rs:24-147`).
6. Keep export/evidence **fire-and-forget/off critical path**. SONA's Hub upload shells out to git/curl and should not be copied into a Prime UI path (`crates/sona/src/export/huggingface_hub.rs:107-181`, `crates/sona/src/export/huggingface_hub.rs:184-235`).

## Assumptions and changed constants

- No constants were changed; this was read-only research.
- All performance/quality numbers from comments or generated scripts are marked as claim or treated as unverified because no benchmark was run (`crates/sona/src/types.rs:400-414`, `crates/sona/src/export/pretrain.rs:325-328`).
- Estimates for Prime token/error reduction are planning estimates for prioritization; they need Prime-side A/B or verifier-backed measurement before product claims.
