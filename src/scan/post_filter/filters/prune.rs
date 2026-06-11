//! `--prune`: when a directory hit survives every upstream filter,
//! emit it but suppress every descendant in the result set. PLAN.md
//! §3.1 describes the tradeoff: Everything returns hits in
//! index-implementation order, so we can't reject descendants until
//! their parent is known. The filter buffers everything to a
//! [`BTreeMap`] (parent paths sort before child paths), and flushes
//! through [`Filter::drain`] after the backend finishes. PLAN
//! explicitly accepts the latency hit *only* when `--prune` is on; the
//! pipeline builder omits this filter from the chain otherwise.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct PruneFilter {
    buffer: BTreeMap<PathBuf, RawHit>,
}

impl PruneFilter {
    pub fn new() -> Self {
        Self {
            buffer: BTreeMap::new(),
        }
    }
}

impl Default for PruneFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter for PruneFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        // Buffer every survivor; the actual prune decision happens on
        // drain when the full set is in hand. Verdict::Drop here means
        // "we'll re-emit it later", not "discard forever".
        self.buffer.insert(hit.path.clone(), hit.clone());
        Verdict::Drop
    }

    fn drain(&mut self) -> Vec<RawHit> {
        let buffer = std::mem::take(&mut self.buffer);
        let mut emitted_dirs: Vec<PathBuf> = Vec::new();
        let mut out = Vec::with_capacity(buffer.len());
        // BTreeMap iterates ascending, so parents always come before
        // their descendants — descendant suppression is a single
        // prefix check against the most recent emitted dir.
        for (path, hit) in buffer {
            if emitted_dirs
                .iter()
                .any(|d| path.starts_with(d) && &path != d)
            {
                continue;
            }
            if hit.is_dir {
                emitted_dirs.push(path.clone());
            }
            out.push(hit);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn dir(path: &str) -> RawHit {
        RawHit {
            path: PathBuf::from(path),
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

    fn file(path: &str) -> RawHit {
        RawHit {
            is_dir: false,
            ..dir(path)
        }
    }

    /// Encodes the `--prune` contract: matched dir is kept; every
    /// descendant inside it is dropped on drain. If this regresses,
    /// `--prune` users see duplicated trees.
    #[test]
    fn matched_dir_keeps_self_drops_descendants() {
        let mut f = PruneFilter::new();
        // Backend feeds them in arbitrary order — make sure children
        // arrive before parent to prove the BTreeMap ordering matters.
        let _ = f.evaluate(&file("/a/b/c.rs"));
        let _ = f.evaluate(&file("/a/b/d.rs"));
        let _ = f.evaluate(&dir("/a/b"));
        let _ = f.evaluate(&file("/a/sibling.rs"));

        let out = f.drain();
        let paths: Vec<_> = out.iter().map(|h| h.path.clone()).collect();
        assert_eq!(
            paths,
            vec![PathBuf::from("/a/b"), PathBuf::from("/a/sibling.rs"),],
            "matched dir kept; its descendants dropped; siblings survive"
        );
    }
}
