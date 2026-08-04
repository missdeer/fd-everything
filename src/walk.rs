use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use etcetera::BaseStrategy;
use ignore::overrides::{Override, OverrideBuilder};
use ignore::{WalkBuilder, WalkParallel, WalkState};
use regex::bytes::Regex;

use crate::config::Config;
use crate::dir_entry::DirEntry;
use crate::error::print_error;
use crate::exec;
use crate::exit_codes::{ExitCode, merge_exitcodes};
use crate::filesystem;
use crate::output;
#[cfg(target_os = "windows")]
use crate::scan::backend::everything::{
    EverythingBackend, EverythingError,
    probe::EverythingVolumeIndexProbe,
    query::TranslationInput,
    selection::{AssumeIndexedProbe, BackendChoice, VolumeIndexProbe, select_backend},
};
use crate::scan::backend::{BackendError, BackendQuery, CancellationToken, SearchBackend};
use crate::scan::mock_injection;
use crate::scan::pipeline_builder::build_pipeline;
use crate::scan::post_filter::PostFilterSink;
use crate::scan::sink::{Batch, BatchSender, WorkerResult};
use crate::scan::sink_adapter::{RawHitBatchSink, RootRebaseSink};

/// The receiver thread can either be buffering results or directly streaming to the console.
#[derive(PartialEq)]
enum ReceiverMode {
    /// Receiver is still buffering in order to sort the results, if the search finishes fast
    /// enough.
    Buffering,

    /// Receiver is directly printing results to the output.
    Streaming,
}

/// Maximum size of the output buffer before flushing results to the console
const MAX_BUFFER_LENGTH: usize = 1000;
/// Default duration until output buffering switches to streaming.
///
/// fd upstream uses 100ms here so small queries finish inside the buffer
/// window and come out sorted. fde diverges: Everything's index makes the
/// first hit available within a few ms, so the 100ms wait is pure perceived
/// latency for the common case. Default to zero (immediate streaming);
/// users who want the opportunistic sort can pass `--sort` or
/// `--max-buffer-time`.
const DEFAULT_MAX_BUFFER_TIME: Duration = Duration::ZERO;

/// Wrapper for the receiver thread's buffering behavior.
struct ReceiverBuffer<'a, W> {
    /// The configuration.
    config: &'a Config,
    /// For shutting down the senders.
    quit_flag: &'a AtomicBool,
    /// The ^C notifier.
    interrupt_flag: &'a AtomicBool,
    /// Receiver for worker results.
    rx: Receiver<Batch>,
    /// Standard output.
    stdout: W,
    /// The current buffer mode.
    mode: ReceiverMode,
    /// The deadline to switch to streaming mode.
    deadline: Instant,
    /// The buffer of quickly received paths.
    buffer: Vec<DirEntry>,
    /// Result count.
    num_results: usize,
}

impl<'a, W: Write> ReceiverBuffer<'a, W> {
    /// Create a new receiver buffer.
    fn new(state: &'a WorkerState, rx: Receiver<Batch>, stdout: W) -> Self {
        let config = &state.config;
        let quit_flag = state.quit_flag.as_ref();
        let interrupt_flag = state.interrupt_flag.as_ref();
        let max_buffer_time = config.max_buffer_time.unwrap_or(DEFAULT_MAX_BUFFER_TIME);
        let deadline = Instant::now() + max_buffer_time;
        // Skip the Buffering state entirely when there's no budget for it —
        // otherwise the first poll iteration parks a batch in `buffer` and
        // only the second iteration's `recv_deadline` timeout flushes it,
        // which costs a context switch the user paid `--stream` to avoid.
        let mode = if max_buffer_time.is_zero() {
            ReceiverMode::Streaming
        } else {
            ReceiverMode::Buffering
        };

        Self {
            config,
            quit_flag,
            interrupt_flag,
            rx,
            stdout,
            mode,
            deadline,
            buffer: Vec::with_capacity(MAX_BUFFER_LENGTH),
            num_results: 0,
        }
    }

    /// Process results until finished.
    fn process(&mut self) -> ExitCode {
        loop {
            if let Err(ec) = self.poll() {
                self.quit_flag.store(true, Ordering::Relaxed);
                return ec;
            }
        }
    }

    /// Receive the next worker result.
    fn recv(&self) -> Result<Batch, RecvTimeoutError> {
        match self.mode {
            ReceiverMode::Buffering => {
                // Wait at most until we should switch to streaming
                self.rx.recv_deadline(self.deadline)
            }
            ReceiverMode::Streaming => {
                // Wait however long it takes for a result
                Ok(self.rx.recv()?)
            }
        }
    }

