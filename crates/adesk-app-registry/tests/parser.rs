//! Integration tests for `[Desktop Entry]` parsing and typing.
//!
//! Public API only: fixtures are written to a `tempfile::tempdir()`, read back
//! through [`parse_str`] and typed with [`DesktopEntry::from_raw`], exactly like
//! `AppRegistry::scan` does.

mod support;

use adesk_app_registry::{parse_str, DesktopEntry, EntryError, ParseError, RawEntry};
use adesk_core::AppInfo;

use support::{entry_at, parse_body, try_entry_at};

/// Localized-name fixture covering base, language, country and modifier keys.
const NAMES: &str = "\
Type=Application
Name=Base
Name[fr]=Francais
Name[fr_FR]=France
Name[fr_FR@euro]=FranceEuro
Name[fr@euro]=FrEuro
Name[de]=Deutsch
Name[sr@latin]=SrLatin
Name[sr_RS]=SrCyr
";

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

// --- happy path ------------------------------------------------------------

#[test]
fn parses_a_full_entry_from_disk_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(dir.path(), "org.example.app.desktop", FULL, None);

    assert_eq!(entry.id, support::app("org.example.app"));
    assert_eq!(entry.path, dir.path().join("org.example.app.desktop"));
    assert_eq!(entry.name, "Example");
    assert_eq!(entry.exec.as_deref(), Some("/usr/bin/example --flag %U"));
    assert_eq!(entry.icon.as_deref(), Some("example-icon"));
    assert!(entry.terminal);
    assert!(!entry.no_display);
    assert!(!entry.hidden);
    assert_eq!(entry.categories, ["Utility", "Core"]);
    assert_eq!(entry.startup_wm_class.as_deref(), Some("example-wm"));
    assert!(entry.dbus_activatable);
    assert_eq!(entry.try_exec.as_deref(), Some("/usr/bin/example"));

    // The raw layer keeps non-special keys verbatim; absent keys stay absent.
    let raw = parse_body(FULL);
    assert_eq!(raw.get("GenericName"), Some("An example"));
    assert_eq!(raw.get("Missing"), None);
}

#[test]
fn nested_entries_get_a_dotted_id_from_their_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(
        dir.path(),
        "kde/kate.desktop",
        "Type=Application\nName=Kate\n",
        None,
    );
    assert_eq!(entry.id, support::app("kde.kate"));
    assert_eq!(entry.path, dir.path().join("kde").join("kate.desktop"));
}

#[test]
fn raw_lookup_ignores_localized_keys() {
    let raw = parse_body(NAMES);
    assert_eq!(raw.get("Name"), Some("Base"));
    assert_eq!(raw.get("Name[fr]"), Some("Francais"));
    assert_eq!(raw.get("name"), None, "keys are case-sensitive");
    assert_eq!(raw.localized("Name", Some("fr")), Some("Francais"));
    assert_eq!(raw.localized("Name", None), Some("Base"));

    // `localized` is key-generic and yields `None` when nothing matches.
    let per_key = parse_body("Name=Base\nName[fr]=F\nComment=Base\nComment[fr]=Commentaire\n");
    assert_eq!(per_key.localized("Name", Some("fr")), Some("F"));
    assert_eq!(
        per_key.localized("Comment", Some("fr")),
        Some("Commentaire")
    );
    assert_eq!(per_key.localized("Missing", Some("fr")), None);

    // A localized-only key resolves for its locale and is invisible otherwise.
    let localized_only = parse_body("Name[de]=Deutsch\n");
    assert_eq!(
        localized_only.localized("Name", Some("de_AT.UTF-8")),
        Some("Deutsch")
    );
    assert_eq!(localized_only.localized("Name", Some("fr")), None);
    assert_eq!(localized_only.localized("Name", None), None);
    assert_eq!(localized_only.get("Name"), None);
}

// --- localization ----------------------------------------------------------

