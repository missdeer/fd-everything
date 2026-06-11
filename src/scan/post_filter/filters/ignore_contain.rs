//! `--ignore-contain`: when a directory contains any of the configured
//! marker files, drop the directory hit. (The descendants will arrive
//! later in the RawHit stream; they're discarded by the path-prefix
//! check below.)
//!
//! Order matters: PLAN.md §Phase 3 demands this run **before** any
//! depth or root check, because a marker placed in a search-root
//! directory must still suppress the whole subtree even if the user
//! restricts the depth window.
//!
//! Mirrors `walk.rs:402-408` semantics — there the check is done on
//! `WalkBuilder` directory-enter, here it's done lazily on the
//! directory hit itself, plus a prefix check on every subsequent hit.

use std::path::PathBuf;

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct IgnoreContainFilter {
    markers: Vec<String>,
    /// Directories whose marker check passed (i.e. the marker exists).
    /// Descendant hits whose path starts with one of these are dropped.
    /// Sorted by length to keep prefix checks linear on a typically
    /// small set; PLAN.md notes this is rarely a hot path.
    pruned_dirs: Vec<PathBuf>,
}

impl IgnoreContainFilter {
    pub fn new(markers: Vec<String>) -> Self {
        Self {
            markers,
            pruned_dirs: Vec::new(),
        }
    }
}

impl Filter for IgnoreContainFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if self.markers.is_empty() {
            return Verdict::Keep;
        }

        for pruned in &self.pruned_dirs {
            if hit.path.starts_with(pruned) {
                return Verdict::Drop;
            }
        }

        if hit.is_dir && self.markers.iter().any(|m| hit.path.join(m).exists()) {
            self.pruned_dirs.push(hit.path.clone());
            return Verdict::Drop;
        }

        Verdict::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn dir_hit(path: PathBuf) -> RawHit {
        RawHit {
            path,
            is_dir: true,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    fn file_hit(path: PathBuf) -> RawHit {
        RawHit {
            is_dir: false,
            ..dir_hit(path)
        }
    }

    /// Encodes PLAN.md §Phase 3 ordering: marker-bearing dir is
    /// dropped AND its descendants are dropped on subsequent visits.
    /// If this regresses, `--ignore-contain CACHEDIR.TAG` leaks cache
    /// trees into the result set.
    #[test]
    fn marker_dir_and_descendants_are_dropped() {
        let tmp = tempdir().unwrap();
        let cache_dir = tmp.path().join("cache");
        std::fs::create_dir(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("CACHEDIR.TAG"), "").unwrap();

        let mut f = IgnoreContainFilter::new(vec!["CACHEDIR.TAG".into()]);
        assert_eq!(f.evaluate(&dir_hit(cache_dir.clone())), Verdict::Drop);
        assert_eq!(
            f.evaluate(&file_hit(cache_dir.join("data.bin"))),
            Verdict::Drop,
            "descendants of a pruned dir must drop"
        );
    }

    /// Encodes the "no markers configured" fast path: zero-overhead
    /// when the user isn't using `--ignore-contain`.
    #[test]
    fn empty_marker_list_keeps_everything() {
        let mut f = IgnoreContainFilter::new(vec![]);
        assert_eq!(f.evaluate(&file_hit(PathBuf::from("/x/y"))), Verdict::Keep);
    }
}
