//! PLAN.md §Phase 6 — query translation from fd's CLI surface into a
//! [`BackendQuery`] consumable by [`EverythingBackend`]. Returns
//! [`TranslationGiveUp`] when any single CLI dimension cannot be
//! faithfully expressed, so the backend-selection layer
//! ([`super::selection`]) can transparently fall back to the
//! LegacyWalker.
//!
//! Why a separate `TranslationInput` struct rather than `&Config`?
//!
//! `Config` strips the raw user pattern after compiling the `Regex` —
//! `translate()` needs the *unprocessed* string to drive Everything's
//! own regex engine. Plumbing the raw pattern through `Config` is a
//! Phase 7 concern; for Phase 6 the translation API is self-contained
//! so it can be unit-tested without a `Config` mock.
//!
//! [`BackendQuery`]: crate::scan::backend::BackendQuery
//! [`EverythingBackend`]: crate::scan::backend::everything::EverythingBackend

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::filetypes::FileTypes;
use crate::filter::{SizeFilter, TimeFilter};
use crate::scan::backend::{
    BackendQuery, CaseModifier, EntryTypeHint, PatternScope, SizeRange, TimeRange,
    TranslatedPattern,
};

mod glob;
mod literal;
mod unsupported;

#[cfg(test)]
pub(crate) use unsupported::RejectReason;
pub(crate) use unsupported::check_regex_pattern;

/// Sentinel returned when any single CLI dimension can't be expressed in
/// Everything's query language. PLAN §Phase 6 contract: the
/// [`super::selection::select_backend`] layer maps this to a
/// LegacyWalkerBackend selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationGiveUp {
    pub reason: GiveUpReason,
}

/// Cause of a [`TranslationGiveUp`]. Names intentionally drop the
/// shared `Unsupported` prefix so `clippy::enum_variant_names` stays
/// quiet without an allow attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GiveUpReason {
    /// Regex AST contains a construct on PLAN §6.2's reject list.
    Regex(unsupported::RejectReason),
    /// `--fixed-strings` literal contains `"`, `\`, `*`, or `?` —
    /// see PLAN §6.1.1 rules 3-5.
    Literal,
    /// `--glob` pattern triggered PLAN §6.2 rule 9 (negation /
    /// Unicode class ranges) or contains nested braces we don't expand.
    Glob,
}

/// Input bundle for the translation pipeline. All fields come straight
/// from `cli::Opts` with the same names; the smartcase merge into
/// `case_sensitive` is the caller's responsibility (matches PLAN §6.1
/// "复用 fd 的判断结果").
pub struct TranslationInput<'a> {
    pub pattern: &'a str,
    pub glob: bool,
    pub fixed_strings: bool,
    pub exact: bool,
    pub and_patterns: &'a [String],
    pub full_path: bool,
    pub case_sensitive: bool,
    pub file_types: Option<&'a FileTypes>,
    pub extensions: &'a [String],
    pub size_filters: &'a [SizeFilter],
    pub time_filters: &'a [TimeFilter],
    pub max_depth: Option<usize>,
    pub max_results: Option<usize>,
}

/// Build a [`BackendQuery`] from CLI state plus a list of search roots
/// (already canonicalized by the caller).
pub fn translate(
    input: &TranslationInput<'_>,
    paths: Vec<PathBuf>,
) -> Result<BackendQuery, TranslationGiveUp> {
    let case_modifier = if input.case_sensitive {
        CaseModifier::Case
    } else {
        CaseModifier::Nocase
    };
    let scope = if input.full_path {
        PatternScope::FullPath
    } else {
        PatternScope::Basename
    };

    let pattern = translate_one(input.pattern, input, scope, case_modifier)?;
    let mut and_patterns = Vec::with_capacity(input.and_patterns.len());
    for p in input.and_patterns {
        and_patterns.push(translate_one(p, input, scope, case_modifier)?);
    }

    Ok(BackendQuery {
        paths,
        pattern,
        and_patterns,
        type_hint: derive_type_hint(input.file_types),
        size_hint: derive_size_hint(input.size_filters),
        time_hint: derive_time_hint(input.time_filters),
        max_depth: input.max_depth,
        max_results: input.max_results,
    })
}

