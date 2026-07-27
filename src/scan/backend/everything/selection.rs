//! PLAN.md §6.4 — backend auto-selection.
//!
//! Per search path P:
//!
//! ```text
//! if P is not on an Everything-indexed volume
//!    or force_legacy is true
//!    or translate(...) returns TranslationGiveUp
//! then
//!     LegacyWalkerBackend
//! else
//!     EverythingBackend(query)
//! ```
//!
//! This module is purely a routing decision. The actual
//! `LegacyWalkerBackend` lives elsewhere (Phase 7 wiring) — Phase 6 only
//! commits to the *shape* of the choice, expressed as
//! [`BackendChoice`]. That keeps Phase 6 testable without the legacy
//! walker, the Everything SDK, or a real volume probe.

use std::path::{Path, PathBuf};

use crate::scan::backend::BackendQuery;

use super::query::{TranslationGiveUp, TranslationInput, translate};

/// Outcome of [`select_backend`] for a single search path.
///
/// `BackendQuery` doesn't implement `PartialEq` (no need; the post-filter
/// pipeline never compares two of them), so `BackendChoice` doesn't
/// either. Tests use `matches!` plus field assertions.
#[derive(Debug, Clone)]
pub enum BackendChoice {
    /// EverythingBackend can serve this path with the translated query.
    /// Phase 7 wraps this in `Box<dyn SearchBackend>`.
    Everything(BackendQuery),
    /// Fall back to the legacy `ignore::WalkBuilder`-based walker.
    /// `reason` is purely informational — used by `--show-errors` to
    /// explain *why* a query degraded.
    Legacy { reason: FallbackReason },
}

/// Why a given search path was routed to the legacy walker. Useful for
/// diagnostics and future telemetry; not load-bearing for correctness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// User passed `--filesystem-walker`. Phase 7 plumbs this from CLI.
    ForcedByFlag,
    /// PLAN §6.4: the path lives on a non-indexed volume or network
    /// share that Everything hasn't been told to index.
    PathNotIndexed,
    /// Translation gave up — any of the PLAN §6.1 / §6.2 reasons.
    QueryUnsupported(TranslationGiveUp),
}

/// Pluggable probe answering "is `path` on an Everything-indexed
/// volume?". Defined as a trait so Phase 6 tests inject a fake and
/// Phase 7 can wire a real Everything-SDK probe without touching this
/// module.
pub trait VolumeIndexProbe {
    fn is_indexed(&self, path: &Path) -> bool;
}

/// Default probe — assumes every path is indexed. This is the right
/// pre-wiring stub because the alternative (assume none indexed) would
/// silently route every query to the legacy walker once Phase 7 hooks
/// this into `walk::scan`. Phase 7 must replace it with a real probe
/// before EverythingBackend can ever fire.
#[derive(Debug, Clone, Copy, Default)]
pub struct AssumeIndexedProbe;
impl VolumeIndexProbe for AssumeIndexedProbe {
    fn is_indexed(&self, _path: &Path) -> bool {
        true
    }
}

/// Decide a backend for each `path` independently. Multi-path queries
/// can mix backends — Phase 7 fans them out and merges the streams.
///
/// `force_legacy` mirrors the planned `--filesystem-walker` CLI flag.
pub fn select_backend(
    paths: Vec<PathBuf>,
    input: &TranslationInput<'_>,
    probe: &dyn VolumeIndexProbe,
    force_legacy: bool,
) -> Vec<BackendChoice> {
    paths
        .into_iter()
        .map(|p| classify(p, input, probe, force_legacy))
        .collect()
}

