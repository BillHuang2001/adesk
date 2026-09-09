//! Crate error type and its mapping to the umbrella [`adesk_core::Error`].
//!
//! `adesk-server` converts this crate's errors with `?`/`.into()`; the mapping is
//! pinned by [`Error::code`] so AGP `error.code` values never drift
//! (`docs/protocol.md` §6).

use std::path::PathBuf;

use adesk_core::{AppId, ErrorCode};

use crate::exec::ExecError;
use crate::launch::SpawnError;

/// Everything that can go wrong while discovering, describing or launching an
/// application.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The requested app id is not in the registry (`get_app`/`launch_app`).
    #[error("unknown app: {0}")]
    UnknownApp(AppId),
    /// A `.desktop` file is structurally invalid; the scan records it as an issue.
    #[error("invalid desktop entry {path}: {reason}")]
    InvalidEntry {
        /// File that failed to parse.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },
    /// The entry has no usable `Exec` line (includes `DBusActivatable` entries
    /// with no `Exec` fallback).
    #[error("app {app} has no usable Exec line")]
    NoExec {
        /// The app that cannot be launched.
        app: AppId,
    },
    /// The entry's `Exec` line is malformed (e.g. an unterminated quote).
    #[error("app {app} has an invalid Exec line: {source}")]
    InvalidExec {
        /// The app whose `Exec` line is invalid.
        app: AppId,
        /// Tokenizer failure.
        #[source]
        source: ExecError,
    },
    /// `TryExec` is set but does not resolve to an executable file.
    #[error("TryExec program not found: {program}")]
    TryExecNotFound {
        /// The unresolved `TryExec` value.
        program: String,
    },
    /// The process could not be spawned.
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// A filesystem operation failed.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path being read when the failure occurred.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// A caller-supplied argument is invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl Error {
    /// The AGP error code this failure maps to (`docs/protocol.md` §6).
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::UnknownApp(_) => ErrorCode::UnknownApp,
            Error::InvalidEntry { .. } => ErrorCode::Internal,
            Error::NoExec { .. } => ErrorCode::NotSupported,
            Error::InvalidExec { .. } => ErrorCode::LaunchFailed,
            Error::TryExecNotFound { .. } => ErrorCode::LaunchFailed,
            Error::Spawn(_) => ErrorCode::LaunchFailed,
            Error::Io { .. } => ErrorCode::Internal,
            Error::InvalidArgument(_) => ErrorCode::InvalidRequest,
        }
    }
}

impl From<Error> for adesk_core::Error {
    fn from(error: Error) -> adesk_core::Error {
        adesk_core::Error::new(error.code(), error.to_string())
    }
}

/// Crate-wide result alias: `Result<T, adesk_app_registry::Error>`.
pub type Result<T> = std::result::Result<T, Error>;
