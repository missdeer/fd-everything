//! `--extension`: matches against `RawHit::extension`. The walker
//! version (`walk.rs:467-475`) matches against the full file name; the
//! Config's `RegexSet` is already compiled with the leading-`.` regex
//! semantics built in, so we have to feed it the file name byte form.
//!
//! Backends populate `extension` with the lowercased suffix without
//! the leading dot. To stay byte-equivalent with `walk.rs` we test
//! against the file name itself when the hit carries one.

use regex::bytes::RegexSet;

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

pub struct ExtensionFilter {
    set: RegexSet,
}

impl ExtensionFilter {
    pub fn new(set: RegexSet) -> Self {
        Self { set }
    }
}

impl Filter for ExtensionFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        let name = match hit.path.file_name() {
            Some(n) => n,
            None => return Verdict::Drop,
        };

        // Cheap byte view of the OsStr — same approach `walk.rs` uses
        // via `filesystem::osstr_to_bytes`. We can't depend on a
        // private helper here, so go through `to_string_lossy` which
        // is allocation-free on the typical UTF-8 path.
        let bytes = match name.to_str() {
            Some(s) => s.as_bytes(),
            None => {
                // Lossy round-trip on non-UTF-8 names. Rare; matches
                // walk.rs's fallback semantics (it also goes through
                // an OsStr -> bytes conversion that loses non-UTF-8
                // on Windows).
                return if self.set.is_match(name.to_string_lossy().as_bytes()) {
                    Verdict::Keep
                } else {
                    Verdict::Drop
                };
            }
        };
        if self.set.is_match(bytes) {
            Verdict::Keep
        } else {
            Verdict::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit(path: &str) -> RawHit {
        RawHit {
            path: PathBuf::from(path),
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

    /// Encodes `--extension rs|toml` semantics — matches anchored on
    /// the file name, not the full path. Regression here would mean a
    /// directory named "foo.rs" gets matched on its children's paths.
    #[test]
    fn extension_set_matches_file_name_suffix() {
        // The regex set is what cli.rs builds: each ext gets compiled
        // as `\.ext$` against the file name bytes.
        let set = RegexSet::new([r"\.rs$", r"\.toml$"]).unwrap();
        let mut f = ExtensionFilter::new(set);
        assert_eq!(f.evaluate(&hit("/p/a.rs")), Verdict::Keep);
        assert_eq!(f.evaluate(&hit("/p/Cargo.toml")), Verdict::Keep);
        assert_eq!(f.evaluate(&hit("/p/README")), Verdict::Drop);
    }
}
