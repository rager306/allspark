# МАТРИЦА allspark (из design-matrix.jsonl)

Фишек: **142**. Мета: **88**. Обновлено по анализу `matrix-analysis.md`.

## Источники
| Источник | Фишек |
|---|---|
| ruvector | 53 |
| oh-my-pi | 36 |
| Serena | 26 |
| codebase-memory | 18 |
| pogocache | 9 |

## P0
| ID | Фишка | Описание |
|---|---|---|
| L1 | lsp_client | Клиент LSP: spawn/initialize/didOpen/query по stdio |
| L2 | lsp_tool | Единый action-tool: diagnostics/definition/type_definition/implementation/refere |
| L3 | name_path_addressing | Адресация символов без позиции: relative (suffix-match), absolute (/prefix), [ov |
| L4 | diagnostics | Диагностики файла/символа: pull (textDocument/diagnostic) + publish fallback |
| L6 | refs_def_hover | references/definition/type_definition/implementation/hover/symbols |
| L7 | servers_table | Таблица серверов (57): command/args/fileTypes/rootMarkers/initOptions; auto-dete |
| G1 | sqlite_graph | SQLite-граф кода: nodes(label/name/qn/file/lines/props) + edges(source/target/ty |
| G2 | indexing_tree_sitter | Индексация: обход файлов → tree-sitter → узлы (Function/Struct/...) + рёбра (DEF |
| G3 | graph_search | Поиск символов: BM25 + regex name/qn + semantic_query; in/out degree |
| G4 | graph_trace | callers/callees, data_flow, cross_service; direction in/out/both, depth, risk_la |
| S10 | find_symbol | find_symbol: по name_path + include_body/include_info/kinds/substring/max_matche |
| S11 | find_referencing_symbols | find_referencing_symbols: LSP references + include_imports=False, фильтр kinds |