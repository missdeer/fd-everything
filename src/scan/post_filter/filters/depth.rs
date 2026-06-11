//! `--max-depth` / `--min-depth` / `--exact-depth`. `RawHit::depth` is
//! always populated (Phase 1 contract), so this is a pure comparison.

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct DepthFilter {
    min_depth: Option<usize>,
    max_depth: Option<usize>,
}

impl DepthFilter {
    pub fn new(min_depth: Option<usize>, max_depth: Option<usize>) -> Self {
        Self {
            min_depth,
            max_depth,
        }
    }
}

impl Filter for DepthFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if let Some(min) = self.min_depth
            && hit.depth < min
        {
            return Verdict::Drop;
        }
        if let Some(max) = self.max_depth
            && hit.depth > max
        {
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

    fn hit(depth: usize) -> RawHit {
        RawHit {
            path: PathBuf::from("/x"),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// Encodes the `--min-depth N --max-depth M` window. If this
    /// regresses, `--exact-depth K` (which CLI lowers to min=max=K)
    /// silently widens.
    #[test]
    fn min_and_max_depth_form_inclusive_window() {
        let mut f = DepthFilter::new(Some(2), Some(4));
        assert_eq!(f.evaluate(&hit(1)), Verdict::Drop);
        assert_eq!(f.evaluate(&hit(2)), Verdict::Keep);
        assert_eq!(f.evaluate(&hit(4)), Verdict::Keep);
        assert_eq!(f.evaluate(&hit(5)), Verdict::Drop);
    }
}
