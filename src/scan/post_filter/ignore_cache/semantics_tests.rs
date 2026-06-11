//! PLAN.md §4.2 semantics fixtures.
//!
//! Each test encodes one row / one nuance of the inheritance table and the
//! v4 kind-priority model (§4.2 #7). The model is unintuitive: cross-kind
//! priority dominates depth — a *shallower* `.fdignore !foo` beats a
//! *deeper* `.gitignore foo`. The v3 model (per-dir kind ordering) gets the
//! `test_custom_ignore_precedence` regression wrong; we test against that
//! exact fixture here.

use super::*;
use crate::scan::backend::CaseModifier;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn build(flags: IgnoreFlags, ceilings: Vec<PathBuf>) -> IgnoreCache {
    IgnoreCache::new(flags, GlobalLayers::empty(), ceilings, CaseModifier::Nocase)
}

fn write_file(p: &std::path::Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// PLAN.md §4.2 #7 / `test_custom_ignore_precedence`:
/// - `root/.fdignore` → `!foo` (whitelist, Fdignore kind = higher priority)
/// - `inner/.gitignore` → `foo` (ignore, Gitignore kind = lower priority)
/// - file `inner/foo` → MUST be shown.
///
/// Why this matters: v3's "per-dir kind ordering" would walk
/// leaf-first and see `inner/.gitignore foo` first → Hide. v4 walks
/// kind-first then dir, so Fdignore evaluates the entire chain first,
/// finds `!foo` at the root, and returns Show before Gitignore gets a turn.
/// If this regresses, fd's own `tests.rs:837` integration test fails.
#[test]
fn fdignore_whitelist_beats_deeper_gitignore_ignore() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let inner = root.join("inner");
    write_file(&root.join(".fdignore"), "!foo\n");
    write_file(&inner.join(".gitignore"), "foo\n");
    fs::create_dir_all(root.join(".git")).unwrap();

    let cache = build(IgnoreFlags::default(), Vec::new());
    let target = inner.join("foo");
    write_file(&target, "");
    assert_eq!(cache.matched(&target, false, root), Decision::Show);
}

/// Inheritance table row 1: `.gitignore` is inherited across directories.
/// `root/.gitignore` saying `*.log` MUST hide `root/sub/app.log`.
#[test]
fn gitignore_inherits_across_directories() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_file(&root.join(".gitignore"), "*.log\n");
    fs::create_dir_all(root.join(".git")).unwrap();
    let target = root.join("sub").join("app.log");
    write_file(&target, "");

    let cache = build(IgnoreFlags::default(), Vec::new());
    assert_eq!(cache.matched(&target, false, root), Decision::Hide);
}

/// Inheritance table row 1, column 2: `.gitignore` truncates at nested
/// `.git/`. An `outer/.gitignore *.log` MUST NOT apply to
/// `outer/inner_repo/app.log` if `outer/inner_repo/.git/` exists.
#[test]
fn gitignore_truncates_at_nested_git_root() {
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path();
    let inner = outer.join("inner_repo");
    write_file(&outer.join(".gitignore"), "*.log\n");
    fs::create_dir_all(outer.join(".git")).unwrap();
    fs::create_dir_all(inner.join(".git")).unwrap();
    let target = inner.join("app.log");
    write_file(&target, "");

    let cache = build(IgnoreFlags::default(), Vec::new());
    // inner has its own .git → outer's .gitignore truncates → Show.
    assert_eq!(cache.matched(&target, false, outer), Decision::Show);
}

/// Inheritance table row 4 / row 5: `.fdignore` and `.ignore` do NOT
/// truncate at a `.git` boundary — they keep climbing through nested
/// repos.
#[test]
fn fdignore_does_not_truncate_at_git_boundary() {
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path();
    let inner = outer.join("inner_repo");
    write_file(&outer.join(".fdignore"), "*.log\n");
    fs::create_dir_all(outer.join(".git")).unwrap();
    fs::create_dir_all(inner.join(".git")).unwrap();
    let target = inner.join("app.log");
    write_file(&target, "");

    let cache = build(IgnoreFlags::default(), Vec::new());
    // Inner has its own .git, but .fdignore is not gitignore-kind → does
    // not truncate; outer's `*.log` still applies → Hide.
    assert_eq!(cache.matched(&target, false, outer), Decision::Hide);
}

/// Inheritance table: `--no-ignore-parent` (read_parent_ignore = false)
/// MUST stop the climb at the search root for ALL kinds that respect the
/// flag (.gitignore, .fdignore, .ignore). `.git/info/exclude` and globals
/// are unaffected.
#[test]
fn no_ignore_parent_stops_climb_at_search_root() {
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path();
    let search_root = outer.join("project");
    write_file(&outer.join(".fdignore"), "*.log\n");
    fs::create_dir_all(&search_root).unwrap();
    let target = search_root.join("app.log");
    write_file(&target, "");

    let flags = IgnoreFlags {
        read_parent_ignore: false,
        ..IgnoreFlags::default()
    };
    let cache = build(flags, vec![search_root.clone()]);
    // outer's `.fdignore` is above the ceiling → does not apply → Show.
    assert_eq!(cache.matched(&target, false, &search_root), Decision::Show);
}

