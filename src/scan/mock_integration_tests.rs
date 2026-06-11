//! PLAN.md §Phase 8 §1 — MockBackend-driven integration tests.
//!
//! These are not bin-level integration tests (the crate has no `lib.rs`,
//! deliberately); they live as a `#[cfg(test)]` module under `src/scan/`
//! so they can compose private filters with the public `MockBackend` +
//! `Pipeline` + `PostFilterSink` types. Each test is justified by what
//! it would catch if it regressed — Rule 9 from CLAUDE.md ("tests verify
//! intent, not just behavior").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use regex::bytes::RegexSetBuilder;

use crate::filetypes::FileTypes;
use crate::scan::backend::{
    BackendError, BackendQuery, BackendSink, CancellationToken, CaseModifier, MockBackend,
    PatternScope, RawHit, SearchBackend, TranslatedPattern,
};
use crate::scan::post_filter::filters::depth::DepthFilter;
use crate::scan::post_filter::filters::extension::ExtensionFilter;
use crate::scan::post_filter::filters::hidden_by_name::HiddenByNameFilter;
use crate::scan::post_filter::filters::max_results::MaxResultsFilter;
use crate::scan::post_filter::filters::prune::PruneFilter;
use crate::scan::post_filter::filters::type_filter::TypeFilter;
use crate::scan::post_filter::{Filter, Pipeline, PostFilterSink};

/// Collects every forwarded `RawHit` into a `Vec` so each test can
/// assert paths in order. Used by every test in this module instead of
/// the per-test `VecSink` duplicated across `src/scan/post_filter/`.
struct CollectSink(Vec<RawHit>);
impl BackendSink for CollectSink {
    fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
        self.0.push(hit);
        Ok(())
    }
}

