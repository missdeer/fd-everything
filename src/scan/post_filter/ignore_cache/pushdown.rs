//! Static gitignore pushdown (PLAN.md §4.7).
//!
//! Translates a *safe* subset of root-level `.gitignore` / `.fdignore` /
//! `.ignore` / `.git/info/exclude` rules into Everything query fragments so
//! that big ignored subtrees (`node_modules/`, `target/`, `__pycache__/`,
//! `dist/`) are excluded at the index level rather than streamed through
//! the post-filter.
//!
//! ## Whitelist (every rule must satisfy all)
//!
//! 1. Rule comes from the search root's `.gitignore` / `.git/info/exclude` /
//!    `.fdignore` / `.ignore` (NOT a subdir).
//! 2. Form is `<dirname>/` or `/<dirname>/` where `dirname` ⊆
//!    `[A-Za-z0-9._+-]`.
//! 3. Not a negation (no leading `!`).
//! 4. No `!<dirname>` whitelist anywhere in the parent chain (caller's
//!    responsibility — pass `whitelisted_dirnames` containing every such
//!    name to filter them out).
//!
//! ## Case sensitivity
//!
//! Per §4.2 #8, Windows matching is case-insensitive by default. Everything's
//! `case:` modifier is a *global* query switch; we cannot mix per-clause
//! cases. If `case_modifier == Case`, we MUST NOT push down (the resulting
//! `Target/` subtree would slip past Everything but be caught by the post-
//! filter — correctness is preserved but the §4.7 speedup is lost; PLAN.md
//! §4.7 says explicitly to return empty in this case).

use std::path::Path;

use crate::scan::backend::CaseModifier;

/// Parsed shape of a pushdownable rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleShape {
    /// `<dirname>/` — match a directory of this name at any depth under the
    /// repo root.
    AnyDepth(String),
    /// `/<dirname>/` — match a directory of this name only as a direct
    /// child of the repo root.
    RootOnly(String),
}

/// One classified rule originating from a specific anchor directory (the
/// `.gitignore` etc. containing it).
#[derive(Debug, Clone)]
pub struct GitignoreRule {
    pub shape: RuleShape,
}

impl GitignoreRule {
    fn dirname(&self) -> &str {
        match &self.shape {
            RuleShape::AnyDepth(s) | RuleShape::RootOnly(s) => s,
        }
    }
}

/// Parse a raw `.gitignore` line. Returns `None` if the line is not safely
/// pushdownable. Public so callers can run it against a real file.
pub fn parse_rule(raw: &str) -> Option<GitignoreRule> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    if line.starts_with('!') {
        return None; // negations excluded from pushdown (whitelist 안전 #3)
    }
    let (anchored, body) = if let Some(rest) = line.strip_prefix('/') {
        (true, rest)
    } else {
        (false, line)
    };
    // Must end with `/` (directory marker).
    let dir = body.strip_suffix('/')?;
    if dir.is_empty() || dir.contains('/') {
        return None; // multi-segment paths or wildcards drop out
    }
    if !dir
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
    {
        return None; // anything else might be a glob (`*`, `?`, `[`)
    }
    let shape = if anchored {
        RuleShape::RootOnly(dir.to_string())
    } else {
        RuleShape::AnyDepth(dir.to_string())
    };
    Some(GitignoreRule { shape })
}

