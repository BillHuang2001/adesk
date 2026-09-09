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

#[allow(unused_variables)] // stub: parameters are consumed by the implementation.
impl ExecExpander {
    /// Creates an expander.
    pub fn new() -> ExecExpander {
        Self
    }

    /// Splits an `Exec` value into arguments (quotes removed, escapes resolved).
    pub fn tokenize(&self, exec: &str) -> std::result::Result<Vec<String>, ExecError> {
        todo!("stub: implementation phase")
    }

    /// Applies field-code expansion to already tokenized arguments.
    pub fn expand_tokens(&self, tokens: &[String], context: &ExecContext<'_>) -> Vec<String> {
        todo!("stub: implementation phase")
    }

    /// Tokenizes and expands in one step.
    pub fn expand(
        &self,
        exec: &str,
        context: &ExecContext<'_>,
    ) -> std::result::Result<Vec<String>, ExecError> {
        todo!("stub: implementation phase")
    }
}
