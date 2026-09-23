//! Bridge between the compositor and `adesk-wm`.
//!
//! `adesk-wm` is the pure-logic window model: it owns the window registry, the
//! single-visible-toplevel tiling policy, the focus state machine and — crucially —
//! the conversion of window-relative coordinates to output coordinates
//! (`docs/architecture.md` §4). It never sees Smithay types.
//!
//! [`WmBridge`] is the only place in this crate where Smithay surfaces meet the
//! window model, and the only place allowed to call `adesk_wm`. Protocol handlers
//! and the command dispatcher go through [`crate::state::State`], never through the
//! window manager directly.
//!
//! # Bridge API
//!
//! Accessors and queries (frozen — other compositor code and the render/inject paths
//! rely on them):
//!
//! - `new(output_size) -> WmBridge`
//! - `active_window() -> Option<WindowId>`, `keyboard_focus() -> Option<WindowId>`
//! - `window_for_surface(&WlSurface) -> Option<WindowId>` (toplevel, subsurface or popup)
//! - `windows() -> Vec<WindowInfo>`, `tiled_rect() -> Rect`
//! - `toplevel_of(WindowId) -> Option<ToplevelSurface>`, `surface_of(WindowId) -> Option<WlSurface>`
//! - `root_id(WindowId) -> Option<ObjectId>`, `window_geometry(WindowId) -> Option<Rect>`
//!   (O(1) lookups for the commit and render hot paths, which need one id/rect, not a
//!   projected `Vec<WindowInfo>`)
//! - `last_commit_seq(WindowId) -> u64`
//! - `resolve_position(WindowId, &Position) -> Result<Point>`
//! - `note_launch(LaunchId, AppId, Option<i32>)`
//!
//! Lifecycle (used by [`crate::state`] and `crate::protocols::xdg_shell`):
//!
//! - `register_toplevel(&ToplevelSurface) -> SurfaceKey` — called from
//!   `XdgShellHandler::new_toplevel`, before the first commit; idempotent.
//! - `map_toplevel(&ToplevelSurface, pid, created_seq) -> MapOutcome` — called on the
//!   first *buffer* commit of a registered toplevel root. `Created` carries the id, the
//!   metadata for `WindowCreated` and the [`WmDecision`] to apply; `AlreadyMapped` is
//!   the idempotent duplicate-map case (adesk-wm returns `[WmAction::None]`).
//! - `destroy_toplevel(&ToplevelSurface) -> Option<DestroyedWindow>` — drops the
//!   window and its popups; the returned popup ids must be emitted *before*
//!   `WindowDestroyed`.
//! - `title_changed(&ToplevelSurface) -> Option<TitleChange>` — `None` when the title
//!   did not actually change.
//! - `app_id_changed(&ToplevelSurface) -> Option<AppIdChange>` — change detection, the
//!   late-`set_app_id` write-back into the window model, and launch-correlation
//!   bookkeeping (there is no app-id event in AGP v1).
//! - `popup_added(&PopupSurface, offset) -> Option<PopupAdded>`,
//!   `popup_removed(&PopupSurface) -> Option<PopupRemoved>` — stable `popup_id`s and
//!   the owner window.
//! - `popup_window_offset(&ObjectId) -> Option<(i32, i32)>` — window-relative popup
//!   origin, accumulated over the popup chain.
//! - `commit(WindowId, &Region) -> u64` — `last_commit_seq + 1`, fed through
//!   `WindowManager::on_commit`.
//! - `activate(WindowId) -> Result<WmDecision>` — `require_window` + `activate`
//!   (`unknown_window` for an unknown id, `[WmAction::None]` when already active).
//! - `note_popup_grab(&PopupSurface)`, `popup_grab() -> Option<PopupGrab>`,
//!   `take_popup_grab() -> Option<PopupGrab>` — v1 grab bookkeeping (see below).
//!
//! # Launch correlation
//!
//! `note_launch` records a pending launch so a toplevel mapping shortly afterwards can
//! be correlated with it. The tiers are the documented ones (exact pid → app id /
//! `StartupWMClass` → title substring, most-recent tie-break, expiry after 10s).
//!
//! The server feeds it with `RuntimeCommand::NoteLaunch` after every successful
//! `launch_app`; the compositor's own `WindowCreated` broadcast is the event that
//! needs the id, because the server-side `adesk_app_registry::Correlator` only
//! stamps the events the server projects.
//!
//! # Popup grabs (v1 semantics)
//!
//! A grab is *recorded*, never enforced by the seat: the headless runtime has no
//! physical pointer, so there is no pointer-leave that could dismiss a popup. The most
//! recent grab wins (client-side popup stacks). It is cleared when the grabbing popup
//! or its owner is destroyed, and `State` sends `popup_done` when a runtime-native
//! operation makes the grab stale (`activate_window` to another window, `close_window`
//! of the owner).

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use adesk_core::{AppId, LaunchId, Point, Position, Rect, Region, Size, WindowId, WindowInfo};
use adesk_wm::{MapRequest, SurfaceKey, WmAction};
use smithay::{
    reexports::wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource},
    wayland::{
        compositor::with_states,
        shell::xdg::{PopupSurface, ToplevelSurface, XdgToplevelSurfaceData},
    },
};

