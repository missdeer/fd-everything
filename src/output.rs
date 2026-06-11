use std::borrow::Cow;
use std::io::{self, Write};

use lscolors::{Indicator, LsColors, Style};

use crate::config::Config;
use crate::dir_entry::DirEntry;
use crate::fmt::FormatTemplate;
use crate::hyperlink::PathUrl;
use crate::sanitize::maybe_sanitize;

fn replace_path_separator(path: &str, new_path_separator: &str) -> String {
    path.replace(std::path::MAIN_SEPARATOR, new_path_separator)
}

// TODO: this function is performance critical and can probably be optimized
pub fn print_entry<W: Write>(stdout: &mut W, entry: &DirEntry, config: &Config) -> io::Result<()> {
    // PLAN.md §2.X: hyperlink target uses the absolute raw_path (terminals
    // need a fully-qualified file:// URL to open it), display text uses the
    // projected form. See PathUrl::new — it canonicalizes again internally,
    // but raw_path is already absolute so that call is effectively a no-op.
    let mut has_hyperlink = false;
    if config.hyperlink
        && let Some(url) = PathUrl::new(entry.path())
    {
        write!(stdout, "\x1B]8;;{url}\x1B\\")?;
        has_hyperlink = true;
    }

    let projector = config.path_projector();
    let projected = projector.project_for_output(entry.path());

    if let Some(ref format) = config.format {
        print_entry_format(stdout, entry, config, &projected, format)?;
    } else if let Some(ref ls_colors) = config.ls_colors {
        print_entry_colorized(stdout, entry, config, &projected, ls_colors)?;
    } else {
        print_entry_uncolorized(stdout, entry, config, &projected)?;
    };

    if has_hyperlink {
        write!(stdout, "\x1B]8;;\x1B\\")?;
    }

    if config.null_separator {
        write!(stdout, "\0")
    } else {
        writeln!(stdout)
    }
}

// Display a trailing slash if the path is a directory and the config option is enabled.
// If the path_separator option is set, display that instead.
// The trailing slash will not be colored.
#[inline]
fn print_trailing_slash<W: Write>(
    stdout: &mut W,
    entry: &DirEntry,
    config: &Config,
    style: Option<&Style>,
) -> io::Result<()> {
    if entry.is_directory_for_display() {
        write!(
            stdout,
            "{}",
            style
                .map(Style::to_nu_ansi_term_style)
                .unwrap_or_default()
                .paint(&config.actual_path_separator)
        )?;
    }
    Ok(())
}

// TODO: this function is performance critical and can probably be optimized
fn print_entry_format<W: Write>(
    stdout: &mut W,
    _entry: &DirEntry,
    config: &Config,
    projected: &std::path::Path,
    format: &FormatTemplate,
) -> io::Result<()> {
    let output = format.generate(projected, config.path_separator.as_deref());
    // TODO: support writing raw bytes on unix?
    let s = output.to_string_lossy();
    write!(
        stdout,
        "{}",
        maybe_sanitize(&s, config.interactive_terminal)
    )
}

// TODO: this function is performance critical and can probably be optimized
fn print_entry_colorized<W: Write>(
    stdout: &mut W,
    entry: &DirEntry,
    config: &Config,
    projected: &std::path::Path,
    ls_colors: &LsColors,
) -> io::Result<()> {
    let mut offset = 0;
    let path = projected;
    let path_str = path.to_string_lossy();

    if let Some(parent) = path.parent() {
        offset = parent.to_string_lossy().len();
        for c in path_str[offset..].chars() {
            if std::path::is_separator(c) {
                offset += c.len_utf8();
            } else {
                break;
            }
        }
    }

    if offset > 0 {
        let mut parent_str = Cow::from(&path_str[..offset]);
        if let Some(ref separator) = config.path_separator {
            *parent_str.to_mut() = replace_path_separator(&parent_str, separator);
        }

        let style = ls_colors
            .style_for_indicator(Indicator::Directory)
            .map(Style::to_nu_ansi_term_style)
            .unwrap_or_default();
        let safe_parent = maybe_sanitize(&parent_str, config.interactive_terminal);
        write!(stdout, "{}", style.paint(safe_parent.as_ref()))?;
    }

    let style = entry
        .style(ls_colors)
        .map(Style::to_nu_ansi_term_style)
        .unwrap_or_default();
    let safe_basename = maybe_sanitize(&path_str[offset..], config.interactive_terminal);
    write!(stdout, "{}", style.paint(safe_basename.as_ref()))?;

    print_trailing_slash(
        stdout,
        entry,
        config,
        ls_colors.style_for_indicator(Indicator::Directory),
    )?;

    Ok(())
}

// TODO: this function is performance critical and can probably be optimized
fn print_entry_uncolorized_base<W: Write>(
    stdout: &mut W,
    entry: &DirEntry,
    config: &Config,
    projected: &std::path::Path,
) -> io::Result<()> {
    let mut path_string = projected.to_string_lossy();
    if let Some(ref separator) = config.path_separator {
        *path_string.to_mut() = replace_path_separator(&path_string, separator);
    }
    let safe = maybe_sanitize(&path_string, config.interactive_terminal);
    write!(stdout, "{safe}")?;
    print_trailing_slash(stdout, entry, config, None)
}

#[cfg(not(unix))]
fn print_entry_uncolorized<W: Write>(
    stdout: &mut W,
    entry: &DirEntry,
    config: &Config,
    projected: &std::path::Path,
) -> io::Result<()> {
    print_entry_uncolorized_base(stdout, entry, config, projected)
}

#[cfg(unix)]
fn print_entry_uncolorized<W: Write>(
    stdout: &mut W,
    entry: &DirEntry,
    config: &Config,
    projected: &std::path::Path,
) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    if config.interactive_terminal || config.path_separator.is_some() {
        print_entry_uncolorized_base(stdout, entry, config, projected)
    } else {
        // Piped output: raw bytes so invalid UTF-8 filenames reach downstream tools intact.
        stdout.write_all(projected.as_os_str().as_bytes())?;
        print_trailing_slash(stdout, entry, config, None)
    }
}
