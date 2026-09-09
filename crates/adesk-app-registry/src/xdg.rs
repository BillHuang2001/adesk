//! XDG base-directory resolution for application discovery.
//!
//! Implements the XDG Base Directory specification as used by
//! `docs/architecture.md` §7: `$XDG_DATA_HOME/applications` first, then each entry
//! of `$XDG_DATA_DIRS` in order. First directory wins on desktop-file id
//! collisions, so the returned order *is* the precedence order.

use std::path::{Path, PathBuf};

/// Default `XDG_DATA_DIRS` value when the variable is unset or empty.
pub const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// Subdirectory of every XDG data directory that holds `.desktop` files.
pub const APPLICATIONS_SUBDIR: &str = "applications";

/// Search directories for `.desktop` files, in precedence order, read from the
/// process environment.
///
/// Reads `$XDG_DATA_HOME`, `$XDG_DATA_DIRS` and `$HOME` and delegates to
/// [`search_dirs_with`].
pub fn search_dirs() -> Vec<PathBuf> {
    todo!("stub: implementation phase")
}

/// Pure core of [`search_dirs`], for tests and explicit configuration.
///
/// Rules (XDG base directory spec):
/// - `data_home` is used when it is an absolute path, otherwise `home/.local/share`
///   is used when `home` is known; otherwise the home base is skipped.
/// - `data_dirs` is split on `:`, empty and relative entries are ignored, and
///   [`DEFAULT_DATA_DIRS`] is used when the value is `None` or contains no usable
///   entries.
/// - [`APPLICATIONS_SUBDIR`] is appended to every base directory.
/// - Duplicates are removed, keeping the first occurrence.
pub fn search_dirs_with(
    data_home: Option<&Path>,
    data_dirs: Option<&str>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    todo!("stub: implementation phase")
}
