//! Post-filter pipeline (PLAN.md §Phase 3).
//!
//! Consumes [`RawHit`]s emitted by a [`SearchBackend`], applies the
//! filter chain in the order specified in PLAN.md §Phase 3, and
//! forwards survivors to a downstream [`BackendSink`]. The legacy
//! `walk.rs` pipeline still runs in parallel until Phase 5 flips the
//! switch; the post-filter is exercised entirely through `MockBackend`
//! at this phase.
//!
//! Threading: each `Pipeline` is single-threaded — Phase 5 fans out
//! parallelism at the `BackendSink` boundary, not inside the filter
//! chain — so stateful filters own their state via `&mut self` rather
//! than `Mutex`.
//!
//! [`SearchBackend`]: crate::scan::backend::SearchBackend
//! [`RawHit`]: crate::scan::backend::RawHit
//! [`BackendSink`]: crate::scan::backend::BackendSink

use crate::scan::backend::RawHit;

pub mod filters;
pub mod ignore_cache;
pub mod pipeline;
pub mod sink;

pub use pipeline::{Pipeline, PipelineOutcome};
#[allow(unused_imports)]
pub use sink::PostFilterSink;

#[cfg(test)]
mod integration_tests {
    //! End-to-end smoke test: `MockBackend` → `PostFilterSink` →
    //! collector. Proves the pieces compose and that PLAN.md §Phase 3
    //! ordering is preserved (specifically: `same_filesystem` running
    //! before `size_filter` matters for hits that would survive size
    //! but cross volumes).

    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use crate::filter::SizeFilter;
    use crate::scan::backend::{
        BackendError, BackendQuery, BackendSink, CancellationToken, CaseModifier, MockBackend,
        PatternScope, RawHit, SearchBackend, TranslatedPattern,
    };
    use crate::scan::post_filter::filters::max_results::MaxResultsFilter;
    use crate::scan::post_filter::filters::same_filesystem::{
        SameFilesystemFilter, VolumeIdProvider,
    };
    use crate::scan::post_filter::filters::size::SizeConstraints;
    use crate::scan::post_filter::{Filter, Pipeline, PostFilterSink};

    struct VecSink(Vec<RawHit>);
    impl BackendSink for VecSink {
        fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
            self.0.push(hit);
            Ok(())
        }
    }

    struct FakeProvider {
        same_volume_prefix: PathBuf,
    }
    impl VolumeIdProvider for FakeProvider {
        fn volume_id(&self, p: &Path) -> Option<u64> {
            if p.starts_with(&self.same_volume_prefix) {
                Some(1)
            } else {
                Some(2)
            }
        }
    }

    fn hit(path: &str, size: u64, root: &Arc<PathBuf>) -> RawHit {
        RawHit {
            path: PathBuf::from(path),
            is_dir: false,
            size,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::clone(root),
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

    /// Encodes the PLAN.md §Phase 3 ordering invariant via a
    /// motivating example: a 2 KB file on a foreign volume must be
    /// dropped even though it would otherwise pass `--size +1k`. If
    /// this regresses, the size filter starts measuring across volumes
    /// — a result `walk.rs` never had to handle because the legacy
    /// walker enforces same-fs at traversal time.
    #[test]
    fn pipeline_runs_filters_in_chain_order_via_mock_backend() {
        let root = Arc::new(PathBuf::from("/C/repo"));
        let staged = vec![
            hit("/C/repo/keep.bin", 2000, &root), // same vol + big → keep
            hit("/D/foreign.bin", 5000, &root),   // foreign vol → drop early
            hit("/C/repo/small.bin", 500, &root), // same vol but too small → drop
        ];

        let filters: Vec<Box<dyn Filter>> = vec![
            Box::new(SameFilesystemFilter::new(Box::new(FakeProvider {
                same_volume_prefix: PathBuf::from("/C"),
            }))),
            Box::new(SizeConstraints::new(vec![
                SizeFilter::from_string("+1k").unwrap(),
            ])),
        ];
        let pipeline = Pipeline::new(filters);

        let mut downstream = VecSink(Vec::new());
        let cancel = CancellationToken::new();
        let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);

        let backend = MockBackend::new(staged);
        backend
            .run(&dummy_query(), &mut sink, &cancel)
            .expect("no cancel expected");

        // Drain (no buffering filters in this minimal chain, so a no-op).
        sink.finalize().unwrap();

        assert_eq!(downstream.0.len(), 1);
        assert_eq!(downstream.0[0].path, PathBuf::from("/C/repo/keep.bin"));
    }

    /// `--max-results 2` mid-stream: MockBackend stages 4 hits; the
    /// sink forwards 2, surfaces `Cancelled`, and trips the token. If
    /// this regresses, large Everything indices keep streaming past
    /// the limit and the user pays index-traversal cost for hits they
    /// will never see.
    #[test]
    fn max_results_cancels_the_backend_mid_stream() {
        let root = Arc::new(PathBuf::from("/C/repo"));
        let staged = vec![
            hit("/C/repo/a", 0, &root),
            hit("/C/repo/b", 0, &root),
            hit("/C/repo/c", 0, &root),
            hit("/C/repo/d", 0, &root),
        ];

        let cancel = CancellationToken::new();
        let filters: Vec<Box<dyn Filter>> =
            vec![Box::new(MaxResultsFilter::new(2, cancel.clone()))];
        let pipeline = Pipeline::new(filters);

        let mut downstream = VecSink(Vec::new());
        let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);

        let backend = MockBackend::new(staged);
        let err = backend
            .run(&dummy_query(), &mut sink, &cancel)
            .expect_err("backend must surface Cancelled");
        assert!(matches!(err, BackendError::Cancelled));
        assert!(cancel.is_cancelled());
        assert_eq!(
            downstream.0.len(),
            2,
            "exactly --max-results hits must be forwarded"
        );
    }
}

/// Per-hit decision returned by each [`Filter`] in the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Forward the hit downstream and keep walking the chain / accepting
    /// more hits from the backend.
    Keep,
    /// Drop the hit; keep accepting more from the backend.
    Drop,
    /// Forward the hit downstream, then ask the pipeline to cancel the
    /// backend. `max_results_limiter` is the only producer.
    Cancel,
}

/// One stage of the post-filter pipeline. Stateful filters store their
/// state on `&mut self` (e.g. `prune_filter`'s `BTreeMap`,
/// `same_filesystem`'s volume-ID cache, `max_results_limiter`'s
/// counter).
pub trait Filter: Send {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict;

    /// Called once after the backend's `run` completes. Default: empty.
    /// `prune_filter` overrides this to dump its buffered hits in path
    /// order (parents before children). Drained hits re-enter the
    /// pipeline at the next downstream filter (see
    /// [`Pipeline::drain_into`]).
    fn drain(&mut self) -> Vec<RawHit> {
        Vec::new()
    }
}
