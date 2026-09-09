//! `Exec` line tokenization and field-code expansion.
//!
//! Two stages, both deterministic and fully table-tested:
//!
//! 1. [`ExecExpander::tokenize`] splits the `Exec` value into arguments: ASCII
//!    whitespace separates arguments, double quotes group (and are removed), and
//!    the quoting escapes `\"` → `"`, `` \` `` → `` ` ``, `\$` → `$`, `\\` → `\`
//!    are resolved. Other `\X` sequences are kept verbatim. Single quotes are not
//!    special. An unterminated quote is [`ExecError::UnterminatedQuote`].
//! 2. [`ExecExpander::expand_tokens`] applies field codes (spec rules, pinned
//!    below) to the tokenized arguments.
//!
//! Field-code rules (design decisions, pinned by tests):
//! - a token that is exactly `%f`/`%u` is replaced by the first
//!   [`ExecContext::files`] entry, `%F`/`%U` by all of them; an empty expansion
//!   removes the token;
//! - a token that is exactly `%i` expands to `--icon <icon>`, or is removed when
//!   [`ExecContext::icon`] is unset;
//! - `%c` is replaced by [`ExecContext::name`], `%k` by the desktop-file path,
//!   `%%` by a literal `%`, anywhere inside a token;
//! - the deprecated codes `%d %D %n %N %v %m` are removed; a token that becomes
//!   empty *because of field-code removal* is dropped, while an empty token that
//!   came from an empty quoted argument is preserved;
//! - unknown `%X` sequences and `%i` outside a standalone token are left verbatim.
//!
//! [`crate::AppRegistry::launch`] passes an empty `files` list (AGP `launch_app`
//! carries no file arguments), so `%f/%F/%u/%U` normally vanish.

use std::path::Path;

/// Values substituted for `Exec` field codes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecContext<'a> {
    /// Localized application name (`%c`).
    pub name: &'a str,
    /// Icon name or path (`%i`).
    pub icon: Option<&'a str>,
    /// Path of the `.desktop` file (`%k`).
    pub desktop_file: Option<&'a Path>,
    /// File/URL arguments spliced for `%f`/`%F`/`%u`/`%U`.
    pub files: &'a [String],
}

/// Failure modes of `Exec` tokenization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExecError {
    /// A double quote was opened but never closed.
    #[error("unterminated quote at byte {offset}")]
    UnterminatedQuote {
        /// Byte offset of the opening quote.
        offset: usize,
    },
}

/// Expands `Exec` lines per the XDG desktop-entry specification.
///
/// Stateless; a value type so callers can hold one per registry or per call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExecExpander;

impl ExecExpander {
    /// Creates an expander.
    pub fn new() -> ExecExpander {
        Self
    }

    /// Splits an `Exec` value into arguments (quotes removed, escapes resolved).
    pub fn tokenize(&self, exec: &str) -> std::result::Result<Vec<String>, ExecError> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        // Whether the token being built exists at all: it is set by the first
        // non-whitespace character *or* by an opening quote, so `""` yields an
        // empty argument while a whitespace run yields nothing.
        let mut started = false;
        // Byte offset of the opening quote of the currently open double-quoted
        // section, if any.
        let mut quote_offset: Option<usize> = None;
        let mut chars = exec.char_indices().peekable();

        while let Some((index, ch)) = chars.next() {
            let in_quotes = quote_offset.is_some();
            match ch {
                '"' => {
                    quote_offset = if in_quotes { None } else { Some(index) };
                    started = true;
                }
                // The quoting escapes apply inside and outside quotes alike;
                // any other `\X` keeps the backslash verbatim.
                '\\' => {
                    match chars.peek().map(|(_, next)| *next) {
                        Some(next) if matches!(next, '"' | '`' | '$' | '\\') => {
                            current.push(next);
                            chars.next();
                        }
                        _ => current.push('\\'),
                    }
                    started = true;
                }
                _ if !in_quotes && ch.is_ascii_whitespace() => {
                    if started {
                        tokens.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    current.push(ch);
                    started = true;
                }
            }
        }

        if let Some(offset) = quote_offset {
            return Err(ExecError::UnterminatedQuote { offset });
        }
        if started {
            tokens.push(current);
        }
        Ok(tokens)
    }

