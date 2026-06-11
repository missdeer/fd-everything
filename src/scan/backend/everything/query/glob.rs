//! `--glob`/`-g` translation to Everything's `wildcards:` modifier
//! (PLAN.md §6.1 row 2).
//!
//! Mapping rules from the PLAN table:
//!
//! * `**/` → drop. Everything's wildcards already match any number of
//!   path segments; the recursive-glob marker has no equivalent and no
//!   need.
//! * `{a,b}` brace expansion → expand to multiple wildcards joined with
//!   `|` (Everything's OR). Nested braces are not currently expanded —
//!   we surface that as `GiveUp` to keep the table semantics honest.
//! * Globset's `!` prefix (negation) → `GiveUp`. Everything has `!` for
//!   NOT but mixing it with a `path:<root>` constraint is fiddly and the
//!   `wildcards:` modifier doesn't compose with `!` cleanly.
//! * Unicode ranges in character classes (e.g. `[α-ω]`) → `GiveUp`.

/// Outcome of glob translation.
pub enum GlobResult {
    /// One or more wildcard fragments joined with `|`. Caller pastes
    /// directly into `wildcards:<fragment>`.
    Fragment(String),
    GiveUp,
}

/// Translate a single fd `--glob` pattern.
pub fn translate_glob(pat: &str) -> GlobResult {
    // PLAN §6.2 rule 9 part A: negation prefix from globset.
    if pat.starts_with('!') {
        return GlobResult::GiveUp;
    }

    // PLAN §6.1.1 mapping: drop the recursive-glob marker. Everything
    // wildcards already span path separators implicitly when we set
    // `path:` scope, and don't span them at all under `nopath:`. Either
    // way `**/` is a noise prefix.
    let pat = strip_recursive_marker(pat);

    // Brace expansion. Single-level only — nested or escaped braces
    // give up rather than guess.
    let alternatives = match expand_braces(&pat) {
        Some(alts) => alts,
        None => return GlobResult::GiveUp,
    };

    // PLAN §6.2 rule 9 part B: reject Unicode ranges inside `[…]`.
    for alt in &alternatives {
        if has_unicode_class_range(alt) {
            return GlobResult::GiveUp;
        }
    }

    if alternatives.len() == 1 {
        GlobResult::Fragment(alternatives.into_iter().next().unwrap())
    } else {
        GlobResult::Fragment(alternatives.join("|"))
    }
}

fn strip_recursive_marker(pat: &str) -> String {
    // Walk slash-separated segments and drop bare `**`.
    let mut out = String::with_capacity(pat.len());
    let mut first = true;
    for seg in pat.split('/') {
        if seg == "**" {
            continue;
        }
        if !first {
            out.push('/');
        }
        out.push_str(seg);
        first = false;
    }
    out
}

/// Expand a single level of `{a,b,c}`. Returns `None` if the input
/// contains nested braces or an unbalanced brace — the caller surfaces
/// that as `GiveUp` (we won't gamble on heuristics).
fn expand_braces(pat: &str) -> Option<Vec<String>> {
    if !pat.contains('{') {
        return Some(vec![pat.to_string()]);
    }

    // Find first `{`. Anything before is a literal prefix.
    let open = pat.find('{')?;
    let prefix = &pat[..open];
    let rest = &pat[open + 1..];

    let close = rest.find('}')?;
    let group = &rest[..close];
    let suffix = &rest[close + 1..];

    // Nested braces inside the group → give up.
    if group.contains('{') || group.contains('}') {
        return None;
    }
    // Braces in suffix would require iteration; defer for now.
    if suffix.contains('{') || suffix.contains('}') {
        return None;
    }

    let mut out = Vec::new();
    for alt in group.split(',') {
        out.push(format!("{prefix}{alt}{suffix}"));
    }
    Some(out)
}

fn has_unicode_class_range(s: &str) -> bool {
    // PLAN §6.2 rule 9 part B: walk through `[…]` segments and flag any
    // non-ASCII byte inside.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let close = match bytes[i + 1..].iter().position(|&b| b == b']') {
                Some(p) => i + 1 + p,
                None => return false,
            };
            let class = &s[i + 1..close];
            if class.bytes().any(|b| b >= 0x80) {
                return true;
            }
            i = close + 1;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(g: GlobResult) -> String {
        match g {
            GlobResult::Fragment(f) => f,
            GlobResult::GiveUp => panic!("expected Fragment, got GiveUp"),
        }
    }

    /// PLAN §6.1: plain `*.rs` passes through verbatim.
    #[test]
    fn plain_extension_glob_passes_through() {
        assert_eq!(frag(translate_glob("*.rs")), "*.rs");
    }

    /// PLAN §6.1: recursive `**/` marker is stripped.
    #[test]
    fn recursive_marker_is_stripped() {
        assert_eq!(frag(translate_glob("**/*.rs")), "*.rs");
        assert_eq!(frag(translate_glob("src/**/*.rs")), "src/*.rs");
    }

    /// PLAN §6.1: `{a,b}` expands to `|`-joined alternatives.
    #[test]
    fn brace_expansion_uses_pipe() {
        assert_eq!(frag(translate_glob("*.{rs,toml}")), "*.rs|*.toml");
    }

    /// PLAN §6.2 rule 9: globset negation is unsupported.
    #[test]
    fn negation_prefix_gives_up() {
        assert!(matches!(translate_glob("!*.rs"), GlobResult::GiveUp));
    }

    /// PLAN §6.2 rule 9: Unicode character classes are unsupported.
    #[test]
    fn unicode_class_range_gives_up() {
        assert!(matches!(translate_glob("[α-ω]*"), GlobResult::GiveUp));
    }

    /// Nested braces could be expanded but the resulting cross-product
    /// is large and error-prone; we explicitly bail. If this regresses,
    /// a `{a,{b,c}}` glob silently produces just one of the alternatives.
    #[test]
    fn nested_braces_give_up() {
        assert!(matches!(translate_glob("{a,{b,c}}"), GlobResult::GiveUp));
    }

    /// ASCII character classes survive — `[abc]` is fine, only Unicode
    /// ranges are rejected.
    #[test]
    fn ascii_class_passes_through() {
        assert_eq!(frag(translate_glob("[abc]*")), "[abc]*");
    }
}
