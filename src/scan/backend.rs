//! Search-backend trait surface (PLAN.md §Phase 1, v7.1 RawHit revision).
//!
//! The Phase 1 scope is strictly the abstraction and an in-memory mock — no
//! integration with the existing `walk::scan` pipeline (which still drives
//! the Phase 5 EverythingBackend integration is what flips the switch). Field
//! shape and naming come directly from PLAN.md §Phase 1 and must match so that
//! Phase 3 (post-filter pipeline) can be wired against `MockBackend` without
//! further churn.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Phase 5 (PLAN.md §Phase 5): EverythingBackend lives in this submodule.
// Windows-only; on other targets the trait/MockBackend in this file are still
// available so post-filter/IgnoreCache tests keep compiling.
#[cfg(target_os = "windows")]
pub mod everything;

// ---------------------------------------------------------------------------
// Query inputs
// ---------------------------------------------------------------------------

/// Description of a backend invocation. Already-normalized: callers (Phase 6
/// query-translation + backend-selection layer) are responsible for
/// canonicalizing paths and translating the user pattern to
/// [`TranslatedPattern`].
#[derive(Debug, Clone)]
pub struct BackendQuery {
    /// Canonicalized absolute search roots.
    pub paths: Vec<PathBuf>,
    /// Primary translated pattern. Phase 6 produces this from the CLI regex /
    /// glob / fixed-string state.
    pub pattern: TranslatedPattern,
    /// `--and` patterns (each already translated).
    pub and_patterns: Vec<TranslatedPattern>,
    /// File-vs-directory hint that can be pushed down into the backend when
    /// available. Post-filter still re-checks (see PLAN.md §3.2).
    pub type_hint: Option<EntryTypeHint>,
    pub size_hint: Option<SizeRange>,
    pub time_hint: Option<TimeRange>,
    pub max_depth: Option<usize>,
    pub max_results: Option<usize>,
}

/// Translated pattern ready for the EverythingBackend. The
/// LegacyWalkerBackend ignores this and uses its own state.
#[derive(Debug, Clone)]
pub struct TranslatedPattern {
    /// Everything query fragment, e.g. `regex:^foo$`, `wildcards:*.rs`, or a
    /// phrase-form literal.
    pub everything_query: String,
    pub scope: PatternScope,
    pub case_modifier: CaseModifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternScope {
    /// Match basename only (Everything `nopath:`).
    Basename,
    /// Match full path (Everything `path:`).
    FullPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseModifier {
    /// Case-sensitive (`case:`).
    Case,
    /// Case-insensitive (`nocase:`; Everything default).
    Nocase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryTypeHint {
    File,
    Directory,
}

/// Half-open size range in bytes. `None` on either side means unbounded on
/// that side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeRange {
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
}

/// Half-open time range expressed as Windows FILETIME (100ns ticks since
/// 1601-01-01 UTC). FILETIME chosen to match [`RawHit::mtime`] /
/// [`RawHit::ctime`] without an intermediate `SystemTime` conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub min_filetime: Option<i64>,
    pub max_filetime: Option<i64>,
}

// ---------------------------------------------------------------------------
// Streaming hit type (v7.1: all fields non-Option, one shot from backend)
// ---------------------------------------------------------------------------

/// A single search hit emitted by a backend. Field shape matches PLAN.md
/// §Phase 1 v7.1 exactly — every field is mandatory because the post-filter
/// pipeline (Phase 3) is forbidden from issuing `stat` calls outside of
/// `hidden_filter`, so the backend must populate all metadata up-front.
#[derive(Debug, Clone)]
pub struct RawHit {
    /// Absolute canonicalized path.
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// Windows FILETIME (100ns ticks since 1601-01-01 UTC).
    pub mtime: i64,
    pub ctime: i64,
    /// Win32 `FILE_ATTRIBUTE_*` bitmask.
    pub attributes: u32,
    /// File extension (without leading dot), or empty string. `Box<str>`
    /// shaves 16 bytes per hit vs `String` on 64-bit.
    pub extension: Box<str>,
    /// Depth relative to the originating search root. EverythingBackend
    /// computes this from `path` and `search_root`; LegacyWalkerBackend gets
    /// it from `ignore::DirEntry::depth()`.
    pub depth: usize,
    /// Shared pointer to the search root that produced this hit. Required by
    /// hidden_filter and the Phase 2 PathProjector; `Arc` lets us share a
    /// single allocation across every hit from the same root.
    pub search_root: Arc<PathBuf>,
}

// ---------------------------------------------------------------------------
// Sink + cancellation + error
// ---------------------------------------------------------------------------

/// Streaming sink consumed by [`SearchBackend::run`]. Phase 3 wires this to
/// the post-filter pipeline; Phase 1 mock tests can use any `impl`.
pub trait BackendSink {
    fn send(&mut self, hit: RawHit) -> Result<(), BackendError>;
}

/// Cheap clone, multiple producers / multiple consumers. Backends must check
/// `is_cancelled()` between hits (or at least at a bounded cadence) so the
/// Phase 3 `max_results_limiter` can short-circuit Everything queries on
/// large indices.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    flag: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub enum BackendError {
    /// Cooperative cancellation: a [`CancellationToken`] fired, or the sink
    /// signalled that downstream consumers stopped reading (e.g. SIGPIPE).
    Cancelled,
    /// Anything else. Carrying `anyhow::Error` here keeps the surface small
    /// at Phase 1 — Phase 5 (FFI) and Phase 6 (translation) will refine this
    /// into typed variants if call sites need to discriminate.
    Other(anyhow::Error),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("backend run cancelled"),
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cancelled => None,
            Self::Other(err) => Some(err.as_ref()),
        }
    }
}

