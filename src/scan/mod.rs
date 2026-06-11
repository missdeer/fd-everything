//! Search-backend abstraction introduced by PLAN.md Phase 1.
//!
//! This module is the future home of the EverythingBackend, the
//! LegacyWalkerBackend wrapper, the post-filter pipeline (Phase 3), the
//! IgnoreCache (Phase 4) and the PathProjector (Phase 2). At Phase 1 only the
//! backend trait surface and an in-memory `MockBackend` exist so that later
//! phases can be developed against a deterministic hit stream via TDD without
//! depending on either the legacy `ignore::WalkBuilder` or the Everything SDK.

// Phase 1 scaffolding: types are wired up by Phase 3 (post-filter) and
// Phase 5 (FFI). Until then they appear "unused" to the binary build but are
// exercised by `#[cfg(test)]` and consumed by later phases.
#[allow(dead_code)]
pub mod backend;

/// Phase 2 §Phase 2: shared sink types (`Batch`, `BatchSender`, `WorkerResult`)
/// promoted out of `walk.rs` so the post-filter pipeline (Phase 3) and the
/// EverythingBackend (Phase 5) can produce into the same channel shape that
/// `walk::WorkerState::receive` consumes.
pub mod sink;

/// Phase 2 PathProjector: turns an absolute, canonicalized `raw_path` into the
/// fd-compatible display form for `output.rs` and `--exec` placeholder
/// substitution. See PLAN.md §2.X consumer table.
pub mod path_projection;

/// Phase 3 post-filter pipeline: consumes `RawHit` streams from a
/// `SearchBackend` and applies the filter chain previously baked into
/// `walk.rs`. Wired up in Phase 5; carries `dead_code` until then.
#[allow(dead_code)]
pub mod post_filter;

/// PLAN.md §Phase 8.5-B: adapter that bridges a `SearchBackend`'s
/// `RawHit` stream to the `BatchSender<WorkerResult>` channel that
/// `walk::WorkerState::receive` already consumes. Wired by Phase 8.5-D
/// inside `walk::scan`.
pub mod sink_adapter;

/// PLAN.md §Phase 8.5-C: assemble a post-filter `Pipeline` from `&Config`
/// with the §0 P6 "fast-path skip" principle (filters not driven by any
/// CLI dimension never enter the chain).
pub mod pipeline_builder;

/// PLAN.md §Phase 8.6 / Phase 8 §8g: `FDE_TEST_MOCK_HITS` injection so the
/// `tests/mock_e2e.rs` end-to-end suite can replace `EverythingBackend`
/// with a deterministic hit stream parsed from a text file.
pub mod mock_injection;

/// PLAN.md §Phase 8 §1 "MockBackend 集成测试": end-to-end composition
/// tests that stage `RawHit` fixtures through `MockBackend` ->
/// `PostFilterSink` -> a collecting downstream sink, exercising filter
/// combinations that the per-filter unit tests can't reach. The two
/// pre-existing tests in `post_filter/mod.rs` cover the minimal happy
/// path and the `max_results` cancel contract; this module adds the
/// breadth coverage Phase 8 calls for (depth-extension-type combos,
/// prune buffering across drain, dotfile + ignore_cache layering,
/// multi-root depth attribution, and SIGPIPE-equivalent cancellation
/// propagation). Kept inside `src/scan/` rather than as a top-level
/// `tests/mock_backend.rs` integration test because the crate is a pure
/// binary target — Phase 8 deliberately does not add a `lib.rs`.
#[cfg(test)]
mod mock_integration_tests;
