# gsd-pi: безопасный захват ключей через tiu (без LLM)

Источник: /usr/lib/node_modules/@opengsd/gsd-pi@1.22.0 — src/resources/extensions/get-secrets-from-user.ts (609 строк, MIT © 2026 Open GSD). Агент-исследователь умер на коннект-ошибках; отчёт восстановлен ручным чтением кода.

## Механизм (полный поток)

1. **Манифест**: SECRETS-файл на майлстоун объявляет ключи (key, hint, guidance — шаги «где взять ключ»); parseSecretsManifest.
2. **Постраничный masked-ввод** (collectOneSecret): ctx.ui.custom() — свой Editor, где КАЖДАЯ строка рендерится через maskEditorLine() (значение не отображается), Preview = first4+***+last4 (maskPreview); enter=подтвердить, esc/ctrl+s=скипить. Fallback для RPC-поверхностей: ctx.ui.input(..., {secure:true}).
3. **Без-LLM гарантия** (как заэнфоржено):
   - значение живёт ТОЛЬКО в замыкании виджета → applySecrets → хранилище;
   - tool-результат агенту содержит ТОЛЬКО имена: строки «✓ KEY: applied / • KEY: skipped», details {destination, applied[], skipped[]} — значений нет ни в content, ни в details;
   - renderCall/renderResult (TUI-транскрипт) тоже только имена/статусы.
4. **Хранилища**: dotenv (.env — writeEnvKey с эскейпингом и replace-in-place), vercel env add / convex env set через pi.exec; после записи — hydrateProcessEnv (process.env сразу виден расширениям).
5. **UX-гигиена**: summary-экран статусов (done/pending/skipped + «already set» через checkExistingEnvKeys), страница = один ключ, guidance-шаги с wrap, hasUI=false → отказ («interactive mode required»).

## Слабости (не переносить)
- vercel-путь: секрет через `sh -c printf %s ... | vercel env add` — торчит в argv дочернего процесса (ps-экспозиция);
- .env — плоский текст, без chmod-ужесточения;
- НЕТ валидации живости ключа при вводе (наш /login валидирует) — просто запись.

## Перенос в наш форк (паттерны; код MIT, можно с атрибуцией)

Топ-3:
1. **[P1, M] secrets-manifest + masked-захват в pa-tui**: форковая команда/панель `/secrets` (или native-тул `secure_env_collect` с гейтом): проект кладёт манифест {key, hint, guidance[]}; TUI — постраничный masked-Editor (у нас есть Editor-виджеты в /login); результат в чат/транскрипт — только имена. Хранилище: auth.json (для провайдеров) / .env (для инструментов) + сразу в env воркера.
2. **[P0, S] имя-only инвариант для секретных тулов**: правило «tool-результат секретного тула не несёт значений» — у нас уже частично (login-панель), формализовать как паттерн + тест на отсутствие секретов в content/details (trace-инвариант из blackbox-qa).
3. **[P1, S] env-гигиена рядом**: hydrateProcessEnv-паттерн у нас уже есть (env воркера), но НАША дыра #39 (полный env баш-детям) превращает «не участвует LLM» в «участвует любая bash-команда» — секреты захваченные так нужно scrub'ить из child-env (работа уже запланирована в #39-fix).

## Связь с фичами дня
- [[qa-trace-invariants-pattern]]: «секретов нет в транскрипте» — тривиальный trace-инвариант для нового сьюта.
- jev/arbiter: модель может ПРОСИТЬ захват ключа (choice «какой ключ нужен»), но не читать его.
