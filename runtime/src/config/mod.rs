pub mod error;
pub mod recipe;

use crate::config::error::ConfigError;
use crate::config::recipe::{FileConfig, RecipeDef, parse_config};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub file: FileConfig,
    pub source_path: Option<PathBuf>,
}

impl AppConfig {
    pub fn recipe(&self, name: &str) -> Result<&RecipeDef, ConfigError> {
        self.file
            .recipe
            .get(name)
            .ok_or_else(|| ConfigError::MissingRecipe(name.to_string()))
    }
}

/// Load config with first-match priority:
/// 1. Explicit path (`--config`)
/// 2. `AGENT_SIM_CONFIG` env var
/// 3. `./agent-sim.toml` in cwd
/// 4. Empty defaults
pub fn load_config(explicit_path: Option<&Path>) -> Result<AppConfig, ConfigError> {
    if let Some(path) = explicit_path {
        return load_single(path);
    }

    if let Ok(path) = std::env::var("AGENT_SIM_CONFIG") {
        return load_single(Path::new(&path));
    }

    let project_path = std::env::current_dir()?.join("agent-sim.toml");
    if project_path.exists() {
        return load_single(&project_path);
    }

    Ok(AppConfig::default())
}

fn load_single(path: &Path) -> Result<AppConfig, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let file = parse_config(&content)?;
    Ok(AppConfig {
        file,
        source_path: Some(path.to_path_buf()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::io::Write;

    #[test]
    #[serial]
    fn explicit_path_takes_priority() {
        let mut temp = tempfile::NamedTempFile::new().expect("temp config should be creatable");
        let content = r#"
[defaults]
json = true
speed = 2.0
"#;
        std::io::Write::write_all(&mut temp, content.as_bytes())
            .expect("temp config should be writable");

        let config = load_config(Some(temp.path())).expect("explicit config should load");
        let defaults = config
            .file
            .defaults
            .expect("defaults should be present in explicit config");
        assert_eq!(defaults.json, Some(true));
        assert_eq!(defaults.speed, Some(2.0));
        assert_eq!(config.source_path, Some(temp.path().to_path_buf()));
    }

    #[test]
    #[serial]
    fn env_var_used_when_no_explicit_path() {
        let sandbox = tempfile::tempdir().expect("sandbox tempdir should be creatable");
        let env_cfg_path = sandbox.path().join("env.toml");
        let mut env_cfg =
            std::fs::File::create(&env_cfg_path).expect("env config file should be creatable");
        env_cfg
            .write_all(
                br#"
[defaults]
json = true
speed = 3.0

[recipe.only_env]
steps = [{ step = "5ms" }]
"#,
            )
            .expect("env config should be writable");

        let env_guard = TestEnvGuard::new();
        set_env_var("AGENT_SIM_CONFIG", &env_cfg_path);

        let config = load_config(None).expect("env config should load");
        let defaults = config
            .file
            .defaults
            .expect("defaults should exist in env config");
        assert_eq!(defaults.json, Some(true));
        assert_eq!(defaults.speed, Some(3.0));
        assert!(config.file.recipe.contains_key("only_env"));
        assert_eq!(config.source_path, Some(env_cfg_path));

        env_guard.restore();
    }

    #[test]
    #[serial]
    fn project_local_config_loaded_from_cwd() {
        let sandbox = tempfile::tempdir().expect("sandbox tempdir should be creatable");
        let project_dir = sandbox.path().join("project");
        std::fs::create_dir_all(&project_dir).expect("project dir should be creatable");
        std::fs::write(
            project_dir.join("agent-sim.toml"),
            r#"
[defaults]
speed = 1.5

[recipe.check]
steps = [{ print = "*" }]
"#,
        )
        .expect("project config should be writable");

        let env_guard = TestEnvGuard::new();
        remove_env_var("AGENT_SIM_CONFIG");
        std::env::set_current_dir(&project_dir).expect("should change current dir to project");

        let config = load_config(None).expect("project config should load");
        let defaults = config
            .file
            .defaults
            .expect("defaults should exist in project config");
        assert_eq!(defaults.speed, Some(1.5));
        assert!(config.file.recipe.contains_key("check"));

        env_guard.restore();
    }

    #[test]
    #[serial]
    fn returns_empty_defaults_when_no_config_found() {
        let sandbox = tempfile::tempdir().expect("sandbox tempdir should be creatable");

        let env_guard = TestEnvGuard::new();
        remove_env_var("AGENT_SIM_CONFIG");
        std::env::set_current_dir(sandbox.path()).expect("should change cwd");

        let config = load_config(None).expect("should return empty defaults");
        assert!(config.file.defaults.is_none());
        assert!(config.file.recipe.is_empty());
        assert!(config.source_path.is_none());

        env_guard.restore();
    }

    struct TestEnvGuard {
        cwd: PathBuf,
        agent_sim_config: Option<OsString>,
    }

    impl TestEnvGuard {
        fn new() -> Self {
            Self {
                cwd: std::env::current_dir().expect("cwd should be readable"),
                agent_sim_config: std::env::var_os("AGENT_SIM_CONFIG"),
            }
        }

        fn restore(&self) {
            std::env::set_current_dir(&self.cwd).expect("cwd should restore");
            match &self.agent_sim_config {
                Some(v) => set_env_var("AGENT_SIM_CONFIG", v),
                None => remove_env_var("AGENT_SIM_CONFIG"),
            }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            self.restore();
        }
    }

    fn set_env_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
        // SAFETY: tests in this module are marked `#[serial]` to prevent concurrent
        // environment mutation across threads/processes while these variables are changed.
        unsafe { std::env::set_var(key, value) };
    }

    fn remove_env_var(key: &str) {
        // SAFETY: tests in this module are marked `#[serial]` to prevent concurrent
        // environment mutation across threads/processes while these variables are changed.
        unsafe { std::env::remove_var(key) };
    }
}
