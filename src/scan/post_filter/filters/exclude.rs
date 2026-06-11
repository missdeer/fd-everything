//! `-E` / `--exclude`: wraps `ignore::overrides::Override` over an
//! already-built matcher. Mirrors `walk.rs:259-274`.

use anyhow::{Result, anyhow};
use ignore::Match;
use ignore::overrides::{Override, OverrideBuilder};

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct ExcludeFilter {
    overrides: Override,
}

impl ExcludeFilter {
    /// Build from the same inputs `walk.rs::build_overrides` uses: the
    /// first search root (anchors the matcher) plus the
    /// `Config::exclude_patterns` list.
    pub fn new(first_root: &std::path::Path, patterns: &[String]) -> Result<Self> {
        let mut builder = OverrideBuilder::new(first_root);
        for p in patterns {
            builder
                .add(p)
                .map_err(|e| anyhow!("Malformed exclude pattern: {}", e))?;
        }
        let overrides = builder
            .build()
            .map_err(|_| anyhow!("Mismatch in exclude patterns"))?;
        Ok(Self { overrides })
    }
}

impl Filter for ExcludeFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        // `Override::matched` semantics: `Ignore` means "matches a
        // negative pattern" → drop; `Whitelist` would mean an explicit
        // `!` re-include (not produced by fd's CLI) → keep; `None`
        // means no rule applies → keep.
        match self.overrides.matched(&hit.path, hit.is_dir) {
            Match::Ignore(_) => Verdict::Drop,
            Match::Whitelist(_) | Match::None => Verdict::Keep,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn hit(path: PathBuf, is_dir: bool) -> RawHit {
        RawHit {
            path,
            is_dir,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// Encodes the `--exclude '*.tmp'` contract: anchored matcher drops
    /// `.tmp` files under the root, keeps unrelated files. If this
    /// regresses, `walk.rs` and post-filter diverge on the same flag.
    ///
    /// Note `main.rs:389` prepends `!` to each user pattern before it
    /// reaches `Config::exclude_patterns`, so the production input to
    /// `ExcludeFilter::new` is `!*.tmp` (an `ignore::overrides`
    /// negative rule) — the test mirrors that wire-up.
    #[test]
    fn glob_pattern_drops_matching_files() {
        let tmp = tempdir().unwrap();
        let mut f = ExcludeFilter::new(tmp.path(), &["!*.tmp".to_string()]).unwrap();
        assert_eq!(
            f.evaluate(&hit(tmp.path().join("a.tmp"), false)),
            Verdict::Drop
        );
        assert_eq!(
            f.evaluate(&hit(tmp.path().join("a.rs"), false)),
            Verdict::Keep
        );
    }
}
