//! Pixel assertions over [`ImageBuffer`](adesk_core::ImageBuffer).
//!
//! [`ImageAssert`] is a *view* over a borrowed image: it never copies pixels and never owns
//! the buffer, so it is cheap to construct inside a test body. Comparison methods panic on
//! failure with a message naming the offending coordinate, the expected value and the actual
//! value (see the [module docs](self)); the two methods that touch the filesystem return
//! [`Result`] because a failing write is an environment problem, not an assertion.
//!
//! ## One ground truth
//!
//! Pattern comparisons never re-implement a pattern: the expected pixel at `(x, y)` is
//! always [`FillPattern::at`]`(x, y, size)` for the image's own
//! [`Size`](adesk_core::Size). The Wayland test client fills SHM buffers by evaluating the
//! same function, so an assertion can never disagree with what the client drew.
//!
//! ## Coordinates
//!
//! Every coordinate and rect here is image-relative pixels, origin top-left — the same space
//! as window-relative AGP coordinates. The default test window is tiled 1:1, so image and
//! window coordinates coincide unless a capture was cropped or scaled.
//!
//! ## Strides
//!
//! Pixel access goes through [`ImageBuffer::pixel`], which honours `stride`; row padding is
//! never compared and never influences means. Repacking to a tightly packed buffer happens
//! only on the PNG paths.

use std::path::{Path, PathBuf};

use adesk_core::{ImageBuffer, Rect};

use crate::error::Result;
use crate::fill::FillPattern;

/// An assertion view over one [`ImageBuffer`].
///
/// Construct with [`ImageAssert::new`] and chain comparisons:
///
/// ```ignore
/// ImageAssert::new(&image).matches_pattern(FillPattern::default());
/// assert!(ImageAssert::new(&image).region_avg(target).iter().all(|c| *c > 128.0));
/// ```
///
/// The struct holds only `&ImageBuffer`, so it is `Copy`-cheap to create and cannot outlive
/// the image it asserts on.
pub struct ImageAssert<'a> {
    image: &'a ImageBuffer,
}

impl<'a> ImageAssert<'a> {
    /// Wraps `image` for assertion.
    pub fn new(image: &'a ImageBuffer) -> Self {
        ImageAssert { image }
    }

