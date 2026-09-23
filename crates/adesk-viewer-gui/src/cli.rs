//! Command-line parsing and viewer-endpoint resolution.
//!
//! GTK-free and unit-testable through [`clap::Parser::try_parse_from`]. The flag
//! names and help text mirror the headless `adesk-viewer` binary
//! (`crates/adesk-viewer/src/main.rs`) so a human can move between the two
//! front-ends without relearning the transport flags — and the default Unix
//! socket comes from the same [`adesk_viewer::resolve_socket_path`] the headless
//! viewer uses, so both dial the socket a default-configured `adesk-server`
//! actually binds its viewer endpoint on (never the AGP socket by omission).

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use crate::error::{GuiError, Result};

/// `adesk-viewer-gui` — a GTK4/libadwaita Viewer Attachment Protocol client.
#[derive(Debug, Parser)]
#[command(name = "adesk-viewer-gui", version, long_about = None)]
pub struct Cli {
    /// VAP viewer Unix socket to connect to — the server's viewer endpoint,
    /// e.g. $XDG_RUNTIME_DIR/adesk-viewer.sock — NOT the AGP socket
    /// (adesk.sock; a connection there is closed as an undecodable frame).
    /// Default: $ADESK_VIEWER_SOCKET, else the sibling of the server's default
    /// AGP socket.
    #[arg(long, value_name = "PATH")]
    pub unix: Option<PathBuf>,

    /// Connect over TCP instead of Unix, to `HOST:PORT` (a `--viewer-tcp`
    /// listener of the runtime, not its AGP TCP endpoint).
    #[arg(long, value_name = "HOST:PORT", conflicts_with = "unix")]
    pub tcp: Option<String>,

    /// `tracing-subscriber` env-filter directive (also read from `ADESK_LOG`).
    #[arg(long, env = "ADESK_LOG", value_name = "FILTER", default_value = "info")]
    pub log: String,
}

impl Cli {
    /// Resolves the viewer endpoint from `--unix`/`--tcp`, defaulting to the
    /// runtime-derived Unix socket when neither is given.
    ///
    /// The Unix path is [`adesk_viewer::resolve_socket_path`]: an explicit
    /// `--unix` always wins, otherwise `$ADESK_VIEWER_SOCKET`, else the sibling
    /// of the server's default AGP socket — exactly the server's own
    /// viewer-endpoint derivation, so the GUI's default can never be the AGP
    /// socket.
    ///
    /// # Errors
    ///
    /// Returns [`GuiError::Config`] when `--tcp` is not a valid `HOST:PORT`
    /// socket address.
    pub fn target(&self) -> Result<adesk_viewer::ViewerTarget> {
        if let Some(address) = &self.tcp {
            let address = address.parse::<SocketAddr>().map_err(|error| {
                GuiError::Config(format!("invalid --tcp address `{address}`: {error}"))
            })?;
            return Ok(adesk_viewer::ViewerTarget::Tcp(address));
        }
        Ok(adesk_viewer::ViewerTarget::Unix(
            adesk_viewer::resolve_socket_path(self.unix.clone()),
        ))
    }
}

/// The default Unix socket path: `adesk_viewer::resolve_socket_path(None)` —
/// `$ADESK_VIEWER_SOCKET`, else the sibling of the server's default AGP socket
/// (`$ADESK_SOCKET`'s sibling, else `$XDG_RUNTIME_DIR/adesk-viewer.sock`, else
/// `<temp_dir>/adesk-viewer.sock`).
pub fn default_socket_path() -> PathBuf {
    adesk_viewer::resolve_socket_path(None)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("the CLI should parse")
    }

    /// Serializes the tests that mutate the process-global environment: the
    /// resolver reads it with `std::env::var_os`, which is process-global state
    /// (the same pattern as `crates/adesk-viewer/src/socket.rs`).
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
    fn parses_a_unix_invocation() {
        let cli = parse(&["adesk-viewer-gui", "--unix", "/tmp/viewer.sock"]);
        assert_eq!(cli.unix, Some(PathBuf::from("/tmp/viewer.sock")));
        assert_eq!(cli.tcp, None);
        assert_eq!(cli.log, "info");
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(PathBuf::from("/tmp/viewer.sock"))
        );
    }

    #[test]
    fn parses_a_tcp_invocation() {
        let cli = parse(&["adesk-viewer-gui", "--tcp", "127.0.0.1:7100"]);
        assert_eq!(cli.tcp.as_deref(), Some("127.0.0.1:7100"));
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Tcp("127.0.0.1:7100".parse().unwrap())
        );
    }

    #[test]
    fn rejects_unix_combined_with_tcp() {
        let error = Cli::try_parse_from([
            "adesk-viewer-gui",
            "--unix",
            "/tmp/viewer.sock",
            "--tcp",
            "127.0.0.1:7100",
        ])
        .expect_err("--unix and --tcp conflict");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn reports_a_bad_tcp_address_as_a_config_error() {
        let cli = parse(&["adesk-viewer-gui", "--tcp", "not-an-address"]);
        assert!(matches!(cli.target().unwrap_err(), GuiError::Config(_)));
    }

    #[test]
    fn defaults_to_the_runtime_socket_path() {
        // The GUI default must be exactly the shared resolver's default — the
        // server's own viewer-endpoint derivation — not a GUI-local guess.
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", None),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
        ]);
        let cli = parse(&["adesk-viewer-gui"]);
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(adesk_viewer::resolve_socket_path(None))
        );
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(default_socket_path())
        );
    }

    #[test]
    fn an_explicit_unix_flag_wins_over_every_environment() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", Some("/env/viewer.sock")),
            ("ADESK_SOCKET", Some("/env/agp.sock")),
            ("XDG_RUNTIME_DIR", Some("/env/xdg")),
        ]);
        let cli = parse(&["adesk-viewer-gui", "--unix", "/flag/viewer.sock"]);
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(PathBuf::from("/flag/viewer.sock"))
        );
    }

    #[test]
    fn the_default_follows_the_viewer_socket_environment() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", Some("/env/viewer.sock")),
            ("ADESK_SOCKET", None),
            ("XDG_RUNTIME_DIR", Some("/env/xdg")),
        ]);
        let cli = parse(&["adesk-viewer-gui"]);
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(PathBuf::from("/env/viewer.sock"))
        );
    }

    #[test]
    fn the_default_follows_the_agp_socket_environment_to_its_sibling() {
        let _lock = lock_env();
        let _env = EnvGuard::set(&[
            ("ADESK_VIEWER_SOCKET", None),
            ("ADESK_SOCKET", Some("/run/custom/agp.sock")),
            ("XDG_RUNTIME_DIR", Some("/env/xdg")),
        ]);
        // A custom `$ADESK_SOCKET` moves the server's viewer endpoint to that
        // socket's sibling; the GUI default must follow it — that is what keeps
        // the GUI off the AGP socket in a non-default deployment.
        let cli = parse(&["adesk-viewer-gui"]);
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(PathBuf::from("/run/custom/agp-viewer.sock"))
        );
    }
}
