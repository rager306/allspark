//! # pa-mem
//!
//! Low-level RAM layout structures, ported safely from
//! [`pogocache`](https://github.com/tidwall/pogocache) (C11).
//!
//! pogocache earns its throughput from a hand-packed memory layout: 6-byte
//! (48-bit) pointers, 10-byte hash buckets, Robin Hood open addressing spread
//! over 4096 shards, one allocation per entry with a 1/2/4/8-byte size prefix,
//! spinlocks that double as batch pointers, and atomic refcounts that let an
//! iterator drop the lock early. The C code is full of `unsafe` pointer
//! arithmetic; this crate reproduces the *layout and algorithms* while staying
//! inside `#![forbid(unsafe_code)]` — pointers become arena indices, locks and
//! counters become atomics, and every decode is bounds-checked.
//!
//! ## Modules (phase F9 backlog, P1-P8)
//!
//! - [`shard`] — P1 [`ShardedMap`]: a sharded map; the high 32 hash bits pick
//!   the shard, each shard has its own lock.
//! - [`robin_hood`] — P2 [`RobinHoodMap`]: open addressing with `dib`
//!   (distance to bucket) and backward-shift deletion.
//! - [`ptr48`] — P3 [`SixBytePtr`] + [`Bucket`]: a 48-bit index packed into 6
//!   bytes, and the 10-byte bucket (6-byte pointer + 3-byte hash + 1-byte dib).
//! - [`entry`] — P4 [`EntryArena`]: one blob per entry, a minimal size prefix,
//!   a bitfield header, and varint key lengths.
//! - [`spinlock`] — P5 [`SpinLock`]: `spin_loop`/`yield_now` waiting and batch
//!   reentrancy (a lock encodes the batch that holds it).
//! - [`refcount`] — P6 [`RefCounted`] / [`SnapshotList`]: iterate a snapshot
//!   without holding the lock.
//! - [`snapshot`] — P7 [`BlockSnapshot`]: 1 MiB LZ4 blocks, 16-byte headers,
//!   CRC32 per block.
//! - [`sha_cache`] — P8 [`ShaCache`]: input-fingerprint build cache (the
//!   cross-compile helper, not a CI target list).
//! - [`hash`] — [`th64`] and [`mix13`], the sharding/hashmap primitives.
//! - [`varint`] — the varint codec the entry layout uses.
//!
//! ## Example
//!
//! ```
//! use pa_mem::{BlockSnapshot, EntryArena, EntryOpts, RobinHoodMap, ShardedMap, SixBytePtr};
//!
//! // P1: a sharded map routes each key to one of 4096 shards.
//! let shards: ShardedMap<String, u64> = ShardedMap::new();
//! shards.insert("file:src/lib.rs".into(), 1);
//! assert!(shards.shard_of(&"file:src/lib.rs".to_string()) < shards.shard_count());
//!
//! // P2: a Robin Hood map with the same key/value contract.
//! let mut robin: RobinHoodMap<&str, u64> = RobinHoodMap::new();
//! robin.insert("k", 7);
//! assert_eq!(robin.get(&"k"), Some(&7));
//!
//! // P3: a 48-bit index packed into six bytes.
//! let packed = SixBytePtr::pack(42).expect("fits 48 bits");
//! assert_eq!(SixBytePtr::unpack(packed.to_bytes()), 42);
//!
//! // P4: one blob per entry in a byte-buffer arena.
//! let mut arena = EntryArena::new();
//! let entry = arena.alloc(EntryOpts::at(9), b"key", b"value").expect("alloc");
//! assert_eq!(arena.get(entry).expect("get").value, b"value");
//!
//! // P7: a block-LZ4 snapshot round trip.
//! let blob = BlockSnapshot::compress(b"hello world");
//! assert_eq!(BlockSnapshot::decompress(&blob).expect("decode"), b"hello world");
//! ```

#![forbid(unsafe_code)]
// pogocache's whole point is packing sizes and hashes into the fewest bytes.
// Every narrowing cast here is over a value that is structurally bounded
// (a length that already fits the field it is written into, a hash masked to
// its bucket width, a block count that fits the header); the conversions are
// the layout, so the cast family is allowed crate-wide with that argument
// stated here rather than at each of the ~dozen sites.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod entry;
pub mod error;
pub mod hash;
pub mod ptr48;
pub mod refcount;
pub mod robin_hood;
pub mod sha_cache;
pub mod shard;
pub mod snapshot;
pub mod spinlock;
pub mod varint;

pub use entry::{EntryArena, EntryOpts, EntryRef, EntryView, SizePrefix};
pub use error::{MemError, MemResult};
pub use hash::{mix13, th64};
pub use ptr48::{Bucket, SixBytePtr, BUCKET_BYTES, HASH_BYTES, MAX_INDEX, PTR_BITS, PTR_BYTES};
pub use refcount::{RefCounted, Retained, SnapshotList};
pub use robin_hood::{RobinHoodMap, MAX_DIB};
pub use sha_cache::ShaCache;
pub use shard::{ShardedMap, DEFAULT_SHARDS};
pub use snapshot::{
    decode_block, BlockHeader, BlockIter, BlockSnapshot, FileHeader, SnapshotWriter,
    BLOCK_HEADER_LEN, BLOCK_MAGIC, BLOCK_SIZE, FILE_HEADER_LEN, FILE_MAGIC,
};
pub use spinlock::{Batch, BatchGuard, SpinGuard, SpinLock, UINTPTR_MAX};
