//! Policy matrix driven through the public `adesk-wm` API only.
//!
//! This complements the inline unit tests in `src/policy.rs`: it uses exactly
//! the calls `adesk-compositor` makes, so the compositor contract is verified
//! without reaching into crate internals.

use adesk_core::{AppId, Point, Position, Rect, Region, Size, WindowId, WindowState};
use adesk_wm::{Error, MapRequest, PolicyConfig, SurfaceKey, WindowManager, WmAction};

fn manager() -> WindowManager {
    WindowManager::new(PolicyConfig::new(Size::new(1280, 800)))
}

/// The invariants that must hold after any operation sequence: at most one
/// active record matching `active_window`, no id `0`, every tracked window
/// mapped and tiled to `tiled_rect`.
fn assert_consistent(wm: &WindowManager) {
    let active: Vec<WindowId> = wm
        .windows()
        .iter()
        .filter(|record| record.state == WindowState::Active)
        .map(|record| record.id)
        .collect();
    assert!(active.len() <= 1, "at most one active window: {active:?}");
    assert_eq!(active.first().copied(), wm.active_window());
    for record in wm.windows() {
        assert_ne!(record.id, WindowId(0));
        assert!(record.mapped);
        assert_eq!(record.geometry, wm.tiled_rect());
    }
}

/// Maps a window and asserts the map-time contract (`ConfigureWindow` then
/// `Activate`, both against the tiled rect).
fn map(wm: &mut WindowManager, key: u64, title: &str) -> WindowId {
    let (id, actions) = wm.on_map(MapRequest {
        surface_key: SurfaceKey::new(key),
        app_id: Some(AppId::from("org.example.app")),
        pid: Some(1000 + key as i32),
        title: Some(title.into()),
        created_seq: key * 10,
    });
    assert_eq!(
        actions,
        vec![
            WmAction::ConfigureWindow {
                id,
                rect: wm.tiled_rect(),
            },
            WmAction::Activate { id },
        ]
    );
    id
}

#[test]
fn full_lifecycle_through_the_public_api() {
    let mut wm = manager();
    assert_eq!(wm.tiled_rect(), Rect::new(0, 0, 1280, 800));
    assert_eq!(wm.active_window(), None);
    assert!(wm.windows().is_empty());
    assert_consistent(&wm);

    // Map two windows: ids are monotonic, the new one is active and the
    // previous one stays mapped but inactive.
    let first = map(&mut wm, 1, "One");
    let second = map(&mut wm, 2, "Two");
    assert_eq!((first, second), (WindowId(1), WindowId(2)));
    assert_eq!(wm.active_window(), Some(second));
    assert_eq!(wm.windows().len(), 2);
    assert_eq!(wm.windows()[0].id, first);
    assert_eq!(wm.windows()[1].id, second);
    assert_eq!(
        wm.window(first).expect("first").state,
        WindowState::Inactive
    );
    assert!(wm.window(first).expect("first").mapped);
    assert_consistent(&wm);

    // Duplicate map of a tracked surface key is an evaluated no-op.
    let (again, actions) = wm.on_map(MapRequest::new(SurfaceKey::new(2)));
    assert_eq!(again, second);
    assert_eq!(actions, vec![WmAction::None]);
    assert_eq!(wm.windows().len(), 2);
    assert_consistent(&wm);

    // Activating the already-active window is an explicit no-op; activating
    // another window switches visibility.
    assert_eq!(wm.activate(second), vec![WmAction::None]);
    assert_eq!(wm.activate(first), vec![WmAction::Activate { id: first }]);
    assert_eq!(wm.active_window(), Some(first));
    assert_eq!(
        wm.window(second).expect("second").state,
        WindowState::Inactive
    );
    assert_consistent(&wm);

    // Unknown ids never panic: empty action list / structured error.
    assert_eq!(wm.activate(WindowId(404)), Vec::new());
    assert_eq!(wm.on_destroy(WindowId(404)), Vec::new());
    assert!(wm.window(WindowId(404)).is_none());
    assert!(wm.window_info(WindowId(404)).is_none());
    assert_eq!(
        wm.require_window(WindowId(404)).unwrap_err(),
        Error::UnknownWindow(WindowId(404))
    );
    assert_eq!(
        wm.resolve_position(WindowId(404), Position::pixels(1, 1)),
        None
    );

    // Title / commit / popup events flow into the read-model.
    assert!(wm.on_title(first, Some("One (edited)".into())).is_empty());
    assert!(wm.on_commit(first, 11, &Region::empty()).is_empty());
    assert!(wm.on_popup_added(first).is_empty());
    assert!(wm.on_popup_added(first).is_empty());
    assert!(wm.on_popup_removed(first).is_empty());
    let info = wm.window_info(first).expect("info");
    assert_eq!(info.id, first);
    assert_eq!(info.app_id, Some(AppId::from("org.example.app")));
    assert_eq!(info.title.as_deref(), Some("One (edited)"));
    assert_eq!(info.geometry, wm.tiled_rect());
    assert_eq!(info.state, WindowState::Active);
    assert!(info.mapped);
    assert_eq!(info.pid, Some(1001));
    assert_eq!(info.created_seq, 10);
    assert_eq!(info.last_commit_seq, 11);
    assert_eq!(info.popup_count, 1);

    // Window-relative input resolves through the window geometry.
    assert_eq!(
        wm.resolve_position(first, Position::normalized(0.0, 0.0)),
        Some(Point::new(0, 0))
    );
    assert_eq!(
        wm.resolve_position(first, Position::normalized(1.0, 1.0)),
        Some(Point::new(1279, 799))
    );
    assert_eq!(
        wm.resolve_position(first, Position::pixels(-1, -1)),
        Some(Point::new(0, 0))
    );

    // Resize re-tiles every mapped window in creation order.
    let actions = wm.set_output_size(Size::new(800, 600));
    assert_eq!(
        actions,
        vec![
            WmAction::ConfigureWindow {
                id: first,
                rect: Rect::new(0, 0, 800, 600),
            },
            WmAction::ConfigureWindow {
                id: second,
                rect: Rect::new(0, 0, 800, 600),
            },
        ]
    );
    assert_eq!(wm.tiled_rect(), Rect::new(0, 0, 800, 600));
    assert_eq!(wm.config().output_size, Size::new(800, 600));
    assert_consistent(&wm);

    // Destroying the active window activates the MRU fallback.
    assert_eq!(
        wm.on_destroy(first),
        vec![WmAction::ActivatePrevious { id: second }]
    );
    assert_eq!(wm.active_window(), Some(second));
    assert_consistent(&wm);

    // Destroying the last window leaves no active window.
    assert!(wm.on_destroy(second).is_empty());
    assert_eq!(wm.active_window(), None);
    assert!(wm.windows().is_empty());
    assert_consistent(&wm);
}

