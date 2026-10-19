# pogocache (tidwall/pogocache): приёмы памяти и процессора

Репо: https://github.com/tidwall/pogocache (MIT, Polypoint Labs, 2025)
Автор: Josh Baker (tidwall) — shardmap, buntdb, tile38, evio, th64, hashmap.c.
Язык: **C11**. Цель: минимальная латентность и **наименьшее число CPU-циклов на запрос**.
Бенчмарк (8 threads, c8g.8xlarge): **3 145 960 throughput / 111μs / 6 968 циклов** —
быстрее memcache (2.6M/191μs/13400), redis (909k/447μs/11799).

Embeddable: `pogocache.c` компилируется внутрь → **100M+ ops/sec** без сети.

## Обзор src/ (22 .c)
| Файл | LOC | Назначение |
|---|---|---|
| cmds.c | 71.5K | команды всех протоколов |
| pogocache.c | 64.3K | **движок кэша** (embeddable) |
| lz4.c | 115.6K | сжатие (persistence) |
| postgres.c | 42.8K | Postgres wire |
| net.c | 38.6K | event loops (epoll/kqueue) |
| hashmap.c | 36.4K | Robin Hood hashmap (tidwall) |
| main.c | 24.7K | CLI |
| save.c | 17.3K | персистентность (LZ4) |
| memcache.c | 15.2K | Memcache wire |
| conn.c | 13.6K | буферы соединений |
| util.c | 12.5K | varint, clamp, crc32, u64toa |
| http.c | 10.7K | HTTP wire |
| xmalloc.c | 3.7K | аллокатор-обёртка |
| resp.c | 6.0K | RESP wire |

---

## ПРИЁМ №1: 6-байтные указатели (48-bit) ⭐⭐⭐
`pogocache.c`: `#define PTRSIZE 6` (на 64-bit, если не `NO48BITPTRS`).

x86-64 (и ARM64) используют только младшие **48 бит** виртуального адреса.
pogocache хранит указатели в **6 байтах** вместо 8 → бакет 10 байт вместо 12:
```c
struct bucket {
    uint8_t entry[PTRSIZE]; // 6-byte pointer (48-bit)
    uint8_t hash[HASHSIZE]; // 3-byte hash (24-bit)
    uint8_t dib;            // 1 byte: distance to bucket
};
static_assert(sizeof(struct bucket) == PTRSIZE+HASHSIZE+1, "bad bucket size");
```
`load_ptr`/`store_ptr` — ручная упаковка/распаковка через сдвиги.
**Экономия**: 10/12 = −17% памяти на бакет; на 4096 шардов × миллионы бакетов — гигабайты.

## ПРИЁМ №2: Robin Hood hashing + шардинг ⭐⭐⭐
- **4096 шардов** (default; автоподбор по размеру). Каждый — свой hashmap.
- **Open addressing + Robin Hood**: бакет хранит `dib` (distance to bucket).
- High 32 бита хэша → номер шарда; low 32 бита → hashmap внутри шарда.
- Каждый шард — свой spinlock (мало contention).
- Наследие: `tidwall/shardmap`.

**Почему**: шардинг → параллелизм без глобальных блокировок; Robin Hood →
предсказуемое время поиска + cache-friendly.

## ПРИЁМ №3: Entry = одна аллокация, variable-size поля ⭐⭐⭐
`struct entry`:
```c
struct entry {
    int64_t time;           // timestamp
    atomic_int rc;          // reference counter (без внешних блокировок)
    unsigned memszsz:2;     // размер size-префикса: 0=1B, 1=2B, 2=4B, 3=8B
    unsigned has_expires:1; // есть 64-bit expiration
    unsigned has_flags:1;   // есть 32-bit flags
    unsigned has_sixpack:1; // ключ sixpack-сжат
    uint8_t data[];         // key + value + опциональные поля
};
```
**Приёмы:**
- **Одна heap-аллокация на entry** (key+value+expiry+flags+size-prefix).
  Нет отдельных аллокаций для полей → меньше фрагментации, лучше locality.
- **Размер-префикс 1/2/4/8 байт** (`memszsz:2`): размер аллокации хранится
  минимально возможным числом байт → экономия на маленьких entry.
- **Битовые поля** (`:1`) упакованы в один `unsigned` → 0 лишних байт.
- **`atomic_int rc`** — refcount без мьютекса (для retain/release при итерации).

## ПРИЁМ №4: sixpack — сжатие ключей 6 бит/байт ⭐⭐
`sixpack()`/`unsixpack()`: кодирует символы, использующие только 64 значения,
в **6 бит вместо 8** (4 символа → 3 байта). Таблица `fromsix[]` для декодирования.
Порядок битов: little-endian nibble-пакинг (i%4: 0/1/2/3).
**Когда**: ключи типа hex/base64/URL-safe. Экономия 25% на ключах.
Флаг `has_sixpack:1` в entry; `--sixpack yes/no`.

