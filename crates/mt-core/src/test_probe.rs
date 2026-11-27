//! Counters a test reads to prove that UI-thread paths resolve nothing.
//!
//! A `canonicalize` on a slow or disconnected volume blocks its caller, so
//! the live-disk paths must reach one only off the UI thread. The counter is
//! process-wide and present only in test builds.

use std::sync::atomic::{AtomicUsize, Ordering};

static PATH_RESOLUTIONS: AtomicUsize = AtomicUsize::new(0);

/// Records one path resolution (`canonicalize`) attempt.
pub fn note_path_resolution() {
    PATH_RESOLUTIONS.fetch_add(1, Ordering::Relaxed);
}

/// How many path resolution attempts were recorded.
pub fn path_resolutions() -> usize {
    PATH_RESOLUTIONS.load(Ordering::Relaxed)
}
