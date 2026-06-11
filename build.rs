//! Build script for fd-everything.
//!
//! Phase 5 (PLAN.md §Phase 5): on Windows targets only, generate Rust FFI
//! bindings for the Everything SDK from the vendored
//! `Everything-SDK/include/Everything.h` header, and emit link directives for
//! `Everything64.lib`. The generated file lands at
//! `$OUT_DIR/everything_bindings.rs` and is included by
//! `src/scan/backend/everything/ffi.rs`.
//!
//! Non-Windows hosts are a no-op: the entire `everything` backend tree is
//! gated behind `#[cfg(target_os = "windows")]`, so the bindings file never
//! needs to exist there.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    #[cfg(target_os = "windows")]
    generate_everything_bindings();
}

#[cfg(target_os = "windows")]
fn generate_everything_bindings() {
    use std::path::PathBuf;

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sdk_root = manifest_dir.join("Everything-SDK");
    let header = sdk_root.join("include").join("Everything.h");
    let lib_dir = sdk_root.join("lib");

    // Resolve target architecture to pick the right Everything*.lib stub.
    // PLAN.md §Phase 5 names Everything64 explicitly; we extend to the other
    // SDK-shipped stubs so the same build.rs serves arm64 / x86 cross builds
    // without surprising the consumer.
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let lib_name = match target_arch.as_str() {
        "x86_64" => "Everything64",
        "x86" => "Everything32",
        "aarch64" => "EverythingARM64",
        "arm" => "EverythingARM",
        other => panic!("Unsupported Windows target arch for Everything SDK: {other}"),
    };

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib={lib_name}");
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed=build.rs");

    // Allowlist Everything_* / EVERYTHING_* only. Everything.h pulls in
    // <windows.h>, so without an allowlist bindgen would emit the entire Win32
    // type universe. We still let bindgen pick up the transitively-needed
    // types (BOOL, DWORD, LARGE_INTEGER, FILETIME, HWND, LPCWSTR) by design,
    // since FFI signatures reference them.
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .allowlist_function("Everything_.*")
        .allowlist_var("EVERYTHING_.*")
        // UNICODE so the macro aliases (`Everything_SetSearch` etc.) map to
        // the W variants. We always call the W variants directly anyway, but
        // this keeps the generated constants stable.
        .clang_arg("-DUNICODE")
        .clang_arg("-D_UNICODE")
        // Keep the generated file small + readable.
        .layout_tests(false)
        .derive_default(false)
        .generate_comments(false)
        .generate()
        .expect("bindgen failed to generate Everything SDK bindings");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("everything_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write Everything SDK bindings");
}
