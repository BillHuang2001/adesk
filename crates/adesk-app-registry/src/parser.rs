//! Parser for `.desktop` files: raw `[Desktop Entry]` groups and their typed form.
//!
//! Two layers:
//!
//! 1. [`parse_str`] reads the file text into a [`RawEntry`] — an ordered key/value
//!    view of the `[Desktop Entry]` group with stage-1 escape resolution.
//! 2. [`DesktopEntry::from_raw`] validates and types that group into a
//!    [`DesktopEntry`], resolving the localized `Name` and rejecting non-Application
//!    entries ([`EntryError::NotApplication`]) and malformed ones.
//!
//! Parser tolerance (design decision): lines without `=`, keys outside any group,
//! unknown keys and unknown groups are ignored; only a missing `[Desktop Entry]`
//! group is a [`ParseError`]. Duplicate keys keep the **last** occurrence, matching
//! the reference key-file implementations.
//!
//! Escapes (stage 1, spec "string" values): `\s` → space, `\n` → newline,
//! `\t` → tab, `\r` → CR, `\\` → backslash. Any other `\X` is kept verbatim
//! (backslash included) so `Exec` quoting (stage 2, [`crate::exec`]) still sees it.

use std::path::PathBuf;

use adesk_core::{AppId, AppInfo};

/// Raw key/value view of the `[Desktop Entry]` group of one `.desktop` file.
///
/// Values are already stage-1 unescaped; keys keep their raw form, including
/// locale suffixes such as `Name[fr_FR]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawEntry {
    // stub: ordered (key, value) pairs in file order.
    #[allow(dead_code)]
    values: Vec<(String, String)>,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl RawEntry {
    /// Base value of `key` (the last occurrence in the file wins), unlocalized.
    pub fn get(&self, key: &str) -> Option<&str> {
        todo!("stub: implementation phase")
    }

    /// Localized value of `key` for `locale`, falling back to [`RawEntry::get`].
    ///
    /// The locale string is normalized by dropping the `.ENCODING` part, then
    /// candidate keys are tried in order:
    /// `lang_COUNTRY@MODIFIER`, `lang@MODIFIER`, `lang_COUNTRY`, `lang`.
    /// `locale = None` goes straight to the base value.
    pub fn localized(&self, key: &str, locale: Option<&str>) -> Option<&str> {
        todo!("stub: implementation phase")
    }
}

/// Structural failure while parsing a `.desktop` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The file contains no `[Desktop Entry]` group.
    #[error("missing [Desktop Entry] group")]
    MissingGroup,
}

/// Parses the `[Desktop Entry]` group from the text of a `.desktop` file.
///
/// CRLF line endings are accepted; a leading BOM is ignored.
#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
pub fn parse_str(text: &str) -> Result<RawEntry, ParseError> {
    todo!("stub: implementation phase")
}

/// Failure while typing a raw group into a [`DesktopEntry`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EntryError {
    /// The `Type` key is missing.
    #[error("missing Type key")]
    MissingType,
    /// `Type` is present but not `Application`; the scan skips such files.
    #[error("Type is {0:?}, not Application")]
    NotApplication(String),
    /// `Name` is missing or empty after localization.
    #[error("missing Name key")]
    MissingName,
}

/// A validated `Type=Application` desktop entry.
///
/// The path is kept because `Exec` field codes `%k` (desktop-file location) and
/// `%i` (icon) need it at launch time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// Desktop-file id (see [`crate::app_id::desktop_file_id`]).
    pub id: AppId,
    /// Path of the `.desktop` file this entry was read from.
    pub path: PathBuf,
    /// Localized `Name` (required).
    pub name: String,
    /// Raw `Exec` line, field codes unexpanded; `None` when the key is absent.
    pub exec: Option<String>,
    /// `Icon` name or path.
    pub icon: Option<String>,
    /// `Terminal=true`.
    pub terminal: bool,
    /// `NoDisplay=true`.
    pub no_display: bool,
    /// `Hidden=true`.
    pub hidden: bool,
    /// `Categories` split on `;`, trimmed, empty items dropped.
    pub categories: Vec<String>,
    /// `StartupWMClass`, used by [`crate::Correlator`].
    pub startup_wm_class: Option<String>,
    /// `DBusActivatable=true`.
    pub dbus_activatable: bool,
    /// `TryExec` program used for availability checks.
    pub try_exec: Option<String>,
}

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl DesktopEntry {
    /// Validates a raw `[Desktop Entry]` group and resolves its localized name.
    ///
    /// Boolean keys are parsed case-insensitively: only `true` is true, anything
    /// else is false. `Type` must be exactly `Application` (case-sensitive, per
    /// spec), otherwise [`EntryError::NotApplication`] is returned and the scan
    /// counts the file as skipped rather than reporting an issue.
    pub fn from_raw(
        id: AppId,
        path: PathBuf,
        raw: &RawEntry,
        locale: Option<&str>,
    ) -> std::result::Result<DesktopEntry, EntryError> {
        todo!("stub: implementation phase")
    }

    /// `true` when the entry passes the `list_apps` filters.
    ///
    /// With `include_hidden = false`, entries with `Hidden=true` or
    /// `NoDisplay=true` are excluded (`docs/architecture.md` §7).
    pub fn is_listable(&self, include_hidden: bool) -> bool {
        todo!("stub: implementation phase")
    }

    /// Converts to the wire-facing [`AppInfo`] returned by `list_apps`/`get_app`.
    pub fn to_app_info(&self) -> AppInfo {
        todo!("stub: implementation phase")
    }
}
