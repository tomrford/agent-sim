use crate::daemon::error::DaemonError;
use crate::daemon::lifecycle::{bootstrap_daemon, ensure_daemon_running, socket_path};
use crate::envd::error::EnvDaemonError;
use crate::envd::lifecycle::{ensure_env_running, socket_path as env_socket_path};
use crate::ipc;
use crate::protocol::{InstanceAction, Request, RequestAction, Response};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
        .send_to_endpoint(&socket_path(session), request)
        .await
}

pub async fn send_env_request(env: &str, request: &Request) -> Result<Response, ConnectionError> {
    EnvConnector.prepare(env).await?;
    RequestTransport::default()
        .send_to_endpoint(&env_socket_path(env), request)
        .await
}

#[derive(Debug, Clone, Copy)]
struct RequestTransport {
    max_attempts: u32,
    retry_delay_base: Duration,
    connect_timeout: Duration,
    write_timeout: Duration,
    read_timeout: Duration,
}

impl Default for RequestTransport {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            retry_delay_base: Duration::from_millis(100),
            connect_timeout: Duration::from_secs(2),
            write_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(30),
        }
    }
}

impl RequestTransport {
    async fn send_to_endpoint(
        self,
        endpoint: &std::path::Path,
        request: &Request,
    ) -> Result<Response, ConnectionError> {
        let payload = {
            let mut line = serde_json::to_string(request)?;
            line.push('\n');
            line
        };

        let mut attempt = 0_u32;
        loop {
            match self.send_once(endpoint, &payload).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    attempt += 1;
                    if attempt >= self.max_attempts || !self.should_retry(&err) {
                        return Err(err);
                    }
                    sleep(self.retry_delay_for_attempt(attempt)).await;
                }
            }
        }
    }

    fn should_retry(self, err: &ConnectionError) -> bool {
        match err {
            ConnectionError::Timeout => true,
            ConnectionError::Io(io_err) => matches!(
                io_err.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            ),
            ConnectionError::Daemon(_)
            | ConnectionError::EnvDaemon(_)
            | ConnectionError::Serde(_)
            | ConnectionError::MissingResponse => false,
        }
    }

    fn retry_delay_for_attempt(self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(3);
        self.retry_delay_base
            .checked_mul(1_u32 << shift)
            .unwrap_or(self.retry_delay_base)
    }

    async fn send_once(
        self,
        endpoint: &std::path::Path,
        payload: &str,
    ) -> Result<Response, ConnectionError> {
        let mut stream = timeout(self.connect_timeout, ipc::connect(endpoint))
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
            RequestAction::Instance(InstanceAction::Load { load_spec }) => {
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

#[cfg(test)]
mod tests {
    use super::{ConnectionError, RequestTransport};
    use std::io::ErrorKind;
    use tokio::time::Duration;

    #[test]
    fn request_transport_retries_only_transient_failures() {
        let transport = RequestTransport::default();
        assert!(transport.should_retry(&ConnectionError::Timeout));
        assert!(
            transport.should_retry(&ConnectionError::Io(std::io::Error::from(
                ErrorKind::ConnectionRefused
            )))
        );
        assert!(
            !transport.should_retry(&ConnectionError::Io(std::io::Error::from(
                ErrorKind::InvalidData
            )))
        );
        assert!(!transport.should_retry(&ConnectionError::MissingResponse));
    }

    #[test]
    fn request_transport_uses_bounded_exponential_backoff() {
        let transport = RequestTransport::default();
        assert_eq!(
            transport.retry_delay_for_attempt(1),
            Duration::from_millis(100)
        );
        assert_eq!(
            transport.retry_delay_for_attempt(2),
            Duration::from_millis(200)
        );
        assert_eq!(
            transport.retry_delay_for_attempt(3),
            Duration::from_millis(400)
        );
        assert_eq!(
            transport.retry_delay_for_attempt(4),
            Duration::from_millis(800)
        );
        assert_eq!(
            transport.retry_delay_for_attempt(5),
            Duration::from_millis(800)
        );
    }
}
