# Creusot и hotpath-rs: разбор и вердикт по применимости к форку allspark

> Дата: 2026-10-17. Метод: shallow-clone обоих репозиториев в `/tmp` + чтение README/guide/docs,
> API GitHub, crates.io, **плюс собственные пробы компиляции** (маленькие cargo-проекты в `/tmp/hptest*`,
> `/tmp/crtest*`, `/tmp/hpbench*`) — все числа ниже измерены на этой машине (12 vCPU, 47 GB,
> rustc 1.99.0 stable), а не перепечатаны из документации.
>
> Проверяемые репозитории: `creusot-rs/creusot` (master, v0.14.0, 2026-10-12),
> `pawurb/hotpath-rs` (main, v0.28.6, 2026-10-08).

---

## 0. TL;DR — вердикт одной таблицей

| Инструмент | Что это | Приоритет для allspark | Усилие | Где реально применим |
|---|---|---|---|---|
| **hotpath-rs** | инструментальный профилировщик (время/аллокации/локи/каналы/tokio) с `#[measure]`, Prometheus, MCP | **ПРИНЯТЬ как эксперимент (opt-in фича `profiling`)**, не в прод-сборку | 1–2 дня на первый профиль | `pa-daemon` (AIMD-очередь, contention), `pa-tui` (рендер), `pa-diff` (Myers), `pa-graph` (SQL) |
| **Creusot** | дедуктивный верификатор Rust (контракты → Why3 → SMT) | **ОТКАЗАТЬ для кодовой базы; допустить точечный эксперимент на 1 чистой функции** | 1 ч установка; 1–3 дня на первую теорему на `varint`; дальше — постоянный налог | `pa-mem::varint` (кодек), `pa-types::framing` (префикс), инвариант AIMD-лимита — и только они |

Коротко почему:

- **hotpath-rs** — зрелый (874k загрузок, 1894★, 70 релизов за 13 месяцев), MIT, ставится одной
  строкой, **ноль цены при выключенной фиче** (проверено: `cargo tree` = 1 пакет, `clippy -D warnings`
  зелёный). Единственный настоящий конфликт с нашей политикой — `unsafe(export_name)` в макросах
  литеральных label'ов при `forbid(unsafe_code)`; обходится (см. §2.5).
- **Creusot** — научный инструмент с реальными ограничениями, каждое из которых бьёт именно по нашим
  кандидатам: **async не поддерживается вообще** (`error: async is not yet supported`), `String`
  в `creusot-std` — непрозрачный тип без `push`/`push_str`, `HashMap`/`BTreeMap` не имеют
  спецификаций мутации (`insert`/`entry`), `vec!`/`format!` с интерполяцией не транслируются.
  То есть `pa-jev` (serde_json + BTreeMap), `pa-graph::cypher` (String/format!), `pa-diff`
  (`Rc<Component>`, `HashMap`) и `pa-daemon` (tokio, `Mutex`, async) — **вне зоны действия**.
  Остаются 2–3 функции, где выигрыш не оправдывает содержание цепочки opam+why3+4 SMT-солвера.

---

## 1. Creusot — дедуктивная верификация Rust

### 1.1. Что это и как работает

Creusot доказывает, что код **всегда** удовлетворяет контракту, для всех входов (не тестами).
Пайплайн:

```
Rust + аннотации  --(rustc wrapper, creusot-rustc)-->  MIR/THIR
      --(перевод)-->  Coma (промежуточный язык Why3)
      --(why3find)-->  VC (verification conditions)
      --(SMT: alt-ergo / z3 / cvc5 / cvc4)-->  proved | unknown | counterexample
```

Ключевая идея (RustHorn): заимствования моделируются парой «текущее значение + пророческое
финальное», поэтому программа переводится в **чисто функциональную** — без separation logic.
Отсюда высокая автоматика SMT на функциональных программах.

**Intrusiveness — важный ответ на вопрос из задачи:** это **не** отдельный язык спецификаций рядом
с файлом. Это **атрибуты прямо на функции** плюс, при необходимости, `ghost!`-код внутри тела:

```rust
use creusot_std::prelude::*;

#[requires(n@ <= 1000u64)]
#[ensures(result@ == n@ * (n@ + 1) / 2)]
fn sum_first_n(n: u64) -> u64 {
    let mut sum = 0;
    let mut i = 0;
    #[invariant(sum@ == i@ * (i@ + 1) / 2)]
    while i < n { i += 1; sum += i; }
    sum
}
```

Три уровня вторжения:

1. **Контракты-атрибуты** (`#[requires]`, `#[ensures]`, `#[invariant]`, `#[variant]`) — на сигнатуре
   и на цикле.
2. **Логические функции** (`#[logic]`, тело в `pearlite!{...}`) — «призрачные» функции, существуют
   только в доказательстве; в них `Int` (математические целые без переполнения), `Seq<T>`, `FMap`,
   `forall`/`exists`, операторы `*` (текущее) и `^` (финальное) для `&mut`.
3. **Ghost-код в теле**: `snapshot!(...)`, `proof_assert!(...)`, `ghost!{...}`, `Ghost<T>`.

`@` — view-оператор: `x@` переводит `u64` в `Int`. Отдельный `#[check(terminates)]` переключает
с partial на total correctness (без него цикл может не завершиться — и это законно).

**Инварианты типов** поддерживаются (`impl Invariant for T`), т.е. можно выразить «этот `struct`
всегда держит такое-то соотношение полей» и Creusot будет требовать сохранения на границах функций.