/// Translate one pattern (primary OR `--and` clause) — picks regex /
/// glob / literal / exact branch based on the CLI flag combination.
fn translate_one(
    raw: &str,
    input: &TranslationInput<'_>,
    scope: PatternScope,
    case_modifier: CaseModifier,
) -> Result<TranslatedPattern, TranslationGiveUp> {
    // fd's --glob and --fixed-strings are mutually exclusive at the CLI
    // layer (clap `conflicts_with`); --exact rules out both. So the
    // branches are disjoint by construction.
    let fragment = if input.glob {
        match glob::translate_glob(raw) {
            glob::GlobResult::Fragment(f) => format!("wildcards:{f}"),
            glob::GlobResult::GiveUp => {
                return Err(TranslationGiveUp {
                    reason: GiveUpReason::Glob,
                });
            }
        }
    } else if input.fixed_strings {
        match literal::translate_fixed_string(raw) {
            literal::LiteralResult::Term(t) => t,
            literal::LiteralResult::GiveUp => {
                return Err(TranslationGiveUp {
                    reason: GiveUpReason::Literal,
                });
            }
        }
    } else if input.exact {
        // PLAN §6.1 row 4: "--exact" is full-match literal. Reuse the
        // literal pipeline for escaping, then anchor with ^…$ in regex
        // syntax (Everything's `regex:` engine). Quote the completed regex
        // so Everything's outer query parser does not consume `^` or `\`.
        match literal::translate_fixed_string(raw) {
            literal::LiteralResult::Term(_) if raw.is_empty() => String::new(),
            literal::LiteralResult::Term(_) => {
                format!("regex:\"^{}$\"", escape_regex_meta(raw))
            }
            literal::LiteralResult::GiveUp => {
                return Err(TranslationGiveUp {
                    reason: GiveUpReason::Literal,
                });
            }
        }
    } else {
        // Default: treat as regex.
        if let Err(reason) = check_regex_pattern(raw) {
            return Err(TranslationGiveUp {
                reason: GiveUpReason::Regex(reason),
            });
        }
        if raw.is_empty() {
            String::new()
        } else {
            // Everything parses search operators before the `regex:` engine
            // sees its payload. Without phrase quotes, alternation in a Rust
            // regex (for example `(d|f)`) is consumed as Everything-level OR
            // and can silently return no matches on Everything 1.4.x.
            format!("regex:\"{raw}\"")
        }
    };

    Ok(TranslatedPattern {
        everything_query: fragment,
        scope,
        case_modifier,
    })
}

/// Escape the regex metacharacters in a literal so it can be embedded
/// inside `regex:"^…$"` for `--exact`. Mirrors `regex::escape` but produces
/// output Everything's regex engine accepts (POSIX-ish), which is the
/// same metaset for our purposes.
fn escape_regex_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            other => out.push(other),
        }
    }
    out
}

fn derive_type_hint(types: Option<&FileTypes>) -> Option<EntryTypeHint> {
    let t = types?;
    // PLAN §6.3 row 1: only the pure `file:` / `folder:` push-down is
    // safe. Mixed filters (e.g. file+symlink, executables_only,
    // empty_only) need post-filter to be canonical, so we don't emit a
    // hint when any of those are in play.
    if t.executables_only || t.empty_only {
        return None;
    }
    if t.symlinks || t.block_devices || t.char_devices || t.sockets || t.pipes {
        return None;
    }
    match (t.files, t.directories) {
        (true, false) => Some(EntryTypeHint::File),
        (false, true) => Some(EntryTypeHint::Directory),
        _ => None,
    }
}

fn derive_size_hint(filters: &[SizeFilter]) -> Option<SizeRange> {
    if filters.is_empty() {
        return None;
    }
    let mut min: Option<u64> = None;
    let mut max: Option<u64> = None;
    for f in filters {
        match *f {
            SizeFilter::Min(v) => {
                min = Some(min.map_or(v, |cur| cur.max(v)));
            }
            SizeFilter::Max(v) => {
                max = Some(max.map_or(v, |cur| cur.min(v)));
            }
            SizeFilter::Equals(v) => {
                // `+v -v` is the same as exactly v; the post-filter
                // checks the exact-equal branch precisely, so a hint of
                // [v, v] is the loosest correct bound.
                min = Some(min.map_or(v, |cur| cur.max(v)));
                max = Some(max.map_or(v, |cur| cur.min(v)));
            }
        }
    }
    Some(SizeRange {
        min_bytes: min,
        max_bytes: max,
    })
}

fn derive_time_hint(filters: &[TimeFilter]) -> Option<TimeRange> {
    if filters.is_empty() {
        return None;
    }
    let mut min_ft: Option<i64> = None;
    let mut max_ft: Option<i64> = None;
    for f in filters {
        match f {
            TimeFilter::After(t) => {
                let ft = system_time_to_filetime(*t);
                min_ft = Some(min_ft.map_or(ft, |cur| cur.max(ft)));
            }
            TimeFilter::Before(t) => {
                let ft = system_time_to_filetime(*t);
                max_ft = Some(max_ft.map_or(ft, |cur| cur.min(ft)));
            }
        }
    }
    Some(TimeRange {
        min_filetime: min_ft,
        max_filetime: max_ft,
    })
}

