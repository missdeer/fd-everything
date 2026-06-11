//! Adaptive chunked parallel IgnoreCache evaluation (PLAN.md §4.8).
//!
//! Sequential `IgnoreCache::matched` is correct but stat-bound on cold trees;
//! parallelizing across hits hides that latency. The constraint (C1, derived
//! from PLAN.md §0) is that arrival order is preserved AND the first hit's
//! latency must not regress vs the legacy walker — so a naive
//! `into_par_iter().chunks(64)` is the wrong shape (it would wait for 64
//! hits to arrive before producing any output).
//!
//! ## Strategy
//!
//! - Driver thread reads hits one at a time from upstream.
//! - Chunk size starts at 1, doubles on each completed batch up to
//!   `FDE_MAX_CHUNK` (default 256).
//! - Deadline timer (`FDE_FLUSH_DEADLINE_MS`, default 5 ms) forces a flush
//!   when hits stop arriving — prevents tail latency in low-rate streams.
//! - `exec_mode == PerResult` forces chunk size = 1 so `--exec` per-result
//!   commands fire on the first hit without batching delay.
//! - Within a chunk, rayon's `into_par_iter` parallelizes; results land in
//!   a `Vec<(usize, Verdict)>` keyed by original arrival index so the
//!   driver emits in order.
//!
//! Used by the Phase 3 pipeline indirectly: when the pipeline contains an
//! `IgnoreCacheFilter`, the orchestrator wraps the pipeline in
//! `ParallelDriver` instead of calling `Pipeline::process` directly. For
//! Phase 4 the driver is exposed as a building block; Phase 5 wires it in
//! once the EverythingBackend stream exists.

use std::time::{Duration, Instant};

use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

use super::IgnoreCache;

/// Tunables (PLAN.md §4.8). Defaults from PLAN; env overrides for benchmark
/// runs.
#[derive(Debug, Clone, Copy)]
pub struct ChunkPolicy {
    pub max_chunk: usize,
    pub flush_deadline: Duration,
    /// Force `chunk = 1` regardless of growth (set when `--exec` per-result).
    pub force_unit: bool,
}

impl ChunkPolicy {
    pub fn from_env(force_unit: bool) -> Self {
        let max_chunk = std::env::var("FDE_MAX_CHUNK")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(256);
        let flush_ms = std::env::var("FDE_FLUSH_DEADLINE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5);
        Self {
            max_chunk,
            flush_deadline: Duration::from_millis(flush_ms),
            force_unit,
        }
    }
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self::from_env(false)
    }
}

/// A filter wrapper that evaluates `IgnoreCache::matched` across rayon
/// workers in adaptive chunks. Implements [`Filter`] so it slots into the
/// existing pipeline without touching `pipeline.rs` semantics.
///
/// **Limitation**: rayon parallelism requires a buffered driver. The plain
/// `Filter::evaluate` interface is one-hit-at-a-time and cannot batch on its
/// own. Phase 5 must drive this via the orchestrator (`drive`) instead of
/// the pipeline's per-hit `process`. The `Filter` impl below is therefore a
/// **sequential fallback** — correct but slow — used when the orchestrator
/// has not adopted the batch driver. The driver is the actual §4.8
/// implementation.
pub struct ParallelIgnoreFilter {
    cache: IgnoreCache,
}

impl ParallelIgnoreFilter {
    pub fn new(cache: IgnoreCache) -> Self {
        Self { cache }
    }
}

impl Filter for ParallelIgnoreFilter {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        // Sequential fallback. Phase 5's orchestrator should route hits
        // through `drive_chunk` for the actual parallel gain.
        match self.cache.matched(&hit.path, hit.is_dir, &hit.search_root) {
            super::Decision::Show => Verdict::Keep,
            super::Decision::Hide => Verdict::Drop,
        }
    }
}

/// Driver entry: evaluate one chunk of hits in parallel using rayon. Returns
/// a parallel `Vec<Verdict>` aligned with `hits`. Caller forwards on `Keep`,
/// drops on `Drop`. Order is preserved because `rayon::par_iter` keeps
/// indices stable when collected to `Vec`.
pub fn drive_chunk(cache: &IgnoreCache, hits: &[RawHit]) -> Vec<Verdict> {
    use rayon::prelude::*;
    hits.par_iter()
        .map(|h| match cache.matched(&h.path, h.is_dir, &h.search_root) {
            super::Decision::Show => Verdict::Keep,
            super::Decision::Hide => Verdict::Drop,
        })
        .collect()
}

