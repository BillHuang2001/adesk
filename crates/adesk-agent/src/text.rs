//! Character-counted text truncation, shared by the crate's eliding call sites.
//!
//! Three places elide text — [`crate::agent_loop`]'s one-line decision summary,
//! [`crate::context`]'s event/commit details and the OpenAI provider's error bodies —
//! and they differ only in the marker they append. The rule lives here once.
//!
//! Semantics, pinned by the unit tests below:
//!
//! - counting is by `char`, never by byte, so no multi-byte sequence is ever split;
//! - text that fits in `max_chars` is returned **unchanged**: the marker marks a cut
//!   and is therefore never appended when nothing was cut;
//! - `max_chars == 0` drops every character, so the result is exactly the marker
//!   (empty input stays empty, because nothing was cut);
//! - the marker is a parameter, so a caller can elide silently (`""`) or visibly
//!   (`"..."`, `"…"`).

/// Truncate `text` to at most `max_chars` characters, appending `marker` when
/// characters were dropped.
///
/// The result is the first `max_chars` characters of `text` followed by `marker`
/// whenever `text` is longer than `max_chars`; otherwise it is `text` itself. The
/// cut happens on a `char` boundary, so multi-byte text is never mangled.
#[must_use]
pub(crate) fn truncate(text: &str, max_chars: usize, marker: &str) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str(marker);
    truncated
}

#[cfg(test)]
mod tests {
    use super::truncate;

    /// Text that fits is returned as-is: the marker only ever marks a cut.
    #[test]
    fn exact_fit_is_returned_unchanged() {
        assert_eq!(truncate("abc", 3, "..."), "abc");
        assert_eq!(truncate("abc", 5, "…"), "abc");
        assert_eq!(truncate("", 0, "…"), "");
        // The character count is what matters, not the byte length.
        assert_eq!(truncate("日本語", 3, "..."), "日本語");
    }

    /// Long multi-byte text is cut on a character boundary, never inside one.
    #[test]
    fn multibyte_text_is_cut_on_char_boundaries() {
        assert_eq!(truncate("日本語です", 2, "…"), "日本…");
        assert_eq!(truncate("日本語です", 2, "..."), "日本...");
        assert_eq!(truncate("é😀", 1, ""), "é");
        assert_eq!(truncate("abc", 0, "..."), "...");
    }

    /// `max_chars == 0` keeps nothing, so the result is exactly the marker.
    #[test]
    fn max_zero_yields_exactly_the_marker() {
        assert_eq!(truncate("abc", 0, "…"), "…");
        assert_eq!(truncate("日本語", 0, ""), "");
        assert_eq!(truncate("", 0, "..."), "", "nothing was cut");
    }

    /// The marker is free-form; the kept prefix never depends on it.
    #[test]
    fn marker_is_a_parameter() {
        for marker in ["", "...", "…", " [cut]"] {
            assert_eq!(truncate("abcdef", 3, marker), format!("abc{marker}"));
        }
    }
}
