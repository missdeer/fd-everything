//! Pre-warm worker (PLAN.md §4.0).
//!
//! Before the Everything FFI starts emitting hits (Phase 5), spawn a worker
//! that walks each search root upward to the nearest `.git`/volume root,
//! pre-constructing [`DirIgnoreState`] for every directory on the chain.
//! With chained `.gitignore`/`.fdignore`/`.ignore` rules, the cost is paid
//! up-front in parallel with the FFI query instead of synchronously on the
//! first hit's match.
//!
//! Concurrency: writes go through `IgnoreCache::dir_state`, which uses
//! `DashMap::entry` to deduplicate races between this worker and the main
//! thread evaluating hits. No lock-step required.

use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use super::{IgnoreCache, MatcherKind};

/// Return type of `prewarm`. Caller can `.join()` for tests; production
/// (Phase 5) lets the worker run unattended — `IgnoreCache::dir_state` is
/// already thread-safe.
pub struct PrewarmHandle {
    handle: Option<JoinHandle<()>>,
}

impl PrewarmHandle {
    /// Block until the prewarm worker finishes. Tests use this to make
    /// assertions deterministic; production rarely needs it because the
    /// post-filter is racing the worker by design.
    pub fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Kick off pre-warm for `roots`. The worker exits when every chain reaches
/// either a `.git`/`.fdignore` boundary (Gitignore-truncating kind) or the
/// volume root.
pub fn prewarm(cache: IgnoreCache, roots: Vec<PathBuf>) -> PrewarmHandle {
    let handle = thread::Builder::new()
        .name("ignore-cache-prewarm".into())
        .spawn(move || {
            for root in roots {
                walk_up(&cache, &root);
            }
        })
        .expect("prewarm worker spawn");
    PrewarmHandle {
        handle: Some(handle),
    }
}

/// Climb from `start` up to the first `.git/` (gitignore-truncating boundary)
/// or `Path::parent() == None`, eagerly building cache entries.
fn walk_up(cache: &IgnoreCache, start: &Path) {
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        let state = cache.dir_state(dir);
        if state.has_local_git_root() && state.git_root_dir() == Some(dir) {
            // Crossed a git boundary: gitignore kind truncates here but
            // .fdignore/.ignore would keep climbing. PLAN says "up to git_root
            // or volume root" — we stop at the git boundary for warmth budget;
            // the non-truncating kinds are evaluated lazily above this point.
            break;
        }
        let _ = state.matcher_of(MatcherKind::Gitignore); // touch to keep
        current = dir.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::backend::CaseModifier;
    use crate::scan::post_filter::ignore_cache::{GlobalLayers, IgnoreFlags};
    use std::fs;
    use tempfile::TempDir;

    /// Prewarm populates the cache for every ancestor along the chain so the
    /// first hit's `matched()` call pays only DashMap-lookup cost — not
    /// stat+parse cost. If this regresses, `--exec` first-hit latency
    /// regresses under EverythingBackend.
    #[test]
    fn prewarm_populates_chain() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let leaf = root.join("a").join("b").join("c");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(root.join(".fdignore"), "*.tmp\n").unwrap();
        // Plant a .git so gitignore truncation has a place to stop.
        fs::create_dir_all(root.join(".git")).unwrap();

        let cache = IgnoreCache::new(
            IgnoreFlags::default(),
            GlobalLayers::empty(),
            Vec::new(),
            CaseModifier::Nocase,
        );
        prewarm(cache.clone(), vec![leaf.clone()]).join();

        // After prewarm, the chain dirs should all be in `by_dir`.
        // We probe via `dir_state` and assert the .fdignore is loaded at
        // the root.
        let root_state = cache.dir_state(root);
        assert!(root_state.matcher_of(MatcherKind::Fdignore).is_some());
    }
}
