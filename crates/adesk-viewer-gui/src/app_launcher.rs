//! The application launcher's GTK-free model (`docs/viewer.md` §3, §5).
//!
//! The header's **Open app** affordance lets a human list the applications the
//! runtime can launch and start one without an AGP client: `list_apps` and
//! `launch_app` are VAP messages, so the GUI reaches them through the same
//! [`ViewerClient`](adesk_viewer::ViewerClient) the rest of this crate uses.
//!
//! This module owns every decision behind that UI — the filtering and ordering
//! of the fetched list, the launch state machine and the composed status text —
//! so it is unit-tested without a display; [`crate::app_launcher_view`] only
//! paints it.
//!
//! ## Fetch strategy: one fetch per popover open, filtered locally
//!
//! `list_apps` is a round trip over the socket, so the model never issues one
//! request per keystroke and never polls: it asks for the *full* list once when
//! the popover is opened ([`AppLauncher::should_fetch`] gates the request) and
//! [`filter`] narrows that cached list in memory for every keystroke. The
//! runtime's XDG registry is small and effectively static over a session, so a
//! local filter is cheaper *and* instant compared with the alternative — a
//! debounced server-side `list_apps(Some(query))`, which would need its own
//! timer and would make typing lag behind the network. The request carries no
//! query: the whole list travels once and every keystroke after that is free.
//!
//! ## Launch discovery
//!
//! A `launch_result` reply never carries the launched window (the runtime
//! correlates it asynchronously), so [`LaunchState::Launched`] records "waiting
//! for its window" and the window itself arrives through the ordinary desktop
//! state refresh the worker issues right after an accepted launch.

use adesk_core::AppId;
use adesk_viewer_proto::{AppEntry, LaunchOutcome};

use crate::help::escape;

/// The icon shown for an entry that declares none (or whose name the icon theme
/// cannot resolve): a generic "executable" icon, so every row keeps the same
/// shape instead of rendering GTK's missing-image placeholder.
pub(crate) const FALLBACK_ICON: &str = "application-x-executable-symbolic";

/// One row of the launcher's application list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppRow {
    /// The launch target.
    pub(crate) id: AppId,
    /// The human display name (the desktop entry's name, else the id).
    pub(crate) name: String,
    /// The icon name the entry declared, when it declared one.
    pub(crate) icon: Option<String>,
}

impl AppRow {
    /// The row's Pango markup: the name in bold with the desktop-file id under
    /// it. Both are runtime-supplied, so both are escaped.
    pub(crate) fn markup(&self) -> String {
        format!(
            "<b>{}</b>\n<span size=\"small\">{}</span>",
            escape(&self.name),
            escape(self.id.as_str())
        )
    }

    /// The icon to request from the theme: the entry's own name when it declared
    /// one, else [`FALLBACK_ICON`].
    pub(crate) fn icon_name(&self) -> &str {
        self.icon
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(FALLBACK_ICON)
    }

    /// The row's tooltip: the desktop-file id (never markup, so never escaped).
    pub(crate) fn tooltip(&self) -> String {
        self.id.to_string()
    }
}

/// The launch state machine.
///
/// It is driven by the worker's replies, not by the click: a click only moves
/// it to [`LaunchState::Launching`], and the outcome — an accepted launch or a
/// refusal such as `unknown_app` — always comes back from the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchState {
    /// Nothing has been launched from this viewer.
    Idle,
    /// A launch request is in flight.
    Launching {
        /// The application's display name.
        name: String,
        /// The application's desktop-file id.
        app_id: AppId,
    },
    /// The runtime accepted the launch; its window may not be visible yet.
    Launched {
        /// The application's display name.
        name: String,
        /// The application's desktop-file id.
        app_id: AppId,
        /// The window the reply already correlated, when it carried one (the
        /// runtime normally reports it later, through a state refresh).
        window_id: Option<adesk_core::WindowId>,
    },
    /// The launch was refused or failed.
    Failed {
        /// The application's display name.
        name: String,
        /// The application's desktop-file id.
        app_id: AppId,
        /// The failure's `Display`.
        message: String,
    },
}

