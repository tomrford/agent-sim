use crate::config::error::ConfigError;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DefaultsConfig {
    pub json: Option<bool>,
    pub speed: Option<f64>,
    pub load: Option<LoadDefaults>,
}

impl DefaultsConfig {
    pub fn merged_with(&self, higher: &DefaultsConfig) -> DefaultsConfig {
        DefaultsConfig {
            json: higher.json.or(self.json),
            speed: higher.speed.or(self.speed),
            load: match (&self.load, &higher.load) {
                (Some(low), Some(high)) => Some(low.merged_with(high)),
                (None, Some(high)) => Some(high.clone()),
                (Some(low), None) => Some(low.clone()),
                (None, None) => None,
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LoadDefaults {
    pub lib: Option<String>,
}

impl LoadDefaults {
    pub fn merged_with(&self, higher: &LoadDefaults) -> LoadDefaults {
        LoadDefaults {
            lib: higher.lib.clone().or_else(|| self.lib.clone()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileConfig {
    pub defaults: Option<DefaultsConfig>,
    #[serde(default)]
    pub recipe: BTreeMap<String, RecipeDef>,
}

impl FileConfig {
    pub fn merged_with(&self, higher: &FileConfig) -> FileConfig {
        let defaults = match (&self.defaults, &higher.defaults) {
            (Some(low), Some(high)) => Some(low.merged_with(high)),
            (None, Some(high)) => Some(high.clone()),
            (Some(low), None) => Some(low.clone()),
            (None, None) => None,
        };

        let mut recipe = self.recipe.clone();
        for (k, v) in &higher.recipe {
            recipe.insert(k.clone(), v.clone());
        }

        FileConfig { defaults, recipe }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecipeDef {
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PrintSpec {
    All(String),
    Signals(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForSpec {
    pub signal: String,
    pub from: f64,
    pub to: f64,
    pub by: f64,
    pub each: Vec<RecipeStep>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RecipeStep {
    Set { set: BTreeMap<String, toml::Value> },
    Step { step: String },
    Print { print: PrintSpec },
    Speed { speed: f64 },
    Reset { reset: Option<bool> },
    InstanceNew { instance_new: Option<bool> },
    InstanceSelect { instance_select: u32 },
    Sleep { sleep: u64 },
    For { r#for: ForSpec },
}

pub fn parse_config(content: &str) -> Result<FileConfig, ConfigError> {
    toml::from_str(content).map_err(|e| ConfigError::Parse(e.to_string()))
}

pub fn toml_value_to_cli_string(value: &toml::Value) -> Result<String, ConfigError> {
    let rendered = match value {
        toml::Value::String(v) => v.clone(),
        toml::Value::Integer(v) => v.to_string(),
        toml::Value::Float(v) => v.to_string(),
        toml::Value::Boolean(v) => v.to_string(),
        _ => {
            return Err(ConfigError::InvalidRecipeStep(
                "unsupported set value type".to_string(),
            ));
        }
    };
    Ok(rendered)
}
