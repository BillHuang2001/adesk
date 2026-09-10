//! SHM buffer pool for the test client.
//!
//! `#![forbid(unsafe_code)]` rules out `memfd_create`/`mmap`, so the pool is backed by an
//! anonymous `tempfile::tempfile()` whose size is set with [`File::set_len`]. Pixels are
//! written with [`FileExt::write_all_at`] (positioned writes, no `unsafe`), and the file
//! descriptor is handed to `wl_shm.create_pool` through `AsFd::as_fd`.
//!
//! ## Pixel format and byte order
//!
//! Every buffer is committed as `wl_shm::Format::Argb8888`: on little-endian memory the
//! 32-bit word `A:R:G:B 8:8:8:8` is stored as the byte sequence
//! `[B, G, R, A]` (`[b, g, r, 255]` for an opaque RGBA colour `[r, g, b, 255]`). The
//! writer evaluates [`FillPattern::at`] in RGBA order and swaps the first and third byte
//! when serialising, which is the single place that byte-order rule lives. Stride is
//! always `width * 4` (no padding); an allocation is `stride * height` bytes rounded up
//! to a 64-byte multiple, and that rounded length — not the exact pixel payload — is what
//! the pool records as the range's size.
//!
//! ## Allocation
//!
//! The pool is a bump allocator with a free list: `alloc` first fits into the free list
//! (best effort, first fit), then bumps `next_offset`; the range is recorded in the pool's
//! live table under the new `wl_buffer`'s object id, and [`ShmPool::release`] — the handler
//! of `wl_buffer.release` — returns it to the free list, coalescing it with every touching
//! neighbour. A test that commits frames in a loop therefore reuses one allocation instead
//! of growing the pool.
//!
//! Attribution is by *identity*, never by slot: the compositor releases a superseded buffer
//! while it dispatches the superseding commit (so the window slot already holds the new
//! buffer), and a destroyed window's slot is deregistered before its last release arrives.
//! The live table is what can still answer "which range was that?", and a release it cannot
//! attribute is ignored — which is also what makes a re-attached buffer's second release
//! harmless.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;

use adesk_core::Size;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
use wayland_client::{Proxy, QueueHandle};

use crate::error::{Result, TestkitError};
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
    /// Free `(offset, len)` ranges returned by [`ShmPool::release`].
    free_list: Vec<(usize, usize)>,
    /// Bump pointer for allocations the free list cannot satisfy.
    next_offset: usize,
    /// Every live allocation, keyed by the `wl_buffer` object id that owns it: the range is
    /// removed here — and only here — when the compositor releases that buffer.
    live: HashMap<ObjectId, (usize, usize)>,
    /// Queue handle used to create `wl_buffer` objects for this pool.
    qhandle: QueueHandle<ClientState>,
}

/// A `wl_buffer` created by [`ShmPool::alloc`].
///
/// The bytes behind it live in the pool's live table, keyed by the buffer's object id:
/// dropping the handle only drops the client's proxy, the range is returned by
/// [`ShmPool::release`] when the compositor is done with it.
pub(crate) struct ShmBuffer {
    /// The protocol buffer object.
    buffer: wl_buffer::WlBuffer,
}

impl ShmPool {
    /// Creates a pool of `capacity` bytes and its `wl_shm_pool`.
    ///
    /// The backing file is a `tempfile::tempfile()` grown with `set_len`, and the pool is
    /// created with `shm.create_pool(file.as_fd(), capacity as i32, qhandle, ())`. The
    /// caller rounds `capacity` up to a multiple of 4096, so every allocation is page
    /// aligned; a zero capacity or one that does not fit the protocol's `i32` size is
    /// [`TestkitError::Unsupported`]. The file descriptor stays owned by the pool for the
    /// pool's whole life; `wl_shm` keeps its own reference.
    pub(crate) fn new(
        shm: &wl_shm::WlShm,
        qhandle: &QueueHandle<ClientState>,
        capacity: usize,
    ) -> Result<ShmPool> {
        if capacity == 0 {
            return Err(TestkitError::Unsupported(
                "SHM pool capacity must be greater than zero".to_string(),
            ));
        }
        let size = i32::try_from(capacity).map_err(|_| {
            TestkitError::Unsupported(format!(
                "SHM pool capacity {capacity} bytes exceeds the wl_shm i32 size limit"
            ))
        })?;
        let file = tempfile::tempfile()?;
        file.set_len(capacity as u64)?;
        let pool = shm.create_pool(file.as_fd(), size, qhandle, ());
        Ok(ShmPool {
            pool,
            file,
            capacity,
            free_list: Vec::new(),
            next_offset: 0,
            live: HashMap::new(),
            qhandle: qhandle.clone(),
        })
    }

