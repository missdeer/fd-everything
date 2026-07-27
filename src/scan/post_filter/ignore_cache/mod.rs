//! `IgnoreCache` — PLAN.md §Phase 4.
//!
//! Replays fd's gitignore/fdignore/dotignore semantics against a stream of
//! `RawHit`s **without** depending on `ignore::WalkBuilder`. The walker
//! interleaves traversal and ignore evaluation; with the Everything backend
//! the hit stream is flat and unordered, so the post-filter must reconstruct
//! the per-directory matcher chain on demand.
//!
//! ## Layout
//!
//! - `mod.rs` — public `IgnoreCache`, `Decision`, `IgnoreFlags`, top-level
//!   `matched()` driver and `MatcherKind` ordering.
//! - `dir_state.rs` — `DirIgnoreState` and `MatcherLayer`. Builds per-dir
//!   matchers by scanning for `.gitignore` / `.fdignore` / `.ignore` /
//!   `.git/info/exclude`; caches result in `Arc<DirIgnoreState>` keyed by
//!   directory path (PLAN.md §4.3 Arc parent-chain caching).
//! - `global.rs` — git `core.excludesFile`, fd global ignore, `--ignore-file`.
//! - `pushdown.rs` — PLAN.md §4.7 static gitignore pushdown helper.
//! - `prewarm.rs` — PLAN.md §4.0 background warmup worker.
//! - `parallel.rs` — PLAN.md §4.8 adaptive chunked parallel driver.
//!
//! ## Algorithm (PLAN.md §4.2 #7, v4 model)
//!
//! For a path `p`:
//!
//! ```text
//! chain = [leaf_dir(p), parent(leaf_dir(p)), ..., search_root, ..., volume_root]
//! for kind in [Custom, DotIgnore, Fdignore, Gitignore, GitInfoExclude]:
//!     for dir in chain:                       // leaf-first within each kind
//!         if let Some(matcher) = dir.matcher_of(kind):
//!             match matcher.matched(rel(p, dir), is_dir):
//!                 Whitelist => return Show
//!                 Ignore    => return Hide
//!                 None      => continue       // same kind, keep climbing
//!     // kind exhausted -> next kind
//! evaluate global layers (FdGlobal, GitGlobal) under the same kind priority
//! ```
//!
//! `Gitignore` truncates at the nearest enclosing `.git/` directory; the other
//! kinds keep climbing. See the `MatcherKind::truncates_at_git_root` predicate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::filesystem::paths_equal;

use dashmap::DashMap;

pub mod dir_state;
pub mod global;
pub mod parallel;
pub mod prewarm;
pub mod pushdown;

pub use dir_state::DirIgnoreState;
#[allow(unused_imports)]
pub use dir_state::MatcherLayer;
pub use global::GlobalLayers;

use crate::scan::backend::CaseModifier;

/// Outcome of a single path / ignore-cache query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// No ignore rule applies, or a whitelist `!` rule explicitly un-ignored
    /// the path. Caller should emit the hit.
    Show,
    /// At least one ignore rule fires on the path or one of its ancestors.
    Hide,
}

/// Matcher source. Ordering (low discriminant = higher priority) drives
/// the `for kind in [Custom, DotIgnore, Fdignore, Gitignore, GitInfoExclude]`
/// outer loop in `matched()`.
///
/// The priority order is taken verbatim from PLAN.md §4.2 #7 and reverse-
/// engineered from the `test_custom_ignore_precedence` fixture
/// (`tests.rs:837`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(usize)]
pub enum MatcherKind {
    /// `--ignore-file <path>` (global) — currently only at global layer.
    Custom = 0,
    /// `.ignore` (any tool, never VCS-specific).
    DotIgnore = 1,
    /// `.fdignore` (fd-specific).
    Fdignore = 2,
    /// `.gitignore` (truncates at nearest `.git/`).
    Gitignore = 3,
    /// `<git_root>/.git/info/exclude` (bound to a git_root).
    GitInfoExclude = 4,
    /// git `core.excludesFile` — global.
    GitGlobal = 5,
    /// `$XDG_CONFIG_HOME/fd/ignore` — global.
    FdGlobal = 6,
}

impl MatcherKind {
    pub const COUNT: usize = 7;

