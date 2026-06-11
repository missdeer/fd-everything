//! `--fixed-strings` / `--exact` translation (PLAN.md §6.1.1, v6 revision).
//!
//! Everything's query syntax has these metacharacters:
//!
//! * space (AND)
//! * `|` (OR)
//! * `!` (NOT)
//! * `<` `>` (group)
//! * `"` (phrase quote — no in-quote escape supported)
//! * `\` (triggers partial-path matching)
//! * `:` (modifier / macro separator)
//! * `?` / `*` (wildcards — active even inside `"…"`)
//!
//! We use a **conservative whitelist** strategy. The input is a *user
//! literal* — by definition the user wants every byte to compare
//! verbatim. If we can't faithfully express that in Everything's syntax,
//! we surface a `GiveUp` and the caller (`translate()`) routes the query
//! to the LegacyWalker.

/// Result of the §6.1.1 escape pipeline.
pub enum LiteralResult {
    /// A single Everything search fragment (already includes any needed
    /// quoting). Caller pastes it directly into the search string.
    Term(String),
    /// PLAN §6.1.1 rules 3-5: the literal contains a character we cannot
    /// faithfully escape, so the caller must fall back to LegacyWalker.
    GiveUp,
}

/// `s` is the **raw** literal as typed by the user (no fd preprocessing).
/// Returns a §6.1.1-compliant Everything term.
///
/// Empty input is treated as "match everything" (returns an empty term),
/// which is what fd does at the CLI level for omitted patterns.
pub fn translate_fixed_string(s: &str) -> LiteralResult {
    if s.is_empty() {
        return LiteralResult::Term(String::new());
    }

    // PLAN §6.1.1 rule 3: `"` can't be escaped inside a phrase.
    if s.contains('"') {
        return LiteralResult::GiveUp;
    }
    // PLAN §6.1.1 rule 4: `\` triggers partial-path matching even inside
    // a phrase. Safer to bail than to risk a widened match set.
    if s.contains('\\') {
        return LiteralResult::GiveUp;
    }
    // PLAN §6.1.1 rule 5: `?` / `*` stay wildcards in phrases. A user who
    // typed `--fixed-strings 'a*b'` expects a literal star.
    if s.contains(['*', '?']) {
        return LiteralResult::GiveUp;
    }

    // PLAN §6.1.1 rule 1 / 2: every other character — including space, |,
    // !, <, >, :, dots, dashes, slashes — is safe inside `"…"`.
    // Rule 1's pure alphanumerics-only special case isn't worth a separate
    // unquoted path: phrase quoting is one extra byte and keeps the
    // emitted query human-readable for diffing.
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    out.push_str(s);
    out.push('"');
    LiteralResult::Term(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_of(r: LiteralResult) -> String {
        match r {
            LiteralResult::Term(t) => t,
            LiteralResult::GiveUp => panic!("expected Term, got GiveUp"),
        }
    }

    fn is_giveup(r: LiteralResult) -> bool {
        matches!(r, LiteralResult::GiveUp)
    }

    /// PLAN §6.1.1 rule 1: pure alphanumeric literal goes inside quotes.
    /// If this regresses, fd's `--fixed-strings foo` could match the
    /// modifier `foo:` in Everything's parser.
    #[test]
    fn alphanumeric_literal_is_phrase_quoted() {
        assert_eq!(term_of(translate_fixed_string("foo")), "\"foo\"");
    }

    /// PLAN §6.1.1 rule 2: spaces are AND in Everything; quoting is the
    /// canonical fix. Regression here would silently widen a multi-word
    /// `--fixed-strings 'hello world'` into two ANDed search terms.
    #[test]
    fn space_containing_literal_is_phrase_quoted() {
        assert_eq!(
            term_of(translate_fixed_string("hello world")),
            "\"hello world\""
        );
    }

    /// PLAN §6.1.1 rule 2: metacharacters that are inert inside a phrase
    /// (`|`, `!`, `<`, `>`, `:`) survive the round-trip.
    #[test]
    fn modifier_chars_survive_inside_phrase() {
        for s in ["a|b", "a!b", "a<b", "a>b", "key:value"] {
            assert_eq!(
                term_of(translate_fixed_string(s)),
                format!("\"{s}\""),
                "rule-2 metachar {s} should phrase-quote"
            );
        }
    }

    /// PLAN §6.1.1 rule 3: `"` has no in-quote escape, so we give up.
    #[test]
    fn literal_with_quote_gives_up() {
        assert!(is_giveup(translate_fixed_string("foo\"bar")));
    }

    /// PLAN §6.1.1 rule 4: backslash triggers Everything's partial-path
    /// mode. We refuse to gamble that a user literal should silently
    /// switch matching modes.
    #[test]
    fn literal_with_backslash_gives_up() {
        assert!(is_giveup(translate_fixed_string(r"foo\bar")));
    }

    /// PLAN §6.1.1 rule 5: `*` and `?` are wildcards in phrases too.
    #[test]
    fn literal_with_wildcards_gives_up() {
        assert!(is_giveup(translate_fixed_string("foo*bar")));
        assert!(is_giveup(translate_fixed_string("foo?bar")));
    }

    /// Empty literal maps to an empty term — `fd --fixed-strings ''` is
    /// effectively "match every name". Callers concatenate this into the
    /// search string and the resulting `"path:…"`-only query is what we
    /// want.
    #[test]
    fn empty_literal_maps_to_empty_term() {
        assert_eq!(term_of(translate_fixed_string("")), "");
    }
}
