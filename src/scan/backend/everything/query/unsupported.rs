//! Regex AST scanner that decides whether a regex pattern can be handed to
//! Everything's `regex:` query or whether the translation layer
//! must give up and route the query to `LegacyWalkerBackend`.
//!
//! Implements PLAN.md §6.2 (rules 1-9), plus query-layer safety checks needed
//! to embed the regex in Everything's outer search syntax. Named
//! `RejectReason` variants ensure a regression fails a specific test rather
//! than a generic "translation regressed" assertion.
//!
//! The scanner is best-effort: when the input string can't even be parsed by
//! `regex-syntax`, we hand it to Everything anyway and let the SDK reject it.
//! That keeps us from second-guessing the canonical parser (which fd already
//! ran during arg parsing — see `regex_helper.rs`).

use regex_syntax::ast::{
    Assertion, AssertionKind, Ast, ClassSet, ClassSetItem, Flag, FlagsItemKind, GroupKind,
    parse::Parser,
};

/// PLAN.md §6.2 rule 6: defensive ceiling on AST nesting depth. Patterns
/// deeper than this are treated as `TranslationGiveUp` rather than risk
/// Everything's `regex:` engine behaving differently on pathological input.
pub(crate) const MAX_AST_DEPTH: usize = 16;

/// One named incompatibility, surfaced verbatim in tests / future telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// The pattern contains `"`, which cannot be embedded in the quoted
    /// `regex:"..."` term required to protect regex metacharacters from
    /// Everything's outer query parser.
    QueryQuote,
    /// Rule 1: `\p{…}` / `\P{…}` Unicode property class.
    UnicodeProperty,
    /// Rule 2: `\b` / `\B` / `\w` / `\W` / `\d` / `\D` / `\s` / `\S`
    /// without an explicit `(?-u)` flag stripping Unicode awareness.
    UnicodeAwarePerlClassOrBoundary,
    /// Rule 3: `\A`, `\z`, `\Z` — Rust string anchors with no Everything
    /// equivalent (Everything's `^`/`$` are filename anchors).
    UnsupportedAnchor,
    /// Rule 3 + Rule 4: `(?m)` multiline mode (changes `^`/`$` semantics)
    /// or `(?s)` dot-matches-newline (Everything's regex engine differs).
    UnsupportedFlag,
    /// Rule 5: `(?i:…)` / `(?-i:…)` scoped case flag. Everything's
    /// `case:` / `nocase:` is whole-query only.
    ScopedCaseFlag,
    /// Rule 6: AST nesting depth exceeded `MAX_AST_DEPTH`.
    NestingTooDeep,
    /// Rule 7: `\1`-`\9` or `(?P=name)` backreference (Rust doesn't even
    /// support these, but defensive matchers might).
    Backreference,
    /// Rule 8: `(?=…)` / `(?!…)` / `(?<=…)` / `(?<!…)` lookaround.
    Lookaround,
}

/// Inspect the regex pattern's AST. Returns `Ok(())` if Everything can
/// faithfully evaluate it, otherwise the first reason hit.
///
/// PLAN.md §6.2 explicitly says best-effort: if `regex-syntax` cannot parse
/// `pattern`, this function returns `Ok(())` and lets Everything report the
/// failure. Re-running our own parser would just diverge from fd's canonical
/// `regex` crate behavior.
pub fn check_regex_pattern(pattern: &str) -> Result<(), RejectReason> {
    // Everything parses its search language before handing the payload of
    // `regex:` to the regex engine. The payload therefore has to be phrase-
    // quoted so query operators such as `|`, `^`, and `\` survive intact.
    // Everything has no escape for a quote inside a phrase, so fall back to
    // the filesystem walker for this rare (but valid Rust-regex) case.
    if pattern.contains('"') {
        return Err(RejectReason::QueryQuote);
    }

    let mut parser = Parser::new();
    let ast = match parser.parse(pattern) {
        Ok(ast) => ast,
        Err(_) => return Ok(()),
    };
    scan_ast(&ast, 0)
}

