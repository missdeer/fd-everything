//! Raw Everything SDK bindings + serialised access wrapper.
//!
//! ## Why a global Mutex?
//!
//! The Everything DLL keeps a single piece of process-global query state
//! (search string, request flags, result list). Two threads calling
//! `Everything_SetSearchW` + `Everything_QueryW` concurrently would corrupt
//! each other's results. PLAN.md §Phase 5 mandates serialisation; we model
//! it with a `Mutex<SdkGuard>` so that holding the guard *is* the only way
//! to touch the SDK from safe code.
//!
//! ## Why `include!` rather than `#[bindgen(...)]`?
//!
//! `build.rs` emits the bindings file to `$OUT_DIR/everything_bindings.rs`
//! during the build. The `include!` pulls it inline so downstream modules can
//! refer to e.g. `ffi::sys::EVERYTHING_REQUEST_SIZE` without any extra
//! re-export ceremony. Constants land in the [`sys`] sub-namespace; safe
//! wrappers live at the module root.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Raw bindgen output. Treat every item here as `unsafe` to call — the safe
/// wrappers below are the only sanctioned entry points.
pub mod sys {
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(non_upper_case_globals)]
    #![allow(dead_code)]
    #![allow(clippy::all)]

    include!(concat!(env!("OUT_DIR"), "/everything_bindings.rs"));
}

/// Global serialisation point for SDK access. The first call to [`sdk`] lazily
/// constructs the mutex.
fn sdk_mutex() -> &'static Mutex<SdkGuard> {
    static MUTEX: OnceLock<Mutex<SdkGuard>> = OnceLock::new();
    MUTEX.get_or_init(|| Mutex::new(SdkGuard { _private: () }))
}

/// Acquire exclusive access to the Everything SDK.
///
/// Blocks until any prior caller drops their [`SdkGuard`]. Holders MUST call
/// [`SdkGuard::reset`] before dropping if they wrote any state, otherwise the
/// next caller inherits stale request flags / search string.
pub fn sdk() -> MutexGuard<'static, SdkGuard> {
    // PoisonError handling: if a prior caller panicked mid-SDK-call the
    // global SDK state is corrupted. Treat the poison as fatal here rather
    // than papering over it — Phase 5 error path is "structured EverythingError
    // -> ExitCode::GeneralError" per PLAN §Phase 5.
    sdk_mutex()
        .lock()
        .expect("Everything SDK mutex poisoned — a prior call panicked")
}

/// RAII handle: holding one means the current thread is the sole SDK caller.
/// Construction is private so the only way in is via [`sdk()`].
pub struct SdkGuard {
    _private: (),
}

impl SdkGuard {
    /// `Everything_Reset()` — clears search string, results, request flags.
    pub fn reset(&self) {
        unsafe { sys::Everything_Reset() };
    }

    /// `Everything_SetSearchW(query)` — sets the UTF-16 query string.
    pub fn set_search(&self, query_utf16: &[u16]) {
        // bindgen + UNICODE makes `Everything_SetSearch` resolve to the W
        // variant. We pass the explicit W form to stay readable.
        debug_assert!(
            query_utf16.last() == Some(&0),
            "Everything_SetSearchW requires a NUL-terminated UTF-16 string"
        );
        unsafe { sys::Everything_SetSearchW(query_utf16.as_ptr()) };
    }

    /// `Everything_SetRequestFlags(flags)` — bitfield of `EVERYTHING_REQUEST_*`.
    pub fn set_request_flags(&self, flags: u32) {
        unsafe { sys::Everything_SetRequestFlags(flags) };
    }

    /// `Everything_SetMatchPath(true/false)` — toggles match-against-path
    /// vs match-against-name. We always pass `false` and rely on the
    /// `path:` / `nopath:` modifiers in the query string (PLAN §6.1), so the
    /// GUI's "Match Path" toggle can never leak in.
    pub fn set_match_path(&self, enable: bool) {
        unsafe { sys::Everything_SetMatchPath(bool_to_win(enable)) };
    }

    /// `Everything_SetMatchCase(true/false)` — same story as set_match_path:
    /// we drive case-sensitivity through `case:` / `nocase:` in the query.
    pub fn set_match_case(&self, enable: bool) {
        unsafe { sys::Everything_SetMatchCase(bool_to_win(enable)) };
    }