/// A downstream sink that refuses the second hit it sees, surfacing a
/// `BackendError::Other`. Mirrors the SIGPIPE case where stdout closes
/// mid-stream — PLAN.md §Phase 1 specifies that the post-filter must
/// forward the error rather than silently swallow it.
struct BrokenPipeSink {
    delivered: usize,
    break_after: usize,
    received: Vec<PathBuf>,
}
impl BackendSink for BrokenPipeSink {
    fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
        if self.delivered >= self.break_after {
            return Err(BackendError::Other(anyhow::anyhow!("broken pipe")));
        }
        self.delivered += 1;
        self.received.push(hit.path);
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

fn raw_hit(path: &str, depth: usize, is_dir: bool, root: &Arc<PathBuf>) -> RawHit {
    let p = PathBuf::from(path);
    let extension = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    RawHit {
        path: p,
        is_dir,
        size: 0,
        mtime: 0,
        ctime: 0,
        attributes: 0,
        extension: extension.into(),
        depth,
        search_root: Arc::clone(root),
    }
}

/// Run a backend through a pipeline + collect-sink, returning the
/// downstream survivors plus any error the orchestrator surfaced.
fn drive(
    staged: Vec<RawHit>,
    filters: Vec<Box<dyn Filter>>,
    cancel: &CancellationToken,
) -> (Vec<RawHit>, Option<BackendError>) {
    let pipeline = Pipeline::new(filters);
    let mut downstream = CollectSink(Vec::new());
    let mut sink = PostFilterSink::new(pipeline, &mut downstream, cancel);
    let backend = MockBackend::new(staged);
    let err = backend.run(&dummy_query(), &mut sink, cancel).err();
    if err.is_none() {
        sink.finalize().expect("finalize on success path");
    }
    (downstream.0, err)
}

/// PLAN §Phase 3 depth + extension + type composition. Stages a mix of
/// `.rs` and `.txt`, file and directory hits at depths 0/1/2/3, then
/// asks for `--type f --extension rs --min-depth 2 --max-depth 2`.
///
/// What this would catch on regression: a filter-order swap (e.g. type
/// running after extension) would still produce the right set here, but
/// a depth window off-by-one — which the per-filter unit test for
/// `DepthFilter` covers in isolation — would NOT be caught if a future
/// `Pipeline::new` reorder ever broke the contract that `DepthFilter`
/// runs at all. This test re-asserts the *combination* survives.
#[test]
fn depth_extension_type_combo_keeps_only_intersection() {
    let root = Arc::new(PathBuf::from(r"C:\repo"));
    let staged = vec![
        raw_hit(r"C:\repo\top.rs", 1, false, &root), // wrong depth
        raw_hit(r"C:\repo\src\lib.rs", 2, false, &root), // KEEP
        raw_hit(r"C:\repo\src\main.rs", 2, false, &root), // KEEP
        raw_hit(r"C:\repo\src\notes.txt", 2, false, &root), // wrong ext
        raw_hit(r"C:\repo\src", 1, true, &root),     // is_dir
        raw_hit(r"C:\repo\src\sub", 2, true, &root), // is_dir + correct depth
        raw_hit(r"C:\repo\src\sub\deep.rs", 3, false, &root), // too deep
    ];

    let extension_set = RegexSetBuilder::new([r"\.rs$"])
        .case_insensitive(true)
        .build()
        .unwrap();
    let filters: Vec<Box<dyn Filter>> = vec![
        Box::new(DepthFilter::new(Some(2), Some(2))),
        Box::new(ExtensionFilter::new(extension_set)),
        Box::new(TypeFilter::new(FileTypes {
            files: true,
            ..Default::default()
        })),
    ];

    let cancel = CancellationToken::new();
    let (kept, err) = drive(staged, filters, &cancel);
    assert!(err.is_none(), "expected no backend error, got {err:?}");
    let names: Vec<&Path> = kept.iter().map(|h| h.path.as_path()).collect();
    assert_eq!(
        names,
        vec![
            Path::new(r"C:\repo\src\lib.rs"),
            Path::new(r"C:\repo\src\main.rs"),
        ]
    );
}

/// PLAN §Phase 3: `prune` buffers every survivor to drain, and the
/// re-emitted hits must traverse downstream filters that sit *after*
/// `prune` in the chain. This stages dir + descendant file pairs and
/// pins:
///  - the parent dir is kept;
///  - every descendant of an emitted dir is suppressed even when its
///    own path also showed up as a staged hit (the prune semantics);
///  - drain order is BTreeMap-ascending (parents before unrelated
///    siblings) so a downstream stateless filter sees a deterministic
///    sequence;
///  - the downstream `ExtensionFilter` re-runs against every drained
///    survivor (i.e. the chain still applies post-drain).
///
/// What this would catch on regression: if a future change makes
/// `Pipeline::drain_into` re-enter the chain at `filters[0]` instead of
/// `filters[i + 1]`, the drained hits would loop back through
/// `PruneFilter` and never reach the extension stage. The directory
/// hits — whose file_name has no `.rs` — would still get filtered out,
/// but a `prune` invariant change would be invisible until then.
#[test]
fn prune_buffers_then_drains_through_downstream_extension_filter() {
    let root = Arc::new(PathBuf::from(r"C:\src"));
    let staged = vec![
        raw_hit(r"C:\src\a\file1.txt", 2, false, &root),
        raw_hit(r"C:\src\a", 1, true, &root),
        raw_hit(r"C:\src\a\sub\file2.txt", 3, false, &root),
        raw_hit(r"C:\src\b", 1, true, &root),
        raw_hit(r"C:\src\b\file3.txt", 2, false, &root),
        raw_hit(r"C:\src\c", 1, true, &root),
        raw_hit(r"C:\src\standalone.txt", 1, false, &root),
    ];

    let extension_set = RegexSetBuilder::new([r"\.txt$"])
        .case_insensitive(true)
        .build()
        .unwrap();
    let filters: Vec<Box<dyn Filter>> = vec![
        Box::new(PruneFilter::new()),
        Box::new(ExtensionFilter::new(extension_set)),
    ];

    let cancel = CancellationToken::new();
    let (kept, err) = drive(staged, filters, &cancel);
    assert!(err.is_none(), "expected no backend error, got {err:?}");
    let paths: Vec<&Path> = kept.iter().map(|h| h.path.as_path()).collect();
    // BTreeMap drain ascending: `\src\a`, `\src\b`, `\src\c`, then the
    // standalone file. The dirs fall out at ExtensionFilter (no `.txt`);
    // only the standalone `.txt` survives because every other file hit
    // sits under an already-emitted prune dir.
    assert_eq!(paths, vec![Path::new(r"C:\src\standalone.txt")]);
}

/// PLAN §Phase 3 §`--prune` ordering: when `--max-results` is paired
/// with `--prune`, the cap fires during drain — and `finalize()`
/// short-circuits BEFORE flushing buffered survivors to the downstream
/// sink. This is the documented trade-off (prune buys topology
/// correctness at the cost of latency, and max-results during drain
/// surfaces as `Cancelled`). This test pins both halves of the contract:
///  - `MockBackend.run` returns `Ok(())` (no cancel mid-stream because
///    `PruneFilter` returns `Drop` for everything);
///  - `finalize()` returns `BackendError::Cancelled` once drain trips
///    the `MaxResultsFilter`.
///
/// What this would catch on regression: a future "be helpful" change
/// that makes `finalize` deliver buffered hits even when cancelled would
/// over-deliver to `fde --prune --max-results 2`. If the cancel
/// short-circuit gets removed, the user would silently see more results
/// than they asked for, defeating the cap on big indices.
#[test]
fn prune_plus_max_results_surfaces_cancelled_from_finalize() {
    let root = Arc::new(PathBuf::from(r"C:\src"));
    let staged = vec![
        raw_hit(r"C:\src\a", 1, true, &root),
        raw_hit(r"C:\src\b", 1, true, &root),
        raw_hit(r"C:\src\c", 1, true, &root),
    ];

    let cancel = CancellationToken::new();
    let pipeline = Pipeline::new(vec![
        Box::new(PruneFilter::new()),
        Box::new(MaxResultsFilter::new(2, cancel.clone())),
    ]);
    let mut downstream = CollectSink(Vec::new());
    let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);
    let backend = MockBackend::new(staged);

