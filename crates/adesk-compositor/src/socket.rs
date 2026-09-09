//! Binding the Wayland listening socket.
//!
//! The compositor binds its socket **before** it creates any protocol global: the
//! socket name is part of readiness reporting ([`crate::ReadyInfo::display_name`])
//! and clients must be able to connect the moment the runtime announces itself.
//! The returned [`ListeningSocketSource`] is inserted into the `calloop` loop by
//! [`crate::run::run_compositor_thread`]; it owns the socket file and removes it on
//! drop, so shutdown needs no explicit cleanup.
//!
//! `ListeningSocket` requires `XDG_RUNTIME_DIR`; a missing or unwritable runtime
//! directory surfaces as [`CompositorError::Socket`], never a panic.

use smithay::wayland::socket::ListeningSocketSource;

use crate::{config::CompositorConfig, error::CompositorError, Result};

/// Bind the Wayland listening socket described by `config`.
///
/// `config.socket_name == Some(name)` binds exactly that name (used when the
/// server must control `WAYLAND_DISPLAY`, e.g. tests); `None` binds the next free
/// `wayland-N` name (`wayland-1` … `wayland-32`, skipping `wayland-0` so clients
/// cannot accidentally connect to another compositor).
pub(crate) fn bind(config: &CompositorConfig) -> Result<ListeningSocketSource> {
    let source = match config.socket_name.as_deref() {
        Some(name) => ListeningSocketSource::with_name(name),
        None => ListeningSocketSource::new_auto(),
    };
    source.map_err(|error| CompositorError::Socket(error.to_string()))
}

/// The socket name clients must connect to (`wayland-N`).
///
/// The name is an `OsStr` because it comes from the filesystem; Wayland socket
/// names are ASCII, so a lossy conversion cannot lose information in practice.
pub(crate) fn socket_name(source: &ListeningSocketSource) -> String {
    source.socket_name().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the ambient `XDG_RUNTIME_DIR` can actually host a listening socket.
    ///
    /// `None` means the variable is unset; `Some(false)` means it exists but is not
    /// writable (read-only sandboxes, CI containers). Both are legitimate
    /// environments, and both must produce a structured error instead of a panic.
    fn runtime_dir_writable() -> Option<bool> {
        let dir = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
        if dir.as_os_str().is_empty() {
            return Some(false);
        }
        let probe = dir.join(".adesk-socket-probe");
        match std::fs::File::create(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                Some(true)
            }
            Err(_) => Some(false),
        }
    }

    #[test]
    fn bind_honours_the_configured_name_and_reports_structured_errors() {
        // A process-unique name: it can never collide with a session compositor's
        // `wayland-0` or with the auto-bound sockets of parallel tests.
        let unique = format!("adesk-test-{}", std::process::id());
        let explicit = CompositorConfig::default().with_socket_name(unique.clone());

        match runtime_dir_writable() {
            Some(true) => {
                let auto = bind(&CompositorConfig::default()).expect("auto bind succeeds");
                let name = socket_name(&auto);
                assert!(
                    name.starts_with("wayland-"),
                    "unexpected socket name: {name}"
                );
                assert!(
                    name["wayland-".len()..].parse::<u32>().is_ok(),
                    "socket name must end in a number: {name}"
                );

                let named = bind(&explicit).expect("explicit bind succeeds");
                assert_eq!(socket_name(&named), unique);
            }
            // No usable runtime dir: every bind must fail with the structured socket
            // error (missing or unwritable `XDG_RUNTIME_DIR`), never panic.
            _ => {
                assert!(matches!(
                    bind(&CompositorConfig::default()),
                    Err(CompositorError::Socket(_))
                ));
                assert!(matches!(bind(&explicit), Err(CompositorError::Socket(_))));
            }
        }
    }
}