## ПРИЁМ №5: Spinlock, кодирующий batch-указатель ⭐⭐
```c
struct shard { atomic_uintptr_t lock; /* batch pointer ИЛИ UINTPTR_MAX */ ... };
```
- **Без batch**: `lock = UINTPTR_MAX` (обычная блокировка).
- **С batch**: `lock = указатель на batch` — если тот же batch уже держит шард,
  `lock()` видит свой указатель и **не блокируется повторно** (реентерабельность
  для batch-операций без отдельного поля).
- Ожидание: `ctx->yield()` или **`cpu_yield()`** = `__builtin_ia32_pause()`
  (x86) / `yield` (ARM) — снижает энергопотребление и contention в spin-loop.

## ПРИЁМ №6: 2-random eviction (O(1), без LRU-списков) ⭐⭐
`auto_evict_entry()`: при low-mem сканирует бакеты от hash-позиции, берёт
**2 кандидата**, выбирает с **меньшим `time`** (старший). Никаких LRU-списков,
никаких updates на каждый доступ — только timestamp в entry.
Источник идеи: danluu.com/2choices-eviction (Power of Two Choices).
**Почему**: O(1) вместо O(1) с накладными на поддержку LRU-структуры; меньше
CPU-циклов; память освобождается немедленно.

## ПРИЁМ №7: Двухуровневая эвикция (low-mem vs expiration) ⭐
- **Low-mem**: немедленное освобождение при insert (2-random).
- **Expiration**: **фоновый sweep** — гарантирует, что просроченные entry
  занимают **не более 10%** памяти. `pogocache_sweep_poll()` — инкрементальный
  (poll 20 entry за раз), не блокирует.
**Почему**: разделение быстрого пути (low-mem) и постепенной уборки (sweep).

## ПРИЁМ №8: th64 — быстрый 64-bit хэш ⭐⭐
`th64()` (tidwall/th64): обрабатывает по 8 байт + хвост, финализация серией
`h^=h>>31; h*=r`. Сид **крипто-случайный на старте** (защита от hash-flooding).
**Почему**: быстрее SipHash/Murmur для коротких ключей, достаточно для кэша.

## ПРИЁМ №9: Модель потоков — thread-per-core + round-robin ⭐⭐
- Число потоков = число ядер (фиксируется на старте).
- Каждый поток — свой **event queue** (epoll/kqueue). Наследие `tidwall/evio`.
- Соединения распределяются по очередям **round-robin**.
- Опционально **io_uring** для read/write batching (Linux).
**Почему**: no cross-thread migration; минимум синхронизации; cache-affinity.

## ПРИЁМ №10: Ассемблерная оптимизация критичных мест
- `cpu_yield()`: `__builtin_ia32_pause` / ARM `yield` в spin-loop.
- `-O3 -flto=auto` (link-time optimization).
- Статическая линковка (musl) → один бинарь.
- `mimalloc`/`jemalloc` вместо системного malloc (лучше для многопоточности).

## ПРИЁМ №11: Varint + ручные int→str
`util.c`: `varint_write_u64/i64` (1-9 байт), `u64toa`/`i64toa` (свой itoa),
`crc32`, `mix13` (Murmur-финализация), `write_u64` (unaligned memcpy).
**Почему**: избежать libc printf/atoi (медленные, локаль).

## ПРИЁМ №12: Reference counting для итерации без удержания блокировки
`pogocache_entry_retain/release` — entry держится через `atomic_int rc`,
пока итератор его читает. Позволяет **не держать spinlock** во время callback.

---

## Что переносимо в allspark/prime-agent

| # | Приём | Применимость к нам | Вердикт |
|---|---|---|---|
| 1 | 6-байтные указатели | для нашего графа в RAM | ⭐ ПЕРЕНОСИТЬ (если свой arena) |
| 2 | Robin Hood + шардинг | кэш LSP-клиентов, индексов | ⭐ ПЕРЕНОСИТЬ (dashmap уже шардит) |
| 3 | Entry = 1 аллокация, var-size | узлы/рёбра графа в RAM | ⭐ ПЕРЕНОСИТЬ (арена + битовые поля) |
| 4 | sixpack 6-бит ключей | пути файлов/QN в графе | возможно (экономия памяти) |
| 5 | Spinlock с batch-ptr | конкурентные сессии | ⭐ ПЕРЕНОСИТЬ (Rust: parking_lot) |
| 6 | 2-random eviction | кэш LSP-поверхностей/векторов | ⭐ ПЕРЕНОСИТЬ |
| 7 | 2-уровневая эвикция | кэш в долгоживущей сессии | ПЕРЕНОСИТЬ |
| 8 | th64 | хэш для шардинга | есть (xxhash/ahash) |
| 9 | thread-per-core | параллельная индексация | ⭐ (tokio/rayon) |
| 10 | asm-оптимизации | spin-loop yield | частично |
| 11 | varint + itoa | сериализация графа | ⭐ ПЕРЕНОСИТЬ |
| 12 | refcount для итерации | итерация графа без lock | ⭐ ПЕРЕНОСИТЬ |

