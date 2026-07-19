//! PLAN.md §Phase 2 v7.1: `DirEntry` holds a single canonical absolute
//! `raw_path`. Everything that needs the on-disk path (stat,
//! GetFileAttributesW, IgnoreCache, hyperlink target, size/time/owner
//! filters, volume id) reads `raw_path` directly. The user-facing display
//! form is derived on demand at output time by `PathProjector`; this
//! struct intentionally does not carry a display field (see PLAN.md §2.X
//! consumer table).

use std::cell::OnceCell;
use std::ffi::OsString;
use std::fs::{FileType, Metadata};
use std::path::{Path, PathBuf};

use lscolors::{Colorable, LsColors, Style};

use crate::scan::backend::RawHit;

#[derive(Debug)]
enum DirEntryInner {
    /// Wraps the legacy `ignore::DirEntry`. Kept so `metadata()`,
    /// `file_type()` and `depth()` can stay lazy and reuse the work the
    /// walker already did during traversal (PLAN.md P3 hot-path
    /// zero-allocation principle).
    Normal(ignore::DirEntry),
    /// `WalkBuilder` flagged a path as a broken symlink. Metadata has to
    /// come from `symlink_metadata(&raw_path)` since there is no inner
    /// `ignore::DirEntry` to consult.
    BrokenSymlink,
    /// PLAN.md §Phase 3: a hit emitted by a `SearchBackend` (e.g.
    /// EverythingBackend) that has already populated every metadata
    /// field on `RawHit`. Output / hyperlink / color consumers read
    /// directly from the cached fields — the only `std::fs` syscall
    /// that can still happen is the `OnceCell<Option<Metadata>>` lazy
    /// fallback for callers that demand a real `Metadata`, kept so we
    /// don't have to teach every consumer about `RawHit` semantics.
    ///
    /// Invariant: the boxed `RawHit`'s `path` field is **empty** —
    /// `from_raw_hit` moves it out into `DirEntry::raw_path` to avoid
    /// a per-hit clone. Read paths through `DirEntry::path()`, never
    /// through `inner` directly. The remaining `RawHit` fields
    /// (`is_dir`, `depth`, `attributes`, `extension`, etc.) stay
    /// valid.
    //
    // No binary consumer until Phase 5 wires `post_filter` into
    // `walk.rs`; the variant is exercised by the `scan::post_filter`
    // unit tests.
    #[allow(dead_code)]
    RawHit(Box<RawHit>),
}

#[derive(Debug)]
pub struct DirEntry {
    /// Canonicalized absolute path. The single source of truth for every
    /// internal consumer (PLAN.md §2.X).
    raw_path: PathBuf,
    inner: DirEntryInner,
    metadata: OnceCell<Option<Metadata>>,
    style: OnceCell<Option<Style>>,
}

impl DirEntry {
    /// Wrap a hit from the legacy `ignore::WalkBuilder`. `cwd` is the
    /// scan-time current directory (cached once in `walk::WorkerState`)
    /// used to canonicalize relative paths returned by `ignore::DirEntry`
    /// — without it we'd pay an env::current_dir() syscall per hit.
    #[inline]
    pub fn normal(e: ignore::DirEntry, cwd: &Path) -> Self {
        let raw_path = make_absolute(e.path(), cwd);
        Self {
            raw_path,
            inner: DirEntryInner::Normal(e),
            metadata: OnceCell::new(),
            style: OnceCell::new(),
        }
    }

    /// Wrap a path the walker flagged as a broken symlink. Same
    /// canonicalization contract as `normal`.
    pub fn broken_symlink(path: PathBuf, cwd: &Path) -> Self {
        let raw_path = make_absolute(&path, cwd);
        Self {
            raw_path,
            inner: DirEntryInner::BrokenSymlink,
            metadata: OnceCell::new(),
            style: OnceCell::new(),
        }
    }

    /// PLAN.md §Phase 3: wrap a `RawHit` produced by a search backend.
    /// `RawHit.path` is already absolute + canonical (Phase 1 contract),
    /// so no `cwd` join is required and no syscall fires here.
    //
    // Binary consumer arrives in Phase 5 (`scan::run_with_backend`).
    #[allow(dead_code)]
    pub fn from_raw_hit(mut hit: RawHit) -> Self {
        // RawHit.path is the only consumer-visible source of the path;
        // moving it out into `raw_path` and leaving the boxed `RawHit`'s
        // `path` field empty avoids a per-hit PathBuf clone on the hot
        // path. None of the surviving accessors (`file_type`,
        // `metadata`, `depth`, `is_directory_for_display`) read
        // `hit.path`.
        let raw_path = std::mem::take(&mut hit.path);
        Self {
            raw_path,
            inner: DirEntryInner::RawHit(Box::new(hit)),
            metadata: OnceCell::new(),
            style: OnceCell::new(),
        }
    }

    /// Canonical absolute path. Use this for stat, IgnoreCache,
    /// hyperlink targets, etc. For user-visible output, go through
    /// `Config::path_projector()` instead.
    pub fn path(&self) -> &Path {
        &self.raw_path
    }

    #[allow(dead_code)]
    pub fn into_path(self) -> PathBuf {
        self.raw_path
    }