    /// Allocates a buffer of `size` filled with `fill`.
    ///
    /// 1. `len = size.w * 4 * size.h`, rounded up to a 64-byte multiple; a zero-sized or
    ///    overflowing request, and a pool that cannot satisfy it, are
    ///    [`TestkitError`] (not a panic).
    /// 2. The first free range that fits is taken, else `next_offset` is bumped.
    /// 3. Pixels are written row by row with `FileExt::write_all_at`: for every `(x, y)`
    ///    `fill.at(x, y, size)` is evaluated and stored in the little-endian ARGB8888 byte
    ///    order `[b, g, r, 255]` (see the module docs). `fill.require_opaque()?` runs first,
    ///    so a translucent pattern fails here rather than producing pixels that depend on
    ///    compositing.
    /// 4. The buffer is created with `pool.create_buffer(offset as i32, w as i32, h as i32,
    ///    (w * 4) as i32, wl_shm::Format::Argb8888, &self.qhandle, ())`.
    /// 5. The range is recorded in the live table under the new buffer's object id, so the
    ///    `wl_buffer.release` for exactly this buffer is what returns it (see
    ///    [`release`](ShmPool::release)).
    pub(crate) fn alloc(&mut self, size: Size, fill: FillPattern) -> Result<ShmBuffer> {
        fill.require_opaque()?;
        if size.w == 0 || size.h == 0 {
            return Err(TestkitError::Unsupported(format!(
                "cannot allocate a zero-sized SHM buffer ({}x{})",
                size.w, size.h
            )));
        }
        let stride = (size.w as usize).checked_mul(4).ok_or_else(|| {
            TestkitError::Unsupported(format!("SHM stride overflows for width {}", size.w))
        })?;
        let len = stride
            .checked_mul(size.h as usize)
            .and_then(round_up_64)
            .ok_or_else(|| {
                TestkitError::Unsupported(format!(
                    "SHM buffer size overflows for {}x{}",
                    size.w, size.h
                ))
            })?;
        let offset = self.take_range(len)?;

        // Pixels go through positioned writes, one row at a time; the file is never mapped.
        let mut row = vec![0u8; stride];
        for y in 0..size.h {
            for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                let [r, g, b, _] = fill.at(x as u32, y, size);
                pixel.copy_from_slice(&[b, g, r, 255]);
            }
            self.file
                .write_all_at(&row, (offset + y as usize * stride) as u64)?;
        }

