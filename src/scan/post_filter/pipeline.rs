//! [`Pipeline`] — the ordered filter chain that drives the post-filter.
//!
//! See PLAN.md §Phase 3 for the exact filter ordering and the rationale
//! behind it (notably: `ignore_contain` MUST run before any depth /
//! root check; `same_filesystem` runs second so cross-volume hits are
//! discarded before expensive stat-equivalents; `prune` sits late so
//! its buffering tax is paid by `--prune` users only).

use crate::scan::backend::RawHit;

use super::{Filter, Verdict};

/// Result of running a single hit through the filter chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineOutcome {
    /// All filters returned [`Verdict::Keep`] — forward the hit.
    Forward,
    /// Some filter returned [`Verdict::Drop`] — discard the hit.
    Drop,
    /// A filter returned [`Verdict::Cancel`] (i.e. `max_results`
    /// reached). Forward this hit, then signal the backend to stop.
    ForwardAndCancel,
}

pub struct Pipeline {
    filters: Vec<Box<dyn Filter>>,
}

impl Pipeline {
    /// Construct a pipeline from an already-ordered filter list. Phase 5
    /// will introduce a builder that materializes the list from
    /// [`Config`](crate::config::Config); Phase 3 callers pass the
    /// filter `Vec` directly so unit tests can compose minimal chains.
    pub fn new(filters: Vec<Box<dyn Filter>>) -> Self {
        Self { filters }
    }

    /// Run a single hit through every filter in order. Short-circuits
    /// on the first [`Verdict::Drop`] or [`Verdict::Cancel`] — filters
    /// past that point are skipped, which matters for `prune` (its
    /// `BTreeMap` only buffers hits that actually survive everything
    /// upstream) and for cheap-before-expensive ordering.
    pub fn process(&mut self, hit: &RawHit) -> PipelineOutcome {
        for f in self.filters.iter_mut() {
            match f.evaluate(hit) {
                Verdict::Keep => {}
                Verdict::Drop => return PipelineOutcome::Drop,
                Verdict::Cancel => return PipelineOutcome::ForwardAndCancel,
            }
        }
        PipelineOutcome::Forward
    }

    /// Drain stateful filters' buffered hits. Each producer's drained
    /// hits re-enter the chain at the filter *after* it, so e.g.
    /// `prune`'s buffered descendants still get `symlink_filter` +
    /// `max_results_limiter` applied. Survivors land in `out` in the
    /// order they were emitted (each producer in chain order, each
    /// producer's drain in its own emission order).
    pub fn drain_into(&mut self, out: &mut Vec<RawHit>) {
        for i in 0..self.filters.len() {
            let drained = self.filters[i].drain();
            for hit in drained {
                let mut keep = true;
                let mut cancelled = false;
                for f in self.filters[i + 1..].iter_mut() {
                    match f.evaluate(&hit) {
                        Verdict::Keep => {}
                        Verdict::Drop => {
                            keep = false;
                            break;
                        }
                        Verdict::Cancel => {
                            cancelled = true;
                            break;
                        }
                    }
                }
                if keep {
                    out.push(hit);
                }
                if cancelled {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit(name: &str) -> RawHit {
        RawHit {
            path: PathBuf::from(name),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    struct AlwaysKeep;
    impl Filter for AlwaysKeep {
        fn evaluate(&mut self, _: &RawHit) -> Verdict {
            Verdict::Keep
        }
    }

    struct AlwaysDrop;
    impl Filter for AlwaysDrop {
        fn evaluate(&mut self, _: &RawHit) -> Verdict {
            Verdict::Drop
        }
    }

    struct CancelAfter(usize);
    impl Filter for CancelAfter {
        fn evaluate(&mut self, _: &RawHit) -> Verdict {
            if self.0 == 0 {
                Verdict::Cancel
            } else {
                self.0 -= 1;
                Verdict::Keep
            }
        }
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    struct SharedCounter(Arc<AtomicUsize>);
    impl Filter for SharedCounter {
        fn evaluate(&mut self, _: &RawHit) -> Verdict {
            self.0.fetch_add(1, Ordering::Relaxed);
            Verdict::Keep
        }
    }

    /// Encodes the chain-order invariant: an early `Drop` must
    /// short-circuit later filters. If this regresses, expensive
    /// filters would run on already-dropped hits and §0 hot-path
    /// budgets break.
    #[test]
    fn drop_short_circuits_downstream_filters() {
        let n = Arc::new(AtomicUsize::new(0));
        let mut pipe = Pipeline::new(vec![
            Box::new(AlwaysKeep),
            Box::new(AlwaysDrop),
            Box::new(SharedCounter(Arc::clone(&n))),
        ]);
        assert_eq!(pipe.process(&hit("a")), PipelineOutcome::Drop);
        assert_eq!(pipe.process(&hit("b")), PipelineOutcome::Drop);
        assert_eq!(
            n.load(Ordering::Relaxed),
            0,
            "downstream of Drop must not run"
        );
    }

    /// Encodes the `max_results` contract: a `Cancel` still forwards
    /// the hit. If this regresses, `--max-results N` returns `N-1`.
    #[test]
    fn cancel_returns_forward_and_cancel() {
        let mut pipe = Pipeline::new(vec![Box::new(AlwaysKeep), Box::new(CancelAfter(0))]);
        assert_eq!(pipe.process(&hit("a")), PipelineOutcome::ForwardAndCancel);
    }

    /// Drained hits must traverse the *downstream* filters (PLAN §3.1
    /// — prune's BTreeMap dump still respects symlink_filter and
    /// max_results). Producer drains 3 hits; the downstream filter
    /// drops every odd-indexed one.
    #[test]
    fn drain_runs_downstream_filters_on_buffered_hits() {
        struct ProducerDrain(Vec<RawHit>);
        impl Filter for ProducerDrain {
            fn evaluate(&mut self, _: &RawHit) -> Verdict {
                Verdict::Keep
            }
            fn drain(&mut self) -> Vec<RawHit> {
                std::mem::take(&mut self.0)
            }
        }
        struct DropOdd(usize);
        impl Filter for DropOdd {
            fn evaluate(&mut self, _: &RawHit) -> Verdict {
                let n = self.0;
                self.0 += 1;
                if n % 2 == 1 {
                    Verdict::Drop
                } else {
                    Verdict::Keep
                }
            }
        }

        let mut pipe = Pipeline::new(vec![
            Box::new(ProducerDrain(vec![hit("a"), hit("b"), hit("c")])),
            Box::new(DropOdd(0)),
        ]);
        let mut out = Vec::new();
        pipe.drain_into(&mut out);
        assert_eq!(out.len(), 2, "two of three drained hits survive");
        assert_eq!(out[0].path, PathBuf::from("a"));
        assert_eq!(out[1].path, PathBuf::from("c"));
    }
}