**Erasure check** (`#[erasure(f)]`) — механизм, закрывающий разрыв «верифицированная функция ≠
скомпилированная»: он синтаксически сверяет тело `f2` (с ghost-кодом) с телом `f` (прод) по модулю
удаления ghost-конструкций и A-normal form. Это, кстати, ровно тот «tie between verified and shipped
code», которого обычно не хватает в таких инструментах — и это самая интересная для нас часть.

### 1.2. Зрелость — по фактам

| Метрика | Значение |
|---|---|
| ★ / форки | 1910 / 98 |
| Контрибьюторы | 53 (ядро: 6 человек, 4600+ коммитов) |
| Открытых issue | 99 |
| Релизы | v0.14.0 (2026-10-12), v0.13.0 (2026-07), v0.12.0 (2026-06) — **~3 релиза в квартал** |
| crates.io `creusot-std` | 62 874 загрузок всего, 49 062 за 90 дней (создан 2026-01) |
| Собственный статус | «**Creusot is still very experimental software, you should expect some obscure crashes and missing features**» (CHANGELOG.md) |
| Тулчейн | `nightly-2026-08-03` + `rustc-dev` + `llvm-tools` — **пин на конкретный nightly** |
| Внешние зависимости | opam/OCaml, why3 1.8.2, why3find 1.3.0, alt-ergo 2.6.4, z3 5.1.0, cvc5 1.4.1 |

Реальные проекты на Creusot (из README): **CreuSAT** (верифицированный SAT-солвер), **Krabka**
(Kafka-брокер, «key safety algorithms»), Sprout. То есть инструмент выходит за пределы «сортировки
массивов» — но все примеры это алгоритмы/структуры данных, не системы с I/O.

Масштаб их собственных доказательств (посчитал по `proof.json` в репозитории):

| Пример | LoC | VC | Время солверов |
|---|---|---|---|
| `red_black_tree.rs` | 821 | 612 | **103 с** |
| `bounded_mpmc_queue/sc.rs` (lock-free MPMC!) | 1142 | 553 | 35 с |
| `doubly_linked_list.rs` (сырые указатели) | 634 | 288 | 10 с |
| `knapsack.rs` | 96 | 14 | 0.3 с |

Вывод: **они умеют** верифицировать lock-free очередь и связный список на указателях. Это сильно.
Но 600 VC и 100 секунд на одну структуру данных — это цена, которую платят за каждую правку.

### 1.3. Ограничения, критичные именно для нас (проверено)

**(а) async не поддерживается — вообще.**

```
$ cat tests/should_fail/bug/2212-b.rs
pub async fn an_async_fn() {}
$ cargo creusot
error: async is not yet supported
 --> 2212-b.rs:3:28
```

В исходниках: `creusot/src/translation/specification.rs:502` →
`crash_and_error(..., "async is not yet supported")`. Для `pa-daemon` (tokio, 1168 тестов),
`pa-agent`, `pa-ai` это закрывает вопрос полностью.

**(б) `String` в creusot-std — почти пустой.** Полный список специфицированных методов
(`creusot-std/src/std/string.rs`): `deref`, `len`, `clone`, `to_owned`, 10× `from_iter`,
`split_at`. **Нет** `push`, `push_str`, `String::new`, `as_bytes`, `chars`, `from_utf8`.
Для `pa-graph::cypher::lex_string` (строит `String` через `out.push(ch)`) и `pa-jev` (serde_json)
это стоп.

**(в) `HashMap`/`BTreeMap` не имеют спецификаций мутации.** `creusot-std/src/std/collections/hash_map.rs`
специфицирует только `View`/`DeepModel` + `iter`/`iter_mut`/`into_iter`/`FromIterator`.
`insert`, `entry`, `or_insert`, `remove` — **отсутствуют**. `BTreeMap` в `creusot-std` нет вообще
(есть только логический `FMap`). Это бьёт по `pa-diff::intern` (`ids.entry(token).or_insert(next)`)
и по всему `pa-jev` (`BTreeMap<String, f64>` в `Answer::Choice/Score`).

**(г) `Vec` — есть `push`/`len`/`index`, но нет `resize_with`/`drain`/`retain`.** `pa-diff::myers_diff`
начинается с `best.resize_with(...)` — не покроется.

**(д) `Rc<T>` есть, но `View` для него — `Box<T>` (непрозрачный),** а `Component{prev: Option<Rc<Self>>}`
— это рекурсивная структура через `Rc`, что упирается в positivity/recursive-types проверку.
`pa-diff` построен ровно на этом.