    /// Applies field-code expansion to already tokenized arguments.
    pub fn expand_tokens(&self, tokens: &[String], context: &ExecContext<'_>) -> Vec<String> {
        let mut expanded_tokens = Vec::with_capacity(tokens.len());

        for token in tokens {
            // An empty argument can only come from an explicit `""` (tokenize
            // never emits empty arguments for whitespace runs); it is preserved.
            if token.is_empty() {
                expanded_tokens.push(String::new());
                continue;
            }

            // Codes that own a whole argument: they may remove the argument or
            // expand to several arguments, and are only recognized standalone.
            match token.as_str() {
                "%f" | "%u" => {
                    if let Some(first) = context.files.first() {
                        expanded_tokens.push(first.clone());
                    }
                    continue;
                }
                "%F" | "%U" => {
                    expanded_tokens.extend(context.files.iter().cloned());
                    continue;
                }
                "%i" => {
                    if let Some(icon) = context.icon {
                        expanded_tokens.push("--icon".to_string());
                        expanded_tokens.push(icon.to_string());
                    }
                    continue;
                }
                _ => {}
            }

            let mut expanded = String::with_capacity(token.len());
            let mut chars = token.chars();
            while let Some(ch) = chars.next() {
                if ch != '%' {
                    expanded.push(ch);
                    continue;
                }
                match chars.next() {
                    // A lone trailing `%` is unknown, hence literal.
                    None => expanded.push('%'),
                    Some('c') => expanded.push_str(context.name),
                    // A missing desktop-file path removes the code, keeping the
                    // rest of the token.
                    Some('k') => {
                        if let Some(path) = context.desktop_file {
                            expanded.push_str(&path.to_string_lossy());
                        }
                    }
                    Some('%') => expanded.push('%'),
                    // Deprecated file codes: removed.
                    Some('d' | 'D' | 'n' | 'N' | 'v' | 'm') => {}
                    // Unknown code: verbatim, including `%i` outside a
                    // standalone token.
                    Some(other) => {
                        expanded.push('%');
                        expanded.push(other);
                    }
                }
            }

            // A non-empty argument that expanded to nothing (deprecated code,
            // standalone-with-missing-value, `%c` with an empty name) is dropped.
            if !expanded.is_empty() {
                expanded_tokens.push(expanded);
            }
        }

        expanded_tokens
    }

    /// Tokenizes and expands in one step.
    pub fn expand(
        &self,
        exec: &str,
        context: &ExecContext<'_>,
    ) -> std::result::Result<Vec<String>, ExecError> {
        Ok(self.expand_tokens(&self.tokenize(exec)?, context))
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecContext, ExecError, ExecExpander};
    use std::path::Path;

    fn tokenize(exec: &str) -> Vec<String> {
        ExecExpander::new().tokenize(exec).expect("tokenize")
    }

    fn tokenize_error(exec: &str) -> ExecError {
        ExecExpander::new()
            .tokenize(exec)
            .expect_err("expected tokenization error")
    }

    fn expand(tokens: &[&str], context: &ExecContext<'_>) -> Vec<String> {
        let owned: Vec<String> = tokens.iter().map(|token| (*token).to_string()).collect();
        ExecExpander::new().expand_tokens(&owned, context)
    }

    fn files(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    // --- tokenize: whitespace and quoting -----------------------------------

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
    }

    #[test]
    fn tokenize_preserves_empty_quoted_argument() {
        assert_eq!(tokenize("cmd \"\""), ["cmd", ""]);
        assert_eq!(tokenize("\"\" \"\""), ["", ""]);
        assert_eq!(tokenize("cmd \"\" --flag"), ["cmd", "", "--flag"]);
    }

