//! Command-line parsing and viewer-endpoint resolution.
//!
//! GTK-free and unit-testable through [`clap::Parser::try_parse_from`]. The flag
//! names and help text mirror the headless `adesk-viewer` binary
//! (`crates/adesk-viewer/src/main.rs`) so a human can move between the two
//! front-ends without relearning the transport flags.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use crate::error::{GuiError, Result};

/// The Unix socket file name used under `$XDG_RUNTIME_DIR` (`docs/viewer.md` §1).
const SOCKET_FILE_NAME: &str = "adesk-viewer.sock";

/// `adesk-viewer-gui` — a GTK4/libadwaita Viewer Attachment Protocol client.
#[derive(Debug, Parser)]
#[command(name = "adesk-viewer-gui", version, long_about = None)]
pub struct Cli {
    /// Connect over a Unix domain socket at PATH (default:
    /// `$XDG_RUNTIME_DIR/adesk-viewer.sock`, else `<temp_dir>/adesk-viewer.sock`).
    #[arg(long, value_name = "PATH")]
    pub unix: Option<PathBuf>,

    /// Connect over TCP instead of Unix, to `HOST:PORT`.
    #[arg(long, value_name = "HOST:PORT", conflicts_with = "unix")]
    pub tcp: Option<String>,

    /// `tracing-subscriber` env-filter directive (also read from `ADESK_LOG`).
    #[arg(long, env = "ADESK_LOG", value_name = "FILTER", default_value = "info")]
    pub log: String,
}

impl Cli {
    /// Resolves the viewer endpoint from `--unix`/`--tcp`, defaulting to the
    /// runtime-derived Unix socket path when neither is given.
    ///
    /// # Errors
    ///
    /// Returns [`GuiError::Config`] when `--tcp` is not a valid `HOST:PORT`
    /// socket address.
    pub fn target(&self) -> Result<adesk_viewer::ViewerTarget> {
        if let Some(path) = &self.unix {
            return Ok(adesk_viewer::ViewerTarget::Unix(path.clone()));
        }
        if let Some(address) = &self.tcp {
            let address = address.parse::<SocketAddr>().map_err(|error| {
                GuiError::Config(format!("invalid --tcp address `{address}`: {error}"))
            })?;
            return Ok(adesk_viewer::ViewerTarget::Tcp(address));
        }
        Ok(adesk_viewer::ViewerTarget::Unix(default_socket_path()))
    }
}

/// The default Unix socket path: `$XDG_RUNTIME_DIR/adesk-viewer.sock` when the
/// variable is set and non-empty, otherwise `<temp_dir>/adesk-viewer.sock`.
///
/// Reads the process environment once and delegates to [`socket_path_from`].
pub fn default_socket_path() -> PathBuf {
    socket_path_from(std::env::var_os("XDG_RUNTIME_DIR"))
}

/// Pure socket-path helper: joins `adesk-viewer.sock` onto `xdg_runtime_dir`
/// when it is set and non-empty, otherwise onto the system temp directory.
///
/// Taking the environment value as a parameter keeps the logic testable without
/// mutating process-global state.
pub fn socket_path_from(xdg_runtime_dir: Option<OsString>) -> PathBuf {
    match xdg_runtime_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(SOCKET_FILE_NAME),
        _ => std::env::temp_dir().join(SOCKET_FILE_NAME),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("the CLI should parse")
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
        let cli = parse(&["adesk-viewer-gui"]);
        assert_eq!(
            cli.target().unwrap(),
            adesk_viewer::ViewerTarget::Unix(default_socket_path())
        );
    }

    #[test]
    fn socket_path_from_uses_the_xdg_runtime_dir_when_set() {
        let path = socket_path_from(Some(OsString::from("/run/user/1000")));
        assert_eq!(path, PathBuf::from("/run/user/1000/adesk-viewer.sock"));
    }

    #[test]
    fn socket_path_from_falls_back_to_the_temp_dir() {
        let expected = std::env::temp_dir().join("adesk-viewer.sock");
        assert_eq!(socket_path_from(None), expected);
        assert_eq!(socket_path_from(Some(OsString::new())), expected);
    }
}
