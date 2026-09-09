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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(lines: &str) -> RawEntry {
        parse_str(&format!("[Desktop Entry]\n{lines}\n")).expect("fixture parses")
    }

    fn app_id() -> AppId {
        AppId::from("org.example.app")
    }

    fn path() -> PathBuf {
        PathBuf::from("/tmp/applications/org.example.app.desktop")
    }

    fn entry(lines: &str) -> DesktopEntry {
        DesktopEntry::from_raw(app_id(), path(), &raw(lines), None).expect("fixture validates")
    }

    fn entry_err(lines: &str) -> EntryError {
        DesktopEntry::from_raw(app_id(), path(), &raw(lines), None).expect_err("fixture must fail")
    }

    const FULL: &str = "\
Type=Application
Name=Example
Name[fr]=Exemple
GenericName=An example
Comment=A comment
Exec=/usr/bin/example --flag %U
Icon=example-icon
Terminal=true
Categories=Utility;Core;
StartupWMClass=example-wm
DBusActivatable=true
TryExec=/usr/bin/example
NoDisplay=false
Hidden=false
";

    #[test]
    fn parses_raw_values_in_file_order() {
        let raw = raw(FULL);
        assert_eq!(raw.get("Type"), Some("Application"));
        assert_eq!(raw.get("Exec"), Some("/usr/bin/example --flag %U"));
        assert_eq!(raw.get("GenericName"), Some("An example"));
        assert_eq!(raw.get("Missing"), None);
        assert_eq!(raw.localized("Name", Some("fr")), Some("Exemple"));
        assert_eq!(raw.localized("Name", None), Some("Example"));
    }

    #[test]
    fn from_raw_projects_every_field() {
        let entry = entry(FULL);
        assert_eq!(entry.id, app_id());
        assert_eq!(entry.path, path());
        assert_eq!(entry.name, "Example");
        assert_eq!(entry.exec.as_deref(), Some("/usr/bin/example --flag %U"));
        assert_eq!(entry.icon.as_deref(), Some("example-icon"));
        assert!(entry.terminal);
        assert!(!entry.no_display);
        assert!(!entry.hidden);
        assert_eq!(entry.categories, vec!["Utility", "Core"]);
        assert_eq!(entry.startup_wm_class.as_deref(), Some("example-wm"));
        assert!(entry.dbus_activatable);
        assert_eq!(entry.try_exec.as_deref(), Some("/usr/bin/example"));
    }

    #[test]
    fn to_app_info_matches_app_info() {
        let entry = entry(FULL);
        let expected = AppInfo {
            id: app_id(),
            name: "Example".into(),
            icon: Some("example-icon".into()),
            exec: Some("/usr/bin/example --flag %U".into()),
            terminal: true,
            categories: vec!["Utility".into(), "Core".into()],
            startup_wm_class: Some("example-wm".into()),
            dbus_activatable: true,
            hidden: false,
            no_display: false,
            try_exec: Some("/usr/bin/example".into()),
        };
        assert_eq!(entry.to_app_info(), expected);
        assert_eq!(
            entry.to_app_info(),
            entry.to_app_info(),
            "projection is pure"
        );
    }

    #[test]
    fn to_app_info_carries_filter_flags_and_none_fields() {
        let info = entry("Type=Application\nName=X\nHidden=true\nNoDisplay=true\n").to_app_info();
        assert!(info.hidden);
        assert!(info.no_display);
        assert_eq!(info.icon, None);
        assert_eq!(info.exec, None);
        assert_eq!(info.startup_wm_class, None);
        assert_eq!(info.try_exec, None);
        assert!(info.categories.is_empty());
        assert!(!info.terminal);
        assert!(!info.dbus_activatable);
    }

    #[test]
    fn absent_and_empty_optional_keys() {
        let entry = entry(
            "Type=Application\nName=Minimal\nExec=\nIcon=\nStartupWMClass=\nTryExec=\nCategories=\n",
        );
        assert_eq!(entry.exec.as_deref(), Some(""), "Exec keeps presence");
        assert_eq!(entry.icon, None);
        assert_eq!(entry.startup_wm_class, None);
        assert_eq!(entry.try_exec, None);
        assert!(entry.categories.is_empty());
    }

    // --- escapes -----------------------------------------------------------

    #[test]
    fn resolves_stage_one_escapes() {
        let raw = raw(r"Name=a\sb\nc\td\re\\f");
        assert_eq!(raw.get("Name"), Some("a b\nc\td\re\\f"));
    }

    #[test]
    fn keeps_unknown_escapes_verbatim_for_stage_two() {
        let raw = raw(r#"Exec=prog \%U \"quoted arg\" \x"#);
        assert_eq!(raw.get("Exec"), Some(r#"prog \%U \"quoted arg\" \x"#));
    }

    #[test]
    fn trailing_backslash_stays_verbatim() {
        let raw = raw(r"Name=trailing\");
        assert_eq!(raw.get("Name"), Some("trailing\\"));
    }

    #[test]
    fn escaped_trailing_space_is_preserved() {
        let raw = raw(r"Name=Trailing\s");
        assert_eq!(raw.get("Name"), Some("Trailing "));
    }

    // --- localization ------------------------------------------------------

    const NAMES: &str = "\
Name=Base
Name[fr]=Francais
Name[fr_FR]=France
Name[fr_FR@euro]=FranceEuro
Name[fr@euro]=FrEuro
Name[de]=Deutsch
Name[sr@latin]=SrLatin
Name[sr_RS]=SrCyr
";

    #[test]
    fn localized_prefers_country_and_modifier() {
        let raw = raw(NAMES);
        assert_eq!(
            raw.localized("Name", Some("fr_FR.UTF-8@euro")),
            Some("FranceEuro")
        );
        assert_eq!(raw.localized("Name", Some("fr@euro")), Some("FrEuro"));
        assert_eq!(raw.localized("Name", Some("fr_FR.UTF-8")), Some("France"));
        assert_eq!(raw.localized("Name", Some("fr_FR")), Some("France"));
        assert_eq!(raw.localized("Name", Some("fr_CA.UTF-8")), Some("Francais"));
        assert_eq!(raw.localized("Name", Some("de_DE")), Some("Deutsch"));
    }

    #[test]
    fn modifier_candidates_precede_country_candidates() {
        let raw = raw(NAMES);
        // `sr_RS@latin` is absent, so `sr@latin` must win over `sr_RS`.
        assert_eq!(raw.localized("Name", Some("sr_RS@latin")), Some("SrLatin"));
        assert_eq!(raw.localized("Name", Some("sr_RS")), Some("SrCyr"));
    }

    #[test]
    fn localized_falls_back_to_base() {
        let raw = raw(NAMES);
        assert_eq!(raw.localized("Name", Some("es_ES")), Some("Base"));
        assert_eq!(raw.localized("Name", Some("es")), Some("Base"));
        assert_eq!(raw.localized("Name", None), Some("Base"));
        assert_eq!(raw.localized("Name", Some("")), Some("Base"));
        assert_eq!(raw.localized("Name", Some(".UTF-8")), Some("Base"));
        assert_eq!(raw.localized("Name", Some("_FR")), Some("Base"));
        assert_eq!(raw.localized("Name", Some("@euro")), Some("Base"));
    }

    #[test]
    fn localized_only_key_without_base() {
        let raw = raw("Name[de]=Deutsch\n");
        assert_eq!(raw.localized("Name", Some("de_AT.UTF-8")), Some("Deutsch"));
        assert_eq!(raw.localized("Name", Some("fr")), None);
        assert_eq!(raw.localized("Name", None), None);
        assert_eq!(raw.get("Name"), None);
    }

    #[test]
    fn empty_localized_value_falls_back_to_base() {
        let raw = raw("Name=Base\nName[fr]=\n");
        assert_eq!(raw.get("Name[fr]"), Some(""));
        assert_eq!(raw.localized("Name", Some("fr")), Some("Base"));
    }

    #[test]
    fn localization_is_per_key() {
        let raw = raw("Name=Base\nName[fr]=F\nComment=Base comment\nComment[fr]=Commentaire\n");
        assert_eq!(raw.localized("Name", Some("fr")), Some("F"));
        assert_eq!(raw.localized("Comment", Some("fr")), Some("Commentaire"));
        assert_eq!(raw.localized("Missing", Some("fr")), None);
        assert_eq!(raw.get("Name"), Some("Base"), "get ignores localized keys");
    }

    // --- structure ---------------------------------------------------------

    #[test]
    fn missing_group_is_reported() {
        assert_eq!(parse_str("").unwrap_err(), ParseError::MissingGroup);
        assert_eq!(
            parse_str("Name=Foo\n").unwrap_err(),
            ParseError::MissingGroup
        );
        assert_eq!(
            parse_str("[Other]\nName=Foo\n").unwrap_err(),
            ParseError::MissingGroup
        );
        assert_eq!(
            parse_str("[]\nName=Foo\n").unwrap_err(),
            ParseError::MissingGroup
        );
    }

    #[test]
    fn ignores_comments_blanks_and_malformed_lines() {
        let raw = parse_str(
            "# leading comment\n\
             \n\
             Name=Outside\n\
             [Desktop Entry]\n\
             # comment inside\n\
             \n\
             NoEqualsSign\n\
             Name=Real\n\
             =empty key\n\
             Name2\n",
        )
        .unwrap();
        assert_eq!(raw.get("Name"), Some("Real"));
        assert_eq!(raw.get("Name2"), None);
        assert_eq!(raw.get(""), None);
    }

    #[test]
    fn only_the_desktop_entry_group_is_read() {
        let raw = parse_str(
            "[Desktop Action new]\n\
             Name=Action\n\
             Exec=action\n\
             [Desktop Entry]\n\
             Name=Real\n\
             Exec=real\n\
             [Desktop Action other]\n\
             Name=Other\n\
             Exec=other\n",
        )
        .unwrap();
        assert_eq!(raw.get("Name"), Some("Real"));
        assert_eq!(raw.get("Exec"), Some("real"));
    }

    #[test]
    fn repeated_desktop_entry_groups_merge_last_wins() {
        let raw = parse_str(
            "[Desktop Entry]\nName=First\nIcon=a\n\
             [Other]\nName=Ignored\n\
             [Desktop Entry]\nName=Second\n",
        )
        .unwrap();
        assert_eq!(raw.get("Name"), Some("Second"));
        assert_eq!(raw.get("Icon"), Some("a"));
    }

    #[test]
    fn duplicate_keys_keep_the_last_occurrence() {
        let raw = raw("Name=First\nName=Second\nName=Third\nName[fr]=Un\nName[fr]=Deux\n");
        assert_eq!(raw.get("Name"), Some("Third"));
        assert_eq!(raw.localized("Name", Some("fr")), Some("Deux"));
    }

    #[test]
    fn trims_key_and_value_and_splits_on_first_equals() {
        let raw = raw("  Name  =  Spaced Name  \nExec=env FOO=bar prog %U\n");
        assert_eq!(raw.get("Name"), Some("Spaced Name"));
        assert_eq!(raw.get("Exec"), Some("env FOO=bar prog %U"));
    }

    #[test]
    fn accepts_crlf_and_leading_bom() {
        let raw = parse_str("\u{feff}[Desktop Entry]\r\nName=Windows\r\nExec=prog %U\r\n").unwrap();
        assert_eq!(raw.get("Name"), Some("Windows"));
        assert_eq!(raw.get("Exec"), Some("prog %U"));
    }

    #[test]
    fn malformed_input_never_panics() {
        for text in [
            "[",
            "]",
            "[]",
            "[Desktop Entry",
            "[Desktop Entry]",
            "Name",
            "=v",
            "[Desktop Entry]\n=",
            "[Desktop Entry]\r",
            "\u{feff}",
            "[Desktop Entry]\n\\",
        ] {
            let _ = parse_str(text);
        }
        assert!(parse_str("[Desktop Entry]").unwrap().get("Name").is_none());
    }

    // --- typing / validation ----------------------------------------------

    #[test]
    fn missing_or_empty_type_is_rejected() {
        assert_eq!(entry_err("Name=Example\n"), EntryError::MissingType);
        assert_eq!(entry_err("Type=\nName=Example\n"), EntryError::MissingType);
        assert_eq!(
            entry_err("Type=   \nName=Example\n"),
            EntryError::MissingType
        );
    }

    #[test]
    fn non_application_type_is_rejected_case_sensitively() {
        assert_eq!(
            entry_err("Type=Link\nName=Example\n"),
            EntryError::NotApplication("Link".into())
        );
        assert_eq!(
            entry_err("Type=application\nName=Example\n"),
            EntryError::NotApplication("application".into())
        );
        assert_eq!(
            entry_err("Type=APPLICATION\nName=Example\n"),
            EntryError::NotApplication("APPLICATION".into())
        );
    }

    #[test]
    fn missing_or_empty_name_is_rejected() {
        assert_eq!(entry_err("Type=Application\n"), EntryError::MissingName);
        assert_eq!(
            entry_err("Type=Application\nName=\n"),
            EntryError::MissingName
        );
        assert_eq!(
            entry_err("Type=Application\nName=   \n"),
            EntryError::MissingName
        );
        assert_eq!(
            DesktopEntry::from_raw(
                app_id(),
                path(),
                &raw("Type=Application\nName[fr]=Seulement\n"),
                Some("de"),
            )
            .unwrap_err(),
            EntryError::MissingName
        );
    }

    #[test]
    fn localized_name_is_used_and_falls_back() {
        let lines = "Type=Application\nName=Base\nName[fr]=Francais\n";
        let fr = DesktopEntry::from_raw(app_id(), path(), &raw(lines), Some("fr_FR.UTF-8"))
            .unwrap()
            .name;
        let de = DesktopEntry::from_raw(app_id(), path(), &raw(lines), Some("de"))
            .unwrap()
            .name;
        let none = DesktopEntry::from_raw(app_id(), path(), &raw(lines), None)
            .unwrap()
            .name;
        assert_eq!(fr, "Francais");
        assert_eq!(de, "Base");
        assert_eq!(none, "Base");
    }

    #[test]
    fn booleans_are_true_only_for_true_case_insensitively() {
        for value in ["true", "TRUE", "True", "TrUe"] {
            let entry = entry(&format!("Type=Application\nName=X\nTerminal={value}\n"));
            assert!(entry.terminal, "Terminal={value:?} must be true");
        }
        for value in [
            "false", "FALSE", "1", "0", "yes", "no", "tru", "trueish", "",
        ] {
            let entry = entry(&format!("Type=Application\nName=X\nTerminal={value}\n"));
            assert!(!entry.terminal, "Terminal={value:?} must be false");
        }
        assert!(entry("Type=Application\nName=X\nTerminal= true \n").terminal);
    }

    #[test]
    fn every_boolean_key_is_read() {
        let enabled =
            entry("Type=Application\nName=X\nTerminal=true\nHidden=TRUE\nNoDisplay=True\nDBusActivatable=TrUe\n");
        assert!(enabled.terminal);
        assert!(enabled.hidden);
        assert!(enabled.no_display);
        assert!(enabled.dbus_activatable);

        let disabled = entry("Type=Application\nName=X\n");
        assert!(!disabled.terminal);
        assert!(!disabled.hidden);
        assert!(!disabled.no_display);
        assert!(!disabled.dbus_activatable);
    }

    #[test]
    fn categories_are_split_trimmed_and_filtered() {
        assert_eq!(
            entry("Type=Application\nName=X\nCategories=Network;WebBrowser;\n").categories,
            vec!["Network", "WebBrowser"]
        );
        assert_eq!(
            entry("Type=Application\nName=X\nCategories=A; B ;;C\n").categories,
            vec!["A", "B", "C"]
        );
        assert_eq!(
            entry("Type=Application\nName=X\nCategories=\n").categories,
            Vec::<String>::new()
        );
        assert_eq!(
            entry("Type=Application\nName=X\nCategories=;\n").categories,
            Vec::<String>::new()
        );
        assert_eq!(
            entry("Type=Application\nName=X\n").categories,
            Vec::<String>::new()
        );
    }

    #[test]
    fn hidden_entries_still_type_successfully() {
        let entry = entry("Type=Application\nName=X\nHidden=true\n");
        assert!(entry.hidden);
        assert!(!entry.is_listable(false));
        assert!(entry.is_listable(true));
    }

    #[test]
    fn is_listable_matrix() {
        let visible = entry("Type=Application\nName=X\n");
        let hidden = entry("Type=Application\nName=X\nHidden=true\n");
        let no_display = entry("Type=Application\nName=X\nNoDisplay=true\n");
        let both = entry("Type=Application\nName=X\nHidden=true\nNoDisplay=true\n");

        assert!(visible.is_listable(false) && visible.is_listable(true));
        assert!(!hidden.is_listable(false) && hidden.is_listable(true));
        assert!(!no_display.is_listable(false) && no_display.is_listable(true));
        assert!(!both.is_listable(false) && both.is_listable(true));
    }
}
