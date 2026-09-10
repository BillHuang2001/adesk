//! Unit tests for the window policy ([`crate::policy`]).
//!
//! Kept next to `policy.rs` rather than inline so the policy file stays focused
//! on the decisions. The module is crate-internal and only built for tests, so
//! it may construct [`WindowModel`] directly (that is how non-origin geometry
//! and saturated counters are exercised).

use std::collections::BTreeSet;

use adesk_core::{AppId, Point, Position, Rect, Region, Size, WindowId, WindowState};

use crate::action::WmAction;
use crate::config::PolicyConfig;
use crate::error::Error;
use crate::model::{MapRequest, SurfaceKey, WindowModel, WindowRecord};
use crate::policy::{
    activate, active_window, on_app_id, on_commit, on_destroy, on_map, on_output_size,
    on_popup_added, on_popup_removed, on_title, require_window, resolve_position, window,
    window_by_surface, window_info, windows,
};

fn default_config() -> PolicyConfig {
    PolicyConfig::new(Size::new(1280, 800))
}

/// Maps a window with surface key `raw` and returns its id.
fn map(model: &mut WindowModel, config: &PolicyConfig, raw: u64) -> WindowId {
    on_map(model, config, MapRequest::new(SurfaceKey::new(raw))).0
}

/// Maps `count` windows with surface keys `1..=count`.
fn map_windows(model: &mut WindowModel, config: &PolicyConfig, count: u64) -> Vec<WindowId> {
    (1..=count).map(|raw| map(model, config, raw)).collect()
}

fn state_of(model: &WindowModel, id: WindowId) -> WindowState {
    window(model, id).expect("tracked window").state
}

fn commit_seq(model: &WindowModel, id: WindowId) -> u64 {
    window(model, id).expect("tracked window").last_commit_seq
}

fn popup_count(model: &WindowModel, id: WindowId) -> u32 {
    window(model, id).expect("tracked window").popup_count
}

/// A model with one window at an arbitrary geometry, to exercise
/// non-origin windows the v1 policy never produces.
fn model_with_geometry(id: WindowId, geometry: Rect) -> WindowModel {
    let mut model = WindowModel::new();
    model.next_id = id.0 + 1;
    model.records.push(WindowRecord {
        id,
        surface_key: SurfaceKey::new(id.0),
        app_id: None,
        pid: None,
        title: None,
        geometry,
        state: WindowState::Active,
        mapped: true,
        created_seq: 0,
        last_commit_seq: 0,
        popup_count: 0,
    });
    model.mru.push(id);
    model
}

/// The cross-cutting invariants: at most one active record matching
/// `active_window`, no stale MRU ids, policy-owned geometry, no id 0.
fn assert_invariants(model: &WindowModel, config: &PolicyConfig) {
    let active: Vec<WindowId> = model
        .records
        .iter()
        .filter(|record| record.state == WindowState::Active)
        .map(|record| record.id)
        .collect();
    assert!(active.len() <= 1, "at most one active window: {active:?}");
    assert_eq!(active.first().copied(), active_window(model));
    assert_eq!(model.mru.len(), model.records.len(), "no stale MRU ids");
    let mru: BTreeSet<WindowId> = model.mru.iter().copied().collect();
    let records: BTreeSet<WindowId> = model.records.iter().map(|record| record.id).collect();
    assert_eq!(mru, records, "MRU ids == record ids");
    for record in &model.records {
        assert_ne!(record.id, WindowId(0), "id 0 is never assigned");
        assert!(record.mapped, "tracked windows stay mapped");
        assert_eq!(
            record.geometry,
            config.tiled_rect(),
            "geometry is policy-owned"
        );
    }
}

