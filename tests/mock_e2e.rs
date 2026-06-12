//! PLAN.md §Phase 8.6 / Phase 8 §8g: process-level mock e2e suite.
//!
//! These tests spawn the real `fde` binary with `FDE_TEST_MOCK_HITS=<path>`
//! to swap `EverythingBackend` for the deterministic
//! [`MockHitFileBackend`](../../src/scan/mock_injection.rs) replay. They
//! cover the routing dispatcher's edges that the in-process unit tests
//! can't reach: argv → Config → walk::scan → backend choice → pipeline
//! → BatchSender → stdout.
//!
//! PLAN §Phase 8.7.1 MUST 3 retired the `FDE_BACKEND=everything` opt-in
//! gate, so `FDE_TEST_MOCK_HITS` alone is the trigger. When mock injection
//! is active the dispatcher forces `AssumeIndexedProbe` so harness
//! tempdirs (which the real probe would correctly route to Legacy) still
//! flow through `MockHitFileBackend`.
//!
//! Why a separate `tests/mock_e2e.rs` file instead of folding into
//! `tests/tests.rs`? `tests/tests.rs` uses the shared `testenv` harness
//! whose `run_command` doesn't take per-test env vars (by design — the
//! existing fd integration tests must run in a clean env). The mock e2e
//! cases need three custom env vars set per invocation, so they own
//! their own process::Command setup.
//!
//! These tests do NOT require Everything to be installed or running.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Path of the `fde` binary we test against. cargo sets the env var
/// when `tests/` is built; the literal fallback covers `cargo test
/// --no-run`-style invocations.
fn fde_exe() -> PathBuf {
    PathBuf::from(option_env!("CARGO_BIN_EXE_fde").unwrap_or("target/debug/fde.exe"))
}

/// Build a hit-file in `dir` with the given lines and return its path.
/// Each `&str` becomes one line in the file.
fn write_hit_file(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
    let path = dir.join(name);
    let body = lines.join("\n");
    fs::write(&path, body).expect("write hit file");
    path
}