    /// Wait for a result or state change.
    fn poll(&mut self) -> Result<(), ExitCode> {
        match self.recv() {
            Ok(batch) => {
                for result in batch {
                    match result {
                        WorkerResult::Entry(dir_entry) => {
                            if self.config.quiet {
                                return Err(ExitCode::HasResults(true));
                            }

                            match self.mode {
                                ReceiverMode::Buffering => {
                                    self.buffer.push(dir_entry);
                                    if self.buffer.len() > MAX_BUFFER_LENGTH {
                                        self.stream()?;
                                    }
                                }
                                ReceiverMode::Streaming => {
                                    self.print(&dir_entry)?;
                                }
                            }

                            self.num_results += 1;
                            if let Some(max_results) = self.config.max_results
                                && self.num_results >= max_results
                            {
                                return self.stop();
                            }
                        }
                        WorkerResult::Error(err) => {
                            if self.config.show_filesystem_errors {
                                print_error(err.to_string());
                            }
                        }
                    }
                }

                // If we don't have another batch ready, flush before waiting
                if self.mode == ReceiverMode::Streaming && self.rx.is_empty() {
                    self.flush()?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                self.stream()?;
            }
            Err(RecvTimeoutError::Disconnected) => {
                return self.stop();
            }
        }

        Ok(())
    }

    /// Output a path.
    fn print(&mut self, entry: &DirEntry) -> Result<(), ExitCode> {
        if let Err(e) = output::print_entry(&mut self.stdout, entry, self.config)
            && e.kind() != ::std::io::ErrorKind::BrokenPipe
        {
            print_error(format!("Could not write to output: {e}"));
            return Err(ExitCode::GeneralError);
        }

        if self.interrupt_flag.load(Ordering::Relaxed) {
            // Ignore any errors on flush, because we're about to exit anyway
            let _ = self.flush();
            return Err(ExitCode::KilledBySigint);
        }

        Ok(())
    }

    /// Switch ourselves into streaming mode.
    fn stream(&mut self) -> Result<(), ExitCode> {
        self.mode = ReceiverMode::Streaming;

        let buffer = mem::take(&mut self.buffer);
        for path in buffer {
            self.print(&path)?;
        }

        self.flush()
    }

    /// Stop looping.
    fn stop(&mut self) -> Result<(), ExitCode> {
        if self.mode == ReceiverMode::Buffering {
            self.buffer.sort();
            self.stream()?;
        }

        if self.config.quiet {
            Err(ExitCode::HasResults(self.num_results > 0))
        } else {
            Err(ExitCode::Success)
        }
    }

    /// Flush stdout if necessary.
    fn flush(&mut self) -> Result<(), ExitCode> {
        if self.stdout.flush().is_err() {
            // Probably a broken pipe. Exit gracefully.
            return Err(ExitCode::GeneralError);
        }
        Ok(())
    }
}

/// State shared by the sender and receiver threads.
struct WorkerState {
    /// The search patterns.
    patterns: Vec<Regex>,
    /// The command line configuration.
    config: Config,
    /// PLAN.md §Phase 2: cached scan-time CWD. Used by `DirEntry`
    /// constructors to canonicalize the relative paths the legacy
    /// `ignore::WalkBuilder` yields without paying an env::current_dir()
    /// syscall per hit. Captured once after `set_working_dir` runs (so
    /// it already reflects `--base-directory`).
    cwd: Arc<PathBuf>,
    /// Flag for cleanly shutting down the parallel walk
    quit_flag: Arc<AtomicBool>,
    /// Flag specifically for quitting due to ^C
    interrupt_flag: Arc<AtomicBool>,
    /// PLAN §Phase 8.7.1 MUST 4: cancellation signal shared with every
    /// `SearchBackend::run` invocation. The Ctrl-C handler clones and
    /// fires it alongside `quit_flag` so the EverythingBackend's
    /// between-hit `is_cancelled()` check and the `PostFilterSink`
    /// short-circuit both see the signal — without this bridge,
    /// Ctrl-C only reached LegacyWalker (which polls `quit_flag`).
    cancel: CancellationToken,
}

impl WorkerState {
    fn new(patterns: Vec<Regex>, config: Config) -> Result<Self> {
        let quit_flag = Arc::new(AtomicBool::new(false));
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let cancel = CancellationToken::new();
        // env::current_dir() once per scan, shared via Arc — every hit
        // would otherwise pay this syscall in DirEntry::normal.
        let cwd = Arc::new(std::env::current_dir().map_err(|e| {
            anyhow!(
                "Could not determine current directory \
                 (required to canonicalize hit paths). {e}"
            )
        })?);

        Ok(Self {
            patterns,
            config,
            cwd,
            quit_flag,
            interrupt_flag,
            cancel,
        })
    }

    fn build_overrides(&self, paths: &[PathBuf]) -> Result<Override> {
        let first_path = &paths[0];
        let config = &self.config;

        let mut builder = OverrideBuilder::new(first_path);

        for pattern in &config.exclude_patterns {
            builder
                .add(pattern)
                .map_err(|e| anyhow!("Malformed exclude pattern: {}", e))?;
        }

        builder
            .build()
            .map_err(|_| anyhow!("Mismatch in exclude patterns"))
    }

