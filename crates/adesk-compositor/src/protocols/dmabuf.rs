//! `zwp_linux_dmabuf_v1` handling (DMA-BUF client buffers).
//!
//! The global is created in [`State::new`](crate::state::State::new) from the
//! renderer's supported formats; the [`DmabufHandler`] here owns the per-buffer
//! import path into the renderer.
//!
//! Import happens synchronously on the compositor thread through the concrete
//! renderer — both backends implement
//! [`ImportDma`]: `GlesRenderer` through an
//! EGL image, `PixmanRenderer` by mapping the dmabuf. The renderer keeps its own
//! reference to the imported texture (keyed by the weak dmabuf, dropped when the
//! client's buffer is gone), so the handle returned here is only used to learn
//! whether the import succeeded.
//!
//! Failures are reported to the client with [`ImportNotifier::failed`] and logged.
//! The outcome is not always survivable: for an import started with `create_immed`
//! (Smithay's `Import::Infallible`, which GTK4/GDK uses) the protocol requires the
//! compositor to raise `invalid_wl_buffer`, a Fatal error, so a failed import
//! disconnects that client. That is why the global is advertised only for a renderer
//! that really imports client DMA-BUFs
//! ([`HeadlessRenderer::imports_dmabuf`]) — advertising one a client cannot use is a
//! client-killing configuration, not a degradable one.
//!
//! # Defensive descriptor validation
//!
//! Before a buffer reaches the renderer it is checked structurally by
//! [`validate_dmabuf`]: every plane must have a non-zero stride, an offset inside
//! its plane fd, and room for at least one row; a single-plane buffer *without* a
//! real modifier (Linear or implicit) must additionally fit entirely inside its fd.
//! The per-plane checks are exactly the arithmetic preconditions of the backends'
//! own mmap path — Smithay's `Dmabuf::map_plane` computes `len = fd_size - offset`
//! (an underflow for an offset beyond the fd, which would hand a bogus length to
//! `mmap`/EGL) — while the whole-buffer check is `PixmanRenderer`'s own condition
//! for a linearly mapped buffer. That whole-buffer rule is therefore applied only to
//! Linear/implicit single-plane buffers: for a tiled, compressed or vendor modifier
//! the plane allocation's relationship to `stride * height` is driver-defined, so
//! enforcing the strict rule would false-reject a buffer the renderer can import.
//! Rejecting a genuinely out-of-range descriptor here answers the client `failed()`
//! with a WARN naming the defect instead of feeding out-of-range geometry into the C
//! graphics stack.
//!
//! Telemetry ([`describe_dmabuf`]) is structural only — format, size, plane count,
//! and per plane the offset, stride and fd size — and never contains pixel
//! payloads.

use std::{fmt, io::Seek, os::fd::BorrowedFd};

use smithay::{
    backend::{
        allocator::{dmabuf::Dmabuf, Buffer},
        renderer::ImportDma,
    },
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::{render::HeadlessRenderer, state::State};

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // Structural gate: a descriptor that cannot be handed to the graphics
        // stack (underflowing length, zero stride, degenerate size, ...) is
        // refused here with the full telemetry in the log. The renderer is never
        // asked to import it.
        if let Err(defect) = validate_dmabuf(&dmabuf) {
            let description = describe_dmabuf(&dmabuf);
            tracing::warn!(
                defect = %defect,
                description = %description,
                "dmabuf import rejected before the renderer; a `create_immed` import is \
                 terminated by the protocol, and `--dmabuf off` avoids advertising the \
                 global at all"
            );
            notifier.failed();
            return;
        }

        // The two backends return different texture/error types, so each arm
        // reports its own outcome through the shared helper.
        match &mut self.renderer {
            HeadlessRenderer::Gl { renderer, .. } => {
                notify(renderer.import_dmabuf(&dmabuf, None), &dmabuf, notifier)
            }
            HeadlessRenderer::Pixman { renderer, .. } => {
                notify(renderer.import_dmabuf(&dmabuf, None), &dmabuf, notifier)
            }
        }
    }
}