    backend
        .run(&dummy_query(), &mut sink, &cancel)
        .expect("backend returns Ok; prune Drops everything mid-stream");

    let finalize_err = sink
        .finalize()
        .expect_err("finalize must surface Cancelled once max_results fires during drain");
    assert!(matches!(finalize_err, BackendError::Cancelled));
    assert!(cancel.is_cancelled());
}

/// PLAN §3.2 dotfile / hidden-by-name short-circuit. Filter chain:
/// `HiddenByName(true)` then `Extension(.rs)`. Hits under `.git/` get
/// dropped at the first stage; the extension filter is never reached.
///
/// What this would catch on regression: if dotfile detection ever
/// becomes "starts-with-dot in absolute path" rather than "starts-with-
/// dot in file name", repository files like `C:\.well-known\foo.rs`
/// would suddenly drop. This test pins file-name semantics by including
/// a hit whose path component contains `.` but whose name does not.
#[test]
fn hidden_by_name_drops_before_extension_filter_sees_hit() {
    let root = Arc::new(PathBuf::from(r"C:\repo"));
    let staged = vec![
        raw_hit(r"C:\repo\src\lib.rs", 2, false, &root), // KEEP
        raw_hit(r"C:\repo\.git\config", 2, false, &root), // hidden parent ok
        raw_hit(r"C:\repo\.gitignore", 1, false, &root), // dotfile -> drop
        raw_hit(r"C:\repo\src\.hidden.rs", 2, false, &root), // dotfile -> drop
        raw_hit(r"C:\repo\some.dot.dir\plain.rs", 2, false, &root), // not dotfile
    ];

    let extension_set = RegexSetBuilder::new([r"\.rs$"])
        .case_insensitive(true)
        .build()
        .unwrap();
    let filters: Vec<Box<dyn Filter>> = vec![
        Box::new(HiddenByNameFilter::new(true)),
        Box::new(ExtensionFilter::new(extension_set)),
    ];

    let cancel = CancellationToken::new();
    let (kept, _err) = drive(staged, filters, &cancel);
    let names: Vec<&Path> = kept.iter().map(|h| h.path.as_path()).collect();
    assert_eq!(
        names,
        vec![
            Path::new(r"C:\repo\src\lib.rs"),
            Path::new(r"C:\repo\some.dot.dir\plain.rs"),
        ]
    );
}

