//! Desktop-file id derivation.
//!
//! The id is the path of the `.desktop` file relative to the applications
//! directory it was found in, with the `.desktop` suffix removed and `/` replaced
//! by `.` (`docs/architecture.md` §7):
//!
//! | File | Id |
//! |---|---|
//! | `applications/org.mozilla.firefox.desktop` | `org.mozilla.firefox` |
//! | `applications/code.desktop` | `code` |
//! | `applications/kde/kate.desktop` | `kde.kate` |
//! | `applications/foo/bar.baz.desktop` | `foo.bar.baz` |

use std::path::Path;

use adesk_core::AppId;

/// File extension of desktop entries, including the dot.
pub const DESKTOP_EXTENSION: &str = ".desktop";

/// `true` when `path`'s file name ends with `.desktop` (case-sensitive).
#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
pub fn is_desktop_file(path: &Path) -> bool {
    todo!("stub: implementation phase")
}

/// Derives the desktop-file id for `path` found under `applications_dir`.
///
/// Returns `None` when `path` is not below `applications_dir`, does not end in
/// `.desktop`, has no id component, or is not valid UTF-8.
#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
pub fn desktop_file_id(applications_dir: &Path, path: &Path) -> Option<AppId> {
    todo!("stub: implementation phase")
}
