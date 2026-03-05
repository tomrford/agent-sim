use crate::daemon::error::DaemonError;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::sleep;

#[derive(Debug, Clone, Default)]
pub struct DaemonBootstrap {
    pub libpath: String,
    pub env_tag: Option<String>,
    pub init_config_json: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SessionRegistry;

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

pub fn meta_path(session: &str) -> PathBuf {
    session_root().join(format!("{session}.meta"))
}

pub async fn ensure_daemon_running(session: &str) -> Result<(), DaemonError> {
    SessionRegistry.ensure_running(session).await
}

pub async fn bootstrap_daemon(
    session: &str,
    libpath: &str,
    env_tag: Option<&str>,
    init_config_json: Option<&str>,
) -> Result<(), DaemonError> {
    SessionRegistry
        .bootstrap(
            session,
            &DaemonBootstrap {
                libpath: libpath.to_string(),
                env_tag: env_tag.map(ToString::to_string),
                init_config_json: init_config_json.map(ToString::to_string),
            },
        )
        .await
}

impl SessionRegistry {
    pub async fn ensure_running(self, session: &str) -> Result<(), DaemonError> {
        std::fs::create_dir_all(session_root())?;
        let socket = socket_path(session);
        if can_connect(&socket).await {
            return Ok(());
        }
        Err(DaemonError::NotRunning(session.to_string()))
    }

    pub async fn bootstrap(
        self,
        session: &str,
        bootstrap: &DaemonBootstrap,
    ) -> Result<(), DaemonError> {
        std::fs::create_dir_all(session_root())?;
        let socket = socket_path(session);
        if can_connect(&socket).await {
            return Err(DaemonError::AlreadyRunning(session.to_string()));
        }
        let mut child = self.spawn_daemon(session, bootstrap)?;

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

    fn spawn_daemon(
        self,
        session: &str,
        bootstrap: &DaemonBootstrap,
    ) -> Result<std::process::Child, DaemonError> {
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("--daemon")
            .arg("--session")
            .arg(session)
            .arg("--libpath")
            .arg(&bootstrap.libpath)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(env_tag) = &bootstrap.env_tag {
            command.arg("--env-tag").arg(env_tag);
        }
        if let Some(init_config_json) = &bootstrap.init_config_json {
            command.arg("--init-config-json").arg(init_config_json);
        }
        let child = command.spawn()?;
        Ok(child)
    }

    pub async fn list_sessions(
        self,
    ) -> Result<Vec<(String, PathBuf, bool, Option<String>)>, DaemonError> {
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
            let env = read_env_tag(stem);
            out.push((stem.to_string(), path, running, env));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

pub async fn list_sessions() -> Result<Vec<(String, PathBuf, bool, Option<String>)>, DaemonError> {
    SessionRegistry.list_sessions().await
}

async fn can_connect(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

pub fn write_env_tag(session: &str, env: Option<&str>) -> Result<(), DaemonError> {
    let path = meta_path(session);
    if let Some(env) = env {
        std::fs::write(path, env)?;
    } else if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

pub fn read_env_tag(session: &str) -> Option<String> {
    let path = meta_path(session);
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn remove_env_tag(session: &str) {
    let path = meta_path(session);
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

pub fn read_pid(session: &str) -> Option<u32> {
    std::fs::read_to_string(pid_path(session))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
}

pub fn kill_pid(pid: u32) -> Result<(), DaemonError> {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        if result != 0 {
            return Err(DaemonError::Request(format!(
                "failed to kill pid {pid}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(DaemonError::Request(
            "pid kill fallback is not supported on this platform".to_string(),
        ))
    }
}
