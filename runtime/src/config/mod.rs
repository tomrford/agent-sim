pub mod error;
pub mod recipe;

use crate::config::error::ConfigError;
use crate::config::recipe::{parse_config, FileConfig, RecipeDef};
use std::collections::BTreeMap;
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

    let user_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agent-sim")
        .join("config.toml");
    if user_path.exists() {
        return load_single(&user_path);
    }

    Ok(AppConfig {
        file: FileConfig {
            defaults: None,
            recipe: BTreeMap::new(),
        },
        source_path: None,
    })
}

fn load_single(path: &Path) -> Result<AppConfig, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let file = parse_config(&content)?;
    Ok(AppConfig {
        file,
        source_path: Some(path.to_path_buf()),
    })
}
