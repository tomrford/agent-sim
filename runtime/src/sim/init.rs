use crate::sim::types::{SignalValue, SimInitConfigRaw, SimInitEntryRaw};
use serde::{Deserialize, Serialize};
use std::ffi::CString;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitEntry {
    pub key: String,
    pub value: SignalValue,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InitConfig {
    #[serde(default)]
    pub entries: Vec<InitEntry>,
}

impl InitConfig {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub struct InitConfigRawScope {
    _keys: Vec<CString>,
    entries: Vec<SimInitEntryRaw>,
    config: Option<SimInitConfigRaw>,
}

impl InitConfigRawScope {
    pub fn new(config: &InitConfig) -> Result<Self, String> {
        if config.entries.is_empty() {
            return Ok(Self {
                _keys: Vec::new(),
                entries: Vec::new(),
                config: None,
            });
        }

        let mut keys = Vec::with_capacity(config.entries.len());
        let mut entries = Vec::with_capacity(config.entries.len());
        for entry in &config.entries {
            let key = CString::new(entry.key.as_str())
                .map_err(|_| format!("init config key '{}' contains interior NUL", entry.key))?;
            entries.push(SimInitEntryRaw {
                key: key.as_ptr(),
                value: entry.value.to_raw(),
            });
            keys.push(key);
        }
        let raw = SimInitConfigRaw {
            entries: entries.as_ptr(),
            count: entries.len() as u32,
        };
        Ok(Self {
            _keys: keys,
            entries,
            config: Some(raw),
        })
    }

    pub fn as_ptr(&self) -> *const SimInitConfigRaw {
        self.config
            .as_ref()
            .map_or(std::ptr::null(), |config| config as *const SimInitConfigRaw)
    }

    #[allow(dead_code)]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}
