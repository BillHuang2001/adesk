//! The `find_accessible` matcher (`docs/protocol.md` §5.11).
//!
//! A pure, synchronous pre-order walk over a [`SourceNode`] tree: no D-Bus, no
//! runtime ids, no clock. The service that answers `find_accessible` calls this
//! and then maps each matched node's handle to the `AccessibleId` the agent sees.
//!
//! Filters are AND-ed and an omitted filter matches everything; `role` is an
//! exact (already normalized, lowercase) match, `name` is exact, and
//! `name_contains`/`value_contains` are case-insensitive substrings — a node
//! without a value never matches `value_contains`.

use crate::source::SourceNode;

/// The filters of an AGP `find_accessible` request (§5.11).
///
/// `max_results` is the raw request value; the server rejects a `max_results` of
/// zero with `invalid_request` and passes the validated bound to
/// [`collect_matches`], so this copy is the request's own record of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NodeQuery {
    /// Exact lowercase role name to match, when given.
    pub role: Option<String>,
    /// Exact accessible name to match, when given.
    pub name: Option<String>,
    /// Case-insensitive substring of the accessible name, when given.
    pub name_contains: Option<String>,
    /// Case-insensitive substring of the node's value, when given.
    pub value_contains: Option<String>,
    /// Maximum number of matches the request asked for.
    pub max_results: u32,
}

/// One pre-order match: the node plus the ancestor names from the root down to
/// (excluding) it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeMatch<'a> {
    /// The matched node.
    pub node: &'a SourceNode,
    /// Ancestor names from the window root down to, but excluding, the match.
    pub path: Vec<String>,
}

/// Collect matches in pre-order, at most `max_results` (the effective bound —
/// callers pass the request's validated [`NodeQuery::max_results`]); the returned
/// bool reports that more nodes would have matched (i.e. the result was
/// truncated).
///
/// `max_results == 0` therefore yields no matches at all: the walk stops before
/// collecting anything and reports truncation as soon as it meets a node that
/// would have matched.
pub(crate) fn collect_matches<'a>(
    root: &'a SourceNode,
    query: &NodeQuery,
    max_results: u32,
) -> (Vec<NodeMatch<'a>>, bool) {
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut path = Vec::new();

    walk(
        root,
        query,
        max_results,
        &mut path,
        &mut matches,
        &mut truncated,
    );

    (matches, truncated)
}

/// Pre-order visit: the node itself, then its children in order.
///
/// Sets `truncated` and stops as soon as a node that would have matched cannot be
/// collected, so the rest of the tree is neither visited nor allocated for. The
/// `path` push/pop is balanced on every path through the function, including
/// those stops.
fn walk<'a>(
    node: &'a SourceNode,
    query: &NodeQuery,
    max_results: u32,
    path: &mut Vec<String>,
    matches: &mut Vec<NodeMatch<'a>>,
    truncated: &mut bool,
) {
    if *truncated {
        return;
    }

    if matches_node(node, query) {
        if (matches.len() as u32) < max_results {
            matches.push(NodeMatch {
                node,
                path: path.clone(),
            });
        } else {
            *truncated = true;
            return;
        }
    }

    // `path` holds the ancestors of whatever is being visited, so the node's own
    // name is pushed only for the subtree below it.
    path.push(node.name.clone());
    for child in &node.children {
        walk(child, query, max_results, path, matches, truncated);
        if *truncated {
            break;
        }
    }
    path.pop();
}

/// Whether `node` satisfies every filter of `query` (an omitted filter matches).
fn matches_node(node: &SourceNode, query: &NodeQuery) -> bool {
    if let Some(role) = &query.role {
        if &node.role != role {
            return false;
        }
    }
    if let Some(name) = &query.name {
        if &node.name != name {
            return false;
        }
    }
    if let Some(needle) = &query.name_contains {
        if !contains_ignore_case(&node.name, needle) {
            return false;
        }
    }
    if let Some(needle) = &query.value_contains {
        match &node.value {
            Some(value) if contains_ignore_case(value, needle) => {}
            _ => return false,
        }
    }

    true
}

