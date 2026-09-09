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
    let path = std::env::var("PATH").ok();
    resolve_try_exec_with(program, path.as_deref())
}

/// Pure core of [`resolve_try_exec`] with an explicit `PATH` value.
///
/// `path` is split on `:`; empty components are ignored, so an unset, empty or
/// all-empty `PATH` resolves nothing (there is no implicit search of the current
/// directory). Relative `PATH` components are searched as given, like `execvp`.
pub fn resolve_try_exec_with(program: &str, path: Option<&str>) -> Option<PathBuf> {
    if program.trim().is_empty() {
        return None;
    }

    if program.contains('/') {
        let candidate = Path::new(program);
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }

    path?
        .split(':')
        .filter(|component| !component.is_empty())
        .map(|component| Path::new(component).join(program))
        .find(|candidate| is_executable(candidate))
}

/// `true` when `path` is a regular file with at least one execute bit set.
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    /// Writes a fixture file with an explicit mode (umask-independent).
    fn write(path: &Path, mode: u32) -> PathBuf {
        std::fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write fixture file");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("chmod fixture file");
        path.to_path_buf()
    }

    /// Creates `<root>/<name>` and returns it.
    fn bin_dir(root: &TempDir, name: &str) -> PathBuf {
        let dir = root.path().join(name);
        std::fs::create_dir_all(&dir).expect("create fixture bin dir");
        dir
    }

    #[test]
    fn is_executable_accepts_any_execute_bit() {
        let root = TempDir::new().expect("tempdir");
        for mode in [0o755, 0o700, 0o050, 0o005, 0o001, 0o111] {
            let path = write(&root.path().join(format!("prog{mode:o}")), mode);
            assert!(is_executable(&path), "mode {mode:o} should be executable");
        }
    }

    #[test]
    fn is_executable_rejects_directories_files_and_missing_paths() {
        let root = TempDir::new().expect("tempdir");
        let plain = write(&root.path().join("plain"), 0o644);
        assert!(!is_executable(&plain));

        let no_perm = write(&root.path().join("noperm"), 0o000);
        assert!(!is_executable(&no_perm));

        assert!(!is_executable(root.path()), "a directory is not executable");
        let subdir = root.path().join("subdir");
        std::fs::create_dir(&subdir).expect("create subdir");
        assert!(!is_executable(&subdir));

        assert!(!is_executable(&root.path().join("missing")));
        assert!(!is_executable(Path::new("")));
    }

    #[test]
    fn is_executable_follows_symlinks() {
        let root = TempDir::new().expect("tempdir");
        let target = write(&root.path().join("target"), 0o755);
        let link = root.path().join("link");
        symlink(&target, &link).expect("symlink");
        assert!(is_executable(&link));

        let dangling = root.path().join("dangling");
        symlink(root.path().join("nope"), &dangling).expect("symlink");
        assert!(!is_executable(&dangling));
    }

    #[test]
    fn resolves_the_first_executable_path_component() {
        let root = TempDir::new().expect("tempdir");
        let first = bin_dir(&root, "first");
        let second = bin_dir(&root, "second");
        write(&first.join("prog"), 0o755);
        write(&second.join("prog"), 0o755);

        let path = format!("{}:{}", first.display(), second.display());
        assert_eq!(
            resolve_try_exec_with("prog", Some(&path)),
            Some(first.join("prog"))
        );
    }

    #[test]
    fn skips_non_executable_and_missing_candidates() {
        let root = TempDir::new().expect("tempdir");
        let first = bin_dir(&root, "first");
        let second = bin_dir(&root, "second");
        let empty = bin_dir(&root, "empty");
        write(&first.join("prog"), 0o644);
        write(&second.join("prog"), 0o755);
        let path = format!(
            "{}:{}:{}",
            first.display(),
            empty.display(),
            second.display()
        );

        assert_eq!(
            resolve_try_exec_with("prog", Some(&path)),
            Some(second.join("prog"))
        );
        assert_eq!(resolve_try_exec_with("other", Some(&path)), None);
    }

    #[test]
    fn ignores_path_components_that_are_not_directories() {
        let root = TempDir::new().expect("tempdir");
        let first = bin_dir(&root, "first");
        let second = bin_dir(&root, "second");
        let first_prog = write(&first.join("prog"), 0o755);
        write(&second.join("prog"), 0o755);

        // A regular file as a PATH entry must not be searched as a directory.
        let path = format!("{}:{}", first_prog.display(), second.display());
        assert_eq!(
            resolve_try_exec_with("prog", Some(&path)),
            Some(second.join("prog"))
        );
    }

    #[test]
    fn ignores_empty_path_components() {
        let root = TempDir::new().expect("tempdir");
        let bin = bin_dir(&root, "bin");
        write(&bin.join("prog"), 0o755);

        for path in [
            format!(":{}", bin.display()),
            format!("{}:", bin.display()),
            format!(":{}:", bin.display()),
            format!("::{}::", bin.display()),
        ] {
            assert_eq!(
                resolve_try_exec_with("prog", Some(&path)),
                Some(bin.join("prog")),
                "PATH {path:?}"
            );
        }
    }

    #[test]
    fn empty_or_absent_path_values_resolve_nothing() {
        for path in [None, Some(""), Some(":"), Some("::")] {
            assert_eq!(resolve_try_exec_with("prog", path), None, "PATH {path:?}");
        }
    }

    #[test]
    fn programs_with_a_slash_are_used_directly() {
        let root = TempDir::new().expect("tempdir");
        let prog = write(&root.path().join("prog"), 0o755);
        let program = prog.to_str().expect("utf-8 fixture path");

        assert_eq!(
            resolve_try_exec_with(program, Some("/nonexistent")),
            Some(prog.clone())
        );
        assert_eq!(resolve_try_exec_with(program, None), Some(prog.clone()));

        let plain = write(&root.path().join("plain"), 0o644);
        assert_eq!(
            resolve_try_exec_with(plain.to_str().expect("utf-8"), Some("/nonexistent")),
            None
        );
        let missing = root.path().join("missing");
        assert_eq!(
            resolve_try_exec_with(missing.to_str().expect("utf-8"), None),
            None
        );
    }

    #[test]
    fn blank_programs_never_resolve() {
        let root = TempDir::new().expect("tempdir");
        let bin = bin_dir(&root, "bin");
        let path = bin.display().to_string();
        for program in ["", " ", "\t", "  \t "] {
            assert_eq!(
                resolve_try_exec_with(program, Some(&path)),
                None,
                "program {program:?}"
            );
        }
    }

    #[test]
    fn environment_wrapper_delegates_without_ambient_assertions() {
        let root = TempDir::new().expect("tempdir");
        let prog = write(&root.path().join("prog"), 0o755);
        assert_eq!(
            resolve_try_exec(prog.to_str().expect("utf-8 fixture path")),
            Some(prog)
        );

        assert_eq!(resolve_try_exec(""), None);
        assert_eq!(resolve_try_exec("   "), None);
        assert_eq!(
            resolve_try_exec("adesk-app-registry-no-such-try-exec"),
            None
        );
    }
}