fn scan_ast(ast: &Ast, depth: usize) -> Result<(), RejectReason> {
    if depth > MAX_AST_DEPTH {
        return Err(RejectReason::NestingTooDeep);
    }

    match ast {
        // -- atoms ----------------------------------------------------------
        Ast::Empty(_) | Ast::Literal(_) | Ast::Dot(_) => Ok(()),

        // PLAN §6.2 rule 3: only `^` and `$` survive — anything else needs
        // a Rust-only feature (\A, \z, \Z) we can't push to Everything.
        // PLAN §6.2 rule 2: word boundaries are Unicode-aware.
        // We match against the kind by its `to_string` to stay
        // resilient against regex-syntax minor-version additions of new
        // anchor variants — the canonical names of `\A`, `\z`, `\Z`,
        // `\b`, `\B` are stable.
        Ast::Assertion(a) => classify_assertion(a),

        // -- class shapes --------------------------------------------------
        Ast::ClassPerl(_) => Err(RejectReason::UnicodeAwarePerlClassOrBoundary),
        // PLAN §6.2 rule 1: `\p{…}` / `\P{…}`.
        Ast::ClassUnicode(_) => Err(RejectReason::UnicodeProperty),

        // PLAN §6.2 rule 9 (glob branch is enforced separately for globs;
        // for regex bracket classes we just descend so nested unicode /
        // perl items fall to their respective rules).
        Ast::ClassBracketed(b) => scan_class_set(&b.kind, depth + 1),

        // -- repetition / concatenation / alternation ----------------------
        Ast::Repetition(r) => scan_ast(&r.ast, depth + 1),
        Ast::Concat(c) => {
            for a in &c.asts {
                scan_ast(a, depth + 1)?;
            }
            Ok(())
        }
        Ast::Alternation(alt) => {
            for a in &alt.asts {
                scan_ast(a, depth + 1)?;
            }
            Ok(())
        }

        // -- groups & flags ------------------------------------------------
        Ast::Group(g) => {
            // PLAN §6.2 rules 5 + 8: classify the group kind first.
            match &g.kind {
                GroupKind::CaptureIndex(_) | GroupKind::CaptureName { .. } => {}
                GroupKind::NonCapturing(flags) => {
                    // `(?i:…)` etc. We treat ANY scoped flag as reject —
                    // the only way to reach Everything is whole-query
                    // case sensitivity already decided in `Config`.
                    if flags_set_contains_problematic(flags.items.iter().map(|i| &i.kind)) {
                        return Err(RejectReason::ScopedCaseFlag);
                    }
                }
            }
            scan_ast(&g.ast, depth + 1)
        }
        // Toplevel `(?m)foo` and friends — these are global flag-only AST
        // nodes that change semantics for everything that follows.
        Ast::Flags(set) => {
            for item in &set.flags.items {
                match item.kind {
                    FlagsItemKind::Flag(f) => match f {
                        Flag::MultiLine | Flag::DotMatchesNewLine => {
                            return Err(RejectReason::UnsupportedFlag);
                        }
                        Flag::Unicode | Flag::SwapGreed | Flag::IgnoreWhitespace => {
                            return Err(RejectReason::UnsupportedFlag);
                        }
                        Flag::CRLF => return Err(RejectReason::UnsupportedFlag),
                        // `(?-u)` flips off the Unicode flag; a scoped
                        // case flag (i) is already covered by the
                        // group-level branch above. A bare top-level
                        // `(?i)` would have been merged into Config's
                        // case_sensitive by `pattern_has_uppercase_char`.
                        Flag::CaseInsensitive => {}
                    },
                    FlagsItemKind::Negation => {}
                }
            }
            Ok(())
        }
    }
}

fn classify_assertion(a: &Assertion) -> Result<(), RejectReason> {
    match a.kind {
        // PLAN §6.2 rule 3 (subset): `^` / `$` translate directly.
        AssertionKind::StartLine | AssertionKind::EndLine => Ok(()),
        // PLAN §6.2 rule 3: Rust-only absolute string anchors.
        AssertionKind::StartText | AssertionKind::EndText => Err(RejectReason::UnsupportedAnchor),
        // PLAN §6.2 rule 2: word boundaries are Unicode-aware.
        AssertionKind::WordBoundary | AssertionKind::NotWordBoundary => {
            Err(RejectReason::UnicodeAwarePerlClassOrBoundary)
        }
        // Defensive: if regex-syntax grows a new anchor (e.g.
        // `EndTextWithOptionalLF`, `WordBoundaryStartHalf`) we can't
        // assume Everything handles it. Treat as unsupported anchor —
        // safer than a silent passthrough that produces wrong matches.
        _ => Err(RejectReason::UnsupportedAnchor),
    }
}

