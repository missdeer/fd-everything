use std::{path::PathBuf, sync::Arc, time::Duration};

use lscolors::LsColors;
use regex::bytes::RegexSet;

use crate::exec::CommandSet;
use crate::filetypes::FileTypes;
#[cfg(unix)]
use crate::filter::OwnerFilter;
use crate::filter::{SizeFilter, TimeFilter};
use crate::fmt::FormatTemplate;
use crate::scan::path_projection::{PathProjector, SearchRoot};

/// Configuration options for *fd*.
pub struct Config {
    /// Whether the search is case-sensitive or case-insensitive.
    pub case_sensitive: bool,

    /// Cached current working directory for absolute path construction.
    /// Populated when `--full-path` is set; `None` means search by filename only.
    pub full_path_base: Option<PathBuf>,

    /// Whether to ignore hidden files and directories (or not).
    pub ignore_hidden: bool,

    /// Whether to respect `.fdignore` files or not.
    pub read_fdignore: bool,

    /// Whether to respect ignore files in parent directories or not.
    pub read_parent_ignore: bool,

    /// Whether to respect VCS ignore files (`.gitignore`, ..) or not.
    pub read_vcsignore: bool,

    /// Whether to require a `.git` directory to respect gitignore files.
    pub require_git_to_read_vcsignore: bool,

    /// Whether to respect the global ignore file or not.
    pub read_global_ignore: bool,

    /// Whether to follow symlinks or not.
    pub follow_links: bool,

    /// Whether to limit the search to starting file system or not.
    pub one_file_system: bool,

    /// Whether elements of output should be separated by a null character
    pub null_separator: bool,

    /// The maximum search depth, or `None` if no maximum search depth should be set.
    ///
    /// A depth of `1` includes all files under the current directory, a depth of `2` also includes
    /// all files under subdirectories of the current directory, etc.
    pub max_depth: Option<usize>,

    /// The minimum depth for reported entries, or `None`.
    pub min_depth: Option<usize>,

    /// Whether to stop traversing into matching directories.
    pub prune: bool,

    /// The number of threads to use.
    pub threads: usize,

    /// If true, the program doesn't print anything and will instead return an exit code of 0
    /// if there's at least one match. Otherwise, the exit code will be 1.
    pub quiet: bool,

    /// Time to buffer results internally before streaming to the console. This is useful to
    /// provide a sorted output, in case the total execution time is shorter than
    /// `max_buffer_time`.
    pub max_buffer_time: Option<Duration>,

    /// `None` if the output should not be colorized. Otherwise, a `LsColors` instance that defines
    /// how to style different filetypes.
    pub ls_colors: Option<LsColors>,

    /// Whether or not we are writing to an interactive terminal
    #[cfg_attr(not(unix), allow(unused))]
    pub interactive_terminal: bool,

    /// The type of file to search for. If set to `None`, all file types are displayed. If
    /// set to `Some(..)`, only the types that are specified are shown.
    pub file_types: Option<FileTypes>,

    /// The extension to search for. Only entries matching the extension will be included.
    ///
    /// The value (if present) will be a lowercase string without leading dots.
    pub extensions: Option<RegexSet>,

    /// A format string to use to format results, similarly to exec
    pub format: Option<FormatTemplate>,

    /// If a value is supplied, each item found will be used to generate and execute commands.
    pub command: Option<Arc<CommandSet>>,

    /// Maximum number of search results to pass to each `command`. If zero, the number is
    /// unlimited.
    pub batch_size: usize,

    /// A list of glob patterns that should be excluded from the search.
    pub exclude_patterns: Vec<String>,

    /// A list of custom ignore files.
    pub ignore_files: Vec<PathBuf>,

    /// The given constraints on the size of returned files
    pub size_constraints: Vec<SizeFilter>,

    /// Constraints on last modification time of files
    pub time_constraints: Vec<TimeFilter>,

    #[cfg(unix)]
    /// User/group ownership constraint
    pub owner_constraint: Option<OwnerFilter>,

    /// Whether or not to display filesystem errors
    pub show_filesystem_errors: bool,

    /// The separator used to print file paths.
    pub path_separator: Option<String>,

    /// The actual separator, either the system default separator or `path_separator`
    pub actual_path_separator: String,

    /// The maximum number of search results
    pub max_results: Option<usize>,

    /// Whether or not to strip the './' prefix for search results
    pub strip_cwd_prefix: bool,

    /// `--absolute-path`. Consumed by `PathProjector` to short-circuit
    /// display projection and emit `raw_path` verbatim (PLAN.md §Phase 2).
    pub absolute_paths: bool,

    /// Phase 2: search roots paired with their user-given display form.
    /// `PathProjector` consults this to translate a hit's absolute
    /// `raw_path` back into the relative form fd prints. `Arc` so the
    /// receiver/sender threads cheap-clone instead of deep-copying.
    pub search_roots: Arc<Vec<SearchRoot>>,

    /// Whether or not to use hyperlinks on paths
    pub hyperlink: bool,

    /// Names that should stop traversal down their parent. (e.g. https://bford.info/cachedir/).
    pub ignore_contain: Vec<String>,

    /// PLAN.md §Phase 6 / §Phase 8: when true (CLI `--filesystem-walker`),
    /// force every search root through the upstream-fd legacy walker even if
    /// the Everything index would otherwise cover it. Consumed by
    /// `scan::backend::everything::selection::select_backend` (the
    /// `force_legacy` parameter) inside `walk::scan` (PLAN §Phase 8.5
    /// routing slice).
    pub force_legacy: bool,

    /// CLI `--probe`. When true, run the real `EverythingVolumeIndexProbe`
    /// (count-only `path:"<root>"` query) before each Everything routing
    /// decision. Default false trusts the user's root is indexed and skips
    /// the per-root IPC. Mock-backend sessions ignore this flag and force
    /// `AssumeIndexedProbe` so tempdir fixtures still exercise the mock.
    pub probe: bool,

    /// PLAN.md §Phase 8.5-A: raw user pattern, kept verbatim so the Phase 6
    /// translation layer (`scan::backend::everything::query::translate`) can
    /// drive Everything's own regex/glob engines. The compiled `Vec<Regex>`
    /// in `walk::scan` is built from this same string but with
    /// glob/exact/fixed-strings transforms baked in — Everything wants the
    /// unprocessed input.
    pub raw_pattern: String,

    /// PLAN.md §Phase 8.5-A: raw `--and` patterns (one per occurrence),
    /// kept verbatim for the same reason as `raw_pattern`.
    pub raw_and_patterns: Vec<String>,

    /// PLAN.md §Phase 8.5-A: pattern interpretation flags forwarded as-is
    /// to the Phase 6 translation layer. The CLI guarantees at most one of
    /// `glob`/`fixed_strings`/`exact` is set; we mirror the tri-state with
    /// three bools to avoid leaking a translation-private enum into
    /// `Config`.
    pub pattern_is_glob: bool,
    pub pattern_is_fixed_strings: bool,
    pub pattern_is_exact: bool,
}

impl Config {
    /// Check whether results are being printed.
    pub fn is_printing(&self) -> bool {
        self.command.is_none()
    }

    /// Build a fresh `PathProjector` that borrows from this `Config`.
    /// Cheap — no allocations beyond the projector struct itself, and
    /// `roots` is already pre-sorted at scan start (PLAN.md §Phase 2).
    pub fn path_projector(&self) -> PathProjector<'_> {
        PathProjector::new(
            self.search_roots.as_slice(),
            self.absolute_paths,
            self.strip_cwd_prefix,
        )
    }
}
