use std::borrow::Cow;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
#[cfg(any(unix, target_os = "redox"))]
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use normpath::PathExt;

use crate::dir_entry;

pub fn path_absolute_form(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let path = path.strip_prefix(".").unwrap_or(path);
    env::current_dir().map(|path_buf| path_buf.join(path))
}

/// Strip `base` from `path` using the host filesystem's path-comparison
/// semantics. Rust's [`Path::strip_prefix`] is case-sensitive on every host,
/// while Windows paths are case-insensitive.
pub fn strip_path_prefix<'a>(path: &'a Path, base: &Path) -> Option<&'a Path> {
    if let Ok(relative) = path.strip_prefix(base) {
        return Some(relative);
    }

    #[cfg(windows)]
    {
        let mut path_components = path.components();
        for base_component in base.components() {
            let path_component = path_components.next()?;
            if !windows_os_str_eq_ignore_case(
                path_component.as_os_str(),
                base_component.as_os_str(),
            ) {
                return None;
            }
        }
        Some(path_components.as_path())
    }

    #[cfg(not(windows))]
    None
}

/// Compare paths using the host filesystem's path-comparison semantics.
pub fn paths_equal(left: &Path, right: &Path) -> bool {
    strip_path_prefix(left, right).is_some_and(|relative| relative.as_os_str().is_empty())
}

#[cfg(windows)]
fn windows_os_str_eq_ignore_case(left: &OsStr, right: &OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    if left == right {
        return true;
    }

    // Avoid UTF-16 buffers for the overwhelmingly common ASCII path. The
    // Win32 comparison below supplies Windows' full ordinal case semantics
    // for non-ASCII names and non-Unicode OsStr values.
    let left_lossy = left.to_string_lossy();
    let right_lossy = right.to_string_lossy();
    if left_lossy.is_ascii() && right_lossy.is_ascii() {
        return left_lossy.eq_ignore_ascii_case(&right_lossy);
    }

    let left_wide: Vec<u16> = left.encode_wide().collect();
    let right_wide: Vec<u16> = right.encode_wide().collect();
    let (Ok(left_len), Ok(right_len)) = (
        i32::try_from(left_wide.len()),
        i32::try_from(right_wide.len()),
    ) else {
        return false;
    };
    // SAFETY: both pointers remain valid for their explicitly supplied
    // lengths for the duration of the call. CompareStringOrdinal does not
    // require NUL termination when lengths are non-negative.
    unsafe {
        CompareStringOrdinal(
            left_wide.as_ptr(),
            left_len,
            right_wide.as_ptr(),
            right_len,
            1,
        ) == CSTR_EQUAL
    }
}

pub fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    let path_buf = path_absolute_form(path)?;

    #[cfg(windows)]
    let path_buf = Path::new(
        path_buf
            .as_path()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    )
    .to_path_buf();

    Ok(path_buf)
}

pub fn is_existing_directory(path: &Path) -> bool {
    // Note: we do not use `.exists()` here, as `.` always exists, even if
    // the CWD has been deleted.
    path.is_dir() && (path.file_name().is_some() || path.normalize().is_ok())
}

pub fn is_empty(entry: &dir_entry::DirEntry) -> bool {
    if let Some(file_type) = entry.file_type() {
        if file_type.is_dir() {
            if let Ok(mut entries) = fs::read_dir(entry.path()) {
                entries.next().is_none()
            } else {
                false
            }
        } else if file_type.is_file() {
            entry.metadata().map(|m| m.len() == 0).unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    }
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_block_device(ft: fs::FileType) -> bool {
    ft.is_block_device()
}

#[cfg(windows)]
pub fn is_block_device(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_char_device(ft: fs::FileType) -> bool {
    ft.is_char_device()
}

#[cfg(windows)]
pub fn is_char_device(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_socket(ft: fs::FileType) -> bool {
    ft.is_socket()
}

#[cfg(windows)]
pub fn is_socket(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_pipe(ft: fs::FileType) -> bool {
    ft.is_fifo()
}

#[cfg(windows)]
pub fn is_pipe(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn osstr_to_bytes(input: &OsStr) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(input.as_bytes())
}

#[cfg(windows)]
pub fn osstr_to_bytes(input: &OsStr) -> Cow<'_, [u8]> {
    let string = input.to_string_lossy();

    match string {
        Cow::Owned(string) => Cow::Owned(string.into_bytes()),
        Cow::Borrowed(string) => Cow::Borrowed(string.as_bytes()),
    }
}

/// Remove the `./` prefix from a path.
pub fn strip_current_dir(path: &Path) -> &Path {
    path.strip_prefix(".").unwrap_or(path)
}

/// Default value for the path_separator, mainly for MSYS/MSYS2, which set the MSYSTEM
/// environment variable, and we set fd's path separator to '/' rather than Rust's default of '\'.
///
/// Returns Some to use a nonstandard path separator, or None to use rust's default on the target
/// platform.
pub fn default_path_separator() -> Option<String> {
    if cfg!(windows) {
        let msystem = env::var("MSYSTEM").ok()?;
        if !msystem.is_empty() {
            return Some("/".to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{paths_equal, strip_current_dir, strip_path_prefix};
    use std::path::Path;

    #[test]
    fn strip_current_dir_basic() {
        assert_eq!(strip_current_dir(Path::new("./foo")), Path::new("foo"));
        assert_eq!(strip_current_dir(Path::new("foo")), Path::new("foo"));
        assert_eq!(
            strip_current_dir(Path::new("./foo/bar/baz")),
            Path::new("foo/bar/baz")
        );
        assert_eq!(
            strip_current_dir(Path::new("foo/bar/baz")),
            Path::new("foo/bar/baz")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_prefix_is_case_insensitive() {
        let relative = strip_path_prefix(
            Path::new(r"C:\VulkanSDK\1.4.350.0\Bin\dxc.exe"),
            Path::new(r"c:\vulkansdk"),
        )
        .unwrap();
        assert_eq!(relative, Path::new(r"1.4.350.0\Bin\dxc.exe"));
        assert!(paths_equal(
            Path::new(r"C:\VulkanSDK"),
            Path::new(r"c:\vulkansdk")
        ));
    }
}
