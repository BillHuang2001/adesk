//! NDJSON line framing over any byte stream (`docs/viewer.md` §1).
//!
//! This is the **only** place the `\n` terminator and the line cap live. The VAP
//! codec (`adesk-viewer-proto`) turns messages into and out of strings; the
//! session and the client push those strings through [`LineReader`] / [`read_line`]
//! / [`write_line`]. Message contents are never logged here (pixel payloads must
//! never reach a log).

use std::io::IoSlice;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Result, ViewerError};

/// Default maximum length of one NDJSON line, in bytes (32 MiB).
///
/// The `docs/viewer.md` §1 cap: comfortably above any single encoded frame, and
/// the default both the [`ViewerServer`](crate::server::ViewerServer) session and
/// the [`ViewerClient`](crate::client::ViewerClient) use for both directions.
pub const DEFAULT_MAX_FRAME_LEN: usize = 32 * 1024 * 1024;

/// A cancellation-safe NDJSON line reader over a buffered byte stream.
///
/// This is the reader a `tokio::select!` loop must use. A free, buffer-clearing
/// read cannot be one arm of a `select!`: when another arm wins while the read is
/// pending, the partially-read line's bytes have already been consumed from the
/// stream but are discarded by the next call's `clear()`, so the rest of the line
/// decodes as a fragment and the stream desynchronizes. `LineReader` owns the
/// buffer and only ever discards a line once it has been *returned*, so a
/// cancelled read resumes exactly where it stopped — `BufRead::read_until` copies
/// every byte it consumes into the buffer before it yields, and a cancelled poll
/// leaves those bytes in place.
pub(crate) struct LineReader<R> {
    /// The buffered inbound byte stream.
    reader: R,
    /// The line being assembled, or the line last returned (see `complete`).
    buf: Vec<u8>,
    /// The inbound line cap, terminators included.
    max_len: usize,
    /// Whether `buf` holds a complete line that has already been handed to the
    /// caller and may be discarded by the next read. `false` while a line is
    /// being assembled — the state a cancelled read leaves behind.
    complete: bool,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    /// Wraps `reader`, enforcing `max_len` bytes per line (terminator included).
    pub(crate) fn new(reader: R, max_len: usize) -> LineReader<R> {
        LineReader {
            reader,
            buf: Vec::new(),
            max_len,
            complete: false,
        }
    }

    /// Reads the next line into the reader's reuse buffer (`docs/viewer.md` §1).
    ///
    /// `Ok(true)` means a complete line is available from [`LineReader::line`];
    /// `Ok(false)` is a clean EOF (no byte of a line was pending). A final line
    /// without a terminator (EOF in the middle of a line) is returned as-is.
    ///
    /// Cancellation-safe: every byte read so far is kept, so dropping the returned
    /// future and calling this again resumes the same line.
    ///
    /// # Errors
    ///
    /// Returns [`ViewerError::Io`] on a read failure, and
    /// [`ViewerError::Transport`] when the line exceeds `max_len` bytes or is not
    /// valid UTF-8.
    pub(crate) async fn read_line(&mut self) -> Result<bool> {
        if self.complete {
            // The caller is done with the line handed out last time.
            self.buf.clear();
            self.complete = false;
        }
        let read = self.reader.read_until(b'\n', &mut self.buf).await?;
        if read == 0 && self.buf.is_empty() {
            return Ok(false);
        }
        let max_len = self.max_len;
        if self.buf.len() > max_len {
            // The buffer is no longer a well-formed line; let the next read start
            // a fresh one.
            self.complete = true;
            return Err(ViewerError::Transport(format!(
                "inbound line of {} bytes exceeds the {max_len}-byte cap",
                self.buf.len()
            )));
        }
        if self.buf.last() == Some(&b'\n') {
            self.buf.pop();
        }
        if let Err(error) = std::str::from_utf8(&self.buf) {
            self.complete = true;
            return Err(ViewerError::Transport(format!(
                "inbound line is not valid UTF-8: {error}"
            )));
        }
        self.complete = true;
        Ok(true)
    }

    /// The raw bytes of the line [`LineReader::read_line`] returned last, without
    /// its terminator.
    ///
    /// Only meaningful after `read_line` returned `Ok(true)`.
    pub(crate) fn line(&self) -> &[u8] {
        &self.buf
    }
}

/// Reads one `\n`-terminated NDJSON line (`docs/viewer.md` §1).
///
/// Returns the line **without** its terminator, or `Ok(None)` on a clean EOF
/// (zero bytes were read). A final line without a terminator (EOF in the middle
/// of a line) is returned as-is. A line whose length — including the terminator —
/// exceeds `max_len` is rejected, as is a line that is not valid UTF-8.
///
/// This is a thin wrapper over `LineReader` for callers that want an owned
/// [`String`] and have no buffer to reuse; a `select!` loop must use `LineReader`
/// directly, because this function's future is not cancellation-safe (it drops its
/// buffer when cancelled).
///
/// # Errors
///
/// Returns [`ViewerError::Io`] on a read failure, and [`ViewerError::Transport`]
/// when the line exceeds `max_len` bytes or is not valid UTF-8.
pub async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_len: usize,
) -> Result<Option<String>> {
    let mut lines = LineReader::new(reader, max_len);
    if !lines.read_line().await? {
        return Ok(None);
    }
    // `LineReader` already rejected invalid UTF-8, so this cannot fail; the
    // error arm is kept for parity with the old implementation.
    String::from_utf8(lines.line().to_vec())
        .map(Some)
        .map_err(|error| {
            ViewerError::Transport(format!("inbound line is not valid UTF-8: {error}"))
        })
}