/// Translate `rules` into Everything query fragments anchored at `repo_root`.
///
/// Returns an empty Vec if `case_modifier == Case` (per §4.7 case-sensitivity
/// safety) or if `rules` is empty.
///
/// `whitelisted_dirnames` lists every `!<name>` rule that appears anywhere in
/// the parent chain / subtree. Any rule whose dirname is in that set is
/// dropped to preserve correctness.
pub fn compile_static_ignore_pushdown(
    repo_root: &Path,
    rules: &[GitignoreRule],
    whitelisted_dirnames: &[String],
    case_modifier: CaseModifier,
) -> Vec<String> {
    if matches!(case_modifier, CaseModifier::Case) {
        return Vec::new();
    }
    if rules.is_empty() {
        return Vec::new();
    }
    let root_regex = regex_escape_path(repo_root);
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        if whitelisted_dirnames
            .iter()
            .any(|w| eq_ignore_case(w, rule.dirname()))
        {
            continue;
        }
        let dir_re = regex_escape_literal(rule.dirname());
        let frag = match &rule.shape {
            RuleShape::AnyDepth(_) => format!(
                "!regex:^{}([\\\\/].*)?[\\\\/]{}(?:[\\\\/]|$)",
                root_regex, dir_re
            ),
            RuleShape::RootOnly(_) => {
                format!("!regex:^{}[\\\\/]{}(?:[\\\\/]|$)", root_regex, dir_re)
            }
        };
        out.push(frag);
    }
    out
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Regex-escape a literal string for embedding in Everything's `regex:`
/// clause. Backslashes need extra escaping because the query is parsed twice
/// (Everything → regex engine).
fn regex_escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn regex_escape_path(p: &Path) -> String {
    regex_escape_literal(&p.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rule_any(name: &str) -> GitignoreRule {
        GitignoreRule {
            shape: RuleShape::AnyDepth(name.into()),
        }
    }
    fn rule_root(name: &str) -> GitignoreRule {
        GitignoreRule {
            shape: RuleShape::RootOnly(name.into()),
        }
    }

    /// `node_modules/` parses as `AnyDepth`. If this regresses, the biggest
    /// pushdown win in typical Node repos disappears.
    #[test]
    fn parse_node_modules_any_depth() {
        assert_eq!(
            parse_rule("node_modules/").map(|r| r.shape),
            Some(RuleShape::AnyDepth("node_modules".into()))
        );
    }

    /// `/target/` parses as `RootOnly`. Equivalence check: this rule MUST
    /// translate differently from `target/`.
    #[test]
    fn parse_target_root_only() {
        assert_eq!(
            parse_rule("/target/").map(|r| r.shape),
            Some(RuleShape::RootOnly("target".into()))
        );
    }

    /// Negations, wildcards, multi-segment paths — never pushdown candidates.
    #[test]
    fn parse_rejects_unsafe_forms() {
        for line in [
            "!node_modules/",
            "*.log",
            "build/output/",
            "node_modules", // no trailing slash → file or dir, ambiguous
            "src/foo/",
            "fo o/",
            "fo*o/",
            "",
            "# comment",
        ] {
            assert!(
                parse_rule(line).is_none(),
                "must reject {line:?} as pushdown candidate"
            );
        }
    }

    /// Case-sensitive queries MUST NOT pushdown (§4.7 case-safety). If this
    /// regresses, `fde -s target` would silently let `Target/` subtree
    /// through Everything and rely on post-filter cleanup — correct but
    /// kills the perf budget.
    #[test]
    fn case_sensitive_query_disables_pushdown() {
        let repo = PathBuf::from(r"C:\repo");
        let rules = vec![rule_any("node_modules"), rule_root("target")];
        assert!(compile_static_ignore_pushdown(&repo, &rules, &[], CaseModifier::Case).is_empty());
    }

    /// A `!node_modules` whitelist anywhere in the chain MUST drop the
    /// pushdown for that name (§4.7 whitelist safety).
    #[test]
    fn whitelisted_dirname_is_dropped() {
        let repo = PathBuf::from(r"C:\repo");
        let rules = vec![rule_any("node_modules"), rule_any("target")];
        let out = compile_static_ignore_pushdown(
            &repo,
            &rules,
            &["node_modules".into()],
            CaseModifier::Nocase,
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("target"));
    }

    /// AnyDepth translation embeds the `([\\/].*)?` interior so any depth
    /// matches. RootOnly omits it. If this regresses, `/target/` would
    /// silently match `sub/target/` (over-broad) or `target/` would only
    /// match repo root (under-broad).
    #[test]
    fn anydepth_and_root_translations_diverge() {
        let repo = PathBuf::from(r"C:\repo");
        let out = compile_static_ignore_pushdown(
            &repo,
            &[rule_any("node_modules"), rule_root("target")],
            &[],
            CaseModifier::Nocase,
        );
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("([\\\\/].*)?[\\\\/]node_modules"));
        assert!(out[1].ends_with("[\\\\/]target(?:[\\\\/]|$)"));
        assert!(!out[1].contains("([\\\\/].*)?"));
    }

    /// Repo root prefix MUST appear in every fragment (so rules from one
    /// repo don't leak to another repo on the same volume). §4.7 source-
    /// limitation.
    #[test]
    fn fragments_anchor_at_repo_root() {
        let repo = PathBuf::from(r"C:\repo");
        let out = compile_static_ignore_pushdown(
            &repo,
            &[rule_any("node_modules")],
            &[],
            CaseModifier::Nocase,
        );
        assert!(out[0].starts_with("!regex:^C:\\\\repo"));
    }
}
