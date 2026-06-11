//! `EverythingBackend` — `SearchBackend` impl that drives the Everything SDK.
//!
//! Implements PLAN.md §Phase 5 (one-shot SetRequestFlags + structured error
//! surface) and §5.1 (RawHit metadata populated in a single SDK round-trip
//! so the post-filter pipeline never calls back into the filesystem).
//!
//! Scope note: this is the day-1 implementation. It uses the blocking
//! `Everything_QueryW(TRUE)` path; the hidden message window described in
//! PLAN §Phase 5 (for instant Ctrl-C interrupts) is deferred. Cancellation
//! is still honoured *between* hits, which is enough for cooperative
//! `--max-results` short-circuiting.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use super::error::EverythingError;
use super::ffi::{self, sys};
use crate::scan::backend::{
    BackendError, BackendQuery, BackendSink, CancellationToken, CaseModifier, PatternScope, RawHit,
    SearchBackend, TranslatedPattern,
};

/// Backend that delegates to the Everything SDK.
///
/// Stateless: every `run` call acquires the global SDK mutex from
/// [`super::ffi::sdk`], so multiple `EverythingBackend` instances coexist
/// safely.
#[derive(Debug, Default, Clone, Copy)]
pub struct EverythingBackend;

impl EverythingBackend {
    pub fn new() -> Self {
        Self
    }
}

/// One-shot request-flag bitfield (PLAN §5.1). Includes everything the post-
/// filter pipeline needs to read off a RawHit without a single fs::metadata
/// call. The mandatory hidden_by_attr lookup (§Phase 5 v7.2) is the only
/// post-filter syscall left.
pub(crate) const REQUEST_FLAGS: u32 = sys::EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME
    | sys::EVERYTHING_REQUEST_ATTRIBUTES
    | sys::EVERYTHING_REQUEST_SIZE
    | sys::EVERYTHING_REQUEST_DATE_MODIFIED
    | sys::EVERYTHING_REQUEST_DATE_CREATED
    | sys::EVERYTHING_REQUEST_EXTENSION;

/// Win32 `FILE_ATTRIBUTE_REPARSE_POINT`. Mirrored locally to avoid pulling
/// `windows-sys` for one constant; matches the definition in
/// `post_filter::filters::{type_filter,symlink_filter}`.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

impl SearchBackend for EverythingBackend {
    fn run(
        &self,
        query: &BackendQuery,
        sink: &mut dyn BackendSink,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        if query.paths.is_empty() {
            // fd always has at least one search root (defaults to ".").
            // An empty paths list would query the global Everything index,
            // which is never what we want.
            return Err(BackendError::Other(anyhow::anyhow!(
                "EverythingBackend requires at least one search root"
            )));
        }

        // One query per search root keeps the search_root => hit mapping
        // trivial — every hit emitted between two consecutive `run_one_root`
        // calls shares the same `Arc<PathBuf>`. Phase 6 may optimise to a
        // single merged Everything query later, but the prefix-matching
        // logic that would require is non-obvious; not worth it for v1.
        for root in &query.paths {
            if cancel.is_cancelled() {
                return Err(BackendError::Cancelled);
            }
            let root_arc = Arc::new(root.clone());
            run_one_root(query, &root_arc, sink, cancel)?;
        }
        Ok(())
    }
}

/// Drive a single Everything query scoped to `root`, streaming hits.
fn run_one_root(
    query: &BackendQuery,
    root: &Arc<PathBuf>,
    sink: &mut dyn BackendSink,
    cancel: &CancellationToken,
) -> Result<(), BackendError> {
    let search = build_search_string(query, root);
    let search_utf16 = ffi::to_utf16_nul(&search);

    let sdk = ffi::sdk();
    // Defensive: clear any prior state. The mutex guarantees no other thread
    // is mid-query, but a previous caller in *this* thread might have left
    // state behind (e.g. via an early-return error path).
    sdk.reset();
    sdk.set_match_case(false);
    sdk.set_match_path(false);
    sdk.set_regex(false);
    sdk.set_request_flags(REQUEST_FLAGS);
    if let Some(max) = query.max_results {
        // Everything_SetMax(0) means "no limit"; only set when we actually
        // have a finite cap.
        if let Ok(max32) = u32::try_from(max) {
            sdk.set_max(max32);
        }
    }
    sdk.set_search(&search_utf16);

    sdk.query(true).map_err(everything_to_backend_err)?;

    let total = sdk.num_results();
    for index in 0..total {
        if cancel.is_cancelled() {
            // Don't bother with Everything_Reset() here — the next `run`
            // does it unconditionally, and the SDK doesn't malloc per-hit.
            return Err(BackendError::Cancelled);
        }

        let hit = match build_raw_hit(&sdk, index, root) {
            Some(h) => h,
            None => continue, // skip hits we can't read (very unlikely)
        };
        // PLAN.md §Phase 8.7: skip hits whose path equals the
        // search root itself. Everything's `path:"<root>"` modifier
        // matches both the root entry and entries inside it, but
        // `ignore::WalkBuilder` (LegacyWalker) does NOT emit the
        // root — multi-root invocations like `fd '' a b c` would
        // otherwise show `a\`, `b\`, `c\` in addition to their
        // contents. The depth==0 check is equivalent to "path is
        // the root" per `depth_under_root` semantics.
        if hit.depth == 0 {
            continue;
        }
        sink.send(hit)?;
    }

    Ok(())
}