    fn build_walker(&self, paths: &[PathBuf]) -> Result<WalkParallel> {
        let first_path = &paths[0];
        let config = &self.config;
        let overrides = self.build_overrides(paths)?;

        let mut builder = WalkBuilder::new(first_path);
        builder
            .hidden(config.ignore_hidden)
            .ignore(config.read_fdignore)
            .parents(config.read_parent_ignore && (config.read_fdignore || config.read_vcsignore))
            .git_ignore(config.read_vcsignore)
            .git_global(config.read_vcsignore)
            .git_exclude(config.read_vcsignore)
            .require_git(config.require_git_to_read_vcsignore)
            .overrides(overrides)
            .follow_links(config.follow_links)
            // No need to check for supported platforms, option is unavailable on unsupported ones
            .same_file_system(config.one_file_system)
            .max_depth(config.max_depth);

        if config.read_fdignore {
            builder.add_custom_ignore_filename(".fdignore");
        }

        if config.read_global_ignore
            && let Ok(basedirs) = etcetera::choose_base_strategy()
        {
            let global_ignore_file = basedirs.config_dir().join("fd").join("ignore");
            if global_ignore_file.is_file() {
                let result = builder.add_ignore(global_ignore_file);
                match result {
                    Some(ignore::Error::Partial(_)) => (),
                    Some(err) => {
                        print_error(format!("Malformed pattern in global ignore file. {err}."));
                    }
                    None => (),
                }
            }
        }

        for ignore_file in &config.ignore_files {
            let result = builder.add_ignore(ignore_file);
            match result {
                Some(ignore::Error::Partial(_)) => (),
                Some(err) => {
                    print_error(format!("Malformed pattern in custom ignore file. {err}."));
                }
                None => (),
            }
        }

        for path in &paths[1..] {
            builder.add(path);
        }

        let walker = builder.threads(config.threads).build_parallel();
        Ok(walker)
    }

    /// Run the receiver work, either on this thread or a pool of background
    /// threads (for --exec).
    fn receive(&self, rx: Receiver<Batch>) -> ExitCode {
        let config = &self.config;

        // This will be set to `Some` if the `--exec` argument was supplied.
        if let Some(ref cmd) = config.command {
            if cmd.in_batch_mode() {
                exec::batch(rx.into_iter().flatten(), cmd, config)
            } else {
                thread::scope(|scope| {
                    // Each spawned job will store its thread handle in here.
                    let threads = config.threads;
                    let mut handles = Vec::with_capacity(threads);
                    for _ in 0..threads {
                        let rx = rx.clone();

                        // Spawn a job thread that will listen for and execute inputs.
                        let handle =
                            scope.spawn(|| exec::job(rx.into_iter().flatten(), cmd, config));

                        // Push the handle of the spawned thread into the vector for later joining.
                        handles.push(handle);
                    }
                    let exit_codes = handles.into_iter().map(|handle| handle.join().unwrap());
                    merge_exitcodes(exit_codes)
                })
            }
        } else {
            let stdout = io::stdout().lock();
            let stdout = io::BufWriter::new(stdout);

            ReceiverBuffer::new(self, rx, stdout).process()
        }
    }

