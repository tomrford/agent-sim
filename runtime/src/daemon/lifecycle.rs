use crate::daemon::error::DaemonError;
use crate::ipc;
use crate::load::{LoadSpec, write_load_spec};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRuntimeInfo {
    pub name: String,
    pub socket_path: PathBuf,
    pub running: bool,
    pub env: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct InstanceRegistry;

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

pub fn bootstrap_dir() -> PathBuf {
    session_root().join("bootstrap")
}

pub async fn ensure_daemon_running(session: &str) -> Result<(), DaemonError> {
    InstanceRegistry.ensure_running(session).await
}

pub async fn bootstrap_daemon(session: &str, load_spec: &LoadSpec) -> Result<(), DaemonError> {
    InstanceRegistry.bootstrap(session, load_spec).await
}

pub async fn bootstrap_daemon_with_exe(
    session: &str,
    load_spec: &LoadSpec,
    exe: &Path,
) -> Result<(), DaemonError> {
    InstanceRegistry
        .bootstrap_with_exe(session, load_spec, exe)
        .await
}

impl InstanceRegistry {
    pub async fn ensure_running(self, session: &str) -> Result<(), DaemonError> {
        std::fs::create_dir_all(session_root())?;
        let socket = socket_path(session);
        let retry_delay = Duration::from_millis(50);
        for attempt in 0..10 {
            if can_connect(&socket).await {
                return Ok(());
            }
            if attempt < 9 {
                sleep(retry_delay).await;
            }
        }
        Err(DaemonError::NotRunning(session.to_string()))
    }

    pub async fn bootstrap(self, session: &str, load_spec: &LoadSpec) -> Result<(), DaemonError> {
        let exe = std::env::current_exe()?;
        self.bootstrap_with_exe(session, load_spec, &exe).await
    }

    pub async fn bootstrap_with_exe(
        self,
        session: &str,
        load_spec: &LoadSpec,
        exe: &Path,
    ) -> Result<(), DaemonError> {
        std::fs::create_dir_all(session_root())?;
        std::fs::create_dir_all(bootstrap_dir())?;
        let socket = socket_path(session);
        if can_connect(&socket).await {
            return Err(DaemonError::AlreadyRunning(session.to_string()));
        }
        let bootstrap_path =
            bootstrap_dir().join(format!("{session}-{}.json", uuid::Uuid::new_v4()));
        write_load_spec(&bootstrap_path, load_spec)
            .map_err(|err| DaemonError::Request(err.to_string()))?;
        let mut child = match self.spawn_daemon_with_exe(exe, session, &bootstrap_path) {
            Ok(child) => child,
            Err(err) => {
                let _ = std::fs::remove_file(&bootstrap_path);
                return Err(err);
            }
        };

        let timeout = Duration::from_secs(5);
        let mut waited = Duration::from_millis(0);
        while waited < timeout {
            if pid_path(session).exists() && can_connect(&socket).await {
                let _ = std::fs::remove_file(&bootstrap_path);
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                let stderr = if let Some(mut pipe) = child.stderr.take() {
                    tokio::task::spawn_blocking(move || {
                        let mut stderr = String::new();
                        let _ = pipe.read_to_string(&mut stderr);
                        stderr
                    })
                    .await
                    .unwrap_or_else(|_| String::new())
                } else {
                    String::new()
                };
                let _ = std::fs::remove_file(&bootstrap_path);
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
        cleanup_bootstrap_timeout(&mut child, &bootstrap_path);
        Err(DaemonError::StartupTimeout)
    }

    fn spawn_daemon_with_exe(
        self,
        exe: &Path,
        session: &str,
        bootstrap_path: &Path,
    ) -> Result<std::process::Child, DaemonError> {
        let mut command = std::process::Command::new(exe);
        command
            .arg("__internal")
            .arg("instance-daemon")
            .arg("--instance")
            .arg(session)
            .arg("--load-spec-path")
            .arg(bootstrap_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = command.spawn()?;
        Ok(child)
    }

    pub async fn list_instances(self) -> Result<Vec<InstanceRuntimeInfo>, DaemonError> {
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
            out.push(InstanceRuntimeInfo {
                name: stem.to_string(),
                socket_path: path,
                running,
                env,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

pub async fn list_instances() -> Result<Vec<InstanceRuntimeInfo>, DaemonError> {
    InstanceRegistry.list_instances().await
}

fn cleanup_bootstrap_timeout(child: &mut std::process::Child, bootstrap_path: &Path) {
    let _ = std::fs::remove_file(bootstrap_path);
    let _ = child.kill();
    let _ = child.wait();
}

async fn can_connect(socket: &Path) -> bool {
    ipc::connect(socket).await.is_ok()
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
    crate::process::kill_pid(pid)
        .map_err(|err| DaemonError::Request(format!("failed to kill pid {pid}: {err}")))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::cleanup_bootstrap_timeout;
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    #[cfg(unix)]
    #[test]
    fn timeout_cleanup_kills_child_and_removes_bootstrap_file() {
        let bootstrap = tempfile::NamedTempFile::new().expect("temp bootstrap file");
        let bootstrap_path = bootstrap.path().to_path_buf();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep child should spawn");

        cleanup_bootstrap_timeout(&mut child, &bootstrap_path);

        assert!(
            !bootstrap_path.exists(),
            "bootstrap file should be removed during timeout cleanup"
        );
        assert!(
            child
                .try_wait()
                .expect("child status should be queryable")
                .is_some(),
            "timed-out child should be reaped"
        );
    }
}
