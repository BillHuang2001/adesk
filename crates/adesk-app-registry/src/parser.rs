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

/// Name of the only group this parser reads.
const DESKTOP_ENTRY_GROUP: &str = "Desktop Entry";

/// Raw key/value view of the `[Desktop Entry]` group of one `.desktop` file.
///
/// Values are already stage-1 unescaped; keys keep their raw form, including
/// locale suffixes such as `Name[fr_FR]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawEntry {
    /// Ordered `(key, value)` pairs in file order; duplicates are kept and the
    /// last occurrence wins on lookup.
    values: Vec<(String, String)>,
}

impl RawEntry {
    /// Base value of `key` (the last occurrence in the file wins), unlocalized.
    ///
    /// Only the exact key is matched, so `get("Name")` never returns `Name[fr]`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }

    /// Localized value of `key` for `locale`, falling back to [`RawEntry::get`].
    ///
    /// The locale string is normalized by dropping the `.ENCODING` part, then
    /// candidate keys are tried in order:
    /// `lang_COUNTRY@MODIFIER`, `lang@MODIFIER`, `lang_COUNTRY`, `lang`.
    /// `locale = None` goes straight to the base value.
    ///
    /// A candidate key whose value is empty is treated as absent, so a broken
    /// localization string never shadows a usable base value.
    pub fn localized(&self, key: &str, locale: Option<&str>) -> Option<&str> {
        for suffix in locale_candidates(locale) {
            if let Some(value) = self.get(&format!("{key}[{suffix}]")) {
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
        self.get(key)
    }
}

/// Locale suffixes for `key[suffix]` lookups, in precedence order.
///
/// `locale` is `lang_COUNTRY.ENCODING@MODIFIER`; the encoding is dropped and the
/// modifier kept. Empty or partial locales contribute only the candidates they
/// actually define.
fn locale_candidates(locale: Option<&str>) -> Vec<String> {
    let Some(locale) = locale else {
        return Vec::new();
    };
    let (core, modifier) = match locale.split_once('@') {
        Some((core, modifier)) => (core, Some(modifier)),
        None => (locale, None),
    };
    let core = core.split_once('.').map_or(core, |(core, _)| core);
    let modifier = modifier.filter(|modifier| !modifier.is_empty());
    let (lang, country) = match core.split_once('_') {
        Some((lang, country)) => (lang, Some(country)),
        None => (core, None),
    };
    let country = country.filter(|country| !country.is_empty());
    let lang = (!lang.is_empty()).then_some(lang);

    let mut candidates = Vec::new();
    if let (Some(lang), Some(country), Some(modifier)) = (lang, country, modifier) {
        candidates.push(format!("{lang}_{country}@{modifier}"));
    }
    if let (Some(lang), Some(modifier)) = (lang, modifier) {
        candidates.push(format!("{lang}@{modifier}"));
    }
    if let (Some(lang), Some(country)) = (lang, country) {
        candidates.push(format!("{lang}_{country}"));
    }
    if let Some(lang) = lang {
        candidates.push(lang.to_string());
    }
    candidates
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
pub fn parse_str(text: &str) -> Result<RawEntry, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut entry = RawEntry::default();
    let mut in_entry_group = false;
    let mut saw_entry_group = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = group_name(line) {
            in_entry_group = group == DESKTOP_ENTRY_GROUP;
            saw_entry_group |= in_entry_group;
            continue;
        }
        if !in_entry_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        entry.values.push((key.to_string(), unescape(value.trim())));
    }

    if saw_entry_group {
        Ok(entry)
    } else {
        Err(ParseError::MissingGroup)
    }
}

/// Returns the group name when `line` is a `[group]` header.
fn group_name(line: &str) -> Option<&str> {
    line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
}

/// Resolves the stage-1 string escapes; unknown `\X` sequences stay verbatim.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
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
    /// `Icon` name or path; `None` when absent or empty.
    pub icon: Option<String>,
    /// `Terminal=true`.
    pub terminal: bool,
    /// `NoDisplay=true`.
    pub no_display: bool,
    /// `Hidden=true`.
    pub hidden: bool,
    /// `Categories` split on `;`, trimmed, empty items dropped.
    pub categories: Vec<String>,
    /// `StartupWMClass`, used by [`crate::Correlator`]; `None` when absent or empty.
    pub startup_wm_class: Option<String>,
    /// `DBusActivatable=true`.
    pub dbus_activatable: bool,
    /// `TryExec` program used for availability checks; `None` when absent or empty.
    pub try_exec: Option<String>,
}

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
        let entry_type = raw
            .get("Type")
            .filter(|value| !value.trim().is_empty())
            .ok_or(EntryError::MissingType)?;
        if entry_type != "Application" {
            return Err(EntryError::NotApplication(entry_type.to_string()));
        }
        let name = raw
            .localized("Name", locale)
            .filter(|value| !value.trim().is_empty())
            .ok_or(EntryError::MissingName)?
            .to_string();

        Ok(DesktopEntry {
            id,
            path,
            name,
            exec: raw.get("Exec").map(str::to_string),
            icon: non_empty(raw.get("Icon")),
            terminal: boolean(raw.get("Terminal")),
            no_display: boolean(raw.get("NoDisplay")),
            hidden: boolean(raw.get("Hidden")),
            categories: split_categories(raw.get("Categories")),
            startup_wm_class: non_empty(raw.get("StartupWMClass")),
            dbus_activatable: boolean(raw.get("DBusActivatable")),
            try_exec: non_empty(raw.get("TryExec")),
        })
    }

    /// `true` when the entry passes the `list_apps` filters.
    ///
    /// With `include_hidden = false`, entries with `Hidden=true` or
    /// `NoDisplay=true` are excluded (`docs/architecture.md` §7).
    pub fn is_listable(&self, include_hidden: bool) -> bool {
        include_hidden || !(self.hidden || self.no_display)
    }

    /// Converts to the wire-facing [`AppInfo`] returned by `list_apps`/`get_app`.
    pub fn to_app_info(&self) -> AppInfo {
        AppInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            icon: self.icon.clone(),
            exec: self.exec.clone(),
            terminal: self.terminal,
            categories: self.categories.clone(),
            startup_wm_class: self.startup_wm_class.clone(),
            dbus_activatable: self.dbus_activatable,
            hidden: self.hidden,
            no_display: self.no_display,
            try_exec: self.try_exec.clone(),
        }
    }
}

/// `Some(value)` unless the key is absent or its value is empty.
fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(str::to_string)
}

/// Desktop-entry boolean: only a case-insensitive `true` is true.
fn boolean(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// `Categories` is a `;`-separated list; empty items are dropped.
fn split_categories(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}
