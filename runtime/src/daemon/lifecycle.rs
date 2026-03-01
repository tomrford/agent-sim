use crate::daemon::error::DaemonError;
use crate::protocol::{Action, Request, ResponseData};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::sleep;
use uuid::Uuid;

pub fn session_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".agent-sim")
}

pub fn socket_path(session: &str) -> PathBuf {
    session_root().join(format!("{session}.sock"))
}

pub fn pid_path(session: &str) -> PathBuf {
    session_root().join(format!("{session}.pid"))
}

pub async fn ensure_daemon_running(session: &str) -> Result<(), DaemonError> {
    std::fs::create_dir_all(session_root())?;
    let socket = socket_path(session);
    if can_connect(&socket).await {
        return Ok(());
    }
    spawn_daemon(session)?;

    let timeout = Duration::from_secs(5);
    let mut waited = Duration::from_millis(0);
    while waited < timeout {
        if can_connect(&socket).await {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }
    Err(DaemonError::StartupTimeout)
}

fn spawn_daemon(session: &str) -> Result<(), DaemonError> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("--daemon")
        .arg("--session")
        .arg(session)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

async fn can_connect(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

pub async fn list_sessions() -> Result<Vec<(String, PathBuf, bool)>, DaemonError> {
    let root = session_root();
    std::fs::create_dir_all(&root)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("sock") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let running = can_connect(&path).await;
        out.push((stem.to_string(), path, running));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn session_request(action: Action) -> Request {
    Request {
        id: Uuid::new_v4(),
        action,
    }
}

pub fn is_ping_success(data: Option<ResponseData>) -> bool {
    matches!(data, Some(ResponseData::Ack))
}
