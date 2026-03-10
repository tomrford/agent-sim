use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(not(any(unix, windows)))]
compile_error!("ipc is only implemented for Unix and Windows targets");

pub trait LocalStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> LocalStream for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub type BoxedLocalStream = Box<dyn LocalStream>;

pub struct LocalListener {
    inner: ListenerInner,
}

#[cfg(unix)]
type ListenerInner = tokio::net::UnixListener;

#[cfg(windows)]
struct ListenerInner {
    pipe_name: String,
    server: tokio::net::windows::named_pipe::NamedPipeServer,
}

impl LocalListener {
    pub fn bind(endpoint: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                inner: tokio::net::UnixListener::bind(endpoint)?,
            })
        }

        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;

            let pipe_name = pipe_name(endpoint);
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)?;
            Ok(Self {
                inner: ListenerInner { pipe_name, server },
            })
        }
    }

    pub async fn accept(&mut self) -> std::io::Result<BoxedLocalStream> {
        #[cfg(unix)]
        {
            let (stream, _) = self.inner.accept().await?;
            Ok(Box::new(stream))
        }

        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;

            self.inner.server.connect().await?;
            let next_server = ServerOptions::new().create(&self.inner.pipe_name)?;
            let connected = std::mem::replace(&mut self.inner.server, next_server);
            Ok(Box::new(connected))
        }
    }
}

pub async fn connect(endpoint: &Path) -> std::io::Result<BoxedLocalStream> {
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(endpoint).await?))
    }

    #[cfg(windows)]
    {
        use std::time::Duration;
        use tokio::net::windows::named_pipe::ClientOptions;
        use tokio::time::sleep;

        let pipe_name = pipe_name(endpoint);
        for attempt in 0..WINDOWS_PIPE_BUSY_RETRY_ATTEMPTS {
            match ClientOptions::new().open(&pipe_name) {
                Ok(client) => return Ok(Box::new(client)),
                Err(err)
                    if is_pipe_busy_error(&err)
                        && attempt + 1 < WINDOWS_PIPE_BUSY_RETRY_ATTEMPTS =>
                {
                    sleep(Duration::from_millis(WINDOWS_PIPE_BUSY_RETRY_DELAY_MS)).await;
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("windows pipe retry loop should always return");
    }
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
        .collect::<String>();
    format!(r"\\.\pipe\agent-sim-{sanitized}-{suffix:016x}")
}

#[cfg(windows)]
const WINDOWS_PIPE_BUSY: i32 = 231;
#[cfg(windows)]
const WINDOWS_PIPE_BUSY_RETRY_ATTEMPTS: u32 = 10;
#[cfg(windows)]
const WINDOWS_PIPE_BUSY_RETRY_DELAY_MS: u64 = 50;

#[cfg(windows)]
fn is_pipe_busy_error(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(WINDOWS_PIPE_BUSY)
}

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
    use super::{is_pipe_busy_error, pipe_name, stable_hash64};
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
    fn pipe_busy_detection_matches_windows_error_code() {
        assert!(is_pipe_busy_error(&std::io::Error::from_raw_os_error(231)));
        assert!(!is_pipe_busy_error(&std::io::Error::from_raw_os_error(5)));
    }
}
