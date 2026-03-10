use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

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
        use tokio::net::windows::named_pipe::ClientOptions;

        Ok(Box::new(ClientOptions::new().open(pipe_name(endpoint))?))
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
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let raw = endpoint.to_string_lossy();
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    let suffix = hasher.finish();
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