    /// Per-directory kinds evaluated against the parent chain. Order matters:
    /// outer loop of `matched()` walks this slice.
    pub const PER_DIR_KINDS: &'static [MatcherKind] = &[
        MatcherKind::Custom, // reserved for future per-dir custom (no-op today)
        MatcherKind::DotIgnore,
        MatcherKind::Fdignore,
        MatcherKind::Gitignore,
        MatcherKind::GitInfoExclude,
    ];

    /// Global kinds, evaluated after the per-dir loop exhausts. Same kind
    /// priority order (Custom -> Git -> Fd) but a single matcher each.
    pub const GLOBAL_KINDS: &'static [MatcherKind] = &[
        MatcherKind::Custom,
        MatcherKind::GitGlobal,
        MatcherKind::FdGlobal,
    ];

    /// Whether matchers of this kind stop walking up the parent chain when
    /// a `.git/` boundary is encountered. Per PLAN.md §4.2 inheritance table:
    /// only `.gitignore` and `.git/info/exclude` truncate.
    pub fn truncates_at_git_root(self) -> bool {
        matches!(self, MatcherKind::Gitignore | MatcherKind::GitInfoExclude)
    }
}

/// Per-cache flags lifted from [`Config`]. Bundled so callers can construct
/// the cache without depending on the whole `Config` type (eases unit tests).
#[derive(Debug, Clone, Copy)]
pub struct IgnoreFlags {
    /// `--no-ignore` switches both `read_fdignore` and `read_vcsignore` off.
    pub read_fdignore: bool,
    pub read_vcsignore: bool,
    /// `--no-require-git`. When `false` (fd default), gitignore rules only
    /// apply inside a tree that contains a `.git`.
    pub require_git: bool,
    /// `--no-ignore-parent`: when `false`, do not climb past each search
    /// root when collecting `.fdignore`/`.ignore`/`.gitignore` chains.
    pub read_parent_ignore: bool,
    /// `--no-global-ignore-file` switches `git_global` and `fd_global` off.
    pub read_global_ignore: bool,
    /// Windows default `true`; passes through to `GitignoreBuilder`.
    pub case_insensitive: bool,
}

impl Default for IgnoreFlags {
    fn default() -> Self {
        Self {
            read_fdignore: true,
            read_vcsignore: true,
            require_git: true,
            read_parent_ignore: true,
            read_global_ignore: true,
            case_insensitive: cfg!(windows),
        }
    }
}

impl IgnoreFlags {
    /// Whether the cache has any work to do. When everything is off, callers
    /// can skip constructing the filter altogether.
    pub fn any_enabled(self) -> bool {
        self.read_fdignore || self.read_vcsignore
    }
}

/// Snapshot of which search root produced a hit, plus the path-prefix boundary
/// for `--no-ignore-parent` evaluation. `Arc<PathBuf>` mirrors
/// `RawHit::search_root` — cheap to clone, shared across all hits from the
/// same root.
#[derive(Debug, Clone)]
pub struct SearchRootContext {
    pub root: Arc<PathBuf>,
}

/// Main cache. Cheap to clone (all state is behind `Arc`/`DashMap`).
#[derive(Clone)]
pub struct IgnoreCache {
    by_dir: Arc<DashMap<PathBuf, Arc<DirIgnoreState>>>,
    globals: Arc<GlobalLayers>,
    flags: IgnoreFlags,
    /// Optional `--no-ignore-parent` ceiling: when set, the parent chain
    /// stops climbing past the matching search root.
    parent_ceilings: Arc<Vec<PathBuf>>,
    case_modifier: CaseModifier,
}

impl IgnoreCache {
    /// Construct a new cache. `parent_ceilings` should contain the canonical
    /// absolute search roots when `read_parent_ignore` is `false` so the
    /// chain stops climbing past them; pass an empty `Vec` to allow free
    /// climb up to volume root.
    pub fn new(
        flags: IgnoreFlags,
        globals: GlobalLayers,
        parent_ceilings: Vec<PathBuf>,
        case_modifier: CaseModifier,
    ) -> Self {
        Self {
            by_dir: Arc::new(DashMap::new()),
            globals: Arc::new(globals),
            flags,
            parent_ceilings: Arc::new(parent_ceilings),
            case_modifier,
        }
    }

    pub fn flags(&self) -> IgnoreFlags {
        self.flags
    }

    pub fn case_modifier(&self) -> CaseModifier {
        self.case_modifier
    }

