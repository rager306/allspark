# JEV (TypeSafe) — глубокое исследование

**Дата:** 2026-10-15 · **Статус:** research note (код не менялся)
**Источники:** EXA search + прямой fetch официальных доков `docs.typesafe.ai`, блогов
TypeSafe, arXiv (2609.38850 OpenJev-RLCD, 2609.29769, 2610.17820, 2610.20634), Arize,
LangChain, The Register, Arthur Katcher, The AI Operator; локальный read-only clone
oh-my-pi `/tmp/omp-src` (`grep -ri 'jev|typesafe|systemone|judgment'`); форк
`/root/prime-agent`.

**TL;DR.** JEV — это **не LLM**, а *System One* модель: на вход `state` + типизированные
`questions`, на выход — типизированные ответы с **калиброванными** вероятностями
(`noul` / `choice` / `score`), без генерации текста. Обучение — **RLCD (Reinforcement
Learning for *Calibrated Decisions*)**, а НЕ «Contrastive Distillation» (см. §1.2 —
важное уточнение). Цена — **$0.042 / 1M input-токенов, output бесплатен**; латентность
70–500 ms; типобезопасность гарантирована конструкцией (нельзя вернуть невалидный тип).
Ключевая ценность для кодинг-агента — **gates/routing/verification**, которые сегодня
либо дороги (полный LLM-turn), либо примитивны (regex), либо отсутствуют. oh-my-pi уже
несёт полноценную интеграцию (`judge` model role + `NativeJudge` + SQLite-кэш ответов) —
это готовый образец для порта в allspark.

---

## 1. Что такое JEV: архитектура и RLCD

### 1.1 Позиционирование

TypeSafe AI (основатель **Diogo Almeida**, со-автор методов, лёгших в InstructGPT/ChatGPT)
выпустила Jev 15 сентября 2026. Название класса — **System One Models** (отсылка к
Канеману: быстрое, интуитивное «Система 1»). Jev назван в честь **William Stanley
Jevons** (парадокс Джевонса: падение стоимости интеллекта → рост спроса).

> «Think of Jev as a frontier-intelligence function call: unstructured state in, typed
> probabilistic decisions out.» — Diogo Almeida

Три примитива (вопросов) и их ответы:

| primitive | вопрос | `criteria` | ответ |
|---|---|---|---|
| `noul` | yes/no | опц. `{true,false}` (описания) | `noul`: P(yes), 0..1. **Нет поля `confidence`.** |
| `choice` | выбрать один из ≤255 | map `option → rubric` (или `null`) | `choice`, `probabilities` (сумма 1), `confidence` |
| `score` | позиция на упорядоченной шкале | массив 2..10 уровней | `score` (взвеш. индекс), `probabilities`, `confidence`, `legend` |

**Ключевые свойства архитектуры:**

- **Параллельный сэмплер.** Все вопросы одного запроса считаются за один проход
  (side-by-side), а не авторегрессивно. «Добавление вопросов почти не меняет время
  ответа» — платите только за дополнительные токены вопросов.
- **Нет генерации строк.** Модель «выдаёт значение, и только значение» — не пишет JSON,
  кавычки, имена полей. Отсюда скорость и отсутствие type-errors.
- **Type-safety = аксиома.** «We can confidently add 0% [hallucination]» — потому что
  пространство ответов = ваш тип; вне-типовое значение математически невозможно. Это
  НЕ значит «ответ правильный» (см. §6, критику).
- **Стабильные веса.** Нет fine-tune/LoRA на данных клиента; домен настраивается
  *запросом* (state + instructions + criteria), а не весами.

### 1.2 RLCD — и важное уточнение по аббревиатуре

⚠️ **Терминологическая ловушка.** В задании RLCD расшифрован как «Reinforcement Learning
from **Contrastive Distillation**». Это **другое** RLCD — статья Yang et al., ICLR 2024
(arXiv:2307.12950), про выравнивание LM через контрастивные пары промптов. **У TypeSafe
RLCD = Reinforcement Learning for *Calibrated Decisions***. Это два разных метода с
совпавшим акронимом. Ниже — про метод TypeSafe.

**Что RLCD делает (по официальному AI primer TypeSafe):**

| | RLHF | RLVR | **RLCD** |
|---|---|---|---|
| Оптимизирует | ответы, которые *предпочитает человек* | *верифицируемо* правильные исходы | **калиброванные решения** |
| Входы | текст, диалог | задача + верификатор | структурированное состояние программы |
| Выходы | строки | строки/трассы | **типизированные значения + вероятности** |
| Сэмплирование | последовательное | последовательное | **параллельное** |

Ключевая идея калибровки: «если модель говорит 0.8, то в среднем в 80% случаев ответ
верен». Это свойство *группы* предсказаний, не отдельного ответа. Именно оно делает
вероятность пригодной для автоматизации: код может действовать сам при высокой
уверенности и эскалировать при низкой.

**Чем RLCD отличается от RLHF/DPO (мотивация TypeSafe):**
1. RLHF вознаграждает *убедительность* → сикофантство и уверенные галлюцинации; RLCD
   вознаграждает *честную неопределённость*.
2. RLHF вызывает **mode dropping** (модель схлопывается к одному стилю) — RLCD работает
   с ограниченным (bounded) пространством ответов, схлопываться некуда.
3. RLHF/DPO не дают калиброванного числа; RLCD ставит калибровку в центр objective.