    /// Spawn the sender threads.
    fn spawn_senders(&self, walker: WalkParallel, tx: Sender<Batch>) {
        walker.run(|| {
            let patterns = &self.patterns;
            let config = &self.config;
            let cwd: &Path = self.cwd.as_ref();
            let quit_flag = self.quit_flag.as_ref();

            let mut limit = 0x100;
            if let Some(cmd) = &config.command
                && !cmd.in_batch_mode()
                && config.threads > 1
            {
                // Evenly distribute work between multiple receivers
                limit = 1;
            }
            // PLAN.md §Phase 7: streaming contract.
            //
            // For the LegacyWalker path, per-result `--exec` is achieved by
            // pinning the BatchSender chunk to 1 above. When Phase 5 wires
            // EverythingBackend into walk, the post-filter orchestrator MUST
            // mirror this by constructing the IgnoreCache `AdaptiveDriver`
            // with `ChunkPolicy { force_unit: !cmd.in_batch_mode(), .. }` —
            // otherwise §4.8 will batch hits and `--exec` fires later than
            // fd does. The contract is pinned by
            // `scan::post_filter::ignore_cache::parallel::tests::force_unit_holds_chunk_at_one`.
            let mut tx = BatchSender::new(tx.clone(), limit);

            Box::new(move |entry| {
                if quit_flag.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }

                if let Ok(e) = &entry {
                    // If the entry is a directory that contains a
                    // "ignore contain" file", we want to skip this
                    // directory.
                    // Check the filetype first to avoid unnecessary
                    // syscalls.
                    if e.file_type().is_some_and(|t| t.is_dir()) {
                        let entry_path = e.path();
                        if config
                            .ignore_contain
                            .iter()
                            .any(|ic| entry_path.join(ic).exists())
                        {
                            return WalkState::Skip;
                        }
                    }
                    if e.depth() == 0 {
                        // Skip the root directory entry.
                        return WalkState::Continue;
                    }
                }
                let entry = match entry {
                    Ok(e) => DirEntry::normal(e, cwd),
                    Err(ignore::Error::WithPath {
                        path,
                        err: inner_err,
                    }) if inner_err
                        .io_error()
                        .is_some_and(|io_error| io_error.kind() == io::ErrorKind::NotFound)
                        && path
                            .symlink_metadata()
                            .ok()
                            .is_some_and(|m| m.file_type().is_symlink()) =>
                    {
                        DirEntry::broken_symlink(path, cwd)
                    }
                    Err(err) => {
                        return match tx.send(WorkerResult::Error(err)) {
                            Ok(_) => WalkState::Continue,
                            Err(_) => WalkState::Quit,
                        };
                    }
                };

                if let Some(min_depth) = config.min_depth
                    && entry.depth().is_none_or(|d| d < min_depth)
                {
                    return WalkState::Continue;
                }

                // Check the name first, since it doesn't require metadata
                let entry_path = entry.path();

                let search_str = search_str_for_entry(entry_path, config.full_path_base.as_deref());

                if !patterns
                    .iter()
                    .all(|pat| pat.is_match(&filesystem::osstr_to_bytes(search_str.as_ref())))
                {
                    return WalkState::Continue;
                }

                // Filter out unwanted extensions.
                if let Some(ref exts_regex) = config.extensions {
                    if let Some(path_str) = entry_path.file_name() {
                        if !exts_regex.is_match(&filesystem::osstr_to_bytes(path_str)) {
                            return WalkState::Continue;
                        }
                    } else {
                        return WalkState::Continue;
                    }
                }

                // Filter out unwanted file types.
                if let Some(ref file_types) = config.file_types
                    && file_types.should_ignore(&entry)
                {
                    return WalkState::Continue;
                }

                #[cfg(unix)]
                {
                    if let Some(ref owner_constraint) = config.owner_constraint {
                        if let Some(metadata) = entry.metadata() {
                            if !owner_constraint.matches(metadata) {
                                return WalkState::Continue;
                            }
                        } else {
                            return WalkState::Continue;
                        }
                    }
                }

                // Filter out unwanted sizes if it is a file and we have been given size constraints.
                if !config.size_constraints.is_empty() {
                    if entry_path.is_file() {
                        if let Some(metadata) = entry.metadata() {
                            let file_size = metadata.len();
                            if config
                                .size_constraints
                                .iter()
                                .any(|sc| !sc.is_within(file_size))
                            {
                                return WalkState::Continue;
                            }
                        } else {
                            return WalkState::Continue;
                        }
                    } else {
                        return WalkState::Continue;
                    }
                }

                // Filter out unwanted modification times
                if !config.time_constraints.is_empty() {
                    let mut matched = false;
                    if let Some(metadata) = entry.metadata()
                        && let Ok(modified) = metadata.modified()
                    {
                        matched = config
                            .time_constraints
                            .iter()
                            .all(|tf| tf.applies_to(&modified));
                    }
                    if !matched {
                        return WalkState::Continue;
                    }
                }

                if config.is_printing()
                    && let Some(ls_colors) = &config.ls_colors
                {
                    // Compute colors in parallel
                    entry.style(ls_colors);
                }

                let send_result = tx.send(WorkerResult::Entry(entry));

                if send_result.is_err() {
                    return WalkState::Quit;
                }

                // Apply pruning.
                if config.prune {
                    return WalkState::Skip;
                }

                WalkState::Continue
            })
        });
    }

