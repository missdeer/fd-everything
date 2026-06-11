//! `--size`: reuses [`SizeFilter::is_within`] verbatim. The walker
//! version (`walk.rs:519-537`) gates on `entry_path.is_file()` — i.e.
//! follows symlinks and only keeps regular files. We mirror that here
//! by dropping both plain directories and any reparse-point entry
//! (symlinks / junctions / mount points). PLAN §Phase 8.7.1 MUST 2
//! tightened this: before, the filter only dropped `hit.is_dir`, which
//! after MUST 1 strips the directory bit from dir-symlinks made them
//! pass as `size=0` files and corrupted `--size 0B` results.

use crate::filter::SizeFilter;
use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

/// Win32 `FILE_ATTRIBUTE_REPARSE_POINT`. Mirrored locally — see the
/// matching definitions in `type_filter` / `symlink_filter`.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub struct SizeConstraints {
    constraints: Vec<SizeFilter>,
}

impl SizeConstraints {
    pub fn new(constraints: Vec<SizeFilter>) -> Self {
        Self { constraints }
    }
}

impl Filter for SizeConstraints {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if self.constraints.is_empty() {
            return Verdict::Keep;
        }
        // Regular-file-only, matching `walk.rs::is_file()` semantics:
        // drop directories and drop reparse points. For symlink-to-file
        // the LegacyWalker would follow and apply the constraint to the
        // target's size; we drop instead. This is a known minor
        // divergence — see PLAN §Phase 8.7.1 deferral list — that
        // matches the only symlinks we see in the test fixtures
        // (symlink → directory).
        if hit.is_dir || (hit.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0) {
            return Verdict::Drop;
        }
        if self.constraints.iter().all(|c| c.is_within(hit.size)) {
            Verdict::Keep
        } else {
            Verdict::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit(size: u64, is_dir: bool) -> RawHit {
        RawHit {
            path: PathBuf::from("/x"),
            is_dir,
            size,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// Encodes the `--size +1k` contract: a 2 KB file passes, a 500 B
    /// file drops, a directory of any size is dropped (`walk.rs:519-537`
    /// parity).
    #[test]
    fn size_constraint_uses_raw_hit_size() {
        let min = SizeFilter::from_string("+1k").unwrap();
        let mut f = SizeConstraints::new(vec![min]);
        assert_eq!(f.evaluate(&hit(2000, false)), Verdict::Keep);
        assert_eq!(f.evaluate(&hit(500, false)), Verdict::Drop);
        assert_eq!(f.evaluate(&hit(10_000, true)), Verdict::Drop);
    }

    /// PLAN §Phase 8.7.1 MUST 2 regression: a directory symlink (after
    /// MUST 1 strips its dir bit) reports `is_dir=false, size=0` and
    /// would otherwise satisfy `--size 0B`. LegacyWalker's `is_file()`
    /// gate follows the link, sees a directory, and drops it. We mirror
    /// that by checking the reparse-point attribute. If this regresses,
    /// `test_size`'s `--size 0B` over-counts every directory symlink.
    #[test]
    fn directory_symlink_does_not_pass_zero_byte_constraint() {
        let zero = SizeFilter::from_string("0B").unwrap();
        let mut f = SizeConstraints::new(vec![zero]);
        let mut sym = hit(0, false);
        sym.attributes = FILE_ATTRIBUTE_REPARSE_POINT;
        assert_eq!(f.evaluate(&sym), Verdict::Drop);
    }

    /// Empty constraint list = pass everything (preserves
    /// `--size`-not-used fast path).
    #[test]
    fn empty_constraints_keep_everything() {
        let mut f = SizeConstraints::new(vec![]);
        assert_eq!(f.evaluate(&hit(0, false)), Verdict::Keep);
        assert_eq!(f.evaluate(&hit(0, true)), Verdict::Keep);
    }
}