**Как это устроено внутри (публичная реконструкция — arXiv:2609.38850, OpenJev-RLCD).**
TypeSafe **не публикует** точный алгоритм. Независимая работа описывает рабочий RLCD для
reasoning-моделей и даёт полезную теорию:
- Модель сэмплит rationale, затем читается распределение ответов `u(r)` после rationale;
  каждый rationale скорится **strictly proper scoring rule** (Brier/log score).
- **Variance identity:** `J(E_r u) = E_r J(u) + Var_r(u)` — скоринг *смеси* сэмплов
  награждает rationale за *несогласие*, что деградирует рассуждение. RLVR = mixture
  objective без diversity-терма (т.е. оптимум = mode, а не калибровка).
- Рецепт «**calibrate, then reinforce**»: сначала обучить readout proper score'ом, затем
  дообучить reasoning тем же score'ом с KL-якорем. Двухстадийный RLCD обходит
  GRPO/SFT/RFT+TS по AURC (risk–coverage) и позволяет автоматически решать 81% vs 19%
  задач при ошибке ≤5% на GSM8K-verify.

**Что это значит практически:** RLCD — это не «ещё один RLHF», а смещение всей
пост-тренировки к *калиброванной вероятностной голове* над bounded-ответами. Калибровка
эмпирически проверяема и на ваших данных не гарантирована (см. §6 — Arize нашёл, что
дефолтный порог 0.5 портит результат; порог надо тюнить).

### 1.3 Модели и алиасы

| | Jev 1.14 | Jev 1.13 |
|---|---|---|
| Model ID | `jev-1.14.0` | `jev-1.13.0` |
| Цена (Btok / Mtok) | $42 / $0.042 | $42 / $0.042 |
| Контекст | 64k / запрос · 32k / вопрос | то же |
| Вход | текст (string / object / array) | текст |

Алиасы: `jev-latest` → `jev-1.13.0` (стабильный), `jev-preview` → `jev-1.14.0` (новейший).
**Alias движется** при релизе — если пороги затюнены под версию, пиньте точный ID.
Long context: заголовок `TypeSafe-Danger-Zone-Long-Context: true` поднимает до 262k
(дороже по времени, менее точно).

### 1.4 Известные слабости (официальный список «jaggedness»)

- **Нет арифметики** — не считает и не сравнивает числа надёжно. Считайте в коде.
- **Нет дат** — читает как текст, не как упорядоченную величину.
- **Нет текста/кода/саммари** — не обучен.
- **Нет объяснений** — возвращает число, никогда причину.
- **Нет multi-hop** — один вопрос = один шаг; двойные отрицания ломают.
- **Падение точности с ростом state** — шум в state ухудшает ответ; фильтруйте.
- **Не считает вход враждебным** — инъекции в state двигают ответы (см. §6).

---

## 2. API reference

### 2.1 Транспорт

```
POST https://api.typesafe.ai/v1/systemone
Authorization: Bearer <TYPESAFE_API_KEY>
Content-Type: application/json
```

- `GET https://api.typesafe.ai/v1/models` — список моделей/алиасов аккаунта.
- Один JSON round-trip, **без стриминга, без состояния сессии**.
- Сжатие тела: `Content-Encoding: gzip | x-gzip | zstd` (один слой, zstd window ≤ 8 MiB).
- Лимиты тела: ≤ 32 MiB (сжатое и распакованное); ≤ 64 уровней вложенности; только
  конечные числа (NaN/Inf/1e400 → 400).
- Long context: `TypeSafe-Danger-Zone-Long-Context: true` → 262k.

### 2.2 Request

```json
{
  "model": "jev-latest",
  "state": "<string | object | array>",
  "questions": {
    "is_urgent": { "type": "noul", "instructions": "Does this convey urgency?",
                   "criteria": { "true": "Explicitly time-sensitive", "false": "No urgency" } },
    "department": { "type": "choice", "instructions": "Which team?",
                    "criteria": { "billing": "Payments", "technical": "Bugs", "sales": "Pricing" } },
    "frustration": { "type": "score", "instructions": "How frustrated?",
                     "criteria": ["Calm", "Frustrated", "Very angry"] }
  }
}
```

- `state` — string | object | array; ингестится **один раз** для всех вопросов.
- `model` — required. `instructions` может быть строкой, объектом или массивом (длинный
  вопрос + данные, ссылки в бэктиках).
- `questions` — map, ключи **не** отправляются модели; ответы приходят под теми же ключами.
- `criteria` у `choice` — map option→rubric (≤255); у `score` — упорядоченный массив 2..10;
  у `noul` — опц. `{true,false}`.

