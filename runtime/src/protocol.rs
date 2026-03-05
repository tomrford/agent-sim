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
    Info,
    Signals,
    Reset,
    Get {
        selectors: Vec<String>,
    },
    Set {
        writes: BTreeMap<String, String>,
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
    CanBuses,
    CanAttach {
        bus_name: String,
        vcan_iface: String,
    },
    CanDetach {
        bus_name: String,
    },
    CanSend {
        bus_name: String,
        arb_id: u32,
        data_hex: String,
        flags: Option<u8>,
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
    },
    ProjectInfo {
        libpath: String,
        tick_duration_us: u32,
        signal_count: usize,
    },
    Signals {
        signals: Vec<SignalData>,
    },
    SignalValues {
        values: Vec<SignalValueData>,
    },
    SetResult {
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
    CanBuses {
        buses: Vec<CanBusData>,
    },
    CanSend {
        bus: String,
        arb_id: u32,
        len: u8,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanBusData {
    pub id: u32,
    pub name: String,
    pub bitrate: u32,
    pub bitrate_data: u32,
    pub fd_capable: bool,
    pub attached_iface: Option<String>,
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
    fn request_response_serde_roundtrip() {
        let request = Request {
            id: Uuid::new_v4(),
            action: Action::Set {
                writes: BTreeMap::from([
                    ("hvac.power".to_string(), "true".to_string()),
                    ("hvac.target_temp".to_string(), "21.5".to_string()),
                ]),
            },
        };
        let encoded_request =
            serde_json::to_string(&request).expect("request should serialize to json");
        let decoded_request: Request =
            serde_json::from_str(&encoded_request).expect("request should deserialize from json");
        match decoded_request.action {
            Action::Set { writes } => {
                assert_eq!(writes.len(), 2);
            }
            other => panic!("expected set action, got {other:?}"),
        }

        let response = Response::ok(request.id, ResponseData::SetResult { writes_applied: 2 });
        let encoded_response =
            serde_json::to_string(&response).expect("response should serialize to json");
        let decoded_response: Response =
            serde_json::from_str(&encoded_response).expect("response should deserialize from json");
        assert!(decoded_response.success);
        assert!(decoded_response.error.is_none());
    }

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