/// Run `fde` with `FDE_TEST_MOCK_HITS=<hits>` and the given argv (search
/// root + pattern). Returns (stdout, stderr, exit_code) so each test can
/// assert what it cares about. `FDE_BACKEND` is explicitly removed: PLAN
/// §Phase 8.7.1 MUST 3 retired the gate, so leaving an inherited
/// `FDE_BACKEND=everything` from a parent shell would now be a no-op,
/// but env_remove keeps the test invariant "mock injection is driven by
/// FDE_TEST_MOCK_HITS alone" explicit and stale-shell-proof.
fn run_fde_with_mock(cwd: &Path, hits_file: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    let mut cmd = Command::new(fde_exe());
    cmd.current_dir(cwd)
        .env_remove("FDE_BACKEND")
        .env("FDE_TEST_MOCK_HITS", hits_file)
        // Mirror tests/testenv: disable global ignore so test ergonomics
        // don't depend on $XDG_CONFIG_HOME.
        .arg("--no-global-ignore-file")
        .args(args);
    let output = cmd.output().expect("spawn fde");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

/// Run `fde` WITHOUT mock injection, for parity comparisons. Mirrors the
/// MUST 3 invariants: no `FDE_BACKEND`, no `FDE_TEST_MOCK_HITS`. Routing
/// defers entirely to the probe (which returns false for fresh tempdirs).
fn run_fde_without_mock(cwd: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    let mut cmd = Command::new(fde_exe());
    cmd.current_dir(cwd)
        .env_remove("FDE_BACKEND")
        .env_remove("FDE_TEST_MOCK_HITS")
        .arg("--no-global-ignore-file")
        .args(args);
    let output = cmd.output().expect("spawn fde");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

fn sorted_lines(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s.lines().map(|l| l.to_string()).collect();
    v.sort();
    v
}

/// Encodes the §8g routing contract: with `FDE_TEST_MOCK_HITS=<file>`,
/// `walk::scan` MUST replace `EverythingBackend` with the parsed mock
/// stream and stream every staged hit through the post-filter pipeline
/// then the path projector. The post-filter chain has no `regex_filter`
/// (PLAN §6 removed it — the real Everything backend pushes the pattern
/// down via `regex:` query syntax, and MockBackend ignores
/// BackendQuery), so this test uses a CLI dimension the pipeline DOES
/// enforce — `--extension foo` — to prove the staged hits flow through
/// filtering. If the dispatcher silently falls back to LegacyWalker,
/// the tempdir is empty and the test fails on missing output.
#[test]
fn mock_injection_streams_hits_through_pipeline() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let hits_dir = TempDir::new().unwrap();
    let hits_file = write_hit_file(
        hits_dir.path(),
        "hits.txt",
        &[
            &format!("{}\\a.foo", root.display()),
            &format!("{}\\bar.txt", root.display()),
            &format!("{}\\sub\tdir", root.display()),
            &format!("{}\\sub\\c.foo", root.display()),
        ],
    );
    // `--extension foo` filters via the post-filter `ExtensionFilter`,
    // which inspects RawHit.path's file_name. Pattern `.` matches
    // anything so the (unused) regex doesn't drop anything.
    let (stdout, stderr, code) =
        run_fde_with_mock(root, &hits_file, &["--extension", "foo", ".", "."]);
    assert_eq!(code, Some(0), "fde must exit 0; stderr={stderr}");
    let got = sorted_lines(&stdout);
    // PathProjector turns absolute hits back into "./<rel>" relative to
    // the user-given root (".").
    assert_eq!(
        got,
        vec!["./a.foo".to_string(), "./sub/c.foo".to_string()],
        "only .foo files should survive the extension filter; got {got:?}"
    );
}

/// Encodes the §Phase 8.5-D + §8g flag-precedence contract:
/// `--filesystem-walker` MUST win over `FDE_TEST_MOCK_HITS`. The user's
/// CLI escape hatch overrides any test harness env var. If this
/// regresses, a developer using `--filesystem-walker` to diagnose
/// Everything-vs-fd differences would silently see the mock fixture
/// instead of the real walker.
#[test]
fn filesystem_walker_flag_overrides_mock_injection() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // Actual on-disk file the legacy walker will discover.
    fs::write(root.join("real.txt"), b"x").unwrap();

    let hits_dir = TempDir::new().unwrap();
    let hits_file = write_hit_file(
        hits_dir.path(),
        "hits.txt",
        // A mock hit that does NOT exist on disk; if the dispatcher
        // honours the env var despite --filesystem-walker, the test
        // sees this path (and real.txt is missing).
        &[&format!("{}\\ghost.txt", root.display())],
    );

    let (stdout, _stderr, code) =
        run_fde_with_mock(root, &hits_file, &["--filesystem-walker", "txt", "."]);
    assert_eq!(code, Some(0));
    let got = sorted_lines(&stdout);
    // Legacy walker emits relative paths (no leading "./" because the
    // default strip_cwd_prefix is on for non-null, non-exec mode).
    assert!(
        got.iter().any(|l| l.ends_with("real.txt")),
        "legacy walker must surface real.txt; got {got:?}"
    );
    assert!(
        !got.iter().any(|l| l.contains("ghost.txt")),
        "mock fixture must NOT leak when --filesystem-walker is set; got {got:?}"
    );
}

/// Encodes the §8g `--max-results` cancellation contract: when the user
/// caps results, the dispatcher MUST stop feeding mock hits past the
/// limit. The MockHitFileBackend honours `CancellationToken`, the
/// PostFilterSink fires the token from `MaxResultsFilter::Cancel`, and
/// the binary's exit code stays 0. If this regresses, large mock
/// fixtures would still process every line — a performance regression
/// that production EverythingBackend tests wouldn't catch because real
/// `Everything_QueryW` halts on the server side.
#[test]
fn mock_injection_respects_max_results() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let hits_dir = TempDir::new().unwrap();
    let hits_file = write_hit_file(
        hits_dir.path(),
        "hits.txt",
        &[
            &format!("{}\\a.foo", root.display()),
            &format!("{}\\b.foo", root.display()),
            &format!("{}\\c.foo", root.display()),
            &format!("{}\\d.foo", root.display()),
            &format!("{}\\e.foo", root.display()),
        ],
    );

    let (stdout, stderr, code) =
        run_fde_with_mock(root, &hits_file, &["--max-results", "2", "foo", "."]);
    assert_eq!(code, Some(0), "stderr={stderr}");
    let got = sorted_lines(&stdout);
    assert_eq!(
        got.len(),
        2,
        "max-results must cap output at 2; got {got:?}"
    );
}

