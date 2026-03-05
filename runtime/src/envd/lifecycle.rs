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

pub async fn ensure_env_running(env: &str) -> Result<(), EnvDaemonError> {
    EnvRegistry.ensure_running(env).await
}

pub async fn bootstrap_env_daemon(env_spec: &EnvSpec) -> Result<(), EnvDaemonError> {
    EnvRegistry.bootstrap(env_spec).await
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EnvRegistry;

impl EnvRegistry {
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
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
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
        let _ = std::fs::remove_file(&bootstrap_path);
        Err(EnvDaemonError::StartupTimeout)
    }

    fn spawn_env_daemon(
        self,
        env: &str,
        bootstrap_path: &Path,
    ) -> Result<std::process::Child, EnvDaemonError> {
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("--env-daemon")
            .arg("--env-name")
            .arg(env)
            .arg("--env-spec-path")
            .arg(bootstrap_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = command.spawn()?;
        Ok(child)
    }
}

async fn can_connect(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}