    /// `Everything_SetRegex(true/false)` — we leave at `false` and use
    /// `regex:"<pat>"` in the query string (per PLAN §6.1) so the toggle path
    /// matches the modifier path exactly.
    pub fn set_regex(&self, enable: bool) {
        unsafe { sys::Everything_SetRegex(bool_to_win(enable)) };
    }

    /// `Everything_SetMax(n)` — caps result count. We pass the configured
    /// `--max-results` value when present so the SDK can stop early.
    pub fn set_max(&self, max: u32) {
        unsafe { sys::Everything_SetMax(max) };
    }

    /// `Everything_QueryW(bWait)`. Returns `Ok(())` on success, otherwise
    /// reads `Everything_GetLastError` and surfaces it as a typed
    /// [`super::EverythingError`].
    pub fn query(&self, wait: bool) -> Result<(), super::EverythingError> {
        let ok = unsafe { sys::Everything_QueryW(bool_to_win(wait)) };
        if ok != 0 {
            Ok(())
        } else {
            Err(super::EverythingError::from_last_error(unsafe {
                sys::Everything_GetLastError()
            }))
        }
    }

    /// `Everything_GetNumResults()` — number of hits in the current result
    /// list. Only valid after a successful [`Self::query`].
    pub fn num_results(&self) -> u32 {
        unsafe { sys::Everything_GetNumResults() }
    }

    /// `Everything_IsFolderResult(index)`.
    pub fn is_folder_result(&self, index: u32) -> bool {
        unsafe { sys::Everything_IsFolderResult(index) != 0 }
    }

    /// `Everything_GetResultFullPathNameW` — fills `buf` and returns the
    /// resulting path as an `OsString`. Buffer is grown on demand if the
    /// first call hints a larger size.
    pub fn result_full_path(&self, index: u32) -> Option<OsString> {
        // First call with empty buf returns required length excluding the
        // terminating NUL (per Everything SDK docs). Round up generously to
        // amortise reallocs across calls.
        let mut buf: Vec<u16> = vec![0; 260];
        loop {
            let written = unsafe {
                sys::Everything_GetResultFullPathNameW(index, buf.as_mut_ptr(), buf.len() as u32)
            };
            if written == 0 {
                return None;
            }
            // The SDK returns the number of characters copied (excluding
            // NUL). If it equals buf.len() - 1 the path may have been
            // truncated; grow and retry.
            let written = written as usize;
            if written + 1 < buf.len() {
                buf.truncate(written);
                return Some(OsString::from_wide(&buf));
            }
            buf.resize(buf.len() * 2, 0);
        }
    }

    /// `Everything_GetResultAttributes(index)` — Win32 `FILE_ATTRIBUTE_*`
    /// bitmask.
    pub fn result_attributes(&self, index: u32) -> u32 {
        unsafe { sys::Everything_GetResultAttributes(index) }
    }

    /// `Everything_GetResultSize(index, &out)` — returns `Some(bytes)` on
    /// success. The SDK reports failure when the index is invalid OR when
    /// the request flags didn't include `EVERYTHING_REQUEST_SIZE`.
    pub fn result_size(&self, index: u32) -> Option<u64> {
        let mut out: sys::LARGE_INTEGER = unsafe { std::mem::zeroed() };
        let ok = unsafe { sys::Everything_GetResultSize(index, &mut out) };
        if ok == 0 {
            return None;
        }
        // LARGE_INTEGER is a union; `QuadPart` is the full signed 64-bit
        // field. Negative sizes are nonsensical here (folders report 0 / -1
        // in some SDK builds) — clamp to 0.
        let quad = unsafe { out.QuadPart };
        Some(quad.max(0) as u64)
    }

    /// `Everything_GetResultDateModified(index, &out)` — FILETIME (100ns
    /// ticks since 1601-01-01 UTC) packed into i64. None if not requested or
    /// not indexed for that result.
    pub fn result_date_modified(&self, index: u32) -> Option<i64> {
        let mut ft: sys::FILETIME = unsafe { std::mem::zeroed() };
        let ok = unsafe { sys::Everything_GetResultDateModified(index, &mut ft) };
        if ok == 0 {
            return None;
        }
        Some(filetime_to_i64(&ft))
    }

    /// `Everything_GetResultDateCreated(index, &out)` — see
    /// [`Self::result_date_modified`].
    pub fn result_date_created(&self, index: u32) -> Option<i64> {
        let mut ft: sys::FILETIME = unsafe { std::mem::zeroed() };
        let ok = unsafe { sys::Everything_GetResultDateCreated(index, &mut ft) };
        if ok == 0 {
            return None;
        }
        Some(filetime_to_i64(&ft))
    }

