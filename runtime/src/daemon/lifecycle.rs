use crate::daemon::error::DaemonError;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::sleep;

pub fn session_root() -> PathBuf {
    if let Some(path) = std::env::var_os("AGENT_SIM_HOME") {
        return PathBuf::from(path);
    }

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
    Err(DaemonError::NotRunning(session.to_string()))
}

pub async fn bootstrap_daemon(session: &str, libpath: &str) -> Result<(), DaemonError> {
    std::fs::create_dir_all(session_root())?;
    let socket = socket_path(session);
    if can_connect(&socket).await {
        return Err(DaemonError::AlreadyRunning(session.to_string()));
    }
    let mut child = spawn_daemon(session, libpath)?;

    let timeout = Duration::from_secs(5);
    let mut waited = Duration::from_millis(0);
    while waited < timeout {
        if can_connect(&socket).await {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let details = stderr.trim();
            let message = if details.is_empty() {
                format!("daemon exited with status {status}")
            } else {
                details.to_string()
            };
            return Err(DaemonError::StartupFailed(message));
        }
        sleep(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }
    Err(DaemonError::StartupTimeout)
}

fn spawn_daemon(session: &str, libpath: &str) -> Result<std::process::Child, DaemonError> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("--daemon")
        .arg("--session")
        .arg(session)
        .arg("--libpath")
        .arg(libpath)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    Ok(child)
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