use crate::{error::CompositorError, Result};

mod registry;

use registry::SurfaceRegistry;

/// How long a pending launch stays eligible for correlation with a window.
const LAUNCH_CORRELATION_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound on remembered pending launches (bounded memory, oldest dropped).
const MAX_PENDING_LAUNCHES: usize = 32;
/// Safety bound for surface-tree (parent) walks.
///
/// Shared with `crate::state::State`, which walks the same tree to compute a
/// committing surface's offset inside its window.
pub(crate) const MAX_SURFACE_TREE_DEPTH: usize = 32;
/// Shortest title substring considered evidence (avoids `"a"` matching everything).
const MIN_TITLE_MATCH_LEN: usize = 3;

/// The actions a window-manager call returned, plus the focus target they were
/// decided against.
///
/// `previous_focus` is captured *before* the policy ran, so `WindowActivated`'s
/// `previous` field is the window that had focus when the decision was made, not the
/// window that has it afterwards.
#[derive(Debug, Clone, Default)]
pub(crate) struct WmDecision {
    /// Actions to apply in order.
    pub(crate) actions: Vec<WmAction>,
    /// Keyboard focus before the policy call.
    pub(crate) previous_focus: Option<WindowId>,
}

/// Result of a toplevel map.
#[derive(Debug, Clone)]
pub(crate) enum MapOutcome {
    /// A new window was created; emit `WindowCreated` and apply `decision`.
    Created(Box<MappedWindow>),
    /// The surface tree already has a window (idempotent duplicate map, no event).
    AlreadyMapped(WindowId),
}

/// Everything `WindowCreated` needs, plus the actions to apply afterwards.
#[derive(Debug, Clone)]
pub(crate) struct MappedWindow {
    /// Assigned window id.
    pub(crate) id: WindowId,
    /// Client `app_id`, when set.
    pub(crate) app_id: Option<AppId>,
    /// Client pid, when known.
    pub(crate) pid: Option<i32>,
    /// Toplevel title, when set.
    pub(crate) title: Option<String>,
    /// Launch this window was correlated with, if any.
    pub(crate) launch_id: Option<LaunchId>,
    /// Map actions (`ConfigureWindow`, `Activate`).
    pub(crate) decision: WmDecision,
}

/// What a toplevel destroy removed from the model.
#[derive(Debug, Clone)]
pub(crate) struct DestroyedWindow {
    /// The window that went away, if it had been mapped.
    pub(crate) window_id: Option<WindowId>,
    /// Open popups of that window, in creation order (emit before `WindowDestroyed`).
    pub(crate) popup_ids: Vec<u64>,
    /// Fallback actions (`ActivatePrevious`), applied after `WindowDestroyed`.
    pub(crate) decision: WmDecision,
}

