# SubQ Snap (Subquadratic) — исследование для pa-jev

Дата: 2026-10-17. Источники: субстраницы subq.ai (анонс), docs.subq.ai (Quickstart/Overview/Authentication/Models/Data retention/Decisions API/Models API/Chat Completions/Responses), LinkedIn-анонс компании, **живые вызовы API** (проверено сегодня, ключ со скретчем — плейсхолдеры в примерах). Побайтовое сравнение с `crates/pa-jev/src/{wire,types,client,error}.rs` и `crates/pa-core/src/jev/mod.rs`.

## 1. Что это

**SubQ Snap** — «decision model» от компании **Subquadratic** (исследовательская AI-компания, архитектура SSA — Subquadratic Sparse Attention; линейная, а не квадратичная сложность внимания по контексту). Анонс 2026-10-12, Public Preview.

- Класс интерфейса — **ровно тот же, что у TypeSafe Jev (System One), Laya, Perplexity Decider**: контекст + вопрос + конечное множество допустимых ответов → вероятности вместо свободной генерации. Сам анонс прямо позиционирует Snap против Jev («ranks four points above Jev on the Decision Index v0.2.1»).
- Назначение: быстрые структурированные решения в агентных workflow — routing, tool picking, проверка результатов, triage, guardrails.
- Параметры: до **384K токенов** контекста (текст + изображения), ~1 c на 100K токенов префилла, мультимодальный вход (PNG/JPEG/WebP).
- Бенчмарки из анонса: Decision Index 0.2.1 — 61.7 (Jev 57.9); Nimble 13 — 82.9% (Jev 76.0%); Decision RULER до 1M токенов — 96.3–99.8.
- Линейка моделей: `subq-snap-preview` (decisions, public preview) и `subq-preview` (long-context LLM для Chat/Responses, **early access** — дизайнерским партнёрам).

## 2. API

Базовый URL: **`https://api.subq.ai/v1`**. Эндпоинты:

| Endpoint | Метод | Путь | Назначение |
| --- | --- | --- | --- |
| Models | GET | `/v1/models` | список моделей ключа (OpenAI-совместимая форма). **Работает и без ключа.** |
| **Decisions** | POST | **`/v1/systemone`** | типизированные yes/no / choice / score решения с вероятностями |
| Responses | POST | `/v1/responses` | OpenAI-совместимый, multi-turn, `previous_response_id`, встроенный web search |
| Chat Completions | POST | `/v1/chat/completions` | OpenAI-совместимый (drop-in миграция) |
| Files | POST/GET/DELETE | `/v1/files` | загрузка документов по `file_id` (для Chat/Responses) |
| Conversations | POST/GET/DELETE | `/v1/conversations` | серверная история |

**Decisions API — это контракт System One, дословно:**
- Запрос: `{"model", "state", "questions"}`; `state` — текст/объект/массив; `questions` — **1–16** именованных вопросов (ключи ≤100 симв.), типы:
  - `noul`: `{type, instructions, criteria?: {true?, false?}}` → ответ `{type:"noul", noul: 0..1}`;
  - `choice`: `{type, instructions, criteria: {имя: rubric|null}}`, **2–255 опций** → `{choice, probabilities: {имя: p}, confidence}`;
  - `score`: `{type, instructions, criteria: [уровни...]}`, **2–10 уровней** → `{score (взвешенный, может быть между уровнями), legend, probabilities (по индексам "0","1",...), confidence}`.
- Ответ: `{"model", "answers": {<имя вопроса>: ...}, "usage": {input_tokens, output_tokens}}`. `output_tokens` всегда 0 (платим только за вход).
- Опционально `images`: до 4 шт., PNG/JPEG/WebP, base64 одной строкой без `data:`-префикса, ≤4 MiB декодированных каждая / 8 MiB суммарно, ~64–256 токенов за изображение, тело запроса ≤13 MiB.
- Предел запроса: окно 384K (393 216) токенов; превышение → 400.
- Стриминга у Decisions нет (одиночный JSON). SSE — только Chat/Responses.

**Аутентификация:** `Authorization: Bearer sk-...` (также принимается `X-API-Key`). Ключи выдаёт консоль `console.subq.ai`; формат **`sk-`** (наш экземпляр — `sk-proj-…`, 40 символов; показывается один раз при создании).

