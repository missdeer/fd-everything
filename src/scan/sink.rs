//! Shared sender/receiver types between the legacy `ignore::WalkBuilder`
//! pipeline (`walk.rs`) and the upcoming EverythingBackend post-filter
//! pipeline (Phase 3+).
//!
//! These were `walk.rs`-private until PLAN.md §Phase 2 — promoted to
//! `pub(crate)` so the post-filter pipeline can produce `Batch`es of the
//! same shape and feed `walk::WorkerState::receive`.

use std::sync::{Arc, Mutex, MutexGuard};

use crossbeam_channel::{SendError, Sender};

use crate::dir_entry::DirEntry;

/// The Worker threads can result in a valid entry having PathBuf or an error.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum WorkerResult {
    // Errors should be rare, so it's probably better to allow large_enum_variant than
    // to box the Entry variant
    Entry(DirEntry),
    Error(ignore::Error),
}

/// A batch of WorkerResults to send over a channel.
#[derive(Clone)]
pub(crate) struct Batch {
    items: Arc<Mutex<Option<Vec<WorkerResult>>>>,
}

impl Batch {
    fn new() -> Self {
        Self {
            items: Arc::new(Mutex::new(Some(vec![]))),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Vec<WorkerResult>>> {
        self.items.lock().unwrap()
    }
}

impl IntoIterator for Batch {
    type Item = WorkerResult;
    type IntoIter = std::vec::IntoIter<WorkerResult>;

    fn into_iter(self) -> Self::IntoIter {
        self.lock().take().unwrap().into_iter()
    }
}

/// Wrapper that sends batches of items at once over a channel.
pub(crate) struct BatchSender {
    batch: Batch,
    tx: Sender<Batch>,
    limit: usize,
}

impl BatchSender {
    pub(crate) fn new(tx: Sender<Batch>, limit: usize) -> Self {
        Self {
            batch: Batch::new(),
            tx,
            limit,
        }
    }

    fn needs_flush(&self, batch: Option<&Vec<WorkerResult>>) -> bool {
        match batch {
            Some(vec) => vec.len() >= self.limit,
            None => true,
        }
    }

    pub(crate) fn send(&mut self, item: WorkerResult) -> Result<(), SendError<()>> {
        let mut batch = self.batch.lock();

        if self.needs_flush(batch.as_ref()) {
            drop(batch);
            self.batch = Batch::new();
            batch = self.batch.lock();
        }

        let items = batch.as_mut().unwrap();
        items.push(item);

        if items.len() == 1 {
            self.tx
                .send(self.batch.clone())
                .map_err(|_| SendError(()))?;
        }

        Ok(())
    }
}
