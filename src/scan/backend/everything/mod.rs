//! Everything SDK integration (PLAN.md §Phase 5).
//!
//! This module is Windows-only — both `cfg(target_os = "windows")` at the
//! point of declaration in `src/scan/backend.rs` and again here for belt-and-
//! braces. It contains:
//!
//! - [`ffi`]   — bindgen-generated raw bindings plus a `Mutex<SdkGuard>` that
//!   serialises every SDK call (the Everything DLL keeps a single global query
//!   state — concurrent calls would race).
//! - [`error`] — structured mapping of `EVERYTHING_ERROR_*` codes.
//! - [`backend`] — `EverythingBackend`, the `SearchBackend` impl that does the
//!   one-shot `Everything_SetRequestFlags` per PLAN.md §5.1 and streams
//!   `RawHit` into a `BackendSink`.
//!
//! The module is loaded but not yet wired into `walk::scan`; Phase 6
//! (backend selection) is what flips the switch.

#![cfg(target_os = "windows")]
// Phase 5 scaffolding wires into the legacy walker in Phase 7. Until then
// these symbols look unused to the binary build but are exercised by
// `cargo run --example list_sdk_flags` and #[cfg(test)] code.
#![allow(dead_code)]

pub mod backend;
pub mod error;
pub mod ffi;
// Phase 6 (PLAN.md §Phase 6): query translation + backend auto-selection.
// Both live behind plain `pub mod` because Phase 7 will wire them into
// `walk::scan` directly; until then they're exercised by their own
// `#[cfg(test)]` blocks.
pub mod probe;
pub mod query;
pub mod selection;

#[allow(unused_imports)]
pub use backend::EverythingBackend;
#[allow(unused_imports)]
pub use error::EverythingError;
