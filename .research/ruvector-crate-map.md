## Главные находки для Prime

Ниже каждая находка дана в требуемом формате: механизм / какой класс ошибок или token-cost режет / куда в Prime. Оценки — инженерные, не benchmark; числа из RuVector README помечены `claim`.

## Специальные семейства и caveats

- **RVM не входит в 177 root workspace crates**: root `Cargo.toml` не содержит `crates/rvm` как member; сам `crates/rvm/Cargo.toml:1-2` объявляет отдельный workspace. Его полезный паттерн — capability/shared-memory/witness VM, а не code port (`crates/rvm/README.md:45-53`).
- **RVF**: старый `crates/rvf` исключён root workspace строкой `Cargo.toml:2`, но `rvf-forge-core` и `rvforge-registry` включены как workspace members (`Cargo.toml:141`, `Cargo.toml:144`). Полезный паттерн — canonical artifact + registry trust ladder (`crates/rvf-forge-core/src/lib.rs:1-17`, `crates/rvforge-registry/src/lib.rs:1-24`).
- **mcp-brain-server** одновременно встречается в `exclude` и `members` (`Cargo.toml:2`, `Cargo.toml:146`). Я не запускал cargo, поэтому фиксирую это как manifest fact, не как build verdict.
- **rvAgent** полезен как набор patterns, но не готовый перенос: его `spawn_parallel` пока последовательный stub (`crates/rvAgent/rvagent-subagents/src/orchestrator.rs:96-110`).
- **ruvllm** огромный (187900 Rust LOC local `wc`) и в основном не нужен Prime, если Prime работает через external providers. Брать только patterns: policy memory/session state/witness log (`crates/ruvllm/src/lib.rs:9-23`), не runtime.

## TOP-30 по размеру: сжатая карта

Метрики: Rust LOC — локальный `find`/`wc`; tests — MCP `query_graph` по `is_test=true OR name STARTS WITH 'test_'`; users — internal manifest dependencies from 177 crate set.

| # | crate | LOC | назначение + source | публичный API source | tests | users | verdict |
|---:|---|---:|---|---|---|---|---|

## Остальные 147: групповая карта

Эта часть сохраняет полное покрытие без раздувания. Имена перечислены полностью; смысл групп — какие patterns можно брать в Prime.

| группа | count | имена | Prime relevance / verdict |
|---|---:|---|---|

## Что НЕ стоит переносить как код

1. Полный LLM runtime (`ruvllm`, `ruvllm-wasm`) — Prime уже provider/harness-first; перенос увеличит surface и maintenance (`crates/ruvllm/src/lib.rs:1-23`, `crates/ruvllm-wasm/src/lib.rs:1-12`).
2. Claim-heavy performance modules без verifier: Tiny Dancer 70-85% cost reduction (`crates/ruvector-tiny-dancer-core/README.md:9-25`), SONA <1ms/$0 claims (`crates/sona/README.md:14-24`), nervous-system throughput/bandwidth claims (`crates/ruvector-nervous-system/README.md:77-81`). Treat as hypotheses.
3. Remote shared brain as default — useful pattern, but Prime should be local/private first because memories can contain prompts/session facts (`crates/mcp-brain-server/src/lib.rs:1-5`).

## Suggested Prime experiments

1. **Stale observation gate** in `pa-core` tools: bind `edit`/host operations to file hash/revision; red-first test: stale read cannot edit.
2. **Budgeted recall API** in harness: `retrieve_context(query, token_budget, scope)` returns snippets + score breakdown + receipt id; red-first test: no unbounded memory stuffing.
3. **VoI escalation helper** in kernel/decision API: same pure gate for System1/System2, verifier, search depth, subagent spawn.
4. **Subagent output sanitizer + budgets**: cap reply length/tool calls; strip prompt-injection patterns before parent context.
5. **Pending memory admission**: memories start pending with evidence; accepted memories need validation/witness; rejected dependencies cascade.

## Method notes

- Existing reports under `/tmp/prime-architecture-research/*.md` were read first; this file avoids repeating their high-level RuVector/RuFlo pattern prose and adds crate map + concrete Prime placement.
- MCP schemas were discovered before new calls: `codebase-memory-mcp` advertised 17 tools. Used `get_architecture`, `get_file_outline`, `search_graph`, `trace_path`, and `query_graph`.
- No build/test/cargo command was run. No installs. No repository writes. Source windows were inspected with bounded line windows; large crates such as `ruvllm` were not read wholesale.
