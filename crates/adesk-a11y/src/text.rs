//! The `docs/protocol.md` §5.11 outline renderer.
//!
//! `accessibility_tree` answers with both the structured `AccessibleTree` and a
//! rendered outline, because they answer different questions: the tree is what an
//! agent consumes when it must *address* an element, and the outline is what it
//! reads — or feeds to a model verbatim — when it must *understand* a window
//! (`docs/accessibility.md`).
//!
//! The grammar is fixed so a line is always exactly one node and a quoted field is
//! always unambiguous; that is what lets the outline be grepped, diffed between
//! two observations, or read line by line by a parser that knows nothing about
//! accessibility.

use adesk_core::{AccessibleNode, AccessibleState, AccessibleTree};

/// Options for rendering an `AccessibleTree` as text.
///
/// The projection flags mirror the `accessibility_tree` request (§5.11): they are
/// a deliberate detail-versus-tokens control, so a cheap overview of a large
/// dialog costs one request with the optional sections off and drilling into one
/// subtree costs another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextOptions {
    /// Spaces of indentation added per tree level; the root is at depth `0`.
    pub indent: usize,
    /// Emit ` id={id}` for every node.
    pub include_ids: bool,
    /// Emit ` bounds={x},{y},{w},{h}` for nodes whose geometry is known.
    pub include_bounds: bool,
    /// Emit ` states=[...]` for nodes with set state flags.
    pub include_states: bool,
    /// Emit ` actions=[...]` for nodes that expose actions.
    pub include_actions: bool,
}

impl Default for TextOptions {
    fn default() -> TextOptions {
        TextOptions {
            indent: 2,
            include_ids: true,
            include_bounds: true,
            include_states: true,
            include_actions: true,
        }
    }
}

/// Render `tree` as the one-line-per-node outline of `docs/protocol.md` §5.11.
///
/// Each node renders as `{role} "{name}"`, then — in this fixed order, each part
/// only when it is known — ` value="{value}"`, ` states=[{s1},{s2}]`,
/// ` actions=[{a1},{a2}]`, ` bounds={x},{y},{w},{h}` and ` id={id}`. Children
/// follow their parent, one per line, indented by `opts.indent` spaces per level
/// with the root at depth `0`. Lines are joined with `\n`; there is no trailing
/// newline.
///
/// Inside names and values, `\`, `"`, newline and tab are escaped as `\\`, `\"`,
/// `\n` and `\t`.
pub fn render_text(tree: &AccessibleTree, opts: TextOptions) -> String {
    let mut lines = Vec::new();
    push_lines(&tree.root, 0, opts, &mut lines);
    lines.join("\n")
}

/// Append the line for `node` and then the lines of its subtree, in order.
fn push_lines(node: &AccessibleNode, depth: usize, opts: TextOptions, lines: &mut Vec<String>) {
    lines.push(render_line(node, depth, opts));
    for child in &node.children {
        push_lines(child, depth + 1, opts, lines);
    }
}

/// Render one node as a single outline line (never containing a newline).
fn render_line(node: &AccessibleNode, depth: usize, opts: TextOptions) -> String {
    let mut line = String::new();

    for _ in 0..depth.saturating_mul(opts.indent) {
        line.push(' ');
    }
    line.push_str(&node.role);
    line.push_str(" \"");
    line.push_str(&escape(&node.name));
    line.push('"');

    if let Some(value) = &node.value {
        line.push_str(" value=\"");
        line.push_str(&escape(value));
        line.push('"');
    }

    if opts.include_states && !node.states.is_empty() {
        line.push_str(" states=[");
        for (index, state) in node.states.iter().enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push_str(state_name(*state));
        }
        line.push(']');
    }

    if opts.include_actions && !node.actions.is_empty() {
        line.push_str(" actions=[");
        line.push_str(&node.actions.join(","));
        line.push(']');
    }

    if opts.include_bounds {
        if let Some(bounds) = node.bounds {
            line.push_str(&format!(
                " bounds={},{},{},{}",
                bounds.x, bounds.y, bounds.w, bounds.h
            ));
        }
    }

    if opts.include_ids {
        line.push_str(&format!(" id={}", node.id));
    }

    line
}

