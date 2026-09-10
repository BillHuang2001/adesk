use super::*;

fn bridge() -> WmBridge {
    WmBridge::new(Size::new(1280, 800))
}

#[test]
fn new_wires_the_output_size_into_the_policy_config() {
    let bridge = WmBridge::new(Size::new(1280, 800));
    assert_eq!(bridge.manager.config().output_size, Size::new(1280, 800));
    assert_eq!(bridge.tiled_rect(), Rect::new(0, 0, 1280, 800));
}

#[test]
fn keys_are_stable_unique_and_never_reused() {
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    let first = registry.register_toplevel(10);
    let second = registry.register_toplevel(20);
    assert_ne!(first, second);
    // Re-registering the same surface keeps its key.
    assert_eq!(registry.register_toplevel(10), first);
    // A key is never handed out twice, even after the surface is destroyed.
    registry.unbind_toplevel(&10);
    let third = registry.register_toplevel(30);
    assert_ne!(third, first);
    assert_ne!(third, second);
}

#[test]
fn duplicate_map_reuses_the_window_and_never_mints_a_second_id() {
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    registry.register_toplevel(1);
    assert!(registry.is_unmapped_toplevel(&1));
    registry.bind(1, WindowId(7));
    assert!(!registry.is_unmapped_toplevel(&1));
    assert_eq!(registry.window_for_surface(&1), Some(WindowId(7)));
    assert_eq!(
        registry.slot(&1).map(|slot| slot.window),
        Some(Some(WindowId(7)))
    );
}

#[test]
fn surface_lookup_covers_roots_subsurfaces_and_popups() {
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    registry.register_toplevel(1);
    registry.bind(1, WindowId(1));
    // A subsurface of the same tree is resolved to the same window.
    registry.owners.insert(2, WindowId(1));
    registry.add_popup(3, Some(1), WindowId(1), (4, 5));
    assert_eq!(registry.window_for_surface(&1), Some(WindowId(1)));
    assert_eq!(registry.window_for_surface(&2), Some(WindowId(1)));
    assert_eq!(registry.window_for_surface(&3), Some(WindowId(1)));
    assert_eq!(registry.window_for_surface(&99), None);
}

#[test]
fn popup_ids_are_monotonic_unique_and_never_reused() {
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    registry.register_toplevel(1);
    registry.bind(1, WindowId(1));
    let first = registry.add_popup(2, Some(1), WindowId(1), (0, 0));
    let second = registry.add_popup(3, Some(1), WindowId(1), (10, 20));
    assert_eq!((first, second), (1, 2));
    // Idempotent for a tracked popup surface.
    assert_eq!(registry.add_popup(2, Some(1), WindowId(1), (0, 0)), first);
    assert_eq!(
        registry.remove_popup(&2).map(|popup| popup.popup_id),
        Some(first)
    );
    let third = registry.add_popup(4, Some(1), WindowId(1), (0, 0));
    assert_eq!(third, 3);
}

#[test]
fn popup_offsets_accumulate_over_the_popup_chain() {
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    registry.register_toplevel(1);
    registry.bind(1, WindowId(1));
    registry.add_popup(2, Some(1), WindowId(1), (10, 20));
    registry.add_popup(3, Some(2), WindowId(1), (1, 2));
    assert_eq!(registry.popup_window_offset(&2), Some((10, 20)));
    assert_eq!(registry.popup_window_offset(&3), Some((11, 22)));
    assert_eq!(registry.popup_window_offset(&1), None);
}

#[test]
fn destroying_a_toplevel_returns_its_open_popup_ids() {
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    registry.register_toplevel(1);
    registry.bind(1, WindowId(1));
    registry.add_popup(2, Some(1), WindowId(1), (0, 0));
    registry.add_popup(3, Some(1), WindowId(1), (5, 5));
    let (window, popups) = registry.unbind_toplevel(&1);
    assert_eq!(window, Some(WindowId(1)));
    assert_eq!(popups, vec![1, 2]);
    assert_eq!(registry.window_for_surface(&2), None);
    assert!(registry.popup(&2).is_none());
    // An unmapped toplevel destroy reports no window and no popups.
    registry.register_toplevel(9);
    assert_eq!(registry.unbind_toplevel(&9), (None, Vec::new()));
}