### 2.3 Response

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "department": { "type": "choice", "choice": "billing",
                    "probabilities": { "billing": 0.88, "technical": 0.12, "sales": 0.0 },
                    "confidence": 0.81 },
    "frustration": { "type": "score", "score": 1.05,
                     "legend": { "0": "Calm", "1": "Frustrated", "2": "Very angry" },
                     "probabilities": { "0": 0.0, "1": 0.95, "2": 0.05 },
                     "confidence": 0.92 },
    "is_urgent": { "type": "noul", "noul": 0.95 }
  },
  "usage": { "input_tokens": 428, "output_tokens": 73 }
}
```

**Гочи (важно для клиента):**
- `score` возвращает **взвешенный индекс** по уровням (напр. `1.05`), может быть *между*
  уровнями. `probabilities` у score ключуются **строковыми индексами** (`"0"`,`"1"`), не
  подписями — маппинг через `legend`.
- `noul` **не несёт `confidence`**. Прокси уверенности: `|2p − 1|`.
- `confidence` — концентрация распределения (1 = вся масса на одном, 0 = равномерно),
  вычисляется из `probabilities`. **Probability ≠ confidence**: 0.51 vs 0.49 — ясный
  победитель, но почти нулевая уверенность.
- **Нельзя арифметику между ответами** — «вероятность что true» и «вероятность что false»
  не обязаны суммироваться в 1. Задавайте вопрос, который имеете в виду.

### 2.4 Errors / retries

| Status | Значение | Действие |
|---|---|---|
| 400 | нарушено правило тела / `max_tokens_exceeded` | не ретраить |
| 401 | нет/неверный ключ | ротация ключа |
| 413 | тело > 32 MiB | уменьшить state |
| 415 | неподдерж. `Content-Encoding` / >1 слой | не ретраить |
| 422 | ошибка валидации (напр. превышен cap вопросов) | **сплит** вопросов |
| 429 | rate limit (per-org, tokens/s + req/s) | `retry-after` + exp. backoff |
| 529 | TypeSafe перегружен | backoff + retry |

Rate limits: **250 000 tokens/s, 1 200 req/min** (TypeSafe предупреждает, что лимиты
нестабильны и меняются без уведомления).

### 2.5 SDK / MCP / gateways

- **Официальные SDK:** Python (`typesafe_sdk`), JS/TS (`@typesafe-ai/sdk`). Типы выводятся
  из вопросов (TS: `response.answers.department.choice: "billing"|"technical"|"sales"`).
- **MCP:** `jkudish/jev-mcp` (530★) — 12 инструментов (`jev_verify`, `jev_screen`,
  `jev_gate`, `jev_review`, `jev_classify`, `jev_rerank`, `jev_extract`, ...).
- **Agent Skill:** `typesafe-ai/skills` (`npx skills add typesafe-ai/skills`).
- **Gateway-хосты:** OpenRouter Decisions (`POST /api/alpha/decisions`), Cloudflare Workers
  AI (Clef), OpenAI Decisions, Vercel, Netlify, Command Code, OpenCode Zen, Kilo, nano-gpt.
  Один и тот же wire `{model, state, questions}` — 7 из 8 decision-моделей принимают
  формат Jev.

---

## 3. Применение в кодинг-агентах

TypeSafe прямо пишет: **Jev не заменяет LLM за Claude Code/Cursor/Codex**, но это
идеальный инструмент *внутри* агента. Пять decision-surface в цикле агента:

| Surface | primitive | на что ветвитесь |
|---|---|---|
| Model routing | `choice` | cheap vs frontier |
| Tool selection | `choice` | какой tool (или none) |
| Context compaction | `score` | keep / summarize / drop |
| Loop detection | `noul` | «повторяется без прогресса?» |
| Risk gate | `noul` + `choice` | allow / confirm / block |

### 3.1 Паттерны (TypeSafe + сообщество)

1. **Speculative Fan-Out** — задавайте много вопросов (включая спекулятивные) одним
   запросом; state биллится один раз, параллельно. 60 вопросов = ~20× меньше токенов и
   ~8× быстрее, чем 60 отдельных запросов, с идентичными ответами.
2. **Confidence-Gated Routing** — confidence как вторая ось решения (act / escalate).
3. **Composite Scoring** — несколько измерений → один скор в коде (код владеет арифметикой).
4. **Intent Routing** — классификация намерения → handler.
5. **Gates** (архетип из harness-анатомии: Instructions, Tools, Memory, Loop, **Gates**) —
   главная ниша Jev. Дорогой gate («второй моделью на каждый шаг») не запускают; при
   ~$0.00002 и ~400 ms он становится *always-on*.

### 3.2 Пять «seat» для Jev в harness (из 100 постов волны)

| Seat | Что делает | Пример |
|---|---|---|
| **Router** | выбирает модель/effort до дорогого вызова | Claude Code mod: `subagent model + effort + main model` (main роутится только на старте сессии — иначе сносится prompt-cache) |
| **Gate** | safety-проверка перед каждым tool call | fx auto-mode: Jev **5–18× быстрее и точнее** GPT-5.6-luna |
| **Selector** | computer/browser use: код строит список кандидатов, Jev выбирает ID | trycua jev-use; OCR-боксы как options (~$0.003, 1.5 s/шаг) |
| **Verifier** | проверка «задача действительно завершена?» | ловит фейковые «done»; но проверка *каждого* turn начинает вето́ить нужные шаги |
| **Triage** | PR-ревью: 14 типизированных проверок → BLOCK/review/merge | Rosson: 14 проверок одним вызовом, **$0.00007 / PR** |

### 3.3 Как это реально делает oh-my-pi (образец для порта)

oh-my-pi несёт Jev как **первоклассную `judge` model role** + *judgment APIs*:

- **Wire-диалекты** (`packages/ai/src/judgment/`): `typesafe` (`POST {base}/v1/systemone`),
  `openrouter-decisions` (`{base}/decisions`), `workers-ai-systemone` (Cloudflare envelope),
  `openai-decisions` (трансляция в обе стороны). Один `NativeJudge` обслуживает все;
  `isJudgmentApi(api)` решает.
- **Role chain** (`judgment/index.ts`): `judge` — model-kind role; цепочка
  `typesafe/jev-latest → openrouter/~typesafe/jev-latest → tiny → smol → default`.
  **Критично:** «от первого native-кандидата и дальше остаются только native» — prompted
  chat-модель никогда не подменяет провалившийся native judge (она не воспроизведёт
  калиброванные вероятности).
- **Answer cache** (`judgment/cache.ts`): SQLite, ключ
  `(sha256(canonical state), model, name, type, instruction, criteria)`; повторный вопрос
  к идентичному state — локально бесплатно, наружу уходят только неотвеченные вопросы.
- **Usage/telemetry:** каждый attempt атрибутируется ровно один раз (ledger + span).
- **Потребители:** `find` (semantic grep, каскад), `auto-thinking` (difficulty classifier),
  `unexpected-stop` (детект оборванного turn), `ttsr` (judged rules), `git ai-stage`
  (selective staging), `eval` `judge()`/`judge_batch()` (Python/JS kernel), `stats`
  (frustration), auth-gateway `POST /v1/systemone`.
- **Token-точность:** oh-my-pi даже **восстановил токенизатор Jev** (639k probe'ов к API,
  `crates/pi-natives/src/utok/jev.rs`) — чтобы точно считать `state`-токены для лимита
  32k и truncation.

**Примеры конкретных вызовов в oh-my-pi:**

```ts
// auto-thinking/classifier.ts — difficulty → effort
const { answers } = await judge.judge(
  { state: { request: preprocessTinyMessage(input.request) },
    questions: { level: { type: "choice", instructions, criteria: {low,medium,high,xhigh} } } });