impl From<anyhow::Error> for BackendError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub trait SearchBackend: Send + Sync {
    /// Push raw hits into `sink`. Implementations MUST stream — call
    /// `sink.send(...)` as hits arrive rather than buffering the full
    /// result set — so that `--exec`, `--max-results` and SIGPIPE on large
    /// indices stay responsive (PLAN.md §Phase 1, §Phase 3 max_results).
    fn run(
        &self,
        query: &BackendQuery,
        sink: &mut dyn BackendSink,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError>;
}

// ---------------------------------------------------------------------------
// MockBackend
// ---------------------------------------------------------------------------

/// Deterministic backend that replays a pre-built `Vec<RawHit>`. Used by the
/// Phase 3 / Phase 4 unit tests so we can exercise the post-filter pipeline
/// and IgnoreCache without depending on the Everything SDK or the legacy
/// walker.
///
/// `MockBackend` ignores [`BackendQuery`] entirely: callers stage the exact
/// hits they want to feed downstream, which is the whole point — assertions
/// stay decoupled from query translation.
#[derive(Debug, Clone, Default)]
pub struct MockBackend {
    hits: Vec<RawHit>,
}

impl MockBackend {
    pub fn new(hits: Vec<RawHit>) -> Self {
        Self { hits }
    }
}

impl SearchBackend for MockBackend {
    fn run(
        &self,
        _query: &BackendQuery,
        sink: &mut dyn BackendSink,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        for hit in &self.hits {
            if cancel.is_cancelled() {
                return Err(BackendError::Cancelled);
            }
            sink.send(hit.clone())?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hit(name: &str, root: &Arc<PathBuf>) -> RawHit {
        RawHit {
            path: root.join(name),
            is_dir: false,
            size: 42,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::clone(root),
        }
    }

    /// Collecting sink — just dumps every hit into a Vec for assertion.
    struct VecSink(Vec<RawHit>);
    impl BackendSink for VecSink {
        fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
            self.0.push(hit);
            Ok(())
        }
    }

    fn dummy_query() -> BackendQuery {
        BackendQuery {
            paths: vec![],
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

    /// Encodes the contract that drives Phase 3 TDD: a backend run must emit
    /// every staged hit in order to the sink. If this ever regresses, the
    /// post-filter pipeline loses its only deterministic fixture source.
    #[test]
    fn mock_backend_streams_every_staged_hit_in_order() {
        let root = Arc::new(PathBuf::from(r"C:\repo"));
        let staged = vec![
            sample_hit("a.txt", &root),
            sample_hit("b.txt", &root),
            sample_hit("c.txt", &root),
        ];
        let backend = MockBackend::new(staged.clone());
        let mut sink = VecSink(Vec::new());
        let cancel = CancellationToken::new();

        backend.run(&dummy_query(), &mut sink, &cancel).unwrap();

        assert_eq!(sink.0.len(), staged.len());
        for (got, want) in sink.0.iter().zip(staged.iter()) {
            assert_eq!(got.path, want.path);
        }
    }

    /// Encodes the §Phase 3 max_results_limiter contract: a cancel that
    /// fires mid-stream must surface as `BackendError::Cancelled`, never as a
    /// silent truncation. If this regresses, `--max-results` can race against
    /// large Everything indices and over-deliver.
    #[test]
    fn mock_backend_honors_cancellation_between_hits() {
        let root = Arc::new(PathBuf::from(r"C:\repo"));
        let backend =
            MockBackend::new(vec![sample_hit("a.txt", &root), sample_hit("b.txt", &root)]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut sink = VecSink(Vec::new());

        let err = backend
            .run(&dummy_query(), &mut sink, &cancel)
            .expect_err("cancelled token must surface");
        assert!(matches!(err, BackendError::Cancelled));
        assert!(sink.0.is_empty(), "no hits should leak after cancel");
    }

    /// SearchBackend MUST be object-safe — the Phase 6 backend-selection
    /// layer stores either EverythingBackend or LegacyWalkerBackend behind a
    /// `Box<dyn SearchBackend>`. If a future field breaks dyn-compat, this
    /// test fails at compile time.
    #[test]
    fn search_backend_is_object_safe() {
        let root = Arc::new(PathBuf::from(r"C:\repo"));
        let backend: Box<dyn SearchBackend> =
            Box::new(MockBackend::new(vec![sample_hit("x.txt", &root)]));
        let mut sink = VecSink(Vec::new());
        let cancel = CancellationToken::new();
        backend.run(&dummy_query(), &mut sink, &cancel).unwrap();
        assert_eq!(sink.0.len(), 1);
    }
}
