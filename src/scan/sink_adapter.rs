//! PLAN.md §Phase 8.5-B: adapter that lets a [`SearchBackend`] feed the
//! same [`BatchSender`] that the legacy `ignore::WalkBuilder` writes to.
//!
//! A backend (`EverythingBackend` or `MockBackend`) emits [`RawHit`]s; the
//! `ReceiverBuffer` in `walk.rs` consumes [`WorkerResult::Entry(DirEntry)`]
//! batches off a channel. This adapter wraps a [`BatchSender`] in a
//! [`BackendSink`], converting each `RawHit` to a `DirEntry` (cheap — the
//! `from_raw_hit` constructor doesn't stat) and pushing it as a
//! `WorkerResult::Entry`.
//!
//! Cancellation: when the downstream receiver disconnects (SIGPIPE-equivalent
//! or `--max-results` short-circuit), `BatchSender::send` returns an
//! `Err(SendError(()))`. We map that to [`BackendError::Cancelled`] so the
//! backend can call `Everything_Reset` at the next hit boundary and the
//! orchestrator stops scheduling more roots — matching PLAN §Phase 3 §3.X
//! "下游错误必须 surface" contract.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dir_entry::DirEntry;
use crate::filesystem::strip_path_prefix;
use crate::scan::backend::{BackendError, BackendSink, RawHit};
use crate::scan::sink::{BatchSender, WorkerResult};

/// Rebase backend hits from a resolved symlink target onto the logical root
/// supplied by the user. This adapter must sit before `PostFilterSink`: ignore
/// matching, metadata fallbacks, `--exec`, and output projection all need to
/// observe the link spelling rather than the physical target spelling.
pub(crate) struct RootRebaseSink<'a, S: BackendSink + ?Sized> {
    downstream: &'a mut S,
    query_root: &'a Path,
    logical_root: Arc<PathBuf>,
}

impl<'a, S: BackendSink + ?Sized> RootRebaseSink<'a, S> {
    pub(crate) fn new(
        downstream: &'a mut S,
        query_root: &'a Path,
        logical_root: Arc<PathBuf>,
    ) -> Self {
        Self {
            downstream,
            query_root,
            logical_root,
        }
    }
}

impl<S: BackendSink + ?Sized> BackendSink for RootRebaseSink<'_, S> {
    fn send(&mut self, mut hit: RawHit) -> Result<(), BackendError> {
        let relative = strip_path_prefix(&hit.path, self.query_root).ok_or_else(|| {
            BackendError::Other(anyhow::anyhow!(
                "backend returned {} outside resolved search root {}",
                hit.path.display(),
                self.query_root.display()
            ))
        })?;
        hit.path = self.logical_root.join(relative);
        hit.search_root = Arc::clone(&self.logical_root);
        self.downstream.send(hit)
    }
}

/// `BackendSink` implementation that bridges a Phase 5 backend to the
/// Phase 2 `BatchSender`. Owns the sender by move because each backend
/// run flushes its own batch; sharing one across multiple backend runs
/// is the orchestrator's job (it constructs a fresh adapter per root).
pub(crate) struct RawHitBatchSink {
    tx: BatchSender,
}

impl RawHitBatchSink {
    pub(crate) fn new(tx: BatchSender) -> Self {
        Self { tx }
    }
}