    #[test]
    fn tokenize_empty_quotes_concatenate_into_one_argument() {
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
    fn tokenize_quotes_concatenate_with_surrounding_text() {
        assert_eq!(tokenize(r#"foo"bar baz"qux"#), ["foobar bazqux"]);
    }

    #[test]
    fn tokenize_resolves_quoting_escapes_inside_quotes() {
        assert_eq!(tokenize(r#""a\"b""#), [r#"a"b"#]);
        assert_eq!(tokenize(r#""a\\b""#), [r"a\b"]);
        assert_eq!(tokenize(r#""a\`b""#), ["a`b"]);
        assert_eq!(tokenize(r#""a\$b""#), ["a$b"]);
        assert_eq!(tokenize(r#""\"\\\`\$""#), [r#""\`$"#]);
    }

    #[test]
    fn tokenize_keeps_unknown_escapes_verbatim() {
        assert_eq!(tokenize(r#""a\xb""#), [r"a\xb"]);
        assert_eq!(tokenize(r"a\xb"), [r"a\xb"]);
        assert_eq!(tokenize(r"a\ b"), [r"a\", "b"]);
    }

    #[test]
    fn tokenize_resolves_escapes_outside_quotes() {
        assert_eq!(tokenize(r"a\\b"), [r"a\b"]);
        assert_eq!(tokenize(r#"a\"b"#), [r#"a"b"#]);
        assert_eq!(tokenize(r"a\`b"), ["a`b"]);
        assert_eq!(tokenize(r"a\$b"), ["a$b"]);
    }

    #[test]
    fn tokenize_trailing_backslash_is_literal() {
        assert_eq!(tokenize(r"a\"), [r"a\"]);
        assert_eq!(tokenize(r"a\\"), [r"a\"]);
    }

    #[test]
    fn tokenize_escaped_quote_before_end_leaves_the_section_open() {
        // `"a\"` ends with an escaped quote, so the section is never closed.
        assert_eq!(
            tokenize_error(r#""a\""#),
            ExecError::UnterminatedQuote { offset: 0 }
        );
    }

    #[test]
    fn tokenize_escaped_quote_does_not_close_a_quoted_section() {
        assert_eq!(tokenize(r#""a\" b" c"#), [r#"a" b"#, "c"]);
    }

    // --- tokenize: errors ---------------------------------------------------

    #[test]
    fn tokenize_reports_unterminated_double_quote_offset() {
        assert_eq!(
            tokenize_error(r#"cmd "abc"#),
            ExecError::UnterminatedQuote { offset: 4 }
        );
    }

    #[test]
    fn tokenize_reports_offset_of_the_last_opening_quote() {
        assert_eq!(
            tokenize_error(r#""a" "b"#),
            ExecError::UnterminatedQuote { offset: 4 }
        );
        assert_eq!(
            tokenize_error(r#""abc"#),
            ExecError::UnterminatedQuote { offset: 0 }
        );
    }

    #[test]
    fn tokenize_reports_offset_after_multibyte_characters() {
        // `é` occupies two bytes, so the opening quote sits at byte 4.
        assert_eq!(
            tokenize_error("aé \"unterminated"),
            ExecError::UnterminatedQuote { offset: 4 }
        );
    }

    #[test]
    fn tokenize_accepts_balanced_quotes() {
        assert_eq!(tokenize(r#""a" "b""#), ["a", "b"]);
    }

    // --- expand_tokens: single-file codes -----------------------------------

    #[test]
    fn expand_tokens_single_file_codes_take_the_first_file() {
        let paths = files(&["/tmp/a", "/tmp/b"]);
        let context = ExecContext {
            files: &paths,
            ..ExecContext::default()
        };
        assert_eq!(expand(&["app", "%f"], &context), ["app", "/tmp/a"]);
        assert_eq!(expand(&["app", "%u"], &context), ["app", "/tmp/a"]);
    }

    #[test]
    fn expand_tokens_single_file_codes_vanish_without_files() {
        let context = ExecContext::default();
        assert_eq!(expand(&["app", "%f", "--flag"], &context), ["app", "--flag"]);
        assert_eq!(expand(&["%u"], &context), Vec::<String>::new());
    }

    #[test]
    fn expand_tokens_multi_file_codes_take_every_file() {
        let paths = files(&["/tmp/a", "/tmp/b", "/tmp/c"]);
        let context = ExecContext {
            files: &paths,
            ..ExecContext::default()
        };
        assert_eq!(
            expand(&["app", "%F"], &context),
            ["app", "/tmp/a", "/tmp/b", "/tmp/c"]
        );
        assert_eq!(expand(&["%U"], &context), ["/tmp/a", "/tmp/b", "/tmp/c"]);
    }

    #[test]
    fn expand_tokens_multi_file_codes_vanish_without_files() {
        let context = ExecContext::default();
        assert_eq!(
            expand(&["app", "%F", "%U", "--flag"], &context),
            ["app", "--flag"]
        );
    }

    // --- expand_tokens: icon ------------------------------------------------

    #[test]
    fn expand_tokens_standalone_icon_expands_to_two_arguments() {
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
    fn expand_tokens_standalone_icon_vanishes_without_icon() {
        let context = ExecContext::default();
        assert_eq!(expand(&["app", "%i"], &context), ["app"]);
    }

    #[test]
    fn expand_tokens_embedded_icon_is_verbatim() {
        let context = ExecContext {
            icon: Some("my-icon"),
            ..ExecContext::default()
        };
        assert_eq!(
            expand(&["--icon=%i", "%i%i"], &context),
            ["--icon=%i", "%i%i"]
        );
        assert_eq!(expand(&["--icon=%i"], &ExecContext::default()), ["--icon=%i"]);
    }

    // --- expand_tokens: in-token substitutions ------------------------------

    #[test]
    fn expand_tokens_name_substitutes_in_token() {
        let context = ExecContext {
            name: "My App",
            ..ExecContext::default()
        };
        assert_eq!(
            expand(&["--name=%c", "%c", "pre%cpost"], &context),
            ["--name=My App", "My App", "preMy Apppost"]
        );
    }

    #[test]
    fn expand_tokens_desktop_file_substitutes_in_token() {
        let path = Path::new("/usr/share/applications/org.example.App.desktop");
        let context = ExecContext {
            desktop_file: Some(path),
            ..ExecContext::default()
        };
        assert_eq!(
            expand(&["%k", "--desktop=%k"], &context),
            [
                "/usr/share/applications/org.example.App.desktop",
                "--desktop=/usr/share/applications/org.example.App.desktop"
            ]
        );
    }

    #[test]
    fn expand_tokens_missing_desktop_file_removes_only_the_code() {
        let context = ExecContext::default();
        assert_eq!(expand(&["--desktop=%k"], &context), ["--desktop="]);
        assert_eq!(expand(&["%k"], &context), Vec::<String>::new());
    }

    #[test]
    fn expand_tokens_double_percent_is_a_literal_percent() {
        let context = ExecContext::default();
        assert_eq!(
            expand(&["100%%", "%%", "a%%b%%c"], &context),
            ["100%", "%", "a%b%c"]
        );
    }

    // --- expand_tokens: deprecated and unknown codes ------------------------

    #[test]
    fn expand_tokens_removes_deprecated_codes() {
        let context = ExecContext::default();
        assert_eq!(
            expand(&["app", "%d", "%D", "%n", "%N", "%v", "%m", "--flag"], &context),
            ["app", "--flag"]
        );
        assert_eq!(expand(&["a%Db", "%n%m", "%v%N"], &context), ["ab"]);
    }

    #[test]
    fn expand_tokens_keeps_unknown_codes_verbatim() {
        let context = ExecContext::default();
        assert_eq!(
            expand(&["%x", "%z%y", "%1", "%", "pre%post", "%%%d"], &context),
            ["%x", "%z%y", "%1", "%", "pre%post", "%"]
        );
    }

    #[test]
    fn expand_tokens_keeps_embedded_file_codes_verbatim() {
        let paths = files(&["/tmp/a"]);
        let context = ExecContext {
            files: &paths,
            ..ExecContext::default()
        };
        assert_eq!(
            expand(&["--file=%f", "--file=%u", "--file=%F", "--file=%U"], &context),
            ["--file=%f", "--file=%u", "--file=%F", "--file=%U"]
        );
    }

    // --- expand_tokens: empty-token semantics -------------------------------

    #[test]
    fn expand_tokens_preserves_quoted_empty_arguments() {
        let context = ExecContext::default();
        assert_eq!(
            expand(&["", "app", "", "--flag"], &context),
            ["", "app", "", "--flag"]
        );
    }

    #[test]
    fn expand_tokens_drops_arguments_emptied_by_removal() {
        let context = ExecContext::default();
        assert_eq!(expand(&["app", "%d", "%k", "%%%d"], &context), ["app", "%"]);
    }

    #[test]
    fn expand_tokens_drops_name_code_with_empty_name() {
        let context = ExecContext::default();
        assert_eq!(expand(&["%c", "--name=%c"], &context), ["--name="]);
    }

    #[test]
    fn expand_tokens_default_context_only_drops_macros() {
        assert_eq!(
            expand(
                &["app", "--flag", "%f", "%F", "%u", "%U", "%i", "%c", "%k"],
                &ExecContext::default()
            ),
            ["app", "--flag"]
        );
    }

    // --- expand: end to end -------------------------------------------------

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
    }

    #[test]
    fn expand_app_with_no_codes_is_unchanged() {
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
    }
}
