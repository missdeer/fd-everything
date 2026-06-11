//! PLAN.md §Phase 8.6 / Phase 8 §8g: process-level mock injection for the
//! `EverythingBackend` path.
//!
//! When `FDE_TEST_MOCK_HITS=<path>` is set and the routing layer would
//! otherwise invoke `EverythingBackend`, the dispatcher instead constructs a
//! [`MockHitFileBackend`] that reads a small text-format hit list from
//! `<path>`, replays it through the same `PostFilterSink` / `RawHitBatchSink`
//! chain, and exits. This lets `tests/mock_e2e.rs` exercise the
//! Phase 8.5-D wiring end-to-end without depending on a real Everything
//! service.
//!
//! ## File format
//!
//! - UTF-8, one hit per line.
//! - Blank lines and lines starting with `#` are ignored.
//! - Each hit line: `<absolute_path>` or `<absolute_path>\t<dir|file>`.
//!   Missing kind defaults to `file`.
//! - Metadata not in the format (size / mtime / ctime / attributes) is
//!   zeroed; the depth field is set to 1 so the depth filter still sees a
//!   sensible relationship with `--max-depth`. Tests that need to probe
//!   richer metadata can extend the format later — the parser is forward-
//!   compatible because unknown tab-separated fields are simply dropped.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::scan::backend::{
    BackendError, BackendQuery, BackendSink, CancellationToken, RawHit, SearchBackend,
};

/// One parsed mock hit. Kept minimal — see module docs for rationale on
/// the omitted metadata fields.
#[derive(Debug, Clone)]
struct MockHit {
    path: PathBuf,
    is_dir: bool,
}

/// PLAN.md §Phase 8.6: a `SearchBackend` that replays a pre-parsed hit
/// list. Used only when `FDE_TEST_MOCK_HITS` is set; production callers
/// hold an [`EverythingBackend`](super::backend::everything::EverythingBackend)
/// instead.
#[derive(Debug)]
pub(crate) struct MockHitFileBackend {
    hits: Vec<MockHit>,
    source: PathBuf,
}

impl MockHitFileBackend {
    fn from_text(source: PathBuf, body: &str) -> Result<Self> {
        let mut hits = Vec::new();
        for (line_idx, raw) in body.lines().enumerate() {
            let line = raw.trim_end_matches(['\r', '\n']);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split('\t');
            let path = fields
                .next()
                .map(PathBuf::from)
                .with_context(|| format!("empty path at {}:{}", source.display(), line_idx + 1))?;
            let is_dir = match fields.next() {
                None | Some("") | Some("file") => false,
                Some("dir") => true,
                Some(other) => bail!(
                    "unknown kind {other:?} at {}:{}; expected `dir` or `file`",
                    source.display(),
                    line_idx + 1
                ),
            };
            hits.push(MockHit { path, is_dir });
        }
        Ok(Self { hits, source })
    }
}

impl SearchBackend for MockHitFileBackend {
    fn run(
        &self,
        query: &BackendQuery,
        sink: &mut dyn BackendSink,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        // Stamp every hit with the query's first search root so the
        // post-filter pipeline (IgnoreCache, hidden_by_name, depth, etc.)
        // sees the same `search_root` that a real EverythingBackend would
        // have populated. Tests typically pass exactly one root, but if
        // they pass more, the rest are silently ignored — symmetric with
        // how `MockBackend` in `backend.rs` ignores `BackendQuery`.
        let root_arc: Arc<PathBuf> = query
            .paths
            .first()
            .cloned()
            .map(Arc::new)
            .unwrap_or_else(|| Arc::new(self.source.clone()));

        for hit in &self.hits {
            if cancel.is_cancelled() {
                return Err(BackendError::Cancelled);
            }
            let extension: Box<str> = hit
                .path
                .extension()
                .and_then(OsStr::to_str)
                .unwrap_or("")
                .to_string()
                .into_boxed_str();
            let raw = RawHit {
                path: hit.path.clone(),
                is_dir: hit.is_dir,
                size: 0,
                mtime: 0,
                ctime: 0,
                attributes: 0,
                extension,
                // Depth = 1 mirrors "direct child of search root" which is
                // the only legible default without round-tripping through
                // `strip_prefix`; tests that need richer depth semantics
                // can rely on the depth filter still applying.
                depth: 1,
                search_root: Arc::clone(&root_arc),
            };
            sink.send(raw)?;
        }
        Ok(())
    }
}

