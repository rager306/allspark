# ruvllm audit для Prime Agent

Scope: read-only анализ `/root/vendor-source/ruvector/crates/ruvllm`, связанных `crates/ruvllm_sparse_attention`, `crates/ruvllm-wasm`, `examples/ruvLLM`. Не запускал сборку, тесты, install или модели. Существующие отчёты в `/tmp/prime-architecture-research/*.md` просмотрены; ниже добавлены только факты по ruvllm и паттерны для Prime.

Легенда вердиктов: **real** = есть исполняемая логика/состояние/алгоритм; **partial** = API и часть алгоритма есть, но нет полного рабочего контура или внешние зависимости/заглушки; **stub/claim** = декларация/пример/маркетинг без подтверждённого кода.