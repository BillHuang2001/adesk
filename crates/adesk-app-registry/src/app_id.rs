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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    use super::*;

    const APPS: &str = "/usr/share/applications";

    fn id(relative: &str) -> Option<String> {
        let apps = Path::new(APPS);
        desktop_file_id(apps, &apps.join(relative)).map(|id| id.0)
    }

    #[test]
    fn derives_ids_from_the_documented_table() {
        let cases = [
            ("org.mozilla.firefox.desktop", "org.mozilla.firefox"),
            ("code.desktop", "code"),
            ("kde/kate.desktop", "kde.kate"),
            ("foo/bar.baz.desktop", "foo.bar.baz"),
            ("a/b/c/deep.desktop", "a.b.c.deep"),
            // Dashes are id characters, never separators.
            ("my-app.desktop", "my-app"),
            (
                "gnome-terminal/gnome-terminal.desktop",
                "gnome-terminal.gnome-terminal",
            ),
            (
                "libre-office/libre-office-writer.desktop",
                "libre-office.libre-office-writer",
            ),
            // Dots in directory names survive.
            ("org.kde/kate.desktop", "org.kde.kate"),
        ];
        for (relative, expected) in cases {
            assert_eq!(id(relative).as_deref(), Some(expected), "{relative}");
        }
    }

    #[test]
    fn rejects_paths_outside_the_applications_dir() {
        let apps = Path::new(APPS);
        for path in [
            PathBuf::from("/usr/share/other/kate.desktop"),
            PathBuf::from("/usr/share/applications-backup/kate.desktop"),
            PathBuf::from("/etc/kate.desktop"),
            PathBuf::from("/usr/share"),
        ] {
            assert_eq!(desktop_file_id(apps, &path), None, "{}", path.display());
        }
    }

    #[test]
    fn rejects_paths_without_the_desktop_suffix() {
        for relative in [
            "kate.txt",
            "kate",
            "kate.desktop.bak",
            "kate.Desktop",
            "kde/kate",
        ] {
            assert_eq!(id(relative), None, "{relative}");
        }
    }

    #[test]
    fn rejects_paths_without_an_id_component() {
        for relative in [".desktop", "kde/.desktop", ""] {
            assert_eq!(id(relative), None, "{relative}");
        }
        let apps = Path::new(APPS);
        assert_eq!(desktop_file_id(apps, apps), None);
    }

    #[test]
    fn rejects_parent_directory_escapes() {
        let apps = Path::new(APPS);
        assert_eq!(desktop_file_id(apps, &apps.join("../kate.desktop")), None);
        assert_eq!(
            desktop_file_id(apps, &apps.join("kde/../../kate.desktop")),
            None
        );
    }

    #[test]
    fn skips_redundant_current_directory_components() {
        assert_eq!(id("./kate.desktop").as_deref(), Some("kate"));
        assert_eq!(id("kde/./kate.desktop").as_deref(), Some("kde.kate"));
    }

    #[test]
    fn handles_relative_application_dirs() {
        assert_eq!(
            desktop_file_id(Path::new("apps"), Path::new("apps/kde/kate.desktop"))
                .map(|id| id.0)
                .as_deref(),
            Some("kde.kate")
        );
    }

    #[test]
    fn rejects_non_utf8_paths() {
        let apps = Path::new(APPS);
        let weird_name = apps.join(OsStr::from_bytes(b"\xff.desktop"));
        assert_eq!(desktop_file_id(apps, &weird_name), None);

        let weird_dir = apps
            .join(OsStr::from_bytes(b"k\xffde"))
            .join("kate.desktop");
        assert_eq!(desktop_file_id(apps, &weird_dir), None);
    }

    #[test]
    fn is_desktop_file_checks_the_case_sensitive_suffix() {
        for path in [
            "kate.desktop",
            "kde/kate.desktop",
            ".desktop",
            "/x/y/kate.desktop",
        ] {
            assert!(is_desktop_file(Path::new(path)), "{path}");
        }
        for path in [
            "kate.Desktop",
            "kate.desktop.bak",
            "kate.deskt",
            "desktop",
            "kate.desktop/x",
            "/",
            ".",
            "..",
        ] {
            assert!(!is_desktop_file(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn is_desktop_file_accepts_non_utf8_names_with_the_suffix() {
        let name = OsStr::from_bytes(b"k\xffte.desktop");
        assert!(is_desktop_file(&PathBuf::from(name)));
        let without_suffix = OsStr::from_bytes(b"k\xffte.txt");
        assert!(!is_desktop_file(&PathBuf::from(without_suffix)));
    }

    #[test]
    fn is_desktop_file_agrees_with_id_derivation() {
        for relative in [
            "kate.desktop",
            "kde/kate.desktop",
            "kate.txt",
            "kate.Desktop",
        ] {
            let apps = Path::new(APPS);
            let path = apps.join(relative);
            assert_eq!(
                is_desktop_file(&path),
                id(relative).is_some(),
                "{relative} mismatch"
            );
        }
    }
}
