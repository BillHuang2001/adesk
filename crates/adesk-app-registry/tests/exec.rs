//! Integration tests for `Exec` tokenization and field-code expansion.
//!
//! Public API only: the tables here pin the two-stage semantics documented on
//! `adesk_app_registry::exec` — quoting escapes in [`ExecExpander::tokenize`],
//! field codes in [`ExecExpander::expand_tokens`].

mod support;

use std::path::Path;

use adesk_app_registry::{ExecContext, ExecError, ExecExpander};

use support::entry_at;

fn tokenize(exec: &str) -> Vec<String> {
    ExecExpander::new().tokenize(exec).expect("tokenize")
}

fn tokenize_error(exec: &str) -> ExecError {
    ExecExpander::new()
        .tokenize(exec)
        .expect_err("expected a tokenization error")
}

fn expand(tokens: &[&str], context: &ExecContext<'_>) -> Vec<String> {
    let owned: Vec<String> = tokens.iter().map(|token| (*token).to_string()).collect();
    ExecExpander::new().expand_tokens(&owned, context)
}

fn files(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

// --- tokenize: whitespace and quoting --------------------------------------

#[test]
fn tokenize_splits_on_whitespace_runs() {
    assert_eq!(
        tokenize("  /usr/bin/app\t--flag\nvalue \r\n"),
        ["/usr/bin/app", "--flag", "value"]
    );
}

#[test]
fn tokenize_empty_input_yields_no_arguments() {
    assert_eq!(tokenize(""), Vec::<String>::new());
    assert_eq!(tokenize("   \t\n "), Vec::<String>::new());
}

#[test]
fn tokenize_double_quotes_group_and_are_removed() {
    assert_eq!(
        tokenize(r#"/usr/bin/app --title "hello world""#),
        ["/usr/bin/app", "--title", "hello world"]
    );
    assert_eq!(tokenize(r#""a" "b""#), ["a", "b"]);
    assert_eq!(tokenize(r#"foo"bar baz"qux"#), ["foobar bazqux"]);
}

#[test]
fn tokenize_preserves_empty_quoted_arguments() {
    assert_eq!(tokenize("cmd \"\""), ["cmd", ""]);
    assert_eq!(tokenize("\"\" \"\""), ["", ""]);
    assert_eq!(tokenize("cmd \"\" --flag"), ["cmd", "", "--flag"]);
    assert_eq!(tokenize("\"\""), [""]);
    assert_eq!(tokenize("foo\"\"bar"), ["foobar"]);
    assert_eq!(tokenize("\"\"foo"), ["foo"]);
}

#[test]
fn tokenize_single_quotes_are_literal() {
    assert_eq!(tokenize("cmd 'a b'"), ["cmd", "'a", "b'"]);
    assert_eq!(tokenize(r#""'a b'""#), ["'a b'"]);
}

#[test]
fn tokenize_quoting_escape_table() {
    let cases = [
        (r#""a\"b""#, r#"a"b"#),
        (r#""a\\b""#, r"a\b"),
        (r#""a\`b""#, "a`b"),
        (r#""a\$b""#, "a$b"),
        (r#""\"\\\`\$""#, r#""\`$"#),
        (r"a\\b", r"a\b"),
        (r#"a\"b"#, r#"a"b"#),
        (r"a\`b", "a`b"),
        (r"a\$b", "a$b"),
        // Only `\"`, `` \` ``, `\$` and `\\` are quoting escapes; anything else
        // keeps the backslash verbatim.
        (r#""a\xb""#, r"a\xb"),
        (r"a\xb", r"a\xb"),
        (r"a\tb", r"a\tb"),
        (r"a\ b", r"a\"),
        (r"a\", r"a\"),
        (r"a\\", r"a\"),
    ];
    for (exec, expected) in cases {
        let tokens = tokenize(exec);
        assert_eq!(
            tokens.first().map(String::as_str),
            Some(expected),
            "exec {exec:?}"
        );
    }
    // `a\ b`: the unknown escape keeps `\`, then the space separates arguments.
    assert_eq!(tokenize(r"a\ b"), [r"a\", "b"]);
}

#[test]
fn tokenize_escaped_quote_does_not_close_the_quoted_section() {
    assert_eq!(tokenize(r#""a\" b" c"#), [r#"a" b"#, "c"]);
    assert_eq!(
        tokenize_error(r#""a\""#),
        ExecError::UnterminatedQuote { offset: 0 }
    );
}

#[test]
fn tokenize_reports_the_opening_quote_offset() {
    assert_eq!(
        tokenize_error(r#"cmd "abc"#),
        ExecError::UnterminatedQuote { offset: 4 }
    );
    assert_eq!(
        tokenize_error(r#""abc"#),
        ExecError::UnterminatedQuote { offset: 0 }
    );
    assert_eq!(
        tokenize_error(r#""a" "b"#),
        ExecError::UnterminatedQuote { offset: 4 },
        "the last opening quote is reported"
    );
    // `é` is two bytes, so the opening quote sits at byte 4.
    assert_eq!(
        tokenize_error("aé \"unterminated"),
        ExecError::UnterminatedQuote { offset: 4 }
    );
}

// --- expand_tokens: standalone file codes ----------------------------------

#[test]
fn single_file_codes_take_the_first_file() {
    let paths = files(&["/tmp/a", "/tmp/b"]);
    let context = ExecContext {
        files: &paths,
        ..ExecContext::default()
    };
    assert_eq!(
        expand(&["app", "%f", "--flag"], &context),
        ["app", "/tmp/a", "--flag"]
    );
    assert_eq!(expand(&["app", "%u"], &context), ["app", "/tmp/a"]);
}

#[test]
fn single_file_codes_vanish_without_files() {
    let context = ExecContext::default();
    assert_eq!(
        expand(&["app", "%f", "--flag"], &context),
        ["app", "--flag"]
    );
    assert_eq!(expand(&["%u"], &context), Vec::<String>::new());
}

#[test]
fn multi_file_codes_take_every_file() {
    let paths = files(&["/tmp/a", "/tmp/b", "/tmp/c"]);
    let context = ExecContext {
        files: &paths,
        ..ExecContext::default()
    };
    assert_eq!(
        expand(&["app", "%F", "--flag"], &context),
        ["app", "/tmp/a", "/tmp/b", "/tmp/c", "--flag"]
    );
    assert_eq!(expand(&["%U"], &context), ["/tmp/a", "/tmp/b", "/tmp/c"]);
}

#[test]
fn multi_file_codes_vanish_without_files() {
    let context = ExecContext::default();
    assert_eq!(
        expand(&["app", "%F", "%U", "--flag"], &context),
        ["app", "--flag"]
    );
}

#[test]
fn embedded_file_codes_stay_verbatim() {
    let paths = files(&["/tmp/a"]);
    let context = ExecContext {
        files: &paths,
        ..ExecContext::default()
    };
    assert_eq!(
        expand(
            &["--file=%f", "--file=%u", "--file=%F", "--file=%U"],
            &context
        ),
        ["--file=%f", "--file=%u", "--file=%F", "--file=%U"]
    );
}

// --- expand_tokens: icon ---------------------------------------------------

#[test]
fn standalone_icon_expands_to_two_arguments() {
    let context = ExecContext {
        icon: Some("my-icon"),
        ..ExecContext::default()
    };
    assert_eq!(
        expand(&["app", "%i", "--flag"], &context),
        ["app", "--icon", "my-icon", "--flag"]
    );
}

#[test]
fn standalone_icon_vanishes_without_icon() {
    let context = ExecContext::default();
    assert_eq!(expand(&["app", "%i"], &context), ["app"]);
}

#[test]
fn embedded_icon_is_verbatim() {
    let context = ExecContext {
        icon: Some("my-icon"),
        ..ExecContext::default()
    };
    assert_eq!(
        expand(&["--icon=%i", "%i%i"], &context),
        ["--icon=%i", "%i%i"]
    );
    assert_eq!(
        expand(&["--icon=%i"], &ExecContext::default()),
        ["--icon=%i"]
    );
}

// --- expand_tokens: in-token substitutions ---------------------------------

#[test]
fn name_and_desktop_file_substitute_inside_a_token() {
    let path = Path::new("/usr/share/applications/org.example.app.desktop");
    let context = ExecContext {
        name: "My App",
        desktop_file: Some(path),
        ..ExecContext::default()
    };
    assert_eq!(
        expand(&["--name=%c", "%c", "pre%cpost", "%c%c"], &context),
        ["--name=My App", "My App", "preMy Apppost", "My AppMy App"]
    );
    assert_eq!(
        expand(&["%k", "--desktop=%k"], &context),
        [
            "/usr/share/applications/org.example.app.desktop",
            "--desktop=/usr/share/applications/org.example.app.desktop"
        ]
    );
}

#[test]
fn missing_desktop_file_removes_only_the_code() {
    let context = ExecContext::default();
    assert_eq!(expand(&["--desktop=%k"], &context), ["--desktop="]);
    assert_eq!(expand(&["%k"], &context), Vec::<String>::new());
}

#[test]
fn double_percent_is_a_literal_percent() {
    let context = ExecContext::default();
    assert_eq!(
        expand(&["100%%", "%%", "a%%b%%c"], &context),
        ["100%", "%", "a%b%c"]
    );
    // `%%f` is a literal `%f`, not a file code.
    assert_eq!(expand(&["%%f"], &context), ["%f"]);
}

#[test]
fn deprecated_codes_are_removed() {
    let context = ExecContext::default();
    assert_eq!(
        expand(
            &["app", "%d", "%D", "%n", "%N", "%v", "%m", "--flag"],
            &context
        ),
        ["app", "--flag"]
    );
    assert_eq!(expand(&["a%Db", "%n%m", "%v%N"], &context), ["ab"]);
}

#[test]
fn unknown_codes_and_a_lone_percent_stay_verbatim() {
    let context = ExecContext::default();
    assert_eq!(
        expand(
            &["%x", "%z%y", "%1", "%", "pre%post", "%%%d", "%C"],
            &context
        ),
        ["%x", "%z%y", "%1", "%", "pre%post", "%", "%C"]
    );
}

#[test]
fn empty_arguments_are_preserved_but_removals_drop_the_argument() {
    let context = ExecContext::default();
    assert_eq!(
        expand(&["", "app", "", "--flag"], &context),
        ["", "app", "", "--flag"]
    );
    assert_eq!(expand(&["app", "%d", "%k", "%%%d"], &context), ["app", "%"]);
}

#[test]
fn name_code_with_an_empty_name_only_empties_embedded_tokens() {
    let context = ExecContext::default();
    assert_eq!(expand(&["%c", "--name=%c"], &context), ["--name="]);
}

#[test]
fn default_context_only_drops_macros() {
    assert_eq!(
        expand(
            &["app", "--flag", "%f", "%F", "%u", "%U", "%i", "%c", "%k"],
            &ExecContext::default()
        ),
        ["app", "--flag"]
    );
}

// --- expand: end to end ----------------------------------------------------

#[test]
fn expand_tokenizes_then_expands() {
    let paths = files(&["https://example.invalid"]);
    let context = ExecContext {
        name: "Firefox",
        icon: Some("firefox"),
        desktop_file: Some(Path::new("/usr/share/applications/firefox.desktop")),
        files: &paths,
    };
    assert_eq!(
        ExecExpander::new()
            .expand(r#"/usr/bin/firefox --name="%c" %i %u %% %d"#, &context)
            .expect("expand"),
        [
            "/usr/bin/firefox",
            "--name=Firefox",
            "--icon",
            "firefox",
            "https://example.invalid",
            "%",
        ]
    );
}

#[test]
fn expand_an_entry_from_disk_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = entry_at(
        dir.path(),
        "org.example.editor.desktop",
        "Type=Application\nName=Editor\nIcon=editor-icon\nExec=/usr/bin/editor --name=%c %i %U %% %d\n",
        None,
    );
    let paths = files(&["/tmp/one.txt", "/tmp/two.txt"]);
    let context = ExecContext {
        name: entry.name.as_str(),
        icon: entry.icon.as_deref(),
        desktop_file: Some(&entry.path),
        files: &paths,
    };
    assert_eq!(
        ExecExpander::new()
            .expand(entry.exec.as_deref().expect("Exec present"), &context)
            .expect("expand"),
        [
            "/usr/bin/editor",
            "--name=Editor",
            "--icon",
            "editor-icon",
            "/tmp/one.txt",
            "/tmp/two.txt",
            "%",
        ]
    );
}

#[test]
fn expand_propagates_tokenization_errors() {
    assert_eq!(
        ExecExpander::new().expand(r#"app "unterminated"#, &ExecContext::default()),
        Err(ExecError::UnterminatedQuote { offset: 4 })
    );
}

#[test]
fn expand_empty_exec_line_yields_no_arguments() {
    assert_eq!(
        ExecExpander::new()
            .expand("   ", &ExecContext::default())
            .expect("expand"),
        Vec::<String>::new()
    );
    assert_eq!(
        ExecExpander::new()
            .expand(r#"/bin/sh -c "echo hi""#, &ExecContext::default())
            .expect("expand"),
        ["/bin/sh", "-c", "echo hi"]
    );
}

#[test]
fn expander_new_equals_default() {
    assert_eq!(ExecExpander::new(), ExecExpander);
    assert_eq!(
        ExecExpander::new().tokenize("a b").expect("tokenize"),
        ["a", "b"]
    );
}

// --- support seam ----------------------------------------------------------

/// Sanity check for the shared mock spawner: it records every command and honours
/// the configured outcome, so the later `tests/registry.rs` wave can rely on it.
#[test]
fn support_recording_spawner_seam() {
    use adesk_app_registry::{ProcessSpawner, SpawnCommand};

    let spawner = support::RecordingSpawner::new();
    assert!(spawner.is_empty());

    let command = SpawnCommand::new("/usr/bin/editor")
        .with_arg("--new")
        .with_env("EDITOR_MODE", "agent");
    let spawned = spawner.spawn(&command).expect("spawn succeeds");
    assert_eq!(spawned.pid, Some(support::SPAWN_PID));
    assert_eq!(spawner.len(), 1);
    assert_eq!(spawner.last(), Some(command.clone()));
    assert_eq!(spawner.commands(), vec![command.clone()]);

    spawner.set_outcome(support::SpawnOutcome::Succeed(None));
    assert_eq!(spawner.spawn(&command).expect("spawn succeeds").pid, None);

    spawner.fail_with("spawn rejected");
    let error = spawner.spawn(&command).expect_err("configured failure");
    assert_eq!(error.to_string(), "spawn rejected");
    assert_eq!(spawner.len(), 3, "failed attempts are recorded too");

    spawner.clear();
    assert!(spawner.is_empty());
}
