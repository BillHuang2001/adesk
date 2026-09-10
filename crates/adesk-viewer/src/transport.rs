//! NDJSON line framing over any byte stream (`docs/viewer.md` §1).
//!
//! This is the **only** place the `\n` terminator and the line cap live. The VAP
//! codec (`adesk-viewer-proto`) turns messages into and out of strings; the
//! session and the client push those strings through [`read_line`] /
//! [`write_line`]. Message contents are never logged here (pixel payloads must
//! never reach a log).

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Result, ViewerError};

/// Default maximum length of one NDJSON line, in bytes (32 MiB).
///
/// The `docs/viewer.md` §1 cap: comfortably above any single encoded frame, and
/// the default both the [`ViewerServer`](crate::server::ViewerServer) session and
/// the [`ViewerClient`](crate::client::ViewerClient) use for both directions.
pub const DEFAULT_MAX_FRAME_LEN: usize = 32 * 1024 * 1024;

/// Reads one `\n`-terminated NDJSON line (`docs/viewer.md` §1).
///
/// Returns the line **without** its terminator, or `Ok(None)` on a clean EOF
/// (zero bytes were read). A final line without a terminator (EOF in the middle
/// of a line) is returned as-is. A line whose length — including the terminator —
/// exceeds `max_len` is rejected, as is a line that is not valid UTF-8.
///
/// # Errors
///
/// Returns [`ViewerError::Io`] on a read failure, and [`ViewerError::Transport`]
/// when the line exceeds `max_len` bytes or is not valid UTF-8.
pub async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_len: usize,
) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let read = reader.read_until(b'\n', &mut buf).await?;
    if read == 0 {
        return Ok(None);
    }
    if buf.len() > max_len {
        return Err(ViewerError::Transport(format!(
            "inbound line of {} bytes exceeds the {max_len}-byte cap",
            buf.len()
        )));
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|error| {
        ViewerError::Transport(format!("inbound line is not valid UTF-8: {error}"))
    })
}

/// Writes one NDJSON line: `line`, then a single `\n`, then a flush
/// (`docs/viewer.md` §1).
///
/// # Errors
///
/// Returns [`ViewerError::Io`] if a write or the flush fails.
pub async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, line: &str) -> Result<()> {
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

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
}