### Ключевые уроки для нашего дизайна
1. **CPU-циклы важнее «красоты»**: 2-random вместо LRU, timestamp вместо списков.
2. **Память = одна аллокация + минимальные префиксы** (не struct-на-поле).
3. **Шардинг по high-bits хэша** — базовый приём параллелизма.
4. **Spinlock + cpu_yield** для коротких критических секций.
5. **Сжатие мелких данных** (6-бит ключи, varint) — реальная экономия.
6. **Фоновая инкрементальная уборка** (sweep poll) вместо stop-the-world.

### Синтез трёх кэш-систем для pa-graph/pa-lsp
- **codebase-memory**: LSP-surface + closure-repair + 11-сигнальные эмбеддинги.
- **pogocache**: layout памяти + 2-random eviction + шардинг + 6-бит указатели.
- **oh-my-pi/Serena**: LSP-клиент + name_path.
→ `pa-graph`: RAM-граф (pogocache-layout) + SQLite (codebase-memory-схема) +
  LSP-поверхность (closure-repair) + кэш с 2-random eviction.


## ПРИЁМ №13: Персистентность — блочный LZ4 + CRC32 ⭐⭐
`save.c`: снапшот кэша на диск.
- **BLOCKSIZE = 1 MiB**: данные режутся на блоки, каждый сжимается LZ4.
- **16-байтный заголовок** на блок: `write_u32` (magic/type), CRC32 сжатых данных,
  длина распакованных, длина сжатых.
- `LZ4_compressBound` → точный расчёт буфера (без перевыделения).
- `LZ4_compress_default` / `LZ4_decompress_safe` (безопасная распаковка).
- **CRC32 на блок** → обнаружение повреждения.
**Почему**: блочное сжатие → потоковая запись/чтение, O(1) память, частичное
восстановление при ошибке блока.

## ПРИЁМ №14: Упаковка проекта (cross-compile) ⭐⭐
`tools/build/run.sh` — сборка под 5 платформ:
- `linux-amd64` / `linux-arm64` (glibc)
- `linux-amd64-musl` / `linux-arm64-musl` (статические)
- `apple-arm64`
**Приёмы:**
- Сборка через Docker (`Dockerfile.glibc`/`Dockerfile.musl`) для воспроизводимости.
- `make distclean clean` перед сборкой — чистый тарбол `deps/ src/`.
- **Кэш сборки по SHA1 репозитория**: `reposha=$(tar -O -xf repo.tar.gz | sha1sum)`;
  если `packages/$name.tar.gz` уже есть с тем же sha → `build=0` (пропуск).
- Статическая линковка musl → один самодостаточный бинарь.
- `.package` + `tools/package.sh` собирают все платформы разом.

## ПРИЁМ №15: HASHSIZE настраиваемый (1-4 байта)
`#define HASHSIZE 3` (default) — 24-bit hash-префикс в бакете.
`static_assert(sizeof(bucket) == PTRSIZE+HASHSIZE+1)`.
Меньше байт хэша → меньше памяти, но больше false-positive сравнений.
**Почему**: тюнинг под нагрузку (короткие ключи vs длинные).

---

## ИТОГ: полный список приёмов pogocache

### Память
1. 6-байтные указатели (48-bit адрес)
2. 10-байтный бакет (6 ptr + 3 hash + 1 dib)
3. Entry = одна аллокация, variable-size поля
4. Размер-префикс 1/2/4/8 байт
5. Битовые поля (`:1`) в одном unsigned
6. sixpack (6 бит/байт на ключах)
7. HASHSIZE тюнинг (1-4 байта)
8. mimalloc/jemalloc вместо malloc
9. Блочный LZ4 (1 MiB) + CRC32
10. varint для чисел

### Процессор
1. Шардинг (4096) → параллелизм без глобальных локов
2. Robin Hood → предсказуемый поиск, cache-friendly
3. Spinlock + `cpu_yield` (pause/yield)
4. 2-random eviction (O(1), без LRU-структур)
5. Timestamp вместо LRU-списков
6. Thread-per-core + round-robin (evio)
7. io_uring (опц.) для batching
8. th64 — быстрый хэш
9. `-O3 -flto=auto`
10. Refcount (atomic) для итерации без удержания лока
11. Фоновый инкрементальный sweep (poll 20)

### Упаковка
1. Single-file embeddable (`pogocache.c`)
2. Cross-compile под 5 платформ (glibc/musl/apple)
3. Docker-сборки для воспроизводимости
4. Кэш сборки по SHA1 репо
5. Статическая линковка musl
6. `static_assert` для layout-инвариантов
