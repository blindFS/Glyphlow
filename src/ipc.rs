//! The Unix socket protocol between the `glyphlow` server and its clients.
//!
//! A client sends one newline-terminated, JSON encoded [`AppSignal`] and gets no
//! reply; the server feeds it into [`crate::AppEngine::handle_signal`]. Path
//! resolution lives here rather than in each binary, so server and client always
//! agree on where the socket is.

use crate::AppSignal;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

pub const SOCKET_FILE_NAME: &str = "glyphlow.socket";

/// Directory holding the runtime files (socket, temporary editing file).
///
/// Honours `XDG_CACHE_HOME`, falling back to `$HOME/.cache`, and creates the
/// directory if it is missing.
pub fn cache_dir() -> Option<PathBuf> {
    let dir = cache_dir_path()?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir).ok()?;
    }
    Some(dir)
}

/// Path of the Unix socket, or `None` when no cache directory can be resolved.
///
/// Unlike [`cache_dir`] this never touches the filesystem, so a client probing
/// for a server that is not running leaves nothing behind.
pub fn socket_path() -> Option<PathBuf> {
    cache_dir_path().map(|dir| dir.join(SOCKET_FILE_NAME))
}

fn cache_dir_path() -> Option<PathBuf> {
    let xdg_cache_home = std::env::var("XDG_CACHE_HOME").ok();
    let home = std::env::var("HOME").ok();

    cache_dir_from(xdg_cache_home.as_deref(), home.as_deref())
}

/// The pure part of [`cache_dir_path`], split out so the precedence rule is
/// testable without mutating the process environment.
fn cache_dir_from(xdg_cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    xdg_cache_home
        .map(PathBuf::from)
        .or_else(|| home.map(|dir| PathBuf::from(dir).join(".cache")))
        .map(|base| base.join("glyphlow"))
}

#[derive(Debug)]
pub enum IpcError {
    /// Neither `HOME` nor `XDG_CACHE_HOME` is set.
    MissingSocketPath,
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    Serialize(serde_json::Error),
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

/// Fire-and-forget: the server never acknowledges, so `Ok` only means the
/// request reached the socket.
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// `XDG_CACHE_HOME` wins over `HOME`, and either base gets a `glyphlow`
    /// suffix. With neither set there is no cache directory to speak of.
    #[rstest]
    #[case::xdg_cache_home_wins(
        Some("/xdg/cache"),
        Some("/home/user"),
        Some("/xdg/cache/glyphlow")
    )]
    #[case::home_is_the_fallback(None, Some("/home/user"), Some("/home/user/.cache/glyphlow"))]
    #[case::neither_is_set(None, None, None)]
    fn cache_dir_prefers_xdg_cache_home(
        #[case] xdg_cache_home: Option<&str>,
        #[case] home: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(
            cache_dir_from(xdg_cache_home, home),
            expected.map(PathBuf::from)
        );
    }

    #[test]
    fn the_connect_error_names_the_socket_it_could_not_reach() {
        let connect = IpcError::Connect {
            path: PathBuf::from("/tmp/glyphlow.socket"),
            source: std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"),
        };

        assert!(
            connect.to_string().contains("/tmp/glyphlow.socket"),
            "the message must name the socket: {connect}"
        );
    }
}
