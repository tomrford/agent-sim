use crate::daemon::lifecycle::session_root;
use crate::envd::error::EnvDaemonError;
use crate::envd::spec::{EnvSpec, write_env_spec};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::sleep;

pub fn env_root() -> PathBuf {
    session_root().join("envs")
}

pub fn bootstrap_dir() -> PathBuf {
    env_root().join("bootstrap")
}

pub fn socket_path(env: &str) -> PathBuf {
    env_root().join(format!("{env}.sock"))
}

pub fn pid_path(env: &str) -> PathBuf {
    env_root().join(format!("{env}.pid"))
}

pub async fn list_envs() -> Result<Vec<(String, PathBuf, bool)>, EnvDaemonError> {
    EnvRegistry.list_envs().await
}

pub async fn ensure_env_running(env: &str) -> Result<(), EnvDaemonError> {
    EnvRegistry.ensure_running(env).await
}

pub async fn bootstrap_env_daemon(env_spec: &EnvSpec) -> Result<(), EnvDaemonError> {
    EnvRegistry.bootstrap(env_spec).await
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EnvRegistry;

impl EnvRegistry {
    pub async fn list_envs(self) -> Result<Vec<(String, PathBuf, bool)>, EnvDaemonError> {
        let root = env_root();
        std::fs::create_dir_all(&root)?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("sock") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let running = can_connect(&path).await;
            out.push((stem.to_string(), path, running));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    pub async fn ensure_running(self, env: &str) -> Result<(), EnvDaemonError> {
        std::fs::create_dir_all(env_root())?;
        let socket = socket_path(env);
        if can_connect(&socket).await {
            return Ok(());
        }
        Err(EnvDaemonError::NotRunning(env.to_string()))
    }

    pub async fn bootstrap(self, env_spec: &EnvSpec) -> Result<(), EnvDaemonError> {
        std::fs::create_dir_all(env_root())?;
        std::fs::create_dir_all(bootstrap_dir())?;
        let socket = socket_path(&env_spec.name);
        if can_connect(&socket).await {
            return Err(EnvDaemonError::AlreadyRunning(env_spec.name.clone()));
        }

        let bootstrap_path =
            bootstrap_dir().join(format!("{}-{}.json", env_spec.name, uuid::Uuid::new_v4()));
        write_env_spec(&bootstrap_path, env_spec).map_err(EnvDaemonError::Request)?;
        let mut child = self.spawn_env_daemon(&env_spec.name, &bootstrap_path)?;

        let timeout = Duration::from_secs(5);
        let mut waited = Duration::ZERO;
        while waited < timeout {
            if can_connect(&socket).await {
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
                return Err(EnvDaemonError::StartupFailed(if details.is_empty() {
                    format!("env daemon exited with status {status}")
                } else {
                    details.to_string()
                }));
            }
            sleep(Duration::from_millis(100)).await;
            waited += Duration::from_millis(100);
        }
        cleanup_bootstrap_timeout(&mut child, &bootstrap_path);
        Err(EnvDaemonError::StartupTimeout)
    }

    fn spawn_env_daemon(
        self,
        _env: &str,
        bootstrap_path: &Path,
    ) -> Result<std::process::Child, EnvDaemonError> {
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("__internal")
            .arg("env-daemon")
            .arg("--env-spec-path")
            .arg(bootstrap_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = command.spawn()?;
        Ok(child)
    }
}

fn cleanup_bootstrap_timeout(child: &mut std::process::Child, bootstrap_path: &Path) {
    let _ = std::fs::remove_file(bootstrap_path);
    let _ = child.kill();
    let _ = child.wait();
}

async fn can_connect(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::cleanup_bootstrap_timeout;
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
