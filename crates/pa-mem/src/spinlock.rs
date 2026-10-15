//! P5 — a spinlock with batch reentrancy and CPU-friendly waiting.
//!
//! pogocache's shard lock is a single `atomic_uintptr_t` that encodes *either*
//! `UINTPTR_MAX` (an ordinary lock) *or* the address of the batch that holds it:
//!
//! ```c
//! struct shard { atomic_uintptr_t lock; /* batch pointer OR UINTPTR_MAX */ };
//! ```
//!
//! That trick buys reentrancy without a second field: when a batch already owns
//! a shard, `lock()` sees its own pointer and returns immediately instead of
//! self-deadlocking. The wait loop calls `cpu_yield()` — `__builtin_ia32_pause`
//! on x86, `yield` on ARM — to cool the core and hand it to the sibling
//! hyperthread.
//!
//! The safe Rust equivalent replaces the pointer with a small integer *owner
//! token*: `0` means free, a [`Batch`]'s token means that batch holds it, and
//! [`SpinLock::UINTPTR_MAX`] (all bits set) means a non-batch holder. Waiting is
//! `std::hint::spin_loop()` with a `std::thread::yield_now()` fallback after a
//! few rounds — the same pause-then-yield escalation, no `unsafe`.
//!
//! # Batches are single-threaded
//!
//! A [`Batch`] is the unit that locks many shards for one operation and
//! releases them at the end; pogocache's batch is thread-local for exactly this
//! reason. Reentrancy is keyed on the batch token alone, so a batch must be
//! used by one thread at a time — sharing one across threads would let a second
//! thread mistake the holder's token for its own and skip the wait. Give each
//! thread its own [`Batch`].

use std::hint::spin_loop;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Hands out batch tokens. A process-global counter (not an address) so two
/// live batches can never collide — a freed allocation's address can be handed
/// straight back to the next one.
static NEXT_BATCH_TOKEN: AtomicUsize = AtomicUsize::new(1);

/// The value an ordinary (non-batch) holder stores — pogocache's
/// `UINTPTR_MAX`.
pub const UINTPTR_MAX: usize = usize::MAX;

/// How many `spin_loop` rounds run before the waiter yields the thread.
const SPIN_ROUNDS: u32 = 64;

/// A reentrant spinlock keyed on a batch token.
///
/// Ordinary users take [`SpinLock::lock`], which stores [`UINTPTR_MAX`].
/// A [`Batch`] takes [`SpinLock::lock_batch`], which stores the batch's token
/// and lets the *same* batch lock the same shard again without blocking.
#[derive(Debug, Default)]
pub struct SpinLock {
    state: AtomicUsize,
}

impl SpinLock {
    /// The all-bits-set marker an ordinary holder stores.
    pub const UINTPTR_MAX: usize = UINTPTR_MAX;

    /// An unlocked spinlock.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
        }
    }

    /// Whether the lock is currently held.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) != 0
    }

    /// Takes the lock, blocking (spinning, then yielding) until it is free.
    pub fn lock(&self) -> SpinGuard<'_> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }
            self.wait();
        }
    }

    /// Takes the lock if it is free.
    #[must_use]
    pub fn try_lock(&self) -> Option<SpinGuard<'_>> {
        match self
            .state
            .compare_exchange(0, UINTPTR_MAX, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => Some(SpinGuard { lock: Some(self) }),
            Err(_) => None,
        }
    }

    /// Takes the lock on behalf of `batch`.
    ///
    /// Reentrant: if `batch` already holds this lock, the call returns at once
    /// (exactly the C `val == batch` check) with a *borrowed* guard that does
    /// not release — the outermost acquisition owns the release. A fresh
    /// acquisition returns a releasing guard.
    ///
    /// `batch` must be used by a single thread; see the module note.
    pub fn lock_batch(&self, batch: &Batch) -> SpinGuard<'_> {
        let token = batch.token();
        loop {
            let current = self.state.load(Ordering::Relaxed);
            if current == token {
                // Reentrant: this batch already holds the shard. Return a
                // guard that will not release, so the outermost guard keeps
                // ownership of the unlock.
                return SpinGuard { lock: None };
            }
            if current == 0
                && self
                    .state
                    .compare_exchange(0, token, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                batch.record_acquire();
                return SpinGuard { lock: Some(self) };
            }
            self.wait();
        }
    }

    /// Spins a bounded number of rounds, then yields the thread.
    fn wait(&self) {
        for _ in 0..SPIN_ROUNDS {
            if self.state.load(Ordering::Relaxed) == 0 {
                return;
            }
            spin_loop();
        }
        thread::yield_now();
    }

    /// Releases the lock (used by the guards).
    fn unlock(&self) {
        self.state.store(0, Ordering::Release);
    }
}

/// A batch: a token that can hold several shards reentrantly.
///
/// pogocache's batch is the unit that locks many shards for one operation and
/// unlocks them all at the end; reentrancy is what lets it revisit a shard.
/// Here the batch is a unique token plus an acquire counter (its "batch
/// pointer" identity and the linked list of held shards, reduced to what the
/// safe layout needs). A batch is single-threaded.
#[derive(Debug)]
pub struct Batch {
    token: usize,
    acquires: AtomicUsize,
}

