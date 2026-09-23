//! The viewer's help/status text, composed GTK-free.
//!
//! The GUI shows a small header affordance — a menu button whose popover lists
//! the dialled endpoint, the active window, the connection state and a short
//! cheat-sheet of what the human can do from here, plus a one-line hint shown
//! while the desktop does **not** have keyboard control. All of that text is
//! derived here, so it is unit-tested without a display; the GTK layer only
//! paints it.
//!
//! The texts are composed once per relevant change (a connection or window
//! change, a focus change) instead of per frame, and every non-constant value is
//! Pango-escaped ([`Help::markup`]) because window titles and paths come from the
//! runtime.

use adesk_viewer_proto::DesktopState;

use crate::keystroke::ESCAPE_LABEL;
use crate::taskbar;

/// The connection state the help panel reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionState {
    /// The viewer is still dialling the endpoint.
    Connecting,
    /// The handshake succeeded and frames are flowing.
    Connected,
    /// The connection failed or ended; the reason is already composed by
    /// [`crate::address`] and names the dialled endpoint.
    Failed(String),
}

impl ConnectionState {
    /// A one-line summary: `connecting…`, `connected` or `not connected — …`.
    pub(crate) fn summary(&self) -> String {
        match self {
            ConnectionState::Connecting => "connecting…".to_owned(),
            ConnectionState::Connected => "connected".to_owned(),
            ConnectionState::Failed(reason) => format!("not connected — {reason}"),
        }
    }
}

/// One labelled row of the help panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    /// The row's label (rendered bold by [`Help::markup`]).
    pub(crate) label: &'static str,
    /// The row's value.
    pub(crate) value: String,
}

/// The viewer's help/status model: what it dialled, what it shows, how it is
/// connected and whether the human currently drives the keyboard.
///
/// The setters report whether the value actually changed, so the GTK layer can
/// re-render only on a change — the model is fed once per connection, state and
/// focus event, never per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Help {
    /// The endpoint actually dialled (a Unix socket path or TCP address).
    target: String,
    /// The active window's label, once a state message named one.
    window: Option<String>,
    /// How the connection is doing.
    state: ConnectionState,
    /// Whether the frame view holds keyboard control.
    ///
    /// A fresh model reports control: the GUI hands the keyboard to the desktop
    /// as soon as it shows it, and corrects this from the widget's real focus.
    focused: bool,
}

impl Help {
    /// Creates a model for the endpoint `target`, still connecting, with the
    /// keyboard in the desktop's hands.
    pub(crate) fn new(target: impl Into<String>) -> Help {
        Help {
            target: target.into(),
            window: None,
            state: ConnectionState::Connecting,
            focused: true,
        }
    }

    /// Records the active window's label; reports whether it changed.
    pub(crate) fn set_window(&mut self, window: Option<String>) -> bool {
        if self.window == window {
            return false;
        }
        self.window = window;
        true
    }

    /// Records the connection state; reports whether it changed.
    pub(crate) fn set_state(&mut self, state: ConnectionState) -> bool {
        if self.state == state {
            return false;
        }
        self.state = state;
        true
    }

    /// Records whether the frame view holds keyboard control; reports whether it
    /// changed.
    pub(crate) fn set_focused(&mut self, focused: bool) -> bool {
        if self.focused == focused {
            return false;
        }
        self.focused = focused;
        true
    }

    /// Whether the frame view currently holds keyboard control.
    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    /// The labelled status rows: endpoint, active window, connection, control.
    pub(crate) fn lines(&self) -> Vec<Line> {
        vec![
            Line {
                label: "Endpoint",
                value: self.target.clone(),
            },
            Line {
                label: "Window",
                value: self
                    .window
                    .clone()
                    .unwrap_or_else(|| "no active window".to_owned()),
            },
            Line {
                label: "Connection",
                value: self.state.summary(),
            },
            Line {
                label: "Control",
                value: self.control_text(),
            },
        ]
    }

    /// The one-line status used as the affordance's tooltip: how the connection
    /// is doing plus who holds the keyboard.
    pub(crate) fn summary(&self) -> String {
        let control = if self.focused {
            "keyboard control"
        } else {
            "no keyboard control"
        };
        format!(
            "{} — {} — {control}",
            self.state.summary(),
            self.active_label()
        )
    }

