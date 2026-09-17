//! Inter-process communication between the `glyphlow` server and its clients.
//!
//! The server listens on a Unix domain socket inside the Glyphlow cache
//! directory. Clients such as `glyphlow-cli` connect to it and send a single
//! newline-terminated, JSON encoded [`AppSignal`], which the server decodes and
//! feeds into [`crate::AppEngine::handle_signal`].
//!
//! Keeping the path resolution here (instead of duplicating it in every binary)
//! guarantees the server and the client always agree on where the socket lives.

use crate::AppSignal;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Name of the socket file inside [`cache_dir`].
pub const SOCKET_FILE_NAME: &str = "glyphlow.socket";

/// Directory holding Glyphlow runtime files (the socket and the temporary
/// editing file).
///
/// Honours `XDG_CACHE_HOME`, falling back to `$HOME/.cache`. The directory is
/// created if it does not exist yet.
pub fn cache_dir() -> Option<PathBuf> {
    let dir = cache_dir_path()?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir).ok()?;
    }
    Some(dir)
}

/// Path of the Unix socket the server listens on.
///
/// Resolving this never touches the filesystem, so a client probing for a
/// server that is not running leaves nothing behind.
pub fn socket_path() -> Option<PathBuf> {
    cache_dir_path().map(|dir| dir.join(SOCKET_FILE_NAME))
}

fn cache_dir_path() -> Option<PathBuf> {
    std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|dir| PathBuf::from(dir).join(".cache"))
        })
        .map(|base| base.join("glyphlow"))
}

/// Errors that can occur while talking to the server.
#[derive(Debug)]
pub enum IpcError {
    /// Neither `HOME` nor `XDG_CACHE_HOME` is set.
    MissingSocketPath,
    /// The server is not listening on the socket.
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The signal could not be encoded as JSON.
    Serialize(serde_json::Error),
    /// The request could not be written to the socket.
    Write(std::io::Error),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::MissingSocketPath => write!(
                f,
                "could not determine the Glyphlow socket path, \
                 please set `HOME` or `XDG_CACHE_HOME`"
            ),
            IpcError::Connect { path, source } => write!(
                f,
                "could not connect to the Glyphlow server at {}: {source} \
                 (is `glyphlow` running?)",
                path.display()
            ),
            IpcError::Serialize(e) => write!(f, "could not encode the request: {e}"),
            IpcError::Write(e) => write!(f, "could not send the request: {e}"),
        }
    }
}

impl std::error::Error for IpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IpcError::Connect { source, .. } => Some(source),
            IpcError::Serialize(e) => Some(e),
            IpcError::Write(e) => Some(e),
            IpcError::MissingSocketPath => None,
        }
    }
}

/// Send a single [`AppSignal`] to the running server.
///
/// This is fire-and-forget: the server acknowledges nothing, so a successful
/// return only means the request was handed to the socket.
pub async fn send_signal(signal: &AppSignal) -> Result<(), IpcError> {
    let path = socket_path().ok_or(IpcError::MissingSocketPath)?;
    let mut stream = UnixStream::connect(&path)
        .await
        .map_err(|source| IpcError::Connect { path, source })?;

    let mut payload = serde_json::to_string(signal).map_err(IpcError::Serialize)?;
    payload.push('\n');

    stream
        .write_all(payload.as_bytes())
        .await
        .map_err(IpcError::Write)?;
    stream.flush().await.map_err(IpcError::Write)?;
    Ok(())
}