**Ошибки** (Decisions): envelope `{"detail": {"error_type", "message"}}` — 400 (превышение контекста), 401 (нет/невалиден ключ), 402 (нет кредитов), 403 (модель недоступна ключу), 422 (невалидный запрос, включая >16 вопросов и битые картинки), 429 (rate limit), 503. Chat/Responses используют OpenAI-форму `{"error": {...}}`.

**Ценообразование:** **$0.05 за 1M входных токенов, выход бесплатный.** Public Preview «first come, first served» при ограниченном capacity (дальше waitlist). **Бесплатный тариф: 100M входных токенов первым 10 000 зарегистрировавшихся** (анонс компании в LinkedIn) — ≈$5, при ~200 токенах на решение это ~500K decisions: с избытком хватает на любые тесты.

**Rate limits:** публичных цифр нет, заголовков вида `x-ratelimit-*` в ответах не наблюдается; есть только код 429 и упоминание `retry-after`.

**Data retention:** по умолчанию хранится всё; на уровне организации — N дней или zero data retention (Decisions под ZDR работают, ничего не хранится). Для тестовых прогонов можно не переживать, но приватные данные в `state` по умолчанию сохраняются у провайдера.

## 3. Живая проверка (2026-10-17, бесплатный ключ)

- `GET /v1/models` без ключа → 200, только `subq-snap-preview`; с ключом — то же (`subq-preview` в early access, этому ключу не выдан).
- `POST /v1/systemone` c телом **ровно из pa-jev тест-фикстуры** (choice 2 опции + score 3 уровня + noul, `criteria: null` у опции) → **200 за 0.63 c**, 177 входных токенов; ответ декодируется нашими `choice_from_wire` / `score_from_wire` / `noul_from_wire` без изменений: `type`-эхо есть, `probabilities` — объект (у score — по индексам уровней), есть `confidence` и `legend`, `usage` — `{input_tokens, output_tokens:0}`.
- 16 вопросов одним запросом → 200 (0.50 c, 781 токен). **17 → 422** `questions: must have at most 16 questions`.
- `POST /v1/chat/completions` с `subq-snap-preview` → 400 «Model does not support completions requests» (Snap — только Decisions).
- `POST /v1/chat/completions` с `subq-preview` → 403 `model_not_allowed` (чат-модель под этим ключом закрыта).

## 4. Вердикт по трём пунктам

**(a) LLM-провайдер для смоук-тестов агента (OpenAI-совместимый чат) — сейчас НЕТ.** Chat Completions/Responses у SubQ OpenAI-совместимы (включая `tools`, `response_format`, SSE), но единственная чат-модель `subq-preview` — Early Access и под бесплатным ключом запрещена (403); `subq-snap-preview` чат не обслуживает (400). Когда Early Access откроют — станет обычным OpenAI-провайдером (base `https://api.subq.ai/v1`), но сегодня смоук-тесты агента на нём не построить.

**(b) Decision/judge-интерфейс, совместимый с System One / pa-jev — ДА, побайтово.** Эндпоинт `POST /v1/systemone`, тело `{model, state, questions}`, примитивы choice/score/noul, ответ `{model, answers, usage}` — всё совпадает с wire-контрактом `pa-jev` поле-в-поле, включая пределы (choice 2–255 — у нас `MAX_CHOICE_OPTIONS=255`; score 2–10 — у нас `MIN/MAX_SCORE_LEVELS`; state объект/массив; `criteria: null` опции). Проверено живым вызовом с телом из нашей тест-фикстуры. Статус-коды ложатся на нашу классификацию ошибок: 400/401/402/403/422 → `JevError::Api` без ретрая (402 «нет кредитов» → cooldown кандидата через `is_account_rejection`), 429/5xx → transient retry (наш backoff уважает `retry-after`). Отличия-мелочи: (1) у SubQ нет refusal-ответов — наш skip-код безвреден; (2) `usage.cost` не приходит → fallback-цена `JEV_INPUT_PRICE_PER_MTOK = 0.042` занижает реальную 0.05/1M на ~16% (на тестах несущественно; для точности можно прокинуть cost-override или price в каталог-запись); (3) лимит 16 вопросов на запрос — наш `JevClientConfig::max_questions` уже умеет чанковать, для этого эндпоинта ставить 16.

