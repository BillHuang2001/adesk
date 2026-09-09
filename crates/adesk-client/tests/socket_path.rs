//! `default_socket_path()` resolution order (protocol §1).
//!
//! Resolution reads the process environment: `$ADESK_SOCKET`, then
//! `$XDG_RUNTIME_DIR/adesk.sock`, then `<system temp dir>/adesk.sock`. The
//! environment is process-global and this crate's suites never mutate it, so
//! every case below asserts in a **child** process of this test binary with a
//! controlled environment — `Command::env`/`env_remove` affect the child only.
//! That keeps the tests deterministic and race-free: the fallback branch is
//! only reachable with both variables unset, which the ambient environment
//! cannot be relied on to provide.

use std::process::Command;

/// Set in the re-executed child so it runs the assertion instead of spawning
/// another child.
const CHILD_MARKER: &str = "ADESK_CLIENT_SOCKET_PATH_CHILD";

/// Re-run this test binary for `test_name` (which then takes its child branch)
/// with `env` applied to the child only; panics with the child's output when it
/// fails.
fn run_in_child(test_name: &str, env: &[(&str, Option<&str>)]) {
    let mut command = Command::new(std::env::current_exe().expect("test binary path"));
    command.args(["--exact", test_name]).env(CHILD_MARKER, "1");
    for (key, value) in env {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let output = command
        .output()
        .expect("re-run the test binary in a child process");
    assert!(
        output.status.success(),
        "child `{test_name}` failed ({})\n--- stdout ---\n{}--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn default_socket_path_prefers_adesk_socket() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        assert_eq!(
            adesk_client::default_socket_path(),
            std::path::PathBuf::from("/tmp/custom.sock")
        );
        return;
    }
    run_in_child(
        "default_socket_path_prefers_adesk_socket",
        &[
            ("ADESK_SOCKET", Some("/tmp/custom.sock")),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ],
    );
}

#[test]
fn default_socket_path_falls_back_to_xdg_runtime_dir() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        assert_eq!(
            adesk_client::default_socket_path(),
            std::path::PathBuf::from("/run/user/1000").join("adesk.sock")
        );
        return;
    }
    run_in_child(
        "default_socket_path_falls_back_to_xdg_runtime_dir",
        &[
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ],
    );
}

#[test]
fn default_socket_path_falls_back_to_system_temp_dir() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        // Both higher-priority variables are unset, so the temp-directory
        // fallback is the only reachable branch; it must be the same
        // expression `adesk_server::default_socket_path()` returns.
        assert_eq!(
            adesk_client::default_socket_path(),
            std::env::temp_dir().join("adesk.sock")
        );
        return;
    }
    // Point the child's `$TMPDIR` at a private directory so the assertion fails
    // if the fallback is ever hard-coded instead of resolved.
    let temp_dir = tempfile::tempdir().expect("private temp dir");
    run_in_child(
        "default_socket_path_falls_back_to_system_temp_dir",
        &[
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", None),
            (
                "TMPDIR",
                Some(temp_dir.path().to_str().expect("utf-8 temp dir")),
            ),
        ],
    );
}
