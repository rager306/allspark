//! P6 — atomic refcounting, so an iterator can drop the lock early.
//!
//! pogocache keeps an `atomic_int rc` inside every entry. An iterator takes the
//! shard spinlock only long enough to `retain` the entries it wants, releases
//! the lock, and then walks them while their refcounts keep them alive — the
//! lock is never held across the callback:
//!
//! ```c
//! struct entry { ...; atomic_int rc; uint8_t data[]; };
//! pogocache_entry_retain(entry);   // rc++
//! // ... release the shard lock, run the callback ...
//! pogocache_entry_release(entry);  // rc--
//! ```
//!
//! [`RefCounted`] is the safe analogue of that pair (a slimmed `Arc` with an
//! explicit count read), and [`SnapshotList`] is the collection that makes the
//! pattern concrete: `retain_all` bumps every element's count under the lock,
//! the caller drops the lock, and iteration then runs lock-free over the
//! retained handles.

use std::fmt;
use std::sync::Arc;

/// An atomically reference-counted handle.
///
/// Wraps [`Arc`] (which already owns the release-on-drop machinery) and adds
/// the one thing the iteration pattern needs beyond it: a way to *observe* the
/// strong count, and to clone it explicitly as an `acquire`. The name mirrors
/// pogocache's `retain`/`release` pair.
pub struct RefCounted<T> {
    inner: Arc<T>,
}

impl<T> RefCounted<T> {
    /// Creates a new handle with a strong count of one.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(value),
        }
    }

    /// Takes a reference (`retain` in the C code): the count goes up by one.
    #[must_use]
    pub fn acquire(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Drops a reference (`release`): the count goes down by one, and the value
    /// is destroyed at zero. Returns whether this call released the last one.
    ///
    /// This is just `drop`, spelled out so call sites read like the C pair.
    #[must_use]
    pub fn release(self) -> bool {
        let last = self.strong_count() == 1;
        drop(self);
        last
    }

    /// The current strong count.
    #[must_use]
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// Borrows the value.
    #[must_use]
    pub fn get(&self) -> &T {
        &self.inner
    }

    /// Unwraps the value when this is the only handle.
    ///
    /// # Errors
    ///
    /// Returns `self` back when other handles are still alive.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match Arc::try_unwrap(self.inner) {
            Ok(value) => Ok(value),
            Err(inner) => Err(Self { inner }),
        }
    }
}

impl<T> Clone for RefCounted<T> {
    /// Cloning takes a reference, exactly like [`RefCounted::acquire`].
    fn clone(&self) -> Self {
        self.acquire()
    }
}

impl<T> std::ops::Deref for RefCounted<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: fmt::Debug> fmt::Debug for RefCounted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefCounted")
            .field("strong_count", &self.strong_count())
            .field("value", self.get())
            .finish()
    }
}

/// A collection whose elements can be retained in bulk and then iterated
/// without the collection's lock.
///
/// `retain_all` is the lock-scoped half: it snapshots every live element into
/// an owned handle and hands the batch back. Once the caller drops the lock it
/// holds a [`Retained`], which walks the handles lock-free — the elements stay
/// alive because each handle's refcount is raised.
#[derive(Debug, Default)]
pub struct SnapshotList<T> {
    items: Vec<RefCounted<T>>,
}

impl<T> SnapshotList<T> {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Appends `value`, returning its handle.
    pub fn push(&mut self, value: T) -> RefCounted<T> {
        let handle = RefCounted::new(value);
        self.items.push(handle.acquire());
        handle
    }

    /// Number of stored elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Removes the element whose handle matches `handle`, if present.
    ///
    /// Matching is by pointer identity of the underlying value.
    pub fn remove(&mut self, handle: &RefCounted<T>) -> bool {
        let target = std::ptr::from_ref(handle.get());
        if let Some(index) = self
            .items
            .iter()
            .position(|item| std::ptr::from_ref(item.get()) == target)
        {
            self.items.swap_remove(index);
            return true;
        }
        false
    }

    /// Retains every element and returns a lock-free iterator over the batch.
    ///
    /// The caller holds the list's lock only for the duration of this call;
    /// the returned [`Retained`] owns one reference per element, so it can be
    /// walked after the lock is released.
    #[must_use]
    pub fn retain_all(&self) -> Retained<T> {
        Retained {
            items: self.items.iter().map(RefCounted::acquire).collect(),
        }
    }
}

