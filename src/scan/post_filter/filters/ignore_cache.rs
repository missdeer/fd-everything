//! Phase 4 wiring: connect the real [`IgnoreCache`] to the filter chain.
//!
//! PLAN.md §Phase 3 reserved a stub for this filter; Phase 4 (now) lands the
//! real evaluator. The filter holds a cheap-clone [`IgnoreCache`] handle and
//! delegates per-hit decisions to its `matched()` driver.

use crate::scan::backend::RawHit;
use crate::scan::post_filter::ignore_cache::{Decision, IgnoreCache};
use crate::scan::post_filter::{Filter, Verdict};

/// Filter wrapper. Construction is cheap (Arc clone); construct one per
/// pipeline.
pub struct IgnoreCacheFilter {
    cache: IgnoreCache,
}

impl IgnoreCacheFilter {
    pub fn new(cache: IgnoreCache) -> Self {
        Self { cache }
    }
}

impl Filter for IgnoreCacheFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if !self.cache.flags().any_enabled() {
            return Verdict::Keep;
        }
        match self.cache.matched(&hit.path, hit.is_dir, &hit.search_root) {
            Decision::Show => Verdict::Keep,
            Decision::Hide => Verdict::Drop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::backend::CaseModifier;
    use crate::scan::post_filter::ignore_cache::{GlobalLayers, IgnoreFlags};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn hit_at(path: PathBuf, root: Arc<PathBuf>) -> RawHit {
        RawHit {
            path,
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

    /// Smoke test: the filter delegates correctly to IgnoreCache. If this
    /// regresses, Phase-5 wiring inherits a broken filter and every
    /// production hit gets miscategorized.
    #[test]
    fn filter_drops_hit_matched_by_gitignore() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();

        let cache = IgnoreCache::new(
            IgnoreFlags::default(),
            GlobalLayers::empty(),
            Vec::new(),
            CaseModifier::Nocase,
        );
        let mut filter = IgnoreCacheFilter::new(cache);
        let root_arc = Arc::new(root.to_path_buf());
        let dropped = hit_at(root.join("app.log"), Arc::clone(&root_arc));
        let kept = hit_at(root.join("app.txt"), Arc::clone(&root_arc));

        assert_eq!(filter.evaluate(&dropped), Verdict::Drop);
        assert_eq!(filter.evaluate(&kept), Verdict::Keep);
    }

    /// When flags are all off, the filter MUST short-circuit Keep without
    /// touching the cache. Phase-5 callers may still construct it (for
    /// uniform pipeline shape); the cost must stay near-zero.
    #[test]
    fn filter_keeps_everything_with_flags_off() {
        let cache = IgnoreCache::new(
            IgnoreFlags {
                read_fdignore: false,
                read_vcsignore: false,
                ..IgnoreFlags::default()
            },
            GlobalLayers::empty(),
            Vec::new(),
            CaseModifier::Nocase,
        );
        let mut filter = IgnoreCacheFilter::new(cache);
        let root = Arc::new(PathBuf::from(r"C:\repo"));
        let h = hit_at(PathBuf::from(r"C:\repo\anything.log"), Arc::clone(&root));
        assert_eq!(filter.evaluate(&h), Verdict::Keep);
    }
}
