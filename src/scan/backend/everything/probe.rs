//! PLAN.md §Phase 8.7 — real [`VolumeIndexProbe`] implementation.
//!
//! Until Phase 8.7 the routing layer used the [`AssumeIndexedProbe`]
//! stub from [`super::selection`], which always reported "indexed".
//! That stub was correct for the Phase 6 isolation tests but unusable
//! in production: a freshly-created tempdir is on an indexed volume
//! yet Everything hasn't seen any of its entries, so a routed query
//! would silently return zero hits. Phase 8.5 worked around this by
//! gating Everything routing behind `FDE_BACKEND=everything`.
//!
//! This module replaces the gate with an actual probe: run a
//! count-only `path:"<root>"` query against Everything. Three
//! outcomes:
//!
//! - **`num_results > 0`** — Everything has at least one entry under
//!   the root, so the volume is indexed and the root is in scope.
//!   Route to [`EverythingBackend`](super::backend::EverythingBackend).
//! - **`num_results == 0`, query succeeded** — Volume is reachable
//!   but Everything has nothing indexed under this path (typical of
//!   freshly-created tempdirs the indexer hasn't caught up to yet).
//!   Route to LegacyWalker so the user actually sees their files.
//! - **Query failed (any [`EverythingError`])** — Everything is
//!   unavailable or returned a programmer error. Fall back to
//!   LegacyWalker; the actual error path runs again later when the
//!   real query fires and surfaces a structured message there.
//!
//! ## Cost
//!
//! Each search root pays one extra `Everything_QueryW` round-trip
//! (count-only, no per-hit accessors). PLAN §Phase 8.7 explicitly
//! flagged this as acceptable: "代价是每个 root 多一次 IPC，可接受".
//! Typical fd invocations have one or two roots, so the overhead is
//! a single sub-millisecond IPC.

use std::path::Path;

use super::ffi;
use super::selection::VolumeIndexProbe;

/// Real probe: asks Everything whether a given path has any indexed
/// entries via a count-only query. See module-level docs for the
/// three-outcome contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct EverythingVolumeIndexProbe;

impl VolumeIndexProbe for EverythingVolumeIndexProbe {
    fn is_indexed(&self, path: &Path) -> bool {
        let search = build_probe_query(path);
        let utf16 = ffi::to_utf16_nul(&search);

        let sdk = ffi::sdk();
        // Probe is independent of any in-flight search state: reset
        // first so we don't inherit `set_match_path(true)` or stale
        // request flags from a prior caller in this thread.
        sdk.reset();
        sdk.set_match_case(false);
        sdk.set_match_path(false);
        sdk.set_regex(false);
        // Zero request flags: we only need the count, not per-hit
        // fields. Skipping FILE_NAME / PATH / SIZE etc. is what makes
        // this cheap compared to a real search.
        sdk.set_request_flags(0);
        // Cap at 1 so the SDK can stop populating its result list the
        // moment it has anything; `num_results()` still reports the
        // total count, which is all we read.
        sdk.set_max(1);
        sdk.set_search(&utf16);

        match sdk.query(true) {
            Ok(()) => sdk.num_results() > 0,
            // Both IPC unavailability and programmer-error variants
            // route to Legacy. For IPC, that's the documented R1
            // fallback. For other variants, falling back lets the
            // real search fire later and surface the structured
            // error through `EverythingBackend::run`'s error path,
            // which is the only place we have logging on.
            Err(_) => false,
        }
    }
}

/// Build the count-only probe query string. Mirrors the path-quoting
/// rule used by [`super::backend::build_search_string`] so the probe
/// and the real query agree on which root means which set of hits.
fn build_probe_query(root: &Path) -> String {
    // Slash normalization matches `backend::push_quoted_path`: Everything's
    // `path:` filter is backslash-only on Windows, so a `D:/foo` root
    // supplied by the user has to be flipped before it hits the SDK.
    let mut s = String::from("path:\"");
    for ch in root.to_string_lossy().chars() {
        if ch == '"' {
            continue;
        }
        let ch = if ch == '/' { '\\' } else { ch };
        s.push(ch);
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Encodes the PLAN §Phase 8.7 quoting contract: the probe string
    /// MUST wrap the root in Everything phrase quotes. A regression
    /// would make `path:` split on whitespace, mis-scoping the probe
    /// (e.g. `path:C:\Program Files` parses as two terms) and
    /// silently overestimating "indexed" coverage to include any path
    /// that happens to contain "Files".
    #[test]
    fn probe_query_quotes_the_root() {
        let q = build_probe_query(&PathBuf::from(r"C:\Program Files\foo"));
        assert_eq!(q, r#"path:"C:\Program Files\foo""#);
    }

    /// PLAN §6.1 quoting: an inner `"` in a path would terminate
    /// Everything's phrase quoting prematurely; strip them. Paths
    /// containing literal quotes are exotic enough that silently
    /// dropping the character is preferable to misreading the probe
    /// as "everything indexed" (which is what an unbalanced quote
    /// would produce).
    #[test]
    fn probe_query_strips_inner_quotes() {
        let q = build_probe_query(&PathBuf::from(r#"C:\weird"name"#));
        assert_eq!(q, r#"path:"C:\weirdname""#);
    }

    /// The probe must agree with `backend::push_quoted_path` on slash
    /// normalization — otherwise a `D:/foo` root would probe as
    /// "not indexed" (backslash-only match) and route to LegacyWalker,
    /// then the real query would ALSO run backslash-only, and the two
    /// paths would silently disagree on scope.
    #[cfg(windows)]
    #[test]
    fn probe_query_normalizes_forward_slashes_on_windows() {
        let q = build_probe_query(&PathBuf::from("D:/repo/sub"));
        assert_eq!(q, r#"path:"D:\repo\sub""#);
    }
}
