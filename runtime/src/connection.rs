use crate::daemon::error::DaemonError;
use crate::daemon::lifecycle::{bootstrap_daemon, ensure_daemon_running, socket_path};
use crate::envd::error::EnvDaemonError;
use crate::envd::lifecycle::{ensure_env_running, socket_path as env_socket_path};
use crate::protocol::{Action, Request, Response};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{Duration, sleep, timeout};

#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error(transparent)]
    Daemon(#[from] DaemonError),
    #[error(transparent)]
    EnvDaemon(#[from] EnvDaemonError),
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
    SessionConnector.prepare(session, request).await?;
    RequestTransport::default()
        .send_to_socket(&socket_path(session), request)
        .await
}

pub async fn send_env_request(env: &str, request: &Request) -> Result<Response, ConnectionError> {
    EnvConnector.prepare(env).await?;
    RequestTransport::default()
        .send_to_socket(&env_socket_path(env), request)
        .await
}

#[derive(Debug, Clone, Copy)]
struct RequestTransport {
    max_attempts: u32,
    retry_delay: Duration,
    connect_timeout: Duration,
    write_timeout: Duration,
    read_timeout: Duration,
}

impl Default for RequestTransport {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            retry_delay: Duration::from_millis(200),
            connect_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
        }
    }
}

impl RequestTransport {
    async fn send_to_socket(
        self,
        socket: &std::path::Path,
        request: &Request,
    ) -> Result<Response, ConnectionError> {
        let payload = {
            let mut line = serde_json::to_string(request)?;
            line.push('\n');
            line
        };

        let mut attempt = 0_u32;
        loop {
            match self.send_once(socket, &payload).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    attempt += 1;
                    if attempt >= self.max_attempts {
                        return Err(err);
                    }
                    sleep(self.retry_delay).await;
                }
            }
        }
    }

    async fn send_once(
        self,
        socket: &std::path::Path,
        payload: &str,
    ) -> Result<Response, ConnectionError> {
        let mut stream = timeout(self.connect_timeout, UnixStream::connect(socket))
            .await
            .map_err(|_| ConnectionError::Timeout)??;
        timeout(self.write_timeout, stream.write_all(payload.as_bytes()))
            .await
            .map_err(|_| ConnectionError::Timeout)??;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        timeout(self.read_timeout, reader.read_line(&mut line))
            .await
            .map_err(|_| ConnectionError::Timeout)??;
        if line.is_empty() {
            return Err(ConnectionError::MissingResponse);
        }
        let response = serde_json::from_str::<Response>(line.trim_end())?;
        Ok(response)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct SessionConnector;

impl SessionConnector {
    async fn prepare(self, session: &str, request: &Request) -> Result<(), ConnectionError> {
        match &request.action {
            Action::Load { load_spec } => {
                bootstrap_daemon(session, load_spec).await?;
            }
            _ => ensure_daemon_running(session).await?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct EnvConnector;

impl EnvConnector {
    async fn prepare(self, env: &str) -> Result<(), ConnectionError> {
        ensure_env_running(env).await?;
        Ok(())
    }
}