/// Read every field of a RawHit from the result list at `index`. Returns
/// `None` only if the full path is unreadable (which would point at SDK
/// corruption — log-and-skip rather than abort).
fn build_raw_hit(sdk: &ffi::SdkGuard, index: u32, root: &Arc<PathBuf>) -> Option<RawHit> {
    let path_os = sdk.result_full_path(index)?;
    let path = PathBuf::from(path_os);

    let attributes = sdk.result_attributes(index);
    let is_dir = classify_is_dir(sdk.is_folder_result(index), attributes);

    // PLAN §Phase 1 v7.1: every RawHit field is mandatory. If the SDK
    // refuses to return a value (e.g. flag missing from REQUEST_FLAGS),
    // fall back to a defined-but-meaningless value rather than panic so
    // the search still returns *something*. Phase 8 perf/integration tests
    // will catch any silent default-population.
    let size = sdk.result_size(index).unwrap_or(0);
    let mtime = sdk.result_date_modified(index).unwrap_or(0);
    let ctime = sdk.result_date_created(index).unwrap_or(0);

    let extension: Box<str> = sdk
        .result_extension(index)
        .to_string_lossy()
        .into_owned()
        .into_boxed_str();

    let depth = depth_under_root(&path, root);

    Some(RawHit {
        path,
        is_dir,
        size,
        mtime,
        ctime,
        attributes,
        extension,
        depth,
        search_root: Arc::clone(root),
    })
}

/// Count path components between `root` and `hit`, exclusive of `root`
/// itself. Matches `ignore::DirEntry::depth()` semantics: a direct child of
/// the root is depth 1, the root is depth 0. Falls back to 0 if `hit` is not
/// under `root` — which can happen if Everything returns a hit from a
/// symlinked location; treating it as "depth 0" puts it at the root, which
/// is the most lenient bound for max-depth filtering.
fn depth_under_root(hit: &Path, root: &Path) -> usize {
    match hit.strip_prefix(root) {
        Ok(rel) => rel.components().filter(is_named_component).count(),
        Err(_) => 0,
    }
}

fn is_named_component(c: &Component<'_>) -> bool {
    matches!(c, Component::Normal(_))
}

/// Decide whether a hit should populate `RawHit::is_dir`. Directory symlinks
/// (and other reparse points like junctions and mount points) set both
/// `FILE_ATTRIBUTE_DIRECTORY` and `FILE_ATTRIBUTE_REPARSE_POINT`, and
/// `Everything_IsFolderResult` returns true for them. The post-filter
/// pipeline and output formatter both treat `is_dir == true` as "plain
/// directory" — so a reparse point classified as a folder leaks a trailing
/// path separator into `--type l` output and confuses the symlink branch of
/// `type_filter`. Stripping the directory bit here makes the single RawHit
/// field the source of truth.
fn classify_is_dir(is_folder_result: bool, attributes: u32) -> bool {
    let is_reparse = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    is_folder_result && !is_reparse
}