    /// The image under assertion.
    pub fn image(&self) -> &ImageBuffer {
        self.image
    }

    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        self.image.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        self.image.height
    }

    /// The RGBA value at `(x, y)`.
    ///
    /// # Panics
    ///
    /// Panics when `(x, y)` is outside the image. Phase 2 message format:
    /// `"pixel (x, y) out of bounds for image WxH"` — the coordinates come first and the
    /// image size second, so a failure immediately shows which axis is wrong.
    ///
    /// Phase 2 implementation: delegate to [`ImageBuffer::pixel`] and panic on `None`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let _ = (x, y);
        todo!("stub: implementation phase — ImageBuffer::pixel, panic with coordinates and image size")
    }

    /// Per-channel mean of the pixels inside `rect`.
    ///
    /// The result is `[r, g, b, a]` where each channel is the arithmetic mean over the
    /// `rect.w * rect.h` pixels, computed in `f32`. The mean is over *pixels*, not bytes,
    /// so stride padding never influences it. An empty rect is a harness bug rather than an
    /// empty image, so it panics instead of returning `NaN`.
    ///
    /// # Panics
    ///
    /// Panics when `rect.w == 0 || rect.h == 0`, when `rect.x < 0 || rect.y < 0`, or when
    /// the rect extends past the image (`rect.x + rect.w > width || rect.y + rect.h >
    /// height`). Phase 2 message format:
    /// `"region {rect:?} out of bounds for image WxH"`.
    pub fn region_avg(&self, rect: Rect) -> [f32; 4] {
        let _ = rect;
        todo!("stub: implementation phase — bounds-check, then per-channel f32 mean over the rect")
    }

    /// Asserts that every pixel equals `pattern`.
    ///
    /// The expected value for `(x, y)` is `pattern.at(x, y, size)` with
    /// `size = Size { w: width(), h: height() }`: [`FillPattern`] is the single ground
    /// truth and is never re-implemented here. Comparison is exact on all four channels,
    /// alpha included.
    ///
    /// # Panics
    ///
    /// Panics on the **first** mismatching pixel in row-major order, so the reported
    /// coordinate is deterministic. Phase 2 message format:
    /// `"pixel (x, y) = [r, g, b, a], expected [r, g, b, a] for pattern {pattern:?}"`.
    pub fn matches_pattern(&self, pattern: FillPattern) {
        let _ = pattern;
        todo!(
            "stub: implementation phase — compare every pixel against FillPattern::at(x, y, size)"
        )
    }

    /// Asserts that every pixel matches `pattern` within `tolerance` per channel.
    ///
    /// For each pixel and each of the four channels, `|actual - expected| <= tolerance`
    /// must hold, where `expected = pattern.at(x, y, size)` exactly as in
    /// [`ImageAssert::matches_pattern`]. `tolerance == 0` is exactly `matches_pattern`.
    /// This is the form to use when the pixel path is not byte-exact (GPU/`llvmpipe`
    /// rendering, scaling, compositing) but the pattern is still recognisable.
    ///
    /// # Panics
    ///
    /// Panics on the first pixel whose worst channel exceeds `tolerance`. Phase 2 message
    /// format: `"pixel (x, y) = [...], expected [...] ± tol for pattern {pattern:?} (worst
    /// channel {index}, delta {delta})"`.
    pub fn matches_pattern_tol(&self, pattern: FillPattern, tolerance: u8) {
        let _ = (pattern, tolerance);
        todo!(
            "stub: implementation phase — per-channel |a - b| <= tolerance against FillPattern::at"
        )
    }

    /// Asserts that every pixel is within `tolerance` of the solid colour `rgba`.
    ///
    /// Exactly
    /// [`ImageAssert::matches_pattern_tol`]`(FillPattern::Solid(rgba), tolerance)`; it
    /// exists because the common case ("the window is this colour") should not require
    /// constructing a [`FillPattern`] first.
    ///
    /// # Panics
    ///
    /// Same as [`ImageAssert::matches_pattern_tol`].
    pub fn matches_solid(&self, rgba: [u8; 4], tolerance: u8) {
        let _ = (rgba, tolerance);
        todo!("stub: implementation phase — delegate to matches_pattern_tol(FillPattern::Solid(rgba), tolerance)")
    }

    /// Asserts that `other` is **not** identical to this image.
    ///
    /// Two images are identical when their dimensions are equal and every pixel compares
    /// equal through [`ImageBuffer::pixel`]. Comparison is over logical pixels, so stride
    /// padding and the stride value itself are ignored: two buffers holding the same pixels
    /// with different strides are identical. Different dimensions count as a difference, so
    /// an image of another size passes this assertion.
    ///
    /// # Panics
    ///
    /// Panics when the images are identical. Phase 2 message format:
    /// `"images are identical (WxH)"`.
    pub fn differs_from(&self, other: &ImageBuffer) {
        let _ = other;
        todo!(
            "stub: implementation phase — panic when sizes match and every logical pixel is equal"
        )
    }

    /// Asserts that `other` differs from this image by more than `tolerance` on at least one
    /// channel of at least one pixel.
    ///
    /// `other` satisfies the assertion when its dimensions differ, or when some pixel has a
    /// channel with `|self - other| > tolerance`. Equivalently, it panics when the images
    /// are equal within `tolerance` — the "the frame did not visibly change" case.
    ///
    /// # Panics
    ///
    /// Panics when the images are equal within `tolerance`. Phase 2 message format:
    /// `"images are equal within tolerance {tolerance} (WxH)"`.
    pub fn differs_from_tol(&self, other: &ImageBuffer, tolerance: u8) {
        let _ = (other, tolerance);
        todo!("stub: implementation phase — panic when every pixel channel differs by at most tolerance")
    }

    /// Encodes the image as PNG at `path`.
    ///
    /// Phase 2 implementation: repack the logical pixels into a tightly packed RGBA8
    /// buffer (dropping stride padding) and call `image::RgbaImage::from_raw` +
    /// `image::ImageBuffer::save`. An `image::ImageError` maps to
    /// [`TestkitError::Io`](crate::TestkitError::Io) via `std::io::Error::other(e)`, so
    /// there is exactly one IO error shape in the crate.
    ///
    /// Parent directories are **not** created; use [`ImageAssert::dump_on_failure`] for the
    /// always-writable debug path. Returns [`TestkitError::Io`](crate::TestkitError::Io)
    /// when the path is not writable or PNG encoding fails.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        let _ = path.as_ref();
        todo!("stub: implementation phase — repack to RGBA8, image::RgbaImage::from_raw + save, map ImageError to Io")
    }

    /// Saves the image as `<dir>/<name>.png` for post-mortem inspection and returns the
    /// absolute path written.
    ///
    /// `dir` is `$ADESK_TEST_ASSERT_DIR` when set and non-empty, otherwise
    /// `std::env::temp_dir().join("adesk-testkit-dumps")`. The directory and its parents are
    /// created when missing and an existing file is overwritten, so this call only fails for
    /// real filesystem problems. `name` is used verbatim as the file stem; callers pass a
    /// test-unique name such as `"click-target"`.
    ///
    /// Phase 2 semantics: never panics — a write failure is returned as
    /// [`TestkitError::Io`](crate::TestkitError::Io) so a test can report "assertion failed
    /// and the dump could not be written" instead of masking the original failure. The
    /// returned path is exactly the path written, so a test can print it or assert on it.
    pub fn dump_on_failure(&self, name: &str) -> Result<PathBuf> {
        let _ = name;
        todo!("stub: implementation phase — resolve $ADESK_TEST_ASSERT_DIR else temp_dir()/adesk-testkit-dumps, create_dir_all, save_png")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_forward_to_the_image() {
        let image = ImageBuffer::new_rgba(7, 3);
        let assert = ImageAssert::new(&image);
        assert_eq!(assert.width(), 7);
        assert_eq!(assert.height(), 3);
        assert_eq!(assert.image().width, 7);
        assert_eq!(assert.image().height, 3);
        assert!(std::ptr::eq(assert.image(), &image));
    }
}
