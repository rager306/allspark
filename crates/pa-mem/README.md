# pa-mem

Low-level RAM layout structures, ported safely from
[`pogocache`](https://github.com/tidwall/pogocache) (C11, MIT).

pogocache earns its throughput from a hand-packed memory layout: 6-byte (48-bit)
pointers, 10-byte hash buckets, Robin Hood open addressing over 4096 shards, one
allocation per entry with a minimal size prefix, spinlocks that double as batch
pointers, and atomic refcounts that let an iterator drop its lock early. The C
source is full of pointer arithmetic; this crate reproduces the **layout and
algorithms** while staying inside `#![forbid(unsafe_code)]` — pointers become
arena indices, locks and counters become atomics, and every decode is
bounds-checked.

## Scope

- **`shard` (P1)**: `ShardedMap` — a sharded map with `DEFAULT_SHARDS` (4096)
  shards, each behind its own `RwLock`. The high 32 hash bits pick the shard
  (`(hash >> 32) & mask`, pogocache's `shard_index`); `shard_of` exposes the
  routing, and a process-unique seed (or an explicit one via `with_seed`) makes
  it reproducible for tests.
- **`robin_hood` (P2)**: `RobinHoodMap` — open addressing with a `dib`
  (distance-to-bucket) per slot, Robin Hood displacement on insert, and
  backward-shift deletion (no tombstones). The load factor is capped at 75%
  (`DEFLOADFACTOR`).
- **`ptr48` (P3)**: `SixBytePtr` (a 48-bit index packed into 6 bytes) and
  `Bucket` (`#[repr(C)]`, exactly 10 bytes: 6-byte pointer + 3-byte hash +
  1-byte `dib`, with a compile-time size assertion). No raw address is ever
  dereferenced; the field holds an arena index.
- **`entry` (P4)**: `EntryArena` — one contiguous blob per entry, a size prefix
  sized to 1/2/4/8 bytes (`SizePrefix`), a bitfield header (one byte for
  `memszsz` + three flags), and a varint key length. `alloc`/`get`/`free` with
  best-fit block reuse.
- **`spinlock` (P5)**: `SpinLock` + `Batch` — an atomic spinlock whose wait loop
  is `std::hint::spin_loop()` escalating to `std::thread::yield_now()`, with
  batch reentrancy: a batch that already holds a shard re-enters instead of
  self-deadlocking (pogocache's batch-pointer trick, as an owner token).
- **`refcount` (P6)**: `RefCounted` (an `Arc`-like handle with an explicit
  `acquire`/`release`/`strong_count`) and `SnapshotList` — `retain_all` snapshots
  every element under the lock, so the caller can drop the lock and iterate
  lock-free.
- **`snapshot` (P7)**: `BlockSnapshot` — 1 MiB LZ4 blocks (`lz4_flex`, safe-only
  codec), a 16-byte per-block header, and a CRC32 per block (`crc32fast`). A
  corrupt block costs its own bytes, not the tail (`decompress_lossy`).
- **`sha_cache` (P8)**: `ShaCache` — fingerprints a set of inputs into one
  SHA-256 and stores/looks up the artifact built from exactly those inputs
  (pogocache's `reposha` build-skip, as a reusable unit).
- **`hash` / `varint`**: `th64` and `mix13` (pinned to C reference vectors) and
  the varint codec the entry layout uses.

## Non-goals

- No raw pointer handling, no `unsafe`, no FFI. Indices and atomics only.
- No eviction policy, TTL sweeps, or cache semantics — those live in
  `pa-graph`'s `two_random`/`query_cache`; this crate owns the layout.
- No CI wiring for cross-compilation: `ShaCache` is the cache-key helper the
  packaging pipeline would call, not a target list.
- No graph coupling — `pa-mem` is a leaf with no workspace dependencies.

## Public API

```rust
use pa_mem::{ShardedMap, RobinHoodMap, SixBytePtr, Bucket, EntryArena, EntryOpts,
             SpinLock, Batch, RefCounted, SnapshotList, BlockSnapshot, ShaCache};
// (P5/P6: SpinLock + Batch, RefCounted + SnapshotList — see the modules.)

let map: ShardedMap<String, u64> = ShardedMap::new();
map.insert("k".into(), 1);
assert!(map.shard_of(&"k".to_string()) < map.shard_count());

let mut rh: RobinHoodMap<&str, u64> = RobinHoodMap::new();
rh.insert("k", 1);
assert_eq!(rh.get(&"k"), Some(&1));

let packed = SixBytePtr::pack(42).unwrap();
assert_eq!(SixBytePtr::unpack(packed.to_bytes()), 42);

let mut arena = EntryArena::new();
let r = arena.alloc(EntryOpts::at(1), b"key", b"value").unwrap();
assert_eq!(arena.get(r).unwrap().value, b"value");

let blob = BlockSnapshot::compress(b"hello world");
assert_eq!(BlockSnapshot::decompress(&blob).unwrap(), b"hello world");
```