impl Batch {
    /// A batch with a process-unique token, never `0` or [`UINTPTR_MAX`], so
    /// it can never be mistaken for a free lock or for an ordinary holder.
    #[must_use]
    pub fn new() -> Self {
        // `fetch_add` starts at 1; skip the two reserved values on the wrap so
        // a long-running process never mints a token that aliases them.
        let mut token = NEXT_BATCH_TOKEN.fetch_add(1, Ordering::Relaxed);
        while token == 0 || token == UINTPTR_MAX {
            token = NEXT_BATCH_TOKEN.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            token,
            acquires: AtomicUsize::new(0),
        }
    }

    /// The token this batch stores into a lock.
    #[must_use]
    pub const fn token(&self) -> usize {
        self.token
    }

    /// How many shards the batch has newly acquired (reentrant re-locks do not
    /// count).
    #[must_use]
    pub fn acquires(&self) -> usize {
        self.acquires.load(Ordering::Relaxed)
    }

    fn record_acquire(&self) {
        self.acquires.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for Batch {
    fn default() -> Self {
        Self::new()
    }
}

/// The guard returned by a successful lock.
///
/// `lock: Some(_)` releases on drop; `lock: None` is a borrowed reentrant
/// guard that must not (the outermost acquisition unlocks).
#[derive(Debug)]
pub struct SpinGuard<'a> {
    lock: Option<&'a SpinLock>,
}

impl Drop for SpinGuard<'_> {
    fn drop(&mut self) {
        if let Some(lock) = self.lock {
            lock.unlock();
        }
    }
}

/// A convenience guard pairing a lock with a batch, so a batch-held critical
/// section reads like an ordinary one.
#[derive(Debug)]
pub struct BatchGuard<'a> {
    _guard: SpinGuard<'a>,
}

impl<'a> BatchGuard<'a> {
    /// Locks `lock` on behalf of `batch`.
    #[must_use]
    pub fn new(lock: &'a SpinLock, batch: &Batch) -> Self {
        Self {
            _guard: lock.lock_batch(batch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize as Counter;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn locks_and_unlocks() {
        let lock = SpinLock::new();
        assert!(!lock.is_locked());
        {
            let _guard = lock.lock();
            assert!(lock.is_locked());
        }
        assert!(!lock.is_locked());
    }

    #[test]
    fn try_lock_fails_while_held() {
        let lock = SpinLock::new();
        let guard = lock.lock();
        assert!(lock.try_lock().is_none(), "a held lock cannot be taken");
        drop(guard);
        assert!(lock.try_lock().is_some(), "a released lock can be taken");
    }

    #[test]
    fn a_batch_reenters_its_own_lock_without_releasing_early() {
        let lock = SpinLock::new();
        let batch = Batch::new();
        let outer = lock.lock_batch(&batch);
        assert_eq!(batch.acquires(), 1);
        {
            let inner = lock.lock_batch(&batch);
            assert_eq!(batch.acquires(), 1, "reentrant re-locks are not counted");
            drop(inner);
            assert!(lock.is_locked(), "the inner guard must not unlock");
        }
        drop(outer);
        assert!(!lock.is_locked(), "the outer guard owns the unlock");
    }

    #[test]
    fn a_batch_token_is_never_free_or_ordinary() {
        for _ in 0..100 {
            let batch = Batch::new();
            assert_ne!(batch.token(), 0);
            assert_ne!(batch.token(), UINTPTR_MAX);
        }
    }

    #[test]
    fn distinct_batches_have_distinct_tokens() {
        let a = Batch::new();
        let b = Batch::new();
        assert_ne!(a.token(), b.token());
    }

    #[test]
    fn the_guard_releases_on_drop() {
        let lock = SpinLock::new();
        {
            let _guard = lock.lock();
            assert!(lock.is_locked());
        }
        assert!(lock.try_lock().is_some());
    }

    #[test]
    fn concurrent_increments_are_not_lost() {
        // The classic counter test: an unserialised read-modify-write would
        // lose updates and the total would come up short.
        let lock = Arc::new(SpinLock::new());
        let counter = Arc::new(Counter::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let lock = Arc::clone(&lock);
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..10_000 {
                        let _guard = lock.lock();
                        let value = counter.load(Ordering::Relaxed);
                        counter.store(value + 1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        for handle in threads {
            handle.join().expect("worker thread");
        }
        assert_eq!(counter.load(Ordering::Relaxed), 80_000);
    }

    #[test]
    fn distinct_batches_serialise_across_threads() {
        // Each thread holds its own batch (the contract); the shared lock must
        // still give them mutual exclusion against one another.
        let lock = Arc::new(SpinLock::new());
        let inside = Arc::new(Counter::new(0));
        let max_inside = Arc::new(Counter::new(0));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let lock = Arc::clone(&lock);
                let inside = Arc::clone(&inside);
                let max_inside = Arc::clone(&max_inside);
                thread::spawn(move || {
                    let batch = Batch::new();
                    for _ in 0..5_000 {
                        let _guard = lock.lock_batch(&batch);
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        max_inside.fetch_max(now, Ordering::SeqCst);
                        inside.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for handle in threads {
            handle.join().expect("worker thread");
        }
        assert_eq!(
            max_inside.load(Ordering::SeqCst),
            1,
            "two batches were inside the critical section at once"
        );
        assert!(!lock.is_locked());
    }
}
