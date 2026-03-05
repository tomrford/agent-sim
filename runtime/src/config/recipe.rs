use crate::config::error::ConfigError;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DefaultsConfig {
    pub json: Option<bool>,
    pub speed: Option<f64>,
    pub load: Option<LoadDefaults>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LoadDefaults {
    pub lib: Option<String>,
    #[serde(default)]
    pub flash: Vec<FlashBlockDef>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileConfig {
    pub defaults: Option<DefaultsConfig>,
    #[serde(default)]
    pub device: BTreeMap<String, DeviceDef>,
    #[serde(default)]
    pub recipe: BTreeMap<String, RecipeDef>,
    #[serde(default)]
    pub env: BTreeMap<String, EnvDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceDef {
    pub lib: String,
    #[serde(default)]
    pub flash: Vec<FlashBlockDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecipeDef {
    pub description: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub instances: Vec<String>,
    pub instance: Option<String>,
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
pub enum StepSpec {
    Duration(String),
    Detailed {
        duration: String,
        instance: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssertSpec {
    pub signal: String,
    pub instance: Option<String>,
    pub eq: Option<toml::Value>,
    pub gt: Option<f64>,
    pub lt: Option<f64>,
    pub gte: Option<f64>,
    pub lte: Option<f64>,
    pub approx: Option<f64>,
    pub tolerance: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvDef {
    #[serde(default)]
    pub instances: Vec<EnvInstance>,
    #[serde(default)]
    pub can: BTreeMap<String, EnvCanBus>,
    #[serde(default)]
    pub shared: BTreeMap<String, EnvSharedChannel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvInstance {
    pub name: String,
    pub lib: Option<String>,
    pub device: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashFileBlockDef {
    pub file: String,
    pub format: Option<String>,
    pub base: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FlashBlockDef {
    File(FlashFileBlockDef),
    InlineU32 { u32: u32, addr: String },
    InlineI32 { i32: i32, addr: String },
    InlineF32 { f32: f32, addr: String },
    InlineBool { bool: bool, addr: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvCanBus {
    #[serde(default)]
    pub members: Vec<String>,
    pub vcan: String,
    pub dbc: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvSharedChannel {
    #[serde(default)]
    pub members: Vec<String>,
    pub writer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetSpec {}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RecipeStep {
    Set {
        set: BTreeMap<String, toml::Value>,
        instance: Option<String>,
    },
    Step {
        step: StepSpec,
        instance: Option<String>,
    },
    Print {
        print: PrintSpec,
        instance: Option<String>,
    },
    Speed {
        speed: f64,
        instance: Option<String>,
    },
    Reset {
        reset: ResetSpec,
        instance: Option<String>,
    },
    Sleep {
        sleep: u64,
    },
    For {
        r#for: ForSpec,
        instance: Option<String>,
    },
    Assert {
        assert: AssertSpec,
    },
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