#[test]
fn localized_name_precedence_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        ("fr_FR.UTF-8@euro", "FranceEuro"),
        ("fr_FR@euro", "FranceEuro"),
        ("fr@euro", "FrEuro"),
        ("fr_FR.UTF-8", "France"),
        ("fr_FR", "France"),
        ("fr_CA.UTF-8", "Francais"),
        ("fr", "Francais"),
        ("de_DE", "Deutsch"),
        ("de", "Deutsch"),
        ("sr_RS@latin", "SrLatin"),
        ("sr_RS", "SrCyr"),
    ];
    for (locale, expected) in cases {
        let entry = entry_at(dir.path(), "org.example.names.desktop", NAMES, Some(locale));
        assert_eq!(entry.name, expected, "locale {locale:?}");
    }
}

#[test]
fn localized_name_falls_back_to_the_base_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    for locale in ["es_ES", "es", "", ".UTF-8", "_FR", "@euro", "fr.CP1252"] {
        let entry = entry_at(dir.path(), "org.example.names.desktop", NAMES, Some(locale));
        // `fr.CP1252` drops the encoding and finds `Name[fr]`.
        let expected = if locale == "fr.CP1252" {
            "Francais"
        } else {
            "Base"
        };
        assert_eq!(entry.name, expected, "locale {locale:?}");
    }

    let entry = entry_at(dir.path(), "org.example.names.desktop", NAMES, None);
    assert_eq!(entry.name, "Base", "no locale uses the base value");
}

#[test]
fn modifier_candidates_precede_country_candidates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(
        dir.path(),
        "org.example.names.desktop",
        "Type=Application\nName=Base\nName[sr@latin]=SrLatin\nName[sr_RS]=SrCyr\n",
        Some("sr_RS@latin"),
    );
    assert_eq!(entry.name, "SrLatin");
}

#[test]
fn empty_localized_value_never_shadows_the_base_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(
        dir.path(),
        "org.example.names.desktop",
        "Type=Application\nName=Base\nName[fr]=\n",
        Some("fr"),
    );
    assert_eq!(entry.name, "Base");
    assert_eq!(
        parse_body("Type=Application\nName=Base\nName[fr]=\n").get("Name[fr]"),
        Some("")
    );
}

#[test]
fn only_name_is_localized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName=Base\nName[fr]=Nom\nExec=/usr/bin/base\nExec[fr]=/usr/bin/fr\nIcon=base-icon\nIcon[fr]=fr-icon\n",
        Some("fr"),
    );
    assert_eq!(entry.name, "Nom");
    assert_eq!(entry.exec.as_deref(), Some("/usr/bin/base"));
    assert_eq!(entry.icon.as_deref(), Some("base-icon"));
}

#[test]
fn localized_only_name_is_rejected_when_the_locale_does_not_match() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = try_entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName[fr]=Seulement\n",
        Some("de"),
    )
    .expect_err("no usable Name");
    assert_eq!(err, EntryError::MissingName);

    let entry = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName[fr]=Seulement\n",
        Some("fr"),
    );
    assert_eq!(entry.name, "Seulement");
}

// --- stage-1 escapes -------------------------------------------------------

