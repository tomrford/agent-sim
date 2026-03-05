use crate::load::LoadSpec;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSpec {
    pub name: String,
    pub instances: Vec<EnvInstanceSpec>,
    #[serde(default)]
    pub can_buses: Vec<EnvCanBusSpec>,
    #[serde(default)]
    pub shared_channels: Vec<EnvSharedChannelSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvInstanceSpec {
    pub name: String,
    pub load_spec: LoadSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvCanBusSpec {
    pub name: String,
    pub vcan_iface: String,
    #[serde(default)]
    pub dbc_path: Option<String>,
    #[serde(default)]
    pub members: Vec<EnvCanBusMemberSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvCanBusMemberSpec {
    pub instance_name: String,
    pub bus_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSharedChannelSpec {
    pub name: String,
    pub writer_instance: String,
    #[serde(default)]
    pub members: Vec<EnvSharedChannelMemberSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSharedChannelMemberSpec {
    pub instance_name: String,
    pub channel_name: String,
}

pub fn read_env_spec(path: &Path) -> Result<EnvSpec, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read env spec '{}': {err}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|err| format!("invalid env spec json '{}': {err}", path.display()))
}

pub fn write_env_spec(path: &Path, spec: &EnvSpec) -> Result<(), String> {
    let content = serde_json::to_string(spec)
        .map_err(|err| format!("failed to serialize env spec '{}': {err}", path.display()))?;
    std::fs::write(path, content)
        .map_err(|err| format!("failed to write env spec '{}': {err}", path.display()))
}
