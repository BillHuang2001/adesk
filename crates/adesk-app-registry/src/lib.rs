//! ADesk application registry — XDG desktop entries, launching and window correlation.
//!
//! This crate implements the registry half of `docs/architecture.md` §7 and AGP §5.2
//! (`list_apps`, `get_app`, `launch_app`). It owns:
//!
//! - XDG search-directory resolution ([`xdg`]) and recursive `.desktop` discovery,
//! - `[Desktop Entry]` parsing ([`parser`]) into validated [`DesktopEntry`] records,
//! - desktop-file id derivation ([`app_id`]),
//! - `Exec` field-code expansion ([`exec`]) and `Terminal=true` wrapping ([`terminal`]),
//! - `TryExec` availability resolution ([`try_exec`]),
//! - process launch through the injectable [`ProcessSpawner`] seam ([`launch`]),
//! - launch↔window correlation ([`correlate`]).
//!
//! The crate is **sync only** — no tokio, no async, no Smithay. Its only I/O is
//! filesystem reads during [`AppRegistry::scan`] and process spawn during
//! [`AppRegistry::launch`]. `adesk-server` owns the AGP surface and is responsible
//! for emitting the `AppLaunched` event, stamping `app_id` on windows, and reaping
//! spawned children (see `CONTEXT.md` → "What adesk-server must know").
//!
//! All state that crosses a thread is `Send + Sync`: [`AppRegistry`] is safe to
//! share as `Arc<AppRegistry>` and both injection seams ([`ProcessSpawner`],
//! [`Clock`]) are `Send + Sync` trait objects.
//!
//! ```no_run
//! use adesk_app_registry::{AppRegistry, LaunchEnv, RegistryOptions};
//! use adesk_core::AppId;
//!
//! let registry = AppRegistry::with_options(RegistryOptions::xdg());
//! let _report = registry.scan().expect("scan application directories");
//! let apps = registry.list(Some("firefox"), false);
//! if let Some(app) = registry.get(&AppId::from("org.mozilla.firefox")) {
//!     let record = registry.launch(&app.id, &[], &LaunchEnv::new())?;
//!     println!("launched {} (pid {:?})", record.app_id, record.pid);
//!     let _ = apps;
//! }
//! # Ok::<(), adesk_app_registry::Error>(())
//! ```
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod app_id;
pub mod clock;
pub mod correlate;
pub mod error;
pub mod exec;
pub mod launch;
pub mod parser;
pub mod registry;
pub mod terminal;
pub mod try_exec;
pub mod xdg;

pub use app_id::{desktop_file_id, is_desktop_file};
pub use clock::{Clock, MonotonicClock};
pub use correlate::{
    Correlation, CorrelationEvidence, CorrelationOutcome, Correlator, WindowCandidate,
    DEFAULT_CORRELATION_TIMEOUT,
};
pub use error::{Error, Result};
pub use exec::{ExecContext, ExecError, ExecExpander};
pub use launch::{
    CommandSpawner, LaunchEnv, LaunchRecord, ProcessSpawner, SpawnCommand, SpawnError,
    SpawnedProcess,
};
pub use parser::{parse_str, DesktopEntry, EntryError, ParseError, RawEntry};
pub use registry::{AppRegistry, RegistryOptions, ScanIssue, ScanReport};
pub use terminal::TerminalSpec;
pub use try_exec::{is_executable, resolve_try_exec, resolve_try_exec_with};
pub use xdg::{search_dirs, search_dirs_with};