/// Writes one NDJSON line: `line`, then a single `\n`, then a flush
/// (`docs/viewer.md` §1).
///
/// The line and its terminator are offered to the writer in a **single vectored
/// write**; a short vectored write is completed in place, so the bytes emitted are
/// always exactly `line` followed by one `\n`.
///
/// # Errors
///
/// Returns [`ViewerError::Io`] if a write or the flush fails.
pub async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, line: &str) -> Result<()> {
    let bytes = line.as_bytes();
    // A short vectored write reports how many bytes it accepted; resubmit the
    // remainder (line tail, then the lone terminator) until all of `line + "\n"`
    // has been handed to the writer, so no byte is ever dropped.
    let mut written = 0usize;
    while written < bytes.len() + 1 {
        let accepted = writer
            .write_vectored(&[IoSlice::new(&bytes[written..]), IoSlice::new(b"\n")])
            .await?;
        if accepted == 0 {
            // Matches `AsyncWriteExt::write_all`, which reports a zero-length
            // write as `WriteZero` rather than looping forever.
            return Err(ViewerError::Io(std::io::ErrorKind::WriteZero.into()));
        }
        written += accepted;
    }
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use tokio::io::{AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn reads_lines_without_their_terminator() {
        let mut reader = BufReader::new(&b"one\ntwo\n"[..]);
        assert_eq!(
            read_line(&mut reader, 1024).await.unwrap(),
            Some("one".to_owned())
        );
        assert_eq!(
            read_line(&mut reader, 1024).await.unwrap(),
            Some("two".to_owned())
        );
        assert_eq!(read_line(&mut reader, 1024).await.unwrap(), None);
    }

    #[tokio::test]
    async fn blank_line_is_an_empty_string() {
        let mut reader = BufReader::new(&b"\n"[..]);
        assert_eq!(
            read_line(&mut reader, 8).await.unwrap(),
            Some(String::new())
        );
        assert_eq!(read_line(&mut reader, 8).await.unwrap(), None);
    }

    #[tokio::test]
    async fn final_line_without_terminator_is_returned() {
        let mut reader = BufReader::new(&b"tail"[..]);
        assert_eq!(
            read_line(&mut reader, 8).await.unwrap(),
            Some("tail".to_owned())
        );
    }

    #[tokio::test]
    async fn line_over_the_cap_is_a_transport_error() {
        // "1234\n" is exactly the cap and is accepted; one more byte is not.
        let mut reader = BufReader::new(&b"1234\n"[..]);
        assert_eq!(
            read_line(&mut reader, 5).await.unwrap(),
            Some("1234".to_owned())
        );

        let mut reader = BufReader::new(&b"12345\n"[..]);
        assert!(matches!(
            read_line(&mut reader, 5).await.unwrap_err(),
            ViewerError::Transport(_)
        ));
    }

    #[tokio::test]
    async fn invalid_utf8_is_a_transport_error() {
        let mut reader = BufReader::new(&[0xff, b'\n'][..]);
        assert!(matches!(
            read_line(&mut reader, 8).await.unwrap_err(),
            ViewerError::Transport(_)
        ));
    }

    #[tokio::test]
    async fn write_line_appends_exactly_one_terminator() {
        let mut out: Vec<u8> = Vec::new();
        write_line(&mut out, "hello").await.unwrap();
        write_line(&mut out, "").await.unwrap();
        assert_eq!(out, b"hello\n\n");
    }

    /// A read cancelled mid-line (the `select!` case) must keep the bytes it had
    /// already consumed, so the next read resumes the same line instead of
    /// returning a fragment. Dropping the future without the shared buffer is
    /// exactly what `select!` does to a losing arm.
    #[tokio::test]
    async fn a_cancelled_read_resumes_the_same_line() {
        let (mut peer, inbound) = tokio::io::duplex(64);
        let mut lines = LineReader::new(BufReader::new(inbound), 1024);

        // Half a line: no terminator yet, so the read can only park.
        peer.write_all(b"{\"type\":\"pointer_").await.unwrap();
        // Dropping the future without the shared buffer is exactly what `select!`
        // does to a losing arm.
        let _elapsed = tokio::time::timeout(Duration::from_millis(50), lines.read_line())
            .await
            .expect_err("a line without its terminator cannot complete");

        // The rest of the line arrives: the reader must reassemble it whole.
        peer.write_all(b"move\",\"x\":0.5}\n").await.unwrap();
        assert!(lines.read_line().await.unwrap());
        assert_eq!(lines.line(), b"{\"type\":\"pointer_move\",\"x\":0.5}");

        // The buffer is reused: the following line is read on its own.
        peer.write_all(b"next\n").await.unwrap();
        assert!(lines.read_line().await.unwrap());
        assert_eq!(lines.line(), b"next");
        peer.shutdown().await.unwrap();
        assert!(!lines.read_line().await.unwrap());
    }

    /// A line split across reads is reassembled even when the reads interleave
    /// with unrelated traffic, and a clean EOF after a partial line reports the
    /// partial line and then the end of the stream (not an empty line).
    #[tokio::test]
    async fn a_line_split_across_reads_is_reassembled() {
        let (mut peer, inbound) = tokio::io::duplex(64);
        let mut lines = LineReader::new(BufReader::new(inbound), 1024);

        peer.write_all(b"one two").await.unwrap();
        tokio::task::yield_now().await;
        peer.write_all(b" three\n").await.unwrap();
        assert!(lines.read_line().await.unwrap());
        assert_eq!(lines.line(), b"one two three");

        // EOF in the middle of a line: the remainder is returned as-is, and the
        // stream then ends rather than yielding an empty line.
        peer.write_all(b"tail").await.unwrap();
        peer.shutdown().await.unwrap();
        assert!(lines.read_line().await.unwrap());
        assert_eq!(lines.line(), b"tail");
        assert!(!lines.read_line().await.unwrap());
    }
}
