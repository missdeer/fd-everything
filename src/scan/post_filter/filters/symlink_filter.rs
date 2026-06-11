//! When `Config::follow_links` is false, drop hits that live under a
//! reparse-point directory. The legacy walker handles this via
//! `WalkBuilder::follow_links(false)`; on the EverythingBackend path
//! we don't get traversal hooks, so we identify reparse-point
//! directories from `RawHit::attributes & FILE_ATTRIBUTE_REPARSE_POINT`
//! and remember them for prefix-suppression of descendants.

use std::path::PathBuf;

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub struct SymlinkFilter {
    enabled: bool,
    /// Sorted by length descending so the longest prefix wins on a tie
    /// (an inner symlink dir inside an outer symlink dir).
    reparse_dirs: Vec<PathBuf>,
}

impl SymlinkFilter {
    /// `follow_links == true` means **don't** filter — pass-through.
    pub fn new(follow_links: bool) -> Self {
        Self {
            enabled: !follow_links,
            reparse_dirs: Vec::new(),
        }
    }
}

impl Filter for SymlinkFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if !self.enabled {
            return Verdict::Keep;
        }

        for d in &self.reparse_dirs {
            if hit.path.starts_with(d) && hit.path != *d {
                return Verdict::Drop;
            }
        }

        if hit.is_dir && hit.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            // Remember this dir, but emit it once — matches `walk.rs`
            // which yields the symlink itself before refusing to follow.
            self.reparse_dirs.push(hit.path.clone());
            self.reparse_dirs
                .sort_by_key(|d| std::cmp::Reverse(d.as_os_str().len()));
        }
        Verdict::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn hit(path: &str, is_dir: bool, attrs: u32) -> RawHit {
        RawHit {
            path: PathBuf::from(path),
            is_dir,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: attrs,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// Encodes "don't follow symlinks": the symlink dir is reported
    /// once, but its contents drop. Regression here means a circular
    /// junction can loop the search forever (the walker would catch it
    /// at recursion limit; post-filter has no such limit).
    #[test]
    fn descendants_of_reparse_dir_drop() {
        let mut f = SymlinkFilter::new(false);
        assert_eq!(
            f.evaluate(&hit("/link", true, FILE_ATTRIBUTE_REPARSE_POINT)),
            Verdict::Keep,
            "the symlink dir itself is emitted once"
        );
        assert_eq!(f.evaluate(&hit("/link/inside.rs", false, 0)), Verdict::Drop);
    }

    /// follow_links=true bypasses the filter entirely.
    #[test]
    fn follow_links_disables_filter() {
        let mut f = SymlinkFilter::new(true);
        assert_eq!(
            f.evaluate(&hit("/link", true, FILE_ATTRIBUTE_REPARSE_POINT)),
            Verdict::Keep
        );
        assert_eq!(f.evaluate(&hit("/link/inside.rs", false, 0)), Verdict::Keep);
    }
}
