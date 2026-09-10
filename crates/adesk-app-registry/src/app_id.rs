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

use std::path::{Component, Path};

use adesk_core::AppId;

/// File extension of desktop entries, including the dot.
pub const DESKTOP_EXTENSION: &str = ".desktop";

/// `true` when `path`'s file name ends with `.desktop` (case-sensitive).
pub fn is_desktop_file(path: &Path) -> bool {
    // `as_encoded_bytes` is exact for ASCII suffixes and also covers non-UTF-8
    // file names, which `desktop_file_id` then rejects.
    path.file_name().is_some_and(|name| {
        name.as_encoded_bytes()
            .ends_with(DESKTOP_EXTENSION.as_bytes())
    })
}

/// Derives the desktop-file id for `path` found under `applications_dir`.
///
/// Returns `None` when `path` is not below `applications_dir`, does not end in
/// `.desktop`, has no id component (the file name is exactly `.desktop`, or the
/// path resolves to `applications_dir` itself), or is not valid UTF-8.
pub fn desktop_file_id(applications_dir: &Path, path: &Path) -> Option<AppId> {
    let relative = path.strip_prefix(applications_dir).ok()?;

    // Collect the directory components and the file name; `.` is redundant and
    // anything else (`..`, a root, a prefix) means `path` is not really below
    // `applications_dir`.
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => segments.push(segment.to_str()?),
            Component::CurDir => continue,
            _ => return None,
        }
    }

    let (file_name, directories) = segments.split_last()?;
    let stem = file_name.strip_suffix(DESKTOP_EXTENSION)?;
    if stem.is_empty() {
        return None;
    }

    let mut id = String::with_capacity(relative.as_os_str().len());
    for directory in directories {
        id.push_str(directory);
        id.push('.');
    }
    id.push_str(stem);

    Some(AppId::from(id))
}
