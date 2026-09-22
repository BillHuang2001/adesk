//! Toolkit role-name normalization — the wire vocabulary of `AccessibleNode::role`.
//!
//! `AccessibleNode::role` is the toolkit's own role name kept as a free string,
//! never a lossy enum: the accessibility vocabulary is large, sparsely
//! implemented and grows with every toolkit release, and an unclassifiable role
//! is exactly the case where the element is still useful (`docs/accessibility.md`).
//! Normalizing the spelling on the runtime side is what keeps a filter such as
//! `find_accessible role` stable across toolkits that spell the same role
//! differently (`"push button"`, `"Push Button"`, `"push_button"`).

/// Normalize a toolkit role name to the AGP `AccessibleNode::role` form:
/// lowercase, with every run of characters outside `[a-z0-9]` collapsed to a
/// single `_`, and no leading/trailing `_`.
///
/// # Examples
///
/// ```
/// use adesk_a11y::normalize_role;
///
/// assert_eq!(normalize_role("push button"), "push_button");
/// assert_eq!(normalize_role("  Read Only "), "read_only");
/// assert_eq!(normalize_role("menu_item"), "menu_item");
/// assert_eq!(normalize_role(""), "");
/// ```
///
/// Digits are kept (`"table cell 2"` becomes `"table_cell_2"`), repeated
/// separators collapse (`"a--b"` becomes `"a_b"`) and a character outside ASCII
/// is a separator like any other (`"naïve"` becomes `"na_ve"`).
pub fn normalize_role(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_separator = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            // A separator is only emitted once the name has actually started, so
            // a leading run of punctuation never leaves a leading `_`.
            if pending_separator && !out.is_empty() {
                out.push('_');
            }
            pending_separator = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_separator = true;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_examples() {
        assert_eq!(normalize_role("push button"), "push_button");
        assert_eq!(normalize_role("  Read Only "), "read_only");
        assert_eq!(normalize_role("menu_item"), "menu_item");
        assert_eq!(normalize_role(""), "");
    }

    #[test]
    fn case_is_folded() {
        assert_eq!(normalize_role("PUSH_BUTTON"), "push_button");
        assert_eq!(normalize_role("PageTabList"), "pagetablist");
        assert_eq!(normalize_role("Push Button"), "push_button");
    }

    #[test]
    fn digits_are_kept() {
        assert_eq!(normalize_role("table cell 2"), "table_cell_2");
        assert_eq!(normalize_role("h1"), "h1");
        assert_eq!(normalize_role("1st item"), "1st_item");
    }

    #[test]
    fn separator_runs_collapse_and_never_dangle() {
        assert_eq!(normalize_role("a---b"), "a_b");
        assert_eq!(normalize_role("__x__"), "x");
        assert_eq!(normalize_role("---a---b---"), "a_b");
        assert_eq!(normalize_role("a.b/c"), "a_b_c");
        assert_eq!(normalize_role("   "), "");
        assert_eq!(normalize_role("!!!"), "");
    }

    #[test]
    fn non_ascii_characters_are_separators() {
        assert_eq!(normalize_role("café"), "caf");
        assert_eq!(normalize_role("naïve"), "na_ve");
        assert_eq!(normalize_role("日本語"), "");
    }

    #[test]
    fn already_normalized_names_are_stable() {
        for role in ["push_button", "menu_item", "table_cell", "label"] {
            assert_eq!(normalize_role(role), role);
        }
    }
}