/// Convert a `SystemTime` to a Win32 FILETIME (100ns ticks since
/// 1601-01-01 UTC). Matches the canonical form used in `RawHit::mtime`
/// per PLAN §Phase 1 v7.1.
fn system_time_to_filetime(t: SystemTime) -> i64 {
    // 11_644_473_600 seconds between 1601-01-01 and 1970-01-01.
    const UNIX_TO_FILETIME_SECONDS: i64 = 11_644_473_600;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs() as i64 + UNIX_TO_FILETIME_SECONDS;
            secs.saturating_mul(10_000_000) + (d.subsec_nanos() as i64 / 100)
        }
        Err(e) => {
            // Pre-1970 time. Negative duration; same arithmetic with
            // sign preserved.
            let d = e.duration();
            let secs = -(d.as_secs() as i64) + UNIX_TO_FILETIME_SECONDS;
            secs.saturating_mul(10_000_000) - (d.subsec_nanos() as i64 / 100)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_for(pattern: &str) -> TranslationInput<'_> {
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

    fn root() -> Vec<PathBuf> {
        vec![PathBuf::from(r"C:\repo")]
    }

    /// PLAN §6.1: default branch (no flags) treats the pattern as a regex
    /// and emits a phrase-protected `regex:"<pat>"`. Regression here would
    /// let Everything's outer query parser consume regex metacharacters and
    /// invocation match nothing on Everything.
    #[test]
    fn default_branch_is_regex() {
        let q = translate(&input_for("foo"), root()).unwrap();
        assert_eq!(q.pattern.everything_query, "regex:\"foo\"");
        assert_eq!(q.pattern.scope, PatternScope::Basename);
        assert_eq!(q.pattern.case_modifier, CaseModifier::Nocase);
    }

    /// PLAN §6.1: `--glob` → `wildcards:` prefix. `**/` is stripped per
    /// the §6.1 mapping table.
    #[test]
    fn glob_branch_emits_wildcards() {
        let mut input = input_for("**/*.rs");
        input.glob = true;
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.pattern.everything_query, "wildcards:*.rs");
    }

    /// PLAN §6.1.1: `--fixed-strings` quotes the literal. Whole-pattern
    /// quoting is the cheapest correct rule; this test makes sure a
    /// future "skip-quotes-for-alphanumerics" tweak doesn't drop them.
    #[test]
    fn fixed_strings_branch_phrase_quotes() {
        let mut input = input_for("hello world");
        input.fixed_strings = true;
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.pattern.everything_query, "\"hello world\"");
    }

    /// PLAN §6.1 row 4: `--exact` anchors the literal with `^…$` under
    /// the regex engine. Verifies the escape pipeline runs on the input.
    #[test]
    fn exact_branch_anchors_with_regex_meta_escaped() {
        let mut input = input_for("foo.bar");
        input.exact = true;
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.pattern.everything_query, r#"regex:"^foo\.bar$""#);
    }

    /// Regression: Everything's query parser treats a bare `|` as its own
    /// OR operator before `regex:` evaluation. The regex payload must be
    /// phrase-quoted so alternation reaches the regex engine intact.
    #[test]
    fn regex_alternation_is_quoted_for_everything_query_parser() {
        let q = translate(&input_for(r"^(d|f)xc\.exe$"), root()).unwrap();
        assert_eq!(q.pattern.everything_query, r#"regex:"^(d|f)xc\.exe$""#);
    }

    /// PLAN §6.1 row 5: case is determined by the caller's
    /// `case_sensitive`. The translation layer doesn't second-guess it.
    #[test]
    fn case_modifier_reflects_input() {
        let mut input = input_for("foo");
        input.case_sensitive = true;
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.pattern.case_modifier, CaseModifier::Case);
    }

    /// PLAN §6.1: `--full-path` flips scope. Without it default is
    /// basename. Regression would change every `--full-path` query into
    /// a basename one — silently widening / narrowing results.
    #[test]
    fn full_path_flips_scope() {
        let mut input = input_for("foo");
        input.full_path = true;
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.pattern.scope, PatternScope::FullPath);
    }

    /// PLAN §6.1: `--and <p2>` patterns are each translated and stored
    /// alongside the primary pattern.
    #[test]
    fn and_patterns_round_trip() {
        let extras = vec!["bar".to_string(), "baz".to_string()];
        let mut input = input_for("foo");
        input.and_patterns = &extras;
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.and_patterns.len(), 2);
        assert_eq!(q.and_patterns[0].everything_query, "regex:\"bar\"");
        assert_eq!(q.and_patterns[1].everything_query, "regex:\"baz\"");
    }

    /// PLAN §6.2: an `--and` pattern triggering the unsupported detector
    /// gives up the whole query — partial pushdown would silently change
    /// the result set.
    #[test]
    fn unsupported_and_pattern_gives_up_whole_query() {
        let extras = vec![r"\p{Greek}".to_string()];
        let mut input = input_for("foo");
        input.and_patterns = &extras;
        let err = translate(&input, root()).unwrap_err();
        assert!(matches!(
            err.reason,
            GiveUpReason::Regex(RejectReason::UnicodeProperty)
        ));
    }

    /// PLAN §6.2: top-level pattern with `\p{…}` rejects.
    #[test]
    fn primary_pattern_rule1_rejection_propagates() {
        let err = translate(&input_for(r"\p{Greek}"), root()).unwrap_err();
        assert!(matches!(
            err.reason,
            GiveUpReason::Regex(RejectReason::UnicodeProperty)
        ));
    }

    /// PLAN §6.1.1 rule 3: `--fixed-strings 'a"b'` gives up. Backends
    /// must route this query to the LegacyWalker.
    #[test]
    fn fixed_strings_with_quote_gives_up() {
        let mut input = input_for("a\"b");
        input.fixed_strings = true;
        let err = translate(&input, root()).unwrap_err();
        assert_eq!(err.reason, GiveUpReason::Literal);
    }

    /// PLAN §6.3 row 1: `--type f` populates the file hint; `--type x`
    /// (executables_only) does not, because the post-filter is the only
    /// component that can answer it correctly.
    #[test]
    fn type_hint_only_for_pure_file_or_dir() {
        let mut input = input_for("foo");
        let files = FileTypes {
            files: true,
            ..Default::default()
        };
        input.file_types = Some(&files);
        assert_eq!(
            translate(&input, root()).unwrap().type_hint,
            Some(EntryTypeHint::File)
        );

        let exec = FileTypes {
            files: true,
            executables_only: true,
            ..Default::default()
        };
        input.file_types = Some(&exec);
        assert_eq!(translate(&input, root()).unwrap().type_hint, None);
    }

    /// PLAN §6.3 row 3: `--size +1k -5k` collapses to [1000, 5000].
    /// Verifies the tightest-bound merge — a bug here either widens
    /// (drops the hint) or narrows (drops valid hits) the pushdown.
    #[test]
    fn size_hints_merge_to_tightest_bounds() {
        let mut input = input_for("foo");
        let filters = vec![
            SizeFilter::Min(1000),
            SizeFilter::Max(5000),
            SizeFilter::Min(2000),
        ];
        input.size_filters = &filters;
        let hint = translate(&input, root()).unwrap().size_hint.unwrap();
        assert_eq!(hint.min_bytes, Some(2000));
        assert_eq!(hint.max_bytes, Some(5000));
    }

    /// PLAN §6.3 row 4: time-range push-down derives FILETIME bounds.
    /// We don't assert the exact value (TZ-dependent) — only the
    /// ordering and direction: `After(t)` populates `min_filetime`.
    #[test]
    fn time_hint_after_populates_min() {
        let mut input = input_for("foo");
        let after = TimeFilter::After(UNIX_EPOCH + std::time::Duration::from_secs(1_000));
        let filters = vec![after];
        input.time_filters = &filters;
        let hint = translate(&input, root()).unwrap().time_hint.unwrap();
        assert!(hint.min_filetime.is_some());
        assert!(hint.max_filetime.is_none());
    }

    /// PLAN §6.3 rows 5-6: `--max-depth`, `--max-results` pass through.
    /// These are already consumed by `EverythingBackend::run` (Phase 5
    /// day-1), so the translation just plumbs them.
    #[test]
    fn depth_and_result_caps_pass_through() {
        let mut input = input_for("foo");
        input.max_depth = Some(3);
        input.max_results = Some(50);
        let q = translate(&input, root()).unwrap();
        assert_eq!(q.max_depth, Some(3));
        assert_eq!(q.max_results, Some(50));
    }

    /// Empty pattern is a fd-supported "match everything" case. The
    /// translation must emit an empty fragment (no `regex:` prefix) so
    /// the resulting search string degenerates to just `path:<root>`.
    #[test]
    fn empty_pattern_emits_empty_fragment() {
        let q = translate(&input_for(""), root()).unwrap();
        assert!(q.pattern.everything_query.is_empty());
    }
}