**(c) Третий диалект pa-jev — НЕ НУЖЕН, и это лучшая новость.** SubQ говорит **существующим диалектом `typesafe`** (тот же путь `/v1/systemone`, то же тело). Архитектура `pa-jev` прямо это предусматривает: «classification is always by wire dialect (`model.api`), never by provider identity» — SubQ Snap это просто ещё одна каталог-запись (System One сервер), а не новый code path. Подключение без единой строки Rust:

```jsonc
// ~/.prime/agent/models.json → providers (ЭТО НЕ ЧАТ-ПРОВАЙДЕР: api "typesafe")
"subquadratic": {
  "name": "Subquadratic",
  "api": "typesafe",
  "baseUrl": "https://api.subq.ai",        // диалект добавит /v1/systemone
  "apiKey": "sk-proj-PLACEHOLDER",
  "models": [
    { "id": "subq-snap-preview", "name": "SubQ Snap Preview", "api": "typesafe",
      "contextWindow": 393216 }
  ]
}
```
затем `[jev] fallbackChain += "subquadratic/subq-snap-preview"` (или `model` для прямых тестов). `resolve_candidate` найдёт запись, `WireDialect::from_api("typesafe")` выберет диалект, `resolve_judge` встроит кандидатом в `ChainJudge` — с общим cooldown-леджером и answer cache.

## 5. Пример вызова

cURL (ключ плейсхолдером):

```bash
curl --request POST https://api.subq.ai/v1/systemone \
  --header "Authorization: Bearer $SUBQ_API_KEY" \
  --header 'Content-Type: application/json' \
  --data '{
    "model": "subq-snap-preview",
    "state": "my card was charged twice",
    "questions": {
      "dept": {"type": "choice", "instructions": "route the ticket",
                "criteria": {"billing": "charges and refunds", "fraud": null}},
      "urgent": {"type": "noul", "instructions": "the sender needs a response soon"}
    }
  }'
```

Python (httpx):

```python
import httpx, os
r = httpx.post(
    "https://api.subq.ai/v1/systemone",
    headers={"Authorization": f"Bearer {os.environ['SUBQ_API_KEY']}",   # sk-proj-...
             "Content-Type": "application/json"},
    json={"model": "subq-snap-preview",
          "state": "my card was charged twice",
          "questions": {"dept": {"type": "choice", "instructions": "route the ticket",
                                  "criteria": {"billing": "charges and refunds", "fraud": None}}}},
    timeout=10.0,
)
print(r.status_code, r.json()["answers"]["dept"]["choice"])
```

Минимальный запрос: `model` + `state` + **минимум 1 вопрос**. Заголовки — только `Authorization` (+`Content-Type`).

## 6. Рекомендация

1. **Использовать SubQ Snap как первый живой бэкенд для тестов pa-jev уже сейчас** — OpenRouter-ключа не нужно: контракт совместим побайтово (проверено), 100M бесплатных входных токенов, задержки 0.5–0.7 c на запрос, ключ уже есть. Это закрывает потребность «живых тестов jev» до появления jev-latest/OpenRouter.
2. Подключать через существующий диалект `typesafe` каталог-записью (см. §4c) — без кода; в `[jev]` ставить пин `subquadratic/subq-snap-preview` (не «latest»-семантика, в духе ADR-пина `jev-1.13.0`), для этого эндпоинта задать `max_questions = 16`.
3. Для смоук-тестов **агента как чата** SubQ пока не годится (пункт a); держать текущий план (OpenRouter/другой OpenAI-совместимый провайдер).
4. При интеграции учесть две мелочи: fallback-цена 0.042 vs фактическая 0.05/1M (расхождение в учёт, не в работу) и то, что тестовые `state` по умолчанию хранятся у провайдера (для чувствительных данных — ZDR-запрос на организацию).

### Приложение: живой ответ (фрагмент)

```json
{
  "model": "subq-snap-preview",
  "answers": {
    "dept": {"type": "choice", "choice": "billing",
              "probabilities": {"billing": 0.9654, "fraud": 0.0346},
              "confidence": 0.9308},
    "mood":  {"type": "score", "score": 0.3095,
              "legend": {"0": "Calm", "1": "Irritated", "2": "Angry"},
              "probabilities": {"0": 0.7190, "1": 0.2524, "2": 0.0285},
              "confidence": 0.5357},
    "urgent": {"type": "noul", "noul": 0.7678}
  },
  "usage": {"input_tokens": 177, "output_tokens": 0}
}
```
