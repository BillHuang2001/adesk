//! Policy matrix driven through the public `adesk-wm` API only.
//!
//! The unit tests in `src/policy_tests.rs` drive the policy functions directly;
//! this integration test is the only place that exercises the `WindowManager`
//! façade in `src/manager.rs`. It uses exactly the calls `adesk-compositor`
//! makes, so the façade-to-policy wiring and the documented return shapes are
//! verified without re-checking policy semantics the unit tests already cover.

use adesk_core::{AppId, Position, Rect, Region, Size, WindowId, WindowState};
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

    // `map` asserts the map-time action shape; ids come back monotonic.
    let first = map(&mut wm, 1, "One");
    let second = map(&mut wm, 2, "Two");
    assert_eq!((first, second), (WindowId(1), WindowId(2)));
    assert_eq!(wm.active_window(), Some(second));
    assert_consistent(&wm);

    // A duplicate map of a tracked surface key is an evaluated no-op.
    let (again, actions) = wm.on_map(MapRequest::new(SurfaceKey::new(2)));
    assert_eq!(again, second);
    assert_eq!(actions, vec![WmAction::None]);
    assert_consistent(&wm);

    // Activating the already-active window is an explicit no-op; activating
    // another window switches visibility.
    assert_eq!(wm.activate(second), vec![WmAction::None]);
    assert_eq!(wm.activate(first), vec![WmAction::Activate { id: first }]);
    assert_eq!(wm.active_window(), Some(first));
    assert_consistent(&wm);

    // Unknown ids never panic on any façade method: empty action list, `None`
    // lookups, structured `unknown_window` error.
    let unknown = WindowId(404);
    assert_eq!(wm.activate(unknown), Vec::new());
    assert_eq!(wm.on_destroy(unknown), Vec::new());
    assert_eq!(wm.on_title(unknown, None), Vec::new());
    assert_eq!(wm.on_app_id(unknown, None), Vec::new());
    assert_eq!(wm.on_commit(unknown, 1, &Region::empty()), Vec::new());
    assert_eq!(wm.on_popup_added(unknown), Vec::new());
    assert_eq!(wm.on_popup_removed(unknown), Vec::new());
    assert!(wm.window(unknown).is_none());
    assert!(wm.window_info(unknown).is_none());
    assert_eq!(
        wm.require_window(unknown).unwrap_err(),
        Error::UnknownWindow(unknown)
    );
    assert_eq!(wm.resolve_position(unknown, Position::pixels(1, 1)), None);
    assert_consistent(&wm);

    // `set_output_size` re-tiles every mapped window and moves the tiled rect.
    let actions = wm.set_output_size(Size::new(800, 600));
    assert_eq!(actions.len(), 2);
    assert!(actions
        .iter()
        .all(|action| matches!(action, WmAction::ConfigureWindow { .. })));
    assert_eq!(wm.tiled_rect(), Rect::new(0, 0, 800, 600));
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
fn late_app_id_change_flows_into_window_info_and_list_windows() {
    let mut wm = manager();
    let id = map(&mut wm, 1, "Konsole");

    // `on_app_id` is metadata-only: no actions, and the late value surfaces
    // through both `window_info` and `windows()`.
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

    // `None` clears it (the compositor maps an empty app id to `None`).
    assert!(wm.on_app_id(id, None).is_empty());
    assert_eq!(wm.window_info(id).expect("info").app_id, None);

    // An unknown id is a no-op.
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

    assert_ne!(remapped, first);
    assert_consistent(&wm);
}