#[test]
fn map_assigns_id_records_metadata_and_returns_actions_in_order() {
    let config = default_config();
    let mut model = WindowModel::new();
    let request = MapRequest {
        surface_key: SurfaceKey::new(7),
        app_id: Some(AppId::from("org.mozilla.firefox")),
        pid: Some(4242),
        title: Some("GitHub".into()),
        created_seq: 41,
    };

    let (id, actions) = on_map(&mut model, &config, request);

    assert_eq!(id, WindowId(1));
    assert_eq!(
        actions,
        vec![
            WmAction::ConfigureWindow {
                id,
                rect: Rect::new(0, 0, 1280, 800),
            },
            WmAction::Activate { id },
        ]
    );
    let record = window(&model, id).expect("record");
    assert_eq!(record.id, id);
    assert_eq!(record.surface_key, SurfaceKey::new(7));
    assert_eq!(record.app_id, Some(AppId::from("org.mozilla.firefox")));
    assert_eq!(record.pid, Some(4242));
    assert_eq!(record.title.as_deref(), Some("GitHub"));
    assert_eq!(record.geometry, config.tiled_rect());
    assert_eq!(record.state, WindowState::Active);
    assert!(record.mapped);
    assert_eq!(record.created_seq, 41);
    assert_eq!(record.last_commit_seq, 0);
    assert_eq!(record.popup_count, 0);
    assert_eq!(model.next_id, 2);
    assert_invariants(&model, &config);
}

#[test]
fn map_deactivates_the_previous_window_but_keeps_it_mapped() {
    let config = default_config();
    let mut model = WindowModel::new();
    let first = map(&mut model, &config, 1);
    let second = map(&mut model, &config, 2);

    assert_eq!(active_window(&model), Some(second));
    assert_eq!(state_of(&model, second), WindowState::Active);
    assert_eq!(state_of(&model, first), WindowState::Inactive);
    let record = window(&model, first).expect("record");
    assert!(record.mapped);
    assert_eq!(record.geometry, config.tiled_rect());
    assert_eq!(model.mru, vec![second, first]);
    assert_invariants(&model, &config);
}

#[test]
fn ids_are_monotonic_and_never_reused_across_destroy_and_remap() {
    let config = default_config();
    let mut model = WindowModel::new();
    let first = map(&mut model, &config, 1);
    let second = map(&mut model, &config, 2);
    assert_eq!((first, second), (WindowId(1), WindowId(2)));

    on_destroy(&mut model, second);
    let remapped = map(&mut model, &config, 2);

    assert_eq!(remapped, WindowId(3));
    assert_ne!(remapped, second);
    assert_eq!(model.next_id, 4);
    assert_eq!(windows(&model).len(), 2);
    assert_invariants(&model, &config);
}

#[test]
fn duplicate_surface_key_reuses_the_id_and_changes_nothing() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 5);
    on_title(&mut model, id, Some("first".into()));
    let records_before = model.records.clone();
    let mru_before = model.mru.clone();

    let (again, actions) = on_map(&mut model, &config, MapRequest::new(SurfaceKey::new(5)));

    assert_eq!(again, id);
    assert_eq!(actions, vec![WmAction::None]);
    assert_eq!(model.records, records_before);
    assert_eq!(model.mru, mru_before);
    assert_eq!(model.next_id, 2);
    assert_invariants(&model, &config);
}

#[test]
fn destroying_the_active_window_activates_the_mru_fallback() {
    let config = default_config();
    let mut model = WindowModel::new();
    let ids = map_windows(&mut model, &config, 3);
    assert_eq!(model.mru, vec![ids[2], ids[1], ids[0]]);
    activate(&mut model, ids[0]);

    let actions = on_destroy(&mut model, ids[0]);

    assert_eq!(actions, vec![WmAction::ActivatePrevious { id: ids[2] }]);
    assert_eq!(active_window(&model), Some(ids[2]));
    assert_eq!(state_of(&model, ids[2]), WindowState::Active);
    assert_eq!(state_of(&model, ids[1]), WindowState::Inactive);
    assert_eq!(windows(&model).len(), 2);
    assert_invariants(&model, &config);
}