/// A lock-free batch of retained elements produced by
/// [`SnapshotList::retain_all`].
pub struct Retained<T> {
    items: Vec<RefCounted<T>>,
}

impl<T> Retained<T> {
    /// Number of retained elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the batch is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterates the retained elements.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(RefCounted::get)
    }
}

impl<T> IntoIterator for Retained<T> {
    type Item = RefCounted<T>;
    type IntoIter = std::vec::IntoIter<RefCounted<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a Retained<T> {
    type Item = &'a T;
    type IntoIter = Box<dyn Iterator<Item = &'a T> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl<T: fmt::Debug> fmt::Debug for Retained<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Retained")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize as Counter, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    #[test]
    fn acquire_and_release_track_the_count() {
        let handle = RefCounted::new("payload");
        assert_eq!(handle.strong_count(), 1);
        let second = handle.acquire();
        assert_eq!(handle.strong_count(), 2);
        assert_eq!(second.strong_count(), 2);
        assert!(!second.release(), "not the last reference");
        assert_eq!(handle.strong_count(), 1);
        assert!(handle.release(), "the last reference");
    }

    #[test]
    fn cloning_takes_a_reference() {
        let handle = RefCounted::new(7);
        let clone = handle.clone();
        assert_eq!(handle.strong_count(), 2);
        assert_eq!(*clone, 7);
    }

    #[test]
    fn the_value_drops_when_the_last_handle_goes() {
        struct DropSpy(Arc<Counter>);
        impl Drop for DropSpy {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(Counter::new(0));
        let handle = RefCounted::new(DropSpy(Arc::clone(&drops)));
        let second = handle.acquire();
        drop(second);
        assert_eq!(drops.load(Ordering::SeqCst), 0, "still one handle alive");
        drop(handle);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "the last handle dropped it"
        );
    }

    #[test]
    fn try_unwrap_returns_the_value_or_the_handle() {
        let handle = RefCounted::new(String::from("only"));
        let value = handle.try_unwrap().expect("sole handle");
        assert_eq!(value, "only");

        let shared = RefCounted::new(String::from("shared"));
        let _keep = shared.acquire();
        let returned = shared.try_unwrap().expect_err("still shared");
        assert_eq!(shared_count(&returned), 2);
    }

    fn shared_count<T>(handle: &RefCounted<T>) -> usize {
        handle.strong_count()
    }

    #[test]
    fn retain_all_keeps_elements_alive_without_the_list() {
        let mut list = SnapshotList::new();
        for i in 0..100u32 {
            list.push(i);
        }
        let retained = list.retain_all();
        assert_eq!(retained.len(), 100);
        // Drop the list entirely; the retained batch still owns every element.
        drop(list);
        let mut seen: Vec<u32> = retained.iter().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..100u32).collect::<Vec<_>>());
    }

    #[test]
    fn iteration_runs_after_the_lock_is_released() {
        // The pattern under test: hold the list under a mutex, retain under the
        // lock, release the lock, then iterate. Another thread mutating the
        // list during the iteration must not be able to invalidate the snapshot.
        let list = Arc::new(Mutex::new(SnapshotList::new()));
        for i in 0..50u32 {
            list.lock().expect("lock").push(i);
        }
        let retained = {
            let guard = list.lock().expect("lock");
            guard.retain_all() // the lock ends here
        };
        // Mutate the live list while iterating the snapshot.
        {
            let mut guard = list.lock().expect("lock");
            for i in 100..150u32 {
                guard.push(i);
            }
        }
        let snapshot: Vec<u32> = retained.iter().copied().collect();
        assert_eq!(snapshot.len(), 50, "the snapshot is frozen");
        assert!(snapshot.iter().all(|n| *n < 100));
    }

    #[test]
    fn retained_handles_survive_removal_from_the_list() {
        let mut list = SnapshotList::new();
        let handle = list.push(String::from("kept"));
        let retained = list.retain_all();
        assert!(list.remove(&handle));
        assert!(list.is_empty());
        let values: Vec<&String> = retained.iter().collect();
        assert_eq!(values, vec![&String::from("kept")]);
    }

    #[test]
    fn concurrent_acquire_release_is_balanced() {
        let handle = RefCounted::new(vec![1u8, 2, 3]);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let handle = handle.acquire();
                thread::spawn(move || {
                    for _ in 0..10_000 {
                        let clone = handle.acquire();
                        assert_eq!(clone.get().len(), 3);
                        drop(clone);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("worker thread");
        }
        assert_eq!(handle.strong_count(), 1, "every acquire was released");
    }
}