/// The launcher's model: the last fetched application list, the current search
/// text and the launch state machine.
///
/// Every setter is idempotent and side-effect free, so the GTK layer can call
/// them from any widget signal.
#[derive(Debug, Clone)]
pub(crate) struct AppLauncher {
    /// The full list as last fetched (never filtered in place).
    entries: Vec<AppEntry>,
    /// Whether the list was fetched successfully at least once.
    listed: bool,
    /// Whether a list fetch is in flight.
    fetching: bool,
    /// The last list fetch failure.
    list_error: Option<String>,
    /// The current search text.
    query: String,
    /// The launch state machine.
    launch: LaunchState,
}

impl Default for AppLauncher {
    fn default() -> AppLauncher {
        AppLauncher::new()
    }
}

impl AppLauncher {
    /// Creates a launcher with no list and no launch yet.
    pub(crate) fn new() -> AppLauncher {
        AppLauncher {
            entries: Vec::new(),
            listed: false,
            fetching: false,
            list_error: None,
            query: String::new(),
            launch: LaunchState::Idle,
        }
    }

    /// Replaces the search text; reports whether it changed.
    pub(crate) fn set_query(&mut self, query: &str) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query.to_owned();
        true
    }

    /// Whether the launcher wants a list fetch right now: any time one is not
    /// already in flight. The popover opener asks this every time it opens, so
    /// the list is re-read (never polled) on each visit and a failed attempt is
    /// retried.
    pub(crate) fn should_fetch(&self) -> bool {
        !self.fetching
    }

    /// Marks a list fetch as started, clearing the previous failure so the
    /// status line does not report a stale error while loading.
    pub(crate) fn begin_fetch(&mut self) {
        self.fetching = true;
        self.list_error = None;
    }

    /// Applies the worker's `list_apps` reply.
    pub(crate) fn apply_list(&mut self, result: Result<Vec<AppEntry>, String>) {
        self.fetching = false;
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.listed = true;
                self.list_error = None;
            }
            Err(message) => self.list_error = Some(message),
        }
    }

    /// The launch state machine's current state.
    pub(crate) fn launch(&self) -> &LaunchState {
        &self.launch
    }

    /// Whether a launch is in flight (so a second click must be ignored).
    pub(crate) fn is_launching(&self) -> bool {
        matches!(self.launch, LaunchState::Launching { .. })
    }

    /// Starts a launch for `app_id`; reports whether it was started.
    ///
    /// Returns `false` while another launch is in flight, so repeated clicks on
    /// a row (or on several rows) cannot enqueue several launches.
    pub(crate) fn begin_launch(&mut self, app_id: &AppId) -> bool {
        if self.is_launching() {
            return false;
        }
        self.launch = LaunchState::Launching {
            name: self.name_for(app_id),
            app_id: app_id.clone(),
        };
        true
    }

    /// Applies the worker's `launch_app` reply.
    ///
    /// The reply resolves the pending [`LaunchState::Launching`] entry, so it is
    /// ignored when nothing is pending: a stale reply can never invent a launch
    /// the human did not ask for.
    pub(crate) fn apply_launch(&mut self, result: Result<LaunchOutcome, String>) {
        let LaunchState::Launching { name, app_id } = self.launch.clone() else {
            return;
        };
        self.launch = match result {
            Ok(outcome) => LaunchState::Launched {
                name,
                app_id,
                window_id: outcome.window_id,
            },
            Err(message) => LaunchState::Failed {
                name,
                app_id,
                message,
            },
        };
    }

    /// The rows to show for the current query: the fetched list filtered and
    /// ordered by [`filter`].
    pub(crate) fn rows(&self) -> Vec<AppRow> {
        filter(&self.entries, &self.query)
    }

    /// The launcher's status line, or [`None`] when there is nothing to say.
    ///
    /// The launch state wins (it is the most recent and most relevant event),
    /// then a list failure, then the loading hint, then the match count.
    pub(crate) fn status_text(&self) -> Option<String> {
        let launch_status = match &self.launch {
            LaunchState::Launching { name, .. } => Some(format!("launching {name}…")),
            LaunchState::Launched {
                name, window_id, ..
            } => Some(match window_id {
                Some(window_id) => format!("launched {name} (window {window_id})"),
                None => format!("launched {name} — waiting for its window"),
            }),
            LaunchState::Failed { name, message, .. } => {
                Some(format!("could not launch {name}: {message}"))
            }
            LaunchState::Idle => None,
        };
        launch_status
            .or_else(|| {
                self.list_error
                    .as_deref()
                    .map(|error| format!("could not list applications: {error}"))
            })
            .or_else(|| (self.fetching && !self.listed).then(|| "loading applications…".to_owned()))
            .or_else(|| self.listed.then(|| self.match_text()))
    }

    /// The one-line notice to show outside the popover after a failed launch, or
    /// [`None`] after a successful one.
    ///
    /// A launch failure must stay visible after the popover closes, so the GTK
    /// layer reveals the window's notice line with this text.
    pub(crate) fn launch_notice(&self) -> Option<String> {
        match &self.launch {
            LaunchState::Failed { name, message, .. } => {
                Some(format!("could not launch {name}: {message}"))
            }
            _ => None,
        }
    }

    /// The `n application(s)` / `n match(es)` / `no application matches "…"`
    /// line, composed from the fetched list and the current query.
    fn match_text(&self) -> String {
        let matches = self.rows().len();
        let total = self.entries.len();
        let query = self.query.trim();
        if query.is_empty() {
            return pluralize(matches, "application");
        }
        if matches == 0 {
            return format!("no application matches “{query}”");
        }
        format!("{matches} of {total} match")
    }

    /// The display name of `app_id` in the fetched list, else the id itself (the
    /// runtime is the authority on which ids exist, so a stale list must never
    /// block a launch).
    fn name_for(&self, app_id: &AppId) -> String {
        self.entries
            .iter()
            .find(|entry| &entry.id == app_id)
            .map(display_name)
            .unwrap_or_else(|| app_id.to_string())
    }
}

