//! `--max-results N`: emit the first N hits, then ask the backend to
//! cancel. The CancellationToken is shared with the backend (Phase 1
//! contract: backends poll it between hits), so the cancellation
//! arrives on the next iteration even if the post-filter is mid-batch.

use crate::scan::backend::{CancellationToken, RawHit};
use crate::scan::post_filter::{Filter, Verdict};

pub struct MaxResultsFilter {
    limit: usize,
    emitted: usize,
    cancel: CancellationToken,
}

impl MaxResultsFilter {
    pub fn new(limit: usize, cancel: CancellationToken) -> Self {
        Self {
            limit,
            emitted: 0,
            cancel,
        }
    }
}

impl Filter for MaxResultsFilter {
    fn evaluate(&mut self, _hit: &RawHit) -> Verdict {
        if self.limit == 0 {
            return Verdict::Keep;
        }
        self.emitted += 1;
        if self.emitted == self.limit {
            // Fire cancel here so the *backend* sees it on its next
            // poll; the sink will also call `cancel.cancel()` once the
            // ForwardAndCancel outcome arrives — calling twice is
            // harmless (CAS to true).
            self.cancel.cancel();
            return Verdict::Cancel;
        }
        if self.emitted > self.limit {
            return Verdict::Drop;
        }
        Verdict::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit() -> RawHit {
        RawHit {
            path: PathBuf::from("/x"),
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

    /// Encodes `--max-results 3` on a stream of 5: first 2 Keep, 3rd
    /// Cancel (which the sink turns into ForwardAndCancel), the
    /// trailing two never reach the filter because the sink stops
    /// pulling. Here we just verify the verdict sequence.
    #[test]
    fn emits_exactly_n_then_cancels() {
        let cancel = CancellationToken::new();
        let mut f = MaxResultsFilter::new(3, cancel.clone());
        assert_eq!(f.evaluate(&hit()), Verdict::Keep);
        assert_eq!(f.evaluate(&hit()), Verdict::Keep);
        assert_eq!(f.evaluate(&hit()), Verdict::Cancel);
        assert!(
            cancel.is_cancelled(),
            "cancel token must fire on the boundary hit"
        );
    }

    /// limit=0 is the "unset" sentinel; pass-through.
    #[test]
    fn zero_limit_keeps_everything() {
        let cancel = CancellationToken::new();
        let mut f = MaxResultsFilter::new(0, cancel.clone());
        for _ in 0..10 {
            assert_eq!(f.evaluate(&hit()), Verdict::Keep);
        }
        assert!(!cancel.is_cancelled());
    }
}
