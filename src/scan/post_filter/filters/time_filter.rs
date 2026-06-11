//! `--changed-within` / `--changed-before`: bridges the FILETIME
//! `mtime` carried by RawHit to the existing
//! [`TimeFilter::applies_to`](crate::filter::TimeFilter::applies_to)
//! which speaks `SystemTime`. The conversion lives here (rather than
//! inside `RawHit`) so RawHit stays a pure data carrier — backends
//! supply FILETIME because that's what both Win32 and the Everything
//! SDK return natively, avoiding a SystemTime round-trip per hit.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::filter::TimeFilter;
use crate::scan::backend::RawHit;
use crate::scan::post_filter::{Filter, Verdict};

/// Difference between the Win32 FILETIME epoch (1601-01-01 UTC) and the
/// UNIX epoch (1970-01-01 UTC), in 100 ns ticks. Pre-computed:
/// 369 years × 365.2425 days × 86400 s × 10_000_000 ticks ≈
/// 116_444_736_000_000_000.
const FILETIME_TO_UNIX_EPOCH_100NS: i64 = 116_444_736_000_000_000;

/// Convert a Win32 FILETIME (100 ns ticks since 1601-01-01 UTC) into a
/// `SystemTime`. Out-of-range values (negative or pre-UNIX-epoch) clamp
/// to `UNIX_EPOCH`, matching how the legacy walker handles a missing
/// `modified()` (treats it as "not matching" via metadata.modified()
/// returning Err, but at least gives a deterministic answer here).
pub fn filetime_to_systemtime(filetime: i64) -> SystemTime {
    let unix_100ns = filetime - FILETIME_TO_UNIX_EPOCH_100NS;
    if unix_100ns <= 0 {
        return UNIX_EPOCH;
    }
    let unix_100ns = unix_100ns as u64;
    let secs = unix_100ns / 10_000_000;
    let sub_100ns = unix_100ns % 10_000_000;
    let nanos = sub_100ns * 100;
    UNIX_EPOCH + Duration::new(secs, nanos as u32)
}

pub struct TimeConstraints {
    constraints: Vec<TimeFilter>,
}

impl TimeConstraints {
    pub fn new(constraints: Vec<TimeFilter>) -> Self {
        Self { constraints }
    }
}

impl Filter for TimeConstraints {
    fn evaluate(&mut self, hit: &RawHit) -> Verdict {
        if self.constraints.is_empty() {
            return Verdict::Keep;
        }
        let modified = filetime_to_systemtime(hit.mtime);
        if self.constraints.iter().all(|c| c.applies_to(&modified)) {
            Verdict::Keep
        } else {
            Verdict::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn hit(mtime: i64) -> RawHit {
        RawHit {
            path: PathBuf::from("/x"),
            is_dir: false,
            size: 0,
            mtime,
            ctime: 0,
            attributes: 0,
            extension: "".into(),
            depth: 1,
            search_root: Arc::new(PathBuf::from("/")),
        }
    }

    /// Encodes the FILETIME → SystemTime conversion. A FILETIME equal
    /// to the UNIX epoch offset must produce exactly `UNIX_EPOCH`. If
    /// this regresses, `--changed-within` slips by hundreds of years.
    #[test]
    fn filetime_at_unix_epoch_round_trips() {
        assert_eq!(
            filetime_to_systemtime(FILETIME_TO_UNIX_EPOCH_100NS),
            UNIX_EPOCH
        );
    }

    /// Encodes the `Before(now)` contract: a hit modified yesterday
    /// passes a "before now" filter; a hit modified far in the future
    /// drops.
    #[test]
    fn before_now_drops_future_hits() {
        // Past: 1980-01-01 → FILETIME ~119_600_064_000_000_000
        let yesterday = FILETIME_TO_UNIX_EPOCH_100NS + 315_532_800_i64 * 10_000_000; // 1980
        let future = FILETIME_TO_UNIX_EPOCH_100NS + (i64::from(u32::MAX)) * 10_000_000; // far enough in the future
        let before_now = TimeFilter::before("0sec").expect("parseable");
        let mut f = TimeConstraints::new(vec![before_now]);
        assert_eq!(f.evaluate(&hit(yesterday)), Verdict::Keep);
        assert_eq!(f.evaluate(&hit(future)), Verdict::Drop);
    }
}
