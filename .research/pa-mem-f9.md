# F9 — pogocache RAM-layout backlog: implementation map

Crate: `crates/pa-mem` (new leaf, `#![forbid(unsafe_code)]`).
Source: `vendor-source/pogocache` (C11) + `.research/pogocache-techniques.md` +
`.research/design-matrix.jsonl` (P1-P8).

Decision: **a new crate, not a `pa-graph` module.** The eight pieces are RAM
layout/primitive code with no graph coupling; `pa-graph` already carries the
graph-specific memory work (`two_random`, `query_cache`, `pdx`, `snapcompact`).
Keeping the primitives in a leaf crate preserves the ownership rule (one area
per crate) and lets `pa-graph` (or anything else) depend on them later without a
cycle.

## P1-P8 → module

| # | Feature | Module | Safe-Rust shape |
|---|---|---|---|
| P1 | `sharded_graph` | `shard.rs` | `ShardedMap`, 4096 `RwLock<HashMap>` shards, `(hash>>32)&mask` routing, `shard_of`, process-unique or explicit seed |
| P2 | `robin_hood` | `robin_hood.rs` | `RobinHoodMap`, open addressing, `dib` per slot, Robin Hood swap, backward-shift delete, 75% load cap, `MAX_DIB` ceiling |
| P3 | `six_byte_pointers` | `ptr48.rs` | `SixBytePtr` (48-bit index in `[u8;6]`), `Bucket` `#[repr(C)]` with a `const _: () = assert!(size == 10)`, `clip_hash` (24-bit) |
| P4 | `entry_single_alloc` | `entry.rs` | `EntryArena` byte buffer + `EntryRef` `(offset,len)`; `SizePrefix` 1/2/4/8-byte, bitfield header byte, varint key length, best-fit free-list |
| P5 | `spinlock_yield` | `spinlock.rs` | `SpinLock` (`AtomicUsize`), `spin_loop()` → `yield_now()`, `Batch` owner-token reentrancy (the C batch-pointer trick) |
| P6 | `refcount_iteration` | `refcount.rs` | `RefCounted` (`Arc` + explicit acquire/release/strong_count), `SnapshotList::retain_all` → lock-free `Retained` |
| P7 | `block_lz4_snapshot` | `snapshot.rs` | `BlockSnapshot`, 1 MiB blocks, 16-byte `BlockHeader`, per-block CRC32, `decompress_lossy` salvage |
| P8 | `cross_compile_sha_cache` | `sha_cache.rs` | `ShaCache`: inputs → SHA-256 → artifact; the build-skip key, no CI targets |

Support: `hash.rs` (`th64`, `mix13`, pinned to C reference vectors) and
`varint.rs` (the entry codec).

## The `unsafe`-to-safe translation

pogocache's speed comes from packing raw pointers and locks into small integers.
Every such trick has a safe equivalent that keeps the *layout* and the
*algorithm*:

- 6-byte **machine pointer** → 48-bit **arena index** (`SixBytePtr`). No
  dereference, so no `unsafe`; `pack` bounds-checks and `raw_pack` masks like
  the C `store_ptr`.
- `atomic_uintptr_t` **lock that stores a batch pointer** → `AtomicUsize` that
  stores a **batch token** (`0` = free, `UINTPTR_MAX` = ordinary holder, else a
  batch). The reentrancy check (`val == batch`) is `current == token`.
- `struct entry { data[] }` **flexible array + size prefix** → a `Vec<u8>` arena
  with `(offset, len)` handles; the fixed header fields (`time`, `rc`) live in
  the arena's side table.
- `atomic_int rc` **refcount** → `Arc`-backed `RefCounted`; `SnapshotList`
  makes the "retain under the lock, iterate without it" pattern explicit.
- `LZ4_*` C calls → `lz4_flex`'s safe-only block codec; the hand-rolled
  `crc32` → `crc32fast` (same IEEE polynomial).

## P8 — cross-compile + SHA build cache

The task scopes P8 as build tooling, not runtime code, and explicitly allows
either a `build.rs` helper, a script, or a `ShaCache` utility. Delivered as the
utility (`sha_cache.rs`, with tests) plus this note:

- pogocache's `tools/build/run.sh` builds five targets — `linux-amd64`,
  `linux-arm64`, `linux-amd64-musl`, `linux-arm64-musl`, `apple-arm64` — via
  Docker for glibc/musl and natively for Apple, and **skips a platform when the
  repo SHA-1 already has a package**: `reposha=$(tar -O -xf repo.tar.gz | sha1sum)`.
- `ShaCache` is that skip decision as a unit: `key(inputs)` folds a sorted,
  length-prefixed input set into one SHA-256; `key_for_files` fingerprints a
  tree by *relative* path (so the same tree fingerprints identically at two
  absolute locations); `store`/`get`/`contains` are the artifact side.
- **No cross-compile targets were added to CI** (explicit instruction). The
  Rust workspace's existing release pipeline owns the actual target matrix;
  `ShaCache` is the helper it would call to skip unchanged platforms.

## Verification

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test -p pa-mem` are green: 85 unit tests + 8 integration tests + 1
doctest. The hash module pins `th64`/`mix13` against values computed from the C
source, so the port is not just self-consistent.
