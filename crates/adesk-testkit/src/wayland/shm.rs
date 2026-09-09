//! SHM buffer pool for the test client.
//!
//! `#![forbid(unsafe_code)]` rules out `memfd_create`/`mmap`, so the pool is backed by an
//! anonymous `tempfile::tempfile()` whose size is set with [`File::set_len`]. Pixels are
//! written with [`FileExt::write_all_at`](std::os::unix::fs::FileExt::write_all_at) (positioned writes, no `unsafe`), and the file
//! descriptor is handed to `wl_shm.create_pool` through `AsFd::as_fd`.
//!
//! ## Pixel format and byte order
//!
//! Every buffer is committed as `wl_shm::Format::Argb8888`: on little-endian memory the
//! 32-bit word `A:R:G:B 8:8:8:8` is stored as the byte sequence
//! `[B, G, R, A]` (`[b, g, r, 255]` for an opaque RGBA colour `[r, g, b, 255]`). The
//! writer evaluates [`FillPattern::at`] in RGBA order and swaps the first and third byte
//! when serialising, which is the single place that byte-order rule lives. Stride is
//! always `width * 4` (no padding), so `len == stride * height`.
//!
//! ## Allocation
//!
//! The pool is a bump allocator with a free list: `alloc` first fits into the free list
//! (best effort, first fit), then bumps `next_offset`; `free` returns the range to the free
//! list (coalescing is optional in Phase 2). `wl_buffer.release` is what calls `free`, so a
//! test that commits frames in a loop reuses one allocation instead of growing the pool.

use std::collections::HashSet;
use std::fs::File;

use adesk_core::Size;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
use wayland_client::QueueHandle;

use crate::error::Result;
use crate::fill::FillPattern;

use super::state::ClientState;

/// An SHM pool backed by an anonymous temporary file.
pub(crate) struct ShmPool {
    /// The `wl_shm_pool` created from `file`.
    pool: wl_shm_pool::WlShmPool,
    /// The backing file; positioned writes go here, never through `mmap`.
    file: File,
    /// Total size of `file` in bytes.
    capacity: usize,
    /// Free `(offset, len)` ranges returned by `wl_buffer.release`.
    free_list: Vec<(usize, usize)>,
    /// Bump pointer for allocations the free list cannot satisfy.
    next_offset: usize,
    /// Queue handle used to create `wl_buffer` objects for this pool.
    qhandle: QueueHandle<ClientState>,
}

/// A `wl_buffer` plus the bookkeeping needed to return its bytes to the pool.
pub(crate) struct ShmBuffer {
    /// The protocol buffer object.
    buffer: wl_buffer::WlBuffer,
    /// Byte offset of the buffer inside the pool.
    pub(crate) offset: usize,
    /// Length of the buffer in bytes (`stride * height`).
    pub(crate) len: usize,
    /// Pixel size the buffer was allocated for.
    size: Size,
    /// Pattern written into the buffer.
    fill: FillPattern,
}

impl ShmPool {
    /// Creates a pool of `capacity` bytes and its `wl_shm_pool`.
    ///
    /// Phase 2 steps: `tempfile::tempfile()?` → `file.set_len(capacity as u64)?` →
    /// `shm.create_pool(file.as_fd(), capacity as i32, qhandle, ())`. `capacity` is rounded
    /// up to a multiple of 4096 by the caller so every allocation is page aligned. The
    /// file descriptor stays owned by the pool for the pool's whole life; `wl_shm` keeps
    /// its own reference.
    pub(crate) fn new(
        shm: &wl_shm::WlShm,
        qhandle: &QueueHandle<ClientState>,
        capacity: usize,
    ) -> Result<ShmPool> {
        let _ = (shm, qhandle, capacity);
        todo!("Phase 2: tempfile + set_len + wl_shm.create_pool(file.as_fd(), capacity)")
    }

    /// Allocates a buffer of `size` filled with `fill`.
    ///
    /// Phase 2 steps:
    ///
    /// 1. `len = size.w * 4 * size.h`, rounded up to a 64-byte multiple; `TestkitError`
    ///    (not a panic) when the pool is exhausted.
    /// 2. Take the first free range that fits, else bump `next_offset`.
    /// 3. Write the pixels row by row with `FileExt::write_all_at`: for every `(x, y)`
    ///    evaluate `fill.at(x, y, size)` and store the little-endian ARGB8888 byte order
    ///    `[b, g, r, 255]` (see the module docs). `fill.require_opaque()?` is checked
    ///    first, so a translucent pattern fails here rather than producing pixels that
    ///    depend on compositing.
    /// 4. `pool.create_buffer(offset as i32, w as i32, h as i32, (w * 4) as i32,
    ///    wl_shm::Format::Argb8888, &self.qhandle, ())`.
    pub(crate) fn alloc(&mut self, size: Size, fill: FillPattern) -> Result<ShmBuffer> {
        let _ = (size, fill);
        todo!("Phase 2: first-fit allocate, write the fill pattern, create the wl_buffer")
    }

    /// Returns a previously allocated range to the free list.
    ///
    /// Phase 2: push `(offset, len)` and coalesce with an adjacent free range when
    /// possible. Ranges not handed out by [`alloc`](ShmPool::alloc) are ignored (they
    /// would indicate a bug in the caller, not in the pool).
    pub(crate) fn free(&mut self, offset: usize, len: usize) {
        let _ = (offset, len);
        todo!("Phase 2: push the range back onto the free list and coalesce")
    }

    /// Total size of the backing file in bytes.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }
}

impl ShmBuffer {
    /// The protocol buffer object.
    pub(crate) fn buffer(&self) -> &wl_buffer::WlBuffer {
        &self.buffer
    }

    /// The pattern the buffer was filled with.
    pub(crate) fn fill(&self) -> FillPattern {
        self.fill
    }

    /// The pixel size the buffer was allocated for.
    pub(crate) fn size(&self) -> Size {
        self.size
    }
}

/// The SHM formats the harness can write; `Argb8888` is the only one the client commits.
///
/// Phase 2 uses this when seeding [`ClientState`] from the connect-time
/// [`Globals`](super::Globals) snapshot.
pub(crate) fn supported_formats() -> HashSet<wl_shm::Format> {
    let mut formats = HashSet::new();
    formats.insert(wl_shm::Format::Argb8888);
    formats
}
