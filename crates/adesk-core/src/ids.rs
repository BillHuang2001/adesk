//! Stable identifiers used across the runtime.
//!
//! All identifiers are transparent over their inner type on the wire: a
//! `WindowId` serializes as the bare number, an [`AppId`] as the bare string.
//! Numeric ids are monotonic and never reused (assignment is the compositor's
//! job, see `docs/architecture.md`).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Defines a numeric identifier newtype with `Display`, `From`/`Into` for its
/// inner type, and transparent serde.
macro_rules! numeric_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

numeric_id!(
    /// Identifies a toplevel window. Stable for the lifetime of the window,
    /// monotonic, never reused.
    WindowId
);

numeric_id!(
    /// Identifies one agent input action (one `click`, `keypress`, ...).
    /// Observations reference it through `after_action`.
    ActionId
);

numeric_id!(
    /// Identifies one `launch_app` call. Windows mapped shortly after a launch
    /// carry the same id, enabling correlation.
    LaunchId
);

/// Identifies an application by its desktop-file id, e.g.
/// `"org.mozilla.firefox"`.
///
/// Unlike the numeric ids this is `Clone` but not `Copy` (it owns a string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppId(pub String);

impl AppId {
    /// Borrows the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for AppId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AppId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<AppId> for String {
    fn from(value: AppId) -> Self {
        value.0
    }
}

impl AsRef<str> for AppId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_ids_display_and_convert() {
        let window = WindowId(17);
        assert_eq!(window.to_string(), "17");
        assert_eq!(u64::from(window), 17);
        assert_eq!(WindowId::from(17_u64), window);

        let action = ActionId(582);
        assert_eq!(action.to_string(), "582");
        assert_eq!(u64::from(action), 582);
        assert_eq!(ActionId::from(582_u64), action);

        let launch = LaunchId(3);
        assert_eq!(launch.to_string(), "3");
        assert_eq!(u64::from(launch), 3);
        assert_eq!(LaunchId::from(3_u64), launch);
    }

    #[test]
    fn numeric_ids_are_ordered_and_hashable() {
        use std::collections::HashSet;

        let mut ids = vec![WindowId(3), WindowId(1), WindowId(2)];
        ids.sort();
        assert_eq!(ids, vec![WindowId(1), WindowId(2), WindowId(3)]);

        let set: HashSet<WindowId> = ids.iter().copied().collect();
        assert_eq!(set.len(), 3);
        assert!(set.contains(&WindowId(2)));
    }

    #[test]
    fn app_id_conversions_and_display() {
        let id = AppId::from("org.mozilla.firefox");
        assert_eq!(id.as_str(), "org.mozilla.firefox");
        assert_eq!(id.as_ref(), "org.mozilla.firefox");
        assert_eq!(id.to_string(), "org.mozilla.firefox");

        let owned: String = id.clone().into();
        assert_eq!(owned, "org.mozilla.firefox");
        assert_eq!(AppId::from(owned.clone()), id);
        assert_eq!(AppId::from(String::from("code")), AppId("code".into()));
    }

    #[test]
    fn numeric_id_serde_is_transparent() {
        assert_eq!(serde_json::to_string(&WindowId(17)).unwrap(), "17");
        assert_eq!(
            serde_json::from_str::<WindowId>("17").unwrap(),
            WindowId(17)
        );
        assert_eq!(
            serde_json::to_string(&AppId::from("org.mozilla.firefox")).unwrap(),
            "\"org.mozilla.firefox\""
        );
    }
}
