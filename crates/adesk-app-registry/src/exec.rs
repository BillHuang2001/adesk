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