    /// `Everything_GetResultExtensionW(index)` — extension without leading
    /// dot, or empty.
    pub fn result_extension(&self, index: u32) -> OsString {
        let ptr = unsafe { sys::Everything_GetResultExtensionW(index) };
        if ptr.is_null() {
            return OsString::new();
        }
        // SAFETY: SDK guarantees NUL-terminated UTF-16. We measure length
        // by scanning until the NUL.
        let mut len = 0usize;
        // Cap scan length defensively; extensions over 4096 chars don't exist.
        while len < 4096 && unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        OsString::from_wide(slice)
    }

    /// `Everything_IsDBLoaded()` — true once Everything has finished
    /// loading its index after startup. PLAN.md §Phase 8 §3 calls this out
    /// as the readiness probe to wait on before running e2e queries.
    /// Returns `false` when the SDK reports IPC unavailable, so callers
    /// can poll this both as "ready?" and "alive?".
    pub fn db_loaded(&self) -> bool {
        unsafe { sys::Everything_IsDBLoaded() != 0 }
    }

    /// `Everything_GetMajorVersion()` / `GetMinorVersion` / `GetRevision` /
    /// `GetBuildNumber` — packed into a tuple so callers can do a single
    /// version sanity check on startup. Returns `None` if the SDK reports
    /// IPC unavailable (Everything not running).
    pub fn version(&self) -> Option<(u32, u32, u32, u32)> {
        let major = unsafe { sys::Everything_GetMajorVersion() };
        if major == 0 {
            // Per SDK docs, every version getter returns 0 + sets
            // EVERYTHING_ERROR_IPC when the service is unavailable.
            return None;
        }
        let minor = unsafe { sys::Everything_GetMinorVersion() };
        let revision = unsafe { sys::Everything_GetRevision() };
        let build = unsafe { sys::Everything_GetBuildNumber() };
        Some((major, minor, revision, build))
    }
}

/// Translate a Rust `bool` to the Win32 `BOOL` ABI value.
fn bool_to_win(b: bool) -> sys::BOOL {
    if b { 1 } else { 0 }
}

/// Pack FILETIME (high/low 32-bit halves) into a single signed i64 of
/// 100ns ticks since 1601-01-01 UTC. PLAN §Phase 1 mandates this canonical
/// form so the post-filter time-range check can compare directly without an
/// intermediate `SystemTime`.
fn filetime_to_i64(ft: &sys::FILETIME) -> i64 {
    ((ft.dwHighDateTime as i64) << 32) | (ft.dwLowDateTime as i64)
}

/// Convenience: build a NUL-terminated UTF-16 buffer from a `&str`. Returned
/// `Vec<u16>` always ends in a single `0`.
pub fn to_utf16_nul(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bindgen output must expose every request flag we lean on in
    /// PLAN §5.1. If the SDK header ever renames one of these constants the
    /// build fails here instead of in the EverythingBackend implementation,
    /// which is where the symptom would be far more confusing.
    #[test]
    fn required_request_flags_are_in_scope() {
        let _flags = sys::EVERYTHING_REQUEST_FILE_NAME
            | sys::EVERYTHING_REQUEST_PATH
            | sys::EVERYTHING_REQUEST_FULL_PATH_AND_FILE_NAME
            | sys::EVERYTHING_REQUEST_ATTRIBUTES
            | sys::EVERYTHING_REQUEST_SIZE
            | sys::EVERYTHING_REQUEST_DATE_MODIFIED
            | sys::EVERYTHING_REQUEST_DATE_CREATED
            | sys::EVERYTHING_REQUEST_EXTENSION;
        // No runtime assertion — the value of `_flags` is irrelevant. The
        // point is that all eight names compile.
    }

    /// Sanity-check the UTF-16 helper: the SDK requires NUL termination and
    /// will read past the end without it.
    #[test]
    fn to_utf16_nul_terminates() {
        let v = to_utf16_nul("ab");
        assert_eq!(v, vec![0x61, 0x62, 0x00]);
    }

    /// Mutex contract: two sequential `sdk()` calls succeed (no deadlock,
    /// no poisoning).
    #[test]
    fn sdk_mutex_is_reentrant_across_calls() {
        {
            let _guard = sdk();
        }
        {
            let _guard = sdk();
        }
    }
}