#[test]
fn stage_one_escape_table() {
    let cases = [
        (r"a\sb", "a b"),
        (r"a\nb", "a\nb"),
        (r"a\tb", "a\tb"),
        (r"a\rb", "a\rb"),
        (r"a\\b", r"a\b"),
        (r"a\db", r"a\db"),
        (r"a\$b", r"a\$b"),
        (r#"a\"b"#, r#"a\"b"#),
        (r"a\ b", r"a\ b"),
        (r"a\sb\nc\td\re\\f", "a b\nc\td\re\\f"),
        (r"trailing\", r"trailing\"),
        (r"Trailing\s", "Trailing "),
    ];
    for (value, expected) in cases {
        let raw = parse_body(&format!("Name={value}\n"));
        assert_eq!(raw.get("Name"), Some(expected), "value {value:?}");
    }
}

#[test]
fn unknown_escapes_stay_verbatim_for_exec_stage_two() {
    let raw = parse_body(r#"Exec=prog \%U \"quoted arg\" \x"#);
    assert_eq!(raw.get("Exec"), Some(r#"prog \%U \"quoted arg\" \x"#));
}

// --- structure tolerance ---------------------------------------------------

#[test]
fn comments_blank_lines_and_malformed_lines_are_ignored() {
    let raw = parse_str(
        "# leading comment\n\
         \n\
         Name=OutsideAnyGroup\n\
         [Desktop Entry]\n\
         # comment inside\n\
         \n\
         NoEqualsSign\n\
         Name=Real\n\
         =empty key\n\
         TrailingKey\n",
    )
    .expect("group present");
    assert_eq!(raw.get("Name"), Some("Real"));
    assert_eq!(raw.get("NoEqualsSign"), None);
    assert_eq!(raw.get("TrailingKey"), None);
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
    .expect("group present");
    assert_eq!(raw.get("Name"), Some("Real"));
    assert_eq!(raw.get("Exec"), Some("real"));
}

#[test]
fn group_headers_are_trimmed() {
    let raw = parse_str("[ Desktop Entry ]\nName=Trimmed\n").expect("group present");
    assert_eq!(raw.get("Name"), Some("Trimmed"));
}

#[test]
fn repeated_desktop_entry_groups_merge_with_last_wins() {
    let raw = parse_str(
        "[Desktop Entry]\nName=First\nIcon=a\n\
         [Other]\nName=Ignored\n\
         [Desktop Entry]\nName=Second\n",
    )
    .expect("group present");
    assert_eq!(raw.get("Name"), Some("Second"));
    assert_eq!(raw.get("Icon"), Some("a"), "earlier values survive");
}

#[test]
fn duplicate_keys_keep_the_last_occurrence() {
    let raw = parse_body("Name=First\nName=Second\nName=Third\nName[fr]=Un\nName[fr]=Deux\n");
    assert_eq!(raw.get("Name"), Some("Third"));
    assert_eq!(raw.localized("Name", Some("fr")), Some("Deux"));
}

#[test]
fn missing_group_is_reported() {
    for text in ["", "Name=Foo\n", "[Other]\nName=Foo\n", "[]\nName=Foo\n"] {
        assert_eq!(
            parse_str(text).expect_err("no [Desktop Entry]"),
            ParseError::MissingGroup,
            "input {text:?}"
        );
    }
}

#[test]
fn key_and_value_are_trimmed_and_split_on_the_first_equals() {
    let raw = parse_body("  Name  =  Spaced Name  \nExec=env FOO=bar prog %U\n");
    assert_eq!(raw.get("Name"), Some("Spaced Name"));
    assert_eq!(raw.get("Exec"), Some("env FOO=bar prog %U"));
}

#[test]
fn accepts_crlf_line_endings_and_a_leading_bom() {
    let raw = parse_str("\u{feff}[Desktop Entry]\r\nName=Windows\r\nExec=prog %U\r\n")
        .expect("group present");
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
    let empty = parse_str("[Desktop Entry]").expect("group present");
    assert_eq!(empty.get("Name"), None);
}

// --- typing / validation ---------------------------------------------------

#[test]
fn missing_or_empty_type_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    for body in [
        "Name=Example\n",
        "Type=\nName=Example\n",
        "Type=   \nName=Example\n",
    ] {
        let err = try_entry_at(dir.path(), "org.example.app.desktop", body, None)
            .expect_err("no usable Type");
        assert_eq!(err, EntryError::MissingType, "body {body:?}");
    }
}

#[test]
fn non_application_type_is_rejected_case_sensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    for value in ["Link", "application", "APPLICATION", "ApplicationX", "Dir"] {
        let body = format!("Type={value}\nName=Example\n");
        let err = try_entry_at(dir.path(), "org.example.app.desktop", &body, None)
            .expect_err("not an Application");
        assert_eq!(
            err,
            EntryError::NotApplication(value.to_string()),
            "Type={value:?}"
        );
    }

    // Values are trimmed before typing, so surrounding whitespace is harmless.
    let entry = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type= Application \nName=Example\n",
        None,
    );
    assert_eq!(entry.name, "Example");
}

#[test]
fn missing_or_empty_name_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    for body in [
        "Type=Application\n",
        "Type=Application\nName=\n",
        "Type=Application\nName=   \n",
    ] {
        let err = try_entry_at(dir.path(), "org.example.app.desktop", body, None)
            .expect_err("no usable Name");
        assert_eq!(err, EntryError::MissingName, "body {body:?}");
    }
}

#[test]
fn boolean_keys_are_true_only_for_true_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    for value in ["true", "TRUE", "True", "TrUe", " true "] {
        let body = format!("Type=Application\nName=X\nTerminal={value}\n");
        let entry = entry_at(dir.path(), "org.example.app.desktop", &body, None);
        assert!(entry.terminal, "Terminal={value:?} must be true");
    }
    for value in [
        "false", "FALSE", "1", "0", "yes", "no", "tru", "trueish", "",
    ] {
        let body = format!("Type=Application\nName=X\nTerminal={value}\n");
        let entry = entry_at(dir.path(), "org.example.app.desktop", &body, None);
        assert!(!entry.terminal, "Terminal={value:?} must be false");
    }
}