/// Report a renderer import outcome to the client.
///
/// Generic over the backend's texture and error type so both renderer paths share
/// one notification path: on success the client gets its `wl_buffer`, on failure
/// `failed()` (an implementation-dependent import failure, not a protocol error).
/// For a `create_immed` import that failure is fatal to the client — the protocol
/// layer raises `invalid_wl_buffer` — so the log says so and names the
/// `--dmabuf off` escape hatch; this helper never hides the failure behind a
/// fallback. The `notifier` is consumed on every path; `dmabuf` is only used to
/// describe the buffer in the log, and only when a message is actually emitted.
fn notify<T, E: std::fmt::Display>(
    imported: Result<T, E>,
    dmabuf: &Dmabuf,
    notifier: ImportNotifier,
) {
    match imported {
        Ok(_texture) => {
            // The renderer holds its own reference to the texture; the client may
            // still have vanished between import and notification.
            let description = describe_dmabuf(dmabuf);
            if let Err(error) = notifier.successful::<State>() {
                tracing::debug!(%error, description = %description, "dmabuf imported for a client that is already gone");
            } else {
                tracing::debug!(description = %description, "dmabuf import succeeded");
            }
        }
        Err(error) => {
            let description = describe_dmabuf(dmabuf);
            tracing::warn!(
                %error,
                description = %description,
                "dmabuf import failed; a `create_immed` import is terminated by the \
                 protocol, and `--dmabuf off` avoids advertising the global at all"
            );
            notifier.failed();
        }
    }
}

/// A structural defect that makes a client's DMA-BUF descriptor unusable.
///
/// Every variant is a *rejection*: the compositor answers the client `failed()`
/// and never hands the buffer to the renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DmabufDefect {
    /// The descriptor carries no planes, so there is nothing to import.
    NoPlanes,
    /// The buffer has a non-positive width or height.
    EmptySize {
        /// Declared buffer width in pixels.
        width: i32,
        /// Declared buffer height in pixels.
        height: i32,
    },
    /// A plane declares a zero stride, so no pixel can be addressed in it.
    ZeroStride {
        /// Zero-based plane index.
        plane: usize,
    },
    /// A plane's byte offset is at or beyond its fd's size; Smithay's
    /// `len = fd_size - offset` would underflow.
    PlaneOffsetBeyondSize {
        /// Zero-based plane index.
        plane: usize,
        /// The plane's byte offset.
        offset: u32,
        /// The plane fd's size in bytes.
        plane_size: u64,
    },
    /// A plane's fd is too small to hold even one stride starting at its offset.
    PlaneTooShort {
        /// Zero-based plane index.
        plane: usize,
        /// The plane's byte offset.
        offset: u32,
        /// The plane's stride in bytes.
        stride: u32,
        /// The plane fd's size in bytes.
        plane_size: u64,
    },
    /// A linearly mapped single-plane buffer's fd cannot hold the whole image
    /// (`offset + stride * height`).
    ///
    /// Only reported for a single-plane buffer without a real modifier (Linear or
    /// implicit): a tiled/compressed/vendor modifier makes the plane allocation's
    /// relationship to `stride * height` driver-defined, so the whole-buffer rule is
    /// not applied there.
    WholeBufferTooShort {
        /// Zero-based plane index (always `0`).
        plane: usize,
        /// Bytes the image needs.
        required: u64,
        /// The plane fd's size in bytes.
        plane_size: u64,
    },
    /// A plane's extent arithmetic overflowed while being checked.
    ///
    /// Unreachable with the `u32` offset/stride and `i32` dimension domains of the
    /// protocol (the sums are computed in `u64`); kept so that every checked
    /// operation has a typed failure instead of a wrap.
    Overflow {
        /// Zero-based plane index.
        plane: usize,
    },
    /// A plane fd's size could not be determined (`lseek` failed), so its extent
    /// cannot be validated.
    PlaneSizeUnavailable {
        /// Zero-based plane index.
        plane: usize,
        /// Why the size could not be read.
        error: String,
    },
}