**(е) `format!` с интерполяцией не транслируется** (issue #2222, закрыт 2026-08-10 как частично
исправленный в #2140): `format!("prefix-{s}")` → `Unsupported constant value`. `pa-graph::cypher`
содержит 121 вхождение `format!`/`String`/`BTreeMap`.

**(ж) Прод-сборка и верификация — это разные сборки.** Проверено: `creusot-std` версии 0.14
компилируется на **stable 1.99 без nightly**, контракты в прод-сборке **не типизируются** —
я специально написал заведомо несуществующие имена в контрактах:

```rust
#[requires(this_is_not_a_function(42))]
#[ensures(result@ == undefined_thing@ + 1)]
pub fn f(x: u32) -> u32 { proof_assert!(also_not_a_thing(x@)); x }
```

`cargo build` — OK, `cargo clippy -D warnings` — OK. **Мусор в контрактах не ловится ничем, кроме
самого Creusot.** Это одновременно плюс (не мешает прод-сборке) и главный риск (контракты гниют
молча, если верификация не в CI).

**(з) `vec!` макрос из std не поддерживается** — нужен `creusot_std::std::vec::vec`, «slightly less
efficient» (аллокация на стеке + move). То есть верифицируемый код ≠ байт-в-байт прод-код.

**(и) Тесты/примеры/бенчи не верифицируются** — «Within a package, Creusot only compiles **libraries
and binaries**. Tests, examples, and benches are currently unsupported» (`guide/src/command_line_interface.md`).
Наши ~6000 атрибутов `#[test]`/`#[tokio::test]` (grep по `crates/*/src`) — вне зоны.

### 1.4. Цена входа и поддержки (измерено/оценено)

| Этап | Что нужно | Реальная оценка |
|---|---|---|
| Установка | `git clone` + `./INSTALL` → локальный opam-switch (`~/.local/share/creusot/_opam`), why3, why3find, 4 солвера | **20–60 мин** машино-времени (в задаче верно сказано: не собирать) |
| Тулчейн | `nightly-2026-08-03` + `rustc-dev` + `llvm-tools`, **отдельный** от нашего `1.98.1`/`1.99.0` | параллельный rustup-toolchain, `RUSTUP_TOOLCHAIN` подставляется `cargo creusot` |
| Первая теорема | контракт + инвариант цикла на простой функции | **часы**; на структуре данных — дни |
| Первая структура данных | как `red_black_tree` | **недели** (у них 821 LoC и 612 VC) |
| Поддержка | обновление при смене nightly (каждые ~3 месяца), пере-доказательство при правке кода | **постоянный налог** |

Что важно: `creusot-std` **не тянет за собой ничего тяжёлого** в прод-сборку — я проверил
`cargo tree`: `creusot-std` → `creusot-std-proc` (proc-macro) → `syn`/`quote`/`proc-macro2`/`uuid`
+ `pearlite-syn`. 19 пакетов, из них новых для нас — 4 (`pearlite-syn`, `uuid`, `getrandom`, `libc`).
Cold build: 0.5 с без vs **4.0 с** с `creusot-std` (цена только в верификационной сборке).
Release-бинарь того же кода: **464 848 байт** с контрактами и **464 848 байт** без;
`nm -C | grep -c creusot` = 0, `strings | grep -c creusot_std` = 0. То есть
**контракты полностью стираются, кода `creusot-std` в бинаре нет** — хорошая новость для
«zero-cost» политики (совпадение размеров — совпадение, значимо здесь отсутствие символов).

### 1.5. Лицензия

`LGPL-2.1-or-later` (`creusot-std`, `creusot-std-proc`, `creusot`, тесты). Сам верификатор
(LGPL) — dev-tool, не распространяется. Но **`creusot-std` линкуется в наш бинарь** в
верификационной сборке.

**Для нашего `deny.toml` это блокер:** allowlist =
`Apache-2.0, Apache-2.0 WITH LLVM-exception, MIT, ISC, BSD-2-Clause, BSD-3-Clause, 0BSD, Zlib,
Unicode-3.0, CDLA-Permissive-2.0, CC0-1.0` — **LGPL там нет**, и `cargo deny check licenses` в CI
(`make deny`) упадёт на любом крейте, который объявит `creusot-std` зависимостью.

Обход: `creusot-std` живёт только в **отдельном, исключённом из workspace** крейте `verif/`
со своим `Cargo.lock` и своим `cargo deny` (проверено: `exclude = ["verif"]` в корневом
`Cargo.toml` → `cargo tree` корня не видит `creusot-std`, а `verif/` собирается отдельно).
LGPL на dev-only артефакт, который никогда не линкуется в `prime-agent`, — юридически защищаемо
(это ровно тот же класс, что LGPL-компилятор), но требует явной записи в `deny.toml` и в
release-политике. Это отдельное решение, не «просто добавить зависимость».

### 1.6. Что у нас **стоит** верифицировать (кандидаты по убыванию отдачи)

Я прогнал реальные типы наших функций через `creusot-std` 0.14 на stable, чтобы понять, что
компилируется и что нет.

#### Кандидат A — `pa-mem::varint` (96 LoC) — **единственный кандидат с чистым «да»**

Кодек `write_u64`/`read_u64` — чистые целочисленные функции над `Vec<u8>`/`&[u8]`, без String,
без HashMap, без Rc, без async. Я аннотировал **реальный код** (не игрушку) и он прошёл
`cargo clippy -- -D warnings` под pedantic + `forbid(unsafe_code)`:

```rust
#[requires(out@.len()@ <= 100)]
#[ensures(result@ >= 1 && result@ <= 10)]
#[ensures((^out)@.len()@ == out@.len()@ + result@)]
pub fn write_u64(out: &mut Vec<u8>, mut value: u64) -> usize { ... }
```

**Что докажет:** каноничность (ровно одна кодировка на значение), инъективность, `1 <= len <= 10`,
`read(write(v)) == v` для всех `u64` (включая `u64::MAX` — 10 байт), и главное —
**отсутствие переполнения сдвига** `u64::from(byte & 0x7f) << (7 * i)` при `i = 9`.

Последнее — не абстракция: я нашёл реальную асимметрию в текущем коде. `read_u64` **молча
отбрасывает** биты выше 63 у 10-го байта:

```
вход [0x80 ×9, 0x7f] → Some((9223372036854775808, 10))   // биты 63..69 потеряны
вход [0x80 ×9, 0x7e] → Some((0, 10))
```

Оба входа дают `Ok`, оба «успешно декодированы», значения разные — но это **не** то значение,
которое закодировал бы `write_u64` (он бы выдал `u64::MAX`). То есть декодер принимает
неканонические 10-байтовые последовательности с усечением. Для формата снапшота, который читает
**свой же** writer, это безвредно; для wire-контракта с внешним читателем — это класс баг-сов
(«decoder accepts a non-canonical encoding and silently truncates»). Creusot-контракт
`#[ensures(result == Ok(v) ==> encode(v) == data[..n])]` поймал бы это сразу.

**Усилие:** 1–2 дня на функцию + инвариант цикла. **Выигрыш:** высокая уверенность в формате,
который читает снапшоты памяти.

#### Кандидат B — `pa-types::daemon::framing` (416 LoC) — **частично**

8-байтовый big-endian префикс (u32 header_len, u32 payload_len) — арифметика чистая. Я проверил:
аннотированный `encode_prefix`/`decode_prefix` **компилируется** под pedantic, но `as u32`
требует `#[allow(clippy::cast_possible_truncation)]` — и это ровно то, что Creusot **докажет**:
`#[requires(header_len@ <= u32::MAX@)]` снимает необходимость в allow.

**Что докажет:** `frame_len = 8 + h + p` не переполняет `usize` (сегодня в `push()` есть
`FRAME_PREFIX_BYTES + header_len + payload_len` — при `h`/`p` близких к `u32::MAX` на 32-битной
цели это переполнение; на 64-битной безопасно, но **не доказано**), и что `decode(encode(h,p)) == (h,p)`.

**Границы:** сам `PrivateFrameDecoder::push` работает с `serde_json::Value` — **вне Creusot**.
Верифицируется только арифметика префикса, т.е. ~40 строк из 416.

#### Кандидат C — инвариант AIMD-лимита (`pa-daemon/src/agent_engine/provider_admission.rs`) — **как спецификация, не как доказательство**

Логика AIMD — идеальный кандидат *по смыслу*: чистые переходы `limit' = min(limit+1, cap)` /
`limit' = max(limit/2, floor)` и инвариант `floor <= limit <= cap` при любых исходах. Creusot
такое доказывает легко (`examples/bounded_mpmc_queue` — 553 VC на lock-free очередь).

**Но:** сам `ProviderAdmission` — это `Mutex<RegistryState>` + `HashMap<ProviderKey, ProviderState>`
+ `VecDeque<Waiter>` + `oneshot`-каналы, т.е. `HashMap` (нет спеков мутации) + tokio (нет async).
Верифицировать можно **только вынесенную чистую функцию перехода**, если её сначала вынести:

```rust
#[ensures(result@ >= floor@ && result@ <= cap@)]
fn aimd_next(limit: usize, cap: usize, floor: usize, outcome: Outcome) -> usize
```

Сегодня этой функции нет — логика вкомпилирована в `settle()` под мьютексом (строки 803–850).
**Выигрыш от Creusot тут ниже, чем выигрыш от простого вынесения функции + proptest** (у нас
`proptest` уже в workspace, 8 использований). Creusot дал бы «для всех исходов», proptest — «для
10 000 случайных». Для 5-строчной арифметики разница не окупает opam.

#### Кандидат D — `pa-diff::myers_diff` — **отказ**

Заманчиво (Myers — это алгоритм, ровно профиль Creusot), но: `Rc<Component>` (непрозрачный `View`),
`HashMap` в `intern`, `resize_with`, `is_some_and`, `expect`. Плюс `unreachable!("Myers diff
terminates...")` — Creusot потребует доказать termination, что для O(ND) с двумя границами
диагоналей нетривиально. Оценка: **недели** и переписывание структуры. Отказ.

#### Кандидат E — `pa-graph::cypher` lexer — **отказ**

`lex()` — 130 строк на `&str`/`u8`/`Vec<Tok>`, выглядит идеально. Но `Tok::Ident(String)` и
`Tok::Str(String)`: `String::new()` и `out.push(ch)` **не имеют спецификаций** в `creusot-std`
(см. §1.3б). Плюс 121 вхождение `format!`/`String` в файле. Отказ без форка `creusot-std`.

### 1.7. Итог по Creusot

**ОТКАЗАТЬ** как инструмент кодовой базы. Причины в порядке весомости:

1. **async не поддерживается** → 3 из 6 крупнейших крейтов (`pa-daemon`, `pa-agent`, `pa-ai` —
   3000+ тестов) недоступны в принципе.
2. **`String`/`HashMap` без спеков мутации** → `pa-jev`, `pa-graph`, `pa-diff` недоступны.
3. **LGPL-2.1 в `deny.toml` allowlist** → требует отдельного excluded-крейта и юридической записи.
4. **Пин на nightly + opam + 4 SMT-солвера** → постоянный налог на 3–4 релиза в квартал.
5. **Соотношение цена/выигрыш** на оставшихся 2–3 функциях (~150 строк) не в нашу пользу.

**Допустимый эксперимент (2–3 дня, не более):** один `verif/`-крейт (excluded из workspace,
свой lockfile, свой `cargo deny`), в нём — `pa-mem::varint` с контрактами каноничности и
инъективности. Критерий успеха: `cargo creusot` доказывает все VC за < 60 с и ловит
неканонический-10-байтовый случай как контрпример. Если да — можно обсуждать расширение;
если нет — тема закрыта с данными, а не с мнением.

Отдельно стоит **украсть идею**, а не инструмент: `#[erasure(f)]` — синтаксическая проверка
«верифицированная функция ведёт себя как скомпилированная» — решает реальную нашу проблему
(контракты, которые никто не проверяет, гниют). Это дешёвая идея для QA-сьюта.

---

## 2. hotpath-rs — профилировщик

### 2.1. Что это

**Инструментальный** профилировщик (не сэмплирующий): макросы вставляют в код замеры, фоновый
поток собирает события, на выходе — таблица или JSON, плюс живой TUI, Prometheus `/metrics`
и MCP-сервер для LLM-агентов.

Что умеет (по фичам):

| Ресурс | Макрос | Что меряет |
|---|---|---|
| функции | `#[hotpath::measure]`, `measure_all` | время, аллокации (байты+кол-во), CPU-сэмплы |
| блоки | `measure_block!` | то же для произвольного выражения |
| локи | `mutex!`, `rw_lock!` | wait (ждал лок) и acquire (держал) |
| каналы | `channel!` | throughput, send→receive latency, max queue depth |
| futures/streams | `future!`, `stream!` | polls, время полла, items |
| SQL | `sqlx_tracing_layer()`, `instrument_diesel_sql()` | по нормализованному запросу + атрибуция к функции |
| HTTP клиент | `http!(client)` | по endpoint'у (reqwest/ureq) |
| HTTP сервер | `axum!(router)` | по роуту (axum) |
| I/O | `io!(stream)` | байты, throughput, P95 латентность |
| Tokio runtime | `tokio_runtime!()` | воркеры, steal-счётчики, глубины очередей |
| произвольные значения | `dbg!`, `val!`, `gauge!` | — |

### 2.2. Как работает (и почему это дёшево)

Из `docs/src/profiling_overhead.md` + чтение исходников:

- **Zero cost when disabled**: все макросы — no-op, все зависимости `optional`. Проверено:
  `cargo tree` без фичи = **1 пакет** (сам крейт), `cargo clippy -D warnings` зелёный.
- **Lock-free транспорт**: per-thread chunked SPSC очереди (`hotpath-drain`), запись события =
  запись в слот + один `Release` store. Фоновый воркер — единственный консьюмер, обходит очереди
  каждые 50 мс.
- **Быстрые часы**: собственный `Instant` (`quanta` на Linux / `mach_absolute_time` на macOS):
  6.1 нс против 17.2 нс у `std::time::Instant`.
- **Time sampling**: `HOTPATH_TIME_SAMPLING_RATE=0.01` — замерять время только у 1% вызовов
  (счётчики остаются точными).

### 2.3. Чем отличается от criterion / flamegraph / hyperfine

| | criterion | flamegraph / samply / perf | hyperfine | **hotpath** |
|---|---|---|---|---|
| Вопрос | «стало ли быстрее?» | «где горит CPU?» | «какая команда быстрее?» | **«почему система медленная?»** |
| Метод | статистические прогоны, микро-бенчи | сэмплирование стека (perf events) | N прогонов процесса | **инструментальные замеры в живом процессе** |
| Единица | функция в бенче | стек-фрейм | процесс | **функция/лок/канал/запрос в проде** |
| Видит I/O-ожидание | нет | нет (стек спит — не горит) | нет | **да** (wall-time вокруг `await`) |
| Долгоживущий процесс | нет | да | нет | **да** (TUI, Prometheus) |
| Цена | ноль в проде (отдельный таргет) | ~1–3% CPU | ноль | **~180–250 нс/вызов** (измерено) |
| CI-гейт | нет (только вручную) | нет | нет | **да (hotpath Cloud, платный)** |

Ключевое: criterion отвечает «быстрее ли», hotpath — «где болит». Они не конкуренты, а разные слои.
Сэмплирующий профиль не отличает `sleep(10ms)` от 10 мс счёта — hotpath отличает (это его главный
аргумент, и он верный).

### 2.4. Зрелость

| Метрика | Значение |
|---|---|
| ★ / форки | 1894 / 61 |
| crates.io | **874 674** загрузок, **633 600** за 90 дней |
| Релизы | 70 за 13 месяцев; v0.28.6 (2026-10-08) |
| Контрибьюторы | 8 (1 основной — 1205 коммитов) |
| CI | ubuntu/macos/windows × Rust 1.89/stable/nightly, miri, 36 тест-крейтов |
| Статус API | «Core public APIs are stable, but implementation details (JSON report formats, TUI/MCP internals...) may change» |
| Модель | OSS + платный hosted «hotpath Cloud» (PR-комментарии, baseline-сравнение) |

**Bus factor = 1.** Это главный риск. Смягчается тем, что это dev-tool: если он сломается,
мы просто теряем профилировщик, а не продукт.

### 2.5. Совместимость с нашей политикой (проверено компиляцией)

Это самое важное, потому что у нас `unsafe_code = "forbid"` в `[workspace.lints.rust]` и
`-D warnings` в CI.

**Что работает без единой правки:**

```toml
[dependencies]
hotpath = { version = "0.28", optional = true }

[features]
profiling = ["dep:hotpath", "hotpath/hotpath", "hotpath/hotpath-alloc"]
```

| Проба | Результат |
|---|---|
| `#[hotpath::measure]` на синхронной функции | ✅ |
| `#[hotpath::measure]` на `async fn` (в т.ч. `Send` + `tokio::spawn`) | ✅ |
| `#[hotpath::measure(impl_type = "Worker")]` на методе | ✅ |
| `#[hotpath::measure(future = true)]` на async | ✅ (синтаксис именно `future = true`, не `future`) |
| `#[tokio::main]` + `#[hotpath::main]` (порядок: tokio сверху) | ✅ |
| `HotpathGuardBuilder` в библиотеке (без `#[main]`) | ✅ |
| `hotpath::mutex!` / `rw_lock!` / `measure_block!` | ✅ |
| `#[hotpath::measure]` + `#[tokio::test]` | ✅ |
| `cargo clippy --all-targets --all-features -- -D warnings` | ✅ зелёный |
| `hotpath-prometheus` → `:6772/metrics` | ✅ |
| `hotpath-mcp` → `:6771/mcp` (JSON-RPC `initialize` отвечает) | ✅ |
| фича выключена → `cargo tree` = 1 пакет | ✅ |

**Единственный конфликт — `unsafe(export_name)` в литеральных label'ах.**

Макросы `#[hotpath::measure(label = "...")]`, `measure_block!("...")`, `mutex!(..., label = "...")`
для проверки уникальности label'а генерируют:

```rust
#[unsafe(export_name = "__hotpath_unique_...")] static __HOTPATH_UNIQUE_LABEL: u8 = 0;
```

Под `forbid(unsafe_code)` это **жёсткая ошибка**:

```
error: usage of the unsafe `export_name` attribute
  = note: requested on the command line with `-F unsafe-code`
```

Причём `forbid` нельзя обойти локальным `#[allow(unsafe_code)]`:

```
error[E0453]: allow(unsafe_code) incompatible with previous forbid
```

**Обходы (проверены):**

1. **Не использовать литеральные label'ы.** Без `label = "..."` статик не генерируется.
   Вместо `label = "daemon.parse"` — runtime-выражение (`label = format!(...)`) либо
   `measure_block!` с runtime-строкой. Проверено: работает, `-D warnings` зелёный.
2. **Дать `unsafe_code = "deny"` вместо `forbid`** на крейт с профилированием и точечный
   `#[allow(unsafe_code)]` вокруг литерального label'а. Проверено: компилируется и печатает
   `| lab | ... |`, `| blk | ... |`. Но это ослабление политики — **не рекомендую**.
3. **Патч макроса** (убрать `export_name`, заменить на `#[used]`/`const`-трюк) — форк hotpath,
   не нужно.

Рекомендация: **вариант 1** — правило «в коде allspark label'ы только runtime-выражения»,
зафиксировать в `AGENTS.md` рядом с профилированием. Это дешевле, чем ослаблять `forbid`.

**`deny.toml`:** прогон `cargo metadata` по набору `hotpath + hotpath-alloc + hotpath-prometheus
+ tokio + parking_lot` даёт 72 пакета; всё уже в нашем allowlist **кроме** `hdrhistogram` и
`winapi*` — и те с legacy-форматом `MIT/Apache-2.0`, который `cargo-deny` нормально разбирает
(у нас в графе уже 22 таких же: `bitflags`, `walkdir`, `signal-hook`...). Новых лицензий нет.
`--all-features` тянет 142 пакета (`axum`, `hyper`, `rmcp`, `schemars`) — поэтому в CI
**нельзя** гонять `cargo deny --all-features` на крейте с профилированием, либо нужно
ограничить фичи. Это надо учесть: у нас `make deny` использует `--all-features`.

### 2.6. Реальная цена (измерено, не из доков)

Документация обещает ~40 нс/вызов. **На моей машине получилось ~180–250 нс** — в 5 раз больше.
Методика: `lto = "thin"`, `codegen-units = 4` (как наш `[profile.release]`), медиана из 21 прогона,
инструментированная и «голая» функция в одном бинаре, обе `#[inline(never)]`, тело — 3 итерации
цикла с `black_box`:

```
n=0     plain=  2.7ns  inst= 220.3ns   delta= 217.6ns     (по умолчанию)
n=0     plain=  2.7ns  inst= 199.2ns   delta= 196.5ns     (+ HOTPATH_METRICS_SERVER_OFF=1)
n=0     plain=  2.7ns  inst= 170.9ns   delta= 168.2ns     (+ sampling 0.0)
n=0     plain=  2.8ns  inst= 162.1ns   delta= 159.3ns     (+ sampling 0.01)
```

Их собственный бенч (`cargo run --example benchmark_noop`) на этой машине:
`baseline 1195 ns/op, instrumented 1478 ns/op (+283 ns/op)` — при теле ~1 µs. То есть **их же
цифры на их же бенче дают +283 нс**, а в доке написано +38.6 нс. Расхождение — вопрос методики
(у них тело 1 µs и один прогон, у меня медиана и микро-тело).

**Практический вывод:** считать ~200 нс на вызов. Это значит:

- функция с телом **> 10 µs** → накладные < 2%, шум. Инструментируй смело.
- функция с телом **1–10 µs** → 2–20%. Инструментируй выборочно.
- функция с телом **< 1 µs** в горячем цикле → **нельзя** (200 нс на 100 нс работы = 3×).
  Для таких — `HOTPATH_FUNCTIONS_TIME_SAMPLING_RATE=0.01` (падает до ~160 нс, но точность
  длительностей становится статистической; счётчики остаются точными).

Отдельно: `HOTPATH_FOCUS="regex"` отключает незаматченные функции практически в ноль
(**25 нс** вместо 312 нс в моей пробе) — это правильный режим для точечного профилирования.

### 2.7. Лицензия

**MIT** — конфликтов с `deny.toml` нет.

### 2.8. Что мерить у нас (конкретика)

#### Точка 1 — `pa-daemon`: AIMD-очередь и contention (наивысший приоритет)

`crates/pa-daemon/src/agent_engine/provider_admission.rs` (1481 LoC) — один `Mutex<RegistryState>`
на все `(provider, model)`, FIFO-очередь `VecDeque<Waiter>`, `oneshot`-гранты, брейкеры.
Всё, что hotpath умеет и criterion не умеет:

```rust
// 1. Замерять полный путь admit→stream→settle
#[hotpath::measure(impl_type = "ProviderAdmission")]
pub async fn acquire(&self, key: &ProviderKey, holder: &str) -> Lease { ... }

// 2. contention на реестре (wait vs hold — ровно то, что нужно для AIMD)
let state = hotpath::mutex!(std::sync::Mutex::new(RegistryState::default()),
                            label = concat!("admission.", "registry"));
```

Что покажет: **сколько времени воркеры стоят в очереди** (wait avg/P95) vs держат слот (acquire).
Гипотеза, которую это проверит: при `limit=6` и 9 субагентах (инцидент из шапки модуля)
wait-время доминирует — то есть «медленно» не потому что провайдер тормозит, а потому что мы
сериализуемся на своём же лимите. Это ровно тот вопрос, на который criterion не отвечает.

#### Точка 2 — `pa-daemon`: каналы supervisor↔worker

`backpressure.rs` уже содержит точные константы (`WORKER_INFLIGHT_CAPACITY=128`,
`CLIENT_OUTBOUND_CAPACITY=32*1024`, `EVENT_RING_CAPACITY=4096`). `hotpath::channel!` даёт
**max queue depth** и send→receive latency по каждому каналу — то есть прямое измерение того,
правильно ли выбраны эти константы, вместо рассуждений.

#### Точка 3 — `pa-diff`: Myers (только через `measure_block!`, не `#[measure]`)

`myers_diff` в горячем цикле редактора. Функция целиком — O(ND), внутри цикл по диагоналям.
`#[hotpath::measure]` на неё даст 200 нс накладных на вызов (вызов нечастый — нормально), но
лучше обернуть **весь путь** `diff_lines_*` через `measure_block!`, чтобы увидеть соотношение
`intern` vs `myers_diff` vs `structured_patch_hunks` — это ответ на «что оптимизировать».

#### Точка 4 — `pa-tui`: рендер

У нас ~1490 тестовых атрибутов в `pa-tui`, и `[profile.release]` специально настроен на «cross-crate inlining
in the hot render path». `hotpath::measure` на функции рендера + `hotpath::rw_lock!` на кэшах
даст картину «сколько рендерим vs ждём локи». Плюс `tokio_runtime!()` — если TUI использует tokio.

#### Точка 5 — `pa-graph`: SQL

`rusqlite` (не sqlx/diesel) — значит `sql_tracing` не применим, но `measure_block!` вокруг
`CypherEngine::query` и `GraphStore` операций даст per-query тайминги с атрибуцией к функции.

#### Что **не** стоит делать

- `hotpath-alloc` на всём воркспейсе: он ставит `#[global_allocator]`, а у нас в `pa-core`/`pa-cli`
  свои аллокаторы/`unsafe`-исключения. Включать только точечно и только в отдельном
  профилировочном бинаре.
- `hotpath::main` в прод-бинаре `prime-agent`: guard печатает отчёт в **stdout** при выходе
  (проверено: отчёт идёт в stdout, не stderr), что сломает любой `--json`/pipe-контракт CLI.
  Только `HotpathGuardBuilder` + `Format::None` + `HOTPATH_OUTPUT_PATH`.
- Два guard'а одновременно: **паника** `More than one _hotpath guard cannot be alive at the same
  time` (проверено). Это значит: в тестах guard'ы нельзя (параллельные тесты = паника);
  в `cargo test` работает только `#[measure]` без guard'а.

### 2.9. Итог по hotpath-rs

**ПРИНЯТЬ как эксперимент.** Конкретно:

1. Добавить `hotpath` как `optional` зависимость в `pa-daemon` (и, если пойдёт, `pa-tui`),
   фича `profiling`, в CI **не включать** (или включать отдельной job).
2. Правило в `AGENTS.md`: label'ы — только runtime-выражения (иначе `forbid(unsafe_code)`).
3. Первый замер: `provider_admission` (mutex wait/hold + полный admit→settle) и каналы
   supervisor↔worker. Это ответит на вопрос, который сейчас решается рассуждениями.
4. `deny.toml`: убедиться, что `--all-features` не подтягивает 142 пакета на крейт с профилированием.
5. **Не** брать hotpath Cloud (платный, внешний сервис, OIDC-токены в CI). Свой CI-гейт у нас
   уже есть — `pa-perf` + `.github/workflows/perf.yml` с baseline и порогом >2x.

---

## 3. Сравнение по нашим критериям

| Критерий | Creusot | hotpath-rs |
|---|---|---|
| Надёжность протоколов (wire-кодеки) | **теоретически идеален**, практически — только на 2 функциях | не про это (измеряет, не доказывает) |
| Параллелизм (tokio) | **не поддерживает async вообще** | **сильная сторона** (локи, каналы, futures, tokio runtime) |
| Zero-warning политика | ✅ контракты не типизируются в прод-сборке; `clippy -D warnings` зелёный | ✅ зелёный, **кроме** `unsafe(export_name)` при `forbid` |
| `forbid(unsafe_code)` | ✅ (creusot-std сам компилируется, но линкует `uuid`/`getrandom`) | ⚠️ макросы генерируют `#[unsafe(export_name)]` для литеральных label'ов |
| 3300+ тестов / QA-сьют | не верифицирует тесты/примеры/бенчи | работает **внутри** тестов (`#[measure]`), guard — нет |
| Ноль цены в проде | ✅ контракты стираются, бинарь байт-в-байт | ✅ фича off = 1 пакет в дереве |
| Лицензия | ❌ LGPL-2.1 (нет в allowlist) | ✅ MIT |
| Усилие входа | 20–60 мин установка + недели на теоремы | **~1 час** до первого отчёта |
| Bus factor | 6 контрибьюторов, 53 всего, академический | **1** контрибьютор (1894★) |
| Совместимость с нашим тулчейном | ❌ требует `nightly-2026-08-03` + `rustc-dev` | ✅ stable 1.89+ |

---

## 4. Рекомендация (приоритеты)

### Принять: hotpath-rs (эксперимент, 1–2 дня)

Цель: получить **данные** о том, где `pa-daemon` теряет время в admission-пути, вместо гипотез.
Критерий успеха: отчёт с wait/hold по `provider_admission` и queue depth по каналам, на котором
видно, доминирует ли ожидание слота. Если да — это вход в оптимизацию AIMD (например, отдельный
лимит на spawn vs stream, или неблокирующий fast-path).

### Эксперимент (ограниченный, 2–3 дня): Creusot на `pa-mem::varint`

Цель: проверить на **одной** реальной функции, окупается ли содержание цепочки. Критерий успеха:
`cargo creusot` доказывает каноничность+инъективность `write_u64`/`read_u64` и находит
неканонический 10-байтовый вход как контрпример к `#[ensures(... encode(v) == data[..n])]`.
Изолировать в `verif/` (excluded из workspace, свой lockfile, свой `cargo deny` с LGPL).

### Отказать: Creusot как инструмент кодовой базы

Не потому что плох, а потому что: (1) async нет, (2) `String`/`HashMap` без мутации,
(3) LGPL vs наш allowlist, (4) пин на nightly при 3–4 релизах в квартал, (5) на оставшихся
~150 строках выигрыш не покрывает налог.

### Украсть идею (бесплатно, для QA-сьюта)

`#[erasure(f)]` — синтаксическая сверка «верифицированная версия ведёт себя как прод».
Наш QA-сьют (`qa-tasks/`) проверяет поведение чёрным ящиком; erasure-проверка — это тот же
принцип, но на уровне AST: «код под контрактом и код в проде — один и тот же код». Для нас это
актуально, если мы когда-нибудь заведём контракты (даже в виде debug-assertions).

---

## Приложение A: точные команды для воспроизведения

```bash
# Creusot: shallow clone + guide (без сборки)
git clone --depth 1 https://github.com/creusot-rs/creusot /tmp/creusot
cat /tmp/creusot/guide/src/limitations.md
cat /tmp/creusot/rust-toolchain          # nightly-2026-08-03
python3 -c "import json;d=json.load(open('/tmp/creusot/examples/red_black_tree/proof.json'));print(d)" | head

# hotpath: shallow clone + overhead benchmark
git clone --depth 1 https://github.com/pawurb/hotpath-rs /tmp/hotpath-rs
cd /tmp/hotpath-rs && cargo run --example benchmark_noop --features hotpath --release
```

## Приложение B: ключевые файлы, на которые опираются выводы

| Утверждение | Источник |
|---|---|
| async не поддерживается | `/tmp/creusot/tests/should_fail/bug/2212-b.stderr`, `creusot/src/translation/specification.rs:502` |
| `String` без `push`/`push_str` | `/tmp/creusot/creusot-std/src/std/string.rs` (полный список `fn` — 15) |
| `HashMap` без `insert`/`entry` | `/tmp/creusot/creusot-std/src/std/collections/hash_map.rs` |
| тесты/бенчи не верифицируются | `/tmp/creusot/guide/src/command_line_interface.md:55` |
| `vec!` не поддерживается | `/tmp/creusot/guide/src/limitations.md` |
| «very experimental software» | `/tmp/creusot/CHANGELOG.md` (шапка) |
| VC и время по примерам | `examples/*/proof.json` (посчитано скриптом) |
| ~40 нс vs ~200 нс overhead | `/tmp/hotpath-rs/docs/src/profiling_overhead.md` + мои `/tmp/hpbench*` |
| `unsafe(export_name)` в label'ах | `/tmp/hotpath-rs/crates/hotpath-macros/src/lib_on.rs:512` |
| guard паникует при втором экземпляре | `/tmp/hotpath-rs/crates/hotpath/src/lib_on/hotpath_guard.rs:652` |
| отчёт в stdout | проба `/tmp/hptest8` |
| неканонический 10-байтовый varint | проба `/tmp/vt` на коде `crates/pa-mem/src/varint.rs` |