/// Encodes the §8g parser-error contract: a malformed mock fixture
/// MUST surface as a worker error (visible under `--show-errors`) AND
/// the dispatcher MUST fall every root back to LegacyWalker so the
/// user doesn't get a silent empty result set. If this regresses, a
/// typo'd test fixture would look like "no matches" and pass for the
/// wrong reason.
#[test]
fn malformed_mock_fixture_surfaces_error_and_falls_back() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(root.join("real.txt"), b"x").unwrap();

    let hits_dir = TempDir::new().unwrap();
    // `dr` is not a valid kind token (`dir` is). The parser rejects it.
    let hits_file = write_hit_file(hits_dir.path(), "bad.txt", &["C:\\x\tdr"]);

    let (stdout, stderr, code) =
        run_fde_with_mock(root, &hits_file, &["--show-errors", "txt", "."]);
    // Exit 0 even with the worker-error: matches fde's existing
    // semantics around filesystem errors (they print to stderr, but the
    // search still completes from whatever roots it could process).
    assert_eq!(code, Some(0), "stderr={stderr}");
    assert!(
        stderr.contains("dr"),
        "parse error must echo the bad token to stderr; got: {stderr}"
    );
    // Fallback path: the legacy walker found `real.txt`.
    let got = sorted_lines(&stdout);
    assert!(
        got.iter().any(|l| l.ends_with("real.txt")),
        "legacy fallback must surface real.txt; got {got:?}"
    );
}

/// PLAN §Phase 8.7.1 MUST 3 sanity pin: the mock-injection trigger is
/// `FDE_TEST_MOCK_HITS` alone — `FDE_BACKEND` is no longer consulted
/// (the opt-in gate was retired). This test pins the negative half of
/// that contract: with neither env var set, the dispatcher must NOT
/// load any mock fixture and the legacy walker has to surface the
/// real on-disk file. Without this pin, a regression that re-introduces
/// a stale `FDE_TEST_MOCK_HITS` lookup from a previous shell session
/// could let mock data silently leak into production `fde` invocations.
///
/// The verification mechanism: a fresh tempdir is not yet in
/// Everything's index, so `--probe` routes it to the legacy walker,
/// which reads `real.txt` straight off disk. Without `--probe` the new
/// default skips the probe and queries Everything directly; Everything
/// returns 0 for the un-indexed tempdir and the test would see an empty
/// result that looks identical to "mock loaded but matched nothing" —
/// destroying the test's ability to tell the two apart. `--probe` keeps
/// this specific test's signal alive; it does not affect the
/// `FDE_TEST_MOCK_HITS` gate itself.
///
/// The positive half ("FDE_TEST_MOCK_HITS alone DOES trigger the mock")
/// is covered by `mock_injection_streams_hits_through_pipeline` —
/// `run_fde_with_mock` now removes `FDE_BACKEND` before launching, so
/// each successful run there is a positive proof that the gate is gone.
#[test]
fn mock_env_unset_means_no_mock_injection() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(root.join("real.txt"), b"x").unwrap();

    // No mock hit on disk; if the dispatcher still reaches for a stale
    // FDE_TEST_MOCK_HITS, this won't match any real file but the test
    // harness would also report something other than real.txt.
    let (stdout, _stderr, code) = run_fde_without_mock(root, &["--probe", "txt", "."]);
    assert_eq!(code, Some(0));
    let got = sorted_lines(&stdout);
    assert!(
        got.iter().any(|l| l.ends_with("real.txt")),
        "legacy walker (probe→Legacy on fresh tempdir) must surface real.txt; got {got:?}"
    );
    assert_eq!(
        got.len(),
        1,
        "with no mock fixture set, exactly one hit (real.txt) is expected; got {got:?}"
    );
}
