pub mod error;
pub mod recipe;

use crate::config::error::ConfigError;
use crate::config::recipe::{FileConfig, RecipeDef, parse_config};
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

    let mut merged = empty_file_config();
    let mut source_path = None;

    let user_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("agent-sim")
        .join("config.toml");
    if user_path.exists() {
        let cfg = load_single_file(&user_path)?;
        merged = merged.merged_with(&cfg);
        source_path = Some(user_path);
    }

    let project_path = std::env::current_dir()?.join("agent-sim.toml");
    if project_path.exists() {
        let cfg = load_single_file(&project_path)?;
        merged = merged.merged_with(&cfg);
        source_path = Some(project_path);
    }

    if source_path.is_some() {
        return Ok(AppConfig {
            file: merged,
            source_path,
        });
    }

    Ok(AppConfig {
        file: empty_file_config(),
        source_path: None,
    })
}

fn load_single(path: &Path) -> Result<AppConfig, ConfigError> {
    let file = load_single_file(path)?;
    Ok(AppConfig {
        file,
        source_path: Some(path.to_path_buf()),
    })
}

fn load_single_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    parse_config(&content)
}

fn empty_file_config() -> FileConfig {
    FileConfig {
        defaults: None,
        recipe: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::recipe::{DefaultsConfig, LoadDefaults, RecipeDef, RecipeStep};

    #[test]
    fn file_config_merge_prefers_higher_precedence_values() {
        let mut low_recipe = BTreeMap::new();
        low_recipe.insert(
            "shared".to_string(),
            RecipeDef {
                description: Some("low".to_string()),
                steps: vec![RecipeStep::Step {
                    step: "10ms".to_string(),
                }],
            },
        );
        low_recipe.insert(
            "only_low".to_string(),
            RecipeDef {
                description: None,
                steps: vec![],
            },
        );

        let low = FileConfig {
            defaults: Some(DefaultsConfig {
                json: Some(false),
                speed: Some(1.0),
                load: Some(LoadDefaults {
                    lib: Some("low.so".to_string()),
                }),
            }),
            recipe: low_recipe,
        };

        let mut high_recipe = BTreeMap::new();
        high_recipe.insert(
            "shared".to_string(),
            RecipeDef {
                description: Some("high".to_string()),
                steps: vec![RecipeStep::Step {
                    step: "20ms".to_string(),
                }],
            },
        );

        let high = FileConfig {
            defaults: Some(DefaultsConfig {
                json: Some(true),
                speed: None,
                load: Some(LoadDefaults {
                    lib: Some("high.so".to_string()),
                }),
            }),
            recipe: high_recipe,
        };

        let merged = low.merged_with(&high);
        let defaults = merged.defaults.expect("defaults should exist");
        assert_eq!(defaults.json, Some(true));
        assert_eq!(defaults.speed, Some(1.0));
        assert_eq!(
            defaults.load.and_then(|v| v.lib),
            Some("high.so".to_string())
        );

        assert_eq!(
            merged
                .recipe
                .get("shared")
                .and_then(|r| r.description.clone()),
            Some("high".to_string())
        );
        assert!(merged.recipe.contains_key("only_low"));
    }

    #[test]
    fn load_single_is_used_when_explicit_path_is_set() {
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
}
