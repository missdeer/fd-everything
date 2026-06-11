//! `--type f/d/l/e/s/p/x`: decides solely from RawHit's `is_dir` and
//! `attributes`, with one PLAN-acknowledged caveat:
//!
//! * `executables_only` and `empty_only` would require `stat()`-style
//!   checks (`PathExt::executable()`, "is dir empty" enumeration). PLAN
//!   §3.2 forbids syscalls in the post-filter hot path, so for Phase 3
//!   these are **stubbed** to pass-through. Phase 5 will push them down
//!   to the backend (Everything index already knows file-empty via
//!   `size:0` and Windows executability via file-extension fallback).
//!
//! Mirrors `walk.rs:478-482` + `filetypes.rs::should_ignore`.

use crate::filetypes::FileTypes;
use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

/// Win32 reparse-point attribute. Mirrored locally to avoid pulling
/// `windows-sys` in just for one constant; Phase 5 will switch to the
/// real binding when the FFI crate lands.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub struct TypeFilter {
    types: FileTypes,
}

impl TypeFilter {
    pub fn new(types: FileTypes) -> Self {
        Self { types }
    }
}

impl Filter for TypeFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        let t = &self.types;
        let is_symlink = hit.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        let is_file = !hit.is_dir && !is_symlink;
        let is_dir = hit.is_dir && !is_symlink;

        // Drop any hit whose type wasn't asked for.
        if !t.files && is_file {
            return Verdict::Drop;
        }
        if !t.directories && is_dir {
            return Verdict::Drop;
        }
        if !t.symlinks && is_symlink {
            return Verdict::Drop;
        }
        // Windows has no block/char/socket/pipe file types on NTFS, so
        // those fields are effectively no-ops here; the Unix backend
        // (LegacyWalkerBackend) keeps using `walk.rs`'s stat-driven
        // FileTypes::should_ignore until Phase 5.

        // TODO(PLAN.md §3.2): push-down to backend.
        // `executables_only` and `empty_only` would require stat / dir
        // enumeration here, so they pass through. The EverythingBackend
        // can answer these from its index in Phase 5; the legacy walker
        // still applies them via `FileTypes::should_ignore` in
        // `walk.rs:478-482`.
        let _ = t.executables_only;
        let _ = t.empty_only;

        Verdict::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit(is_dir: bool, attrs: u32) -> RawHit {
        RawHit {
            path: PathBuf::from("/x"),
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

    /// Encodes `--type f` => files only. If this regresses,
    /// directories or symlinks leak into a file-only result set.
    #[test]
    fn files_only_drops_dirs_and_symlinks() {
        let types = FileTypes {
            files: true,
            ..Default::default()
        };
        let mut f = TypeFilter::new(types);
        assert_eq!(f.evaluate(&hit(false, 0)), Verdict::Keep);
        assert_eq!(f.evaluate(&hit(true, 0)), Verdict::Drop);
        assert_eq!(
            f.evaluate(&hit(false, FILE_ATTRIBUTE_REPARSE_POINT)),
            Verdict::Drop
        );
    }

    /// Encodes the Phase 3 stub for `executables_only`: it MUST be a
    /// pass-through, not a silent drop. If this regresses,
    /// `--type x` returns empty results on the EverythingBackend path.
    #[test]
    fn executables_only_is_passthrough_stub() {
        let types = FileTypes {
            files: true,
            executables_only: true,
            ..Default::default()
        };
        let mut f = TypeFilter::new(types);
        assert_eq!(f.evaluate(&hit(false, 0)), Verdict::Keep);
    }
}
