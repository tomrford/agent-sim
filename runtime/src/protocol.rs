use crate::sim::types::{SignalType, SignalValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("invalid duration: {0}")]
    InvalidDuration(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: Uuid,
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Action {
    Ping,
    Load {
        libpath: String,
    },
    Unload,
    Info,
    Signals,
    InstanceNew,
    InstanceList,
    InstanceSelect {
        index: u32,
    },
    InstanceReset {
        index: Option<u32>,
    },
    InstanceFree {
        index: u32,
    },
    Get {
        selectors: Vec<String>,
        instance: Option<u32>,
    },
    Set {
        writes: BTreeMap<String, String>,
        instance: Option<u32>,
    },
    TimeStart,
    TimePause,
    TimeStep {
        duration: String,
    },
    TimeSpeed {
        multiplier: Option<f64>,
    },
    TimeStatus,
    Watch {
        selector: String,
        interval_ms: u64,
        samples: Option<u32>,
        instance: Option<u32>,
    },
    RunRecipe {
        recipe: String,
        dry_run: bool,
        config: Option<String>,
    },
    SessionStatus,
    SessionList,
    Close,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: Uuid,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ResponseData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: Uuid, data: ResponseData) -> Self {
        Self {
            id,
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(id: Uuid, message: impl Into<String>) -> Self {
        Self {
            id,
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ResponseData {
    Ack,
    Loaded {
        libpath: String,
        signal_count: usize,
        instance_count: usize,
    },
    ProjectInfo {
        loaded: bool,
        libpath: Option<String>,
        tick_duration_us: Option<u32>,
        signal_count: usize,
        instance_count: usize,
        active_instance: Option<u32>,
    },
    Signals {
        signals: Vec<SignalData>,
    },
    Instances {
        instances: Vec<InstanceData>,
        active_instance: Option<u32>,
    },
    SelectedInstance {
        active_instance: u32,
    },
    SignalValues {
        instance: u32,
        values: Vec<SignalValueData>,
    },
    SetResult {
        instance: u32,
        writes_applied: usize,
    },
    TimeStatus {
        state: TimeStateData,
        elapsed_ticks: u64,
        elapsed_time_us: u64,
        speed: f64,
    },
    TimeAdvanced {
        requested_us: u64,
        advanced_ticks: u64,
        advanced_us: u64,
    },
    Speed {
        speed: f64,
    },
    WatchSamples {
        samples: Vec<WatchSampleData>,
    },
    RecipeResult {
        recipe: String,
        dry_run: bool,
        steps_executed: usize,
        events: Vec<String>,
    },
    SessionStatus {
        session: String,
        socket_path: String,
        running: bool,
    },
    SessionList {
        sessions: Vec<SessionInfoData>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalData {
    pub id: u32,
    pub name: String,
    pub signal_type: SignalType,
    pub units: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceData {
    pub index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalValueData {
    pub id: u32,
    pub name: String,
    pub signal_type: SignalType,
    pub value: SignalValue,
    pub units: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchSampleData {
    pub tick: u64,
    pub time_us: u64,
    pub instance: u32,
    pub signal: String,
    pub value: SignalValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeStateData {
    Paused,
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoData {
    pub name: String,
    pub socket_path: String,
    pub running: bool,
}

pub fn parse_duration_us(input: &str) -> Result<u64, ProtocolError> {
    let trimmed = input.trim();
    let (value_part, unit) = if let Some(v) = trimmed.strip_suffix("ms") {
        (v.trim(), "ms")
    } else if let Some(v) = trimmed.strip_suffix("us") {
        (v.trim(), "us")
    } else if let Some(v) = trimmed.strip_suffix('s') {
        (v.trim(), "s")
    } else {
        return Err(ProtocolError::InvalidDuration(trimmed.to_string()));
    };

    let value: f64 = value_part
        .parse()
        .map_err(|_| ProtocolError::InvalidDuration(trimmed.to_string()))?;
    if !value.is_finite() || value < 0.0 {
        return Err(ProtocolError::InvalidDuration(trimmed.to_string()));
    }

    let us = match unit {
        "s" => value * 1_000_000.0,
        "ms" => value * 1_000.0,
        "us" => value,
        _ => unreachable!(),
    };

    if us > u64::MAX as f64 {
        return Err(ProtocolError::InvalidDuration(trimmed.to_string()));
    }
    Ok(us.floor() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parser_handles_units() {
        assert_eq!(parse_duration_us("1s").expect("1s should parse"), 1_000_000);
        assert_eq!(
            parse_duration_us("250ms").expect("250ms should parse"),
            250_000
        );
        assert_eq!(parse_duration_us("500us").expect("500us should parse"), 500);
        assert_eq!(
            parse_duration_us("0.5s").expect("0.5s should parse"),
            500_000
        );
    }

    #[test]
    fn duration_parser_rejects_invalid_values() {
        assert!(parse_duration_us("abc").is_err());
        assert!(parse_duration_us("-1s").is_err());
        assert!(parse_duration_us("1m").is_err());
    }
}