fn classify(
    path: PathBuf,
    input: &TranslationInput<'_>,
    probe: &dyn VolumeIndexProbe,
    force_legacy: bool,
) -> BackendChoice {
    if force_legacy {
        return BackendChoice::Legacy {
            reason: FallbackReason::ForcedByFlag,
        };
    }
    if !probe.is_indexed(&path) {
        return BackendChoice::Legacy {
            reason: FallbackReason::PathNotIndexed,
        };
    }
    match translate(input, vec![path]) {
        Ok(query) => BackendChoice::Everything(query),
        Err(give_up) => BackendChoice::Legacy {
            reason: FallbackReason::QueryUnsupported(give_up),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::backend::everything::query::RejectReason;

    /// Test double: index-membership is a path-prefix lookup.
    struct PrefixProbe {
        indexed: Vec<PathBuf>,
    }
    impl VolumeIndexProbe for PrefixProbe {
        fn is_indexed(&self, p: &Path) -> bool {
            self.indexed.iter().any(|root| p.starts_with(root))
        }
    }

    fn input_for(pattern: &'static str) -> TranslationInput<'static> {
        TranslationInput {
            pattern,
            glob: false,
            fixed_strings: false,
            exact: false,
            and_patterns: &[],
            full_path: false,
            case_sensitive: false,
            file_types: None,
            extensions: &[],
            size_filters: &[],
            time_filters: &[],
            max_depth: None,
            max_results: None,
        }
    }

    /// PLAN §6.4 path 1: when `--filesystem-walker` is set, every path
    /// goes to the legacy walker — translation never runs and the probe
    /// is never consulted.
    #[test]
    fn forced_legacy_skips_translation_and_probe() {
        struct PanickingProbe;
        impl VolumeIndexProbe for PanickingProbe {
            fn is_indexed(&self, _: &Path) -> bool {
                panic!("probe should not be consulted under force_legacy");
            }
        }
        let choices = select_backend(
            vec![PathBuf::from(r"C:\repo")],
            &input_for("foo"),
            &PanickingProbe,
            true,
        );
        assert_eq!(choices.len(), 1);
        assert!(matches!(
            choices[0],
            BackendChoice::Legacy {
                reason: FallbackReason::ForcedByFlag
            }
        ));
    }

    /// PLAN §6.4 path 2: paths off the indexed set fall back without
    /// translation noise. Regression here would silently broadcast every
    /// query against the global Everything index.
    #[test]
    fn non_indexed_path_routes_to_legacy() {
        let probe = PrefixProbe {
            indexed: vec![PathBuf::from(r"C:\")],
        };
        let choices = select_backend(
            vec![PathBuf::from(r"\\server\share")],
            &input_for("foo"),
            &probe,
            false,
        );
        assert!(matches!(
            choices[0],
            BackendChoice::Legacy {
                reason: FallbackReason::PathNotIndexed
            }
        ));
    }

    /// PLAN §6.4 path 3: indexed path + supported query => Everything.
    #[test]
    fn indexed_path_with_supported_query_picks_everything() {
        let probe = PrefixProbe {
            indexed: vec![PathBuf::from(r"C:\")],
        };
        let choices = select_backend(
            vec![PathBuf::from(r"C:\repo")],
            &input_for("foo"),
            &probe,
            false,
        );
        match &choices[0] {
            BackendChoice::Everything(q) => {
                assert_eq!(q.pattern.everything_query, "regex:\"foo\"");
                assert_eq!(q.paths, vec![PathBuf::from(r"C:\repo")]);
            }
            other => panic!("expected Everything choice, got {other:?}"),
        }
    }

    /// PLAN §6.4 path 4: indexed path + unsupported query falls back
    /// with the structured reason preserved (useful for `--show-errors`
    /// in Phase 7).
    #[test]
    fn indexed_path_with_unsupported_query_falls_back_with_reason() {
        let probe = PrefixProbe {
            indexed: vec![PathBuf::from(r"C:\")],
        };
        let choices = select_backend(
            vec![PathBuf::from(r"C:\repo")],
            &input_for(r"\p{Greek}"),
            &probe,
            false,
        );
        match &choices[0] {
            BackendChoice::Legacy {
                reason: FallbackReason::QueryUnsupported(g),
            } => {
                use super::super::query::GiveUpReason;
                assert!(matches!(
                    g.reason,
                    GiveUpReason::Regex(RejectReason::UnicodeProperty)
                ));
            }
            other => panic!("expected Legacy/QueryUnsupported, got {other:?}"),
        }
    }

    /// PLAN §6.4: multi-path queries can mix backends. The selector
    /// classifies each path independently — Phase 7 fans them out.
    #[test]
    fn mixed_paths_get_per_path_decisions() {
        let probe = PrefixProbe {
            indexed: vec![PathBuf::from(r"C:\")],
        };
        let choices = select_backend(
            vec![
                PathBuf::from(r"C:\repo"),
                PathBuf::from(r"\\server\share"),
                PathBuf::from(r"C:\src"),
            ],
            &input_for("foo"),
            &probe,
            false,
        );
        assert!(matches!(choices[0], BackendChoice::Everything(_)));
        assert!(matches!(
            choices[1],
            BackendChoice::Legacy {
                reason: FallbackReason::PathNotIndexed
            }
        ));
        assert!(matches!(choices[2], BackendChoice::Everything(_)));
    }

    /// `AssumeIndexedProbe` is the Phase 6 pre-wiring stub: every path
    /// is reported as indexed so once `walk::scan` integrates this
    /// module, the Everything fast path actually fires. If this
    /// regresses (e.g. accidental `false`), Phase 7 would silently
    /// always-walk.
    #[test]
    fn default_probe_assumes_every_path_indexed() {
        assert!(AssumeIndexedProbe.is_indexed(Path::new(r"C:\anywhere")));
        assert!(AssumeIndexedProbe.is_indexed(Path::new(r"\\srv\share")));
    }
}