impl fmt::Display for DmabufDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPlanes => f.write_str("no planes"),
            Self::EmptySize { width, height } => {
                write!(f, "degenerate buffer size {width}x{height}")
            }
            Self::ZeroStride { plane } => write!(f, "plane {plane} has a zero stride"),
            Self::PlaneOffsetBeyondSize {
                plane,
                offset,
                plane_size,
            } => write!(
                f,
                "plane {plane} offset {offset} is not inside its fd size {plane_size}"
            ),
            Self::PlaneTooShort {
                plane,
                offset,
                stride,
                plane_size,
            } => write!(
                f,
                "plane {plane} offset {offset} + stride {stride} leaves no row inside its fd size {plane_size}"
            ),
            Self::WholeBufferTooShort {
                plane,
                required,
                plane_size,
            } => write!(
                f,
                "plane {plane} needs {required} bytes (offset + stride * height) but its fd is {plane_size} bytes"
            ),
            Self::Overflow { plane } => {
                write!(f, "plane {plane} extent arithmetic overflowed")
            }
            Self::PlaneSizeUnavailable { plane, error } => {
                write!(f, "plane {plane} fd size is unavailable: {error}")
            }
        }
    }
}

/// Check a client's DMA-BUF descriptor before it reaches the renderer.
///
/// Every plane must have a non-zero stride, an offset inside its plane fd and room
/// for at least one row. A single-plane buffer that has no real modifier (Linear or
/// implicit) must additionally fit entirely inside its fd, mirroring
/// `PixmanRenderer`'s Linear-only `IncompleteBuffer` condition; a tiled, compressed
/// or vendor modifier makes the plane allocation's relationship to
/// `stride * height` driver-defined, so for those the whole-buffer rule is skipped
/// and a buffer the renderer can import is never false-rejected.
///
/// Pure: it reads the buffer and its plane fds (a dup'd `lseek` per plane for the
/// fd size) and never mutates anything. Returns the first structural defect found,
/// or `Ok(())` when the descriptor is safe to hand to a backend.
pub(crate) fn validate_dmabuf(dmabuf: &Dmabuf) -> Result<(), DmabufDefect> {
    let size = dmabuf.size();
    if dmabuf.num_planes() == 0 {
        return Err(DmabufDefect::NoPlanes);
    }
    if size.w <= 0 || size.h <= 0 {
        return Err(DmabufDefect::EmptySize {
            width: size.w,
            height: size.h,
        });
    }

    let single_plane = dmabuf.num_planes() == 1;
    let height = size.h as u64;

    for (plane, ((fd, offset), stride)) in dmabuf
        .handles()
        .zip(dmabuf.offsets())
        .zip(dmabuf.strides())
        .enumerate()
    {
        if stride == 0 {
            return Err(DmabufDefect::ZeroStride { plane });
        }

        let plane_size = match plane_fd_size(fd) {
            Ok(plane_size) => plane_size,
            Err(error) => {
                return Err(DmabufDefect::PlaneSizeUnavailable {
                    plane,
                    error: error.to_string(),
                })
            }
        };

        let offset_u64 = u64::from(offset);
        if offset_u64 >= plane_size {
            return Err(DmabufDefect::PlaneOffsetBeyondSize {
                plane,
                offset,
                plane_size,
            });
        }

        // At least one full row must fit behind the offset.
        let row = match offset_u64.checked_add(u64::from(stride)) {
            Some(row) => row,
            None => return Err(DmabufDefect::Overflow { plane }),
        };
        if row > plane_size {
            return Err(DmabufDefect::PlaneTooShort {
                plane,
                offset,
                stride,
                plane_size,
            });
        }

        // A linearly mapped single-plane image is read as one contiguous mapping, so
        // the whole buffer must fit — the same condition `PixmanRenderer` enforces
        // with its own `IncompleteBuffer` check. With a tiled/compressed/vendor
        // modifier the plane allocation's relationship to `stride * height` is
        // driver-defined (the modifier describes the layout, not a row-major
        // allocation), so only the universal per-plane checks above apply and a
        // buffer the renderer can import is never false-rejected.
        if single_plane && !dmabuf.has_modifier() {
            let required = match u64::from(stride)
                .checked_mul(height)
                .and_then(|bytes| offset_u64.checked_add(bytes))
            {
                Some(required) => required,
                None => return Err(DmabufDefect::Overflow { plane }),
            };
            if required > plane_size {
                return Err(DmabufDefect::WholeBufferTooShort {
                    plane,
                    required,
                    plane_size,
                });
            }
        }
    }

    Ok(())
}

