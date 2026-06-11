//! Hidden-by-attribute: drops hits whose Win32 file attribute bitmask
//! has `FILE_ATTRIBUTE_HIDDEN` set. PLAN.md §Phase 3 v7.2 places this
//! after `ignore_cache` because a hit that's already ignored doesn't
//! need the attribute look-up — and the EverythingBackend pre-populates
//! `RawHit::attributes`, so no syscall fires here even on the post-
//! filter path (the legacy walker keeps its own `walk.rs` flow).

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

/// Win32 `FILE_ATTRIBUTE_HIDDEN`. Constant kept local to avoid a
/// `windows-sys` dep until Phase 5 (FFI). Value source: Win32 API docs
/// (FileAttributes documentation page).
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;

pub struct HiddenByAttrFilter {
    enabled: bool,
}

impl HiddenByAttrFilter {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl Filter for HiddenByAttrFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if !self.enabled {
            return Verdict::Keep;
        }
        if hit.attributes & FILE_ATTRIBUTE_HIDDEN != 0 {
            Verdict::Drop
        } else {
            Verdict::Keep
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit(attrs: u32) -> RawHit {
        RawHit {
            path: PathBuf::from("/x"),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: attrs,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// Encodes the FILE_ATTRIBUTE_HIDDEN drop on Win32. If this
    /// regresses, `desktop.ini` and `Thumbs.db` reappear under
    /// `--no-hidden`.
    #[test]
    fn hidden_bit_drops_when_enabled() {
        let mut f = HiddenByAttrFilter::new(true);
        assert_eq!(f.evaluate(&hit(FILE_ATTRIBUTE_HIDDEN)), Verdict::Drop);
        assert_eq!(f.evaluate(&hit(0)), Verdict::Keep);
    }
}
