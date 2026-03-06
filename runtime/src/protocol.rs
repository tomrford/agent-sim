use crate::load::LoadSpec;
use crate::sim::types::{SignalType, SignalValue, SimCanFrame};
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
        load_spec: LoadSpec,
    },
    Info,
    Signals,
    Reset,
    Get {
        selectors: Vec<String>,
    },
    Sample {
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
    WorkerCanBuses,
    WorkerStep {
        can_rx: Vec<CanBusFramesData>,
    },
    CanBuses,
    CanAttach {
        bus_name: String,
        vcan_iface: String,
    },
    CanDetach {
        bus_name: String,
    },
    CanLoadDbc {
        bus_name: String,
        path: String,
    },
    SharedList,
    SharedAttach {
        channel_name: String,
        path: String,
        writer: bool,
        writer_session: String,
    },
    SharedGet {
        channel_name: String,
    },
    CanSend {
        bus_name: String,
        arb_id: u32,
        data_hex: String,
        flags: Option<u8>,
    },
    EnvStatus {
        env: String,
    },
    EnvReset {
        env: String,
    },
    EnvTimeStart {
        env: String,
    },
    EnvTimePause {
        env: String,
    },
    EnvTimeStep {
        env: String,
        duration: String,
    },
    EnvTimeSpeed {
        env: String,
        multiplier: Option<f64>,
    },
    EnvTimeStatus {
        env: String,
    },
    EnvCanBuses {
        env: String,
    },
    EnvCanLoadDbc {
        env: String,
        bus_name: String,
        path: String,
    },
    EnvCanSend {
        env: String,
        bus_name: String,
        arb_id: u32,
        data_hex: String,
        flags: Option<u8>,
    },
    EnvCanInspect {
        env: String,
        bus_name: String,
    },
    EnvCanScheduleAdd {
        env: String,
        bus_name: String,
        job_id: Option<String>,
        arb_id: u32,
        data_hex: String,
        every: String,
        flags: Option<u8>,
    },
    EnvCanScheduleUpdate {
        env: String,
        job_id: String,
        arb_id: u32,
        data_hex: String,
        every: String,
        flags: Option<u8>,
    },
    EnvCanScheduleRemove {
        env: String,
        job_id: String,
    },
    EnvCanScheduleStop {
        env: String,
        job_id: String,
    },
    EnvCanScheduleStart {
        env: String,
        job_id: String,
    },
    EnvCanScheduleList {
        env: String,
        bus_name: Option<String>,
    },
    EnvClose {
        env: String,
    },
    InstanceStatus,
    InstanceList,
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
    SignalSample {
        tick: u64,
        time_us: u64,
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
    CanInspect {
        bus: String,
        frames: Vec<CanFrameData>,
    },
    CanSchedules {
        schedules: Vec<CanScheduleData>,
    },
    DbcLoaded {
        bus: String,
        signal_count: usize,
    },
    WorkerStep {
        can_tx: Vec<CanBusFramesData>,
    },
    SharedChannels {
        channels: Vec<SharedChannelData>,
    },
    SharedValues {
        channel: String,
        slots: Vec<SharedSlotValueData>,
    },
    WatchSamples {
        samples: Vec<WatchSampleData>,
    },
    RecipeResult {
        recipe: String,
        dry_run: bool,
        steps_executed: usize,
        steps: Vec<RecipeStepResultData>,
    },
    EnvStatus {
        env: String,
        running: bool,
        instance_count: usize,
        tick_duration_us: u32,
    },
    InstanceStatus {
        instance: String,
        socket_path: String,
        running: bool,
        env: Option<String>,
    },
    InstanceList {
        instances: Vec<InstanceInfoData>,
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
pub struct InstanceInfoData {
    pub name: String,
    pub socket_path: String,
    pub running: bool,
    pub env: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanBusFramesData {
    pub bus_name: String,
    pub frames: Vec<CanFrameWireData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanFrameWireData {
    pub arb_id: u32,
    pub len: u8,
    pub flags: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanFrameData {
    pub arb_id: u32,
    pub len: u8,
    pub flags: u8,
    pub data_hex: String,
}

impl From<SimCanFrame> for CanFrameWireData {
    fn from(value: SimCanFrame) -> Self {
        Self {
            arb_id: value.arb_id,
            len: value.len,
            flags: value.flags,
            data: value.payload().to_vec(),
        }
    }
}

impl From<&SimCanFrame> for CanFrameWireData {
    fn from(value: &SimCanFrame) -> Self {
        Self {
            arb_id: value.arb_id,
            len: value.len,
            flags: value.flags,
            data: value.payload().to_vec(),
        }
    }
}

impl TryFrom<CanFrameWireData> for SimCanFrame {
    type Error = String;

    fn try_from(value: CanFrameWireData) -> Result<Self, Self::Error> {
        if value.data.len() > 64 {
            return Err(format!(
                "CAN frame payload exceeds 64 bytes ({} bytes provided)",
                value.data.len()
            ));
        }
        let mut data = [0_u8; 64];
        data[..value.data.len()].copy_from_slice(&value.data);
        let len = usize::from(value.len);
        if len != value.data.len() {
            return Err(format!(
                "CAN frame length {} does not match payload size {}",
                value.len,
                value.data.len()
            ));
        }
        Ok(SimCanFrame {
            arb_id: value.arb_id,
            len: value.len,
            flags: value.flags,
            data,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanScheduleData {
    pub job_id: String,
    pub bus: String,
    pub arb_id: u32,
    pub data_hex: String,
    pub flags: u8,
    pub every_ticks: u64,
    pub next_due_tick: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedChannelData {
    pub id: u32,
    pub name: String,
    pub slot_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedSlotValueData {
    pub slot_id: u32,
    pub signal_type: SignalType,
    pub value: SignalValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeStepKindData {
    Set,
    Step,
    Print,
    Speed,
    Reset,
    Sleep,
    Assert,
    ForIteration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeStepResultData {
    pub kind: RecipeStepKindData,
    pub instance: Option<String>,
    pub detail: String,
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