#[test]
fn correlation_prefers_pid_then_app_id_then_title() {
    let mut ledger = LaunchLedger::default();
    let now = Instant::now();
    ledger.record(
        LaunchId(1),
        AppId::from("org.mozilla.firefox"),
        Some(42),
        now,
    );
    ledger.record(LaunchId(2), AppId::from("org.gnome.Nautilus"), None, now);

    // Exact pid wins, even when a title would match another launch.
    assert_eq!(
        ledger.correlate(
            Some(&AppId::from("org.gnome.Nautilus")),
            Some(42),
            Some("Nautilus"),
            now
        ),
        Some(LaunchId(1))
    );
    // No pid: app id (or its tail) wins over the title.
    assert_eq!(
        ledger.correlate(Some(&AppId::from("Nautilus")), None, Some("Firefox"), now),
        Some(LaunchId(2))
    );
    // Only a title matches.
    assert_eq!(
        ledger.correlate(None, None, Some("GitHub - Firefox Developer Edition"), now),
        Some(LaunchId(1))
    );
    // Nothing matches.
    assert_eq!(
        ledger.correlate(Some(&AppId::from("code")), None, Some("Files"), now),
        None
    );
}

#[test]
fn correlation_uses_the_most_recent_launch_as_tie_break() {
    let mut ledger = LaunchLedger::default();
    let now = Instant::now();
    ledger.record(LaunchId(1), AppId::from("org.mozilla.firefox"), None, now);
    ledger.record(LaunchId(2), AppId::from("org.mozilla.firefox"), None, now);
    assert_eq!(
        ledger.correlate(Some(&AppId::from("org.mozilla.firefox")), None, None, now),
        Some(LaunchId(2))
    );
    ledger.take(LaunchId(2));
    assert_eq!(
        ledger.correlate(Some(&AppId::from("org.mozilla.firefox")), None, None, now),
        Some(LaunchId(1))
    );
}

#[test]
fn correlation_expires_after_the_documented_window() {
    let mut ledger = LaunchLedger::default();
    let start = Instant::now();
    ledger.record(LaunchId(1), AppId::from("org.mozilla.firefox"), None, start);
    let later = start + LAUNCH_CORRELATION_TIMEOUT + Duration::from_millis(1);
    assert_eq!(
        ledger.correlate(Some(&AppId::from("org.mozilla.firefox")), None, None, later),
        None
    );
    ledger.expire(later);
    assert_eq!(ledger.len(), 0);
}

#[test]
fn pending_launches_are_bounded() {
    let mut ledger = LaunchLedger::default();
    let now = Instant::now();
    for id in 1..=(MAX_PENDING_LAUNCHES as u64 + 5) {
        ledger.record(LaunchId(id), AppId::from("app"), None, now);
    }
    assert_eq!(ledger.len(), MAX_PENDING_LAUNCHES);
    // The oldest entries were dropped, the newest is still there.
    assert_eq!(
        ledger.correlate(Some(&AppId::from("app")), None, None, now),
        Some(LaunchId(MAX_PENDING_LAUNCHES as u64 + 5))
    );
}

#[test]
fn title_evidence_ignores_too_short_tails() {
    let app = AppId::from("code");
    assert!(title_matches(&app, "main.rs - Visual Studio Code"));
    assert!(!title_matches(&app, "unrelated"));
    // A one-character tail must not match everything.
    assert!(!title_matches(&AppId::from("a"), "anything"));
}

#[test]
fn app_ids_match_by_tail_like_startup_wm_class() {
    assert!(app_ids_match(
        &AppId::from("org.mozilla.firefox"),
        &AppId::from("firefox")
    ));
    assert!(app_ids_match(
        &AppId::from("org.mozilla.Firefox"),
        &AppId::from("Firefox")
    ));
    assert!(!app_ids_match(
        &AppId::from("org.mozilla.firefox"),
        &AppId::from("thunderbird")
    ));
}

#[test]
fn unknown_windows_never_panic_and_report_unknown_window() {
    let mut bridge = bridge();
    assert_eq!(bridge.active_window(), None);
    assert_eq!(bridge.keyboard_focus(), None);
    assert!(bridge.windows().is_empty());
    assert!(bridge.toplevel_of(WindowId(1)).is_none());
    assert!(bridge.surface_of(WindowId(1)).is_none());
    assert_eq!(bridge.last_commit_seq(WindowId(1)), 0);
    let error = bridge
        .activate(WindowId(1))
        .expect_err("unknown window must fail");
    assert_eq!(error.code(), adesk_core::ErrorCode::UnknownWindow);
    let error = bridge
        .resolve_position(WindowId(1), &Position::normalized(0.5, 0.5))
        .expect_err("unknown window must fail");
    assert_eq!(error.code(), adesk_core::ErrorCode::Internal);
    assert!(bridge.commit(WindowId(1), &Region::empty()) >= 1);
}

