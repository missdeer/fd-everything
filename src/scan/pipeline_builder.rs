//! PLAN.md §Phase 8.5-C: assemble a `post_filter::Pipeline` from
//! `&Config`, applying the §0 P6 "fast-path skip" principle so that
//! filters whose CLI dimension is unused never enter the chain.
//!
//! Filter ordering is dictated by PLAN.md §Phase 3 (the diagram at
//! lines 311-329) and must match `walk.rs`'s implicit ordering so that
//! the LegacyWalker and Everything paths are byte-equivalent for every
//! `RawHit` they both reach. The pipeline runs single-threaded inside
//! `PostFilterSink::send`; parallelism is fanned out elsewhere
//! (`IgnoreCache::matched` is internally parallel, `--exec`-bound work
//! is handled by `walk::WorkerState::receive`).

use std::path::Path;

use anyhow::Result;
use etcetera::BaseStrategy;

use crate::config::Config;
use crate::scan::backend::{CancellationToken, CaseModifier};
use crate::scan::post_filter::filters::depth::DepthFilter;
use crate::scan::post_filter::filters::exclude::ExcludeFilter;
use crate::scan::post_filter::filters::extension::ExtensionFilter;
use crate::scan::post_filter::filters::hidden_by_attr::HiddenByAttrFilter;
use crate::scan::post_filter::filters::hidden_by_name::HiddenByNameFilter;
use crate::scan::post_filter::filters::ignore_cache::IgnoreCacheFilter;
use crate::scan::post_filter::filters::ignore_contain::IgnoreContainFilter;
use crate::scan::post_filter::filters::max_results::MaxResultsFilter;
use crate::scan::post_filter::filters::prune::PruneFilter;
use crate::scan::post_filter::filters::same_filesystem::{
    NoopVolumeIdProvider, SameFilesystemFilter,
};
use crate::scan::post_filter::filters::size::SizeConstraints;
use crate::scan::post_filter::filters::symlink_filter::SymlinkFilter;
use crate::scan::post_filter::filters::time_filter::TimeConstraints;
use crate::scan::post_filter::filters::type_filter::TypeFilter;
use crate::scan::post_filter::ignore_cache::{GlobalLayers, IgnoreCache, IgnoreFlags};
use crate::scan::post_filter::{Filter, Pipeline};

/// Assemble a [`Pipeline`] for the given `Config`. `first_root` is the
/// canonical absolute path of the first search root — it anchors
/// [`ExcludeFilter`]'s `ignore::overrides::Override` builder (which
/// requires an anchor even when the supplied patterns are pure-name
/// globs).
///
/// The returned pipeline is consumed by a single [`PostFilterSink`] —
/// build a new one per backend run.
///
/// [`PostFilterSink`]: crate::scan::post_filter::PostFilterSink
pub(crate) fn build_pipeline(
    config: &Config,
    first_root: &Path,
    cancel: &CancellationToken,
) -> Result<Pipeline> {
    let mut filters: Vec<Box<dyn Filter>> = Vec::new();

    // PLAN §Phase 3 #1: ignore_contain runs first so a marker-bearing
    // directory's whole subtree gets cut before depth / root accounting.
    if !config.ignore_contain.is_empty() {
        filters.push(Box::new(IgnoreContainFilter::new(
            config.ignore_contain.clone(),
        )));
    }

    // PLAN §Phase 3 #2 (§3.3): cross-volume hits dropped before any
    // expensive stat-equivalent fires. NoopVolumeIdProvider is fail-
    // open on Windows — `--one-file-system` becomes a no-op until
    // Phase 5+ lands the real `GetVolumeInformationW` probe. Match
    // walk.rs behaviour on Windows where the legacy walker's
    // `same_file_system` is also a no-op without `windows-sys`.
    if config.one_file_system {
        filters.push(Box::new(SameFilesystemFilter::new(Box::new(
            NoopVolumeIdProvider,
        ))));
    }

    // PLAN §Phase 3 #3: `-E/--exclude`. The patterns in `Config` already
    // carry the leading `!` (see `main.rs:389`) so they're parsed as
    // ignore-style negations.
    if !config.exclude_patterns.is_empty() {
        filters.push(Box::new(ExcludeFilter::new(
            first_root,
            &config.exclude_patterns,
        )?));
    }

    // PLAN §Phase 3 #4-#7: cheap RawHit-only filters (no syscall).
    if let Some(types) = config.file_types.as_ref() {
        filters.push(Box::new(TypeFilter::new(types.clone())));
    }
    if let Some(set) = config.extensions.as_ref() {
        filters.push(Box::new(ExtensionFilter::new(set.clone())));
    }
    if !config.size_constraints.is_empty() {
        filters.push(Box::new(SizeConstraints::new(
            config.size_constraints.clone(),
        )));
    }
    if !config.time_constraints.is_empty() {
        filters.push(Box::new(TimeConstraints::new(
            config.time_constraints.clone(),
        )));
    }

    // PLAN §Phase 3 #9: depth. Both `--max-depth` and `--min-depth`
    // flow through here (and `--exact-depth` is CLI-lowered to
    // min==max upstream).
    if config.min_depth.is_some() || config.max_depth.is_some() {
        filters.push(Box::new(DepthFilter::new(
            config.min_depth,
            config.max_depth,
        )));
    }

    // PLAN §Phase 3 #10 (v7.2 split): hidden_by_name runs BEFORE
    // ignore_cache so e.g. the entire `.git/` subtree is cut by a
    // single byte-check rather than evaluating gitignore against every
    // descendant.
    if config.ignore_hidden {
        filters.push(Box::new(HiddenByNameFilter::new(true)));
    }

    // PLAN §Phase 3 #11: ignore_cache (Phase 4). The flags drive both
    // construction (whether we even build the cache) and per-hit
    // short-circuiting inside the filter itself.
    let ignore_flags = ignore_flags_from(config);
    if ignore_flags.any_enabled() {
        let cache = build_ignore_cache(config, ignore_flags);
        filters.push(Box::new(IgnoreCacheFilter::new(cache)));
    }

    // PLAN §Phase 3 #12 (v7.2 split): hidden_by_attr runs AFTER
    // ignore_cache to pay the GetFileAttributesW syscall only for hits
    // that survived the cheaper filters above.
    if config.ignore_hidden {
        filters.push(Box::new(HiddenByAttrFilter::new(true)));
    }

    // PLAN §Phase 3 #14: prune. Buffering filter — only paid by
    // `--prune` users (PLAN §3.1).
    if config.prune {
        filters.push(Box::new(PruneFilter::new()));
    }

    // PLAN §Phase 3 #15: symlink. `follow_links == true` means
    // pass-through; the constructor handles the inversion.
    if !config.follow_links {
        filters.push(Box::new(SymlinkFilter::new(config.follow_links)));
    }

    // PLAN §Phase 3 #16: max_results sits at the TAIL so it counts
    // survivors of every preceding filter. Without `--max-results` no
    // counter is constructed.
    if let Some(max) = config.max_results {
        filters.push(Box::new(MaxResultsFilter::new(max, cancel.clone())));
    }

    Ok(Pipeline::new(filters))
}

