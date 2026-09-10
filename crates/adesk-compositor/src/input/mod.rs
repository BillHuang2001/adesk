//! Input injection: key/chord parsing, the keysym table and the Smithay wrappers.
//!
//! Three layers, deliberately separated:
//!
//! 1. [`keycode`] parses AGP key names (`docs/protocol.md` §3) into [`KeyCode`]s
//!    and expands them into the ordered injection sequence ([`chord_sequence`]).
//!    Pure logic plus `xkb_keysym_from_name`, no Smithay state.
//! 2. [`keymap`] compiles the configured xkb keymap once and answers "which
//!    physical key and shift level produces this keysym?".
//! 3. [`injector`] owns the seat's keyboard and pointer handles and turns an
//!    already-resolved event into exactly one Smithay call.
//!
//! Orchestration (resolving keysyms through the table, pressing Shift for
//! shifted characters, choosing focus targets) stays in `crate::state`.

mod injector;
mod keycode;
mod keymap;

pub(crate) use injector::{InputInjector, LEVEL3_KEYSYM, SHIFT_KEYSYM};
pub(crate) use keycode::chord_sequence;
pub use keycode::{KeyCode, Keysym};
