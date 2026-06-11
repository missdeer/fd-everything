//! [`PostFilterSink`] — adapter that plugs a [`Pipeline`] between a
//! [`SearchBackend`](crate::scan::backend::SearchBackend) and a
//! downstream [`BackendSink`].
//!
//! Production wiring (Phase 5) puts a `RawHit -> DirEntry -> BatchSender`
//! adapter as the downstream sink so `walk.rs` can keep consuming
//! [`Batch`](crate::scan::sink::Batch)es unchanged. Phase 3 tests wire
//! a `VecSink` downstream and assert on the collected hits directly.

use crate::scan::backend::{BackendError, BackendSink, CancellationToken, RawHit};

use super::{Pipeline, PipelineOutcome};

/// Generic over the downstream sink so unit tests can collect into a
/// `Vec<RawHit>` while production wires the Phase 5 `BatchSender`
/// adapter.
pub struct PostFilterSink<'a, S: BackendSink + ?Sized> {
    pipeline: Pipeline,
    downstream: &'a mut S,
    cancel: &'a CancellationToken,
}

impl<'a, S: BackendSink + ?Sized> PostFilterSink<'a, S> {
    pub fn new(pipeline: Pipeline, downstream: &'a mut S, cancel: &'a CancellationToken) -> Self {
        Self {
            pipeline,
            downstream,
            cancel,
        }
    }

    /// Drain the pipeline's buffered hits (currently only `prune`) into
    /// the downstream sink. Called by the orchestrator after the
    /// backend's `run` returns `Ok(())`.
    pub fn finalize(mut self) -> Result<(), BackendError> {
        let mut buffered = Vec::new();
        self.pipeline.drain_into(&mut buffered);
        for hit in buffered {
            if self.cancel.is_cancelled() {
                return Err(BackendError::Cancelled);
            }
            self.downstream.send(hit)?;
        }
        Ok(())
    }
}

impl<S: BackendSink + ?Sized> BackendSink for PostFilterSink<'_, S> {
    fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
        // PLAN §Phase 8.7.1 MUST 4 defence-in-depth: if the shared
        // `CancellationToken` fired between hits (typically from the
        // Ctrl-C handler in `walk::scan`), surface `Cancelled` here
        // even before consulting the pipeline. The backend's own
        // `is_cancelled()` poll catches this too, but adding the check
        // at the sink ensures a backend that forgets to poll still
        // unwinds promptly.
        if self.cancel.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        match self.pipeline.process(&hit) {
            PipelineOutcome::Drop => Ok(()),
            PipelineOutcome::Forward => self.downstream.send(hit),
            PipelineOutcome::ForwardAndCancel => {
                self.downstream.send(hit)?;
                self.cancel.cancel();
                Err(BackendError::Cancelled)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::post_filter::{Filter, Verdict};
    use std::path::PathBuf;
    use std::sync::Arc;

    struct VecSink(Vec<RawHit>);
    impl BackendSink for VecSink {
        fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
            self.0.push(hit);
            Ok(())
        }
    }

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

    /// Encodes the cancel contract: when the pipeline returns
    /// `ForwardAndCancel`, the sink forwards the hit AND fires the
    /// shared CancellationToken AND surfaces `BackendError::Cancelled`.
    /// If this regresses, `--max-results` either over-delivers (no
    /// surface) or under-delivers (cancels before forward).
    #[test]
    fn forward_and_cancel_emits_hit_and_fires_token() {
        struct CancelEverything;
        impl Filter for CancelEverything {
            fn evaluate(&mut self, _: &RawHit) -> Verdict {
                Verdict::Cancel
            }
        }
        let pipeline = Pipeline::new(vec![Box::new(CancelEverything)]);
        let mut downstream = VecSink(Vec::new());
        let cancel = CancellationToken::new();
        let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);

        let err = sink.send(hit("a")).expect_err("Cancel must surface as Err");
        assert!(matches!(err, BackendError::Cancelled));
        assert!(cancel.is_cancelled(), "cancel token must fire");
        drop(sink);
        assert_eq!(
            downstream.0.len(),
            1,
            "the cancelling hit must be forwarded"
        );
    }

    /// PLAN §Phase 8.7.1 MUST 4 contract pin: when the shared
    /// `CancellationToken` fires externally (i.e. from the Ctrl-C
    /// handler in `walk::scan`, not from a pipeline filter returning
    /// `Verdict::Cancel`), the very next `BackendSink::send` MUST
    /// return `BackendError::Cancelled` and MUST NOT forward the hit
    /// downstream. The existing `forward_and_cancel_emits_hit_and_fires_token`
    /// case covers the *pipeline-internal* cancel path (`--max-results`
    /// hits its cap); this new case covers the *external* cancel-via-
    /// flag path. If this regresses, Ctrl-C against EverythingBackend
    /// would keep streaming hits until the SDK returns naturally,
    /// because the backend's between-hit poll is the only other
    /// place the signal is checked.
    #[test]
    fn external_cancel_short_circuits_next_send_without_forwarding() {
        // Pipeline with no filters — every hit would normally Forward.
        let pipeline = Pipeline::new(vec![]);
        let mut downstream = VecSink(Vec::new());
        let cancel = CancellationToken::new();
        let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);

        // Sanity: with cancel un-fired, hits flow.
        sink.send(hit("warm"))
            .expect("pre-cancel send must succeed");

        // External cancel (e.g. Ctrl-C handler in walk::scan).
        cancel.cancel();

        let err = sink
            .send(hit("after-cancel"))
            .expect_err("post-cancel send must surface Cancelled");
        assert!(matches!(err, BackendError::Cancelled));

        drop(sink);
        assert_eq!(
            downstream.0.len(),
            1,
            "exactly the pre-cancel hit must reach downstream; got {:?}",
            downstream.0.iter().map(|h| &h.path).collect::<Vec<_>>()
        );
    }
}
