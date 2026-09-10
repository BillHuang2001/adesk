//! Environment-gated capabilities.
//!
//! The sandbox and CI have no GPU and no system EGL; the harness therefore uses the
//! pixman (software) renderer by default and only exercises GL when a developer opts in.
//! GL tests must call [`require_gl`] first and return early when it fails, so a GPU-less
//! machine reports "skipped", never a failure.

use adesk_compositor::RendererKind;

use crate::error::{Result, TestkitError};

/// Environment variable that enables GPU/EGL tests (set it to `"1"`).
pub const GL_ENV_VAR: &str = "ADESK_TEST_GL";

/// Whether GL tests are enabled for this process (`ADESK_TEST_GL=1`).
pub fn gl_enabled() -> bool {
    matches!(std::env::var(GL_ENV_VAR).as_deref(), Ok("1"))
}

/// Returns `Ok(())` when GL tests are enabled, otherwise a skip-shaped error.
///
/// The canonical pattern in a GL test is:
///
/// ```no_run
/// if adesk_testkit::require_gl().is_err() {
///     eprintln!("skipping GL test: set ADESK_TEST_GL=1");
///     return;
/// }
/// ```
pub fn require_gl() -> Result<()> {
    if gl_enabled() {
        Ok(())
    } else {
        Err(TestkitError::Unsupported(format!(
            "GL tests are disabled; set {GL_ENV_VAR}=1 to enable them (CI has no GPU)"
        )))
    }
}

/// The renderer a test runtime should use: [`RendererKind::Pixman`] unless GL tests are
/// enabled, in which case [`RendererKind::Gl`] (so a broken GL setup fails loudly instead
/// of silently falling back).
pub fn test_renderer() -> RendererKind {
    if gl_enabled() {
        RendererKind::Gl
    } else {
        RendererKind::Pixman
    }
}