/// The size in bytes of a plane's fd, read with a dup'd `lseek(fd, SEEK_END)`.
///
/// The fd is duplicated with [`BorrowedFd::try_clone_to_owned`] so the borrow is
/// released as soon as the size is known and no `unsafe` is needed; the buffer's
/// own fd is left untouched (only its shared file position is advanced, which
/// matters to nothing on a dma-buf: both backends map at a fixed offset).
fn plane_fd_size(fd: BorrowedFd<'_>) -> std::io::Result<u64> {
    let mut file = std::fs::File::from(fd.try_clone_to_owned()?);
    file.seek(std::io::SeekFrom::End(0))
}

/// A lazily formatted, human-readable description of a DMA-BUF's structure.
///
/// Constructed by [`describe_dmabuf`]; the per-plane `lseek` happens only when the
/// value is actually formatted, so a disabled log level costs nothing. Never
/// contains pixel payloads.
pub(crate) struct DmabufDescription<'a> {
    dmabuf: &'a Dmabuf,
}

impl fmt::Display for DmabufDescription<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format = self.dmabuf.format();
        let size = self.dmabuf.size();
        write!(
            f,
            "format=0x{:08x}({}) modifier=0x{:016x} planes={} size={}x{}",
            format.code as u32,
            format.code,
            u64::from(format.modifier),
            self.dmabuf.num_planes(),
            size.w,
            size.h,
        )?;

        for (plane, ((fd, offset), stride)) in self
            .dmabuf
            .handles()
            .zip(self.dmabuf.offsets())
            .zip(self.dmabuf.strides())
            .enumerate()
        {
            match plane_fd_size(fd) {
                Ok(plane_size) => write!(
                    f,
                    " | plane{plane} offset={offset} stride={stride} fd_size={plane_size}"
                )?,
                Err(error) => write!(
                    f,
                    " | plane{plane} offset={offset} stride={stride} fd_size=unavailable({error})"
                )?,
            }
        }

        Ok(())
    }
}

/// Describe a DMA-BUF's structure (format, size, plane count, per-plane offset,
/// stride and fd size) for logging. No pixel data, no allocation until formatted.
pub(crate) fn describe_dmabuf(dmabuf: &Dmabuf) -> DmabufDescription<'_> {
    DmabufDescription { dmabuf }
}

