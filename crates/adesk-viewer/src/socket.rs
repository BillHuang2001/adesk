//! Viewer-socket resolution for VAP clients (`docs/viewer.md` §1).
//!
//! One helper answers "which Unix socket is the runtime's viewer endpoint on"
//! for every VAP client — the headless `adesk-viewer` binary today,
//! `adesk-viewer-gui` next — so a viewer never dials the AGP socket by mistake
//! (the server closes such a connection as an undecodable frame). The default is
//! the exact mirror of the server's bind logic: the sibling of the server's
//! default AGP socket, which is where a default-configured `adesk-server` binds
//! its viewer endpoint.
//!
//! `adesk-server` depends on this crate, so the server's derivation cannot be
//! called from here; the two small functions it mirrors
//! (`adesk_server::config::{default_socket_path, viewer_socket_sibling}`) are
//! kept in lockstep by mirror tests on both sides.
//!
//! The resolver is pure environment reading — it never touches the filesystem.
//! Binding the socket is the server's job; connecting to it is the client's.

use std::path::{Path, PathBuf};

/// The Unix socket file name of the VAP viewer endpoint (`docs/viewer.md` §1):
/// `$XDG_RUNTIME_DIR/adesk-viewer.sock` by default.
pub const VIEWER_SOCKET_FILE_NAME: &str = "adesk-viewer.sock";

/// The Unix socket file name of the AGP server, whose sibling carries the viewer
/// endpoint.
const AGP_SOCKET_FILE_NAME: &str = "adesk.sock";

/// Resolves the Unix socket path of the runtime's viewer (VAP) endpoint, in order:
///
/// 1. `explicit` — the client's `--unix <PATH>` flag; it always wins;
/// 2. `$ADESK_VIEWER_SOCKET` — the same environment fallback the server's
///    `--viewer-socket` flag reads;
/// 3. the sibling of the server's default AGP socket ([`viewer_socket_sibling`]):
///    `$ADESK_SOCKET`'s sibling when set, else `$XDG_RUNTIME_DIR/adesk-viewer.sock`,
///    else `<system temp dir>/adesk-viewer.sock` — exactly where a
///    default-configured `adesk-server` binds its viewer endpoint.
///
/// Environment variables are read with bare `std::env::var_os`, exactly like the
/// server's own derivation: a set-but-empty variable counts as set, so client and
/// server derive the same (broken) path under the same environment instead of
/// silently diverging.
///
/// This is pure environment resolution; the returned path is never checked
/// against the filesystem. TCP targets bypass it entirely (`--tcp` /
/// `ViewerTarget::Tcp`).
pub fn resolve_socket_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    if let Some(path) = std::env::var_os("ADESK_VIEWER_SOCKET") {
        return PathBuf::from(path);
    }
    viewer_socket_sibling(&default_agp_socket_path())
}

/// The server's default AGP socket path, the exact mirror of
/// `adesk_server::config::default_socket_path`: `$ADESK_SOCKET`, else
/// `$XDG_RUNTIME_DIR/adesk.sock`, else `<system temp dir>/adesk.sock`.
fn default_agp_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ADESK_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join(AGP_SOCKET_FILE_NAME);
    }
    std::env::temp_dir().join(AGP_SOCKET_FILE_NAME)
}