    /// The popover body as Pango markup: the status rows (bold labels) followed
    /// by the capability list.
    pub(crate) fn markup(&self) -> String {
        let mut lines: Vec<String> = self
            .lines()
            .iter()
            .map(|line| format!("<b>{}</b> {}", line.label, escape(&line.value)))
            .collect();
        lines.push(String::new());
        lines.push("<b>From here you can</b>".to_owned());
        lines.extend(capabilities().iter().map(|capability| {
            // The bullet is a literal here (not markup), so only the text is escaped.
            format!("• {}", escape(capability))
        }));
        lines.join("\n")
    }

    /// The label of the active window, or `no active window`.
    fn active_label(&self) -> String {
        self.window
            .clone()
            .unwrap_or_else(|| "no active window".to_owned())
    }

    /// The `Control` row's value.
    fn control_text(&self) -> String {
        if self.focused {
            format!("the desktop has the keyboard ({ESCAPE_LABEL} releases it)")
        } else {
            "released — click the desktop to take the keyboard back".to_owned()
        }
    }
}

/// What the human can do from the viewer, in the order the panel lists it.
///
/// Kept deliberately short: it is a cheat-sheet, not documentation.
pub(crate) fn capabilities() -> Vec<String> {
    vec![
        "click, drag and scroll on the desktop — they are sent to the remote app".to_owned(),
        "type while the desktop has the keyboard; Tab, Escape, Space and Ctrl-chords go to the \
         remote app, not to this window"
            .to_owned(),
        format!("release the keyboard with the header button (or {ESCAPE_LABEL}) and click the desktop to take it back"),
        "switch windows with the task bar at the bottom".to_owned(),
        "record the desktop with the Record button".to_owned(),
        "close this window to quit".to_owned(),
    ]
}

/// The hint shown while the desktop does **not** hold keyboard control, or
/// [`None`] while it does (the hint would then be noise).
pub(crate) fn focus_hint(focused: bool) -> Option<&'static str> {
    if focused {
        return None;
    }
    Some("Keyboard control is released — click the desktop to send clicks, keys and scroll to the remote app again")
}

/// The label the task bar and the help panel both use for `state`'s active
/// window, or [`None`] when no window is active.
///
/// Reuses the task-bar view model so the two never name a window differently.
pub(crate) fn active_window_label(state: &DesktopState) -> Option<String> {
    taskbar::entries(state)
        .into_iter()
        .find(|entry| entry.active)
        .map(|entry| entry.label)
}