/// A title change that actually changed the title.
#[derive(Debug, Clone)]
pub(crate) struct TitleChange {
    /// Window whose title changed.
    pub(crate) window_id: WindowId,
    /// New title.
    pub(crate) title: Option<String>,
    /// Actions returned by the policy (empty in v1).
    pub(crate) decision: WmDecision,
}

/// An `app_id` change observed after the map.
#[derive(Debug, Clone)]
pub(crate) struct AppIdChange {
    /// Window whose app id changed.
    pub(crate) window_id: WindowId,
    /// The new app id.
    pub(crate) app_id: Option<AppId>,
    /// Launch newly correlated thanks to the app id, if any.
    pub(crate) launch_id: Option<LaunchId>,
}

/// A popup that became visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupAdded {
    /// Owner window.
    pub(crate) window_id: WindowId,
    /// Stable popup id (never reused).
    pub(crate) popup_id: u64,
}

/// A popup that went away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupRemoved {
    /// Owner window.
    pub(crate) window_id: WindowId,
    /// The id that was announced by [`PopupAdded`].
    pub(crate) popup_id: u64,
}

/// The most recent popup grab.
#[derive(Debug, Clone)]
pub(crate) struct PopupGrab {
    /// Surface that requested the grab.
    pub(crate) popup: ObjectId,
    /// Owner window.
    pub(crate) window_id: WindowId,
    /// Id announced by [`PopupAdded`].
    pub(crate) popup_id: u64,
}

/// A launch that was recorded but is not yet correlated with a window.
#[derive(Debug, Clone)]
struct PendingLaunch {
    launch_id: LaunchId,
    app_id: AppId,
    pid: Option<i32>,
    noted_at: Instant,
}

/// Pending launches and the documented correlation tiers.
#[derive(Debug, Default)]
struct LaunchLedger {
    pending: Vec<PendingLaunch>,
}

impl LaunchLedger {
    /// Record a launch, expiring stale entries first.
    fn record(&mut self, launch_id: LaunchId, app_id: AppId, pid: Option<i32>, now: Instant) {
        self.expire(now);
        if self.pending.len() >= MAX_PENDING_LAUNCHES {
            self.pending.remove(0);
        }
        self.pending.push(PendingLaunch {
            launch_id,
            app_id,
            pid,
            noted_at: now,
        });
    }

    /// Drop launches that are older than the correlation window.
    fn expire(&mut self, now: Instant) {
        self.pending.retain(|launch| {
            now.saturating_duration_since(launch.noted_at) <= LAUNCH_CORRELATION_TIMEOUT
        });
    }

    /// Find the launch a freshly mapped window belongs to.
    ///
    /// Tiers, in order: exact pid, app id / `StartupWMClass` match, title substring.
    /// Within a tier the most recently recorded launch wins; expired launches are
    /// ignored.
    fn correlate(
        &self,
        app_id: Option<&AppId>,
        pid: Option<i32>,
        title: Option<&str>,
        now: Instant,
    ) -> Option<LaunchId> {
        let fresh = |launch: &&PendingLaunch| {
            now.saturating_duration_since(launch.noted_at) <= LAUNCH_CORRELATION_TIMEOUT
        };

        if let Some(pid) = pid {
            if let Some(launch) = self
                .pending
                .iter()
                .rfind(|launch| fresh(launch) && launch.pid == Some(pid))
            {
                return Some(launch.launch_id);
            }
        }
        if let Some(app_id) = app_id {
            if let Some(launch) = self
                .pending
                .iter()
                .rfind(|launch| fresh(launch) && app_ids_match(&launch.app_id, app_id))
            {
                return Some(launch.launch_id);
            }
        }
        if let Some(title) = title {
            if let Some(launch) = self
                .pending
                .iter()
                .rfind(|launch| fresh(launch) && title_matches(&launch.app_id, title))
            {
                return Some(launch.launch_id);
            }
        }
        None
    }

