//! Structured Everything SDK errors (PLAN.md §Phase 5 failure-mode mapping).
//!
//! The SDK signals failure by returning a falsy `BOOL` from query functions
//! and setting an `EVERYTHING_ERROR_*` code retrievable via
//! `Everything_GetLastError`. We mirror those codes here so the
//! `walk::scan` consumer can:
//!
//! 1. Decide whether to fall back to the legacy walker
//!    (`Ipc` ⇒ Everything isn't running, definitely fall back), and
//! 2. Surface a useful message — PLAN §Phase 5 calls out
//!    "启动 Everything 后重试" (start Everything and retry) verbatim.

use super::ffi::sys;

/// One per known `EVERYTHING_ERROR_*` constant, plus an `Unknown(code)` arm
/// so future SDK revisions don't silently regress to "everything is fine".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EverythingError {
    /// `EVERYTHING_ERROR_MEMORY` — out of memory inside the SDK process.
    Memory,
    /// `EVERYTHING_ERROR_IPC` — Everything service is not running. This is
    /// the most common failure mode in practice; the consumer should fall
    /// back to LegacyWalkerBackend and print the "start Everything" hint.
    Ipc,
    /// `EVERYTHING_ERROR_REGISTERCLASSEX` — SDK failed to register its
    /// listening window class.
    RegisterClassEx,
    /// `EVERYTHING_ERROR_CREATEWINDOW` — listening window creation failed.
    CreateWindow,
    /// `EVERYTHING_ERROR_CREATETHREAD` — listening thread creation failed.
    CreateThread,
    /// `EVERYTHING_ERROR_INVALIDINDEX` — result-list accessor called with
    /// an out-of-range index. Indicates a backend bug, never a user error.
    InvalidIndex,
    /// `EVERYTHING_ERROR_INVALIDCALL` — SDK call order violation
    /// (e.g. result accessor before query). Indicates a backend bug.
    InvalidCall,
    /// `EVERYTHING_ERROR_INVALIDREQUEST` — accessor for a field the
    /// preceding `Everything_SetRequestFlags` didn't ask for. This catches
    /// drift between PLAN §5.1's flag list and the per-hit accessors.
    InvalidRequest,
    /// `EVERYTHING_ERROR_INVALIDPARAMETER` — generic bad-arg, e.g. NULL
    /// buffer to `GetResultFullPathName`. Indicates a backend bug.
    InvalidParameter,
    /// Anything else the SDK returns. Carries the raw code for the message.
    Unknown(u32),
}

impl EverythingError {
    /// Map a raw `Everything_GetLastError` code into a structured variant.
    pub fn from_last_error(code: u32) -> Self {
        match code {
            sys::EVERYTHING_ERROR_MEMORY => Self::Memory,
            sys::EVERYTHING_ERROR_IPC => Self::Ipc,
            sys::EVERYTHING_ERROR_REGISTERCLASSEX => Self::RegisterClassEx,
            sys::EVERYTHING_ERROR_CREATEWINDOW => Self::CreateWindow,
            sys::EVERYTHING_ERROR_CREATETHREAD => Self::CreateThread,
            sys::EVERYTHING_ERROR_INVALIDINDEX => Self::InvalidIndex,
            sys::EVERYTHING_ERROR_INVALIDCALL => Self::InvalidCall,
            sys::EVERYTHING_ERROR_INVALIDREQUEST => Self::InvalidRequest,
            sys::EVERYTHING_ERROR_INVALIDPARAMETER => Self::InvalidParameter,
            other => Self::Unknown(other),
        }
    }

    /// True iff this error means "Everything just isn't available"; the
    /// caller should silently fall back to LegacyWalkerBackend rather than
    /// surfacing a hard failure (PLAN §Phase 5 fallback semantics).
    pub fn is_unavailable(self) -> bool {
        matches!(
            self,
            Self::Ipc | Self::RegisterClassEx | Self::CreateWindow | Self::CreateThread
        )
    }
}

impl std::fmt::Display for EverythingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory => f.write_str("Everything SDK reported out-of-memory"),
            Self::Ipc => {
                f.write_str("Everything service is not running — start Everything and retry")
            }
            Self::RegisterClassEx => {
                f.write_str("Everything SDK could not register its listening window class")
            }
            Self::CreateWindow => {
                f.write_str("Everything SDK could not create its listening window")
            }
            Self::CreateThread => {
                f.write_str("Everything SDK could not create its listening thread")
            }
            Self::InvalidIndex => {
                f.write_str("Everything SDK call used an out-of-range result index (bug)")
            }
            Self::InvalidCall => f.write_str("Everything SDK call sequenced incorrectly (bug)"),
            Self::InvalidRequest => f.write_str(
                "Everything SDK accessor asked for a field that wasn't in SetRequestFlags (bug)",
            ),
            Self::InvalidParameter => {
                f.write_str("Everything SDK call passed a bad parameter (bug)")
            }
            Self::Unknown(code) => {
                write!(f, "Everything SDK returned unknown error code {code}")
            }
        }
    }
}

impl std::error::Error for EverythingError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PLAN-mandated user-facing message for the most-common failure
    /// (Everything not running) must include the explicit "start Everything"
    /// instruction. If marketing wording drifts here, end-users lose the
    /// only actionable error message they ever see from this code path.
    #[test]
    fn ipc_error_carries_actionable_hint() {
        let msg = EverythingError::Ipc.to_string();
        assert!(
            msg.contains("start Everything"),
            "IPC error message must tell the user to start Everything; got: {msg}"
        );
    }

    /// `is_unavailable()` drives the LegacyWalker fallback in Phase 6.
    /// Programmer-error variants must NOT trigger fallback — otherwise a
    /// bug in our flag set silently degrades to slow-path forever.
    #[test]
    fn programmer_error_variants_do_not_trigger_fallback() {
        for v in [
            EverythingError::InvalidIndex,
            EverythingError::InvalidCall,
            EverythingError::InvalidRequest,
            EverythingError::InvalidParameter,
            EverythingError::Memory,
        ] {
            assert!(!v.is_unavailable(), "{v:?} must not trigger fallback");
        }
    }

    /// `is_unavailable()` is the affirmative side of the same contract.
    #[test]
    fn unavailable_variants_trigger_fallback() {
        assert!(EverythingError::Ipc.is_unavailable());
        assert!(EverythingError::RegisterClassEx.is_unavailable());
        assert!(EverythingError::CreateWindow.is_unavailable());
        assert!(EverythingError::CreateThread.is_unavailable());
    }
}
