use crate::config::error::ConfigError;
use crate::sim::types::SignalValue;
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
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileConfig {
    pub defaults: Option<DefaultsConfig>,
    #[serde(default)]
    pub recipe: BTreeMap<String, RecipeDef>,
    #[serde(default)]
    pub env: BTreeMap<String, EnvDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecipeDef {
    pub description: Option<String>,
    pub env: Option<String>,
    #[serde(default)]
    pub sessions: Vec<String>,
    pub session: Option<String>,
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
        session: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssertSpec {
    pub signal: String,
    pub session: Option<String>,
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
    pub sessions: Vec<EnvSession>,
    #[serde(default)]
    pub can: BTreeMap<String, EnvCanBus>,
    #[serde(default)]
    pub shared: BTreeMap<String, EnvSharedChannel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvSession {
    pub name: String,
    pub lib: String,
    #[serde(default)]
    pub init: BTreeMap<String, toml::Value>,
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
        session: Option<String>,
    },
    Step {
        step: StepSpec,
        session: Option<String>,
    },
    Print {
        print: PrintSpec,
        session: Option<String>,
    },
    Speed {
        speed: f64,
        session: Option<String>,
    },
    Reset {
        reset: ResetSpec,
        session: Option<String>,
    },
    Sleep {
        sleep: u64,
    },
    For {
        r#for: ForSpec,
        session: Option<String>,
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

pub fn toml_value_to_signal_value(value: &toml::Value) -> Result<SignalValue, ConfigError> {
    match value {
        toml::Value::Boolean(v) => Ok(SignalValue::Bool(*v)),
        toml::Value::Integer(v) if *v >= 0 && *v <= u32::MAX as i64 => {
            Ok(SignalValue::U32(*v as u32))
        }
        toml::Value::Integer(v) if i32::try_from(*v).is_ok() => Ok(SignalValue::I32(*v as i32)),
        toml::Value::Integer(v) => Err(ConfigError::InvalidRecipeStep(format!(
            "integer init value {v} is outside supported i32/u32 range"
        ))),
        toml::Value::Float(v) => Ok(SignalValue::F64(*v)),
        _ => Err(ConfigError::InvalidRecipeStep(
            "unsupported init value type".to_string(),
        )),
    }
}