    /// Remove a correlated launch so it cannot match a second window.
    fn take(&mut self, launch_id: LaunchId) {
        self.pending.retain(|launch| launch.launch_id != launch_id);
    }

    /// Number of pending launches. Test-only: no production path inspects the
    /// ledger's size, it only records, correlates and expires.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.pending.len()
    }
}

/// The last dot-separated segment of a desktop-file id (`"firefox"`).
fn app_id_tail(app_id: &AppId) -> &str {
    app_id
        .as_str()
        .rsplit('.')
        .next()
        .unwrap_or(app_id.as_str())
}

/// App ids match when they are equal or share their tail (`StartupWMClass`).
fn app_ids_match(launched: &AppId, window: &AppId) -> bool {
    launched.as_str().eq_ignore_ascii_case(window.as_str())
        || app_id_tail(launched).eq_ignore_ascii_case(app_id_tail(window))
}

/// Title evidence: the window title contains the app id or its tail.
fn title_matches(app_id: &AppId, title: &str) -> bool {
    let title = title.to_ascii_lowercase();
    let full = app_id.as_str().to_ascii_lowercase();
    if full.len() >= MIN_TITLE_MATCH_LEN && title.contains(&full) {
        return true;
    }
    let tail = app_id_tail(app_id).to_ascii_lowercase();
    tail.len() >= MIN_TITLE_MATCH_LEN && title.contains(&tail)
}

/// Adapter around `adesk_wm::WindowManager`.
///
/// The manager is the single source of truth for window geometry, focus and
/// lifecycle; this type adds the Smithay surface lookups the protocol handlers need
/// and keeps `adesk-wm` free of compositor types.
pub(crate) struct WmBridge {
    /// The pure-logic window model.
    manager: adesk_wm::WindowManager,
    /// Surface-tree bookkeeping (stable keys, owners, popups).
    surfaces: SurfaceRegistry<ObjectId>,
    /// Registered toplevels by root surface id (mapped or not).
    toplevels: HashMap<ObjectId, ToplevelSurface>,
    /// Mapped window → its root surface id.
    roots: HashMap<WindowId, ObjectId>,
    /// Popup handles, so a grab can be dismissed with `popup_done`.
    popup_handles: HashMap<ObjectId, PopupSurface>,
    /// The most recent popup grab.
    grab: Option<PopupGrab>,
    /// Pending launches for window correlation.
    launches: LaunchLedger,
    /// Launch recorded for each window, so it is reported once.
    launch_ids: HashMap<WindowId, LaunchId>,
    /// Last observed app id per window (change detection).
    app_ids: HashMap<WindowId, Option<AppId>>,
}

impl WmBridge {
    /// Create the bridge around a fresh window manager for the given output size.
    ///
    /// The output size is the tiling target: the active window always fills exactly
    /// this area (`docs/architecture.md` §4).
    pub(crate) fn new(output_size: Size) -> WmBridge {
        WmBridge {
            manager: adesk_wm::WindowManager::new(adesk_wm::PolicyConfig::new(output_size)),
            surfaces: SurfaceRegistry::new(),
            toplevels: HashMap::new(),
            roots: HashMap::new(),
            popup_handles: HashMap::new(),
            grab: None,
            launches: LaunchLedger::default(),
            launch_ids: HashMap::new(),
            app_ids: HashMap::new(),
        }
    }

    /// The active (visible, tiled) window, if any.
    pub(crate) fn active_window(&self) -> Option<WindowId> {
        self.manager.active_window()
    }

