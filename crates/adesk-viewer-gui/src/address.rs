//! Connection-addressing diagnostics for the GUI viewer.
//!
//! GTK-free and unit-testable. Every failure message the GUI surfaces names the
//! endpoint it actually dialed — the same wording the headless `adesk-viewer`
//! binary prints — because the wrong-socket incident (the GUI pointed at the
//! AGP socket `…/adesk.sock` while the server's VAP viewer endpoint lives at
//! the sibling `…/adesk-viewer.sock`) was only visible as a bare "connection
//! closed". When the dialed Unix path carries the AGP socket's file name, the
//! message additionally gains a hint naming the sibling to try instead.
//!
//! This is message composition only: no reconnects, no protocol behavior.

use std::fmt::Display;
use std::path::Path;

use adesk_viewer::{viewer_socket_sibling, ViewerTarget};

/// The Unix socket file name of the AGP server: a dialed path whose file name
/// is this is almost certainly the AGP socket, not the VAP viewer endpoint
/// (the server closes such a connection as an undecodable frame).
pub const AGP_SOCKET_FILE_NAME: &str = "adesk.sock";

/// Formats the message for a failed connection attempt: the resolved endpoint
/// plus the underlying cause (the headless `adesk-viewer` wording), with the
/// AGP-misdirection hint appended when the dialed path looks like the AGP
/// socket.
pub fn connect_failure(target: &ViewerTarget, cause: impl Display) -> String {
    compose(
        format!("cannot connect to viewer socket {target}: {cause}"),
        target,
    )
}

/// Formats the message for a connection the server (or the transport) ended
/// mid-session: the resolved endpoint plus the stream failure (the headless
/// `adesk-viewer` wording), with the AGP-misdirection hint appended when the
/// dialed path looks like the AGP socket.
pub fn unexpected_close(target: &ViewerTarget, cause: impl Display) -> String {
    compose(
        format!("connection closed by server (viewer socket {target}): {cause}"),
        target,
    )
}

/// Appends the AGP-misdirection hint to `message` when `target` dials a Unix
/// path carrying [`AGP_SOCKET_FILE_NAME`]; TCP targets are never flagged.
fn compose(message: String, target: &ViewerTarget) -> String {
    let hint = match target {
        ViewerTarget::Unix(path) => agp_socket_hint(path),
        ViewerTarget::Tcp(_) => None,
    };
    match hint {
        Some(hint) => format!("{message}. {hint}"),
        None => message,
    }
}

/// The AGP-misdirection hint: [`Some`] when `path`'s file name is the AGP
/// socket's ([`AGP_SOCKET_FILE_NAME`]), naming the viewer sibling to try
/// instead; [`None`] for any other path, including paths with no file name.
pub fn agp_socket_hint(path: &Path) -> Option<String> {
    if path.file_name().and_then(|name| name.to_str()) != Some(AGP_SOCKET_FILE_NAME) {
        return None;
    }
    let sibling = viewer_socket_sibling(path);
    Some(format!(
        "{} looks like the AGP socket, not the VAP viewer endpoint; \
         the viewer endpoint is normally at {}",
        path.display(),
        sibling.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use adesk_viewer::ViewerError;

    use super::*;

    fn unix(path: &str) -> ViewerTarget {
        ViewerTarget::Unix(PathBuf::from(path))
    }

    #[test]
    fn a_normal_viewer_socket_gets_no_hint() {
        assert_eq!(
            agp_socket_hint(Path::new("/run/user/1000/adesk-viewer.sock")),
            None
        );
    }

    #[test]
    fn a_path_with_a_different_name_gets_no_hint() {
        assert_eq!(agp_socket_hint(Path::new("/tmp/viewer.sock")), None);
        // Only the exact file name is the AGP socket's; prefix/suffix matches
        // must not count.
        assert_eq!(
            agp_socket_hint(Path::new("/run/user/1000/my-adesk.sock")),
            None
        );
        assert_eq!(
            agp_socket_hint(Path::new("/run/user/1000/adesk.sock.bak")),
            None
        );
    }

    #[test]
    fn a_path_without_a_file_name_gets_no_hint() {
        assert_eq!(agp_socket_hint(Path::new("")), None);
        assert_eq!(agp_socket_hint(Path::new("/")), None);
    }

    #[test]
    fn the_agp_socket_file_name_gets_a_hint_naming_the_sibling() {
        let hint = agp_socket_hint(Path::new("/run/user/1000/adesk.sock"))
            .expect("adesk.sock must be flagged");
        assert!(hint.contains("adesk.sock"), "{hint}");
        assert!(hint.contains("/run/user/1000/adesk-viewer.sock"), "{hint}");
    }

    #[test]
    fn a_tcp_connect_failure_is_composed_without_a_hint() {
        let target = ViewerTarget::Tcp("127.0.0.1:7100".parse().unwrap());
        assert_eq!(
            connect_failure(&target, ViewerError::Closed),
            "cannot connect to viewer socket tcp:127.0.0.1:7100: connection closed"
        );
    }

    #[test]
    fn connect_failure_names_the_socket_path() {
        let message = connect_failure(
            &unix("/run/user/1000/adesk-viewer.sock"),
            ViewerError::Closed,
        );
        assert_eq!(
            message,
            "cannot connect to viewer socket \
             unix:/run/user/1000/adesk-viewer.sock: connection closed"
        );
    }

    #[test]
    fn connect_failure_carries_the_underlying_cause() {
        let message = connect_failure(
            &unix("/run/user/1000/adesk-viewer.sock"),
            ViewerError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
        );
        assert!(
            message.starts_with(
                "cannot connect to viewer socket \
                 unix:/run/user/1000/adesk-viewer.sock: "
            ),
            "{message}"
        );
        // The cause's own text follows the path (its exact wording is the
        // platform's business).
        assert!(!message.ends_with("adesk-viewer.sock: "), "{message}");
    }

    #[test]
    fn connect_failure_on_the_agp_socket_appends_the_hint() {
        let message = connect_failure(&unix("/run/user/1000/adesk.sock"), ViewerError::Closed);
        assert_eq!(
            message,
            "cannot connect to viewer socket unix:/run/user/1000/adesk.sock: \
             connection closed. /run/user/1000/adesk.sock looks like the AGP \
             socket, not the VAP viewer endpoint; the viewer endpoint is \
             normally at /run/user/1000/adesk-viewer.sock"
        );
    }

    #[test]
    fn unexpected_close_names_the_socket_path() {
        let message = unexpected_close(
            &unix("/run/user/1000/adesk-viewer.sock"),
            ViewerError::Closed,
        );
        assert_eq!(
            message,
            "connection closed by server \
             (viewer socket unix:/run/user/1000/adesk-viewer.sock): connection closed"
        );
    }

    #[test]
    fn unexpected_close_on_the_agp_socket_appends_the_hint() {
        let message = unexpected_close(&unix("/run/user/1000/adesk.sock"), ViewerError::Closed);
        assert_eq!(
            message,
            "connection closed by server (viewer socket \
             unix:/run/user/1000/adesk.sock): connection closed. \
             /run/user/1000/adesk.sock looks like the AGP socket, not the VAP \
             viewer endpoint; the viewer endpoint is normally at \
             /run/user/1000/adesk-viewer.sock"
        );
    }
}
