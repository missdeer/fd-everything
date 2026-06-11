//! `DirIgnoreState` — per-directory matcher bundle (PLAN.md §4.1).
//!
//! One state per directory, cached by absolute path. Stores up to five
//! `MatcherLayer`s (one per per-dir `MatcherKind`) plus the `git_root`
//! anchor for gitignore truncation. Parent linkage is *not* stored here —
//! the chain walk in `IgnoreCache::matched` re-derives it by climbing
//! `Path::parent()`; this keeps the cache invariant simple and avoids the
//! "stale parent Arc" problem when prewarm and lazy construction race.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::{Decision, IgnoreCache, MatcherKind};

/// One compiled ignore matcher anchored at `base` (the directory whose ignore
/// file produced it).
#[derive(Debug)]
pub struct MatcherLayer {
    base: PathBuf,
    matcher: Gitignore,
    kind: MatcherKind,
}

impl MatcherLayer {
    pub fn kind(&self) -> MatcherKind {
        self.kind
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Evaluate the layer against an absolute path. Returns:
    /// - `Some(Decision::Show)`  if the rule is a whitelist (`!pattern`),
    /// - `Some(Decision::Hide)`  if the rule is an ignore,
    /// - `None`                  if no rule fires (caller continues the chain).
    ///
    /// `path` MUST start with `self.base`; the relative form is what
    /// `Gitignore::matched_path_or_any_parents` expects.
    pub fn evaluate(&self, path: &Path, is_dir: bool) -> Option<Decision> {
        let rel = path.strip_prefix(&self.base).ok()?;
        if rel.as_os_str().is_empty() {
            // The path *is* the base directory; gitignore semantics never let
            // a dir's own ignore file ignore the dir itself.
            return None;
        }
        match self.matcher.matched_path_or_any_parents(rel, is_dir) {
            ignore::Match::None => None,
            ignore::Match::Ignore(_) => Some(Decision::Hide),
            ignore::Match::Whitelist(_) => Some(Decision::Show),
        }
    }
}

/// One slot per [`MatcherKind`]. Indexed by `kind as usize`. `Option` because
/// most directories have no ignore file of a given kind.
#[derive(Debug, Default)]
pub struct DirIgnoreState {
    matchers: [Option<MatcherLayer>; MatcherKind::COUNT],
    /// The directory that owns the nearest `.git/` (or `.git` file pointing
    /// at a worktree gitdir). `None` if no `.git` is found at or above this
    /// dir. Used to gate gitignore truncation and `--require-git`.
    git_root: Option<PathBuf>,
    /// `gitdir` resolved from `.git` worktree files. Distinguishes
    /// "no info/exclude" from "`.git` is a directory; check `git_root/info/exclude`".
    /// `None` when `.git` is a normal directory or absent.
    worktree_gitdir: Option<PathBuf>,
}

impl DirIgnoreState {
    /// Probe `dir` for ignore files matching the cache's enabled kinds.
    /// Stat budget: at most one read per ignore-file kind + one `.git` probe.
    pub fn build(dir: &Path, cache: &IgnoreCache) -> Self {
        let mut state = DirIgnoreState::default();
        let flags = cache.flags();
        let case_insensitive = flags.case_insensitive;

        if flags.read_fdignore {
            state.try_load(dir, ".fdignore", MatcherKind::Fdignore, case_insensitive);
            state.try_load(dir, ".ignore", MatcherKind::DotIgnore, case_insensitive);
        }
        if flags.read_vcsignore {
            state.try_load(dir, ".gitignore", MatcherKind::Gitignore, case_insensitive);
            // .git probe — directory OR file (worktree).
            state.resolve_git(dir);
            if state.has_local_git_root() {
                state.try_load_git_info_exclude(case_insensitive);
            }
        }
        state
    }

    fn try_load(&mut self, dir: &Path, filename: &str, kind: MatcherKind, case_insensitive: bool) {
        let path = dir.join(filename);
        if !path.is_file() {
            return;
        }
        let mut builder = GitignoreBuilder::new(dir);
        builder.case_insensitive(case_insensitive).ok();
        if let Some(err) = builder.add(&path) {
            // Partial errors are non-fatal; mirror walk.rs:307. Hard errors
            // are surfaced to stderr by Phase-5 wiring; here we just skip.
            let _ = err;
        }
        if let Ok(matcher) = builder.build() {
            self.matchers[kind as usize] = Some(MatcherLayer {
                base: dir.to_path_buf(),
                matcher,
                kind,
            });
        }
    }

