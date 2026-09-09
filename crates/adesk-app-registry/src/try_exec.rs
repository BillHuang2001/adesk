//! `TryExec` availability resolution.
//!
//! Per the desktop-entry spec, `TryExec` names an executable that decides whether
//! the entry is installed. A value containing `/` is used as a path; otherwise it
//! is looked up in `$PATH`. The registry keeps unresolvable entries listed (they
//! are visible through `try_exec` in [`adesk_core::AppInfo`]) and reports
//! [`crate::Error::TryExecNotFound`] at launch time instead of hiding them.

use std::path::{Path, PathBuf};

/// Resolves `program` against the process `$PATH`.
///
/// Returns the path of the first executable match, or the given path when it
/// contains a `/` and is executable.
pub fn resolve_try_exec(program: &str) -> Option<PathBuf> {
    todo!("stub: implementation phase")
}

/// Pure core of [`resolve_try_exec`] with an explicit `PATH` value.
pub fn resolve_try_exec_with(program: &str, path: Option<&str>) -> Option<PathBuf> {
    todo!("stub: implementation phase")
}

/// `true` when `path` is a regular file with at least one execute bit set.
pub fn is_executable(path: &Path) -> bool {
    todo!("stub: implementation phase")
}
