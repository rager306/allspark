# QA-экспансия: анализ логов и вывода (уровень 2.5 + мост к 3)

Агент умер на финише; код доведён и валидирован родителем. Прогон: RT-001/003/008, --judge live (SubQ Snap), 3/3 pass, вердикты achieved 0.78-0.91 с вероятностями.

## Что вошло

**A. Лог-аналитика (scripts/qa/qa_logs.py, 635 строк)**
- Три источника на триал: session.jsonl (транскрипт), logs/agent.jsonl (request timing, PI_REQUEST_TIMING), logs/debug-*.log (ротация debugLog — ALLSPARK_DEBUG).
- Нормализация транскрипта в таймлайн-события; лог-инварианты в task.yaml (`logs:`): max_requests, max_consecutive_error_turns, max_consecutive_failed_requests, max_warn_lines. Инвариант «breaker не открывался» — через debug-логи admission (готов к включению).
- Схема-стабильность: манифест хранит schema-подпись транскрипта (дельта между прогонами = сигнал).

**B. Анализ вывода (qa_judge.py, 400 строк)**
- Output-оракул в task.yaml (`output:`): {contains, not_contains, regex(min/max), equals_any, first_line_regex, min/max_chars, all_of/any_of} по target=answer|outcome|both. Баг min/max-инверсии (max:0 читался как min:1) пойман первым прогоном и починен.
- Живой судья SubQ Snap (--judge live|scripted|auto): choice achieved|partial|not_achieved по рубрике задачи; в манифесте: verdict+probabilities, rubric_sha256, judge bundle {bundle, prompt_rev, key_fingerprint} — трассируемость уровня 4 без света ключа.
- A/B-дельта: compare_judge_deltas против --baseline (сколько задач judge оценил хуже/лучше) — прото уровня 3.

## Прогон-валидация
- 3/3 pass; RT-008 новый output-инвариант сначала поймал инверсию семантики (починено), затем зелёный; судья live 3/3 achieved.

## Осталось (уровень 3-4 полный)
- Парный A/B на ДВУХ сборках (нужна вторая сборка в baseline-манифесте) + знаковый тест; якорный corpus судьи (label-flip при смене prompt_rev); trace-capture нормализатор live/offline единым кодом (omp C1) в Rust.