/// Filters `entries` for `query` and orders the result for presentation.
///
/// - an empty (or whitespace-only) query matches every entry;
/// - otherwise an entry matches when `query` is a case-insensitive substring of
///   its display name *or* its desktop-file id;
/// - the result is sorted by display name (case-insensitively), ties broken by
///   the desktop-file id, so the order is stable across refetches.
pub(crate) fn filter(entries: &[AppEntry], query: &str) -> Vec<AppRow> {
    let needle = query.trim().to_lowercase();
    let mut rows: Vec<AppRow> = entries
        .iter()
        .filter(|entry| matches(entry, &needle))
        .map(row)
        .collect();
    rows.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    rows
}

/// Whether `entry` matches the already-lowercased `needle` (an empty needle
/// matches everything).
fn matches(entry: &AppEntry, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    entry.name.to_lowercase().contains(needle) || entry.id.as_str().to_lowercase().contains(needle)
}

/// Projects an entry onto a display row.
fn row(entry: &AppEntry) -> AppRow {
    AppRow {
        id: entry.id.clone(),
        name: display_name(entry),
        icon: entry.icon.clone(),
    }
}

/// The name to show for `entry`: its display name, else its desktop-file id (an
/// entry with a blank name would otherwise render as an empty row).
fn display_name(entry: &AppEntry) -> String {
    let name = entry.name.trim();
    if name.is_empty() {
        entry.id.to_string()
    } else {
        name.to_owned()
    }
}