    fn resolve_git(&mut self, dir: &Path) {
        let git_path = dir.join(".git");
        match std::fs::metadata(&git_path) {
            Ok(md) if md.is_dir() => {
                self.git_root = Some(dir.to_path_buf());
            }
            Ok(md) if md.is_file() => {
                // Worktree / submodule: `gitdir: <path>`.
                if let Ok(contents) = std::fs::read_to_string(&git_path) {
                    for line in contents.lines() {
                        if let Some(rest) = line.strip_prefix("gitdir:") {
                            let gitdir = PathBuf::from(rest.trim());
                            self.git_root = Some(dir.to_path_buf());
                            self.worktree_gitdir = Some(gitdir);
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn try_load_git_info_exclude(&mut self, case_insensitive: bool) {
        let info_exclude = if let Some(gitdir) = &self.worktree_gitdir {
            gitdir.join("info").join("exclude")
        } else if let Some(root) = &self.git_root {
            root.join(".git").join("info").join("exclude")
        } else {
            return;
        };
        if !info_exclude.is_file() {
            return;
        }
        // The exclude file is anchored at the repo working root.
        let Some(base) = self.git_root.clone() else {
            return;
        };
        let mut builder = GitignoreBuilder::new(&base);
        builder.case_insensitive(case_insensitive).ok();
        if let Some(err) = builder.add(&info_exclude) {
            let _ = err;
        }
        if let Ok(matcher) = builder.build() {
            self.matchers[MatcherKind::GitInfoExclude as usize] = Some(MatcherLayer {
                base,
                matcher,
                kind: MatcherKind::GitInfoExclude,
            });
        }
    }

    pub fn matcher_of(&self, kind: MatcherKind) -> Option<&MatcherLayer> {
        self.matchers[kind as usize].as_ref()
    }

    pub fn has_local_git_root(&self) -> bool {
        self.git_root.is_some()
    }

    pub fn git_root_dir(&self) -> Option<&Path> {
        self.git_root.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{GlobalLayers, IgnoreFlags};
    use super::*;
    use crate::scan::backend::CaseModifier;
    use std::fs;
    use tempfile::TempDir;

    fn cache(flags: IgnoreFlags) -> IgnoreCache {
        IgnoreCache::new(
            flags,
            GlobalLayers::empty(),
            Vec::new(),
            CaseModifier::Nocase,
        )
    }

    /// A `.gitignore` whose rules fire on a matching file proves the basic
    /// per-dir matcher plumbing. If this regresses, every higher-level
    /// algorithm test is meaningless.
    #[test]
    fn local_gitignore_hides_matching_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap(); // satisfy require_git
        let cache = cache(IgnoreFlags::default());
        let state = cache.dir_state(root);
        let layer = state.matcher_of(MatcherKind::Gitignore).expect("layer");
        let p = root.join("app.log");
        assert_eq!(layer.evaluate(&p, false), Some(Decision::Hide));
    }

    /// `.git` as a *file* with `gitdir:` should be recognized as a worktree
    /// boundary so .gitignore still truncates the climb correctly. If this
    /// regresses, gitignore semantics for submodules/worktrees silently
    /// break.
    #[test]
    fn dot_git_as_file_resolves_worktree_gitdir() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("worktree");
        let gitdir = tmp.path().join("real-gitdir");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(gitdir.join("info")).unwrap();
        fs::write(repo.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        fs::write(gitdir.join("info").join("exclude"), "*.tmp\n").unwrap();

        let cache = cache(IgnoreFlags::default());
        let state = cache.dir_state(&repo);
        assert!(state.has_local_git_root());
        let layer = state
            .matcher_of(MatcherKind::GitInfoExclude)
            .expect("worktree info/exclude must be loaded");
        let p = repo.join("scratch.tmp");
        assert_eq!(layer.evaluate(&p, false), Some(Decision::Hide));
    }
}
