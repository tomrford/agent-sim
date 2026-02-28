use crate::daemon::error::DaemonError;
use crate::daemon::lifecycle::{ensure_daemon_running, socket_path};
use crate::protocol::{Request, Response};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{sleep, timeout, Duration};

#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error(transparent)]
    Daemon(#[from] DaemonError),
    #[error("connection timeout")]
    Timeout,
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("response missing")]
    MissingResponse,
}

pub async fn send_request(session: &str, request: &Request) -> Result<Response, ConnectionError> {
    ensure_daemon_running(session).await?;
    let socket = socket_path(session);
    let payload = {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        line
    };

    let mut attempt = 0_u32;
    loop {
        match send_once(&socket, &payload).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                attempt += 1;
                if attempt >= 5 {
                    return Err(e);
                }
                sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn send_once(socket: &std::path::Path, payload: &str) -> Result<Response, ConnectionError> {
    let mut stream = timeout(Duration::from_secs(30), UnixStream::connect(socket))
        .await
        .map_err(|_| ConnectionError::Timeout)??;
    timeout(Duration::from_secs(5), stream.write_all(payload.as_bytes()))
        .await
        .map_err(|_| ConnectionError::Timeout)??;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    timeout(Duration::from_secs(30), reader.read_line(&mut line))
        .await
        .map_err(|_| ConnectionError::Timeout)??;
    if line.is_empty() {
        return Err(ConnectionError::MissingResponse);
    }
    let response = serde_json::from_str::<Response>(line.trim_end())?;
    Ok(response)
}