    /// Perform the recursive scan.
    ///
    /// PLAN.md §Phase 8.5-D: routes each search root through
    /// [`select_backend`] before traversal. Roots that land on a supported
    /// Everything index drive the `EverythingBackend` → `PostFilterSink` →
    /// `RawHitBatchSink` chain; roots that fall back (`--filesystem-walker`,
    /// unsupported query, or runtime IPC failure) flow through the legacy
    /// `ignore::WalkBuilder` exactly as before. Both producers share a
    /// single `BatchSender`-fed channel so [`ReceiverBuffer`] is unaware
    /// of which backend produced any given hit.
    fn scan(&self, paths: &[PathBuf]) -> Result<ExitCode> {
        let config = &self.config;

        // PLAN §Phase 8.7.1 MUST 4: bridge Ctrl-C to the backend
        // CancellationToken. Without this clone the EverythingBackend
        // keeps polling the SDK until the query returns naturally; the
        // cooperative `is_cancelled()` check in `backend.rs` then
        // abandons the result list on the very next hit. The hidden
        // message window (PLAN §Phase 5 B.4) would let
        // `Everything_QueryW(TRUE)` itself be interrupted mid-flight;
        // until that lands, between-hit cancellation is the strongest
        // signal we have.
        //
        // Installed unconditionally — earlier this was gated on
        // `ls_colors.is_some() && is_printing()`, which meant `--exec`,
        // piped output and `--color=never` left the backend without a
        // cancel signal (MUST 4 regression).
        {
            let quit_flag = Arc::clone(&self.quit_flag);
            let interrupt_flag = Arc::clone(&self.interrupt_flag);
            let cancel = self.cancel.clone();

            ctrlc::set_handler(move || {
                quit_flag.store(true, Ordering::Relaxed);
                cancel.cancel();

                if interrupt_flag.fetch_or(true, Ordering::Relaxed) {
                    // Ctrl-C has been pressed twice, exit NOW
                    ExitCode::KilledBySigint.exit();
                }
            })
            .unwrap();
        }

        let (tx, rx) = bounded(2 * config.threads);

        let exit_code = thread::scope(|scope| {
            // Spawn the receiver thread(s) first so backend drivers can
            // push to the bounded channel without blocking forever.
            let receiver = scope.spawn(|| self.receive(rx));

            // Decide each root's backend up-front. The Everything path
            // can still demote a root to LegacyWalker at runtime via IPC
            // fallback (see `drive_everything_root`); that demotion is
            // collected into `fallback_paths` and folded back into the
            // legacy walker's path set below.
            let (legacy_paths, fallback_paths) = self.run_everything_paths(paths, tx.clone());
            let mut legacy_paths = legacy_paths;
            legacy_paths.extend(fallback_paths);

            if legacy_paths.is_empty() {
                // No legacy roots: dropping `tx` lets the receiver finish
                // immediately after the Everything stream completes.
                drop(tx);
            } else {
                // Legacy walker covers all remaining roots in a single
                // `WalkParallel` pass, matching pre-Phase-8.5 behaviour
                // when no Everything routing happens (force_legacy=true
                // or every root unindexed).
                match self.build_walker(&legacy_paths) {
                    Ok(walker) => self.spawn_senders(walker, tx),
                    Err(err) => {
                        // build_walker only fails on malformed --exclude
                        // patterns; surface as a worker error so the
                        // receiver still drains cleanly.
                        let mut sender = BatchSender::new(tx, 1);
                        let _ = sender.send(WorkerResult::Error(ignore::Error::from(
                            std::io::Error::other(format!("{err:#}")),
                        )));
                    }
                }
            }

            receiver.join().unwrap()
        });

        if self.interrupt_flag.load(Ordering::Relaxed) {
            Ok(ExitCode::KilledBySigint)
        } else {
            Ok(exit_code)
        }
    }

    /// PLAN.md §Phase 8.5-D dispatcher: classify each user-supplied path,
    /// drive every Everything-routed root sequentially through the SDK
    /// mutex, and partition out (a) the paths the user already asked to
    /// run on LegacyWalker (`--filesystem-walker`, unsupported query, or
    /// the path is on a non-indexed volume) and (b) the paths whose
    /// Everything invocation failed with `EverythingError::is_unavailable`
    /// (silent fallback per PLAN R1).
    ///
    /// Returns `(initial_legacy_paths, runtime_fallback_paths)`. Both
    /// vectors are unioned by the caller before invoking the legacy
    /// walker so a single `WalkParallel` pass covers them.
    fn run_everything_paths(
        &self,
        paths: &[PathBuf],
        tx: Sender<Batch>,
    ) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let config = &self.config;

        // PLAN.md §Phase 8.7.1 MUST 3 retired the `FDE_BACKEND=everything`
        // opt-in gate, and §Phase 8.8 then flipped the per-root probe
        // default to off. So routing now goes: every root drives
        // `EverythingBackend` directly unless `--filesystem-walker` is
        // set, the translation gives up, or `--probe` is on and the
        // count-only probe reports zero hits. The semantic parity gaps
        // that originally kept the §8.7 gate (directory-symlink type,
        // hidden-by-name ancestors, size-filter on reparse points) were
        // closed by §8.7.1 MUST 1 + MUST 2.
        //
        // Test-only escape hatch: `FDE_TEST_FORCE_LEGACY=1` shorts the
        // Everything path entirely, mirroring `--filesystem-walker`.
        // The integration harness in `tests/testenv` sets it on every
        // invocation so `cargo test --test tests` stays deterministic
        // on dev machines where Everything has cached the tempdir
        // parent but not the freshly-created leaf entries. NOT consumed
        // anywhere production; without it MUST 3 would trade an opt-in
        // gate for an opt-out flake.
        if std::env::var_os("FDE_TEST_FORCE_LEGACY").is_some() {
            return (paths.to_vec(), Vec::new());
        }

        // PLAN.md §Phase 8.6 / §Phase 8 §8g: when the test harness sets
        // `FDE_TEST_MOCK_HITS`, swap `EverythingBackend` for a
        // deterministic text-file replay so `tests/mock_e2e.rs` can
        // verify the dispatcher and pipeline wiring without depending on
        // a running Everything service. A parse error surfaces as a
        // worker error and falls every root back to LegacyWalker (so
        // bad fixtures don't silently zero results).
        let mock_backend = match mock_injection::try_load_from_env() {
            Ok(m) => m,
            Err(err) => {
                let mut sender = BatchSender::new(tx.clone(), 1);
                let _ = sender.send(WorkerResult::Error(ignore::Error::from(
                    std::io::Error::other(format!("{err:#}")),
                )));
                return (paths.to_vec(), Vec::new());
            }
        };

