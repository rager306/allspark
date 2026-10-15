//! End-to-end checks that the F9 pieces compose.
//!
//! Each module has unit tests for its own behaviour; this file drives the
//! structures together the way a RAM cache would — a sharded index of arenas,
//! a snapshot of the whole thing, and a lock-free read of a retained batch.

use std::sync::Arc;
use std::thread;

use pa_mem::{
    BlockSnapshot, Bucket, EntryArena, EntryOpts, RefCounted, RobinHoodMap, ShaCache, ShardedMap,
    SixBytePtr, SnapshotList,
};

#[test]
fn a_sharded_map_indexes_arena_entries() {
    // The index maps a key to its shard; the arena owns the bytes. Putting the
    // two together is the layout pogocache builds, minus its raw pointers.
    let index: ShardedMap<String, u64> = ShardedMap::new();
    let mut arena = EntryArena::new();
    for i in 0..1000u64 {
        let key = format!("key-{i}");
        let reference = arena
            .alloc(
                EntryOpts::at(i.cast_signed()),
                key.as_bytes(),
                &i.to_le_bytes(),
            )
            .expect("alloc");
        index.insert(key, reference.offset() as u64);
    }
    assert_eq!(index.len(), 1000);
    // Every key routes to a stable shard, and the arena holds every blob.
    let mut seen_shards = std::collections::HashSet::new();
    for i in 0..1000u64 {
        let key = format!("key-{i}");
        seen_shards.insert(index.shard_of(&key));
    }
    assert!(
        seen_shards.len() > 500,
        "keys spread over only {} shards",
        seen_shards.len()
    );
}

#[test]
fn robin_hood_and_sharded_map_agree_on_a_workload() {
    // The two map implementations must return identical results for the same
    // operations, including after churn.
    let sharded: ShardedMap<String, u64> = ShardedMap::new();
    let mut robin: RobinHoodMap<String, u64> = RobinHoodMap::new();
    for i in 0..2000u64 {
        let key = format!("k{i}");
        sharded.insert(key.clone(), i);
        robin.insert(key, i);
    }
    for i in (0..2000u64).step_by(3) {
        let key = format!("k{i}");
        assert_eq!(sharded.remove(&key), robin.remove(&key));
    }
    for i in 0..2000u64 {
        let key = format!("k{i}");
        assert_eq!(sharded.get(&key), robin.get(&key).copied(), "key {i}");
    }
    assert_eq!(sharded.len(), robin.len());
}

#[test]
fn a_bucket_round_trips_through_its_packed_form() {
    let bucket = Bucket::with(123_456, 0xabcd_ef01_2345_6789, 7).expect("bucket");
    assert_eq!(bucket.index(), 123_456);
    assert_eq!(bucket.hash24(), 0x0045_6789);
    assert_eq!(bucket.dib, 7);
    assert_eq!(std::mem::size_of_val(&bucket), 10);
    // The pointer half survives a byte-level round trip.
    assert_eq!(SixBytePtr::unpack(bucket.ptr), 123_456);
}

#[test]
fn a_snapshot_round_trips_a_realistic_payload() {
    // Build a few MiB of skewed data (some compressible, some not), snapshot
    // it, and confirm a byte-exact round trip across several blocks.
    let mut payload = Vec::new();
    for i in 0..600_000u32 {
        payload.extend_from_slice(&i.to_le_bytes());
        if i % 5 == 0 {
            payload.extend_from_slice(b"repeated-tail");
        }
    }
    let blob = BlockSnapshot::compress(&payload);
    assert!(blob.len() < payload.len(), "the payload compressed");
    let restored = BlockSnapshot::decompress(&blob).expect("decompress");
    assert_eq!(restored, payload);
}

#[test]
fn a_snapshot_survives_a_file_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("snapshot.pms");
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let blob = BlockSnapshot::compress(&payload);
    std::fs::write(&path, &blob).expect("write");
    let read_back = std::fs::read(&path).expect("read");
    assert_eq!(
        BlockSnapshot::decompress(&read_back).expect("decompress"),
        payload
    );
}

#[test]
fn concurrent_readers_of_a_retained_snapshot_agree() {
    // The refcount pattern end to end: retain under a lock, iterate lock-free,
    // from several threads, while a writer keeps mutating the live list.
    let list = Arc::new(std::sync::Mutex::new(SnapshotList::new()));
    for i in 0..200u64 {
        list.lock().expect("lock").push(i);
    }
    let retained = Arc::new({
        let guard = list.lock().expect("lock");
        guard.retain_all()
    });
    let writer = {
        let list = Arc::clone(&list);
        thread::spawn(move || {
            for i in 200..400u64 {
                list.lock().expect("lock").push(i);
            }
        })
    };
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let retained = Arc::clone(&retained);
            thread::spawn(move || {
                let mut total = 0u64;
                for _ in 0..100 {
                    total = retained.iter().copied().sum();
                }
                total
            })
        })
        .collect();
    writer.join().expect("writer");
    let expected: u64 = (0..200u64).sum();
    for reader in readers {
        assert_eq!(reader.join().expect("reader"), expected);
    }
}

#[test]
fn the_build_cache_skips_unchanged_inputs() {
    // The P8 flow: fingerprint inputs, build if the key misses, hit on rerun.
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = ShaCache::new(dir.path().join("cache"));
    let inputs = [
        ("Cargo.toml", b"[package]".as_slice()),
        ("src/lib.rs", b"pub fn a() {}".as_slice()),
    ];
    let key = cache.key(inputs);
    assert!(!cache.contains(&key), "a fresh key misses");

    // "Build" and store the artifact.
    let artifact = b"compiled-binary-bytes";
    cache
        .store(&key, &["inputs=2".to_string()], artifact)
        .expect("store");

    // A rerun with the same inputs hits; a changed input misses.
    assert_eq!(cache.key(inputs), key);
    assert_eq!(cache.get(&key).expect("get").expect("present"), artifact);
    let changed = [
        ("Cargo.toml", b"[package]".as_slice()),
        ("src/lib.rs", b"pub fn b() {}".as_slice()),
    ];
    assert_ne!(cache.key(changed), key);
    assert!(!cache.contains(&cache.key(changed)));
}

#[test]
fn refcounted_handles_survive_their_owner() {
    let handle = RefCounted::new(vec![1u8, 2, 3]);
    let survivor = handle.acquire();
    drop(handle);
    assert_eq!(survivor.strong_count(), 1);
    assert_eq!(survivor.get(), &vec![1u8, 2, 3]);
}
