//! Crate-local error type.
//!
//! `adesk-machine` is a different domain from the GUI runtime and does not
//! depend on `adesk-core`, so it carries its own error enum. Every fallible
//! operation on the machine lifecycle and the host control plane returns
//! `MachineError`; no request path panics (see `docs/machine.md`).

/// Errors produced by the AI Machine runtime and host control plane.
///
/// The variants separate *whose* fault a failure is: a [`MachineError::Backend`]
/// is a container-engine CLI that exited non-zero, [`MachineError::InvalidSpec`]
/// is a malformed `MachineSpec` rejected before reaching the engine, and
/// [`MachineError::InvalidState`] is a lifecycle action that the machine's
/// current state forbids.
#[derive(Debug, thiserror::Error)]
pub enum MachineError {
    /// A backend/runtime failure that is not a CLI exit status (for example an
    /// unparseable `inspect` payload).
    #[error("runtime error: {message}")]
    Runtime {
        /// Human-readable description of the failure.
        message: String,
    },
    /// A container-engine CLI exited non-zero.
    #[error("backend `{program}` exited with status {status}: {stderr}")]
    Backend {
        /// The CLI program that was invoked (for example `podman`).
        program: String,
        /// The process exit status; `-1` means it was terminated by a signal.
        status: i32,
        /// Captured standard error, trimmed.
        stderr: String,
    },
    /// No machine with the given name is known.
    #[error("machine not found: {name}")]
    NotFound {
        /// The unknown machine name.
        name: String,
    },
    /// A machine with the given name already exists.
    #[error("machine already exists: {name}")]
    Duplicate {
        /// The conflicting machine name.
        name: String,
    },
    /// The machine spec was rejected before reaching the backend.
    #[error("invalid spec: {message}")]
    InvalidSpec {
        /// Human-readable description of what is wrong with the spec.
        message: String,
    },
    /// The machine is not in a state that allows the requested action.
    #[error("machine `{name}` in state `{state}` cannot {action}")]
    InvalidState {
        /// The machine the action targets.
        name: String,
        /// The machine's current state (`MachineState::as_str`).
        state: String,
        /// The action that was attempted (for example `start`).
        action: String,
    },
    /// An I/O error that is not a failure to spawn a process.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A process could not be spawned.
    ///
    /// Distinct from [`MachineError::Io`] so callers can tell "the engine ran
    /// and failed" from "the engine could not be started at all". There is no
    /// `#[from]` here because `std::io::Error` already converts into
    /// [`MachineError::Io`]; construct this variant explicitly with
    /// `MachineError::Spawn`.
    #[error("failed to spawn process: {0}")]
    Spawn(std::io::Error),
}

/// Crate-wide result alias: `Result<T, adesk_machine::MachineError>`.
pub type Result<T, E = MachineError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_stable() {
        assert_eq!(
            MachineError::Runtime {
                message: "bad inspect".into(),
            }
            .to_string(),
            "runtime error: bad inspect"
        );
        assert_eq!(
            MachineError::Backend {
                program: "podman".into(),
                status: 125,
                stderr: "boom".into(),
            }
            .to_string(),
            "backend `podman` exited with status 125: boom"
        );
        assert_eq!(
            MachineError::NotFound {
                name: "adesk".into()
            }
            .to_string(),
            "machine not found: adesk"
        );
        assert_eq!(
            MachineError::Duplicate {
                name: "adesk".into()
            }
            .to_string(),
            "machine already exists: adesk"
        );
        assert_eq!(
            MachineError::InvalidSpec {
                message: "empty image".into(),
            }
            .to_string(),
            "invalid spec: empty image"
        );
        assert_eq!(
            MachineError::InvalidState {
                name: "adesk".into(),
                state: "stopped".into(),
                action: "stop".into(),
            }
            .to_string(),
            "machine `adesk` in state `stopped` cannot stop"
        );
    }

    #[test]
    fn io_error_converts_into_io_variant() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: MachineError = io.into();
        assert!(matches!(err, MachineError::Io(_)));
        assert_eq!(err.to_string(), "missing");
    }

    #[test]
    fn spawn_is_constructible_and_distinct_from_io() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no podman");
        let err = MachineError::Spawn(io);
        assert!(matches!(err, MachineError::Spawn(_)));
        assert_eq!(err.to_string(), "failed to spawn process: no podman");
    }

    #[test]
    fn result_alias_defaults_to_machine_error() {
        fn ok() -> Result<u32> {
            Ok(7)
        }
        fn fail() -> Result<u32> {
            Err(MachineError::NotFound { name: "x".into() })
        }
        assert_eq!(ok().unwrap(), 7);
        assert!(matches!(fail().unwrap_err(), MachineError::NotFound { .. }));
    }

    #[test]
    fn error_is_a_std_error() {
        let boxed: Box<dyn std::error::Error + Send + Sync> =
            Box::new(MachineError::NotFound { name: "x".into() });
        assert_eq!(boxed.to_string(), "machine not found: x");
    }
}
