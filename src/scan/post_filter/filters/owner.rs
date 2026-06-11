//! `--owner`: Unix-only on the legacy walker (see `walk.rs:484-495`,
//! `filter::OwnerFilter`). Phase 3 keeps this as a pass-through stub on
//! both Windows and Unix because:
//!
//! * Windows has no parity owner concept in fd's CLI today;
//! * Unix continues to run through `walk.rs` until Phase 5, so the
//!   legacy stat-based check still applies there.
//!
//! Phase 5 will revisit and either push down to the backend or pay one
//! syscall here.

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct OwnerFilter;

impl OwnerFilter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OwnerFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter for OwnerFilter {
    fn evaluate(&mut self, _hit: &RawHit) -> Verdict {
        // TODO(PLAN.md Phase 5): push down to backend or stat once.
        Verdict::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn stub_keeps_all_hits() {
        let mut f = OwnerFilter::new();
        let hit = RawHit {
            path: PathBuf::from("/x"),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        };
        assert_eq!(f.evaluate(&hit), Verdict::Keep);
    }
}