/// Case-insensitive substring test; an empty needle matches everything.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ElementHandle;

    fn node(role: &str, name: &str, children: Vec<SourceNode>) -> SourceNode {
        SourceNode {
            role: role.into(),
            name: name.into(),
            description: None,
            value: None,
            states: Vec::new(),
            bounds: None,
            actions: Vec::new(),
            handle: ElementHandle(format!("{role}/{name}")),
            children,
        }
    }

    fn valued(mut node: SourceNode, value: &str) -> SourceNode {
        node.value = Some(value.into());
        node
    }

    /// ```text
    /// frame "Main"
    ///   panel "Toolbar"
    ///     push_button "Open"
    ///     push_button "Save As"
    ///   entry "" value="hello world"
    ///   label "Ready"
    /// ```
    fn sample_tree() -> SourceNode {
        node(
            "frame",
            "Main",
            vec![
                node(
                    "panel",
                    "Toolbar",
                    vec![
                        node("push_button", "Open", Vec::new()),
                        node("push_button", "Save As", Vec::new()),
                    ],
                ),
                valued(node("entry", "", Vec::new()), "hello world"),
                node("label", "Ready", Vec::new()),
            ],
        )
    }

    fn query(max_results: u32) -> NodeQuery {
        NodeQuery {
            max_results,
            ..NodeQuery::default()
        }
    }

    /// Collect with the query's own `max_results` as the effective bound, which
    /// is how the §5.11 handler drives the matcher.
    fn collect<'a>(root: &'a SourceNode, query: &NodeQuery) -> (Vec<NodeMatch<'a>>, bool) {
        collect_matches(root, query, query.max_results)
    }

    fn names(matches: &[NodeMatch<'_>]) -> Vec<String> {
        matches.iter().map(|m| m.node.name.clone()).collect()
    }

    #[test]
    fn an_omitted_filter_matches_everything_in_pre_order() {
        let tree = sample_tree();
        let (matches, truncated) = collect(&tree, &query(50));

        assert!(!truncated);
        assert_eq!(
            names(&matches),
            ["Main", "Toolbar", "Open", "Save As", "", "Ready"]
        );
        assert_eq!(matches[0].node.role, "frame");
        assert_eq!(matches[4].node.role, "entry");
    }

    #[test]
    fn role_is_an_exact_lowercase_match() {
        let tree = sample_tree();

        let (matches, truncated) = collect(
            &tree,
            &NodeQuery {
                role: Some("push_button".into()),
                ..query(50)
            },
        );
        assert!(!truncated);
        assert_eq!(names(&matches), ["Open", "Save As"]);

        // `role` is exact, never a substring or a case-insensitive match.
        for role in ["push", "PUSH_BUTTON", "push button", "button_"] {
            let (matches, truncated) = collect(
                &tree,
                &NodeQuery {
                    role: Some(role.into()),
                    ..query(50)
                },
            );
            assert!(matches.is_empty(), "{role} must not match");
            assert!(!truncated);
        }
    }

    #[test]
    fn name_is_an_exact_match() {
        let tree = sample_tree();

        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                name: Some("Open".into()),
                ..query(50)
            },
        );
        assert_eq!(names(&matches), ["Open"]);

        // Exact, not a prefix and not case-insensitive.
        for name in ["Ope", "open", "Open "] {
            let (matches, _) = collect(
                &tree,
                &NodeQuery {
                    name: Some(name.into()),
                    ..query(50)
                },
            );
            assert!(matches.is_empty(), "{name} must not match");
        }
    }

    #[test]
    fn name_contains_is_a_case_insensitive_substring() {
        let tree = sample_tree();

        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                name_contains: Some("open".into()),
                ..query(50)
            },
        );
        assert_eq!(names(&matches), ["Open"]);

        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                name_contains: Some("sAVe".into()),
                ..query(50)
            },
        );
        assert_eq!(names(&matches), ["Save As"]);

        // The empty needle matches every node, like an omitted filter.
        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                name_contains: Some(String::new()),
                ..query(50)
            },
        );
        assert_eq!(matches.len(), 6);
    }

    #[test]
    fn value_contains_matches_only_nodes_that_have_a_value() {
        let tree = sample_tree();

        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                value_contains: Some("WORLD".into()),
                ..query(50)
            },
        );
        assert_eq!(names(&matches), [""]);
        assert_eq!(matches[0].node.value.as_deref(), Some("hello world"));

        // "Open" has no value, so a needle matching its *name* must not match it.
        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                value_contains: Some("open".into()),
                ..query(50)
            },
        );
        assert!(matches.is_empty());
    }

    #[test]
    fn filters_are_anded() {
        let tree = sample_tree();

        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                role: Some("push_button".into()),
                name_contains: Some("save".into()),
                ..query(50)
            },
        );
        assert_eq!(names(&matches), ["Save As"]);

        // Each filter alone matches, but not all together.
        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                role: Some("push_button".into()),
                name: Some("Ready".into()),
                ..query(50)
            },
        );
        assert!(matches.is_empty());
    }

    #[test]
    fn path_is_the_ancestor_names_excluding_the_match() {
        let tree = sample_tree();

        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                name_contains: Some("As".into()),
                ..query(50)
            },
        );
        assert_eq!(names(&matches), ["Save As"]);
        assert_eq!(matches[0].path, ["Main", "Toolbar"]);

        // The root has no ancestors.
        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                role: Some("frame".into()),
                ..query(50)
            },
        );
        assert!(matches[0].path.is_empty());

        // A child of the root has exactly one ancestor.
        let (matches, _) = collect(
            &tree,
            &NodeQuery {
                role: Some("label".into()),
                ..query(50)
            },
        );
        assert_eq!(matches[0].path, ["Main"]);
    }

    #[test]
    fn max_results_bounds_the_matches_and_reports_truncation() {
        let tree = sample_tree();

        let (matches, truncated) = collect(&tree, &query(3));
        assert_eq!(names(&matches), ["Main", "Toolbar", "Open"]);
        assert!(truncated, "three of six matches must report truncation");

        // Exactly as many as there are: not truncated.
        let (matches, truncated) = collect(&tree, &query(6));
        assert_eq!(matches.len(), 6);
        assert!(!truncated);

        // More than there are: not truncated.
        let (matches, truncated) = collect(&tree, &query(7));
        assert_eq!(matches.len(), 6);
        assert!(!truncated);

        // One: truncated.
        let (matches, truncated) = collect(&tree, &query(1));
        assert_eq!(names(&matches), ["Main"]);
        assert!(truncated);
    }

    #[test]
    fn max_results_of_zero_yields_no_matches_and_reports_truncation() {
        let tree = sample_tree();
        let (matches, truncated) = collect(&tree, &query(0));
        assert!(matches.is_empty());
        assert!(truncated);
    }

    #[test]
    fn no_match_reports_no_truncation() {
        let tree = sample_tree();
        let (matches, truncated) = collect(
            &tree,
            &NodeQuery {
                role: Some("menu".into()),
                ..query(2)
            },
        );
        assert!(matches.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn a_single_node_tree_is_matched_at_the_root() {
        let tree = node("frame", "Only", Vec::new());
        let (matches, truncated) = collect(&tree, &query(50));
        assert_eq!(names(&matches), ["Only"]);
        assert!(matches[0].path.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn the_default_query_has_no_filters() {
        let query = NodeQuery::default();
        assert_eq!(query.role, None);
        assert_eq!(query.name, None);
        assert_eq!(query.name_contains, None);
        assert_eq!(query.value_contains, None);
        assert_eq!(query.max_results, 0);
    }
}