return LEVEL_EFFORT[answers.level.choice];

// unexpected-stop-classifier.ts — «turn обещал действовать и оборвался?»
const { answers } = await judge.judge(
  { state: { message: text }, questions: { stopped: QUESTIONS.stopped } });
return answers.stopped.noul >= 0.5;   // порог 0.5, калиброванная вероятность

// ttsr — judged rules: все вопросы кандидатов одним запросом над общим state
// state {output, content}; content обрезается до 32 000 Jev-токенов (Encoding.Jev);
// yes ≥ 0.7 → правило срабатывает; Jev биллит state один раз → правила делят цену
```

**Паттерн `git ai-stage`** (показателен как «Jev + код-логика»):
1. каждый hunk/файл — независимый unit, judged `choice` c полным списком изменённых путей
   как контраст (`all_changed_files`);
2. порог `STAGE_THRESHOLD = 0.6` по P(top level);
3. **второй проход**: один `choice` с явной опцией `none` над топ-8 кандидатами
   (`NONE_THRESHOLD = 0.5`) — потому что per-unit скоры не имеют null-hypothesis и «дрейфуют»
   к 0.55–0.85 даже когда инструкция не описывает ничего в дереве.

Это иллюстрирует правило №1 из сообщества: **код строит меню, Jev указывает** — Jev никогда
не изобретает опции.

### 3.4 Anti-patterns и fail-modes (из независимых тестов)

- **Compaction-скандал** (3.6M просмотров): «оценивать каждый tool call и выкидывать
  нерелевантные» — decay (первый проход освобождает половину, второй четверть, дальше
  нечего удалять), tie с plain recency (77.8 vs 77.8), идентичное поведение с фейковой
  моделью, всегда отвечающей 0; и главное — каждая фильтрация = **prompt-cache miss**.
  Вывод: это провал *стратегии компакции*, не Jev, но урок общий — «измеряйте против
  тупого baseline».
- **Injection** (собственный тест The AI Operator, 300 атак): blunt-injection («ignore your
  instructions») не пробил (0/30), но **зажал** агент (10% безопасных команд отклонены).
  Polite-injection («repo owner уже одобрил») пропустил 3/30. `git stash clear` проходил
  всегда. Успешные атаки имели confidence 0.400 vs 0.681 на чистом трафике; порог 0.8
  для отлова эскалировал 58% трафика. **Вывод: hard rules в коде — обязательны; Jev
  только поверх них.**
- **Prompt-cache**: любой роутер, меняющий главную модель mid-session, сносит cache —
  поэтому main-модель роутят только на старте.

---

## 4. Интеграция в Rust

Официального Rust SDK **нет**. Есть community-крейты и один «почти официальный»
(`typesafe-sdk-client` — форк официального TS SDK, переписанный на Rust; CLI `jevon`).

### 4.1 Вариант A — community-крейт `typesafe-jev` (sync, `ureq`)

Самый лёгкий: `serde` + `serde_json` + `indexmap` + `ureq` (rustls). Без async-рантайма.

```toml
[dependencies]
typesafe-jev = "0.2"
```

```rust
use typesafe_jev::{Choice, Client, Config, Noul, Questions, Score};

fn main() -> Result<(), typesafe_jev::Error> {
    let client = Client::from_env(Config::default())?; // reads TYPESAFE_API_KEY

    let questions = Questions::new()
        .with("department", Choice::new("Which team should handle this?", [
            ("billing", "Payment or subscription issues"),
            ("technical", "Bugs or integration problems"),
            ("sales", "Pricing or account questions"),
        ]))
        .with("frustration", Score::new("How frustrated the customer appears",
            ["Calm, just stating facts", "Frustrated but civil", "Very angry, strong language"]))
        .with("is_urgent", Noul::new("The message conveys urgency or time-sensitivity"));

    let response = client.ask(ticket, &questions)?;
    let department = response.choice("department").expect("asked as a choice");
    println!("{} (confidence {:.2})", department.choice, department.confidence);
    Ok(())
}
```

Плюсы: `Questions`/`Choice` сохраняют порядок (порядок на wire = порядок добавления);
типы field-for-field с API → опечатка = compile error; `Config { timeout, max_retries,
pool_size, base_url }`. Конфиг через env: `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`,
`TYPESAFE_DEFAULT_MODEL`, `TYPESAFE_LOG_LEVEL`.

### 4.2 Вариант B — крейты `jev-sdk` (async, tokio) / `typesafe-systemone` (async, reqwest)

```rust
use jev_sdk::{Choice, Noul, Question, Score, TypeSafeClient};

