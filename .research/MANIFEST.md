# МАНИФЕСТ МАТРИЦЫ allspark

## Формат: JSON-LD-lite (гибрид JSONL + JSON-LD)

**Файлы:**
- `design-matrix.jsonld` — источник истины (231 строка: манифест + 142 фишки + 88 мета)
- `design-matrix.jsonl` — плоский JSONL (обратная совместимость, 230 строк)
- `design-matrix-overview.md` — читаемый обзор
- `matrix-analysis.md` — анализ по 7 осям
- `design-matrix.md` — полные разборы (1571 строка)

## Почему JSON-LD-lite (не полный RDF/SPARQL)

**Решает (найденные анализом проблемы):**
| Проблема (из matrix-analysis.md) | JSON-LD-решение |
|---|---|
| 63 цикла в intersects[] (нетипизированные рёбра) | **типизированные предикаты**: `dependsOn` (DAG, 0 циклов), `sharesApi`, `feeds` |
| 11 коллизий терминов (generation/witness/coherence) | **namespace** `allspark:` + `@id`-URI |
| Нет манифеста (версия/схема/владелец) | **`@context` + `@graph` + manifest-узел** |
| Поля-enum без семантики | **`@context`** мапит поля на `allspark:`-термины + `xsd:`-типы |
| Связь между документами | **`@id` URI** + `sameAs` |

**НЕ берём (оверкилл для 142 записей):**
- Полный RDF/SPARQL (нет oxigraph в кэше; json-ld crate незрелый)
- Внешние `@context` URL (offline-проект)
- Triples-развёртка

**Инструменты (в cargo-кэше):** `oxrdf 0.2.4`, `rio_turtle 0.8.6` — для **экспорта в Turtle** (анализ графа через petgraph/SPARQL), но хранение в JSON-LD-lite.

## Схема

### Манифест (строка 1)
```json
{
  "@context": {...}, "@id": "allspark:matrix/manifest", "@type": "allspark:Manifest",
  "version": "1.0.0", "counts": {"features": 142, "meta": 88},
  "constraints": {"unsafe_code": "forbid", "invasion_points": 5, "flags_default": "off"}
}
```

### Фишка (строки 2..143)
```json
{
  "@id": "allspark:G3", "@type": "allspark:GraphFeature",
  "feature": "graph_search", "desc": "...",
  "source": "allspark:source/codebase-memory", "category": "graph",
  "mechanic": "native", "visibility": "tool", "sync": "sync", "persist": "project",
  "value": 5, "cost": "medium", "priority": "P0", "unsafe": 0, "license": "MIT",
  "dependsOn": ["allspark:G1", "allspark:G2"],
  "sharesApi": ["allspark:V2"],
  "feeds": ["allspark:G4"]
}
```

### Мета (строки 144+)
```json
{"@id": "allspark:meta/host_request_contract/lsp.did_change", "@type": "allspark:Meta/host_request_contract", "kind": "lsp.did_change", ...}
```

## Типизированные рёбра (главное улучшение)

| Предикат | Значение | Кол-во | Свойство |
|---|---|---|---|
| `dependsOn` | X не работает без Y | 75 | **DAG (0 циклов)** |
| `sharesApi` | X и Y делят контракт/схему | 148 | симметрично |
| `feeds` | X поставляет данные Y | 58 | поток |

**Было**: `intersects[]` (281 плоское ребро, 63 цикла, фазовый план невыводим).
**Стало**: `dependsOn` (75, DAG → топосорт → фазы), `sharesApi` (148), `feeds` (58).

## Утилиты

### Загрузка (Python)
```python
import json
rows = [json.loads(l) for l in open('design-matrix.jsonld')]
manifest = rows[0]
feats = [r for r in rows[1:] if 'Feature' in r.get('@type','')]
```

### Топосорт (фазы из dependsOn)
```python
from collections import deque
adj = {r['@id']: r.get('dependsOn',[]) for r in feats}
indeg = {i:0 for i in adj}
for u in adj:
    for v in adj[u]:
        if v in indeg: indeg[v]+=1
q = deque([i for i in adj if indeg[i]==0]); order=[]
while q:
    u=q.popleft(); order.append(u)
    for v in adj[u]:
        if v in indeg:
            indeg[v]-=1
            if indeg[v]==0: q.append(v)
# order[0..] = порядок реализации
```

### Экспорт в Turtle (Rust, oxrdf+rio_turtle)
```rust
// для каждого ребра: (subject, predicate, object) → rio_turtle::NTriplesWriter
// затем: SPARQL/petgraph по RDF
```

## Инварианты (проверяемые)
1. `dependsOn` — **ацикличен** (DAG) → топосорт всегда успешен.
2. Каждый `@id` уникален; все ссылки в рёбрах разрешаются.
3. `unsafe > 0` только у фишек с `notes` про safe-перепись.
4. `license` GPL-3 → `notes` содержит «переписать».

## Миграция из JSONL
`design-matrix.jsonl` → `design-matrix.jsonld`: `intersects[]` классифицируется по слоям
(`perf/ast`=1, `graph`=2, `vectors/edit/symbol`=3-4, `memory/config/agent`=5):
нижний→верхний = `dependsOn`, верхний→нижний = `feeds`, равные = `sharesApi`.
Явные пары (`DiagnosticIdentity`, guards, surface, SQLite) заданы в `sharesApi`/`feeds`.