impl BackendSink for RawHitBatchSink {
    fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
        let entry = DirEntry::from_raw_hit(hit);
        // `BatchSender::send` only fails when the bounded `crossbeam` channel
        // is disconnected, which means `ReceiverBuffer::process` has already
        // returned (typically `--max-results` reached, or SIGPIPE on
        // downstream). Surfacing this as `Cancelled` lets `EverythingBackend`
        // bail at the next hit boundary instead of finishing the SDK
        // result-list walk we are about to throw away.
        self.tx
            .send(WorkerResult::Entry(entry))
            .map_err(|_| BackendError::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use crossbeam_channel::bounded;

    use super::*;
    use crate::scan::sink::Batch;

    fn hit(name: &str, root: &Arc<PathBuf>) -> RawHit {
        RawHit {
            path: root.join(name),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::clone(root),
        }
    }

    /// Encodes the §Phase 8.5-B contract: every staged RawHit must arrive
    /// downstream as a `WorkerResult::Entry`. If the adapter ever silently
    /// drops a hit (e.g. by short-circuiting on metadata), `--exec` argv
    /// would lose entries the user is watching for, with no error.
    #[test]
    fn every_staged_hit_arrives_as_worker_result_entry() {
        let (tx, rx) = bounded::<Batch>(4);
        let mut sink = RawHitBatchSink::new(BatchSender::new(tx, 1));
        let root = Arc::new(PathBuf::from(r"C:\repo"));

        sink.send(hit("a.txt", &root)).unwrap();
        sink.send(hit("b.txt", &root)).unwrap();
        sink.send(hit("c.txt", &root)).unwrap();

        // Drop the sender side of the BatchSender by dropping `sink` so the
        // receiver loop terminates. We don't expose BatchSender's tx, so the
        // adapter going out of scope is the only way to close the channel.
        drop(sink);
        let collected: Vec<_> = rx.into_iter().flatten().collect();
        assert_eq!(collected.len(), 3, "all three hits must arrive");
        for (got, want) in collected.iter().zip(["a.txt", "b.txt", "c.txt"]) {
            match got {
                WorkerResult::Entry(entry) => {
                    assert_eq!(entry.path(), root.join(want));
                }
                WorkerResult::Error(_) => panic!("unexpected error in stream"),
            }
        }
    }

    /// Encodes the §Phase 8.5-B cancellation contract: when the receiver
    /// side of the channel disconnects (the production analogue of
    /// `--max-results` tripping mid-stream or SIGPIPE on stdout), the next
    /// `sink.send` must surface `BackendError::Cancelled` so the backend
    /// can `Everything_Reset` and return early. Silent ok-on-disconnect
    /// would mean the backend keeps walking the SDK result-list for hits
    /// the user already stopped reading — wasted Everything CPU and
    /// delayed exit.
    #[test]
    fn receiver_disconnect_surfaces_as_cancelled() {
        let (tx, rx) = bounded::<Batch>(2);
        let mut sink = RawHitBatchSink::new(BatchSender::new(tx, 1));
        drop(rx);

        let root = Arc::new(PathBuf::from(r"C:\repo"));
        let err = sink
            .send(hit("a.txt", &root))
            .expect_err("disconnect must surface as Cancelled");
        assert!(matches!(err, BackendError::Cancelled));
    }

    #[test]
    fn root_rebase_sink_rewrites_path_and_search_root() {
        struct VecSink(Vec<RawHit>);
        impl BackendSink for VecSink {
            fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
                self.0.push(hit);
                Ok(())
            }
        }

        let query_root = PathBuf::from(r"D:\real\repo");
        let logical_root = Arc::new(PathBuf::from(r"D:\links\repo"));
        let physical_root = Arc::new(query_root.clone());
        let mut downstream = VecSink(Vec::new());
        let mut sink = RootRebaseSink::new(&mut downstream, &query_root, Arc::clone(&logical_root));

        sink.send(hit(r"sub\a.py", &physical_root)).unwrap();
        drop(sink);

        assert_eq!(downstream.0.len(), 1);
        assert_eq!(
            downstream.0[0].path,
            PathBuf::from(r"D:\links\repo\sub\a.py")
        );
        assert_eq!(downstream.0[0].search_root, logical_root);
    }

    #[test]
    fn root_rebase_sink_rejects_hits_outside_query_root() {
        struct VecSink(Vec<RawHit>);
        impl BackendSink for VecSink {
            fn send(&mut self, hit: RawHit) -> Result<(), BackendError> {
                self.0.push(hit);
                Ok(())
            }
        }

        let query_root = PathBuf::from(r"D:\real\repo");
        let logical_root = Arc::new(PathBuf::from(r"D:\links\repo"));
        let outside_root = Arc::new(PathBuf::from(r"D:\other"));
        let mut downstream = VecSink(Vec::new());
        let mut sink = RootRebaseSink::new(&mut downstream, &query_root, logical_root);

        let err = sink
            .send(hit("a.py", &outside_root))
            .expect_err("out-of-root backend hit must fail");
        assert!(matches!(err, BackendError::Other(_)));
        drop(sink);
        assert!(downstream.0.is_empty());
    }
}