#[test]
fn mru_fallback_prefers_the_most_recently_used_window() {
    let mut wm = manager();
    let ids: Vec<WindowId> = (1..=3).map(|key| map(&mut wm, key, "w")).collect();

    // MRU after mapping 1, 2, 3 is [3, 2, 1].
    assert_eq!(wm.activate(ids[0]), vec![WmAction::Activate { id: ids[0] }]);
    assert_eq!(
        wm.on_destroy(ids[0]),
        vec![WmAction::ActivatePrevious { id: ids[2] }]
    );

    // MRU is now [3, 2]; activating 2 makes it [2, 3].
    assert_eq!(wm.activate(ids[1]), vec![WmAction::Activate { id: ids[1] }]);
    assert_eq!(
        wm.on_destroy(ids[1]),
        vec![WmAction::ActivatePrevious { id: ids[2] }]
    );
    assert_eq!(wm.active_window(), Some(ids[2]));
    assert_consistent(&wm);
}

#[test]
fn late_app_id_change_flows_into_window_info_and_list_windows() {
    let mut wm = manager();
    let id = map(&mut wm, 1, "Konsole");
    assert_eq!(
        wm.window_info(id).expect("info").app_id,
        Some(AppId::from("org.example.app"))
    );

    // A client may set `xdg_toplevel.app_id` after its first buffer commit, so
    // the compositor applies the late value with `on_app_id`; it is metadata
    // only, hence no actions and no configure/damage.
    assert!(wm
        .on_app_id(id, Some(AppId::from("org.kde.konsole")))
        .is_empty());
    assert_eq!(
        wm.window_info(id).expect("info").app_id,
        Some(AppId::from("org.kde.konsole"))
    );
    assert_eq!(
        wm.windows()[0].info().app_id,
        Some(AppId::from("org.kde.konsole"))
    );
    assert_consistent(&wm);

    // `None` clears it (the compositor maps an empty app id to `None`).
    assert!(wm.on_app_id(id, None).is_empty());
    assert_eq!(wm.window_info(id).expect("info").app_id, None);
    assert_eq!(wm.windows()[0].info().app_id, None);
    assert_consistent(&wm);

    // Unknown ids are ignored, never a panic.
    assert!(wm
        .on_app_id(WindowId(404), Some(AppId::from("org.kde.konsole")))
        .is_empty());
    assert_eq!(wm.windows().len(), 1);
    assert_consistent(&wm);
}

#[test]
fn destroyed_surface_key_gets_a_fresh_id_when_remapped() {
    let mut wm = manager();
    let first = map(&mut wm, 1, "One");
    assert!(wm.on_destroy(first).is_empty());

    let remapped = map(&mut wm, 1, "One again");

    assert_eq!(remapped, WindowId(2));
    assert_ne!(remapped, first);
    assert_eq!(wm.window_by_surface(SurfaceKey::new(1)), Some(remapped));
    assert_consistent(&wm);
}