/// Escapes `text` for Pango markup (`&`, `<` and `>` are the only characters
/// Pango treats specially).
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    use adesk_core::{AppId, Rect, WindowId, WindowInfo, WindowState};

    /// A window fixture with the given id, title and state.
    fn window(id: u64, title: Option<&str>, state: WindowState) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            app_id: Some(AppId::from("org.example.App")),
            title: title.map(str::to_owned),
            geometry: Rect {
                x: 0,
                y: 0,
                w: 1280,
                h: 800,
            },
            state,
            mapped: true,
            pid: None,
            created_seq: 1,
            last_commit_seq: 2,
            popup_count: 0,
        }
    }

    /// A one-window state whose active window carries `title`.
    fn state_with_active(title: &str) -> DesktopState {
        DesktopState {
            active_window_id: Some(WindowId(1)),
            windows: vec![window(1, Some(title), WindowState::Active)],
        }
    }

    #[test]
    fn a_new_model_is_connecting_with_the_desktop_in_control() {
        let help = Help::new("unix:/run/user/1000/adesk-viewer.sock");
        assert_eq!(help.state, ConnectionState::Connecting);
        assert!(help.is_focused());
        assert_eq!(help.window, None);
    }

    #[test]
    fn the_lines_name_the_endpoint_the_window_the_connection_and_the_control() {
        let mut help = Help::new("tcp:127.0.0.1:7100");
        help.set_window(Some("Files".to_owned()));
        help.set_state(ConnectionState::Connected);

        let lines = help.lines();
        let values: Vec<(&str, &str)> = lines
            .iter()
            .map(|line| (line.label, line.value.as_str()))
            .collect();
        assert_eq!(
            values,
            vec![
                ("Endpoint", "tcp:127.0.0.1:7100"),
                ("Window", "Files"),
                ("Connection", "connected"),
                (
                    "Control",
                    "the desktop has the keyboard (Ctrl+Alt+Escape releases it)"
                ),
            ]
        );
    }

    #[test]
    fn an_unknown_window_is_reported_as_none() {
        let help = Help::new("unix:/tmp/adesk-viewer.sock");
        assert_eq!(help.lines()[1].value, "no active window");
    }

    #[test]
    fn releasing_control_flips_the_control_line() {
        let mut help = Help::new("unix:/tmp/adesk-viewer.sock");
        help.set_state(ConnectionState::Connected);
        assert!(help.set_focused(false));
        assert!(!help.is_focused());
        assert_eq!(
            help.lines()[3].value,
            "released — click the desktop to take the keyboard back"
        );
        assert!(help.set_focused(true));
        assert!(help.lines()[3]
            .value
            .starts_with("the desktop has the keyboard"));
    }

    #[test]
    fn a_failed_state_names_the_reason() {
        let mut help = Help::new("unix:/tmp/adesk.sock");
        assert!(help.set_state(ConnectionState::Failed(
            "cannot connect to viewer socket unix:/tmp/adesk.sock: connection closed".to_owned()
        )));
        assert_eq!(
            help.lines()[2].value,
            "not connected — cannot connect to viewer socket unix:/tmp/adesk.sock: \
             connection closed"
        );
    }

    #[test]
    fn identical_values_are_not_reported_as_changes() {
        let mut help = Help::new("unix:/tmp/adesk-viewer.sock");
        assert!(help.set_window(Some("Files".to_owned())));
        assert!(!help.set_window(Some("Files".to_owned())));
        assert!(help.set_window(None));
        assert!(!help.set_window(None));

        assert!(help.set_state(ConnectionState::Connected));
        assert!(!help.set_state(ConnectionState::Connected));
        assert!(help.set_state(ConnectionState::Connecting));

        assert!(help.set_focused(false));
        assert!(!help.set_focused(false));
    }

    #[test]
    fn the_summary_is_one_line_naming_the_connection_the_window_and_the_control() {
        let mut help = Help::new("unix:/tmp/adesk-viewer.sock");
        help.set_state(ConnectionState::Connected);
        help.set_window(Some("Files".to_owned()));
        assert_eq!(help.summary(), "connected — Files — keyboard control");

        help.set_focused(false);
        assert_eq!(help.summary(), "connected — Files — no keyboard control");
    }

    #[test]
    fn the_markup_carries_every_line_and_the_capabilities() {
        let mut help = Help::new("unix:/tmp/adesk-viewer.sock");
        help.set_state(ConnectionState::Connected);
        let markup = help.markup();

        assert!(markup.contains("<b>Endpoint</b> unix:/tmp/adesk-viewer.sock"));
        assert!(markup.contains("<b>Window</b> no active window"));
        assert!(markup.contains("<b>Connection</b> connected"));
        assert!(markup.contains("<b>Control</b>"));
        assert!(markup.contains("<b>From here you can</b>"));
        for capability in capabilities() {
            assert!(markup.contains(&capability), "missing {capability}");
        }
        assert!(!markup.ends_with('\n'), "{markup:?}");
    }

    #[test]
    fn the_markup_escapes_untrusted_text() {
        // A window title is the runtime's (and the application's) to choose, so
        // it must never be able to inject markup.
        let mut help = Help::new("unix:/tmp/<socket>");
        help.set_window(Some("a <b>&amp;</b> title".to_owned()));
        let markup = help.markup();

        assert!(!markup.contains("<b>&amp;</b>"), "{markup}");
        assert!(
            markup.contains("a &lt;b&gt;&amp;amp;&lt;/b&gt; title"),
            "{markup}"
        );
        assert!(markup.contains("unix:/tmp/&lt;socket&gt;"), "{markup}");
    }

    #[test]
    fn the_focus_hint_is_shown_only_without_control() {
        assert_eq!(focus_hint(true), None);
        let hint = focus_hint(false).expect("an unfocused view gets a hint");
        assert!(hint.contains("click the desktop"), "{hint}");
    }

    #[test]
    fn the_active_window_label_follows_the_active_window() {
        let state = state_with_active("Files");
        assert_eq!(active_window_label(&state), Some("Files".to_owned()));

        let state = DesktopState {
            active_window_id: None,
            windows: vec![window(1, Some("Files"), WindowState::Inactive)],
        };
        assert_eq!(active_window_label(&state), None);
    }

    #[test]
    fn the_capabilities_name_the_escape_hatch() {
        let all = capabilities().join("\n");
        assert!(all.contains(ESCAPE_LABEL), "{all}");
        assert!(all.contains("task bar"), "{all}");
    }
}