/// Assemble the Everything search string from a `BackendQuery` scoped to one
/// `root`. PLAN §6.1 / §6.3 syntax:
///
/// ```text
/// [case:|nocase:] [nopath:|path:]<pattern>  [AND-pattern...]  path:"<root>"
/// ```
///
/// We always emit the case modifier explicitly so the global "Match Case"
/// toggle in the Everything GUI can't bleed in. Same for `path:` / `nopath:`
/// — we never rely on the global "Match Path" toggle, hence the
/// `set_match_path(false)` + `set_match_case(false)` + `set_regex(false)`
/// calls above.
fn build_search_string(query: &BackendQuery, root: &Path) -> String {
    let mut s = String::new();

    // Global case modifier (applies to the whole rest of the query).
    match query.pattern.case_modifier {
        CaseModifier::Case => s.push_str("case:"),
        CaseModifier::Nocase => s.push_str("nocase:"),
    }

    push_pattern_term(&mut s, &query.pattern);
    for and in &query.and_patterns {
        s.push(' ');
        push_pattern_term(&mut s, and);
    }

    s.push(' ');
    s.push_str("path:");
    push_quoted_path(&mut s, root);

    s
}

fn push_pattern_term(s: &mut String, pat: &TranslatedPattern) {
    match pat.scope {
        PatternScope::Basename => s.push_str("nopath:"),
        PatternScope::FullPath => s.push_str("path:"),
    }
    s.push_str(&pat.everything_query);
}

fn push_quoted_path(s: &mut String, root: &Path) {
    // Everything's phrase syntax: "..." treats most metacharacters literally.
    // We don't escape quotes — a search root containing `"` is exotic enough
    // that Phase 6's translation layer will reject it before we get here.
    // Defensive: strip any inner quotes so we can't construct a malformed
    // query that would silently widen the search.
    s.push('"');
    for ch in root.to_string_lossy().chars() {
        if ch != '"' {
            s.push(ch);
        }
    }
    s.push('"');
}