#[test]
fn destroying_an_inactive_window_leaves_the_active_window_alone() {
    let config = default_config();
    let mut model = WindowModel::new();
    let ids = map_windows(&mut model, &config, 2);

    assert_eq!(on_destroy(&mut model, ids[0]), Vec::new());
    assert_eq!(active_window(&model), Some(ids[1]));
    assert_eq!(windows(&model).len(), 1);
    assert_invariants(&model, &config);
}

#[test]
fn destroying_the_last_window_leaves_no_active_window() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(on_destroy(&mut model, id), Vec::new());
    assert_eq!(active_window(&model), None);
    assert!(windows(&model).is_empty());
    assert!(model.mru.is_empty());
    assert_invariants(&model, &config);
}

#[test]
fn activate_switches_windows_and_reorders_mru() {
    let config = default_config();
    let mut model = WindowModel::new();
    let ids = map_windows(&mut model, &config, 3);
    assert_eq!(model.mru, vec![ids[2], ids[1], ids[0]]);

    assert_eq!(
        activate(&mut model, ids[0]),
        vec![WmAction::Activate { id: ids[0] }]
    );
    assert_eq!(model.mru, vec![ids[0], ids[2], ids[1]]);
    assert_eq!(state_of(&model, ids[0]), WindowState::Active);
    assert_eq!(state_of(&model, ids[1]), WindowState::Inactive);
    assert_eq!(state_of(&model, ids[2]), WindowState::Inactive);
    assert!(window(&model, ids[1]).expect("record").mapped);

    assert_eq!(
        activate(&mut model, ids[1]),
        vec![WmAction::Activate { id: ids[1] }]
    );
    assert_eq!(model.mru, vec![ids[1], ids[0], ids[2]]);
    assert_invariants(&model, &config);
}

#[test]
fn activate_on_the_active_window_returns_an_explicit_no_op() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(activate(&mut model, id), vec![WmAction::None]);
    assert_eq!(model.mru, vec![id]);
    assert_eq!(state_of(&model, id), WindowState::Active);
    assert_invariants(&model, &config);
}

#[test]
fn title_changes_update_the_record_and_return_no_actions() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(on_title(&mut model, id, Some("GitHub".into())), Vec::new());
    assert_eq!(
        window(&model, id).expect("record").title.as_deref(),
        Some("GitHub")
    );
    assert_eq!(on_title(&mut model, id, None), Vec::new());
    assert_eq!(window(&model, id).expect("record").title, None);
    assert_invariants(&model, &config);
}

#[test]
fn app_id_changes_update_the_record_and_return_no_actions() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);
    assert_eq!(window_info(&model, id).expect("info").app_id, None);

    assert_eq!(
        on_app_id(&mut model, id, Some(AppId::from("org.kde.konsole"))),
        Vec::new()
    );
    assert_eq!(
        window(&model, id).expect("record").app_id,
        Some(AppId::from("org.kde.konsole"))
    );
    assert_eq!(
        window_info(&model, id).expect("info").app_id,
        Some(AppId::from("org.kde.konsole"))
    );
    assert_eq!(
        windows(&model)[0].info().app_id,
        Some(AppId::from("org.kde.konsole"))
    );

    // A second change replaces the previous value.
    assert_eq!(
        on_app_id(&mut model, id, Some(AppId::from("org.gnome.Nautilus"))),
        Vec::new()
    );
    assert_eq!(
        window_info(&model, id).expect("info").app_id,
        Some(AppId::from("org.gnome.Nautilus"))
    );

    // `None` clears the app id again.
    assert_eq!(on_app_id(&mut model, id, None), Vec::new());
    assert_eq!(window(&model, id).expect("record").app_id, None);
    assert_eq!(window_info(&model, id).expect("info").app_id, None);
    assert_invariants(&model, &config);
}