fn scan_class_set(set: &ClassSet, depth: usize) -> Result<(), RejectReason> {
    if depth > MAX_AST_DEPTH {
        return Err(RejectReason::NestingTooDeep);
    }
    match set {
        ClassSet::Item(item) => scan_class_item(item, depth + 1),
        ClassSet::BinaryOp(op) => {
            scan_class_set(&op.lhs, depth + 1)?;
            scan_class_set(&op.rhs, depth + 1)
        }
    }
}

fn scan_class_item(item: &ClassSetItem, depth: usize) -> Result<(), RejectReason> {
    if depth > MAX_AST_DEPTH {
        return Err(RejectReason::NestingTooDeep);
    }
    match item {
        ClassSetItem::Empty(_) | ClassSetItem::Literal(_) | ClassSetItem::Range(_) => Ok(()),
        ClassSetItem::Ascii(_) => Ok(()),
        ClassSetItem::Unicode(_) => Err(RejectReason::UnicodeProperty),
        ClassSetItem::Perl(_) => Err(RejectReason::UnicodeAwarePerlClassOrBoundary),
        ClassSetItem::Bracketed(b) => scan_class_set(&b.kind, depth + 1),
        ClassSetItem::Union(u) => {
            for it in &u.items {
                scan_class_item(it, depth + 1)?;
            }
            Ok(())
        }
    }
}