/// Escape the four characters the outline grammar reserves (`docs/protocol.md` §5.11).
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// The AGP wire name of a state flag (`docs/protocol.md` §4).
///
/// Mirrors the `snake_case` spelling `AccessibleState` serializes as; the
/// catch-all arm keeps the function total for a variant a later `adesk-core` adds
/// (the enum is `#[non_exhaustive]`), and the unit tests assert the mapping
/// against `serde_json` for every variant known today.
///
/// The `atspi` mapping module sorts its output by this name rather than by the
/// enum's declaration order, so the wire vocabulary stays the single source of the
/// normal form.
pub(crate) fn state_name(state: AccessibleState) -> &'static str {
    match state {
        AccessibleState::Enabled => "enabled",
        AccessibleState::Sensitive => "sensitive",
        AccessibleState::Showing => "showing",
        AccessibleState::Visible => "visible",
        AccessibleState::Focusable => "focusable",
        AccessibleState::Focused => "focused",
        AccessibleState::Checkable => "checkable",
        AccessibleState::Checked => "checked",
        AccessibleState::Selected => "selected",
        AccessibleState::Selectable => "selectable",
        AccessibleState::Expandable => "expandable",
        AccessibleState::Expanded => "expanded",
        AccessibleState::Collapsed => "collapsed",
        AccessibleState::Editable => "editable",
        AccessibleState::Multiline => "multiline",
        AccessibleState::ReadOnly => "read_only",
        AccessibleState::Pressed => "pressed",
        AccessibleState::Active => "active",
        AccessibleState::Busy => "busy",
        AccessibleState::Modal => "modal",
        AccessibleState::Defunct => "defunct",
        AccessibleState::Invalid => "invalid",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adesk_core::{AccessibleId, AppId, Rect, WindowId};

    fn node(id: u64, role: &str, name: &str) -> AccessibleNode {
        AccessibleNode {
            id: AccessibleId(id),
            role: role.into(),
            name: name.into(),
            description: None,
            value: None,
            states: Vec::new(),
            bounds: None,
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Option<Rect> {
        Some(Rect { x, y, w, h })
    }

    /// The shape `docs/accessibility.md` illustrates, as a real tree:
    ///
    /// ```text
    /// frame "Open File"
    ///   dialog "Open File"
    ///     text "File name:"
    ///     entry "" value="report.txt"
    ///     push_button "Open"
    ///     push_button "Cancel"
    /// ```
    fn sample_tree() -> AccessibleTree {
        let mut text = node(3, "text", "File name:");
        text.bounds = rect(0, 0, 80, 24);

        let mut entry = node(4, "entry", "");
        entry.value = Some("report.txt".into());
        entry.states = vec![AccessibleState::Focused];
        entry.actions = vec!["set_value".into()];
        entry.bounds = rect(88, 0, 400, 24);

        let mut open = node(5, "push_button", "Open");
        open.actions = vec!["click".into()];

        let mut cancel = node(6, "push_button", "Cancel");
        cancel.states = vec![AccessibleState::Enabled, AccessibleState::Sensitive];

        let mut dialog = node(2, "dialog", "Open File");
        dialog.bounds = rect(10, 10, 620, 460);
        dialog.children = vec![text, entry, open, cancel];

        let mut root = node(1, "frame", "Open File");
        root.bounds = rect(0, 0, 640, 480);
        root.children = vec![dialog];

        AccessibleTree {
            window_id: WindowId(17),
            app_id: Some(AppId::from("org.gnome.TextEditor")),
            app_name: Some("Text Editor".into()),
            root,
            node_count: 6,
            truncated: false,
        }
    }

    #[test]
    fn golden_outline() {
        assert_eq!(
            render_text(&sample_tree(), TextOptions::default()),
            concat!(
                r#"frame "Open File" bounds=0,0,640,480 id=1"#,
                "\n",
                r#"  dialog "Open File" bounds=10,10,620,460 id=2"#,
                "\n",
                r#"    text "File name:" bounds=0,0,80,24 id=3"#,
                "\n",
                r#"    entry "" value="report.txt" states=[focused] actions=[set_value] bounds=88,0,400,24 id=4"#,
                "\n",
                r#"    push_button "Open" actions=[click] id=5"#,
                "\n",
                r#"    push_button "Cancel" states=[enabled,sensitive] id=6"#,
            )
        );
    }

    #[test]
    fn the_root_is_at_depth_zero_and_lines_have_no_trailing_newline() {
        let text = render_text(&sample_tree(), TextOptions::default());
        assert!(text.starts_with("frame "));
        assert!(!text.ends_with('\n'));
        assert_eq!(text.lines().count(), 6);
    }

    #[test]
    fn a_bare_node_renders_role_name_and_id_only() {
        let mut tree = sample_tree();
        tree.root = node(9, "label", "");
        assert_eq!(
            render_text(&tree, TextOptions::default()),
            r#"label "" id=9"#
        );
    }

    #[test]
    fn an_empty_string_value_is_rendered_while_an_absent_one_is_not() {
        let mut tree = sample_tree();

        let mut empty = node(9, "label", "Ready");
        empty.value = Some(String::new());
        tree.root = empty;
        assert_eq!(
            render_text(&tree, TextOptions::default()),
            r#"label "Ready" value="" id=9"#
        );

        let absent = node(9, "label", "Ready");
        assert_eq!(absent.value, None);
        tree.root = absent;
        assert_eq!(
            render_text(&tree, TextOptions::default()),
            r#"label "Ready" id=9"#
        );
    }

    #[test]
    fn indent_is_configurable_and_zero_removes_it() {
        let tree = sample_tree();

        let wide = TextOptions {
            indent: 4,
            ..TextOptions::default()
        };
        let text = render_text(&tree, wide);
        assert!(text.lines().nth(1).unwrap().starts_with("    dialog "));
        assert!(text.lines().nth(2).unwrap().starts_with("        text "));

        let flat = TextOptions {
            indent: 0,
            ..TextOptions::default()
        };
        let text = render_text(&tree, flat);
        assert!(text
            .lines()
            .nth(2)
            .unwrap()
            .starts_with(r#"text "File name:""#));
    }

    #[test]
    fn include_ids_projects_the_id_out() {
        let opts = TextOptions {
            include_ids: false,
            ..TextOptions::default()
        };
        let text = render_text(&sample_tree(), opts);
        assert!(!text.contains(" id="));
        assert!(text.starts_with(r#"frame "Open File" bounds=0,0,640,480"#));
    }

    #[test]
    fn include_bounds_projects_the_bounds_out() {
        let text = render_text(
            &sample_tree(),
            TextOptions {
                include_bounds: false,
                ..TextOptions::default()
            },
        );
        assert!(!text.contains(" bounds="));
        assert!(text.lines().next().unwrap().ends_with(" id=1"));
    }

    #[test]
    fn include_states_projects_the_states_out() {
        let text = render_text(
            &sample_tree(),
            TextOptions {
                include_states: false,
                ..TextOptions::default()
            },
        );
        assert!(!text.contains(" states="));
        assert!(text.contains(r#"actions=[set_value]"#));
    }

    #[test]
    fn include_actions_projects_the_actions_out() {
        let text = render_text(
            &sample_tree(),
            TextOptions {
                include_actions: false,
                ..TextOptions::default()
            },
        );
        assert!(!text.contains(" actions="));
        assert!(text.contains("states=[focused]"));
    }

    #[test]
    fn every_projection_off_leaves_role_name_value_and_children() {
        let text = render_text(
            &sample_tree(),
            TextOptions {
                indent: 1,
                include_ids: false,
                include_bounds: false,
                include_states: false,
                include_actions: false,
            },
        );
        assert_eq!(
            text,
            concat!(
                r#"frame "Open File""#,
                "\n",
                r#" dialog "Open File""#,
                "\n",
                r#"  text "File name:""#,
                "\n",
                r#"  entry "" value="report.txt""#,
                "\n",
                r#"  push_button "Open""#,
                "\n",
                r#"  push_button "Cancel""#,
            )
        );
    }

    #[test]
    fn names_and_values_are_escaped() {
        let mut tree = sample_tree();
        let mut root = node(1, "label", "a\"b\\c\nd\te");
        root.value = Some("v\"w\\x\ny\tz".into());
        tree.root = root;

        assert_eq!(
            render_text(&tree, TextOptions::default()),
            r#"label "a\"b\\c\nd\te" value="v\"w\\x\ny\tz" id=1"#
        );
    }

    #[test]
    fn escaping_keeps_one_line_per_node() {
        let mut tree = sample_tree();
        tree.root = node(1, "label", "two\nlines");
        let text = render_text(&tree, TextOptions::default());
        assert_eq!(text.lines().count(), 1);
        assert_eq!(text, r#"label "two\nlines" id=1"#);
    }

    #[test]
    fn options_default_to_two_spaces_and_every_section_on() {
        let opts = TextOptions::default();
        assert_eq!(
            opts,
            TextOptions {
                indent: 2,
                include_ids: true,
                include_bounds: true,
                include_states: true,
                include_actions: true,
            }
        );
    }

    #[test]
    fn state_names_match_the_agp_wire_names() {
        let states = [
            AccessibleState::Enabled,
            AccessibleState::Sensitive,
            AccessibleState::Showing,
            AccessibleState::Visible,
            AccessibleState::Focusable,
            AccessibleState::Focused,
            AccessibleState::Checkable,
            AccessibleState::Checked,
            AccessibleState::Selected,
            AccessibleState::Selectable,
            AccessibleState::Expandable,
            AccessibleState::Expanded,
            AccessibleState::Collapsed,
            AccessibleState::Editable,
            AccessibleState::Multiline,
            AccessibleState::ReadOnly,
            AccessibleState::Pressed,
            AccessibleState::Active,
            AccessibleState::Busy,
            AccessibleState::Modal,
            AccessibleState::Defunct,
            AccessibleState::Invalid,
        ];
        assert_eq!(states.len(), 22);
        for state in states {
            let wire = serde_json::to_value(state).unwrap();
            assert_eq!(
                Some(state_name(state)),
                wire.as_str(),
                "state_name must mirror the serde wire name"
            );
        }
    }

    #[test]
    fn states_render_in_their_reported_order() {
        let mut tree = sample_tree();
        tree.root = node(1, "check_box", "Wrap");
        tree.root.states = vec![
            AccessibleState::Enabled,
            AccessibleState::ReadOnly,
            AccessibleState::Checked,
        ];
        assert_eq!(
            render_text(&tree, TextOptions::default()),
            r#"check_box "Wrap" states=[enabled,read_only,checked] id=1"#
        );
    }
}