/// Derives the viewer socket path beside the AGP socket `agp_socket_path`:
/// `…/adesk.sock` → `…/adesk-viewer.sock`. The file stem gains a `-viewer`
/// suffix; directory and extension are preserved. A path with no file name (or
/// an empty stem) yields `adesk-viewer.sock` in the same directory.
///
/// This is the exact rule `adesk_server::config::viewer_socket_sibling` applies
/// when the server derives its viewer endpoint from its AGP socket; the mirror
/// is asserted by tests on both sides.
pub fn viewer_socket_sibling(agp_socket_path: &Path) -> PathBuf {
    let name = match agp_socket_path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) if !stem.is_empty() => {
            let mut name = format!("{stem}-viewer");
            if let Some(extension) = agp_socket_path.extension() {
                name.push('.');
                name.push_str(&extension.to_string_lossy());
            }
            name
        }
        _ => VIEWER_SOCKET_FILE_NAME.to_owned(),
    };
    agp_socket_path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    /// Serializes the tests that mutate the process-global environment: the
    /// resolver reads it with `std::env::var_os`, which is process-global state.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Holds [`ENV_LOCK`] for the test body. Poisoning is tolerated: a failing
    /// test has already reported its own assertion, and the others must still run.
    fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Overrides a set of environment variables for the test body, restoring
    /// their prior state (present or absent) on drop.
    struct EnvGuard {
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        /// Sets every `name` to `value`, or removes it when `value` is `None`,
        /// remembering the prior state.
        fn set(vars: &[(&'static str, Option<&str>)]) -> EnvGuard {
            let mut previous = Vec::with_capacity(vars.len());
            for (name, _) in vars {
                previous.push((*name, std::env::var_os(name)));
            }
            for (name, value) in vars {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            EnvGuard { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, previous) in self.previous.drain(..) {
                match previous {
                    Some(previous) => std::env::set_var(name, previous),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn sibling_suffixes_the_file_stem() {
        assert_eq!(
            viewer_socket_sibling(Path::new("/run/user/1000/adesk.sock")),
            PathBuf::from("/run/user/1000/adesk-viewer.sock")
        );
    }

    #[test]
    fn sibling_preserves_the_directory() {
        assert_eq!(
            viewer_socket_sibling(Path::new("/tmp/nested/deeper/adesk.sock")),
            PathBuf::from("/tmp/nested/deeper/adesk-viewer.sock")
        );
    }

    #[test]
    fn sibling_preserves_the_extension() {
        // The suffix goes on the stem, so a multi-dot name keeps its last
        // extension — the same rule `adesk_server::config` applies.
        assert_eq!(
            viewer_socket_sibling(Path::new("/tmp/adesk.v2.sock")),
            PathBuf::from("/tmp/adesk.v2-viewer.sock")
        );
        assert_eq!(
            viewer_socket_sibling(Path::new("/run/desk.custom")),
            PathBuf::from("/run/desk-viewer.custom")
        );
    }

    #[test]
    fn sibling_handles_a_path_without_a_file_name() {
        // No file name (or an empty stem): the canonical default name in the
        // same directory.
        assert_eq!(
            viewer_socket_sibling(Path::new("")),
            PathBuf::from("adesk-viewer.sock")
        );
        assert_eq!(
            viewer_socket_sibling(Path::new("/")),
            PathBuf::from("/adesk-viewer.sock")
        );
    }

    #[test]
    fn explicit_flag_wins_over_every_environment() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", Some("/env/viewer.sock")),
            ("ADESK_SOCKET", Some("/env/agp.sock")),
            ("XDG_RUNTIME_DIR", Some("/env/xdg")),
        ]);
        assert_eq!(
            resolve_socket_path(Some(PathBuf::from("/flag/viewer.sock"))),
            PathBuf::from("/flag/viewer.sock")
        );
    }

    #[test]
    fn viewer_socket_env_beats_the_derived_default() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", Some("/env/viewer.sock")),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/env/xdg")),
        ]);
        assert_eq!(resolve_socket_path(None), PathBuf::from("/env/viewer.sock"));
    }

    #[test]
    fn default_follows_the_agp_socket_env_to_its_sibling() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", None),
            ("ADESK_SOCKET", Some("/run/custom/agp.sock")),
            ("XDG_RUNTIME_DIR", Some("/env/xdg")),
        ]);
        // A custom `$ADESK_SOCKET` moves the server's viewer endpoint to that
        // socket's sibling, so the client must follow it instead of the XDG
        // default — exactly what the server's derivation does.
        assert_eq!(
            resolve_socket_path(None),
            PathBuf::from("/run/custom/agp-viewer.sock")
        );
    }

    #[test]
    fn default_uses_the_xdg_runtime_dir() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", None),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ]);
        assert_eq!(
            resolve_socket_path(None),
            PathBuf::from("/run/user/1000/adesk-viewer.sock")
        );
    }

    #[test]
    fn default_falls_back_to_the_temp_dir() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", None),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", None),
        ]);
        assert_eq!(
            resolve_socket_path(None),
            std::env::temp_dir().join(VIEWER_SOCKET_FILE_NAME)
        );
    }

    #[test]
    fn empty_env_vars_count_as_set_for_server_parity() {
        let _lock = lock_env();
        // An empty `$ADESK_VIEWER_SOCKET` is set for the server's clap `env`
        // too (plain `var_os`), so both sides resolve the same broken path
        // instead of silently skipping it.
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", Some("")),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ]);
        assert_eq!(resolve_socket_path(None), PathBuf::from(""));

        // Same for the AGP chain: an empty `$XDG_RUNTIME_DIR` yields the
        // relative sibling of `adesk.sock`, exactly like the server's default
        // derivation.
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", None),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("")),
        ]);
        assert_eq!(
            resolve_socket_path(None),
            PathBuf::from(VIEWER_SOCKET_FILE_NAME)
        );
    }
}
