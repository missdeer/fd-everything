//! `--one-file-system`: drop any hit whose volume serial number
//! differs from the search root it came from.
//!
//! Implementation strategy (PLAN.md §3.3): on the first hit per
//! search root, look up the root's volume ID via the injected
//! [`VolumeIdProvider`] and remember it. For each subsequent hit, look
//! up the hit's volume ID and compare. The provider abstraction lets
//! the unit tests skip the actual syscall by injecting a deterministic
//! map; production will plug in a Win32 (`GetVolumeInformationW`) or
//! Unix (`statvfs.f_fsid`) implementation in Phase 5 once those
//! dependencies are wired in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

/// Resolve a path to its filesystem volume ID. `None` = lookup failed
/// (treated as "same volume", i.e. don't drop — fail-open so the user
/// doesn't lose hits to a syscall error).
pub trait VolumeIdProvider: Send {
    fn volume_id(&self, path: &Path) -> Option<u64>;
}

/// Production no-op provider. Phase 5 swaps this for a real Win32
/// implementation once `windows-sys` lands in `Cargo.toml`. Until then
/// the filter is fail-open — equivalent to today's behavior on Windows
/// where `--one-file-system` is a no-op anyway.
pub struct NoopVolumeIdProvider;

impl VolumeIdProvider for NoopVolumeIdProvider {
    fn volume_id(&self, _: &Path) -> Option<u64> {
        None
    }
}

pub struct SameFilesystemFilter {
    provider: Box<dyn VolumeIdProvider>,
    /// Volume ID per search root, populated lazily on first hit. `Arc`
    /// match avoids a deep PathBuf compare every hit.
    root_volume: HashMap<*const PathBuf, Option<u64>>,
    /// Hit-path volume cache keyed by parent directory; same path
    /// shares a volume so repeated lookups in a hot directory collapse
    /// to one cache hit.
    path_volume: HashMap<PathBuf, Option<u64>>,
}

// SAFETY: `*const PathBuf` is used only as a hash key derived from
// `Arc::as_ptr`. We never deref it; the Arcs themselves live as long
// as the producing backend, which outlives the filter.
unsafe impl Send for SameFilesystemFilter {}

impl SameFilesystemFilter {
    pub fn new(provider: Box<dyn VolumeIdProvider>) -> Self {
        Self {
            provider,
            root_volume: HashMap::new(),
            path_volume: HashMap::new(),
        }
    }

    fn root_volume_id(&mut self, root: &Arc<PathBuf>) -> Option<u64> {
        let key = Arc::as_ptr(root);
        if let Some(v) = self.root_volume.get(&key) {
            return *v;
        }
        let v = self.provider.volume_id(root.as_ref());
        self.root_volume.insert(key, v);
        v
    }

    fn hit_volume_id(&mut self, hit_path: &Path) -> Option<u64> {
        // Cache by parent dir — a single directory all shares one
        // volume on Windows / Unix.
        let parent = hit_path.parent().unwrap_or(hit_path);
        if let Some(v) = self.path_volume.get(parent) {
            return *v;
        }
        let v = self.provider.volume_id(parent);
        self.path_volume.insert(parent.to_path_buf(), v);
        v
    }
}

impl Filter for SameFilesystemFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        let root_vol = self.root_volume_id(&hit.search_root);
        let root_vol = match root_vol {
            Some(v) => v,
            None => return Verdict::Keep, // fail-open
        };
        let hit_vol = self.hit_volume_id(&hit.path);
        match hit_vol {
            Some(v) if v != root_vol => Verdict::Drop,
            _ => Verdict::Keep,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Test provider that returns canned IDs based on a path-prefix
    /// table. Wrapped in RefCell so the closure can capture the
    /// mapping; `unsafe impl Send` because tests run single-threaded.
    struct FakeProvider {
        table: RefCell<Vec<(PathBuf, u64)>>,
    }
    unsafe impl Send for FakeProvider {}

    impl FakeProvider {
        fn new(table: Vec<(PathBuf, u64)>) -> Self {
            Self {
                table: RefCell::new(table),
            }
        }
    }

    impl VolumeIdProvider for FakeProvider {
        fn volume_id(&self, p: &Path) -> Option<u64> {
            self.table
                .borrow()
                .iter()
                .find(|(prefix, _)| p.starts_with(prefix))
                .map(|(_, id)| *id)
        }
    }

    fn hit_with_root(path: &str, root: Arc<PathBuf>) -> RawHit {
        RawHit {
            path: PathBuf::from(path),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: root,
        }
    }

    /// Encodes the cross-volume drop. PLAN.md §3.3: a hit whose volume
    /// differs from the root must drop — e.g. Windows junctions
    /// landing on a different drive letter.
    #[test]
    fn drops_hits_on_other_volume() {
        let root = Arc::new(PathBuf::from("/C/repo"));
        let provider = Box::new(FakeProvider::new(vec![
            (PathBuf::from("/C"), 1),
            (PathBuf::from("/D"), 2),
        ]));
        let mut f = SameFilesystemFilter::new(provider);
        assert_eq!(
            f.evaluate(&hit_with_root("/C/repo/a.rs", Arc::clone(&root))),
            Verdict::Keep
        );
        assert_eq!(
            f.evaluate(&hit_with_root("/D/mount/x.rs", Arc::clone(&root))),
            Verdict::Drop
        );
    }

    /// Encodes fail-open semantics: a provider that returns `None` for
    /// the root volume must NOT drop — losing all hits because of a
    /// transient syscall error would be a much worse regression than
    /// occasionally including a junction.
    #[test]
    fn fail_open_when_root_lookup_fails() {
        let root = Arc::new(PathBuf::from("/C/repo"));
        let provider = Box::new(NoopVolumeIdProvider);
        let mut f = SameFilesystemFilter::new(provider);
        assert_eq!(
            f.evaluate(&hit_with_root("/D/mount/x.rs", Arc::clone(&root))),
            Verdict::Keep
        );
    }
}
