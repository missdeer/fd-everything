//! PLAN.md §Phase 2 PathProjector.
//!
//! A `DirEntry` now stores a single, canonicalized, absolute `raw_path`
//! (see `dir_entry::DirEntry`). Every internal consumer that needs to call
//! `stat`, `GetFileAttributesW`, match an ignore rule or look up a volume
//! id reads `raw_path` directly. The only place fd needs the
//! "user-friendly" projected form (e.g. `./src/main.rs` instead of
//! `C:\repo\src\main.rs`) is at output time and inside `--exec` placeholder
//! substitution.
//!
//! `PathProjector` is that conversion. It is configured once at scan
//! start with the user-given search-root display forms plus the relevant
//! CLI flags, and produces a `Cow<'_, Path>` per hit — zero-alloc when the
//! borrow path is hit (e.g. `--absolute-path`), one allocation otherwise.
//!
//! Invariant (PLAN.md C1): for any `Config` and fixture tree, the bytes
//! produced by `project_for_output` MUST equal what stock fd would print.
//! This is enforced by golden-diff tests (`Phase 2.5`).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::filesystem::strip_current_dir;

/// A search root in two forms: the canonical absolute path used to match
/// every hit's `raw_path`, and the display form as supplied by the user
/// (could be relative like `./` or `src/`, or absolute like `C:\repo`).
///
/// `display` is what gets glued back onto the relative portion of
/// `raw_path` to reproduce stock fd's output layout.
#[derive(Debug, Clone)]
pub struct SearchRoot {
    pub canonical: PathBuf,
    pub display: PathBuf,
}

/// Stateless projector. Constructed once per scan from `Config`.
#[derive(Debug, Clone)]
pub struct PathProjector<'a> {
    /// Sorted longest-first by `canonical.as_os_str().len()` so a path that
    /// falls under multiple roots gets the most-specific one.
    roots: Cow<'a, [SearchRoot]>,
    /// `--absolute-path`. When set, raw_path is returned verbatim.
    absolute_paths: bool,
    /// `--strip-cwd-prefix`. Trims a leading `./` from the projected path
    /// when applicable (see `filesystem::strip_current_dir`).
    strip_cwd_prefix: bool,
}

impl<'a> PathProjector<'a> {
    /// Build a projector over a borrowed slice of search roots.
    pub fn new(roots: &'a [SearchRoot], absolute_paths: bool, strip_cwd_prefix: bool) -> Self {
        // The hot path is `starts_with`; we sort once so the lookup walks
        // longest roots first and can early-exit. Cow::Borrowed when the
        // input is already sorted, Owned otherwise.
        let needs_sort = roots
            .windows(2)
            .any(|w| w[0].canonical.as_os_str().len() < w[1].canonical.as_os_str().len());
        let roots = if needs_sort {
            let mut owned = roots.to_vec();
            owned.sort_by(|a, b| {
                b.canonical
                    .as_os_str()
                    .len()
                    .cmp(&a.canonical.as_os_str().len())
            });
            Cow::Owned(owned)
        } else {
            Cow::Borrowed(roots)
        };
        Self {
            roots,
            absolute_paths,
            strip_cwd_prefix,
        }
    }

