//! `cargo run --example list_sdk_flags` (Windows only).
//!
//! Day-1 validation tool required by PLAN.md §5.1: enumerate every
//! `EVERYTHING_REQUEST_*` constant exposed by the bindgen output and confirm
//! the names match the assumptions baked into
//! `src/scan/backend/everything/backend.rs::REQUEST_FLAGS`.
//!
//! ## Why is this here and not a unit test?
//!
//! The unit test in `src/scan/backend/everything/ffi.rs` already proves the
//! constants compile. This example exists for the *human reviewer*: PLAN §5.1
//! mandates a "list bindgen-emitted constants and check the names" step
//! before Phase 6 (query translation) starts. Running it prints the bitfield
//! values so a reviewer can cross-check them against `Everything.h` by eye.
//!
//! ## Why `include!` instead of `use fd_find::...`?
//!
//! `fd-find` is a binary-only crate (`[[bin]] fde`). There is no library
//! target, so the example can't reach into the bin's modules. Instead it
//! re-uses the same `$OUT_DIR/everything_bindings.rs` file that the bin's
//! `src/scan/backend/everything/ffi.rs` includes — `OUT_DIR` is shared
//! across all targets of a package.
//!
//! Output:
//!
//! ```text
//! 0x00000004  EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME  [REQUIRED]
//! ...
//! ```

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("list_sdk_flags is Windows-only — Everything SDK does not exist on this target.");
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
mod sys {
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(non_upper_case_globals)]
    #![allow(dead_code)]
    #![allow(clippy::all)]

    // The build script emits `cargo:rustc-link-lib=dylib=Everything64`, which
    // applies cleanly to the `fde` bin but does NOT propagate to example
    // targets in this binary-only package on MSRV — `cargo test` then fails to
    // link the example with LNK2019 on `Everything_Get*Version`. The empty
    // extern block below carries a `#[link]` attribute that tells rustc to
    // pull in `Everything64.lib` specifically for this example.
    #[cfg_attr(target_arch = "x86_64", link(name = "Everything64", kind = "dylib"))]
    #[cfg_attr(target_arch = "x86", link(name = "Everything32", kind = "dylib"))]
    #[cfg_attr(target_arch = "aarch64", link(name = "EverythingARM64", kind = "dylib"))]
    #[cfg_attr(target_arch = "arm", link(name = "EverythingARM", kind = "dylib"))]
    unsafe extern "C" {}

    include!(concat!(env!("OUT_DIR"), "/everything_bindings.rs"));
}

#[cfg(target_os = "windows")]
fn main() {
    // (value, name, is_required_by_backend)
    //
    // The "required" set tracks PLAN §5.1's flag list verbatim. Any future
    // additions to REQUEST_FLAGS in backend.rs should land here in lockstep
    // so this example keeps catching drift.
    let flags: &[(u32, &str, bool)] = &[
        (
            sys::EVERYTHING_REQUEST_FILE_NAME,
            "EVERYTHING_REQUEST_FILE_NAME",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_PATH,
            "EVERYTHING_REQUEST_PATH",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME,
            "EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME",
            true,
        ),
        (
            sys::EVERYTHING_REQUEST_EXTENSION,
            "EVERYTHING_REQUEST_EXTENSION",
            true,
        ),
        (
            sys::EVERYTHING_REQUEST_SIZE,
            "EVERYTHING_REQUEST_SIZE",
            true,
        ),
        (
            sys::EVERYTHING_REQUEST_DATE_CREATED,
            "EVERYTHING_REQUEST_DATE_CREATED",
            true,
        ),
        (
            sys::EVERYTHING_REQUEST_DATE_MODIFIED,
            "EVERYTHING_REQUEST_DATE_MODIFIED",
            true,
        ),
        (
            sys::EVERYTHING_REQUEST_DATE_ACCESSED,
            "EVERYTHING_REQUEST_DATE_ACCESSED",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_ATTRIBUTES,
            "EVERYTHING_REQUEST_ATTRIBUTES",
            true,
        ),
        (
            sys::EVERYTHING_REQUEST_FILE_LIST_FILE_NAME,
            "EVERYTHING_REQUEST_FILE_LIST_FILE_NAME",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_RUN_COUNT,
            "EVERYTHING_REQUEST_RUN_COUNT",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_DATE_RUN,
            "EVERYTHING_REQUEST_DATE_RUN",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_DATE_RECENTLY_CHANGED,
            "EVERYTHING_REQUEST_DATE_RECENTLY_CHANGED",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_HIGHLIGHTED_FILE_NAME,
            "EVERYTHING_REQUEST_HIGHLIGHTED_FILE_NAME",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_HIGHLIGHTED_PATH,
            "EVERYTHING_REQUEST_HIGHLIGHTED_PATH",
            false,
        ),
        (
            sys::EVERYTHING_REQUEST_HIGHLIGHTED_FULL_PATH_AND_FILE_NAME,
            "EVERYTHING_REQUEST_HIGHLIGHTED_FULL_PATH_AND_FILE_NAME",
            false,
        ),
    ];

    println!("Everything SDK request flags (bindgen-emitted):");
    println!();
    let mut required_present = 0;
    let mut required_total = 0;
    for (value, name, required) in flags {
        let tag = if *required {
            required_total += 1;
            required_present += 1;
            " [REQUIRED]"
        } else {
            ""
        };
        println!("0x{value:08x}  {name}{tag}");
    }
    println!();
    println!(
        "{required_present}/{required_total} required flags compiled — bindgen exposes all PLAN §5.1 names."
    );

    // Bitfield composition sanity check: exercise the same OR pattern the
    // backend uses so any constant-width mismatch (e.g. bindgen widening
    // these to u64) surfaces here, not in the Phase 6 query path.
    let composed: u32 = sys::EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME
        | sys::EVERYTHING_REQUEST_ATTRIBUTES
        | sys::EVERYTHING_REQUEST_SIZE
        | sys::EVERYTHING_REQUEST_DATE_MODIFIED
        | sys::EVERYTHING_REQUEST_DATE_CREATED
        | sys::EVERYTHING_REQUEST_EXTENSION;
    let expected: u32 =
        0x0000_0004 | 0x0000_0100 | 0x0000_0010 | 0x0000_0040 | 0x0000_0020 | 0x0000_0008;
    println!();
    println!("Composed REQUEST_FLAGS bitfield = 0x{composed:08x}");
    println!("Expected per PLAN §5.1 / Everything.h = 0x{expected:08x}");

    if composed != expected {
        eprintln!(
            "FAIL: bindgen constant values diverge from PLAN §5.1. \
             Re-check Everything.h before continuing Phase 5."
        );
        std::process::exit(1);
    }

    // Bonus: probe the Everything service. Version getters return 0 + set
    // EVERYTHING_ERROR_IPC if the service isn't running. We don't fail the
    // example here — CI hosts typically don't have Everything installed —
    // we just print so a human reviewer can see "yes, the DLL loaded and
    // the IPC pipe responded".
    let major = unsafe { sys::Everything_GetMajorVersion() };
    if major == 0 {
        println!();
        println!(
            "Everything service did not respond (likely not running). \
             DLL link succeeded, so bindings are valid."
        );
    } else {
        let minor = unsafe { sys::Everything_GetMinorVersion() };
        let rev = unsafe { sys::Everything_GetRevision() };
        let build = unsafe { sys::Everything_GetBuildNumber() };
        println!();
        println!("Everything service responded: v{major}.{minor}.{rev}.{build}");
    }
}
