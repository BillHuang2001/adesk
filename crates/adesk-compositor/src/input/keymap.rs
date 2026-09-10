//! Keysym → (keycode, shift level) resolution.
//!
//! The compositor injects input as *keycodes* plus modifier state, but AGP
//! speaks keysyms. [`KeymapTable`] bridges the two: it compiles the configured
//! xkb keymap once (the same [`XkbSettings`](crate::config::XkbSettings) the
//! seat uses) and records, for every keysym the keymap can produce, the first
//! physical key and shift level that produces it.
//!
//! "First" means: lowest level wins (so `a` is unshifted, `+` is shifted on a US
//! layout), and among equal levels the lowest keycode wins. The table is built
//! at startup and never mutated afterwards.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use smithay::input::keyboard::xkb;

use crate::{config::XkbSettings, error::CompositorError, Result};

/// A physical key plus the shift level that produces a keysym.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedKey {
    /// xkb keycode (evdev code + 8, as used by the Wayland keyboard protocol).
    pub(crate) keycode: u32,
    /// xkb shift level: `0` unshifted, `1` shifted, ...
    pub(crate) level: usize,
}

/// The compiled keysym lookup table.
pub(crate) struct KeymapTable {
    /// keysym value → first key/level that produces it.
    table: HashMap<u32, ResolvedKey>,
}

impl KeymapTable {
    /// Compile a keymap from `settings` and index every keysym it can produce.
    ///
    /// Fails with [`CompositorError::Keyboard`] when libxkbcommon cannot compile
    /// the requested rules/model/layout/variant/options (e.g. a layout that is
    /// not installed).
    pub(crate) fn new(settings: &XkbSettings) -> Result<KeymapTable> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names::<str>(
            &context,
            &settings.rules,
            &settings.model,
            &settings.layout,
            &settings.variant,
            settings.options.clone(),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| {
            CompositorError::Keyboard(format!(
                "failed to compile xkb keymap (rules={:?} model={:?} layout={:?} \
                 variant={:?} options={:?})",
                settings.rules, settings.model, settings.layout, settings.variant, settings.options
            ))
        })?;

        Ok(KeymapTable {
            table: index_keymap(&keymap),
        })
    }

    /// The key and shift level producing `keysym`, if the keymap can produce it.
    pub(crate) fn resolve(&self, keysym: u32) -> Option<ResolvedKey> {
        self.table.get(&keysym).copied()
    }
}

/// Scan every keycode/layout/level of `keymap` and keep the first
/// `(keycode, level)` per keysym, preferring the lowest level.
fn index_keymap(keymap: &xkb::Keymap) -> HashMap<u32, ResolvedKey> {
    let mut table = HashMap::new();
    for raw_keycode in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        let keycode = xkb::Keycode::new(raw_keycode);
        for layout in 0..keymap.num_layouts() {
            for level in 0..keymap.num_levels_for_key(keycode, layout) {
                for keysym in keymap.key_get_syms_by_level(keycode, layout, level) {
                    let value = keysym.raw();
                    if value == 0 {
                        continue; // NoSymbol: not a usable key
                    }
                    let candidate = ResolvedKey {
                        keycode: raw_keycode,
                        level: level as usize,
                    };
                    match table.entry(value) {
                        Entry::Vacant(slot) => {
                            slot.insert(candidate);
                        }
                        Entry::Occupied(mut slot) => {
                            // Same keysym on another key: prefer the lower level,
                            // otherwise keep the first (lowest) keycode.
                            if candidate.level < slot.get().level {
                                slot.insert(candidate);
                            }
                        }
                    }
                }
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Keysym;

    fn us_table() -> KeymapTable {
        KeymapTable::new(&XkbSettings::us()).expect("the us keymap must compile")
    }

    #[test]
    fn letters_resolve_unshifted() {
        let table = us_table();
        let a = Keysym::parse("a").unwrap();
        let resolved = table.resolve(a.value()).expect("`a` is in the us keymap");
        assert_eq!(resolved.level, 0);
        assert_ne!(resolved.keycode, 0);
    }

    #[test]
    fn shifted_characters_resolve_at_level_one() {
        let table = us_table();
        let plus = Keysym::parse("+").unwrap();
        let resolved = table
            .resolve(plus.value())
            .expect("`+` is in the us keymap");
        assert_eq!(resolved.level, 1);
    }

    #[test]
    fn unknown_keysyms_do_not_resolve() {
        let table = us_table();
        assert_eq!(table.resolve(0), None);
        assert_eq!(table.resolve(0xdead_beef), None);
    }

    #[test]
    fn uncompilable_settings_are_keyboard_errors() {
        let settings = XkbSettings {
            layout: "no_such_layout_xyz".to_owned(),
            ..XkbSettings::us()
        };
        let error = KeymapTable::new(&settings)
            .err()
            .expect("a layout that does not exist must fail to compile");
        let message = match error {
            CompositorError::Keyboard(message) => message,
            other => panic!("expected a keyboard error, got {other:?}"),
        };
        assert!(message.contains("no_such_layout_xyz"), "{message}");
    }
}