smithay::delegate_dmabuf!(State);

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::{
        dmabuf::{Dmabuf, DmabufFlags},
        Buffer, Fourcc, Modifier,
    };
    use std::{
        fs::File,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };

    use super::{describe_dmabuf, plane_fd_size, validate_dmabuf, DmabufDefect};

    /// A regular file of `len` bytes, handed over as a dmabuf plane fd.
    ///
    /// The file is unlinked immediately: the fd stays valid (and seekable) for as
    /// long as the `OwnedFd` lives, which the `Dmabuf` owns.
    fn plane_fd(len: u64) -> OwnedFd {
        let path = std::env::temp_dir().join(format!(
            "adesk-dmabuf-{}-{:?}.plane",
            std::process::id(),
            std::thread::current().id()
        ));
        let file = File::create(&path).expect("create plane file");
        file.set_len(len).expect("size plane file");
        let _ = std::fs::remove_file(&path);
        file.into()
    }

    /// A one-plane `Dmabuf` over a `fd_len`-byte file, `size` pixels at `stride`.
    fn single_plane(size: (i32, i32), stride: u32, offset: u32, fd_len: u64) -> Dmabuf {
        single_plane_with_modifier(size, stride, offset, fd_len, Modifier::Linear)
    }

    /// A one-plane `Dmabuf` with an explicit format modifier.
    fn single_plane_with_modifier(
        size: (i32, i32),
        stride: u32,
        offset: u32,
        fd_len: u64,
        modifier: Modifier,
    ) -> Dmabuf {
        let mut builder = Dmabuf::builder(size, Fourcc::Argb8888, modifier, DmabufFlags::empty());
        assert!(builder.add_plane(plane_fd(fd_len), 0, offset, stride));
        builder.build().expect("one plane builds a dmabuf")
    }

    #[test]
    fn well_formed_single_plane_is_accepted() {
        // 8x8 Argb8888 at 32 bytes per row needs 256 bytes.
        let dmabuf = single_plane((8, 8), 32, 0, 256);
        assert_eq!(validate_dmabuf(&dmabuf), Ok(()));

        // A trailing slack region behind the image is fine.
        let padded = single_plane((8, 8), 32, 0, 4096);
        assert_eq!(validate_dmabuf(&padded), Ok(()));
    }

    #[test]
    fn offset_beyond_the_plane_fd_is_rejected() {
        // offset > fd size is exactly the `fd_size - offset` underflow.
        let dmabuf = single_plane((8, 8), 32, 512, 256);
        assert_eq!(
            validate_dmabuf(&dmabuf),
            Err(DmabufDefect::PlaneOffsetBeyondSize {
                plane: 0,
                offset: 512,
                plane_size: 256,
            })
        );

        // An offset landing exactly on the end has no room for a row either.
        let at_end = single_plane((8, 8), 32, 256, 256);
        assert_eq!(
            validate_dmabuf(&at_end),
            Err(DmabufDefect::PlaneOffsetBeyondSize {
                plane: 0,
                offset: 256,
                plane_size: 256,
            })
        );
    }

    #[test]
    fn zero_stride_is_rejected() {
        let dmabuf = single_plane((8, 8), 0, 0, 256);
        assert_eq!(
            validate_dmabuf(&dmabuf),
            Err(DmabufDefect::ZeroStride { plane: 0 })
        );
    }

    #[test]
    fn degenerate_size_is_rejected() {
        // smithay's `Size` refuses negative dimensions when the buffer is built, so
        // a zero edge is the constructible degenerate case; the guard tests `<= 0`
        // so it also covers a negative size reaching it by another route.
        for size in [(0, 8), (8, 0), (0, 0)] {
            let dmabuf = single_plane(size, 32, 0, 4096);
            assert_eq!(
                validate_dmabuf(&dmabuf),
                Err(DmabufDefect::EmptySize {
                    width: size.0,
                    height: size.1,
                }),
                "size {size:?} must be rejected"
            );
        }
    }

    #[test]
    fn plane_without_room_for_one_row_is_rejected() {
        // The fd holds 16 bytes but the stride is 32: even one row does not fit.
        let dmabuf = single_plane((8, 8), 32, 0, 16);
        assert_eq!(
            validate_dmabuf(&dmabuf),
            Err(DmabufDefect::PlaneTooShort {
                plane: 0,
                offset: 0,
                stride: 32,
                plane_size: 16,
            })
        );
    }

    #[test]
    fn single_plane_buffer_larger_than_its_fd_is_rejected() {
        // 4 rows of 8 bytes in a 16-byte fd: one row fits, the image does not.
        let dmabuf = single_plane((8, 4), 8, 0, 16);
        assert_eq!(
            validate_dmabuf(&dmabuf),
            Err(DmabufDefect::WholeBufferTooShort {
                plane: 0,
                required: 32,
                plane_size: 16,
            })
        );
    }

    #[test]
    fn non_linear_single_plane_smaller_than_the_image_is_accepted() {
        // A tiled modifier makes the plane allocation's relationship to
        // `stride * height` driver-defined, so the whole-buffer rule must not apply:
        // 4 rows of 8 bytes (32 bytes) are declared over a 16-byte fd that still
        // holds one full row — the same shape the Linear test above rejects.
        let dmabuf = single_plane_with_modifier((8, 4), 8, 0, 16, Modifier::I915_y_tiled);
        assert!(
            dmabuf.has_modifier(),
            "the fixture must exercise a real modifier"
        );
        assert_eq!(validate_dmabuf(&dmabuf), Ok(()));
    }

    #[test]
    fn implicit_modifier_single_plane_still_gets_the_whole_buffer_check() {
        // `Invalid` means "implicit tiling", which PixmanRenderer still maps linearly,
        // so the whole-buffer rule keeps applying to it.
        let dmabuf = single_plane_with_modifier((8, 4), 8, 0, 16, Modifier::Invalid);
        assert!(!dmabuf.has_modifier());
        assert_eq!(
            validate_dmabuf(&dmabuf),
            Err(DmabufDefect::WholeBufferTooShort {
                plane: 0,
                required: 32,
                plane_size: 16,
            })
        );
    }

    #[test]
    fn non_linear_single_plane_still_enforces_the_per_plane_checks() {
        // An offset at the end of the fd is rejected whatever the modifier is.
        let beyond = single_plane_with_modifier((8, 4), 8, 16, 16, Modifier::I915_y_tiled);
        assert_eq!(
            validate_dmabuf(&beyond),
            Err(DmabufDefect::PlaneOffsetBeyondSize {
                plane: 0,
                offset: 16,
                plane_size: 16,
            })
        );

        // So is an offset that leaves no room for a full row.
        let too_short = single_plane_with_modifier((8, 4), 16, 8, 16, Modifier::I915_y_tiled);
        assert_eq!(
            validate_dmabuf(&too_short),
            Err(DmabufDefect::PlaneTooShort {
                plane: 0,
                offset: 8,
                stride: 16,
                plane_size: 16,
            })
        );
    }

    #[test]
    fn plane_whose_size_cannot_be_read_is_rejected() {
        // A socket fd is not seekable, so its size is unavailable — the same
        // condition that would make Smithay's `map_plane` fail.
        let (socket, _peer) = UnixStream::pair().expect("socket pair");
        let mut builder = Dmabuf::builder(
            (8, 8),
            Fourcc::Argb8888,
            Modifier::Linear,
            DmabufFlags::empty(),
        );
        assert!(builder.add_plane(socket.into(), 0, 0, 32));
        let dmabuf = builder.build().expect("one plane builds a dmabuf");

        match validate_dmabuf(&dmabuf) {
            Err(DmabufDefect::PlaneSizeUnavailable { plane, error }) => {
                assert_eq!(plane, 0);
                assert!(!error.is_empty());
            }
            other => panic!("expected an unavailable plane size, got {other:?}"),
        }
    }

    #[test]
    fn multi_plane_is_accepted_when_every_plane_fits() {
        let mut builder =
            Dmabuf::builder((4, 4), Fourcc::Nv12, Modifier::Linear, DmabufFlags::empty());
        assert!(builder.add_plane(plane_fd(64), 0, 0, 8));
        assert!(builder.add_plane(plane_fd(32), 1, 0, 4));
        let dmabuf = builder.build().expect("two planes build a dmabuf");
        assert_eq!(dmabuf.num_planes(), 2);
        // The whole-buffer check is single-plane only: each plane just needs a row.
        assert_eq!(validate_dmabuf(&dmabuf), Ok(()));
    }

    #[test]
    fn multi_plane_rejection_names_the_offending_plane() {
        let mut builder =
            Dmabuf::builder((4, 4), Fourcc::Nv12, Modifier::Linear, DmabufFlags::empty());
        assert!(builder.add_plane(plane_fd(64), 0, 0, 8));
        assert!(builder.add_plane(plane_fd(32), 1, 4096, 4));
        let dmabuf = builder.build().expect("two planes build a dmabuf");
        assert_eq!(
            validate_dmabuf(&dmabuf),
            Err(DmabufDefect::PlaneOffsetBeyondSize {
                plane: 1,
                offset: 4096,
                plane_size: 32,
            })
        );
    }

    #[test]
    fn description_reports_format_planes_and_stride() {
        let dmabuf = single_plane((8, 8), 32, 0, 256);
        let description = describe_dmabuf(&dmabuf).to_string();

        assert!(
            description.contains("format=0x34325241(AR24)"),
            "{description}"
        );
        assert!(
            description.contains("modifier=0x0000000000000000"),
            "{description}"
        );
        assert!(description.contains("planes=1"), "{description}");
        assert!(description.contains("size=8x8"), "{description}");
        assert!(
            description.contains("plane0 offset=0 stride=32 fd_size=256"),
            "{description}"
        );
    }

    #[test]
    fn description_covers_every_plane_and_reports_an_unreadable_fd() {
        let mut builder =
            Dmabuf::builder((4, 4), Fourcc::Nv12, Modifier::Linear, DmabufFlags::empty());
        let (socket, _peer) = UnixStream::pair().expect("socket pair");
        assert!(builder.add_plane(plane_fd(64), 0, 0, 8));
        assert!(builder.add_plane(socket.into(), 1, 8, 4));
        let dmabuf = builder.build().expect("two planes build a dmabuf");

        let description = describe_dmabuf(&dmabuf).to_string();
        assert!(description.contains("planes=2"), "{description}");
        assert!(
            description.contains("plane0 offset=0 stride=8 fd_size=64"),
            "{description}"
        );
        assert!(
            description.contains("plane1 offset=8 stride=4 fd_size=unavailable"),
            "{description}"
        );
    }

    #[test]
    fn every_defect_message_names_the_problem() {
        let messages = [
            (DmabufDefect::NoPlanes, "no planes"),
            (
                DmabufDefect::EmptySize {
                    width: 0,
                    height: 4,
                },
                "degenerate buffer size 0x4",
            ),
            (
                DmabufDefect::ZeroStride { plane: 2 },
                "plane 2 has a zero stride",
            ),
            (
                DmabufDefect::PlaneOffsetBeyondSize {
                    plane: 1,
                    offset: 8,
                    plane_size: 4,
                },
                "plane 1 offset 8 is not inside its fd size 4",
            ),
            (
                DmabufDefect::PlaneTooShort {
                    plane: 1,
                    offset: 0,
                    stride: 8,
                    plane_size: 4,
                },
                "plane 1 offset 0 + stride 8 leaves no row inside its fd size 4",
            ),
            (
                DmabufDefect::WholeBufferTooShort {
                    plane: 0,
                    required: 32,
                    plane_size: 16,
                },
                "plane 0 needs 32 bytes (offset + stride * height) but its fd is 16 bytes",
            ),
            (
                DmabufDefect::Overflow { plane: 3 },
                "plane 3 extent arithmetic overflowed",
            ),
            (
                DmabufDefect::PlaneSizeUnavailable {
                    plane: 0,
                    error: "Illegal seek".to_owned(),
                },
                "plane 0 fd size is unavailable: Illegal seek",
            ),
        ];

        for (defect, expected) in messages {
            let rendered = defect.to_string();
            assert_eq!(rendered, expected);
            assert!(!rendered.is_empty());
        }
    }

    #[test]
    fn validation_does_not_consume_the_plane_fds() {
        // `validate_dmabuf` takes `&Dmabuf` and must leave the descriptor usable
        // and describable afterwards.
        let dmabuf = single_plane((8, 8), 32, 0, 256);
        assert_eq!(validate_dmabuf(&dmabuf), Ok(()));
        assert_eq!(dmabuf.num_planes(), 1);
        assert_eq!(dmabuf.offsets().collect::<Vec<_>>(), vec![0]);
        assert_eq!(dmabuf.strides().collect::<Vec<_>>(), vec![32]);
        assert_eq!((dmabuf.size().w, dmabuf.size().h), (8, 8));

        let plane = dmabuf.handles().next().expect("one plane");
        assert_eq!(plane_fd_size(plane).expect("seek the plane fd"), 256);
    }
}
