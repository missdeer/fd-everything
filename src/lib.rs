//! PLAN.md §Phase 8i: library target for criterion benchmarks.
//!
//! The fde binary is intentionally a `[[bin]]` target — see PLAN §Phase
//! 0 / §Phase 8c. The criterion benchmark harness in `benches/` needs to
//! import internal types (`Pipeline`, `MockBackend`, `RawHit`, …) and
//! cargo only links benchmarks against a `[lib]` target. To unblock the
//! benches without refactoring `src/main.rs` into a thin wrapper, this
//! lib re-declares the same module tree the binary already compiles and
//! exposes a `bench_support` re-export for the bench file to use.
//!
//! Both crate roots compile their own copy of `src/scan/**` etc; the
//! cost is one extra compilation per source file, which `cargo bench`
//! amortises over many bench runs. If the duplicated build ever becomes
//! a real bottleneck, the standard fix is to merge `main.rs` into this
//! lib (a mechanical change that PLAN §8i flags as out of scope here).

#[cfg(not(windows))]
compile_error!("fde is Windows-only. The Everything-SDK search backend requires Windows.");

pub mod cli;
pub mod config;
pub mod dir_entry;
pub mod error;
pub mod exec;
pub mod exit_codes;
pub mod filesystem;
pub mod filetypes;
pub mod filter;
pub mod fmt;
pub mod hyperlink;
pub mod output;
pub mod regex_helper;
pub mod scan;
pub mod walk;

/// PLAN.md §8i: small public surface that lets `benches/post_filter.rs`
/// drive `Pipeline` + `MockBackend` without `crate::`-private types
/// leaking. Re-exports only; no logic lives here.
pub mod bench_support {
    pub use crate::filetypes::FileTypes;
    pub use crate::scan::backend::{
        BackendError, BackendQuery, BackendSink, CancellationToken, CaseModifier, MockBackend,
        PatternScope, RawHit, SearchBackend, TranslatedPattern,
    };
    pub use crate::scan::post_filter::filters::depth::DepthFilter;
    pub use crate::scan::post_filter::filters::extension::ExtensionFilter;
    pub use crate::scan::post_filter::filters::type_filter::TypeFilter;
    pub use crate::scan::post_filter::{Pipeline, PostFilterSink};

    /// PLAN.md §8i: a default `BackendQuery` good enough to invoke
    /// `MockBackend::run` (which ignores its query). Pulled out of
    /// `benches/post_filter.rs` so the bench file stays free of
    /// struct-shape boilerplate.
    pub fn dummy_query() -> BackendQuery {
        BackendQuery {
            paths: vec![],
            pattern: TranslatedPattern {
                everything_query: String::new(),
                scope: PatternScope::Basename,
                case_modifier: CaseModifier::Nocase,
            },
            and_patterns: vec![],
            type_hint: None,
            size_hint: None,
            time_hint: None,
            max_depth: None,
            max_results: None,
        }
    }

    /// PLAN.md §8i: counting sink. Discards hit content; tracks count so
    /// criterion's volatile read can confirm the pipeline actually ran
    /// (and the optimiser didn't constant-fold the bench away).
    #[derive(Default)]
    pub struct CountingSink(pub usize);

    impl BackendSink for CountingSink {
        fn send(&mut self, _hit: RawHit) -> Result<(), BackendError> {
            self.0 += 1;
            Ok(())
        }
    }
}
