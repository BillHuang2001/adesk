//! Integration tests for desktop-file id derivation.
//!
//! Public API only: the table pins the documented mapping (relative path →
//! dotted id) and the rejection rules for paths that are not a desktop entry
//! below the applications directory.

mod support;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use adesk_app_registry::app_id::DESKTOP_EXTENSION;
use adesk_app_registry::{desktop_file_id, is_desktop_file};

use support::{app, write_entry};

const APPS: &str = "/usr/share/applications";

fn id(relative: &str) -> Option<String> {
    let apps = Path::new(APPS);
    desktop_file_id(apps, &apps.join(relative)).map(|id| id.0)
}

fn id_at(apps: &Path, path: &Path) -> Option<String> {
    desktop_file_id(apps, path).map(|id| id.0)
}

#[test]
fn documented_id_table_on_the_real_filesystem() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        ("org.mozilla.firefox.desktop", "org.mozilla.firefox"),
        ("code.desktop", "code"),
        ("kde/kate.desktop", "kde.kate"),
        ("foo/bar.baz.desktop", "foo.bar.baz"),
        ("a/b/c/deep.desktop", "a.b.c.deep"),
        ("my-app.desktop", "my-app"),
        (
            "gnome-terminal/gnome-terminal.desktop",
            "gnome-terminal.gnome-terminal",
        ),
        (
            "libre-office/libre-office-writer.desktop",
            "libre-office.libre-office-writer",
        ),
        ("org.kde/kate.desktop", "org.kde.kate"),
        ("dir.with.dots/app.desktop", "dir.with.dots.app"),
        ("UPPER/Case.desktop", "UPPER.Case"),
    ];
    for (relative, expected) in cases {
        let path = write_entry(dir.path(), relative, "Type=Application\nName=X\n");
        assert_eq!(
            id_at(dir.path(), &path).as_deref(),
            Some(expected),
            "{relative}"
        );
        assert!(is_desktop_file(&path), "{relative}");
        assert_eq!(id_at(dir.path(), &path), Some(app(expected).0));
    }
}

#[test]
fn extension_constant_matches_the_suffix() {
    assert_eq!(DESKTOP_EXTENSION, ".desktop");
    for path in ["a.desktop", "kde/kate.desktop"] {
        assert!(
            path.ends_with(DESKTOP_EXTENSION),
            "{path} must end with {DESKTOP_EXTENSION}"
        );
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
        PathBuf::from("/"),
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
        "kate.deskt",
    ] {
        assert_eq!(id(relative), None, "{relative}");
    }
}

#[test]
fn rejects_paths_without_an_id_component() {
    for relative in [".desktop", "kde/.desktop", ""] {
        assert_eq!(id(relative), None, "{relative:?}");
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
    assert_eq!(
        desktop_file_id(apps, &apps.join("kde/../kate.desktop")),
        None
    );
}

#[test]
fn skips_redundant_current_directory_components() {
    assert_eq!(id("./kate.desktop").as_deref(), Some("kate"));
    assert_eq!(id("kde/./kate.desktop").as_deref(), Some("kde.kate"));
}

#[test]
fn handles_relative_applications_dirs() {
    assert_eq!(
        desktop_file_id(Path::new("apps"), Path::new("apps/kde/kate.desktop"))
            .map(|id| id.0)
            .as_deref(),
        Some("kde.kate")
    );
    assert_eq!(
        desktop_file_id(Path::new("."), Path::new("./kate.desktop"))
            .map(|id| id.0)
            .as_deref(),
        Some("kate")
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
fn is_desktop_file_matrix() {
    for path in [
        "kate.desktop",
        "kde/kate.desktop",
        ".desktop",
        "/x/y/kate.desktop",
        "kate.DESKTOP.desktop",
    ] {
        assert!(is_desktop_file(Path::new(path)), "{path}");
    }
    for path in [
        "kate.Desktop",
        "kate.DESKTOP",
        "kate.desktop.bak",
        "kate.deskt",
        "desktop",
        "kate.desktop/x",
        "/",
        ".",
        "..",
        "",
    ] {
        assert!(!is_desktop_file(Path::new(path)), "{path}");
    }
}

#[test]
fn is_desktop_file_accepts_non_utf8_names_with_the_suffix() {
    assert!(is_desktop_file(&PathBuf::from(OsStr::from_bytes(
        b"k\xffte.desktop"
    ))));
    assert!(!is_desktop_file(&PathBuf::from(OsStr::from_bytes(
        b"k\xffte.txt"
    ))));
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

    // The only divergence: `.desktop` has the suffix but no id component, so
    // `is_desktop_file` accepts it while `desktop_file_id` rejects it.
    assert!(is_desktop_file(Path::new(".desktop")));
    assert_eq!(id(".desktop"), None);
}

#[test]
fn derivation_is_lexical_and_ignores_the_file_system() {
    let dir = tempfile::tempdir().expect("tempdir");
    let apps = dir.path();
    // No file exists at this path, yet the id derives from the path alone.
    let missing = apps.join("kde").join("kate.desktop");
    assert!(!missing.exists());
    assert_eq!(id_at(apps, &missing).as_deref(), Some("kde.kate"));

    // A directory named `*.desktop` also derives an id: derivation is lexical.
    let directory = apps.join("looks-like.desktop");
    std::fs::create_dir(&directory).expect("create directory");
    assert!(is_desktop_file(&directory));
    assert_eq!(id_at(apps, &directory).as_deref(), Some("looks-like"));
}