    /// The window that currently holds keyboard focus, if any.
    ///
    /// Focus follows the active window: the compositor moves seat focus in the same
    /// call that applies `WmAction::Activate`, so the two only differ while an action
    /// is being applied.
    pub(crate) fn keyboard_focus(&self) -> Option<WindowId> {
        self.manager.active_window()
    }

    /// The window owning a Wayland surface (toplevel, subsurface or popup).
    ///
    /// The registry stores toplevel roots and popups; a subsurface is resolved by
    /// walking up the surface tree to the first tracked ancestor.
    pub(crate) fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        let id = surface.id();
        if let Some(window) = self.surfaces.window_for_surface(&id) {
            return Some(window);
        }
        let mut current = surface.clone();
        for _ in 0..MAX_SURFACE_TREE_DEPTH {
            let parent = smithay::wayland::compositor::get_parent(&current)?;
            if let Some(window) = self.surfaces.window_for_surface(&parent.id()) {
                return Some(window);
            }
            current = parent;
        }
        None
    }

    /// All known windows in creation order, for `QueryState`.
    pub(crate) fn windows(&self) -> Vec<WindowInfo> {
        self.manager
            .windows()
            .iter()
            .map(|record| record.info())
            .collect()
    }

    /// The rect every mapped window is tiled to.
    pub(crate) fn tiled_rect(&self) -> Rect {
        self.manager.tiled_rect()
    }

    /// The toplevel surface of a window, for configuring or closing it.
    pub(crate) fn toplevel_of(&self, id: WindowId) -> Option<ToplevelSurface> {
        let root = self.roots.get(&id)?;
        self.toplevels.get(root).cloned()
    }

    /// The root `wl_surface` of a window, for keyboard/pointer focus.
    pub(crate) fn surface_of(&self, id: WindowId) -> Option<WlSurface> {
        self.toplevel_of(id)
            .map(|toplevel| toplevel.wl_surface().clone())
    }

    /// The `ObjectId` of a window's toplevel root, for O(1) identity comparisons.
    ///
    /// Used by the commit path to detect the common case — the committing surface is
    /// the window root — by `ObjectId` rather than by cloning and comparing surfaces.
    pub(crate) fn root_id(&self, id: WindowId) -> Option<ObjectId> {
        self.roots.get(&id).cloned()
    }

    /// The rect a window is tiled to, or `None` when the window is not tracked.
    ///
    /// Unlike `windows()`, this reads the one record and clones no `WindowInfo`.
    pub(crate) fn window_geometry(&self, id: WindowId) -> Option<Rect> {
        self.manager.window(id).map(|record| record.geometry)
    }

    /// Per-window commit counter of the last observed commit.
    pub(crate) fn last_commit_seq(&self, id: WindowId) -> u64 {
        self.manager
            .window(id)
            .map(|record| record.last_commit_seq)
            .unwrap_or(0)
    }

    /// Record that the app registry spawned a process, so a toplevel mapping shortly
    /// afterwards can be correlated with the launch.
    ///
    /// This is the compositor-local launch correlation, fed by
    /// [`RuntimeCommand::NoteLaunch`](crate::RuntimeCommand): the server's `launch_app`
    /// sends it right after a successful spawn, `run::dispatch` serves it and
    /// `State::note_launch` forwards it here, so the compositor's own `WindowCreated`
    /// broadcast can carry `launch_id`. The server-side
    /// `adesk_app_registry::Correlator` additionally stamps the events the server
    /// projects.
    pub(crate) fn note_launch(&mut self, launch_id: LaunchId, app_id: AppId, pid: Option<i32>) {
        self.launches.record(launch_id, app_id, pid, Instant::now());
    }

    /// Resolve a window-relative position to an output point.
    ///
    /// The window model — never a hard-coded constant — is the authority for
    /// geometry, so `(0,0)` output origin assumptions stay out of this crate.
    /// An unknown window surfaces as [`CompositorError::WindowManagement`].
    pub(crate) fn resolve_position(&self, id: WindowId, position: &Position) -> Result<Point> {
        self.manager
            .resolve_position(id, *position)
            .ok_or_else(|| CompositorError::WindowManagement(format!("unknown window {id}")))
    }

    // -----------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------

    /// Register a toplevel root surface (from `new_toplevel`), returning its stable key.
    pub(crate) fn register_toplevel(&mut self, toplevel: &ToplevelSurface) -> SurfaceKey {
        let key = self.surfaces.register_toplevel(toplevel.wl_surface().id());
        self.toplevels
            .entry(toplevel.wl_surface().id())
            .or_insert_with(|| toplevel.clone());
        key
    }

    /// The registered toplevel that owns `surface` and has not been mapped yet.
    pub(crate) fn unmapped_toplevel(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        let id = surface.id();
        if !self.surfaces.is_unmapped_toplevel(&id) {
            return None;
        }
        self.toplevels.get(&id).cloned()
    }

    /// Map a toplevel, assigning its window id and returning the actions to apply.
    pub(crate) fn map_toplevel(
        &mut self,
        toplevel: &ToplevelSurface,
        pid: Option<i32>,
        created_seq: u64,
    ) -> MapOutcome {
        let surface = toplevel.wl_surface().id();
        if let Some(slot) = self.surfaces.slot(&surface) {
            if let Some(existing) = slot.window {
                // adesk-wm treats a duplicate map as an evaluated no-op: never mint a
                // second id for one surface tree.
                return MapOutcome::AlreadyMapped(existing);
            }
        } else {
            // A toplevel that committed before `new_toplevel` registered it: register now.
            self.register_toplevel(toplevel);
        }

        let key = self
            .surfaces
            .slot(&surface)
            .map(|slot| slot.key)
            .unwrap_or_else(|| self.register_toplevel(toplevel));
        let (app_id, title) = toplevel_metadata(toplevel);
        let launch_id =
            self.launches
                .correlate(app_id.as_ref(), pid, title.as_deref(), Instant::now());

        let request = MapRequest {
            surface_key: key,
            app_id: app_id.clone(),
            pid,
            title: title.clone(),
            created_seq,
        };
        let previous_focus = self.keyboard_focus();
        let (id, actions) = self.manager.on_map(request);
        self.surfaces.bind(surface.clone(), id);
        self.roots.insert(id, surface);
        self.app_ids.insert(id, app_id.clone());
        if let Some(launch_id) = launch_id {
            self.launches.take(launch_id);
            self.launch_ids.insert(id, launch_id);
        }
        tracing::debug!(
            window_id = id.0,
            surface = %key,
            app_id = app_id.as_ref().map(AppId::as_str),
            "mapped toplevel through the window manager"
        );

        MapOutcome::Created(Box::new(MappedWindow {
            id,
            app_id,
            pid,
            title,
            launch_id,
            decision: WmDecision {
                actions,
                previous_focus,
            },
        }))
    }

    /// Drop a toplevel and every popup it owns.
    pub(crate) fn destroy_toplevel(
        &mut self,
        toplevel: &ToplevelSurface,
    ) -> Option<DestroyedWindow> {
        let surface = toplevel.wl_surface().id();
        self.surfaces.slot(&surface)?;
        let (window_id, popups) = self.surfaces.unbind_toplevel(&surface);
        self.toplevels.remove(&surface);
        // Popups die with their owner: drop the handle of exactly the popups the registry
        // just forgot. Their records are gone after this, so `popup_removed` returns early
        // for them and would never reap the handle itself.
        for popup in &popups {
            self.popup_handles.remove(&popup.surface);
        }
        if let Some(id) = window_id {
            self.roots.remove(&id);
            self.launch_ids.remove(&id);
            self.app_ids.remove(&id);
        }
        let actions = match window_id {
            Some(id) => self.manager.on_destroy(id),
            None => Vec::new(),
        };
        Some(DestroyedWindow {
            window_id,
            // In creation order: each popup's disappearance is emitted before the owner's.
            popup_ids: popups.into_iter().map(|popup| popup.popup_id).collect(),
            // `ActivatePrevious`'s predecessor is already destroyed, so its `previous`
            // is always `None`; capture nothing.
            decision: WmDecision {
                actions,
                previous_focus: None,
            },
        })
    }

    /// Handle a title change, or `None` when the title did not change.
    pub(crate) fn title_changed(&mut self, toplevel: &ToplevelSurface) -> Option<TitleChange> {
        let window_id = self.window_for_surface(toplevel.wl_surface())?;
        let (_, title) = toplevel_metadata(toplevel);
        let current = self
            .manager
            .window(window_id)
            .and_then(|record| record.title.clone());
        if current == title {
            return None;
        }
        let previous_focus = self.keyboard_focus();
        let actions = self.manager.on_title(window_id, title.clone());
        Some(TitleChange {
            window_id,
            title,
            decision: WmDecision {
                actions,
                previous_focus,
            },
        })
    }

    /// Handle an `app_id` that arrived after the map, or `None` when it did not change.
    ///
    /// `xdg_toplevel.set_app_id` is not double-buffered: smithay applies it while the
    /// request is dispatched and calls `XdgShellHandler::app_id_changed` right there, so a
    /// client may set the app id long after its first buffer commit. A genuine change is
    /// written back into the window model (see [`WmBridge::note_app_id`]), which is what
    /// `WindowInfo.app_id` / `QueryState` report.
    pub(crate) fn app_id_changed(&mut self, toplevel: &ToplevelSurface) -> Option<AppIdChange> {
        let window_id = self.window_for_surface(toplevel.wl_surface())?;
        let (app_id, title) = toplevel_metadata(toplevel);
        self.note_app_id(window_id, app_id, title)
    }

    /// The bookkeeping [`WmBridge::app_id_changed`] runs once it read the toplevel metadata.
    ///
    /// Split out from the metadata read (a `ToplevelSurface` can only be built by the
    /// xdg-shell protocol path, so it cannot be faked in a unit test): detect the change,
    /// write it into the window model, and refine the launch ledger.
    pub(crate) fn note_app_id(
        &mut self,
        window_id: WindowId,
        app_id: Option<AppId>,
        title: Option<String>,
    ) -> Option<AppIdChange> {
        if self.app_ids.get(&window_id).cloned().flatten() == app_id {
            return None;
        }
        self.app_ids.insert(window_id, app_id.clone());
        // Metadata-only write-back: `on_app_id` returns no actions, never re-configures and
        // never marks damage, so a late app id changes no pixels and emits no event.
        self.manager.on_app_id(window_id, app_id.clone());
        let launch_id = if self.launch_ids.contains_key(&window_id) {
            None
        } else {
            let pid = self.manager.window(window_id).and_then(|record| record.pid);
            let launch_id =
                self.launches
                    .correlate(app_id.as_ref(), pid, title.as_deref(), Instant::now());
            if let Some(launch_id) = launch_id {
                self.launches.take(launch_id);
                self.launch_ids.insert(window_id, launch_id);
            }
            launch_id
        };
        Some(AppIdChange {
            window_id,
            app_id,
            launch_id,
        })
    }

    /// Track a popup under its owner window.
    ///
    /// `offset` is the popup's window-relative origin (from the positioner geometry).
    pub(crate) fn popup_added(
        &mut self,
        popup: &PopupSurface,
        offset: (i32, i32),
    ) -> Option<PopupAdded> {
        let surface = popup.wl_surface().id();
        let parent = popup.get_parent_surface().map(|parent| parent.id());
        let window_id = parent
            .as_ref()
            .and_then(|parent| self.surfaces.window_for_surface(parent))
            // A popup whose parent is unknown cannot be attributed: never guess.
            .or_else(|| {
                tracing::debug!("popup without a known parent window is not tracked");
                None
            })?;
        let popup_id = self
            .surfaces
            .add_popup(surface.clone(), parent, window_id, offset);
        self.popup_handles.insert(surface, popup.clone());
        self.manager.on_popup_added(window_id);
        tracing::debug!(window_id = window_id.0, popup_id, "popup tracked");
        Some(PopupAdded {
            window_id,
            popup_id,
        })
    }

    /// Untrack a popup.
    pub(crate) fn popup_removed(&mut self, popup: &PopupSurface) -> Option<PopupRemoved> {
        let surface = popup.wl_surface().id();
        let record = self.surfaces.remove_popup(&surface)?;
        self.popup_handles.remove(&surface);
        if self.grab.as_ref().is_some_and(|grab| grab.popup == surface) {
            self.grab = None;
        }
        self.manager.on_popup_removed(record.window_id);
        tracing::debug!(
            window_id = record.window_id.0,
            popup_id = record.popup_id,
            "popup untracked"
        );
        Some(PopupRemoved {
            window_id: record.window_id,
            popup_id: record.popup_id,
        })
    }

    /// The window-relative origin of a popup, accumulated over its popup chain.
    pub(crate) fn popup_window_offset(&self, surface: &ObjectId) -> Option<(i32, i32)> {
        self.surfaces.popup_window_offset(surface)
    }

    /// Record the most recent popup grab (v1: recorded, not enforced by the seat).
    pub(crate) fn note_popup_grab(&mut self, popup: &PopupSurface) {
        let surface = popup.wl_surface().id();
        let Some(record) = self.surfaces.popup(&surface) else {
            tracing::debug!("popup grab for an untracked popup ignored");
            return;
        };
        let grab = PopupGrab {
            popup: surface,
            window_id: record.window_id,
            popup_id: record.popup_id,
        };
        tracing::debug!(
            window_id = grab.window_id.0,
            popup_id = grab.popup_id,
            "popup grab recorded"
        );
        self.grab = Some(grab);
    }

    /// The current grab, if any.
    pub(crate) fn popup_grab(&self) -> Option<&PopupGrab> {
        self.grab.as_ref()
    }

    /// Take the current grab and its surface, so the caller can send `popup_done`.
    pub(crate) fn take_popup_grab(&mut self) -> Option<(PopupGrab, PopupSurface)> {
        let grab = self.grab.take()?;
        let handle = self.popup_handles.get(&grab.popup).cloned();
        handle.map(|handle| (grab, handle))
    }

    /// Feed a commit into the model and return the new per-window commit counter.
    pub(crate) fn commit(&mut self, id: WindowId, damage: &Region) -> u64 {
        let commit_seq = self.last_commit_seq(id) + 1;
        self.manager.on_commit(id, commit_seq, damage);
        commit_seq
    }

    /// Activate a window directly (AGP `activate_window`).
    ///
    /// Unknown ids surface as [`CompositorError::UnknownWindow`]; an already active
    /// window yields `[WmAction::None]`, which emits nothing.
    pub(crate) fn activate(&mut self, id: WindowId) -> Result<WmDecision> {
        self.manager
            .require_window(id)
            .map_err(|_| CompositorError::UnknownWindow(id))?;
        let previous_focus = self.keyboard_focus();
        let actions = self.manager.activate(id);
        Ok(WmDecision {
            actions,
            previous_focus,
        })
    }
}

/// Read the client-set `app_id`/`title` of a toplevel.
fn toplevel_metadata(toplevel: &ToplevelSurface) -> (Option<AppId>, Option<String>) {
    with_states(toplevel.wl_surface(), |states| {
        let Some(data) = states.data_map.get::<XdgToplevelSurfaceData>() else {
            return (None, None);
        };
        let attributes = data.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            attributes.app_id.clone().map(AppId::from),
            attributes.title.clone(),
        )
    })
}

#[cfg(test)]
#[path = "wm_tests.rs"]
mod tests;