#[test]
fn app_id_changes_have_no_policy_side_effects() {
    let config = default_config();
    let mut model = WindowModel::new();
    let ids = map_windows(&mut model, &config, 2);
    on_commit(&mut model, ids[0], 7, &Region::empty());
    on_popup_added(&mut model, ids[0]);
    let geometry = window(&model, ids[0]).expect("record").geometry;
    let mru_before = model.mru.clone();

    assert_eq!(
        on_app_id(&mut model, ids[0], Some(AppId::from("org.kde.konsole"))),
        Vec::new()
    );

    let record = window(&model, ids[0]).expect("record");
    assert_eq!(record.geometry, geometry);
    assert_eq!(
        record.state,
        WindowState::Inactive,
        "app id changes never touch visibility"
    );
    assert_eq!(record.last_commit_seq, 7);
    assert_eq!(record.popup_count, 1);
    assert_eq!(active_window(&model), Some(ids[1]));
    assert_eq!(model.mru, mru_before);
    assert_invariants(&model, &config);
}

#[test]
fn app_id_change_on_an_unknown_id_is_ignored() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);
    let records_before = model.records.clone();
    let mru_before = model.mru.clone();

    assert_eq!(
        on_app_id(
            &mut model,
            WindowId(99),
            Some(AppId::from("org.kde.konsole"))
        ),
        Vec::new()
    );

    assert_eq!(model.records, records_before);
    assert_eq!(model.mru, mru_before);
    assert_eq!(active_window(&model), Some(id));
    assert_invariants(&model, &config);
}

#[test]
fn commit_watermark_is_monotonic_and_never_regresses() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);
    let damage = Region::empty();

    assert_eq!(on_commit(&mut model, id, 5, &damage), Vec::new());
    assert_eq!(commit_seq(&model, id), 5);
    on_commit(&mut model, id, 3, &damage);
    assert_eq!(
        commit_seq(&model, id),
        5,
        "reordered commit must not regress"
    );
    on_commit(&mut model, id, 9, &damage);
    on_commit(&mut model, id, 9, &damage);
    assert_eq!(commit_seq(&model, id), 9);
    assert_invariants(&model, &config);
}

#[test]
fn popup_count_saturates_in_both_directions() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(on_popup_removed(&mut model, id), Vec::new());
    assert_eq!(popup_count(&model, id), 0, "removal below zero is ignored");

    for _ in 0..3 {
        assert_eq!(on_popup_added(&mut model, id), Vec::new());
    }
    assert_eq!(popup_count(&model, id), 3);
    for _ in 0..5 {
        on_popup_removed(&mut model, id);
    }
    assert_eq!(popup_count(&model, id), 0);

    let record = model
        .records
        .iter_mut()
        .find(|record| record.id == id)
        .expect("record");
    record.popup_count = u32::MAX;
    on_popup_added(&mut model, id);
    assert_eq!(popup_count(&model, id), u32::MAX);
    assert_invariants(&model, &config);
}

#[test]
fn every_event_path_tolerates_unknown_ids() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);
    let unknown = WindowId(99);

    assert_eq!(on_title(&mut model, unknown, Some("x".into())), Vec::new());
    assert_eq!(
        on_commit(&mut model, unknown, 7, &Region::empty()),
        Vec::new()
    );
    assert_eq!(on_popup_added(&mut model, unknown), Vec::new());
    assert_eq!(on_popup_removed(&mut model, unknown), Vec::new());
    assert_eq!(activate(&mut model, unknown), Vec::new());
    assert_eq!(on_destroy(&mut model, unknown), Vec::new());
    assert!(window(&model, unknown).is_none());
    assert!(window_info(&model, unknown).is_none());
    assert!(window_by_surface(&model, SurfaceKey::new(99)).is_none());
    assert!(resolve_position(&model, unknown, Position::pixels(0, 0)).is_none());
    assert_eq!(
        require_window(&model, unknown),
        Err(Error::UnknownWindow(unknown))
    );
    assert_eq!(windows(&model).len(), 1);
    assert_eq!(active_window(&model), Some(id));
    assert_invariants(&model, &config);
}