        // Pick a probe. The real `EverythingVolumeIndexProbe` issues a
        // count-only `path:"<root>"` query per root and routes to Legacy
        // on either an IPC error or a zero-count response — so a tempdir
        // that Everything hasn't seen yet falls through to the legacy
        // walker and still produces results.
        //
        // Default flipped: `AssumeIndexedProbe` skips the per-root IPC
        // and trusts every root is indexed. Empirically Everything's USN
        // indexer catches up within seconds, so by the time a user types
        // `fde` the file is in the index; paying an IPC per root every
        // run to guard a sub-second race is a bad trade. The escape
        // hatches stay: `--probe` re-enables the real probe, and
        // `--filesystem-walker` skips Everything entirely.
        //
        // Mock-injection sessions force `AssumeIndexedProbe`: the test
        // harness uses tempdirs that the real probe would correctly
        // route to Legacy, which would skip the mock backend entirely
        // and defeat the whole point of `FDE_TEST_MOCK_HITS`. Per-test
        // env isolation in `tests/mock_e2e.rs` keeps this from leaking
        // into production binaries.
        let assume_probe = AssumeIndexedProbe;
        let real_probe = EverythingVolumeIndexProbe;
        let probe: &dyn VolumeIndexProbe = if mock_backend.is_some() || !config.probe {
            &assume_probe
        } else {
            &real_probe
        };

        // Build TranslationInput from Config. The lifetime ties to
        // `config`, which lives for the entire scan, so this is safe to
        // pass to `select_backend` for every root.
        let translation_input = self.translation_input();

        let mut initial_legacy: Vec<PathBuf> = Vec::new();
        let mut runtime_fallback: Vec<PathBuf> = Vec::new();

        for (idx, path) in paths.iter().enumerate() {
            if self.quit_flag.load(Ordering::Relaxed) {
                break;
            }

            // The logical absolute root retains the user's symlink spelling.
            // For a reparse-point root, query Everything at the resolved
            // physical target and rebase each hit before post-filtering.
            let logical_root = config
                .search_roots
                .get(idx)
                .map(|sr| sr.canonical.clone())
                .unwrap_or_else(|| path.clone());

            let root_mapping = match resolve_reparse_query_root(&logical_root) {
                Ok(Some(query_root)) => {
                    // Everything applies full-path patterns before hits reach
                    // our rebase adapter. Rewriting an arbitrary regex/glob
                    // from the logical prefix to the physical prefix is not
                    // semantics-preserving, so keep upstream fd behavior.
                    if config.full_path_base.is_some() {
                        initial_legacy.push(path.clone());
                        continue;
                    }
                    Some(RootMapping {
                        query_root,
                        logical_root: Arc::new(logical_root.clone()),
                    })
                }
                Ok(None) => None,
                Err(_) => {
                    initial_legacy.push(path.clone());
                    continue;
                }
            };

            let backend_root = root_mapping
                .as_ref()
                .map(|mapping| mapping.query_root.clone())
                .unwrap_or(logical_root);

            let choice = self.classify_backend(&translation_input, backend_root, probe);
            match choice {
                BackendChoice::Legacy { .. } => initial_legacy.push(path.clone()),
                BackendChoice::Everything(query) => {
                    // Mock injection (PLAN §8g) wins over EverythingBackend
                    // when both are eligible — that's the whole point of
                    // the env var. force_legacy is still honoured because
                    // `classify_backend` would have returned Legacy before
                    // we got here.
                    let outcome = match mock_backend.as_ref() {
                        Some(mock) => self.drive_backend_root(
                            mock,
                            &query,
                            path,
                            root_mapping.as_ref(),
                            tx.clone(),
                        ),
                        None => self.drive_backend_root(
                            &EverythingBackend::new(),
                            &query,
                            path,
                            root_mapping.as_ref(),
                            tx.clone(),
                        ),
                    };
                    match outcome {
                        EverythingOutcome::Done => {}
                        EverythingOutcome::Fallback => runtime_fallback.push(path.clone()),
                        EverythingOutcome::HardError(err) => {
                            // Surface the structured error to the receiver
                            // — it'll be printed under `--show-errors`,
                            // and exit status remains non-zero via the
                            // normal `WorkerResult::Error` channel.
                            let mut sender = BatchSender::new(tx.clone(), 1);
                            let _ = sender.send(WorkerResult::Error(ignore::Error::from(
                                std::io::Error::other(err),
                            )));
                        }
                    }
                }
            }
        }