fn everything_to_backend_err(e: EverythingError) -> BackendError {
    // Both unavailable (IPC) and programmer-error variants surface as
    // `Other`. The backend-selection layer in Phase 6 inspects
    // `EverythingError::is_unavailable()` BEFORE building this backend; if
    // we get here the SDK was thought to be available and then died, which
    // is a hard error worth surfacing.
    BackendError::Other(anyhow::Error::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::backend::{CaseModifier, PatternScope, TranslatedPattern};

    fn pattern(q: &str, scope: PatternScope) -> TranslatedPattern {
        TranslatedPattern {
            everything_query: q.to_string(),
            scope,
            case_modifier: CaseModifier::Nocase,
        }
    }

    fn base_query() -> BackendQuery {
        BackendQuery {
            paths: vec![PathBuf::from(r"C:\repo")],
            pattern: pattern("regex:foo", PatternScope::Basename),
            and_patterns: vec![],
            type_hint: None,
            size_hint: None,
            time_hint: None,
            max_depth: None,
            max_results: None,
        }
    }

    /// Encodes the PLAN §6.1 contract: every search string emitted by this
    /// backend pins case-mode explicitly so the Everything GUI's "Match
    /// Case" toggle cannot leak into headless query results. If this ever
    /// regresses, identical fd invocations produce different results
    /// depending on whether the user clicked a checkbox.
    #[test]
    fn search_string_always_pins_case_modifier() {
        let q = base_query();
        let s = build_search_string(&q, &q.paths[0]);
        assert!(
            s.starts_with("nocase:") || s.starts_with("case:"),
            "search must start with explicit case modifier; got {s}"
        );
    }

    /// PLAN §6.1: pattern scope is encoded inline as `nopath:` / `path:`,
    /// never via the SDK-level `SetMatchPath` toggle. Regression here would
    /// let `--full-path` interfere across concurrent queries.
    #[test]
    fn pattern_scope_emits_inline_modifier() {
        let mut q = base_query();
        q.pattern = pattern("regex:foo", PatternScope::Basename);
        assert!(build_search_string(&q, &q.paths[0]).contains("nopath:regex:foo"));

        q.pattern = pattern("regex:foo", PatternScope::FullPath);
        let s = build_search_string(&q, &q.paths[0]);
        // The pattern must carry its own `path:` and the root must also
        // appear behind a `path:` — there's no ambiguity because the root
        // is the LAST `path:` term and Everything ANDs them.
        assert!(s.contains("path:regex:foo"));
        assert!(s.contains(r#"path:"C:\repo""#));
    }

    /// PLAN §6.4: when multiple paths are supplied, EverythingBackend runs
    /// one query per root and tags the resulting RawHits with the matching
    /// search_root Arc. The string construction itself takes a single root
    /// — verify it embeds that root verbatim.
    #[test]
    fn search_string_embeds_root_in_quotes() {
        let q = base_query();
        let s = build_search_string(&q, &PathBuf::from(r"C:\repo\sub"));
        assert!(
            s.contains(r#"path:"C:\repo\sub""#),
            "search must end with path:\"<root>\"; got {s}"
        );
    }

    /// PLAN §6.1 AND-pattern semantics: each `--and` clause becomes another
    /// space-separated term with its own scope modifier. Order is preserved
    /// because Everything's AND is commutative for matching but not for
    /// short-circuit evaluation — keeping order makes diff-debugging easier.
    #[test]
    fn and_patterns_append_with_own_scope() {
        let mut q = base_query();
        q.and_patterns = vec![
            pattern("regex:bar", PatternScope::Basename),
            pattern("regex:baz", PatternScope::FullPath),
        ];
        let s = build_search_string(&q, &q.paths[0]);
        assert!(s.contains("nopath:regex:foo"));
        assert!(s.contains("nopath:regex:bar"));
        assert!(s.contains("path:regex:baz"));
    }

    /// PLAN §5.1 depth contract: direct children of the root are depth 1,
    /// the root itself is depth 0, deeper hits scale linearly. Matches
    /// `ignore::DirEntry::depth()` so the LegacyWalkerBackend and
    /// EverythingBackend stay interchangeable inside the post-filter
    /// pipeline.
    #[test]
    fn depth_under_root_matches_ignore_walker_semantics() {
        let root = PathBuf::from(r"C:\repo");
        assert_eq!(depth_under_root(&root, &root), 0);
        assert_eq!(depth_under_root(&PathBuf::from(r"C:\repo\a.txt"), &root), 1);
        assert_eq!(
            depth_under_root(&PathBuf::from(r"C:\repo\src\foo\b.txt"), &root),
            3
        );
    }

    /// Defensive: paths that don't fall under the search root (which
    /// shouldn't happen but might if Everything follows a junction) are
    /// reported as depth 0 — the most permissive bound. Per PLAN §3.2 we
    /// prefer "post-filter passes through" over "silently drops" for
    /// edge-case path shapes.
    #[test]
    fn depth_under_root_falls_back_to_zero_when_not_a_prefix() {
        let root = PathBuf::from(r"C:\repo");
        assert_eq!(depth_under_root(&PathBuf::from(r"D:\other\x"), &root), 0);
    }

    /// PLAN §Phase 8.7.1 MUST 1: directory symlinks and other reparse
    /// points must NOT be classified as plain folders. `Everything_Is`
    /// `FolderResult` returns true for any entry with the directory bit
    /// set — including directory junctions and symlinks — but the
    /// post-filter pipeline (`type_filter` treats `is_dir && !symlink` as
    /// "plain dir") and the output formatter (appends trailing `\`/`/`
    /// for `is_dir`) both rely on `RawHit::is_dir` meaning "plain
    /// directory only". Regression here surfaces as `--type l` printing
    /// `link\` with a trailing separator and `--type d` over-counting.
    #[test]
    fn directory_symlink_is_classified_as_symlink_not_folder() {
        // FILE_ATTRIBUTE_DIRECTORY (0x10) | FILE_ATTRIBUTE_REPARSE_POINT
        // (0x400) — the exact attribute bitmask Win32 reports for a
        // directory symlink, junction, or mount point.
        let attrs = 0x10 | FILE_ATTRIBUTE_REPARSE_POINT;
        assert!(
            !classify_is_dir(true, attrs),
            "reparse-point folder must not be RawHit.is_dir"
        );
        // Sanity: a plain directory (no reparse bit) still classifies as
        // a folder, otherwise we'd silently demote every dir hit.
        assert!(classify_is_dir(true, 0x10));
        // A non-folder reparse point (file symlink) should keep is_dir
        // false; the symlink_filter handles the symlink-ness independently.
        assert!(!classify_is_dir(false, FILE_ATTRIBUTE_REPARSE_POINT));
    }

    /// PLAN §6.1 quoting: paths with spaces or `:` characters (e.g. drive
    /// letters) MUST be wrapped in Everything phrase quotes so the parser
    /// doesn't split them on whitespace or treat `:` as a modifier
    /// separator. This is the failure mode that would silently widen
    /// every search on a path with a space in it.
    #[test]
    fn search_string_quotes_paths_containing_spaces() {
        let mut q = base_query();
        q.paths = vec![PathBuf::from(r"C:\Program Files")];
        let s = build_search_string(&q, &q.paths[0]);
        assert!(s.contains(r#"path:"C:\Program Files""#), "got {s}");
    }
}