#[test]
fn windows_are_in_creation_order_and_surface_keys_round_trip() {
    let config = default_config();
    let mut model = WindowModel::new();
    let ids = map_windows(&mut model, &config, 3);

    assert_eq!(
        windows(&model)
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        ids
    );
    for (index, id) in ids.iter().enumerate() {
        let key = SurfaceKey::new(index as u64 + 1);
        assert_eq!(window_by_surface(&model, key), Some(*id));
        assert_eq!(window(&model, *id).expect("record").surface_key, key);
    }
    assert_eq!(window_by_surface(&model, SurfaceKey::new(99)), None);
}

#[test]
fn window_and_require_window_agree_on_tracked_ids() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(window(&model, id).map(|record| record.id), Some(id));
    assert_eq!(require_window(&model, id).expect("tracked").id, id);
    assert!(window(&model, WindowId(2)).is_none());
    assert_eq!(
        require_window(&model, WindowId(2)).unwrap_err(),
        Error::UnknownWindow(WindowId(2))
    );
}

#[test]
fn resolve_position_matches_the_documented_non_zero_origin_examples() {
    let id = WindowId(4);
    let model = model_with_geometry(id, Rect::new(100, 50, 100, 50));

    assert_eq!(
        resolve_position(&model, id, Position::normalized(0.0, 0.0)),
        Some(Point::new(100, 50))
    );
    assert_eq!(
        resolve_position(&model, id, Position::normalized(1.0, 1.0)),
        Some(Point::new(199, 99))
    );
    assert_eq!(
        resolve_position(&model, id, Position::pixels(10, 10)),
        Some(Point::new(110, 60))
    );
    assert_eq!(
        resolve_position(&model, id, Position::pixels(-5, -5)),
        Some(Point::new(100, 50))
    );
    assert_eq!(
        resolve_position(&model, id, Position::pixels(500, 500)),
        Some(Point::new(199, 99))
    );

    // A second, differently shaped window proves the translation is driven by
    // the geometry origin, not tied to one offset.
    let id = WindowId(1);
    let model = model_with_geometry(id, Rect::new(10, 20, 30, 40));

    assert_eq!(
        resolve_position(&model, id, Position::pixels(5, 5)),
        Some(Point::new(15, 25))
    );
    assert_eq!(
        resolve_position(&model, id, Position::normalized(1.0, 1.0)),
        Some(Point::new(39, 59))
    );
    assert_eq!(
        resolve_position(&model, id, Position::pixels(1000, 1000)),
        Some(Point::new(39, 59))
    );
}

#[test]
fn resolve_position_at_the_output_origin_clamps_and_normalizes() {
    let config = default_config();
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(
        resolve_position(&model, id, Position::normalized(0.0, 0.0)),
        Some(Point::new(0, 0))
    );
    assert_eq!(
        resolve_position(&model, id, Position::normalized(1.0, 1.0)),
        Some(Point::new(1279, 799))
    );
    assert_eq!(
        resolve_position(&model, id, Position::normalized(0.5, 0.5)),
        Some(Point::new(640, 400))
    );
    assert_eq!(
        resolve_position(&model, id, Position::pixels(-10, -10)),
        Some(Point::new(0, 0))
    );
    assert_eq!(
        resolve_position(&model, id, Position::pixels(5000, 5000)),
        Some(Point::new(1279, 799))
    );
    assert_eq!(
        resolve_position(&model, id, Position::normalized(f64::NAN, f64::NAN)),
        Some(Point::new(0, 0))
    );
    assert_eq!(
        resolve_position(
            &model,
            id,
            Position::normalized(f64::INFINITY, f64::NEG_INFINITY)
        ),
        Some(Point::new(1279, 0))
    );
}