    /// Construct an owning projector — used by tests and by call sites
    /// that build a one-shot projector from a `Vec`.
    //
    // Phase 5 will introduce the production caller; until then the
    // borrowed `new(...)` constructor is the only consumer.
    #[allow(dead_code)]
    pub fn owned(
        roots: Vec<SearchRoot>,
        absolute_paths: bool,
        strip_cwd_prefix: bool,
    ) -> PathProjector<'static> {
        let mut roots = roots;
        roots.sort_by(|a, b| {
            b.canonical
                .as_os_str()
                .len()
                .cmp(&a.canonical.as_os_str().len())
        });
        PathProjector {
            roots: Cow::Owned(roots),
            absolute_paths,
            strip_cwd_prefix,
        }
    }

    /// Project `raw_path` (assumed absolute & canonicalized) into the form
    /// fd would print for it under the current `Config`.
    pub fn project_for_output<'p>(&self, raw_path: &'p Path) -> Cow<'p, Path> {
        if self.absolute_paths {
            return Cow::Borrowed(raw_path);
        }

        let Some(root) = self.find_root(raw_path) else {
            // Path lies outside every search root. We never expect this in
            // practice (the backend wouldn't yield it), but degrade to the
            // raw absolute path rather than panic so a backend bug surfaces
            // visibly as "ugly output" instead of a crash.
            return Cow::Borrowed(raw_path);
        };

        // Strip the canonical prefix to get the relative tail, then re-glue
        // onto the user-given display root.
        //
        // strip_prefix returns Err if raw_path == canonical exactly (it
        // doesn't, it returns Ok("")), or if matching fails. The find_root
        // pre-check guarantees a match here.
        let rel = raw_path
            .strip_prefix(&root.canonical)
            .unwrap_or(Path::new(""));

        // Fast path: display root is the same as canonical (user passed an
        // absolute path). Re-gluing would produce the same bytes as
        // raw_path — skip the allocation.
        if root.display.as_os_str() == root.canonical.as_os_str() {
            return if self.strip_cwd_prefix {
                Cow::Borrowed(strip_current_dir(raw_path))
            } else {
                Cow::Borrowed(raw_path)
            };
        }

        let mut joined = root.display.join(rel);
        if self.strip_cwd_prefix {
            // strip_current_dir borrows; we need an owned PathBuf to return
            // Cow::Owned, so collect after stripping.
            joined = strip_current_dir(&joined).to_path_buf();
        }
        Cow::Owned(joined)
    }

    fn find_root(&self, raw_path: &Path) -> Option<&SearchRoot> {
        // roots is sorted longest-first; first prefix match wins.
        self.roots
            .iter()
            .find(|r| raw_path.starts_with(&r.canonical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(canonical: &str, display: &str) -> SearchRoot {
        SearchRoot {
            canonical: PathBuf::from(canonical),
            display: PathBuf::from(display),
        }
    }

    /// Default fd invocation: `fd foo` inside `C:\repo`. Walker root is
    /// passed as `./`, so the display form is `./` and projection must
    /// re-prefix every hit. This is the workhorse case — if it breaks,
    /// every default-config test in the suite fails.
    #[test]
    fn projects_relative_root_back_onto_canonical_hit() {
        let roots = vec![root(r"C:\repo", r"./")];
        let p = PathProjector::owned(roots, false, false);
        let out = p.project_for_output(Path::new(r"C:\repo\src\main.rs"));
        assert_eq!(out.as_ref(), Path::new(r"./src\main.rs"));
    }

    /// `--strip-cwd-prefix` is the user opting out of the leading `./`.
    /// Same scenario as above but with the strip flag on, so the projected
    /// path drops the prefix.
    #[test]
    fn strip_cwd_prefix_drops_leading_dot_slash() {
        let roots = vec![root(r"C:\repo", r"./")];
        let p = PathProjector::owned(roots, false, true);
        let out = p.project_for_output(Path::new(r"C:\repo\src\main.rs"));
        assert_eq!(out.as_ref(), Path::new(r"src\main.rs"));
    }

    /// `--absolute-path` short-circuits the projector and must always
    /// borrow `raw_path` for zero-alloc output (C1 hot-path invariant).
    #[test]
    fn absolute_paths_borrow_raw_path_zero_alloc() {
        let roots = vec![root(r"C:\repo", r"./")];
        let p = PathProjector::owned(roots, true, false);
        let raw = Path::new(r"C:\repo\src\main.rs");
        let out = p.project_for_output(raw);
        assert!(matches!(out, Cow::Borrowed(_)), "must be borrowed");
        assert_eq!(out.as_ref(), raw);
    }

    /// `fd foo C:\Windows` — user supplied an absolute search root.
    /// Display == canonical, so the fast path borrows raw_path with no
    /// allocation (and the output stays absolute, matching stock fd).
    #[test]
    fn absolute_display_root_takes_borrowed_fast_path() {
        let roots = vec![root(r"C:\Windows", r"C:\Windows")];
        let p = PathProjector::owned(roots, false, false);
        let raw = Path::new(r"C:\Windows\System32\cmd.exe");
        let out = p.project_for_output(raw);
        assert!(matches!(out, Cow::Borrowed(_)), "absolute root → borrow");
        assert_eq!(out.as_ref(), raw);
    }

    /// `fd foo src/` — root is relative but not `./`. Projection must
    /// faithfully re-prefix as `src\...`, not `./src\...`.
    #[test]
    fn projects_non_dot_relative_root() {
        let roots = vec![root(r"C:\repo\src", r"src")];
        let p = PathProjector::owned(roots, false, false);
        let out = p.project_for_output(Path::new(r"C:\repo\src\bin\fd.rs"));
        assert_eq!(out.as_ref(), Path::new(r"src\bin\fd.rs"));
    }

    /// Multi-root case: when several search roots could match, the
    /// longest-canonical-prefix wins. Without this, a deeper root nested
    /// inside an outer root would mis-glue display forms and produce
    /// duplicate path segments in fd output.
    #[test]
    fn longest_canonical_root_wins_for_nested_roots() {
        let roots = vec![
            root(r"C:\repo", r"./"),
            root(r"C:\repo\vendor\thirdparty", r"vendor/thirdparty"),
        ];
        let p = PathProjector::owned(roots, false, false);
        let out = p.project_for_output(Path::new(r"C:\repo\vendor\thirdparty\lib\x.rs"));
        // Must pick the deeper root, projecting to `vendor/thirdparty\lib\x.rs`,
        // NOT `./vendor\thirdparty\lib\x.rs` (which would double-up under the
        // outer root). The forward-slash inside the display root is preserved.
        assert_eq!(out.as_ref(), Path::new(r"vendor/thirdparty\lib\x.rs"));
    }

    /// UNC paths: `\\server\share\dir\file` is absolute. When the user
    /// supplies it as a search root, projection must round-trip cleanly
    /// through the absolute-display fast path.
    #[test]
    fn unc_search_root_round_trips() {
        let roots = vec![root(r"\\server\share", r"\\server\share")];
        let p = PathProjector::owned(roots, false, false);
        let raw = Path::new(r"\\server\share\dir\file.txt");
        let out = p.project_for_output(raw);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), raw);
    }

    /// Windows long-path `\\?\` prefix is a valid canonical form returned
    /// by `std::fs::canonicalize` on long paths. When that form is the
    /// search root, the projector must still pass it through unchanged.
    #[test]
    fn verbatim_long_path_prefix_passes_through() {
        let roots = vec![root(r"\\?\C:\very\long\root", r"\\?\C:\very\long\root")];
        let p = PathProjector::owned(roots, false, false);
        let raw = Path::new(r"\\?\C:\very\long\root\sub\f.txt");
        let out = p.project_for_output(raw);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), raw);
    }

    /// PLAN.md §Phase 2.5 sentinel.
    ///
    /// Before this phase, the legacy display projection was just
    /// `filesystem::strip_current_dir(walker_yielded_path)` — and the
    /// walker yielded `./relative` paths under the default `./` root.
    /// This test pins the contract that the new projector reproduces
    /// that legacy byte sequence exactly under the equivalent Config,
    /// for both the `strip_cwd_prefix=true` and `=false` arms. The full
    /// matrix (absolute path, base-directory, print0, hyperlink target,
    /// exec placeholder substitution, list-details) is covered
    /// end-to-end by `tests/tests.rs` — those run fd against on-disk
    /// fixture trees and assert byte-equal output, which is the true
    /// C1 golden diff. This unit-level sentinel exists so a future
    /// refactor of the projector itself fails before the heavier
    /// integration suite has to spin up a process.
    #[test]
    fn projector_default_arm_reproduces_legacy_strip_behavior() {
        let roots = vec![root(r"C:\repo", r"./")];
        let raw = Path::new(r"C:\repo\src\main.rs");

        // strip_cwd_prefix = false: matches legacy stripped_path() when
        // strip_cwd_prefix is off — fd printed `./src\main.rs`.
        let keep = PathProjector::owned(roots.clone(), false, false);
        assert_eq!(
            keep.project_for_output(raw).as_ref(),
            Path::new(r"./src\main.rs"),
            "default arm must keep leading './' to match legacy fd output",
        );

        // strip_cwd_prefix = true: matches legacy strip_current_dir(...)
        // — fd printed `src\main.rs`.
        let strip = PathProjector::owned(roots, false, true);
        assert_eq!(
            strip.project_for_output(raw).as_ref(),
            Path::new(r"src\main.rs"),
            "strip arm must drop leading './' to match legacy fd output",
        );
    }

    /// Path that lies outside every search root must not panic. We don't
    /// expect a real backend to emit such a path, but a bug should surface
    /// as "ugly absolute output" rather than a crash.
    #[test]
    fn path_outside_roots_falls_back_to_raw() {
        let roots = vec![root(r"C:\repo", r"./")];
        let p = PathProjector::owned(roots, false, false);
        let raw = Path::new(r"C:\elsewhere\x.txt");
        let out = p.project_for_output(raw);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), raw);
    }
}