fn flags_set_contains_problematic<'a>(flags: impl Iterator<Item = &'a FlagsItemKind>) -> bool {
    // A scoped `(?i:…)` / `(?-i:…)` / `(?m:…)` group is the rule-5
    // condition: any of the disallowed flags inside a non-capturing group
    // means the scope only applies to a sub-expression, which Everything
    // can't represent.
    for kind in flags {
        if let FlagsItemKind::Flag(f) = kind {
            match f {
                Flag::CaseInsensitive
                | Flag::MultiLine
                | Flag::DotMatchesNewLine
                | Flag::Unicode
                | Flag::SwapGreed
                | Flag::IgnoreWhitespace
                | Flag::CRLF => return true,
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain literals must never trigger a fallback. If this regresses,
    /// every fd invocation goes through the slow legacy walker even on
    /// trivially-translatable queries.
    #[test]
    fn plain_literal_is_accepted() {
        assert!(check_regex_pattern("foo").is_ok());
    }

    /// Everything's outer query language cannot represent a quote inside
    /// the phrase that protects the regex payload.
    #[test]
    fn quote_routes_to_filesystem_walker() {
        assert_eq!(
            check_regex_pattern("foo\"bar").unwrap_err(),
            RejectReason::QueryQuote
        );
    }

    /// Empty input parses as an empty AST — must succeed (matches fd's
    /// "no pattern == match everything" semantics).
    #[test]
    fn empty_pattern_is_accepted() {
        assert!(check_regex_pattern("").is_ok());
    }

    /// PLAN §6.2 rule 1: `\p{Greek}` and friends are Rust-only.
    #[test]
    fn rule1_unicode_property_rejected() {
        assert_eq!(
            check_regex_pattern(r"\p{Greek}").unwrap_err(),
            RejectReason::UnicodeProperty
        );
        assert_eq!(
            check_regex_pattern(r"\P{Letter}").unwrap_err(),
            RejectReason::UnicodeProperty
        );
    }

    /// PLAN §6.2 rule 2: `\w` etc. are Unicode-aware in Rust.
    #[test]
    fn rule2_perl_classes_rejected() {
        for p in [r"\w", r"\W", r"\d", r"\D", r"\s", r"\S"] {
            assert_eq!(
                check_regex_pattern(p).unwrap_err(),
                RejectReason::UnicodeAwarePerlClassOrBoundary,
                "expected rule-2 rejection for {p}"
            );
        }
    }

    /// PLAN §6.2 rule 2: word boundaries.
    #[test]
    fn rule2_word_boundaries_rejected() {
        for p in [r"\bfoo", r"\Bfoo"] {
            assert_eq!(
                check_regex_pattern(p).unwrap_err(),
                RejectReason::UnicodeAwarePerlClassOrBoundary,
                "expected rule-2 rejection for {p}"
            );
        }
    }

    /// PLAN §6.2 rule 3: `^` and `$` are the only safe anchors.
    #[test]
    fn rule3_safe_anchors_pass() {
        assert!(check_regex_pattern("^foo$").is_ok());
    }

    /// PLAN §6.2 rule 3: `\A`, `\z` reject. `\Z` isn't a stable Rust
    /// `regex_syntax` token (it errors at parse time or becomes a
    /// literal); the best-effort scanner therefore lets `\Z` pass and
    /// Everything's engine rejects it at search time. That's the PLAN
    /// §6.2 "best-effort" stance applied consistently.
    #[test]
    fn rule3_strict_anchors_rejected() {
        for p in [r"\Afoo", r"foo\z"] {
            assert_eq!(
                check_regex_pattern(p).unwrap_err(),
                RejectReason::UnsupportedAnchor,
                "expected rule-3 rejection for {p}"
            );
        }
    }

    /// PLAN §6.2 rule 4: `(?m)`, `(?s)`, `(?x)`, `(?-u)`.
    #[test]
    fn rule4_inline_flags_rejected() {
        for p in [r"(?m)foo", r"(?s)foo", r"(?x)foo", r"(?-u)foo"] {
            assert_eq!(
                check_regex_pattern(p).unwrap_err(),
                RejectReason::UnsupportedFlag,
                "expected rule-4 rejection for {p}"
            );
        }
    }

    /// PLAN §6.2 rule 5: scoped `(?i:…)` cannot map to Everything's
    /// whole-query `case:` / `nocase:` modifier.
    #[test]
    fn rule5_scoped_case_flag_rejected() {
        assert_eq!(
            check_regex_pattern(r"(?i:foo)").unwrap_err(),
            RejectReason::ScopedCaseFlag
        );
        assert_eq!(
            check_regex_pattern(r"(?-i:foo)").unwrap_err(),
            RejectReason::ScopedCaseFlag
        );
    }

    /// PLAN §6.2 rule 6: pathological depth gets rejected before
    /// Everything sees a 17-level-deep regex.
    #[test]
    fn rule6_deep_nesting_rejected() {
        let mut deep = String::new();
        for _ in 0..40 {
            deep.push('(');
        }
        deep.push('a');
        for _ in 0..40 {
            deep.push(')');
        }
        assert_eq!(
            check_regex_pattern(&deep).unwrap_err(),
            RejectReason::NestingTooDeep
        );
    }

    /// Unicode property *inside* a bracket class still triggers rule 1.
    /// If this regresses, `[\p{Letter}]` would slip through to Everything
    /// and either match nothing or match incorrectly.
    #[test]
    fn unicode_property_inside_class_rejected() {
        assert_eq!(
            check_regex_pattern(r"[\p{L}]").unwrap_err(),
            RejectReason::UnicodeProperty
        );
    }

    /// Ordinary capture group with a plain literal inside is fine.
    /// Guards against an over-eager rejection of every parenthesized
    /// subexpression.
    #[test]
    fn capture_group_with_plain_literal_passes() {
        assert!(check_regex_pattern("(foo)").is_ok());
        assert!(check_regex_pattern("(foo|bar)").is_ok());
    }

    /// Garbage input that even `regex-syntax` can't parse is accepted
    /// here so Everything's `regex:` engine can produce the actual error
    /// message — matches PLAN §6.2's "best-effort" stance.
    #[test]
    fn unparseable_pattern_is_passed_through() {
        // Unbalanced paren — regex-syntax errors out.
        assert!(check_regex_pattern("(foo").is_ok());
    }
}
