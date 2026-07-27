//! Hidden-by-name: drop hits whose name — OR any ancestor component
//! between the search root and the leaf — starts with `.`. Mirrors the
//! Unix dotfile convention that `ignore::WalkBuilder::hidden(true)`
//! enforces on the legacy walker: that walker never *descends* into a
//! dot-prefixed directory, so its children are never even seen. The
//! Everything backend, by contrast, returns every indexed entry — so
//! ancestor-aware filtering has to happen here. PLAN.md §Phase 3 v7.2
//! split this out from `hidden_by_attr` so the cheap byte check runs
//! before any `GetFileAttributesW` syscall.

use std::path::Component;

use crate::filesystem::strip_path_prefix;
use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct HiddenByNameFilter {
    enabled: bool,
}

impl HiddenByNameFilter {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl Filter for HiddenByNameFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if !self.enabled {
            return Verdict::Keep;
        }
        // Components ABOVE the search root don't count: the user may
        // explicitly pass a dot-prefixed root (`fd foo /home/u/.config`)
        // and expect its children to be walked, mirroring LegacyWalker.
        // Prefix removal can only fail if `path` isn't under `search_root`,
        // which the EverythingBackend prevents via the `path:"<root>"`
        // clause — fall back to scanning all components in that case.
        let rel =
            strip_path_prefix(&hit.path, hit.search_root.as_path()).unwrap_or(hit.path.as_path());
        for component in rel.components() {
            if let Component::Normal(name) = component {
                // First-byte check is enough for the dotfile convention;
                // '.' is ASCII so no UTF-8 decoding required.
                if name.as_encoded_bytes().first().copied() == Some(b'.') {
                    return Verdict::Drop;
                }
            }
        }
        Verdict::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit_with_root(path: &str, root: &str) -> RawHit {
        RawHit {
            path: PathBuf::from(path),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from(root)),
        }
    }

    fn hit(path: &str) -> RawHit {
        hit_with_root(path, "/repo")
    }

    /// Encodes the dotfile drop. If this regresses, `.git` directories
    /// would leak into the default result set.
    #[test]
    fn dotfiles_drop_when_enabled() {
        let mut f = HiddenByNameFilter::new(true);
        assert_eq!(f.evaluate(&hit("/repo/.git")), Verdict::Drop);
        assert_eq!(f.evaluate(&hit("/repo/git")), Verdict::Keep);
    }

    /// PLAN §Phase 8.7.1 MUST 2 regression: LegacyWalker never descends
    /// into `.git/`, so children like `.git/a.foo` are never visited.
    /// EverythingBackend returns every indexed entry, so ancestor-aware
    /// dot-prefix filtering MUST drop hits whose path has any
    /// dot-prefixed component between the search root and the leaf.
    /// Without this, `test_git_dir`'s `fd --no-ignore foo` leaks all
    /// four `.git/*` entries.
    #[test]
    fn dotfile_ancestor_drops_even_when_leaf_is_visible() {
        let mut f = HiddenByNameFilter::new(true);
        assert_eq!(
            f.evaluate(&hit_with_root("/repo/.git/a.foo", "/repo")),
            Verdict::Drop
        );
        assert_eq!(
            f.evaluate(&hit_with_root("/repo/nested/dir/.git/foo2", "/repo")),
            Verdict::Drop
        );
        assert_eq!(
            f.evaluate(&hit_with_root("/repo/nested/visible/foo", "/repo")),
            Verdict::Keep
        );
    }

    /// PLAN §Phase 8.7.1 MUST 2: an explicitly-named dot-prefixed search
    /// root (e.g. `fd foo /home/u/.config`) must NOT have its children
    /// treated as hidden — LegacyWalker walks them because the user
    /// pointed at them on purpose. Only components UNDER the root count.
    #[test]
    fn dotfile_above_search_root_does_not_drop() {
        let mut f = HiddenByNameFilter::new(true);
        assert_eq!(
            f.evaluate(&hit_with_root(
                "/home/u/.config/app.toml",
                "/home/u/.config"
            )),
            Verdict::Keep
        );
    }

    /// Disabled = pass-through, no syscalls.
    #[test]
    fn disabled_keeps_dotfiles() {
        let mut f = HiddenByNameFilter::new(false);
        assert_eq!(f.evaluate(&hit("/repo/.git")), Verdict::Keep);
    }
}
