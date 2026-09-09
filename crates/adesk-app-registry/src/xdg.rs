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
/// [`search_dirs_with`]. An unset variable and an empty value are equivalent.
pub fn search_dirs() -> Vec<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").map(|value| value.to_string_lossy().into_owned());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    search_dirs_with(data_home.as_deref(), data_dirs.as_deref(), home.as_deref())
}

/// Subdirectory of `$HOME` used when `$XDG_DATA_HOME` is unusable.
const DEFAULT_DATA_HOME_SUBDIR: &str = ".local/share";

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
    let mut dirs = Vec::new();

    if let Some(base) = home_base(data_home, home) {
        push_unique(&mut dirs, base.join(APPLICATIONS_SUBDIR));
    }

    let mut saw_usable_entry = false;
    for entry in data_dirs.unwrap_or(DEFAULT_DATA_DIRS).split(':') {
        if entry.is_empty() {
            continue;
        }
        let base = Path::new(entry);
        if !base.is_absolute() {
            continue;
        }
        saw_usable_entry = true;
        push_unique(&mut dirs, base.join(APPLICATIONS_SUBDIR));
    }

    if !saw_usable_entry {
        for entry in DEFAULT_DATA_DIRS.split(':') {
            push_unique(&mut dirs, Path::new(entry).join(APPLICATIONS_SUBDIR));
        }
    }

    dirs
}

/// Base directory holding the user's data, if it can be determined.
fn home_base(data_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(base) = data_home {
        if base.is_absolute() {
            return Some(base.to_path_buf());
        }
    }
    let home = home?;
    if !home.is_absolute() {
        return None;
    }
    Some(home.join(DEFAULT_DATA_HOME_SUBDIR))
}

/// Appends `dir` unless an equal entry is already present (first occurrence wins).
fn push_unique(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders paths as strings so failures are readable.
    fn strings(dirs: &[PathBuf]) -> Vec<String> {
        dirs.iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn defaults_when_nothing_is_configured() {
        assert_eq!(
            strings(&search_dirs_with(None, None, None)),
            ["/usr/local/share/applications", "/usr/share/applications"]
        );
    }

    #[test]
    fn data_home_precedes_data_dirs() {
        assert_eq!(
            strings(&search_dirs_with(
                Some(Path::new("/home/u/.local/share")),
                Some("/opt/share:/usr/share"),
                Some(Path::new("/home/u")),
            )),
            [
                "/home/u/.local/share/applications",
                "/opt/share/applications",
                "/usr/share/applications",
            ]
        );
    }

    #[test]
    fn data_home_must_be_absolute_to_be_used() {
        for data_home in [
            None,
            Some(Path::new("")),
            Some(Path::new("relative/share")),
            Some(Path::new("./local/share")),
        ] {
            assert_eq!(
                strings(&search_dirs_with(
                    data_home,
                    Some("/opt/share"),
                    Some(Path::new("/home/u")),
                )),
                [
                    "/home/u/.local/share/applications",
                    "/opt/share/applications"
                ],
                "data_home {data_home:?}"
            );
        }
    }

    #[test]
    fn home_base_is_skipped_without_an_absolute_home() {
        for home in [None, Some(Path::new("")), Some(Path::new("home/u"))] {
            assert_eq!(
                strings(&search_dirs_with(None, Some("/opt/share"), home)),
                ["/opt/share/applications"],
                "home {home:?}"
            );
        }
    }

    #[test]
    fn empty_and_relative_data_dir_entries_are_ignored() {
        assert_eq!(
            strings(&search_dirs_with(
                None,
                Some(":/opt/share::relative/dir:/usr/share/"),
                None,
            )),
            ["/opt/share/applications", "/usr/share/applications"]
        );
    }

    #[test]
    fn data_dirs_without_usable_entries_fall_back_to_defaults() {
        for data_dirs in [
            Some(""),
            Some(":"),
            Some("::"),
            Some("relative"),
            Some(":relative:"),
        ] {
            assert_eq!(
                strings(&search_dirs_with(Some(Path::new("/x")), data_dirs, None)),
                [
                    "/x/applications",
                    "/usr/local/share/applications",
                    "/usr/share/applications",
                ],
                "data_dirs {data_dirs:?}"
            );
        }
    }

    #[test]
    fn an_unset_data_dirs_uses_the_defaults() {
        assert_eq!(
            strings(&search_dirs_with(None, None, Some(Path::new("/home/u")))),
            [
                "/home/u/.local/share/applications",
                "/usr/local/share/applications",
                "/usr/share/applications",
            ]
        );
    }

    #[test]
    fn trailing_slashes_do_not_produce_double_separators() {
        assert_eq!(
            strings(&search_dirs_with(
                Some(Path::new("/home/u/.local/share/")),
                Some("/opt/share//"),
                None,
            )),
            [
                "/home/u/.local/share/applications",
                "/opt/share//applications"
            ]
        );
    }

    #[test]
    fn duplicate_bases_are_removed_keeping_the_first_occurrence() {
        assert_eq!(
            strings(&search_dirs_with(
                Some(Path::new("/opt/share")),
                Some("/opt/share:/usr/share"),
                None,
            )),
            ["/opt/share/applications", "/usr/share/applications"]
        );
        assert_eq!(
            strings(&search_dirs_with(
                None,
                Some("/usr/local/share:/usr/share:/usr/local/share"),
                Some(Path::new("/home/u")),
            )),
            [
                "/home/u/.local/share/applications",
                "/usr/local/share/applications",
                "/usr/share/applications",
            ]
        );
    }

    #[test]
    fn the_root_base_yields_a_top_level_applications_dir() {
        assert_eq!(
            strings(&search_dirs_with(None, Some("/"), None)),
            ["/applications"]
        );
    }

    #[test]
    fn search_dirs_from_the_environment_is_absolute_and_applications_scoped() {
        // The ambient environment is not asserted: the wrapper must only produce
        // absolute `<base>/applications` entries.
        let dirs = search_dirs();
        assert!(!dirs.is_empty());
        for dir in &dirs {
            assert!(dir.is_absolute(), "{} is not absolute", dir.display());
            assert!(
                dir.ends_with(APPLICATIONS_SUBDIR),
                "{} is not an applications dir",
                dir.display()
            );
        }
    }
}
