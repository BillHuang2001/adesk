//! Fixtures: `.desktop` files, isolated data dirs and the `adesk-test-app` helper.
//!
//! PLACEHOLDER — replaced by the full module during the same architecture phase.
//! It only exists so the crate compiles while the real module is written in a
//! parallel worktree; it must keep [`FixtureDir::search_dir`] working because
//! [`crate::TestRuntimeConfig::with_fixture_dir`] calls it.

use std::path::{Path, PathBuf};

use crate::error::Result;

/// A temp `XDG_DATA_DIRS` share root holding `.desktop` fixtures (placeholder).
pub struct FixtureDir;

impl FixtureDir {
    /// The share root to pass as an app dir (placeholder).
    pub fn search_dir(&self) -> &Path {
        todo!("stub: replaced by the real fixtures module")
    }
}

/// A `.desktop` entry to write into a [`FixtureDir`] (placeholder).
pub struct DesktopEntryFixture;

/// A helper process that opens a toplevel (placeholder).
pub struct TestApp;

/// Description of a [`TestApp`] (placeholder).
pub struct TestAppSpec;

/// Resolves a helper binary next to the running test executable (placeholder).
pub fn helper_bin_path(name: &str) -> Result<PathBuf> {
    let _ = name;
    todo!("stub: replaced by the real fixtures module")
}