    /// Fetch or build the [`DirIgnoreState`] for `dir`. Cached via DashMap so
    /// the second hit under the same directory pays only the lookup.
    pub fn dir_state(&self, dir: &Path) -> Arc<DirIgnoreState> {
        if let Some(entry) = self.by_dir.get(dir) {
            return Arc::clone(entry.value());
        }
        let state = DirIgnoreState::build(dir, self);
        let state = Arc::new(state);
        // Use `entry` to avoid building twice if two threads race; whoever
        // arrives second discards their build and reuses the winner.
        self.by_dir
            .entry(dir.to_path_buf())
            .or_insert_with(|| Arc::clone(&state))
            .clone()
    }

    pub(crate) fn globals(&self) -> &GlobalLayers {
        &self.globals
    }

    pub(crate) fn parent_ceiling_for(&self, root: &Path) -> Option<PathBuf> {
        if self.flags.read_parent_ignore {
            return None;
        }
        // `--no-ignore-parent`: stop at the search root that this hit belongs
        // to. `parent_ceilings` is typically a 1-element Vec; linear search is
        // fine.
        self.parent_ceilings
            .iter()
            .find(|p| root.starts_with(p) || p.as_path() == root)
            .cloned()
    }

    /// Top-level driver: PLAN.md §4.2 #7 algorithm. `path` must be absolute
    /// and canonicalized; `search_root` is the root that produced the hit
    /// (used as `--no-ignore-parent` ceiling).
    pub fn matched(&self, path: &Path, is_dir: bool, search_root: &Path) -> Decision {
        if !self.flags.any_enabled() {
            return Decision::Show;
        }
        let leaf_dir = match path.parent() {
            Some(p) => p,
            // Volume root or relative path with no parent — nothing to match.
            None => return Decision::Show,
        };
        let ceiling = self.parent_ceiling_for(search_root);
        // Pre-scan for a `.git/` somewhere in the chain so we can gate
        // gitignore-kind kinds on require_git semantics. Cheap: every dir
        // is already cached by the time we evaluate; `has_local_git_root`
        // is O(1).
        let chain_has_git = self.chain_has_git_root(leaf_dir, ceiling.as_deref());
        // Outer loop: kind priority (PER_DIR_KINDS in priority order).
        for &kind in MatcherKind::PER_DIR_KINDS {
            if !self.kind_enabled(kind) {
                continue;
            }
            if kind.truncates_at_git_root() && self.flags.require_git && !chain_has_git {
                // PLAN §4.2 row 1 "默认须 git": gitignore-kind kinds need a
                // .git/ in scope before any of their rules can fire.
                continue;
            }
            if let Some(verdict) =
                self.eval_kind_along_chain(kind, leaf_dir, path, is_dir, ceiling.as_deref())
            {
                return verdict;
            }
        }
        // Per-dir kinds exhausted → globals.
        for &kind in MatcherKind::GLOBAL_KINDS {
            if !self.kind_enabled(kind) {
                continue;
            }
            if let Some(verdict) = self.globals.eval(kind, path, is_dir) {
                return verdict;
            }
        }
        Decision::Show
    }

    fn kind_enabled(&self, kind: MatcherKind) -> bool {
        match kind {
            MatcherKind::Custom => true, // custom is always on if loaded
            MatcherKind::DotIgnore => self.flags.read_fdignore,
            MatcherKind::Fdignore => self.flags.read_fdignore,
            MatcherKind::Gitignore => self.flags.read_vcsignore,
            MatcherKind::GitInfoExclude => self.flags.read_vcsignore,
            MatcherKind::GitGlobal => self.flags.read_vcsignore && self.flags.read_global_ignore,
            MatcherKind::FdGlobal => self.flags.read_fdignore && self.flags.read_global_ignore,
        }
    }

