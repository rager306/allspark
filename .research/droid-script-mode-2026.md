# Droid (Factory) 1.15.0: `Script`-режим и коррекция выбора инструментов

**Статус:** исследование. Третий срез к `omp-tool-steering-2026.md` и
`commandcode-droid-tool-calling-2026.md`.
**Источник:** локально установленный бинарь `droid` 1.15.0 (`~/.local/bin/droid`,
348 MB), разобран через `strings`. Цитаты — точные подстроки бандла, но структура
восстановлена по строкам, а не по исходникам.
**Вопрос:** как Droid заставляет агента выбирать правильный инструмент, и что из
этого применимо к нашему RLM REPL.

## 1. Три режима исполнения, переключаемые **по активной модели**

Центральная функция (минифицирована, но читается):

```js
function Nw(t) {
  if (t.override !== void 0) return t.override;
  let n = po(G7)?.models,
      o = De(t.modelId) ? AD(t.modelId, E().getCustomModels()) : t.modelId;
  return n?.some(s => s === t.modelId || s === o) ? "script_only" : "direct_only";
}
var v = {
  direct_only: "The active model changed. Script is no longer available; call tools
                directly from now on. Earlier Script runs are shown as the direct
                tool calls they made.",
  script_only: "The active model changed. Most tools are now available only inside
                Script; call them through Script from now on, and call the remaining
                top-level tools directly. Earlier direct tool calls remain in the
                conversation as history."
};
```

Это **буквально** ответ на вопрос «коррекция поведения под конкретное семейство
моделей»: набор доступных инструментов и способ их вызова выбираются по
`modelId` из конфигурации (`models` — список моделей, для которых включён
`script_only`). Модель, сменившаяся в середине сессии, получает **объявление**
нового режима, а не молчаливый отказ.

## 2. Батч-коррекция после неверных прямых вызовов

```js
function d6H(t) {
  let n = Xc(t), o = n.join(", "), s = n.length === 1 ? "That call" : "Those calls";
  return `You called ${o} directly. ${s} ran this time, but in this session ${o} and
          every other tool outside your top-level tool list belong inside Script.
          Call them through Script from now on, as \`tools.<path>(input)\`.`;
}
```

Три свойства, которых у нашего арбитра нет:

1. **Одна заметка на ход, а не на вызов.** Место вызова:
   `if (L.size > 0) Y.push(Xe(d6H([...L])))` — `L` копит **имена** всех
   инструментов, вызванных напрямую за ход, и в конце хода отправляется один
   текст со списком. 106 повторов одной подсказки (наш замер #45) у них
   структурно невозможны.
2. **Заметка уходит отдельным сообщением** (`llmOnlyReminders`, `role: "user"`,
   `visibility: "llm_only"`), а не приклеивается к tool-результату. То есть она
   не платит за себя в каждом tool-результате до компакции.
3. **Коррекция называет способ и путь**: «call them through Script from now on,
   as `tools.<path>(input)`». Не «возьми другой инструмент», а «вызови этот же
   правильно».

## 3. `ToolSearch` — загрузка схем по требованию

```
Use the three tools below; load them with ToolSearch
(`select:ReadAutomationFiles,EditAutomationFiles,SetAutomationRunOutcome`) if they are …
```

```
ToolSearch` inside a Script: pass query "select:<name>[,<name>...]" with the exact
names listed below, or pass keywords (e.g. "linear issue") that match words in tool
names and auto-load the …
```

```
Call them directly; ToolSearch only loads tools missing from your list.
```

`select:` принимает точные имена; ключевые слова — нечёткий поиск. Это тот же
приём, что Anthropic tool search (8.7k токенов против 77k), и он же в
CommandCode (`supportsDeferredTools`).

## 4. Доктрина «путь вместо имени»

```
inside Script. Call them through Script from now on, as `tools.<path>(input)`.
A path can differ from the listed name; for example, `server__tool` becomes
`tools.server.tool`. … MCP tools bind under `tools.mcp.<server>` and connector
tools under `tools.app.<app>`.
import and export are not available inside a Script: there are no modules.
Use tools.<path> and the Script globals.
```

Инструменты адресуются **путём**, и путь не совпадает с wire-именем: `server__tool`
→ `tools.server.tool`. Это структурное решение против смутных имён, о котором
предупреждает Anthropic («overlapping/vague tools confuse agents»).

## 5. Бюджеты: что ограничено, а что нет

Есть — `ds = 2`:

```js
class P9 {
  correctionRequests = 0;
  requestCorrection() { return this.correctionRequests += 1, this.correctionRequests <= ds; }
  hasExceededCorrectionBudget() { return this.correctionRequests > ds; }
}
```

Это бюджет **structured output**, не выбора инструментов. Для коррекции
инструментов бюджета в бандле не нашлось: защита там структурная — одна
заметка на ход.

## 6. Сравнение трёх продуктов

| | Droid 1.15.0 | CommandCode 1.91.1 | наш fork |
|---|---|---|---|
| переключение режима по модели | **да**, `script_only`/`direct_only` по `modelId` | `behaviorProfile` (флаги провода) | нет |
| коррекция способа | **да**, батч на ход, отдельным llm-сообщением | `scriptFirstSteer`, на вызов | арбитр, на вызов, в tool-результате |
| загрузка схем по требованию | `ToolSearch` + `select:` | `supportsDeferredTools` | MCP только |
| адресация инструментов | путь `tools.<path>` | плоские имена + retired-редирект | плоские имена |
| бюджет повторов | структурно (1 на ход) | cap в oh-my-pi, у них — нет данных | **3 на правило** (#45) |

## 7. Что это меняет для нас

1. **Наш кап #45 — правильное направление, но не тот механизм.** `3 на правило`
   всё ещё позволяет три раза сказать одно и то же в разных вызовах. Их единица
   — **ход**: один батч-текст на все прямые вызовы хода.
2. **Место имеет значение.** Заметка в tool-результате оплачивается в каждом
   последующем запросе; отдельное `llm_only`-сообщение — один раз.
3. **Переключение по модели — не текст, а конфигурация.** `script_only` — это
   список моделей в конфиге, который решает, какой набор инструментов и какой
   способ вызова доступны. У нас `graph`/`lsp` всегда прямые; `python_repl`
   всегда доступен, и модель выбирает сама.
4. **Путь вместо имени** — дёшево и проверяемо: `tools.<path>` вместо плоских
   имён, где путь выражает семейство и источник.

## 8. Чего в этом исследовании нет

- **Исходников нет**: только `strings` по 348 MB бандла. Цитаты точные, объём
  контекста вокруг них — нет; в бандле много соседних строк из разных модулей.
- **Поведение не подтверждено прогоном**: `droid` требует авторизации Factory,
  которую здесь не настраивали. Всё выше — чтение кода, не наблюдение.
- **A/B нет**: ни один пункт §7 не проверен на нашем дереве.