/// Streaming driver state machine. Push hits one at a time via `push`; the
/// driver buffers up to `chunk_target` and either drives a batch when the
/// buffer fills or when `Instant::now() >= deadline`. Caller pulls completed
/// `(RawHit, Verdict)` pairs via `flush`.
pub struct AdaptiveDriver {
    cache: IgnoreCache,
    policy: ChunkPolicy,
    buf: Vec<RawHit>,
    chunk_target: usize,
    deadline: Instant,
}

impl AdaptiveDriver {
    pub fn new(cache: IgnoreCache, policy: ChunkPolicy) -> Self {
        let now = Instant::now();
        // Start at 1 regardless of `force_unit` — the difference is whether
        // the chunk grows after the first drain (see `drain_inner`).
        let chunk_target = 1;
        Self {
            cache,
            policy,
            buf: Vec::new(),
            chunk_target,
            deadline: now + policy.flush_deadline,
        }
    }

    /// Append a hit. Returns ready-to-emit verdicts when the buffer is
    /// ready to drain (either the chunk filled or the deadline passed).
    pub fn push(&mut self, hit: RawHit) -> Option<Vec<(RawHit, Verdict)>> {
        self.buf.push(hit);
        if self.buf.len() >= self.chunk_target || Instant::now() >= self.deadline {
            Some(self.drain_inner())
        } else {
            None
        }
    }

    /// Force-drain whatever is buffered. Called by the orchestrator after
    /// the backend finishes.
    pub fn drain(&mut self) -> Vec<(RawHit, Verdict)> {
        if self.buf.is_empty() {
            Vec::new()
        } else {
            self.drain_inner()
        }
    }

    fn drain_inner(&mut self) -> Vec<(RawHit, Verdict)> {
        let chunk = std::mem::take(&mut self.buf);
        let verdicts = drive_chunk(&self.cache, &chunk);
        let out = chunk.into_iter().zip(verdicts).collect();
        // Grow next chunk (×2) until cap; PerResult forces 1.
        if !self.policy.force_unit {
            self.chunk_target = (self.chunk_target * 2).min(self.policy.max_chunk);
        }
        self.deadline = Instant::now() + self.policy.flush_deadline;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::{GlobalLayers, IgnoreFlags};
    use super::*;
    use crate::scan::backend::CaseModifier;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn cache() -> IgnoreCache {
        IgnoreCache::new(
            IgnoreFlags {
                read_fdignore: false,
                read_vcsignore: false,
                ..IgnoreFlags::default()
            },
            GlobalLayers::empty(),
            Vec::new(),
            CaseModifier::Nocase,
        )
    }

    fn hit(name: &str) -> RawHit {
        RawHit {
            path: PathBuf::from(name),
            is_dir: false,
            size: 0,
            mtime: 0,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// `force_unit` (set when `--exec` per-result) MUST hold chunk_target at 1
    /// even after multiple drains, so each hit fires the exec immediately.
    /// If this regresses, `--exec` per-result batches and tail-latency
    /// regresses vs fd.
    #[test]
    fn force_unit_holds_chunk_at_one() {
        let policy = ChunkPolicy {
            max_chunk: 256,
            flush_deadline: Duration::from_millis(50),
            force_unit: true,
        };
        let mut driver = AdaptiveDriver::new(cache(), policy);
        for i in 0..5 {
            let result = driver.push(hit(&format!("/{i}")));
            assert!(result.is_some(), "force_unit must emit on every push");
            assert_eq!(result.unwrap().len(), 1);
        }
    }

    /// Adaptive growth: after each chunk drains, the target doubles, capped
    /// by max_chunk. If this regresses, the throughput win of §4.8
    /// disappears.
    #[test]
    fn chunk_target_grows_geometrically_after_drain() {
        let policy = ChunkPolicy {
            max_chunk: 4,
            flush_deadline: Duration::from_secs(60),
            force_unit: false,
        };
        let mut driver = AdaptiveDriver::new(cache(), policy);

        // First push (target=1) drains immediately.
        assert!(driver.push(hit("/a")).is_some());
        // Next target=2: one push, no drain.
        assert!(driver.push(hit("/b")).is_none());
        assert!(driver.push(hit("/c")).is_some());
        // Next target=4: three pushes without drain.
        assert!(driver.push(hit("/d")).is_none());
        assert!(driver.push(hit("/e")).is_none());
        assert!(driver.push(hit("/f")).is_none());
        assert!(driver.push(hit("/g")).is_some());
    }
}