    /// Inner loop: walk the parent chain leaf-first asking each
    /// [`DirIgnoreState`]'s matcher for `kind`. Returns the first non-`None`
    /// decision, or `None` if the entire chain reports `None` (caller moves
    /// to the next kind).
    fn eval_kind_along_chain(
        &self,
        kind: MatcherKind,
        leaf_dir: &Path,
        path: &Path,
        is_dir: bool,
        ceiling: Option<&Path>,
    ) -> Option<Decision> {
        let mut current: Option<&Path> = Some(leaf_dir);
        while let Some(dir) = current {
            let state = self.dir_state(dir);
            if kind.truncates_at_git_root()
                && state.has_local_git_root()
                && state.git_root_dir() != Some(dir)
                && self.kind_already_evaluated_above_git_root(kind, dir, &state)
            {
                // gitignore semantics: don't cross a .git boundary upwards.
                break;
            }
            if let Some(layer) = state.matcher_of(kind)
                && let Some(decision) = layer.evaluate(path, is_dir)
            {
                return Some(decision);
            }
            // Truncation check for gitignore kinds: if we *just* evaluated a
            // dir that owns a `.git/`, the next iteration is out of scope.
            if kind.truncates_at_git_root() && state.git_root_dir() == Some(dir) {
                break;
            }
            // Climb. Stop at ceiling (--no-ignore-parent) and at filesystem root.
            if let Some(c) = ceiling
                && paths_equal(dir, c)
            {
                break;
            }
            // `gitignore` kind: also stop if require_git is on and we never
            // found a `.git`.
            if kind == MatcherKind::Gitignore
                && self.flags.require_git
                && !state.has_local_git_root()
                && !self.flags.read_parent_ignore
            {
                // require_git + no-parent: enforce the boundary explicitly.
                break;
            }
            current = dir.parent();
        }
        None
    }

    /// Guard against double-evaluation when the climb crosses a `.git` boundary
    /// in the same call. The truncation rule is "once we leave the repo, no
    /// more git rules" — never "skip this dir's rules". The current loop
    /// short-circuits *before* fetching the next dir's matcher, so the guard
    /// is currently always `true` (no skipping needed). Defined as a hook
    /// for future tuning if PLAN.md §4.2 #5 grows nuances.
    fn kind_already_evaluated_above_git_root(
        &self,
        _kind: MatcherKind,
        _dir: &Path,
        _state: &DirIgnoreState,
    ) -> bool {
        true
    }

    /// Scan the parent chain leaf-first for the nearest `.git/` boundary.
    /// Stops at `ceiling` (search root when `--no-ignore-parent`) and at
    /// the filesystem root. Used to gate gitignore-kind evaluation under
    /// `require_git`.
    fn chain_has_git_root(&self, leaf_dir: &Path, ceiling: Option<&Path>) -> bool {
        let mut current: Option<&Path> = Some(leaf_dir);
        while let Some(dir) = current {
            let state = self.dir_state(dir);
            if state.has_local_git_root() {
                return true;
            }
            if let Some(c) = ceiling
                && paths_equal(dir, c)
            {
                break;
            }
            current = dir.parent();
        }
        false
    }
}

#[cfg(test)]
mod diff_test;
#[cfg(test)]
mod semantics_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_kind_priority_order_matches_plan() {
        // PLAN.md §4.2 #7 ordering — if this regresses,
        // test_custom_ignore_precedence and the inheritance table both fail.
        assert!(MatcherKind::Custom < MatcherKind::DotIgnore);
        assert!(MatcherKind::DotIgnore < MatcherKind::Fdignore);
        assert!(MatcherKind::Fdignore < MatcherKind::Gitignore);
        assert!(MatcherKind::Gitignore < MatcherKind::GitInfoExclude);
        assert!(MatcherKind::GitInfoExclude < MatcherKind::GitGlobal);
        assert!(MatcherKind::GitGlobal < MatcherKind::FdGlobal);
    }

    #[test]
    fn gitignore_kinds_truncate_at_git_root_others_do_not() {
        assert!(MatcherKind::Gitignore.truncates_at_git_root());
        assert!(MatcherKind::GitInfoExclude.truncates_at_git_root());
        assert!(!MatcherKind::Fdignore.truncates_at_git_root());
        assert!(!MatcherKind::DotIgnore.truncates_at_git_root());
        assert!(!MatcherKind::Custom.truncates_at_git_root());
        assert!(!MatcherKind::GitGlobal.truncates_at_git_root());
        assert!(!MatcherKind::FdGlobal.truncates_at_git_root());
    }

    #[test]
    fn empty_flags_short_circuit_show() {
        let flags = IgnoreFlags {
            read_fdignore: false,
            read_vcsignore: false,
            ..IgnoreFlags::default()
        };
        let cache = IgnoreCache::new(
            flags,
            GlobalLayers::empty(),
            Vec::new(),
            CaseModifier::Nocase,
        );
        let path = PathBuf::from(r"C:\repo\foo.txt");
        let root = PathBuf::from(r"C:\repo");
        assert_eq!(cache.matched(&path, false, &root), Decision::Show);
    }
}
