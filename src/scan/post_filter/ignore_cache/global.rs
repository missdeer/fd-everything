//! Global ignore layers — git `core.excludesFile`, fd global ignore, and
//! `--ignore-file` (custom). Single matcher each; not parent-chained.
//!
//! These are evaluated AFTER the per-directory chain exhausts, but under the
//! same kind-priority order (Custom > Git > Fd). See PLAN.md §4.2 #7.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::{Decision, MatcherKind};

/// One global matcher. `base` is "/" / "C:\\" on Windows (i.e. effectively
/// the filesystem root) so any absolute path is interpretable as relative.
#[derive(Debug)]
pub struct GlobalMatcher {
    base: PathBuf,
    matcher: Gitignore,
    kind: MatcherKind,
}

impl GlobalMatcher {
    pub fn kind(&self) -> MatcherKind {
        self.kind
    }

    pub fn evaluate(&self, path: &Path, is_dir: bool) -> Option<Decision> {
        // Global matchers have no shared anchor (Windows has multiple drives;
        // Unix anchor is "/"). `Gitignore::matched_path_or_any_parents` asserts
        // the path starts with the matcher's root and panics otherwise. We
        // walk ancestors ourselves and call the panic-free `matched()` on
        // each — exactly mirroring what `_or_any_parents` does internally,
        // minus the anchor assertion.
        if let Some(decision) = decide(&self.matcher, path, is_dir) {
            return Some(decision);
        }
        let mut cursor = path.parent();
        while let Some(p) = cursor {
            if let Some(decision) = decide(&self.matcher, p, true) {
                return Some(decision);
            }
            cursor = p.parent();
        }
        None
    }
}

fn decide(matcher: &Gitignore, path: &Path, is_dir: bool) -> Option<Decision> {
    match matcher.matched(path, is_dir) {
        ignore::Match::None => None,
        ignore::Match::Ignore(_) => Some(Decision::Hide),
        ignore::Match::Whitelist(_) => Some(Decision::Show),
    }
}

/// Bundle of the global matchers. `IgnoreCache::globals` holds an `Arc` of
/// this; construction happens once at scan start.
#[derive(Debug, Default)]
pub struct GlobalLayers {
    git_global: Option<GlobalMatcher>,
    fd_global: Option<GlobalMatcher>,
    /// `--ignore-file` may be given multiple times. Evaluated in order;
    /// first non-`None` wins (treated as a kind-Custom precedence group).
    custom: Vec<GlobalMatcher>,
}

impl GlobalLayers {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build globals from caller-supplied paths. Each path that is not a file
    /// is silently skipped (mirrors `walk.rs:304` semantics).
    pub fn build(
        git_global_path: Option<&Path>,
        fd_global_path: Option<&Path>,
        custom_paths: &[PathBuf],
        case_insensitive: bool,
    ) -> Self {
        let mut layers = Self::empty();
        let anchor = anchor_root();

        if let Some(p) = git_global_path
            && p.is_file()
        {
            layers.git_global = build_matcher(p, &anchor, MatcherKind::GitGlobal, case_insensitive);
        }
        if let Some(p) = fd_global_path
            && p.is_file()
        {
            layers.fd_global = build_matcher(p, &anchor, MatcherKind::FdGlobal, case_insensitive);
        }
        for p in custom_paths {
            if !p.is_file() {
                continue;
            }
            if let Some(m) = build_matcher(p, &anchor, MatcherKind::Custom, case_insensitive) {
                layers.custom.push(m);
            }
        }
        layers
    }

    pub fn eval(&self, kind: MatcherKind, path: &Path, is_dir: bool) -> Option<Decision> {
        match kind {
            MatcherKind::Custom => {
                for m in &self.custom {
                    if let Some(v) = m.evaluate(path, is_dir) {
                        return Some(v);
                    }
                }
                None
            }
            MatcherKind::GitGlobal => self
                .git_global
                .as_ref()
                .and_then(|m| m.evaluate(path, is_dir)),
            MatcherKind::FdGlobal => self
                .fd_global
                .as_ref()
                .and_then(|m| m.evaluate(path, is_dir)),
            _ => None,
        }
    }
}

fn build_matcher(
    ignore_file: &Path,
    anchor: &Path,
    kind: MatcherKind,
    case_insensitive: bool,
) -> Option<GlobalMatcher> {
    let mut builder = GitignoreBuilder::new(anchor);
    builder.case_insensitive(case_insensitive).ok();
    if let Some(err) = builder.add(ignore_file) {
        let _ = err;
    }
    let matcher = builder.build().ok()?;
    Some(GlobalMatcher {
        base: anchor.to_path_buf(),
        matcher,
        kind,
    })
}

/// Anchor for global matchers. We want a path that prefixes any absolute hit
/// so `strip_prefix` succeeds. On Windows the volume varies — use the
/// `Path::new("")` empty anchor and pass absolute paths verbatim
/// (`GitignoreBuilder` treats this as no-root). On Unix, use "/".
fn anchor_root() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::new()
    }
    #[cfg(unix)]
    {
        PathBuf::from("/")
    }
}

/// Resolve `core.excludesFile` from `~/.gitconfig`. Returns `None` if no
/// gitconfig exists or the field is unset. Minimal parser — we accept the
/// common ` [core] excludesFile = path` form and reject anything we don't
/// recognize rather than guess.
///
/// Mirrors what `ignore` crate's `WalkBuilder` does internally; we
/// re-implement to avoid binding to `WalkBuilder` state.
pub fn detect_git_core_excludes_file() -> Option<PathBuf> {
    let home = etcetera::home_dir().ok()?;
    let gitconfig = home.join(".gitconfig");
    let contents = std::fs::read_to_string(&gitconfig).ok()?;
    let mut in_core = false;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            in_core = line.eq_ignore_ascii_case("[core]");
            continue;
        }
        if !in_core {
            continue;
        }
        if let Some((k, v)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case("excludesfile")
        {
            let v = v.trim().trim_matches('"');
            let expanded = if let Some(stripped) = v.strip_prefix("~/") {
                home.join(stripped)
            } else {
                PathBuf::from(v)
            };
            return Some(expanded);
        }
    }
    None
}

/// Resolve `$XDG_CONFIG_HOME/fd/ignore` (or platform equivalent). Mirrors
/// `walk.rs:303`.
pub fn detect_fd_global_ignore_file() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    let basedirs = etcetera::choose_base_strategy().ok()?;
    Some(basedirs.config_dir().join("fd").join("ignore"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// A global custom ignore file must hide a matching file even when no
    /// per-dir ignore file exists. If this regresses, `--ignore-file` becomes
    /// silently a no-op for trees that don't already have a .gitignore.
    #[test]
    fn custom_global_ignore_hides_match() {
        let tmp = TempDir::new().unwrap();
        let ignore_file = tmp.path().join("custom.ignore");
        fs::write(&ignore_file, "secret.txt\n").unwrap();

        let g = GlobalLayers::build(None, None, &[ignore_file], false);
        let p = tmp.path().join("secret.txt");
        let decision = g.eval(MatcherKind::Custom, &p, false);
        assert_eq!(decision, Some(Decision::Hide));
    }
}
