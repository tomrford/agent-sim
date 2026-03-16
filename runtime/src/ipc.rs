use std::path::Path;

use interprocess::local_socket::tokio::{Listener as TokioListener, Stream as TokioStream};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, ToFsName};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(not(any(unix, windows)))]
compile_error!("ipc is only implemented for Unix and Windows targets");

pub trait LocalStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> LocalStream for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub type BoxedLocalStream = Box<dyn LocalStream>;

pub struct LocalListener {
    inner: TokioListener,
}

impl LocalListener {
    pub fn bind(endpoint: &Path) -> std::io::Result<Self> {
        let name = endpoint_name(endpoint)?;
        let inner = ListenerOptions::new()
            .name(name)
            .try_overwrite(false)
            .create_tokio()?;
        Ok(Self { inner })
    }

    pub async fn accept(&mut self) -> std::io::Result<BoxedLocalStream> {
        let stream = self.inner.accept().await?;
        Ok(Box::new(stream))
    }
}

pub async fn bind_listener(endpoint: &Path) -> std::io::Result<LocalListener> {
    match LocalListener::bind(endpoint) {
        Ok(listener) => Ok(listener),
        Err(err) if is_bind_conflict(&err) && connect(endpoint).await.is_err() => {
            cleanup_endpoint(endpoint);
            LocalListener::bind(endpoint)
        }
        Err(err) => Err(err),
    }
}

pub async fn connect(endpoint: &Path) -> std::io::Result<BoxedLocalStream> {
    let name = endpoint_name(endpoint)?;
    Ok(Box::new(TokioStream::connect(name).await?))
}

pub fn cleanup_endpoint(endpoint: &Path) {
    if endpoint.exists() {
        let _ = std::fs::remove_file(endpoint);
    }
}

pub fn create_endpoint_marker(endpoint: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = endpoint;
        Ok(())
    }

    #[cfg(windows)]
    {
        std::fs::write(endpoint, [])
    }
}

fn is_bind_conflict(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::AlreadyExists
    )
}

fn endpoint_name(endpoint: &Path) -> std::io::Result<interprocess::local_socket::Name<'_>> {
    #[cfg(unix)]
    {
        endpoint.to_fs_name::<GenericFilePath>()
    }

    #[cfg(windows)]
    {
        pipe_name(endpoint).to_fs_name::<GenericFilePath>()
    }
}

#[cfg(windows)]
fn pipe_name(endpoint: &Path) -> String {
    let raw = endpoint.to_string_lossy();
    let suffix = stable_hash64(&raw);
    let stem = endpoint
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("endpoint");
    let sanitized = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(WINDOWS_PIPE_STEM_MAX_LEN)
        .collect::<String>();
    format!("{WINDOWS_PIPE_NAME_PREFIX}{sanitized}-{suffix:016x}")
}

#[cfg(windows)]
const WINDOWS_PIPE_NAME_PREFIX: &str = r"\\.\pipe\agent-sim-";
#[cfg(windows)]
const WINDOWS_PIPE_NAME_MAX_LEN: usize = 256;
#[cfg(windows)]
const WINDOWS_PIPE_HASH_SUFFIX_LEN: usize = 1 + 16;
#[cfg(windows)]
const WINDOWS_PIPE_STEM_MAX_LEN: usize =
    WINDOWS_PIPE_NAME_MAX_LEN - WINDOWS_PIPE_NAME_PREFIX.len() - WINDOWS_PIPE_HASH_SUFFIX_LEN;

#[cfg(windows)]
fn stable_hash64(raw: &str) -> u64 {
    // Use a fixed FNV-1a hash so pipe names remain stable across Rust upgrades.
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in raw.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::{
        WINDOWS_PIPE_NAME_MAX_LEN, WINDOWS_PIPE_STEM_MAX_LEN, pipe_name, stable_hash64,
    };
    #[cfg(windows)]
    use std::path::Path;

    #[cfg(windows)]
    #[test]
    fn stable_hash64_matches_known_fnv1a_values() {
        assert_eq!(stable_hash64(""), 0xcbf29ce484222325);
        assert_eq!(stable_hash64("agent-sim"), 0x529cc5bfe23c9fb0);
        assert_eq!(
            stable_hash64(r"C:/Users/alice/.agent-sim/demo.sock"),
            0x840f725602d6f670
        );
    }

    #[cfg(windows)]
    #[test]
    fn pipe_name_uses_stable_hash_suffix() {
        let endpoint = Path::new(r"C:/Users/alice/.agent-sim/demo.sock");
        assert_eq!(
            pipe_name(endpoint),
            r"\\.\pipe\agent-sim-demo-840f725602d6f670"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pipe_name_stem_length_is_bounded() {
        let long_stem = "a".repeat(WINDOWS_PIPE_STEM_MAX_LEN * 2);
        let endpoint = format!(r"C:/Users/alice/.agent-sim/{long_stem}.sock");
        let name = pipe_name(Path::new(&endpoint));
        assert!(
            name.len() <= WINDOWS_PIPE_NAME_MAX_LEN,
            "pipe name too long: {}",
            name.len()
        );
    }
}