#[test]
fn resolve_position_on_an_empty_window_returns_its_origin() {
    let config = PolicyConfig::new(Size::ZERO);
    let mut model = WindowModel::new();
    let id = map(&mut model, &config, 1);

    assert_eq!(
        resolve_position(&model, id, Position::pixels(5, 5)),
        Some(Point::new(0, 0))
    );
    assert_eq!(
        resolve_position(&model, id, Position::normalized(1.0, 1.0)),
        Some(Point::new(0, 0))
    );
    assert_invariants(&model, &config);
}

#[test]
fn set_output_size_retiles_every_window_in_creation_order() {
    let mut config = default_config();
    let mut model = WindowModel::new();
    let ids = map_windows(&mut model, &config, 3);

    let actions = on_output_size(&mut model, &mut config, Size::new(800, 600));

    let rect = Rect::new(0, 0, 800, 600);
    assert_eq!(
        actions,
        vec![
            WmAction::ConfigureWindow { id: ids[0], rect },
            WmAction::ConfigureWindow { id: ids[1], rect },
            WmAction::ConfigureWindow { id: ids[2], rect },
        ]
    );
    assert_eq!(config.output_size, Size::new(800, 600));
    assert_eq!(config.tiled_rect(), rect);
    for id in &ids {
        assert_eq!(
            window(&model, *id).expect("record").geometry,
            rect,
            "inactive windows are re-tiled too"
        );
    }
    assert_eq!(active_window(&model), Some(ids[2]));
    assert_invariants(&model, &config);
}

#[test]
fn set_output_size_without_windows_only_updates_the_config() {
    let mut config = default_config();
    let mut model = WindowModel::new();

    assert_eq!(
        on_output_size(&mut model, &mut config, Size::new(640, 480)),
        Vec::new()
    );
    assert_eq!(config.tiled_rect(), Rect::new(0, 0, 640, 480));
    assert_invariants(&model, &config);
}

#[test]
fn mapping_after_a_resize_uses_the_new_tiled_rect() {
    let mut config = default_config();
    let mut model = WindowModel::new();
    on_output_size(&mut model, &mut config, Size::new(800, 600));

    let (id, actions) = on_map(&mut model, &config, MapRequest::new(SurfaceKey::new(1)));

    assert_eq!(
        actions,
        vec![
            WmAction::ConfigureWindow {
                id,
                rect: Rect::new(0, 0, 800, 600),
            },
            WmAction::Activate { id },
        ]
    );
    assert_invariants(&model, &config);
}

#[test]
fn invariants_hold_across_a_mixed_operation_sequence() {
    let mut config = default_config();
    let mut model = WindowModel::new();

    let ids: Vec<WindowId> = (1..=3)
        .map(|raw| {
            let (id, _) = on_map(&mut model, &config, MapRequest::new(SurfaceKey::new(raw)));
            assert_invariants(&model, &config);
            id
        })
        .collect();

    for id in [ids[0], ids[2], ids[1]] {
        activate(&mut model, id);
        assert_invariants(&model, &config);
    }

    on_title(&mut model, ids[1], Some("switched".into()));
    on_commit(&mut model, ids[1], 3, &Region::empty());
    on_popup_added(&mut model, ids[1]);
    on_output_size(&mut model, &mut config, Size::new(1024, 768));
    assert_invariants(&model, &config);

    // Destroying an inactive window keeps the active one.
    on_destroy(&mut model, ids[2]);
    assert_eq!(active_window(&model), Some(ids[1]));
    assert_invariants(&model, &config);

    // Destroying the active window falls back to the MRU window.
    assert_eq!(
        on_destroy(&mut model, ids[1]),
        vec![WmAction::ActivatePrevious { id: ids[0] }]
    );
    assert_invariants(&model, &config);

    let (last, _) = on_map(&mut model, &config, MapRequest::new(SurfaceKey::new(4)));
    assert_invariants(&model, &config);

    on_destroy(&mut model, last);
    on_destroy(&mut model, ids[0]);
    assert_eq!(active_window(&model), None);
    assert!(windows(&model).is_empty());
    assert_invariants(&model, &config);
}