#[test]
fn hot_path_lookups_read_a_single_record() {
    let mut bridge = bridge();
    // Both lookups answer `None` for an unknown id (the command paths turn that into
    // `unknown_window`); neither projects a `WindowInfo`.
    assert_eq!(bridge.window_geometry(WindowId(1)), None);
    assert_eq!(bridge.root_id(WindowId(1)), None);

    let (id, _) = bridge.manager.on_map(MapRequest {
        surface_key: SurfaceKey::new(1),
        app_id: Some(AppId::from("org.example.geometry")),
        pid: None,
        title: Some("Geometry".to_string()),
        created_seq: 1,
    });
    // The geometry is exactly the tiled rect the record carries.
    assert_eq!(bridge.window_geometry(id), Some(Rect::new(0, 0, 1280, 800)));
    assert_eq!(bridge.window_geometry(WindowId(999)), None);

    // `root_id` is the window's entry in the root-surface map, compared as an
    // `ObjectId` so the commit path clones no `WlSurface`. A real root id is the
    // toplevel surface only the xdg-shell path can register (it needs a live
    // `Display`), so this exercises the map directly.
    let root = ObjectId::null();
    bridge.roots.insert(id, root.clone());
    assert_eq!(bridge.root_id(id), Some(root));
    assert_eq!(bridge.root_id(WindowId(999)), None);
}

#[test]
fn grab_bookkeeping_is_cleared_with_the_popup() {
    let mut bridge = bridge();
    // No grab at all.
    assert!(bridge.popup_grab().is_none());
    assert!(bridge.take_popup_grab().is_none());
    // Popup ids are looked up through the pure registry.
    let mut registry: SurfaceRegistry<u64> = SurfaceRegistry::new();
    registry.register_toplevel(1);
    registry.bind(1, WindowId(1));
    let popup_id = registry.add_popup(2, Some(1), WindowId(1), (0, 0));
    assert_eq!(
        registry.popup(&2).map(|popup| popup.popup_id),
        Some(popup_id)
    );
}

#[test]
fn a_late_app_id_is_written_back_into_the_window_model() {
    let mut bridge = bridge();
    // `WmBridge::app_id_changed` reads its metadata from a real `ToplevelSurface`, which only
    // the xdg-shell protocol path can build (no public constructor), so the `u64` surface
    // stand-ins above cannot express a late `set_app_id`. This test therefore drives the
    // bridge-owned bookkeeping that `app_id_changed` delegates to after reading the metadata
    // (`WmBridge::note_app_id`), against a window mapped the way a map fills the model in; the
    // protocol path itself is proven by `tests/window_lifecycle.rs`.
    let (id, _) = bridge.manager.on_map(MapRequest {
        surface_key: SurfaceKey::new(1),
        app_id: Some(AppId::from("org.example.appid.map")),
        pid: None,
        title: Some("App id".to_string()),
        created_seq: 1,
    });
    assert_eq!(
        bridge
            .manager
            .window(id)
            .and_then(|record| record.app_id.clone()),
        Some(AppId::from("org.example.appid.map")),
        "the map-time app id is what the model starts with"
    );

    // A changed app id is reported once and reaches the model.
    let late = AppId::from("org.example.appid.late");
    let change = bridge
        .note_app_id(id, Some(late.clone()), Some("App id".to_string()))
        .expect("a different app id is a change");
    assert_eq!(change.window_id, id);
    assert_eq!(change.app_id.as_ref(), Some(&late));
    assert_eq!(change.launch_id, None, "no launch was pending");
    assert_eq!(
        bridge
            .manager
            .window(id)
            .and_then(|record| record.app_id.clone()),
        Some(late.clone()),
        "the changed app id is written back into the window model"
    );
    assert_eq!(
        bridge.manager.window_info(id).and_then(|info| info.app_id),
        Some(late.clone()),
        "QueryState/list_windows report the late value"
    );

    // The same value again is not a change, and `None` clears the model value.
    assert!(bridge.note_app_id(id, Some(late.clone()), None).is_none());
    assert!(bridge.note_app_id(id, None, None).is_some());
    assert_eq!(
        bridge
            .manager
            .window(id)
            .and_then(|record| record.app_id.clone()),
        None
    );
    assert_eq!(
        bridge.manager.window_info(id).and_then(|info| info.app_id),
        None
    );
}
