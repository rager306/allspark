//! The cold-start index build's single-flight guard.
//!
//! Split from `graph.rs` only because the guard type is shared by the tool and
//! its tests; the flag it releases lives on `GraphTool`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Releases the in-flight flag on drop, so a panicked or errored build cannot
/// wedge the tool into "a build is running" forever.
pub(crate) struct InFlightGuard(pub Arc<AtomicBool>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}