        let buffer = self.pool.create_buffer(
            offset as i32,
            size.w as i32,
            size.h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            &self.qhandle,
            (),
        );
        self.live.insert(buffer.id(), (offset, len));
        Ok(ShmBuffer { buffer })
    }

    /// Returns the range of the buffer `id` to the free list (the compositor released it).
    ///
    /// The `wl_buffer` object id is the only reliable handle on an allocation: `wl_buffer`
    /// carries no user data, the release for a superseded buffer arrives while the
    /// superseding commit is dispatched (so the window slot already holds the new buffer)
    /// and a destroyed window's slot is deregistered before its last release arrives, so no
    /// slot lookup can attribute a release. An id this pool never allocated — or already
    /// released — is ignored, which is what keeps a second release of the same buffer (it
    /// was re-attached by [`commit_pending`](super::TestWindow::commit_pending)) from
    /// handing the same range out twice.
    pub(crate) fn release(&mut self, id: &ObjectId) {
        if let Some((offset, len)) = self.live.remove(id) {
            self.free(offset, len);
        }
    }

    /// Returns a previously allocated range to the free list.
    ///
    /// `(offset, len)` is pushed and coalesced with every touching neighbour (one merge can
    /// make the next one adjacent). A range that [`alloc`](ShmPool::alloc) never handed
    /// out — `len == 0`, past the pool, past the bump pointer, or overlapping an already
    /// free range — is ignored: it indicates a bug in the caller, not in the pool. The only
    /// caller is [`release`](ShmPool::release), which takes the range from the live table,
    /// so a range is never returned twice.
    fn free(&mut self, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        let Some(mut end) = offset.checked_add(len) else {
            return;
        };
        // A range past the bump pointer (or past the pool) was never handed out; ignoring
        // it keeps the free list from claiming bytes `take_range` may still bump into.
        if end > self.capacity || end > self.next_offset {
            return;
        }
        let mut start = offset;
        // A range that overlaps an already free range was never handed out (or is being
        // freed twice); ignoring it keeps the free list free of duplicates.
        if self.free_list.iter().any(|&(free_start, free_len)| {
            start < free_start.saturating_add(free_len) && free_start < end
        }) {
            return;
        }
        // Coalesce with every touching neighbour: one merge can make the next one adjacent.
        loop {
            let neighbour = self.free_list.iter().position(|&(free_start, free_len)| {
                free_start.saturating_add(free_len) == start || end == free_start
            });
            match neighbour {
                Some(index) => {
                    let (free_start, free_len) = self.free_list.remove(index);
                    start = start.min(free_start);
                    end = end.max(free_start.saturating_add(free_len));
                }
                None => break,
            }
        }
        self.free_list.push((start, end - start));
    }

    /// Reserves `len` bytes: the first free range that fits, else a bump from `next_offset`.
    fn take_range(&mut self, len: usize) -> Result<usize> {
        if let Some(index) = self
            .free_list
            .iter()
            .position(|&(_, free_len)| free_len >= len)
        {
            let (offset, free_len) = self.free_list.remove(index);
            let remainder = free_len - len;
            if remainder > 0 {
                self.free_list.push((offset + len, remainder));
            }
            return Ok(offset);
        }
        let end = self
            .next_offset
            .checked_add(len)
            .filter(|&end| end <= self.capacity)
            .ok_or_else(|| self.exhausted(len))?;
        self.next_offset = end;
        Ok(end - len)
    }

    /// The error returned when an allocation does not fit in the pool.
    fn exhausted(&self, len: usize) -> TestkitError {
        TestkitError::Unsupported(format!(
            "SHM pool exhausted: {len} bytes requested, {} of {} bytes already handed out",
            self.next_offset, self.capacity
        ))
    }
}

impl ShmBuffer {
    /// The protocol buffer object.
    pub(crate) fn buffer(&self) -> &wl_buffer::WlBuffer {
        &self.buffer
    }
}

/// Rounds `len` up to a 64-byte multiple, or `None` when the addition overflows.
///
/// Every allocation starts on a 64-byte boundary, which is the alignment the compositor's
/// pixman and GL upload paths are happiest with, and keeps `free(offset, len)` able to
/// return exactly the reserved range.
fn round_up_64(len: usize) -> Option<usize> {
    len.checked_add(63).map(|len| len & !63)
}

/// The SHM formats the harness can write; `Argb8888` is the only one the client commits.
///
/// [`bind_globals`](super::protocol::bind_globals) uses this to seed the connect-time
/// [`Globals`](super::Globals) snapshot, which in turn seeds [`ClientState`].
pub(crate) fn supported_formats() -> HashSet<wl_shm::Format> {
    let mut formats = HashSet::new();
    formats.insert(wl_shm::Format::Argb8888);
    formats
}
