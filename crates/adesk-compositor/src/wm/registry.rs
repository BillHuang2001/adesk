//! Pure surface-tree bookkeeping for the window bridge.
//!
//! Stable [`SurfaceKey`]s, the owner lookup for every surface of a window tree, the
//! popup records and the popup-id counter. Kept free of Smithay types (the surface
//! key is generic) so the logic is unit-tested with plain integers; `super::WmBridge`
//! instantiates it with `ObjectId`.

use std::{collections::HashMap, hash::Hash};

use adesk_core::WindowId;
use adesk_wm::SurfaceKey;

/// Safety bound for popup-chain walks.
const MAX_POPUP_CHAIN: usize = 32;

/// A registered toplevel that may or may not be mapped yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ToplevelSlot {
    pub(super) key: SurfaceKey,
    pub(super) window: Option<WindowId>,
}

/// A tracked popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PopupRecord<K> {
    pub(super) window_id: WindowId,
    pub(super) popup_id: u64,
    pub(super) parent: Option<K>,
    pub(super) offset: (i32, i32),
}

/// A popup that was untracked together with its owner toplevel.
///
/// Carries the popup's own surface key, so the caller can drop the bookkeeping it
/// keys by that surface (the bridge's popup handles) without having to look the
/// popup up again — the record is already gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemovedPopup<K> {
    /// The id the popup was tracked under.
    pub(super) popup_id: u64,
    /// The popup's surface key.
    pub(super) surface: K,
}

/// Pure surface-tree bookkeeping: stable keys, owner lookup and popup ids.
///
/// Kept free of Smithay types (the key is generic) so the logic is unit-tested with
/// plain integers; `super::WmBridge` instantiates it with `ObjectId`.
#[derive(Debug)]
pub(super) struct SurfaceRegistry<K> {
    /// Registered toplevels by root surface key (mapped or not).
    toplevels: HashMap<K, ToplevelSlot>,
    /// Any surface of a window tree (root, subsurface, popup) → owning window.
    pub(super) owners: HashMap<K, WindowId>,
    /// Tracked popups by surface key.
    popups: HashMap<K, PopupRecord<K>>,
    /// Next [`SurfaceKey`] to hand out; never reused.
    next_surface_key: u64,
    /// Next popup id to hand out; never reused.
    next_popup_id: u64,
}

impl<K: Clone + Eq + Hash> SurfaceRegistry<K> {
    pub(super) fn new() -> SurfaceRegistry<K> {
        SurfaceRegistry {
            toplevels: HashMap::new(),
            owners: HashMap::new(),
            popups: HashMap::new(),
            next_surface_key: 1,
            next_popup_id: 1,
        }
    }

    /// Register a toplevel root surface, returning its stable key.
    ///
    /// Registering the same surface twice keeps the first key: a `SurfaceKey` is never
    /// reused and never reassigned.
    pub(super) fn register_toplevel(&mut self, surface: K) -> SurfaceKey {
        if let Some(slot) = self.toplevels.get(&surface) {
            return slot.key;
        }
        let key = SurfaceKey::new(self.next_surface_key);
        self.next_surface_key += 1;
        self.toplevels
            .insert(surface, ToplevelSlot { key, window: None });
        key
    }

    pub(super) fn slot(&self, surface: &K) -> Option<&ToplevelSlot> {
        self.toplevels.get(surface)
    }

    /// Whether `surface` is a registered toplevel that has not been mapped yet.
    pub(super) fn is_unmapped_toplevel(&self, surface: &K) -> bool {
        self.toplevels
            .get(surface)
            .is_some_and(|slot| slot.window.is_none())
    }

    /// The window owning any surface of a tree.
    pub(super) fn window_for_surface(&self, surface: &K) -> Option<WindowId> {
        self.owners.get(surface).copied()
    }

    /// Bind a mapped window to its root surface.
    pub(super) fn bind(&mut self, surface: K, window: WindowId) {
        if let Some(slot) = self.toplevels.get_mut(&surface) {
            slot.window = Some(window);
        }
        self.owners.insert(surface, window);
    }

    /// Drop a toplevel and every popup it owns.
    ///
    /// Returns the window id (when it was mapped) and the popups that were still open,
    /// in creation order. Each [`RemovedPopup`] carries the popup's surface key as well
    /// as its id, so a caller holding per-popup bookkeeping keyed by that surface can
    /// drop exactly the entries the registry just forgot.
    pub(super) fn unbind_toplevel(
        &mut self,
        surface: &K,
    ) -> (Option<WindowId>, Vec<RemovedPopup<K>>) {
        let window = self.toplevels.remove(surface).and_then(|slot| slot.window);
        self.owners.remove(surface);
        let popups = match window {
            Some(window_id) => self.remove_popups_of(window_id),
            None => Vec::new(),
        };
        (window, popups)
    }

    /// Remove every popup owned by `window_id`, returning them in creation order.
    fn remove_popups_of(&mut self, window_id: WindowId) -> Vec<RemovedPopup<K>> {
        let mut popups: Vec<RemovedPopup<K>> = self
            .popups
            .iter()
            .filter(|(_, popup)| popup.window_id == window_id)
            .map(|(key, popup)| RemovedPopup {
                popup_id: popup.popup_id,
                surface: key.clone(),
            })
            .collect();
        popups.sort_unstable_by_key(|popup| (popup.popup_id, key_hash(&popup.surface)));
        self.popups.retain(|_, popup| popup.window_id != window_id);
        self.owners.retain(|_, owner| *owner != window_id);
        popups
    }

    /// Track a popup under its owner window, returning its stable id.
    ///
    /// A popup whose surface is already tracked keeps its id (idempotent).
    pub(super) fn add_popup(
        &mut self,
        surface: K,
        parent: Option<K>,
        window_id: WindowId,
        offset: (i32, i32),
    ) -> u64 {
        if let Some(existing) = self.popups.get(&surface) {
            return existing.popup_id;
        }
        let popup_id = self.next_popup_id;
        self.next_popup_id += 1;
        self.popups.insert(
            surface.clone(),
            PopupRecord {
                window_id,
                popup_id,
                parent,
                offset,
            },
        );
        self.owners.insert(surface, window_id);
        popup_id
    }

    pub(super) fn popup(&self, surface: &K) -> Option<&PopupRecord<K>> {
        self.popups.get(surface)
    }

    /// Remove a tracked popup.
    pub(super) fn remove_popup(&mut self, surface: &K) -> Option<PopupRecord<K>> {
        let record = self.popups.remove(surface)?;
        self.owners.remove(surface);
        Some(record)
    }

    /// Window-relative origin of a popup, accumulated over its popup chain.
    ///
    /// `None` when `surface` is not a tracked popup. The walk stops at the first
    /// ancestor that is not a popup (the toplevel root or a subsurface) or when the
    /// [`MAX_POPUP_CHAIN`] safety bound is hit.
    pub(super) fn popup_window_offset(&self, surface: &K) -> Option<(i32, i32)> {
        self.popups.get(surface)?;
        let mut offset = (0i32, 0i32);
        let mut current = surface;
        let mut depth = 0;
        while let Some(record) = self.popups.get(current) {
            offset.0 = offset.0.saturating_add(record.offset.0);
            offset.1 = offset.1.saturating_add(record.offset.1);
            depth += 1;
            match (&record.parent, depth < MAX_POPUP_CHAIN) {
                (Some(parent), true) => current = parent,
                _ => break,
            }
        }
        Some(offset)
    }
}

/// Stable ordering helper: popup ids are unique, so hashing the key only breaks ties.
fn key_hash<K: Hash>(key: &K) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}
