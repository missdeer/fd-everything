//! PLAN.md §Phase 8 §3 / §8h: real Everything end-to-end smoke suite.
//!
//! These tests spawn `fde` with no env-var gating (PLAN §Phase 8.7.1
//! MUST 3 retired the `FDE_BACKEND=everything` opt-in) and check that
//! results come back. Routing decisions belong entirely to the
//! `EverythingVolumeIndexProbe`. They require:
//!
//! 1. Windows host with Everything installed (the binary itself only
//!    builds on Windows; that's enforced upstream via
//!    `#[cfg(not(windows))] compile_error!`).
//! 2. The Everything service is running and has finished its initial
//!    index load.
//! 3. The path queried below (`C:\Windows\System32`) is in the index.
//!    This holds for any default Everything install because system
//!    drives are auto-indexed.
//!
//! All tests are `#[ignore]`d so CI never runs them — `cargo test
//! --test real_everything` skips them, while `cargo test --test
//! real_everything -- --ignored` opts in. PLAN explicitly excludes
//! these from CI: "CI 不具备，本机测试需要手工准备 Everything 安装".
//!
//! ## Why no instance isolation?
//!
//! PLAN §Phase 8 §3 originally specified each test launch its own
//! Everything instance via `Everything.exe -instance fde-test -config
//! <tmp>/cfg.ini`, plus an SDK call `Everything_SetInstanceName("fde-
//! test")` to route IPC at the named instance. That isolation is what
//! lets PLAN's e2e plan use tempdirs without polluting the global
//! index.
//!
//! Phase 8.6 calibration of PLAN §Phase 5's "API 名以 SDK 头文件为准"
//! clause: the Everything SDK does NOT expose `SetInstanceName` in any
//! released version. `Everything-SDK/include/Everything.h` (1.4.1, the
//! current upstream-latest per voidtools) contains zero matches for
//! `Instance`. Everything.exe itself supports `-instance <name>`, but
//! the SDK's IPC layer hard-codes the default-instance window class
//! and does not let callers pick a different one. Routing the SDK at a
//! named instance would require bypassing it and writing custom IPC —
//! out of scope for this fork.
//!
//! For now these tests run against the user's default Everything
//! instance and query `C:\Windows\System32`, which is universally
//! indexed. PLAN §Phase 8.6 records this as a permanent limitation,
//! not a deferral.

use std::path::PathBuf;
use std::process::Command;

fn fde_exe() -> PathBuf {
    PathBuf::from(option_env!("CARGO_BIN_EXE_fde").unwrap_or("target/debug/fde.exe"))
}

/// PLAN §8h smoke: when Everything is running, querying a path the
/// user's index covers returns at least one hit. If this regresses to
/// empty, either:
///
/// - The dispatcher in `walk::scan` failed to route to EverythingBackend
///   (regression in Phase 8.5-D / 8.7), or
/// - The translation layer in `query::translate` rejected the query
///   (`--type d` against `System32` should never be rejected — every
///   field is supported), or
/// - The SDK call sequence in `EverythingBackend::run` broke (likely a
///   forgotten `set_request_flags` between calls — the global SDK mutex
///   makes this kind of bug latent), or
/// - Phase 8.7 `EverythingVolumeIndexProbe` regressed to "always false".
///
/// Pattern `.exe` over `C:\Windows\System32` is a standing target on
/// every Windows host. We don't assert a specific count because that
/// would couple the test to OS build numbers; "at least one" is enough
/// to prove the wiring fires.
#[test]
#[ignore = "requires Everything running with C:\\Windows indexed; opt in with `--ignored`"]
fn real_everything_smoke_finds_system32_executables() {
    let output = Command::new(fde_exe())
        .env_remove("FDE_BACKEND")
        .env_remove("FDE_TEST_MOCK_HITS")
        .args([
            "--no-global-ignore-file",
            "--type",
            "f",
            "--extension",
            "exe",
            ".",
            r"C:\Windows\System32",
        ])
        .output()
        .expect("spawn fde");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fde must exit 0 against a known-indexed path. stderr={stderr}"
    );
    let n_hits = stdout.lines().count();
    assert!(
        n_hits > 0,
        "Everything should return at least one .exe under System32; \
         stderr={stderr}, stdout_len={}",
        stdout.len()
    );
}

/// PLAN §Phase 8.7 probe-fallback smoke: when Everything is running
/// but the search root is freshly created and therefore NOT in the
/// index, `EverythingVolumeIndexProbe::is_indexed` returns false
/// (count-only `path:"<root>"` query yields zero hits), so the
/// dispatcher silently routes the root to LegacyWalker, which DOES
/// find the on-disk file. Before Phase 8.7 this returned empty —
/// that was the whole reason the `FDE_BACKEND` opt-in gate existed,
/// which §Phase 8.7.1 MUST 3 then retired now that the probe alone
/// handles the unindexed-tempdir case correctly. If this regresses
/// to empty, the probe has likely gone back to
/// always-true (or AssumeIndexedProbe wired into production), and
/// every tempdir test in `tests/tests.rs` will silently break.
#[test]
#[ignore = "requires Everything running; opt in with `--ignored`"]
fn real_everything_probe_falls_back_for_unindexed_tempdir() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("brand_new_file.txt"), b"x").unwrap();

    let output = Command::new(fde_exe())
        .env_remove("FDE_BACKEND")
        .env_remove("FDE_TEST_MOCK_HITS")
        .args([
            "--no-global-ignore-file",
            "brand_new_file",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn fde");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fde must exit 0 after probe-driven Legacy fallback. stderr={stderr}"
    );
    assert!(
        stdout.contains("brand_new_file.txt"),
        "Phase 8.7 probe must route unindexed tempdir to LegacyWalker, \
         which surfaces the on-disk file. got: {stdout}"
    );
}

/// PLAN §8h escape-hatch smoke: `--filesystem-walker` MUST find the
/// freshly-created tempfile. This is the user-facing complement to
/// the probe-fallback test above — the explicit flag and the
/// auto-probe both arrive at LegacyWalker, just via different
/// routes. Pinning both makes it obvious whether a future failure
/// breaks the probe (`real_everything_probe_falls_back_for_unindexed_tempdir`
/// fails) or the flag plumbing itself (this test fails).
#[test]
#[ignore = "requires Everything running; opt in with `--ignored`"]
fn real_everything_filesystem_walker_finds_unindexed_tempdir() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("brand_new_file.txt"), b"x").unwrap();

    let output = Command::new(fde_exe())
        .env_remove("FDE_BACKEND")
        .env_remove("FDE_TEST_MOCK_HITS")
        .args([
            "--no-global-ignore-file",
            "--filesystem-walker",
            "brand_new_file",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn fde");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "fde must exit 0 under --filesystem-walker. stderr={stderr}"
    );
    assert!(
        stdout.contains("brand_new_file.txt"),
        "--filesystem-walker must find the on-disk file; got: {stdout}"
    );
}
