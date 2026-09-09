//! Binding and accepting the AGP Unix socket.

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use tokio::net::{UnixListener, UnixStream};

use crate::error::{Result, ServerError};

/// A bound AGP socket; removes its socket file when dropped.
#[derive(Debug)]
pub struct SocketListener {
    listener: UnixListener,
    path: PathBuf,
    /// Identity of the socket file this listener created, so `Drop` never
    /// unlinks a successor runtime's socket (see [`SocketFileId`]).
    identity: Option<SocketFileId>,
}

/// Identity of a socket file: the `(device, inode)` pair.
///
/// A path can be unlinked and rebound (a successor runtime on the same path),
/// so path equality is not enough to decide whether a socket file is still ours;
/// the inode is, because an unlinked-but-open socket keeps its inode allocated
/// while the listener lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SocketFileId {
    device: u64,
    inode: u64,
}

impl SocketFileId {
    /// The identity of whatever currently occupies `path`, or `None` when it
    /// cannot be stat'ed (absent path included).
    fn of(path: &Path) -> Option<SocketFileId> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        Some(SocketFileId {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl SocketListener {
    /// Binds `path` after [`prepare_socket_path`], and removes the socket file
    /// when dropped (best effort, and only while it is still the file this
    /// listener bound).
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Io`] when the parent directory cannot be created,
    /// the path is already a live socket, or `bind` fails.
    pub async fn bind(path: &Path) -> Result<SocketListener, ServerError> {
        prepare_socket_path(path)?;
        let listener = UnixListener::bind(path)?;
        Ok(SocketListener {
            listener,
            path: path.to_path_buf(),
            identity: SocketFileId::of(path),
        })
    }

    /// The bound path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accepts the next connection.
    ///
    /// # Errors
    ///
    /// Returns the underlying accept error; callers log and continue unless the
    /// runtime is shutting down.
    pub async fn accept(&self) -> io::Result<UnixStream> {
        self.listener.accept().await.map(|(stream, _addr)| stream)
    }
}

impl Drop for SocketListener {
    fn drop(&mut self) {
        // Only unlink the file this listener bound: between the runtime's
        // explicit removal and this drop, a successor runtime may have rebound
        // the same path, and its socket must survive.
        if self.identity.is_some() && SocketFileId::of(&self.path) == self.identity {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Makes `path` bindable: creates the parent directory, refuses to clobber a
/// live socket, and removes a stale socket file.
///
/// A socket file is considered stale when connecting to it fails — an existing
/// runtime keeps its socket alive, so a successful connect is reported as an
/// error rather than silently replaced.
///
/// # Errors
///
/// Returns [`ServerError::Io`] when the path exists and is live, or when it
/// exists and is not a socket.
pub fn prepare_socket_path(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(ServerError::Io(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", path.display()),
                )));
            }
            // A successful connect means a runtime is still serving this socket;
            // never clobber it. A failed connect means the file is stale.
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => Err(ServerError::Io(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "{} is a live socket; another runtime is already listening",
                        path.display()
                    ),
                ))),
                Err(_) => {
                    std::fs::remove_file(path)?;
                    Ok(())
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ServerError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_socket_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("nested").join("adesk.sock")
    }

    #[test]
    fn prepare_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_socket_path(&dir);
        prepare_socket_path(&path).unwrap();
        assert!(path.parent().unwrap().is_dir());
        assert!(!path.exists(), "prepare must not create the socket file");
    }

    #[test]
    fn prepare_removes_a_stale_socket_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adesk.sock");
        // A listener that is dropped leaves its socket file behind (stale).
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        drop(listener);
        assert!(path.exists());
        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_socket());

        prepare_socket_path(&path).unwrap();
        assert!(!path.exists(), "stale socket file must be removed");
    }

    #[test]
    fn prepare_refuses_a_live_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adesk.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

        let error = prepare_socket_path(&path).expect_err("live socket must not be clobbered");
        assert!(matches!(error, ServerError::Io(_)));
        assert!(path.exists(), "live socket file must be preserved");
    }

    #[test]
    fn prepare_refuses_a_non_socket_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adesk.sock");
        std::fs::write(&path, b"not a socket").unwrap();

        let error = prepare_socket_path(&path).expect_err("regular file must not be removed");
        assert!(matches!(error, ServerError::Io(_)));
        assert_eq!(std::fs::read(&path).unwrap(), b"not a socket");
    }

    #[tokio::test]
    async fn bind_accepts_a_connection_and_drop_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adesk.sock");
        let listener = SocketListener::bind(&path).await.unwrap();
        assert!(path.exists());
        assert_eq!(listener.path(), path.as_path());

        let client_path = path.clone();
        let client = tokio::spawn(async move { UnixStream::connect(&client_path).await });
        let server = listener.accept().await.unwrap();
        let client = client.await.unwrap().unwrap();
        assert!(server.peer_addr().is_ok() || client.peer_addr().is_ok());

        drop(listener);
        assert!(!path.exists(), "dropping the listener removes the socket file");
    }

    #[tokio::test]
    async fn bind_refuses_a_second_runtime_on_the_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adesk.sock");
        let _first = SocketListener::bind(&path).await.unwrap();
        let error = SocketListener::bind(&path).await.expect_err("second bind");
        assert!(matches!(error, ServerError::Io(_)));
    }

    #[test]
    fn socket_file_id_is_none_for_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(SocketFileId::of(&dir.path().join("absent.sock")), None);
    }

    #[tokio::test]
    async fn drop_keeps_a_socket_file_rebound_by_a_successor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adesk.sock");
        let first = SocketListener::bind(&path).await.unwrap();
        let first_id = SocketFileId::of(&path).expect("the bound socket file exists");

        // A successor runtime takes over the path: the old socket file is
        // unlinked and a new socket is bound there. The first listener is still
        // alive, so its inode stays allocated and cannot be reused — the two
        // identities differ deterministically.
        std::fs::remove_file(&path).unwrap();
        let second = SocketListener::bind(&path).await.unwrap();
        let second_id = SocketFileId::of(&path).expect("the rebound socket file exists");
        assert_ne!(first_id, second_id, "the successor bound a new socket file");

        drop(first);
        assert!(
            path.exists(),
            "dropping the stale listener must not unlink the successor's socket"
        );
        assert_eq!(
            SocketFileId::of(&path),
            Some(second_id),
            "the successor's socket file must be untouched"
        );

        drop(second);
        assert!(
            !path.exists(),
            "the owning listener still removes its own socket file"
        );
    }
}