fn ignore_flags_from(config: &Config) -> IgnoreFlags {
    IgnoreFlags {
        read_fdignore: config.read_fdignore,
        read_vcsignore: config.read_vcsignore,
        require_git: config.require_git_to_read_vcsignore,
        read_parent_ignore: config.read_parent_ignore,
        read_global_ignore: config.read_global_ignore,
        case_insensitive: !config.case_sensitive,
    }
}

fn build_ignore_cache(config: &Config, flags: IgnoreFlags) -> IgnoreCache {
    let case_modifier = if config.case_sensitive {
        CaseModifier::Case
    } else {
        CaseModifier::Nocase
    };

    // Mirror `walk.rs:300-314`: when global-ignore is enabled, resolve
    // `$XDG_CONFIG_HOME/fd/ignore` and pass it to GlobalLayers; git's
    // `core.excludesFile` is left for a follow-up because reading it
    // needs a libgit2 probe we don't carry yet.
    let fd_global_path = if config.read_global_ignore {
        etcetera::choose_base_strategy()
            .ok()
            .map(|s| s.config_dir().join("fd").join("ignore"))
    } else {
        None
    };

    let globals = GlobalLayers::build(
        None,
        fd_global_path.as_deref(),
        &config.ignore_files,
        flags.case_insensitive,
    );

    // `parent_ceilings` is non-empty only when `--no-ignore-parent` is
    // set: it tells IgnoreCache to stop climbing past these roots.
    let parent_ceilings = if config.read_parent_ignore {
        Vec::new()
    } else {
        config
            .search_roots
            .iter()
            .map(|sr| sr.canonical.clone())
            .collect()
    };

    IgnoreCache::new(flags, globals, parent_ceilings, case_modifier)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::scan::backend::RawHit;
    use crate::scan::path_projection::SearchRoot;
    use crate::scan::post_filter::PipelineOutcome;

    fn empty_config() -> Config {
        Config {
            case_sensitive: false,
            full_path_base: None,
            ignore_hidden: false,
            read_fdignore: false,
            read_parent_ignore: true,
            read_vcsignore: false,
            require_git_to_read_vcsignore: false,
            read_global_ignore: false,
            follow_links: true,
            one_file_system: false,
            null_separator: false,
            max_depth: None,
            min_depth: None,
            prune: false,
            threads: 1,
            quiet: false,
            max_buffer_time: None,
            ls_colors: None,
            interactive_terminal: false,
            file_types: None,
            extensions: None,
            format: None,
            command: None,
            batch_size: 0,
            exclude_patterns: vec![],
            ignore_files: vec![],
            size_constraints: vec![],
            time_constraints: vec![],
            #[cfg(unix)]
            owner_constraint: None,
            show_filesystem_errors: false,
            path_separator: None,
            actual_path_separator: "/".into(),
            max_results: None,
            strip_cwd_prefix: false,
            absolute_paths: false,
            search_roots: Arc::new(vec![SearchRoot {
                canonical: PathBuf::from(r"C:\repo"),
                display: PathBuf::from(r"C:\repo"),
            }]),
            hyperlink: false,
            ignore_contain: vec![],
            force_legacy: false,
            raw_pattern: String::new(),
            raw_and_patterns: vec![],
            pattern_is_glob: false,
            pattern_is_fixed_strings: false,
            pattern_is_exact: false,
        }
    }

    fn hit() -> RawHit {
        RawHit {
            path: PathBuf::from(r"C:\repo\file.txt"),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "txt".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from(r"C:\repo")),
        }
    }

    /// Encodes the P6 "fast-path skip" principle: a Config with no
    /// filter-relevant fields set produces a pipeline that forwards
    /// every hit without running any filter logic. If this regresses,
    /// users who supply no filters still pay per-filter overhead — the
    /// §0 hot-path budget tells callers they should pay zero.
    #[test]
    fn empty_config_produces_no_op_pipeline() {
        let cfg = empty_config();
        let cancel = CancellationToken::new();
        let mut pipe = build_pipeline(&cfg, Path::new(r"C:\repo"), &cancel).unwrap();
        assert_eq!(pipe.process(&hit()), PipelineOutcome::Forward);
    }

    /// Encodes the §Phase 3 ordering invariant for `--max-results`: it
    /// must sit at the tail so its counter only ticks for survivors.
    /// Verify the tail position by observing that a hit dropped by an
    /// upstream filter does NOT trip the counter — if max_results were
    /// at the head, the counter would tick on every input hit and
    /// `--type d --max-results 2` would short-circuit after seeing two
    /// files of any type.
    #[test]
    fn max_results_runs_after_type_filter() {
        let mut cfg = empty_config();
        cfg.max_results = Some(1);
        cfg.file_types = Some(crate::filetypes::FileTypes {
            directories: true,
            ..Default::default()
        });
        let cancel = CancellationToken::new();
        let mut pipe = build_pipeline(&cfg, Path::new(r"C:\repo"), &cancel).unwrap();

        // First hit: file (not directory) — TypeFilter drops, max_results
        // should NOT tick.
        let file_hit = hit();
        assert_eq!(pipe.process(&file_hit), PipelineOutcome::Drop);
        assert!(
            !cancel.is_cancelled(),
            "max_results must not count dropped hits"
        );

        // Second hit: directory — survives TypeFilter, ticks max_results,
        // which forwards-and-cancels because limit=1.
        let dir_hit = RawHit {
            is_dir: true,
            ..hit()
        };
        assert_eq!(pipe.process(&dir_hit), PipelineOutcome::ForwardAndCancel);
        assert!(cancel.is_cancelled());
    }

    /// PLAN §0 P6 verification: when `--no-ignore` is set (both
    /// read_fdignore and read_vcsignore false), IgnoreCacheFilter is
    /// NOT inserted. The point is to skip parent-chain DirIgnoreState
    /// construction — a chain of stats across N ancestors per hit is
    /// not a cost we want to pay only to short-circuit inside the
    /// filter.
    #[test]
    fn no_ignore_skips_ignore_cache_filter() {
        let mut cfg = empty_config();
        cfg.read_fdignore = false;
        cfg.read_vcsignore = false;
        // If IgnoreCacheFilter were present and tried to load the cwd's
        // .gitignore, this test could become non-deterministic. With it
        // omitted, the pipeline is provably a no-op for this hit.
        let cancel = CancellationToken::new();
        let mut pipe = build_pipeline(&cfg, Path::new(r"C:\repo"), &cancel).unwrap();
        assert_eq!(pipe.process(&hit()), PipelineOutcome::Forward);
    }

    /// PLAN §Phase 3 #14: PruneFilter must NOT enter the chain unless
    /// `--prune` is set, because its `evaluate` returns `Drop` on every
    /// hit and only emits real survivors via `drain()` — a stray
    /// PruneFilter would silently swallow the whole result set.
    #[test]
    fn prune_off_means_no_buffering_filter() {
        let cfg = empty_config();
        let cancel = CancellationToken::new();
        let mut pipe = build_pipeline(&cfg, Path::new(r"C:\repo"), &cancel).unwrap();
        // Forward proves no PruneFilter is in the chain. With PruneFilter
        // present this would be Drop (and only drain would re-emit).
        assert_eq!(pipe.process(&hit()), PipelineOutcome::Forward);
    }
}