/// PLAN.md §Phase 8.6 dispatch helper: look up `FDE_TEST_MOCK_HITS`,
/// load the file, and parse it. `Ok(None)` means the env var is unset;
/// `Ok(Some(...))` is the constructed backend; `Err` surfaces file I/O
/// or parse errors so the caller can convert them into a
/// `WorkerResult::Error` rather than silently falling through.
pub(crate) fn try_load_from_env() -> Result<Option<MockHitFileBackend>> {
    let Some(raw) = env::var_os("FDE_TEST_MOCK_HITS") else {
        return Ok(None);
    };
    let path = PathBuf::from(raw);
    let body = fs::read_to_string(&path)
        .with_context(|| format!("reading FDE_TEST_MOCK_HITS={}", path.display()))?;
    let backend = MockHitFileBackend::from_text(path, &body)?;
    Ok(Some(backend))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::scan::backend::{BackendQuery, CaseModifier, PatternScope, TranslatedPattern};

    fn dummy_query(root: PathBuf) -> BackendQuery {
        BackendQuery {
            paths: vec![root],
            pattern: TranslatedPattern {
                everything_query: String::new(),
                scope: PatternScope::Basename,
                case_modifier: CaseModifier::Nocase,
            },
            and_patterns: vec![],
            type_hint: None,
            size_hint: None,
            time_hint: None,
            max_depth: None,
            max_results: None,
        }
    }

    struct VecSink(Vec<RawHit>);
    impl BackendSink for VecSink {
        fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
            self.0.push(hit);
            Ok(())
        }
    }

    /// Encodes the §8g parser contract: blank lines and `#` comments are
    /// skipped; `path` alone defaults to `file`; `path<TAB>dir` flips the
    /// `is_dir` bit. A regression here would make every `tests/mock_e2e`
    /// fixture behave like a flat file stream, silently dropping the
    /// directory-aware filters' coverage.
    #[test]
    fn parser_handles_blank_lines_comments_and_kinds() {
        let body = "\
# header comment
C:\\repo\\a.txt
C:\\repo\\sub\tdir

C:\\repo\\b.log\tfile
# trailing comment
";
        let backend = MockHitFileBackend::from_text(PathBuf::from("X"), body).unwrap();
        assert_eq!(backend.hits.len(), 3);
        assert_eq!(backend.hits[0].path, PathBuf::from(r"C:\repo\a.txt"));
        assert!(!backend.hits[0].is_dir);
        assert_eq!(backend.hits[1].path, PathBuf::from(r"C:\repo\sub"));
        assert!(backend.hits[1].is_dir);
        assert!(!backend.hits[2].is_dir);
    }

    /// Encodes the §8g run contract: every parsed hit reaches the sink
    /// stamped with the BackendQuery's first path as its search_root, so
    /// downstream IgnoreCache lookups see the same shape they would for
    /// a real EverythingBackend hit. If the search_root drifts from the
    /// query, every `--no-ignore-parent` test would silently widen.
    #[test]
    fn run_stamps_hits_with_query_search_root() {
        let body = "C:\\repo\\a.txt\nC:\\repo\\b.txt\n";
        let backend = MockHitFileBackend::from_text(PathBuf::from("X"), body).unwrap();
        let root = PathBuf::from(r"C:\repo");
        let q = dummy_query(root.clone());
        let mut sink = VecSink(Vec::new());
        let cancel = CancellationToken::new();
        backend.run(&q, &mut sink, &cancel).unwrap();
        assert_eq!(sink.0.len(), 2);
        for hit in &sink.0 {
            assert_eq!(hit.search_root.as_path(), Path::new(r"C:\repo"));
        }
    }

    /// Encodes the §8g cancellation contract: a token that fires
    /// mid-stream stops the run between hits. Required so
    /// `--max-results` and SIGPIPE behave the same when the test
    /// harness drives MockHitFileBackend as when production drives
    /// EverythingBackend.
    #[test]
    fn run_honours_cancellation_token() {
        let body = "a\nb\nc\n";
        let backend = MockHitFileBackend::from_text(PathBuf::from("X"), body).unwrap();
        let q = dummy_query(PathBuf::from(r"C:\"));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut sink = VecSink(Vec::new());
        let err = backend
            .run(&q, &mut sink, &cancel)
            .expect_err("cancel must surface");
        assert!(matches!(err, BackendError::Cancelled));
        assert!(sink.0.is_empty());
    }

    /// PLAN §8g parser MUST reject unknown kinds rather than silently
    /// downgrading to `file`. If we accepted any garbage past the path
    /// column, a typo in a test fixture (`dr` instead of `dir`) would
    /// silently change every hit's is_dir to false and the test would
    /// pass for the wrong reason.
    #[test]
    fn parser_rejects_unknown_kind_token() {
        let err = MockHitFileBackend::from_text(PathBuf::from("X"), "a\tdr\n")
            .expect_err("unknown kind must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("dr"),
            "error message must echo the bad token: {msg}"
        );
    }
}