let client = TypeSafeClient::from_env()?;             // async, reqwest
let response = client.system_one(ticket, [
    ("department", Question::from(Choice::new("Which team should handle this", [
        ("billing", Some("Payment or subscription issues")),
        ("technical", Some("Bugs or integration problems")),
    ]))),
    ("is_urgent", Question::from(Noul::new("The message conveys urgency"))),
]).await?;
```

`jev-sdk`: `TypeSafeClient::builder` (timeouts, retry, extra headers, custom reqwest),
`RetryPolicy` (429/529 — exp backoff), `list_models()`. `typesafe-sdk-client` (0.6.2) —
13 крейтов с layering, `#[derive(Questions)]` proc-macro для генерации enum'ов подписей
(exhaustive `match`, опечатка = compile error), `Transport` trait + `Mock` для тестов без
сети. `jev` крейт умеет `TypeSafeClient::local` (self-hosted backend без API-ключа,
читает tagged и untagged ответы).

### 4.3 Вариант C — свой минимальный клиент (рекомендация для форка)

Для allspark **не тянуть сторонний крейт как hard-dep**: wire — это один JSON POST.
Форк уже имеет `reqwest` (в `pa-core`) и общий helper для non-streaming JSON POST.
~300-строчный in-crate клиент (типы + reqwest) даёт: чистый supply-chain, переиспользование
auth-registry форка, контроль над retry/chunking/rotation. Совпадает с правилом форка:
«новая работа — в новых файлах/крейтах, без новых зависимостей, где workspace уже покрывает».

Минимальный контракт клиента:
- `Question::{Noul, Choice, Score}` + `Answer::{...}` (serde, field-for-field);
- `systemone(state, questions, opts) -> Result<JudgmentResult>` с `maxQuestions`-chunking
  (≤4 в полёте), retry (`408/429/5xx` + conn/timeout, `retry-after`-aware, jitter),
  401/403 rotation-hook, capture `x-typesafe-request-id`;
- `Usage { input_tokens, output_tokens, cost }`, где `cost = input_tokens / 1e6 * 0.042`;
- pure `decode` + `Judge` trait (позволяет prompted-fallback для degraded mode, но
  role-chain обязан предпочитать native).

**Cargo:** `reqwest` (есть), `serde`/`serde_json` (есть), `indexmap` (для порядка), опц.
`rusqlite`/переиспользование `pa-graph` store для answer-cache.

---

## 5. Ценообразование и экономика

### 5.1 Официальные цифры

| | Значение |
|---|---|
| Input | **$0.042 / 1M токенов** ($42 / 1B) |
| Output | **бесплатно** (репортится в `usage`, не биллится) |
| Latency | **70–500 ms** end-to-end (70–150 ms в рекламе; ~400 ms реалистично) |
| Rate limits | 250 000 tokens/s, 1 200 req/min (нестабильны) |
| Context | 64k / request, 32k / question |
| Prepaid | паки от $5, кредиты не сгорают; на больших паках до $0.25/M (у части реселлеров) |

**Экономический принцип:** биллится только `state` + токены вопросов, один раз на запрос.
«Единственное число, которое двигает счёт — средняя длина `state`». Батчинг независимых
вопросов в один вызов — главная оптимизация.

### 5.2 Измеренные цифры (не vendor-арифметика)

- Реальная стоимость решения (12 use cases, 364–724 input-токенов): **$0.0000153–$0.0000304**.
  Широко цитируемые «~$0.0004/decision» — это *гораздо больший* state.
- The AI Operator (300 вызовов через OpenRouter, ~450 токенов контекста + 2 вопроса):
  median **371 ms**, p90 495 ms, p95 533 ms; **$0.0000189 / вызов**; 500 вызовов/день < 1¢;
  полный прогон 300 вызовов = $0.0057.
- Arize (RAGTruth, hallucination): **$0.045–$0.05 / 1000 judgments**, ~141 ms, vs Claude
  Opus 5 **$14.30 / 1000**, >3 s → ~**300× дешевле**, ~23× быстрее при равной accuracy.
- Arize decision-model bracket: Jev **$0.022 / 1000 routing**; $0.034–0.045 / 1000 RAGTruth.
- Rosson PR-review: **$0.00007 / PR** (14 проверок одним вызовом).
- Doom-демо: 10 queries/s ≈ **$7/час**.
- Точка безубыточности: при ~500 решениях/день стоимость < 1¢.

### 5.3 Как читать множители (важный контекст)

TypeSafe-маркетинг: «193.6× быстрее, 444.6× дешевле» — это **один** workflow-демо
($0.000081 vs $0.013880; 0.114 s vs 8.566 s), и TypeSafe сам помечает как «higher end».
Официально публикуемый диапазон: **40×–200× быстрее**. Независимый разбор (learnjev):
против *ближайшего по accuracy* peer — ~**76× дешевле, 25× быстрее** при статистически
неразличимой accuracy; против лучшего baseline — **6.3 пункта accuracy позади**. Множитель
зависит от baseline; цитируйте 40×–200×.

---

## 6. Сравнение с альтернативами

### 6.1 vs полноразмерный LLM-judge

**Arize (RAGTruth + SummEval, 23 325 judgments, human labels):**
- На hallucination detection: Jev и Claude Opus 5 оба **87% accuracy** (после тюнинга порога);
  Jev $0.05/1000 vs Opus $14.30/1000; 141 ms vs >3 s.
- **НО** при дефолтном пороге 0.5 Jev выглядит на 7 пунктов хуже (76% vs 83%) — «порог
  оказался рычагом больше, чем выбор модели». ROC-AUC: Jev 0.94, Opus 0.95 — ранжируют
  почти одинаково; Jev проигрывал только из-за линии отсечения.