/// `--no-ignore-vcs` (read_vcsignore = false) MUST disable both
/// `.gitignore` and `.git/info/exclude` evaluation while leaving
/// `.fdignore` / `.ignore` intact.
#[test]
fn no_ignore_vcs_disables_gitignore_kinds() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_file(&root.join(".gitignore"), "*.log\n");
    write_file(&root.join(".fdignore"), "*.tmp\n");
    fs::create_dir_all(root.join(".git")).unwrap();
    let log = root.join("app.log");
    let tmp_file = root.join("app.tmp");
    write_file(&log, "");
    write_file(&tmp_file, "");

    let flags = IgnoreFlags {
        read_vcsignore: false,
        ..IgnoreFlags::default()
    };
    let cache = build(flags, Vec::new());
    assert_eq!(
        cache.matched(&log, false, root),
        Decision::Show,
        ".gitignore must NOT apply with read_vcsignore=false"
    );
    assert_eq!(
        cache.matched(&tmp_file, false, root),
        Decision::Hide,
        ".fdignore must still apply"
    );
}

/// `.git/info/exclude` is bound to its repo: it MUST hide paths in the
/// repo but MUST NOT apply outside.
#[test]
fn git_info_exclude_applies_inside_repo_only() {
    let tmp = TempDir::new().unwrap();
    let outer = tmp.path();
    let repo = outer.join("repo");
    fs::create_dir_all(repo.join(".git").join("info")).unwrap();
    write_file(&repo.join(".git").join("info").join("exclude"), "secret\n");
    let inside = repo.join("secret");
    let outside = outer.join("secret");
    write_file(&inside, "");
    write_file(&outside, "");

    let cache = build(IgnoreFlags::default(), Vec::new());
    assert_eq!(
        cache.matched(&inside, false, &repo),
        Decision::Hide,
        "info/exclude must fire inside its repo"
    );
    // outside the repo, the repo's info/exclude is irrelevant. There's no
    // .git anywhere above `outside`, so with default require_git=true,
    // .gitignore would not even be active here.
    assert_eq!(
        cache.matched(&outside, false, outer),
        Decision::Show,
        "info/exclude must NOT bleed outside its repo"
    );
}

/// Leaf-first within a kind: an *inner* `.gitignore !foo` MUST override
/// the *outer* `.gitignore foo` even though they're the same kind. This
/// is the "same kind, leaf wins" leg of §4.2 #7.
#[test]
fn same_kind_inner_whitelist_overrides_outer_ignore() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let inner = root.join("sub");
    write_file(&root.join(".gitignore"), "foo\n");
    write_file(&inner.join(".gitignore"), "!foo\n");
    fs::create_dir_all(root.join(".git")).unwrap();
    let target = inner.join("foo");
    write_file(&target, "");

    let cache = build(IgnoreFlags::default(), Vec::new());
    assert_eq!(cache.matched(&target, false, root), Decision::Show);
}

/// require_git defaults to true: a `.gitignore` *without* an accompanying
/// `.git/` MUST be a no-op. Phase 4 must respect the flag — if this
/// regresses, fd's default behavior for non-repo trees diverges from
/// upstream.
#[test]
fn gitignore_without_git_is_noop_when_require_git() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_file(&root.join(".gitignore"), "*.log\n");
    // No .git directory.
    let target = root.join("app.log");
    write_file(&target, "");

    let flags = IgnoreFlags {
        require_git: true,
        ..IgnoreFlags::default()
    };
    let cache = build(flags, Vec::new());
    // PLAN §4.2 row 1: require_git → .gitignore needs a .git/ in scope.
    // Our implementation currently *does* load `.gitignore` even without
    // .git (the gitignore crate's matched_path_or_any_parents doesn't
    // gate). For row 1's "默认须 git" column we explicitly gate on
    // `state.has_local_git_root()` in IgnoreCache's chain walk — see
    // `eval_kind_along_chain`'s require_git short-circuit. If this test
    // fails, that gate is missing or wrong.
    assert_eq!(cache.matched(&target, false, root), Decision::Show);
}

/// `--no-require-git` (`require_git = false`): same setup as above, but
/// the `.gitignore` rule now applies even without a `.git/`.
#[test]
fn gitignore_applies_without_git_when_no_require_git() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_file(&root.join(".gitignore"), "*.log\n");
    let target = root.join("app.log");
    write_file(&target, "");

    let flags = IgnoreFlags {
        require_git: false,
        ..IgnoreFlags::default()
    };
    let cache = build(flags, Vec::new());
    assert_eq!(cache.matched(&target, false, root), Decision::Hide);
}