        (initial_legacy, runtime_fallback)
    }

    /// Run a [`SearchBackend`] (production: [`EverythingBackend`];
    /// `tests/mock_e2e.rs` swap-in: [`MockHitFileBackend`]) for one
    /// search root. Threads hits through a Phase 3 `Pipeline` built
    /// from `Config` (`pipeline_builder::build_pipeline`) into a
    /// `RawHitBatchSink`, which writes `WorkerResult::Entry(DirEntry)`
    /// to the shared channel. Returns:
    ///
    /// - `Done` — query completed (possibly cancelled mid-stream by
    ///   `--max-results` or downstream SIGPIPE; both are fine — the
    ///   pipeline already forwarded as many hits as the user wanted).
    /// - `Fallback` — Everything itself is unreachable (IPC error). The
    ///   caller re-runs the root on the legacy walker. This branch is
    ///   never taken by [`MockHitFileBackend`] (it doesn't surface
    ///   `EverythingError`).
    /// - `HardError` — backend is reachable but a non-cancel, non-IPC
    ///   error fired (programmer bug, malformed mock fixture, or
    ///   pattern build failure).
    fn drive_backend_root(
        &self,
        backend: &dyn SearchBackend,
        query: &BackendQuery,
        current_root: &Path,
        root_mapping: Option<&RootMapping>,
        tx: Sender<Batch>,
    ) -> EverythingOutcome {
        let config = &self.config;
        // PLAN §Phase 8.7.1 MUST 4: reuse the shared cancel token so
        // the Ctrl-C handler installed in `scan` can interrupt the
        // running backend. A fresh token here would silently swallow
        // every cancellation signal — backend keeps streaming until
        // SDK exhaustion.
        let cancel = self.cancel.clone();

        // `--exec` per-result mode needs chunk=1 so jobs see hits as
        // they arrive; everything else gets the default chunk. Matches
        // `spawn_senders` (lines 379-386) for the legacy path.
        let limit = match config.command.as_ref() {
            Some(cmd) if !cmd.in_batch_mode() && config.threads > 1 => 1,
            _ => 0x100,
        };
        let batch_sender = BatchSender::new(tx, limit);

        // Anchor the pipeline to THIS root, not `paths.first()`. With
        // multiple search roots and a backend-routed scan, sharing the
        // first root's anchor across every root would make
        // `ExcludeFilter`'s `OverrideBuilder` match `-E` patterns
        // against the wrong base — e.g. `fde -E '*.tmp' rootA rootB`
        // would leak `rootB\a.tmp` on the Everything path while the
        // legacy walker drops it.
        let pipeline = match build_pipeline(config, current_root, &cancel) {
            Ok(p) => p,
            Err(err) => return EverythingOutcome::HardError(format!("{err:#}")),
        };

        let mut downstream = RawHitBatchSink::new(batch_sender);
        let mut sink = PostFilterSink::new(pipeline, &mut downstream, &cancel);

        let result = match root_mapping {
            Some(mapping) => {
                let mut rebase_sink = RootRebaseSink::new(
                    &mut sink,
                    &mapping.query_root,
                    Arc::clone(&mapping.logical_root),
                );
                backend.run(query, &mut rebase_sink, &cancel)
            }
            None => backend.run(query, &mut sink, &cancel),
        };

        // Drain any buffered hits from `prune` etc. — but only when the
        // backend completed cleanly or was cancelled. A hard backend
        // error means the result set is partial / corrupted, and
        // emitting buffered hits would mix partial success with the
        // error report; LegacyWalker would have produced nothing in
        // that case. Cancelled still finalizes so anything queued up to
        // the cancel boundary is forwarded.
        match &result {
            Ok(()) | Err(BackendError::Cancelled) => {
                if let Err(err) = sink.finalize() {
                    // finalize() only fails on Cancelled; that's not a fault.
                    debug_assert!(matches!(err, BackendError::Cancelled));
                }
            }
            Err(BackendError::Other(_)) => {
                // Drop buffered hits without flushing.
            }
        }

        match result {
            Ok(()) | Err(BackendError::Cancelled) => EverythingOutcome::Done,
            Err(BackendError::Other(err)) => {
                if let Some(ev) = err.downcast_ref::<EverythingError>()
                    && ev.is_unavailable()
                {
                    EverythingOutcome::Fallback
                } else {
                    EverythingOutcome::HardError(format!("{err:#}"))
                }
            }
        }
    }

    fn classify_backend(
        &self,
        input: &TranslationInput<'_>,
        canonical: PathBuf,
        probe: &dyn VolumeIndexProbe,
    ) -> BackendChoice {
        let mut choices = select_backend(vec![canonical], input, probe, self.config.force_legacy);
        // `select_backend` always returns one choice per input path.
        choices
            .pop()
            .expect("select_backend yields one choice per input")
    }

    fn translation_input(&self) -> TranslationInput<'_> {
        let config = &self.config;
        TranslationInput {
            pattern: &config.raw_pattern,
            glob: config.pattern_is_glob,
            fixed_strings: config.pattern_is_fixed_strings,
            exact: config.pattern_is_exact,
            and_patterns: &config.raw_and_patterns,
            full_path: config.full_path_base.is_some(),
            case_sensitive: config.case_sensitive,
            file_types: config.file_types.as_ref(),
            // Extensions / size / time hints are CURRENTLY consumed only
            // for backend selection (whether a query is translatable).
            // For 8.5 we err on "translation must succeed" — the existing
            // tests/tests.rs assertions cover the post-filter side, and
            // every CLI-level filter is re-applied in `build_pipeline`
            // regardless of what gets pushed down. Wiring the actual
            // push-down values is Phase 5+ optimisation territory.
            extensions: &[],
            size_filters: &config.size_constraints,
            time_filters: &config.time_constraints,
            max_depth: config.max_depth,
            max_results: config.max_results,
        }
    }
}

