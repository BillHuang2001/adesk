//! Protocol defaults from `docs/protocol.md` §5, in one place.
//!
//! These functions are referenced from `#[serde(default = "...")]` attributes on
//! the params structs so the spec defaults apply when a field is absent.

use adesk_core::OverlayKind;

use crate::EventKind;

/// Default `timeout_ms` for `observe`, `wait_for_change`, `wait_for_quiet` (§5.4).
pub(crate) const fn timeout_ms() -> u64 {
    5000
}

/// Default `quiet_ms` for `wait_for_quiet` (§5.4).
pub(crate) const fn quiet_ms() -> u64 {
    250
}

/// Default `duration_ms` for `drag` (§5.5).
pub(crate) const fn duration_ms() -> u64 {
    150
}

/// Default `min_interval_ms` for `inspect_subscribe` (§5.7).
pub(crate) const fn min_interval_ms() -> u64 {
    100
}

/// Default `count` for `click` (§5.5).
pub(crate) const fn count() -> u32 {
    1
}

/// Default `max_events` for `wait_for_events` (§5.10).
pub(crate) const fn max_events() -> u32 {
    32
}

/// Default `max_depth` for `accessibility_tree` (§5.11, `12`).
pub(crate) const fn accessibility_max_depth() -> u32 {
    12
}

/// Default `max_nodes` for `accessibility_tree` (§5.11, `2000`).
pub(crate) const fn accessibility_max_nodes() -> u32 {
    2000
}

/// Default `max_results` for `find_accessible` (§5.11, `50`).
pub(crate) const fn find_max_results() -> u32 {
    50
}

/// Default of the four `accessibility_tree` projection flags (§5.11, `true`).
pub(crate) const fn accessibility_include() -> bool {
    true
}

/// Default `include_image` for `observe` (§5.4, `true`).
pub(crate) const fn include_image() -> bool {
    true
}

/// Default `scale` of an [`ImagePayload`](crate::ImagePayload) (§4, `1.0`).
pub(crate) const fn scale() -> f64 {
    1.0
}

/// Default `kinds` of `subscribe_events` (§5.6) and `wait_for_events` (§5.10):
/// all fourteen filterable kinds (§5.6/§5.9).
pub(crate) fn event_kinds() -> Vec<EventKind> {
    EventKind::SUBSCRIBABLE.to_vec()
}

/// Default `overlays` of `inspect_capture`: `["window_ids","focus","damage"]` (§5.7).
pub(crate) fn inspect_overlays() -> Vec<OverlayKind> {
    vec![
        OverlayKind::WindowIds,
        OverlayKind::Focus,
        OverlayKind::Damage,
    ]
}
