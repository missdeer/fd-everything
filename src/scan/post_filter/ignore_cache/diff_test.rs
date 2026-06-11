//! Differential regression: `IgnoreCache::matched` MUST agree with
//! `ignore::WalkBuilder` on every path in a fixture tree.
//!
//! This is Phase 4's main correctness contract (PLAN.md §Phase 4 intro:
//! "用 MockBackend 喂任意路径流，对照原 walker 做差分回归"). Without it,
//! semantic drift between IgnoreCache and the legacy walker would surface
//! only as user-visible bugs once Phase 5 flips the switch.
//!
//! The fixture below stresses the kinds we care about:
//! - root `.gitignore` (any-depth + anchored rules),
//! - root `.fdignore` (whitelist counter to a deeper `.gitignore` rule),
//! - nested `.gitignore` (leaf-overrides-outer same-kind),
//! - nested `.git/` boundary (truncation).

use super::*;
use crate::scan::backend::CaseModifier;
use ignore::WalkBuilder;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_file(p: &Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn touch(p: &Path) {
    write_file(p, "");
}

fn build_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Outer repo
    fs::create_dir_all(root.join(".git")).unwrap();
    write_file(&root.join(".gitignore"), "*.log\n/anchored_dir/\n");
    write_file(&root.join(".fdignore"), "!keepme.log\n");

    // Plain files at root
    touch(&root.join("app.log")); // matched by *.log → Hide
    touch(&root.join("keepme.log")); // .fdignore !keepme.log → Show
    touch(&root.join("readme.md")); // no rules → Show

    // Anchored dir at root
    touch(&root.join("anchored_dir").join("file")); // /anchored_dir/ → Hide

    // Sub dir, no nested .git → outer rules still apply
    touch(&root.join("sub").join("nested.log")); // *.log → Hide
    touch(&root.join("sub").join("nested.md")); // → Show

    // Sub dir with nested .gitignore that whitelists *.log
    write_file(&root.join("sub2").join(".gitignore"), "!*.log\n");
    touch(&root.join("sub2").join("ok.log")); // !*.log (same-kind, leaf wins) → Show
    touch(&root.join("sub2").join("ok.md"));

    // Inner repo: outer rules MUST NOT cross its .git
    fs::create_dir_all(root.join("inner_repo").join(".git")).unwrap();
    touch(&root.join("inner_repo").join("untouched.log")); // outer's *.log truncated → Show

    // Sub with anchored_dir name: NOT at root → /anchored_dir/ does NOT match
    touch(&root.join("sub3").join("anchored_dir").join("file")); // → Show

    tmp
}

/// Collect every file under `root` (recursively), record absolute path,
/// is_dir flag, and walker's Verdict (Show if walker yielded it, Hide if not).
fn walker_decisions(root: &Path) -> HashSet<PathBuf> {
    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(true)
        .parents(true);
    walker.add_custom_ignore_filename(".fdignore");
    let mut shown = HashSet::new();
    for entry in walker.build().flatten() {
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            shown.insert(entry.path().to_path_buf());
        }
    }
    shown
}

/// All files under `root` (raw — no ignore evaluation).
fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn rec(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                rec(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    rec(root, &mut out);
    out
}

/// IgnoreCache MUST classify every file the same way `ignore::WalkBuilder`
/// does, modulo a known set of inherent differences we document at the
/// bottom of this file. Any drift here predicts a Phase-5 user-visible
/// regression vs upstream fd.
#[test]
fn ignore_cache_agrees_with_walker_on_fixture_tree() {
    let tmp = build_fixture();
    let root = tmp.path();

    let walker_shown = walker_decisions(root);
    let files = all_files(root);

    let cache = IgnoreCache::new(
        IgnoreFlags::default(),
        GlobalLayers::empty(),
        Vec::new(),
        CaseModifier::Nocase,
    );

    let mut mismatches: Vec<String> = Vec::new();
    for f in &files {
        let cache_decision = cache.matched(f, false, root);
        let walker_shows = walker_shown.contains(f);
        let cache_shows = matches!(cache_decision, Decision::Show);
        if walker_shows != cache_shows {
            mismatches.push(format!(
                "{}: walker={} cache={:?}",
                f.display(),
                if walker_shows { "Show" } else { "Hide" },
                cache_decision
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "IgnoreCache disagrees with walker on {} path(s):\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}