/// Outcome of one EverythingBackend invocation on a single search root.
enum EverythingOutcome {
    /// Query produced hits (possibly cancelled mid-stream, which is OK).
    Done,
    /// Everything itself is unavailable (`EVERYTHING_ERROR_IPC` family).
    /// PLAN R1 silent-fallback contract: caller re-runs this root via
    /// LegacyWalker.
    Fallback,
    /// Non-recoverable error — surfaces to the receiver as a
    /// `WorkerResult::Error`.
    HardError(String),
}

#[cfg(target_os = "windows")]
struct RootMapping {
    query_root: PathBuf,
    logical_root: Arc<PathBuf>,
}

#[cfg(target_os = "windows")]
fn resolve_reparse_query_root(path: &Path) -> io::Result<Option<PathBuf>> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let metadata = path.symlink_metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        return Ok(None);
    }
    path.canonicalize().map(strip_verbatim_prefix).map(Some)
}

#[cfg(target_os = "windows")]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

fn search_str_for_entry<'a>(
    entry_path: &'a std::path::Path,
    full_path_base: Option<&std::path::Path>,
) -> Cow<'a, OsStr> {
    if let Some(cwd) = full_path_base {
        // If full_path_base is some, that means that we need to return
        // the absolute path
        if entry_path.is_absolute() {
            return Cow::Borrowed(entry_path.as_os_str());
        }
        let path = entry_path.strip_prefix(".").unwrap_or(entry_path);
        Cow::Owned(cwd.join(path).into())
    } else {
        match entry_path.file_name() {
            Some(filename) => Cow::Borrowed(filename),
            None => unreachable!(
                "Encountered file system entry without a file name. This should only \
                 happen for paths like 'foo/bar/..' or '/' which are not supposed to \
                 appear in a file system traversal."
            ),
        }
    }
}

/// Recursively scan the given search path for files / pathnames matching the patterns.
///
/// If the `--exec` argument was supplied, this will create a thread pool for executing
/// jobs in parallel from a given command line and the discovered paths. Otherwise, each
/// path will simply be written to standard output.
pub fn scan(paths: &[PathBuf], patterns: Vec<Regex>, config: Config) -> Result<ExitCode> {
    WorkerState::new(patterns, config)?.scan(paths)
}

#[cfg(test)]
mod tests {
    use super::search_str_for_entry;
    #[cfg(target_os = "windows")]
    use super::{resolve_reparse_query_root, strip_verbatim_prefix};
    use std::path::{Path, PathBuf};

    #[test]
    fn search_str_for_entry_with_relative_path() {
        let full_path_base = Some(Path::new("/home/user"));
        assert_eq!(
            search_str_for_entry(Path::new("foo/bar"), full_path_base),
            PathBuf::from("/home/user/foo/bar")
        );
    }

    #[test]
    fn search_str_for_entry_strips_dot_prefix() {
        let full_path_base = Some(Path::new("/home/user"));
        assert_eq!(
            search_str_for_entry(Path::new("./foo/bar"), full_path_base),
            PathBuf::from("/home/user/foo/bar")
        );
    }

    #[test]
    fn search_str_for_entry_with_absolute_path() {
        let full_path_base = Some(Path::new("/home/user"));
        assert_eq!(
            search_str_for_entry(Path::new("/absolute/path"), full_path_base),
            PathBuf::from("/absolute/path")
        );
    }

    #[test]
    fn search_str_no_base_dir() {
        assert_eq!(
            search_str_for_entry(Path::new("./foo/bar"), None),
            PathBuf::from("bar")
        );
    }

    #[test]
    fn search_str_no_base_dir_with_plain_relative_path() {
        assert_eq!(
            search_str_for_entry(Path::new("foo/bar"), None),
            PathBuf::from("bar")
        );
    }

    #[test]
    fn search_str_no_base_dir_with_file_in_current_dir() {
        assert_eq!(
            search_str_for_entry(Path::new("foo"), None),
            PathBuf::from("foo")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reparse_point_search_root_resolves_to_query_target() {
        use std::fs;
        use std::os::windows::fs::symlink_dir;

        let temp = tempfile::tempdir().expect("create temp directory");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::create_dir(&target).expect("create symlink target");
        symlink_dir(&target, &link).expect("create directory symlink");

        assert_eq!(resolve_reparse_query_root(&target).unwrap(), None);
        let resolved = resolve_reparse_query_root(&link)
            .unwrap()
            .expect("directory symlink must resolve");
        // Runner temp directories can themselves contain path aliases, so
        // compare the canonical forms on both sides.
        let expected = strip_verbatim_prefix(
            target
                .canonicalize()
                .expect("canonicalize directory symlink target"),
        );
        assert!(crate::filesystem::paths_equal(&resolved, &expected));
    }
}