- Тюнинг порога (Jev 0.80, Opus 0.65) → **идентичны в пределах тысячных** по всем метрикам,
  включая профиль ошибок.
- **Контр-нарратив про overconfidence:** GPT-5.6 Terra — да (заявляет 96%, права 70%);
  но Opus 5 на high effort *лучше калиброван* (81%→83%), чем Jev (недоуверен). «LLM
  overconfident» — не закон.

**Rao & Callison-Burch (UPenn, arXiv:2609.29769, 9 панелей, 5003 пары):**
- Jev значимо отличается от flash-tier LLM-судей лишь в **8 из 27** парных сравнений,
  впереди — на binary checklist, позади — только на ordinal.
- LLM-судьи дороже в **16–325×** и медленнее в **28–350×**.
- **Но:** судьи ошибаются **в одних и тех же местах**. На самых уверенных ошибках Jev
  **96% LLM-вердиктов повторяют его неверный ответ** (против ~50% при независимости).
  → **Каскад confidence-gated даёт экономию, но не точность**: даже с oracle-порогами
  ни один каскад не бьёт лучшего одиночного судью больше чем на 2.7 пункта. Каскад
  работает, только если судьи дают *комплементарные* ошибки.

**CustomGPT.ai (arXiv:2610.20634, RAG claim verification, 495 claims):**
- Jev vs gpt-6-astra: статистически неразличимы (**−0.56 пункта**, p=0.49) при **187×
  разнице в цене** ($0.048 vs $8.93 / 1000).
- Но: они **расходятся на 15.2%** claims; Jev пропускает **23.2%** неподтверждённых
  против 14.1% у frontier — «более разрешающий guard».
- Каскад (cross-fitted порог) достигает 74.10% при $2.92/1000 vs 73.83% при $8.93; **но
  matched random-escalation control при том же бюджете работает не хуже** → экономию даёт
  *доля эскалируемых*, а не сам сигнал вероятности.

**Agentailor (193 реальных eval-вердикта, 5 прогонов):**
- Jev 90.6% vs Sonnet 4.6 93.2% vs Gemini 3.5 Flash-Lite 94.8%; **$0.07 vs $13.21 / 1000
  (180×)**, 0.25 s vs 5.4 s.
- **Продукт — это вероятность**: на 125 items, где Jev был уверен (p>0.9 или <0.1), он
  сделал **ноль ошибок**. Почти все промахи — около 0.5.
- Каскад (Jev решает уверенное, неопределённые 13–16% → Sonnet) матчит accuracy Sonnet
  за **1/6 цены**.

### 6.2 vs rule-based