#[test]
fn every_boolean_key_is_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let enabled = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName=X\nTerminal=true\nHidden=TRUE\nNoDisplay=True\nDBusActivatable=TrUe\n",
        None,
    );
    assert!(enabled.terminal);
    assert!(enabled.hidden);
    assert!(enabled.no_display);
    assert!(enabled.dbus_activatable);

    let disabled = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName=X\n",
        None,
    );
    assert!(!disabled.terminal);
    assert!(!disabled.hidden);
    assert!(!disabled.no_display);
    assert!(!disabled.dbus_activatable);
}

#[test]
fn categories_are_split_trimmed_and_filtered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        (
            "Categories=Network;WebBrowser;\n",
            vec!["Network", "WebBrowser"],
        ),
        ("Categories=A; B ;;C\n", vec!["A", "B", "C"]),
        ("Categories=\n", Vec::new()),
        ("Categories=;\n", Vec::new()),
        ("", Vec::new()),
    ];
    for (line, expected) in cases {
        let body = format!("Type=Application\nName=X\n{line}");
        let entry = entry_at(dir.path(), "org.example.app.desktop", &body, None);
        assert_eq!(entry.categories, expected, "line {line:?}");
    }
}

#[test]
fn absent_and_empty_optional_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName=Minimal\nExec=\nIcon=\nStartupWMClass=\nTryExec=\nCategories=\n",
        None,
    );
    assert_eq!(entry.exec.as_deref(), Some(""), "Exec keeps presence");
    assert_eq!(entry.icon, None);
    assert_eq!(entry.startup_wm_class, None);
    assert_eq!(entry.try_exec, None);
    assert!(entry.categories.is_empty());
}

#[test]
fn to_app_info_projects_every_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(dir.path(), "org.example.app.desktop", FULL, None);
    let expected = AppInfo {
        id: support::app("org.example.app"),
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
    let dir = tempfile::tempdir().expect("tempdir");
    let info = entry_at(
        dir.path(),
        "org.example.app.desktop",
        "Type=Application\nName=X\nHidden=true\nNoDisplay=true\n",
        None,
    )
    .to_app_info();
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
fn is_listable_matrix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cases = [
        ("Type=Application\nName=X\n", true, true),
        ("Type=Application\nName=X\nHidden=true\n", false, true),
        ("Type=Application\nName=X\nNoDisplay=true\n", false, true),
        (
            "Type=Application\nName=X\nHidden=true\nNoDisplay=true\n",
            false,
            true,
        ),
    ];
    for (body, listable, listable_hidden) in cases {
        let entry = entry_at(dir.path(), "org.example.app.desktop", body, None);
        assert_eq!(entry.is_listable(false), listable, "body {body:?}");
        assert_eq!(entry.is_listable(true), listable_hidden, "body {body:?}");
    }
}

#[test]
fn typed_entry_exposes_the_original_path_and_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry: DesktopEntry = entry_at(
        dir.path(),
        "org.example/nested/app.desktop",
        "Type=Application\nName=Nested\n",
        None,
    );
    assert_eq!(entry.id, support::app("org.example.nested.app"));
    assert!(entry.path.ends_with("org.example/nested/app.desktop"));

    let raw: RawEntry = parse_body("Type=Application\nName=Nested\n");
    assert_eq!(raw.get("Type"), Some("Application"));
}
