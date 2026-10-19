
МОИ НАБЛЮДЕНИЯ XERJ (для сверки с learn-xerj):

1. DEFINITION-FIRST RANKING — их killer feature для нас:
   def.rs:190-240 — пер-символьный индекс с градуированными бустами:
   - term name boost 12 (запрос = идентификатор)
   - match_phrase defs boost 8 (файл-level список определений)
   - match_phrase code boost 4 (declaration text)
   - impl^2 для фраз ("flush the tree to disk")
   Измерено: 94% correct-file-at-rank-1 против 38% у body-text search!
   
   У НАС: pa-graph search — BM25+RRF+FTS5, но НЕТ definition-first буста и
   per-symbol индекса. Наша model: trace/search по qualified_name.
   ПРИМЕНИМО: да — это усиление нашего search, не замена. Часы: 8-16.

2. TOKEN BUDGET КАК КОНТРАКТ ТУЛА (token_budget.rs):
   - max_tokens на ВЕСЬ ответ, гарантирован by construction
   - дедуп перекрывающихся file:line ДО бюджетирования
   - over-budget хиты становятся LOCATORS (file:line) а не удаляются
   - accounting block: kept/locator-only/deduped/dropped
   - honest scope: UAX#29 слова != BPE токены (заявлено в описании)
   
   У НАС: tool results обрезаются по TOOL_RESULT_MAX_CHARS (компакция),
   но НЕТ токен-бюджета с локаторами в самом туле.
   ПРИМЕНИМО: да — lsp.references / graph.search с max_tokens. Часы: 12-20.

3. HONESTY SENTENCES В КАЖДОМ ОПИСАНИИ (lib.rs:532):
   "Honesty: links come from deterministic lexical detection plus explicit
   assertions — not neural understanding. XERJ is a search engine with a
   graph-shaped index, not a graph database."
   + rerank: "sends document text off the machine" (приватность)
   + judge: "zero tokens, nothing leaves the node"
   Модель получает честную границу возможностей ПРЯМО в описании.
   
   У НАС: ничего подобного; наш graph description не говорит что он НЕ умеет.
   ПРИМЕНИМО: да — в наши описания. Часы: 2-4.

4. STALENESS REFUSAL: code_search refuses indices older than 30 days
   (stale_ok override). Прямо перекликается с нашим graph coverage
   (index honesty report) — но у нас refusal нет, только отчёт.
   ПРИМЕНИМО: да — наш search/trace на stale-индексе должен предупреждать.
   Часы: 4-8.

5. CITABLE LINE POST-PROCESSING (search.rs:485): defs-хиты без строки
   сдвигаются после lined-хитов того же пути — "agent that cites the first
   header then cites a file with no line" — борьба с нецитируемыми хитами.
   ПРИМЕНИМО: да, мелочь но приятная. Часы: 2-4.

6. FIELD REPORTS КАК ОБЯЗАТЕЛЬНЫЙ РИТУАЛ (AGENTS.md): каждый агент,
   установивший XERJ, ОБЯЗАН открыть PR с field report (25 строк max).
   "A report that says where the tool frustrated you is worth more than
   one that is polite."
   У НАС: qa-tasks RT-000? нет. Есть SWE-подобный suite, но нет ритуала
   field reports от агентов-пользователей.
   ПРИМЕНИМО: идея — добавить в AGENTS.md / после ST-задач. Часы: 4-8.

7. BENCHMARK ЧЕСТНОСТЬ (grep-vs-retrieval-lucene): "harness only, no
   results yet" — публикуют ПУСТОЙ харнесс пока метрики не собраны,
   blind-judge, native arm not handicapped, prior-hostile questions marked.
   
   У НАС: qa-tasks уже близко, но prior-hostile marking нет.
   ПРИМЕНИМО: для ST-задач — помечать задачи, которые модель могла выучить.
   Часы: 2-4.