- Rule-based (hard rules) — детерминирован, мгновенен, бесплатен, «никогда не передумает».
  The AI Operator нашёл, что Jev **наименее надёжен именно там, где hard rule легче всего
  написать** (`git stash clear` прошёл под всеми framing'ами). Вывод сообщества:
  **hard rules идут первыми; Jev видит только то, что они не решают.**
- Jev выигрывает, когда вопросы меняются еженедельно и нет размеченных данных. При
  стабильных метках и лейблах — обучайте маленький классификатор (дешевле и локально).

### 6.3 vs embedding similarity

- Embedding-поиск (векторная близость) — дешёв, но не отвечает на типизированный вопрос
  («это относится к рефактору X?») и не даёт калиброванной вероятности.
- Jev MCP даёт `jev_find`/`jev_rerank` — «найти документ/файл, отвечающий на вопрос,
  среди сотен кандидатов, **без embeddings и без индекса**». Это semantic rerank через
  суждение, а не через близость векторов.
- Однако на **очень больших** списках Jev упирается в 255 options → паттерн «отфильтровать
  в коде, заскоррить остаток, выбрать из шортлиста» (или embeddings как first stage).

### 6.4 vs decision-модели-конкуренты (Arize bracket, 8 моделей)

| Модель | Halluc (RAGTruth) | Phoenix | Routing | Calib err | Latency p50 | $/1000 |
|---|---|---|---|---|---|---|
| **Jev 1.13** | 85.5% | 91.1% | 49/52 | **0.023** | 0.08 s | $0.045 |
| Clef 27B (Cloudflare) | 85.2% | 91.5% | 50/52 | 0.062 | 0.50 s | $0.223 |
| Liquid d1 | 84.8% | 87.6% | 48/52 | 0.051 | 0.20 s | $0.034 |
| OpenAI Decisions | 81.5% | 90.0% | 48/52 | 0.042 | 0.16 s | $0.090 |
| Kev 9B (open) | 76.8% | 88.7% | 48/52 | 0.055 | 1.03 s | $0 (self-host) |
| Laya multilingual (BERT) | 59.8% | 57.3% | 40/52 | 0.330 | 0.04 s | $0 |

Вывод: топ-модели — **коммодити по accuracy**; выбирайте по deployment (API vs self-host),
цене и **устойчивости к формулировке**. **Wording twist:** переформулировка вопроса с
обратной полярностью переворачивает многие модели (Liquid 0.92→0.17, Jeeves 0.85→0.25),
но **Jev держит позу** (0.93→0.93). Тестируйте обе формулировки.

### 6.5 Adversarial robustness (arXiv:2610.17820, первое контролируемое сравнение)

- **Input moderation** (rule-based задача): Jev — **самый устойчивый**; 8 подделок его
  структурированного входа не обошли ни одного блока; атака «зачем кому-то делать вредный
  запрос» обошла 9% блоков Jev vs 56% GPT-6 Astra.
- **Phishing detection** (judgment-based задача): результат **разворачивается** — те же
  подделки обходят 19–29% писем, которые Jev флагует; без атаки Jev ловит 62% фишинга vs
  77–81% у GLM.
- Jev — самый быстрый, но **флагует больше всего benign (27%** vs 9–17% у LLM).
- **Вывод:** устойчивость зависит от задачи — rule-based Jev устойчив, judgment-based
  более уязвим. Валидация на своём домене обязательна.

---

## 7. Рекомендации для форка allspark

Форк `/root/prime-agent` сейчас **не имеет** Jev/TypeSafe ни в `crates/`, ни в настройках
(`grep` пуст). Есть отдельная note `.research/jev-fork-integration.md` (422 строки),
где предложен детальный дизайн. Ниже — сжатые, приоритезированные рекомендации,
обогащённые новыми данными этого исследования.

### 7.1 Где Jev даёт максимум (для форка)

| # | Точка интеграции | primitive | Почему |
|---|---|---|---|
| **1** | **auto-thinking difficulty classifier** | `choice` | Форк не имеет per-prompt difficulty; oh-my-pi портирован, самодостаточно, высокая отдача (управление `thinking_level`). |
| **2** | **`find`-style semantic rerank над `pa-graph`** | `noul`/`choice` | `pa-graph` уже имеет FTS5/BM25/RRF/MMR; Jev — re-rank top-K одним запросом (каскад как в oh-my-pi `find`). Здесь скорость+цена окупаются. |
| **3** | **Tool-selection arbiter** (`lsp`/`graph`/`bash.grep`) | `choice` | Форк уже несёт `lsp`/`graph`/`edit`; Jev даёт routing-hint. |
| **4** | **`eval`/kernel `judge()` bridge** | все | Форк — Python-оркестрация; регистрация host-request `judge` даёт RLM-слою типизированные решения inline. Высокая ценность, средние усилия. |
| **5** | **Risk gate перед tool call** | `noul`+`choice` | Highest-leverage safety; **только поверх hard rules**, fail-open на ошибке/таймауте. |
| **6** | **Edit-review annotation** | `choice`/`score` | *Пост-хок*, **advisory only**, никогда не блокирует запись. |

### 7.2 Где Jev НЕ ставить (детерминизм важнее)

- **Provider failover** (`provider_failover.rs`, `models/resolver.rs`) — цикл должен быть
  детерминированным, синхронным, offline-safe; классы ошибок уже покрыты чистыми
  предикатами. (Опционально: классификация *текста* незнакомой ошибки в
  `retry|reroute|permanent` — best-effort, вне hot path.)
- **Edit guards** (seen-lines / stale-tag) — вероятностный судья не может быть причиной
  отказа/разрешения записи. Guard = детерминирован; Jev = advisory annotation.
- Где нужна **проза/код/инструменты** — только LLM.

### 7.3 Архитектура (слои, как `lsp`/`graph`/`edit`)

```
pa-jev (NEW crate)        wire types + client + question builders + answer parsing
   ^
pa-core::jev (NEW mod)    Judge trait / role-chain resolver + JevTool (AgentTool)
   ^
pa-core::session_engine   create_session wiring (gate + push + native_tool_names)
   ^
internal call sites       auto-thinking, find-rerank, eval-bridge, tool-arbiter
```

- **Новый крейт `crates/pa-jev`** (`#![forbid(unsafe_code)]`, pedantic-clean): типы
  `Question`/`Answer` field-for-field с API; `JevClient` над `reqwest` с chunking (≤4 в
  полёте), retry, rotation-hook, `Usage` (`cost = input/1e6*0.042`).
- **`pa-core::jev`**: `resolve_judge(settings, registry) -> Option<Arc<dyn Judge>>` через
  существующий `AuthStorage`/`ModelRegistry`; `JevTool` (actions `ask|classify|route|check`).
- **Settings**: `JevSettings { enabled: Option<bool> = OFF, model, baseUrl, maxQuestions,
  confidenceThreshold, cacheEnabled }`; getter рядом с `get_lsp_enabled`; регистрация в
  `KNOWN_FIELDS`. **Default OFF** — платный сторонний endpoint (в отличие от lsp/graph/edit
  default ON).
- **`create_session`**: захват gate до move `settings`; push `jev` после `edit`; запись в
  `native_tool_names` (уже wired в `pa-daemon`).
- **Опционально Phase 3** — `typesafe` provider в `models_fork.rs` + judgment `Provider` +
  `modelRoles.judge`. **Реплицировать правило oh-my-pi:** однажды native в role chain —
  никогда не откатываться к prompted модели.
- **Answer cache** (Phase 2): SQLite, ключ
  `(sha256(canonical state), model, name, type, instruction, criteria)`; `rusqlite` уже в
  workspace через `pa-graph`.

### 7.4 Правила поведения (из независимых тестов — критично)

1. **Hard rules первыми.** Детерминированные запреты в коде до вызова Jev. Именно там,
   где правило легко написать, Jev ненадёжен (`git stash clear`).
2. **Код строит меню, Jev указывает.** Никогда не давать Jev изобретать опции; всегда
   включать явную опцию `none`/`other` (иначе получите уверенный неверный pick).
3. **Батчить вопросы** одним запросом (state биллится один раз; параллельно).
4. **Определить «unsure» до ship.** Общая стартовая полоса: act > 0.9, deny < 0.1,
   escalate между. Но **тюнить на своих данных** (observe-mode неделю → пороги из лога).
5. **Fail-open/fail-closed осознанно** per hook. Claude Code recipe — fail-open (сеть не
   должна ломать сессию); side-effecting agent — fail-closed (эскалация к человеку).
6. **Не считать арифметику/даты** через Jev — только в коде.
7. **Truncate state** (oh-my-pi режет до ~24k символов в git-stage; до 32k Jev-токенов в
   ttsr). `state` — единственное, что двигает счёт.
8. **Respect prompt-cache:** не менять главную модель mid-session (роутить только на старте).
9. **Измерять против тупого baseline** (константа / coin-flip). Если результат тот же —
   Jev не вносит вклада.
10. **Тестировать обе формулировки вопроса** (polarity flip) перед доверием.

### 7.5 Фазирование

1. **Phase 1 — client + tool.** `pa-jev` (типы, client, chunking, retry), `pa-core::jev`
   (`Judge` trait, `resolve_judge`, `JevTool`), settings (OFF), `create_session` wiring,
   тесты. Без внутренних call sites.
2. **Phase 2 — один high-value consumer.** auto-thinking difficulty classifier (7.1#1)
   и/или `find`-rerank (7.1#2) + answer cache.
3. **Phase 3 — опционально model role.** `typesafe` provider, judgment `Provider`,
   `modelRoles.judge`; edit-review annotation; cache-anomaly.
4. **Явно пропустить:** Jev внутри failover и edit guards.

### 7.6 Тесты (что покрыть)

- `pa-jev` unit (без сети): wire encode/decode для всех 3 примитивов против doc-fixtures
  (вкл. `score`=weighted index, `probabilities` по индексам, `noul` без `confidence`);
  `cost = input/1e6*0.042`; chunking (40 вопросов при `maxQuestions=16` → 3 запроса,
  merge, sum usage, abort siblings); retry (429 с `retry-after`; 5xx; 400/404/422 не
  ретраить, 422 → split); error mapping; 401/403 rotation; missing answer → typed error.
- `pa-core::jev`: `resolve_judge` = None когда disabled/нет ключа; role chain предпочитает
  native; answer cache (повтор локально, наружу только неотвеченные).
- session-engine: `jev` пушится только при `jev.enabled`; `native_tool_names` содержит;
  `tool_profile` может исключить; default-OFF = ноль сетевых вызовов.
- Integration (gated): `JevTool::execute` против mock HTTP; live smoke за `TYPESAFE_API_KEY`
  (skip если unset, как `pa-lsp`).
- Merge gate: `Makefile::check` (`fmt --check`, `clippy -D warnings`, `test --workspace`,
  release build).

### 7.7 Зависимости

`reqwest` (уже в `pa-core`), `serde`/`serde_json` (уже), workspace `AuthStorage` для ключа.
**Без сторонних Jev SDK.** `TYPESAFE_API_KEY` в `env_api_keys.rs` если строится model-role.

---

## Приложение A — ключевые источники

- TypeSafe: `typesafe.ai/blog/introducing-system-one-models-and-jev`,
  `docs.typesafe.ai/{api,models,confidence,introduction/machine-learning-primer,
  introduction/coding-agents,introduction/quickstart,patterns}`
- RLCD (метод TypeSafe): AI primer + `docs.typesafe.ai/confidence`; независимая
  реконструкция — arXiv:2609.38850 (OpenJev-RLCD)
- RLCD (другой метод): arXiv:2307.12950 (Yang et al., ICLR 2024) — Contrastive Distillation
- Сравнения: arXiv:2609.29769 (Rao & Callison-Burch), arXiv:2610.17820 (adversarial),
  arXiv:2610.20634 (CustomGPT cascade); arize.com/blog/{jev-as-a-judge,
  decision-model-benchmark,jev-llm-judge-consistency}; blog.agentailor.com AR-001;
  learnjev.com/tutorials/cost-and-benchmarks
- Harness-паттерны: langchain.com/blog/building-a-harness-with-jev;
  theaioperator.io/p/what-is-jev-the-manual-for-agent;
  arthurkatcher.com/blog/how-to-engineer-a-jev-harness;
  jevtypesafeai.com/jev/engineering-for-coding-agents; jevwiki.ai
- Пресса: theregister.com/devops/2026/09/23/shut-up-and-calculate-jevs-new-ai-primitives-for-coders
- oh-my-pi (образец): `/tmp/omp-src/packages/ai/src/judgment/{types,native,systemone}.ts`,
  `packages/coding-agent/src/judgment/{index,cache,standalone}.ts`,
  `auto-thinking/classifier.ts`, `session/unexpected-stop-classifier.ts`,
  `cli/git-tui/ai-stage.ts`, `prompts/system/jevify-notice.md`,
  `docs/{environment-variables,settings,ttsr-injection-lifecycle,models}.md`,
  `crates/pi-natives/src/utok/jev.rs`
- Форк: `.research/jev-fork-integration.md` (детальный дизайн), `crates/{pa-core,pa-ai,
  pa-graph,pa-edit}`, `scripts/allspark-launcher.sh`

## Приложение B — открытые вопросы

1. **RLCD-алгоритм не публичен** — точный reward/data-mixture/architecture не раскрыты;
   независимые работы реконструируют поведение, но не внутренности.
2. **Калибровка на своём домене не гарантирована** — TypeSafe прямо пишет: «test it».
3. **Стоимость каскада не бьёт точность** при коррелированных ошибках судей (Rao, CustomGPT)
   — не строить каскад «вслепую».
4. **Adversarial exposure** зависит от задачи; для risk-gate нужны hard rules + инъекционный
   тест на своём домене.
5. **Latency SLA отсутствует** (TypeSafe: «не в docs, без SLA»); rate limits нестабильны.
