//! Compile-time pinning of the Smithay 0.7 trait surface this crate builds on.
//!
//! `render_scene`, `create_target` and `import_buffer` are generic over the
//! renderer; these assertions fail the build if Smithay changes the traits the
//! pipeline depends on. They also pin that both renderer error types are
//! `Send + Sync + 'static`, which the structured error boxing requires.

use smithay::backend::renderer::gles::{GlesError, GlesRenderbuffer, GlesRenderer};
use smithay::backend::renderer::pixman::{PixmanError, PixmanRenderer};
use smithay::backend::renderer::{Bind, ExportMem, ImportAll, Offscreen, Renderer};
use smithay::reexports::pixman::Image;

fn assert_send_sync_static<T: Send + Sync + 'static>() {}

fn assert_offscreen_renderer<R, T>()
where
    R: Renderer + Bind<T> + ExportMem + Offscreen<T>,
    R::Error: Send + Sync + 'static,
{
}

fn assert_import_all<R: ImportAll>() {}

#[test]
fn gles_renderer_satisfies_pipeline_bounds() {
    assert_send_sync_static::<GlesError>();
    assert_offscreen_renderer::<GlesRenderer, GlesRenderbuffer>();
    assert_import_all::<GlesRenderer>();
}

#[test]
fn pixman_renderer_satisfies_pipeline_bounds() {
    assert_send_sync_static::<PixmanError>();
    assert_offscreen_renderer::<PixmanRenderer, Image<'static, 'static>>();
    assert_import_all::<PixmanRenderer>();
}