/// PLAN §Phase 1 multi-root depth attribution. Two search roots stage
/// hits at depth 1 each; the depth filter (`--min-depth 1 --max-depth 1`)
/// must treat them per-root rather than collapsing to global depth.
///
/// What this would catch on regression: if a future `RawHit::depth`
/// computation switches from "depth relative to search_root" to
/// "depth from path component count", hits from a deep root like
/// `C:\repo\src\sub` would suddenly have inflated depth and fall out of
/// the window, even though the user staged them at depth 1 of that root.
#[test]
fn multi_root_depth_is_per_root_not_global() {
    let root_a = Arc::new(PathBuf::from(r"C:\repo"));
    let root_b = Arc::new(PathBuf::from(r"D:\projects\deep\tree"));
    let staged = vec![
        raw_hit(r"C:\repo\file1.txt", 1, false, &root_a),
        raw_hit(r"D:\projects\deep\tree\file2.txt", 1, false, &root_b),
        raw_hit(r"C:\repo\a\file3.txt", 2, false, &root_a),
    ];

    let filters: Vec<Box<dyn Filter>> = vec![Box::new(DepthFilter::new(Some(1), Some(1)))];

    let cancel = CancellationToken::new();
    let (kept, _err) = drive(staged, filters, &cancel);
    let paths: Vec<&Path> = kept.iter().map(|h| h.path.as_path()).collect();
    assert_eq!(
        paths,
        vec![
            Path::new(r"C:\repo\file1.txt"),
            Path::new(r"D:\projects\deep\tree\file2.txt"),
        ],
        "both roots' depth-1 hits survive; the depth-2 hit drops"
    );
}

/// PLAN §Phase 1 contract: downstream sink errors (e.g. SIGPIPE on
/// stdout) must surface to the orchestrator rather than being swallowed
/// by `PostFilterSink::send`. Staged with 5 hits, downstream breaks
/// after 2 — we assert exactly 2 reach the wire and the third hit
/// surfaces `BackendError::Other`.
///
/// What this would catch on regression: if a future change converts
/// downstream errors into a `Verdict::Drop` (e.g. to be "robust"), the
/// `fde | head -n 5` user would silently lose visibility into broken
/// pipes and the backend would keep walking the index forever.
#[test]
fn downstream_error_propagates_through_post_filter() {
    let root = Arc::new(PathBuf::from(r"C:\repo"));
    let staged = vec![
        raw_hit(r"C:\repo\a.txt", 1, false, &root),
        raw_hit(r"C:\repo\b.txt", 1, false, &root),
        raw_hit(r"C:\repo\c.txt", 1, false, &root),
        raw_hit(r"C:\repo\d.txt", 1, false, &root),
        raw_hit(r"C:\repo\e.txt", 1, false, &root),
    ];

    let pipeline = Pipeline::new(vec![]);
    let mut downstream = BrokenPipeSink {
        delivered: 0,
        break_after: 2,
        received: Vec::new(),
    };
    let cancel = CancellationToken::new();
    let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);
    let backend = MockBackend::new(staged);

    let err = backend
        .run(&dummy_query(), &mut sink, &cancel)
        .expect_err("downstream broken pipe must surface");
    assert!(matches!(err, BackendError::Other(_)));
    drop(sink);
    assert_eq!(
        downstream.delivered, 2,
        "exactly two hits delivered before break"
    );
    assert_eq!(
        downstream.received,
        vec![
            PathBuf::from(r"C:\repo\a.txt"),
            PathBuf::from(r"C:\repo\b.txt")
        ]
    );
}

/// PLAN §Phase 3 ordering invariant: empty filter chain forwards every
/// hit verbatim. Acts as the lower bound for the filter chain — any
/// future "default-deny" mistake (introducing an implicit filter at
/// `Pipeline::new`) would flip this test from pass to fail.
#[test]
fn empty_pipeline_forwards_every_hit_unchanged() {
    let root = Arc::new(PathBuf::from(r"C:\repo"));
    let staged: Vec<RawHit> = (0..16)
        .map(|i| raw_hit(&format!(r"C:\repo\f{i}.txt"), 1, false, &root))
        .collect();
    let expected_paths: Vec<PathBuf> = staged.iter().map(|h| h.path.clone()).collect();

    let cancel = CancellationToken::new();
    let (kept, err) = drive(staged, vec![], &cancel);
    assert!(err.is_none());
    let got: Vec<PathBuf> = kept.into_iter().map(|h| h.path).collect();
    assert_eq!(got, expected_paths);
}