    pub fn file_type(&self) -> Option<FileType> {
        match &self.inner {
            DirEntryInner::Normal(e) => e.file_type(),
            DirEntryInner::BrokenSymlink => self.metadata().map(|m| m.file_type()),
            // RawHit carries `is_dir` + `attributes` but Rust's
            // `FileType` is opaque (the only constructors live behind
            // `MetadataExt`), so we fall back to a real syscall through
            // the same lazy `OnceCell` path. Phase 5 will revisit if
            // hot-path callers need a stat-free answer.
            DirEntryInner::RawHit(_) => self.metadata().map(|m| m.file_type()),
        }
    }

    /// Stat-free "should output append a trailing path separator?". Uses
    /// the walker-supplied file-type for legacy entries and the cached
    /// `RawHit::is_dir` bit (which Phase 8.7.1 MUST 1 corrects to be
    /// false for directory symlinks / junctions) for backend entries. The
    /// plain `file_type()` path on a RawHit would syscall through
    /// `Path::metadata()`, which *follows* symlinks and would re-mark
    /// directory symlinks as directories — defeating MUST 1 at the output
    /// layer and producing `link\` instead of `link` for `--type l`.
    pub fn is_directory_for_display(&self) -> bool {
        match &self.inner {
            DirEntryInner::Normal(e) => e.file_type().is_some_and(|ft| ft.is_dir()),
            DirEntryInner::BrokenSymlink => false,
            DirEntryInner::RawHit(hit) => hit.is_dir,
        }
    }

    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata
            .get_or_init(|| match &self.inner {
                DirEntryInner::Normal(e) => e.metadata().ok(),
                DirEntryInner::BrokenSymlink => self.raw_path.symlink_metadata().ok(),
                // RawHit doesn't carry a `Metadata` (it carries the
                // individual fields). Consumers that want a real
                // `Metadata` (e.g. owner / size filter in Unix paths
                // not yet ported) pay one syscall here.
                DirEntryInner::RawHit(_) => self.raw_path.metadata().ok(),
            })
            .as_ref()
    }

    pub fn depth(&self) -> Option<usize> {
        match &self.inner {
            DirEntryInner::Normal(e) => Some(e.depth()),
            DirEntryInner::BrokenSymlink => None,
            DirEntryInner::RawHit(hit) => Some(hit.depth),
        }
    }

    pub fn style(&self, ls_colors: &LsColors) -> Option<&Style> {
        self.style
            .get_or_init(|| ls_colors.style_for(self).cloned())
            .as_ref()
    }
}

/// Join a possibly-relative `path` onto `cwd`, stripping a leading
/// `"."` component to avoid `./foo` → `/cwd/./foo`. Mirrors the rules
/// used by `filesystem::path_absolute_form` but takes a borrowed `cwd`
/// so the caller can cache it (one env::current_dir() per scan, not per
/// hit).
fn make_absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let path = path.strip_prefix(".").unwrap_or(path);
    cwd.join(path)
}

/// Ported from upstream `stripped_path` (sharkdp/fd PR #2011): when stripping
/// the leading `./` would leave a path starting with `-`, downstream tools may
/// misinterpret it as an option. The projector honours this via
/// `PathProjector::project_for_output` under `--strip-cwd-prefix`.
pub(crate) fn starts_with_dash(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().first() == Some(&b'-')
}

impl PartialEq for DirEntry {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.path() == other.path()
    }
}

impl Eq for DirEntry {}

impl PartialOrd for DirEntry {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DirEntry {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path().cmp(other.path())
    }
}

impl Colorable for DirEntry {
    fn path(&self) -> PathBuf {
        self.path().to_owned()
    }

    fn file_name(&self) -> OsString {
        // For Normal entries the walker's basename is authoritative
        // (handles e.g. trailing-slash quirks the same way). For broken
        // symlinks we fall back to the last component of raw_path —
        // Path::file_name() rejects non-Normal components like `..`, so
        // we open-code the access, copying the LsColors fallback.
        match &self.inner {
            DirEntryInner::Normal(e) => e.file_name().to_owned(),
            DirEntryInner::BrokenSymlink | DirEntryInner::RawHit(_) => self
                .raw_path
                .components()
                .next_back()
                .map(|c| c.as_os_str().to_owned())
                .unwrap_or_else(|| self.raw_path.as_os_str().to_owned()),
        }
    }

    fn file_type(&self) -> Option<FileType> {
        self.file_type()
    }

    fn metadata(&self) -> Option<Metadata> {
        self.metadata().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::starts_with_dash;
    use std::path::Path;

    #[test]
    fn dash_prefixed_paths_detected() {
        assert!(starts_with_dash(Path::new("-rf")));
        assert!(starts_with_dash(Path::new("--delete")));
        assert!(starts_with_dash(Path::new("-")));
    }

    #[test]
    fn safe_paths_not_flagged() {
        assert!(!starts_with_dash(Path::new("foo")));
        assert!(!starts_with_dash(Path::new("./foo")));
        assert!(!starts_with_dash(Path::new("sub/-rf")));
        assert!(!starts_with_dash(Path::new("")));
        assert!(!starts_with_dash(Path::new(" -rf")));
    }
}