/// `1 application` / `2 applications`.
fn pluralize(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use adesk_core::{LaunchId, WindowId};

    /// An application entry with the given id, name and icon.
    fn entry(id: &str, name: &str, icon: Option<&str>) -> AppEntry {
        AppEntry {
            id: AppId::from(id),
            name: name.to_owned(),
            icon: icon.map(str::to_owned),
            categories: Vec::new(),
        }
    }

    /// The three-entry fixture list used by most tests.
    fn apps() -> Vec<AppEntry> {
        vec![
            entry("org.gnome.Nautilus", "Files", Some("org.gnome.Nautilus")),
            entry("org.mozilla.firefox", "Firefox", None),
            entry("org.gnome.TextEditor", "Text Editor", None),
        ]
    }

    /// A launcher holding the fixture list.
    fn listed() -> AppLauncher {
        let mut launcher = AppLauncher::new();
        launcher.begin_fetch();
        launcher.apply_list(Ok(apps()));
        launcher
    }

    #[test]
    fn an_empty_query_lists_every_application_by_name() {
        let rows = filter(&apps(), "");
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["Files", "Firefox", "Text Editor"]);
        assert_eq!(rows[0].id, AppId::from("org.gnome.Nautilus"));
    }

    #[test]
    fn a_whitespace_query_matches_everything() {
        assert_eq!(filter(&apps(), "   ").len(), 3);
    }

    #[test]
    fn filtering_matches_the_name_case_insensitively() {
        let rows = filter(&apps(), "fIrE");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Firefox");
    }

    #[test]
    fn filtering_matches_the_desktop_file_id() {
        let rows = filter(&apps(), "texteditor");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, AppId::from("org.gnome.TextEditor"));
    }

    #[test]
    fn filtering_that_matches_nothing_yields_no_rows() {
        assert!(filter(&apps(), "does-not-exist").is_empty());
    }

    #[test]
    fn filtering_ignores_the_surrounding_whitespace() {
        assert_eq!(filter(&apps(), "  files  ").len(), 1);
    }

    #[test]
    fn ordering_is_case_insensitive_by_name_then_id() {
        let entries = vec![
            entry("z", "banana", None),
            entry("a", "Apple", None),
            entry("b", "apple", None),
        ];
        let rows = filter(&entries, "");
        assert_eq!(rows[0].id, AppId::from("a"));
        assert_eq!(rows[1].id, AppId::from("b"));
        assert_eq!(rows[2].id, AppId::from("z"));
    }

    #[test]
    fn a_blank_name_falls_back_to_the_id() {
        let rows = filter(&[entry("org.example.App", "  ", None)], "");
        assert_eq!(rows[0].name, "org.example.App");
        assert_eq!(rows[0].icon_name(), FALLBACK_ICON);
    }

    #[test]
    fn an_entry_without_an_icon_uses_the_fallback_name() {
        let rows = filter(&apps(), "firefox");
        assert_eq!(rows[0].icon_name(), FALLBACK_ICON);

        let rows = filter(&apps(), "files");
        assert_eq!(rows[0].icon_name(), "org.gnome.Nautilus");
    }

    #[test]
    fn the_row_markup_escapes_the_name_and_the_id() {
        let rows = filter(
            &[entry("org.example.<App>", "a <b>&amp;</b> app", None)],
            "",
        );
        let markup = rows[0].markup();
        assert!(!markup.contains("<b>&amp;</b>"), "{markup}");
        assert!(
            markup.contains("a &lt;b&gt;&amp;amp;&lt;/b&gt; app"),
            "{markup}"
        );
        assert!(markup.contains("org.example.&lt;App&gt;"), "{markup}");
        // The tooltip is plain text, so it stays unescaped.
        assert_eq!(rows[0].tooltip(), "org.example.<App>");
    }

    #[test]
    fn a_new_launcher_wants_a_fetch_and_has_no_status() {
        let launcher = AppLauncher::new();
        assert!(launcher.should_fetch());
        assert!(launcher.rows().is_empty());
        assert_eq!(launcher.launch(), &LaunchState::Idle);
        assert_eq!(launcher.status_text(), None);
    }

    #[test]
    fn an_open_popover_fetches_once_and_shows_the_loading_hint() {
        let mut launcher = AppLauncher::new();
        assert!(launcher.should_fetch());
        launcher.begin_fetch();
        assert!(!launcher.should_fetch());
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("loading applications…")
        );

        launcher.apply_list(Ok(apps()));
        assert_eq!(launcher.status_text().as_deref(), Some("3 applications"));
        assert!(launcher.should_fetch());
    }

    #[test]
    fn the_match_count_reflects_the_query() {
        let mut launcher = listed();
        assert_eq!(launcher.status_text().as_deref(), Some("3 applications"));

        launcher.set_query("f");
        assert_eq!(launcher.status_text().as_deref(), Some("2 of 3 match"));

        launcher.set_query("firefox");
        assert_eq!(launcher.status_text().as_deref(), Some("1 of 3 match"));
    }

    #[test]
    fn an_unmatched_query_says_so() {
        let mut launcher = listed();
        assert!(launcher.set_query("nothing"));
        assert!(launcher.rows().is_empty());
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("no application matches “nothing”")
        );
    }

    #[test]
    fn an_unchanged_query_is_not_reported_as_a_change() {
        let mut launcher = listed();
        assert!(launcher.set_query("fi"));
        assert!(!launcher.set_query("fi"));
        assert!(launcher.set_query("firefox"));
        assert_eq!(launcher.status_text().as_deref(), Some("1 of 3 match"));
    }

    #[test]
    fn a_failed_fetch_is_surfaced_and_retried() {
        let mut launcher = AppLauncher::new();
        launcher.begin_fetch();
        launcher.apply_list(Err("not_supported".to_owned()));

        assert_eq!(
            launcher.status_text().as_deref(),
            Some("could not list applications: not_supported")
        );
        // The failure clears the in-flight flag, so the next open retries.
        assert!(launcher.should_fetch());
    }

    #[test]
    fn a_launch_is_pending_until_the_reply_arrives() {
        let mut launcher = listed();
        assert!(launcher.begin_launch(&AppId::from("org.mozilla.firefox")));

        assert!(launcher.is_launching());
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("launching Firefox…")
        );
        // A second click cannot enqueue another launch.
        assert!(!launcher.begin_launch(&AppId::from("org.gnome.Nautilus")));

        launcher.apply_launch(Ok(LaunchOutcome {
            app_id: AppId::from("org.mozilla.firefox"),
            launch_id: LaunchId(7),
            action_id: None,
            window_id: None,
        }));
        assert!(!launcher.is_launching());
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("launched Firefox — waiting for its window")
        );
        assert_eq!(launcher.launch_notice(), None);
    }

    #[test]
    fn a_correlated_launch_names_its_window() {
        let mut launcher = listed();
        launcher.begin_launch(&AppId::from("org.gnome.Nautilus"));
        launcher.apply_launch(Ok(LaunchOutcome {
            app_id: AppId::from("org.gnome.Nautilus"),
            launch_id: LaunchId(1),
            action_id: None,
            window_id: Some(WindowId(4)),
        }));
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("launched Files (window 4)")
        );
    }

    #[test]
    fn a_refused_launch_is_surfaced_in_the_status_and_the_notice() {
        let mut launcher = listed();
        launcher.begin_launch(&AppId::from("org.mozilla.firefox"));
        launcher.apply_launch(Err("unknown app org.mozilla.firefox".to_owned()));

        assert_eq!(
            launcher.status_text().as_deref(),
            Some("could not launch Firefox: unknown app org.mozilla.firefox")
        );
        assert_eq!(
            launcher.launch_notice().as_deref(),
            Some("could not launch Firefox: unknown app org.mozilla.firefox")
        );
        // The failure leaves the machine ready for another attempt.
        assert!(!launcher.is_launching());
        assert!(launcher.begin_launch(&AppId::from("org.mozilla.firefox")));
    }

    #[test]
    fn an_app_outside_the_fetched_list_launches_under_its_id() {
        let mut launcher = listed();
        let id = AppId::from("org.example.Unknown");
        assert!(launcher.begin_launch(&id));
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("launching org.example.Unknown…")
        );
    }

    #[test]
    fn a_stale_reply_is_ignored() {
        let mut launcher = listed();
        // Nothing pending: a reply must not invent a launch state.
        launcher.apply_launch(Ok(LaunchOutcome {
            app_id: AppId::from("org.mozilla.firefox"),
            launch_id: LaunchId(1),
            action_id: None,
            window_id: None,
        }));
        assert_eq!(launcher.launch(), &LaunchState::Idle);

        // A different launch pending: the stale reply is still ignored.
        launcher.begin_launch(&AppId::from("org.gnome.Nautilus"));
        launcher.apply_launch(Err("unknown app".to_owned()));
        assert!(matches!(
            launcher.launch(),
            LaunchState::Failed { name, .. } if name.as_str() == "Files"
        ));
    }

    #[test]
    fn a_launch_failure_outranks_the_list_status() {
        let mut launcher = listed();
        launcher.begin_launch(&AppId::from("org.mozilla.firefox"));
        launcher.apply_launch(Err("launch failed".to_owned()));
        launcher.begin_fetch();
        assert_eq!(
            launcher.status_text().as_deref(),
            Some("could not launch Firefox: launch failed")
        );
    }
}
