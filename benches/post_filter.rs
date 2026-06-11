//! PLAN.md §Phase 8 §6 / §8i: post-filter pipeline microbenchmarks.
//!
//! These benches drive a `MockBackend` through `PostFilterSink` into a
//! collecting sink — same construction as the unit / integration tests
//! that pin pipeline semantics — and report per-hit cost across several
//! representative filter configurations.
//!
//! ## What this measures
//!
//! - **`pipeline_baseline`** — empty filter chain. Establishes the
//!   per-hit overhead of just streaming RawHit -> sink (channel send,
//!   `Verdict::Forward`, `VecSink::send`). Any number above this is the
//!   filter chain's contribution.
//! - **`pipeline_extension_filter`** — long-literal-equivalent: a
//!   `--extension foo` matcher. Mirrors PLAN §Phase 8 §6's "long
//!   literal (≥5 byte)" bench dimension as the post-filter would see
//!   it after `EverythingBackend` push-down.
//! - **`pipeline_type_filter`** — short-literal-equivalent: `--type f`.
//!   Pure RawHit field comparison; bench of the §0 hot-path zero-syscall
//!   commitment.
//! - **`pipeline_full_chain`** — the realistic production stack:
//!   `--type f --extension foo --max-depth 5 --max-results unbounded`.
//!   Catches regressions in inter-filter overhead (Verdict::Drop
//!   short-circuit, etc.).
//!
//! ## What this DOES NOT measure
//!
//! - 200k synthetic tree end-to-end timing against a real Everything
//!   index. PLAN §Phase 8 §6 spec'd this with cold / hot cache
//!   separation. Per Phase 8.6 calibration, the "isolated Everything
//!   instance" precondition is not achievable — the SDK (1.4.1 is the
//!   current upstream-latest per voidtools) does not expose
//!   `Everything_SetInstanceName` or any IPC instance-name hook, so a
//!   bench cannot scope its query to a synthetic fixture tree without
//!   reading the user's actual index. To approximate the post-filter
//!   side of that bench, extend `SIZES` below (this harness scales
//!   linearly with hit count); the 1k / 10k pair is enough to surface
//!   §0 hot-path regressions on its own. End-to-end Everything timing
//!   against a 200k fixture stays out of `cargo bench` for the same
//!   reason it stays out of `tests/real_everything.rs`: any run is
//!   indistinguishable from a query that walked the user's full index.
//! - IgnoreCache parent-chain walks. That's a Phase 4 internal bench
//!   territory — the pipeline bench would be dominated by filesystem
//!   stat costs that aren't representative of the §0 hot-path budget.
//! - `--prune` buffering tax. PruneFilter is a buffering filter; its
//!   cost shape is fundamentally different (drain-time, not per-hit)
//!   and warrants a dedicated bench, also deferred.

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use regex::bytes::RegexSetBuilder;

use fd_find::bench_support::*;

/// Build a flat hit stream of `n` `*.foo` files under a single root.
/// Half are directories so the type filter has work to do.
fn synth_hits(n: usize, root: &Arc<PathBuf>) -> Vec<RawHit> {
    (0..n)
        .map(|i| {
            let kind_is_dir = i % 2 == 0;
            let ext = if kind_is_dir { "" } else { "foo" };
            let name = if ext.is_empty() {
                format!("dir_{i}")
            } else {
                format!("file_{i}.{ext}")
            };
            let path = root.join(name);
            RawHit {
                path,
                is_dir: kind_is_dir,
                size: i as u64,
                mtime: 0,
                ctime: 0,
                attributes: 0,
                extension: ext.into(),
                depth: 1 + (i % 8),
                search_root: Arc::clone(root),
            }
        })
        .collect()
}

fn run_pipeline_once(hits: &[RawHit], pipeline_factory: &dyn Fn() -> Pipeline) -> usize {
    let cancel = CancellationToken::new();
    let backend = MockBackend::new(hits.to_vec());
    let mut downstream = CountingSink::default();
    let mut sink = PostFilterSink::new(pipeline_factory(), &mut downstream, &cancel);
    let _ = backend.run(&dummy_query(), &mut sink, &cancel);
    let _ = sink.finalize();
    downstream.0
}

const SIZES: &[usize] = &[1_000, 10_000];

fn bench_baseline(c: &mut Criterion) {
    let root = Arc::new(PathBuf::from(r"C:\bench"));
    let mut group = c.benchmark_group("pipeline_baseline");
    for &n in SIZES {
        let hits = synth_hits(n, &root);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &hits, |b, hits| {
            b.iter(|| run_pipeline_once(hits, &|| Pipeline::new(Vec::new())));
        });
    }
    group.finish();
}

fn bench_extension_filter(c: &mut Criterion) {
    let root = Arc::new(PathBuf::from(r"C:\bench"));
    let mut group = c.benchmark_group("pipeline_extension_filter");
    let regex_set = RegexSetBuilder::new([r".\.foo$"])
        .case_insensitive(true)
        .build()
        .unwrap();
    for &n in SIZES {
        let hits = synth_hits(n, &root);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &hits, |b, hits| {
            b.iter(|| {
                run_pipeline_once(hits, &|| {
                    Pipeline::new(vec![Box::new(ExtensionFilter::new(regex_set.clone()))])
                })
            });
        });
    }
    group.finish();
}

fn bench_type_filter(c: &mut Criterion) {
    let root = Arc::new(PathBuf::from(r"C:\bench"));
    let mut group = c.benchmark_group("pipeline_type_filter");
    let types = FileTypes {
        files: true,
        ..Default::default()
    };
    for &n in SIZES {
        let hits = synth_hits(n, &root);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &hits, |b, hits| {
            b.iter(|| {
                run_pipeline_once(hits, &|| {
                    Pipeline::new(vec![Box::new(TypeFilter::new(types.clone()))])
                })
            });
        });
    }
    group.finish();
}

fn bench_full_chain(c: &mut Criterion) {
    let root = Arc::new(PathBuf::from(r"C:\bench"));
    let regex_set = RegexSetBuilder::new([r".\.foo$"])
        .case_insensitive(true)
        .build()
        .unwrap();
    let types = FileTypes {
        files: true,
        ..Default::default()
    };
    let mut group = c.benchmark_group("pipeline_full_chain");
    for &n in SIZES {
        let hits = synth_hits(n, &root);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &hits, |b, hits| {
            b.iter(|| {
                run_pipeline_once(hits, &|| {
                    // Order mirrors PLAN §Phase 3: type -> extension ->
                    // depth. max_results omitted so we measure the full
                    // pipeline, not the short-circuit branch.
                    Pipeline::new(vec![
                        Box::new(TypeFilter::new(types.clone())),
                        Box::new(ExtensionFilter::new(regex_set.clone())),
                        Box::new(DepthFilter::new(None, Some(5))),
                    ])
                })
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_baseline,
    bench_extension_filter,
    bench_type_filter,
    bench_full_chain,
);
criterion_main!(benches);
