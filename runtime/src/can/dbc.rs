use crate::sim::types::{CAN_FLAG_EXTENDED, CAN_FLAG_FD, SimCanFrame};
use can_dbc::{ByteOrder, Dbc, MessageId, MultiplexIndicator, ValueType};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct DbcSignalDef {
    pub name: String,
    pub frame_key: u32,
    pub arb_id: u32,
    pub extended: bool,
    pub message_size: u8,
    pub start_bit: u64,
    pub size: u64,
    pub byte_order: ByteOrder,
    pub value_type: ValueType,
    pub factor: f64,
    pub offset: f64,
    pub min: f64,
    pub max: f64,
    pub unit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DbcBusOverlay {
    signals_by_name: HashMap<String, DbcSignalDef>,
}

impl DbcBusOverlay {
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read DBC '{}': {e}", path.display()))?;
        let dbc = Dbc::try_from(content.as_str())
            .map_err(|e| format!("failed to parse DBC '{}': {e}", path.display()))?;
        let mut signals_by_name = HashMap::new();

        for message in dbc.messages {
            let (arb_id, extended) = match message.id {
                MessageId::Standard(id) => (u32::from(id), false),
                MessageId::Extended(id) => (id, true),
            };
            let message_size = u8::try_from(message.size).map_err(|_| {
                format!(
                    "DBC message '{}' has invalid size {}; max supported is 255 bytes",
                    message.name, message.size
                )
            })?;
            if message_size > 64 {
                return Err(format!(
                    "DBC message '{}' size {} exceeds CAN FD max payload (64)",
                    message.name, message_size
                ));
            }

            for signal in message.signals {
                if signal.multiplexer_indicator != MultiplexIndicator::Plain {
                    return Err(format!(
                        "DBC signal '{}.{}' uses multiplexing, which is unsupported in this version",
                        message.name, signal.name
                    ));
                }
                let size = signal.size;
                if size == 0 || size > 64 {
                    return Err(format!(
                        "DBC signal '{}.{}' has invalid size {}",
                        message.name, signal.name, size
                    ));
                }
                let frame_key = frame_key(arb_id, extended);
                signals_by_name.insert(
                    signal.name.clone(),
                    DbcSignalDef {
                        name: signal.name,
                        frame_key,
                        arb_id,
                        extended,
                        message_size,
                        start_bit: signal.start_bit,
                        size,
                        byte_order: signal.byte_order,
                        value_type: signal.value_type,
                        factor: signal.factor,
                        offset: signal.offset,
                        min: signal.min,
                        max: signal.max,
                        unit: if signal.unit.is_empty() {
                            None
                        } else {
                            Some(signal.unit)
                        },
                    },
                );
            }
        }

        Ok(Self { signals_by_name })
    }

    pub fn signal(&self, name: &str) -> Option<&DbcSignalDef> {
        self.signals_by_name.get(name)
    }

    pub fn signal_names(&self) -> impl Iterator<Item = &String> {
        self.signals_by_name.keys()
    }
}

pub fn decode_signal(frame: &SimCanFrame, signal: &DbcSignalDef) -> Result<f64, String> {
    let raw = extract_raw(frame, signal)?;
    let numeric = match signal.value_type {
        ValueType::Signed => signed_from_raw(raw, signal.size) as f64,
        ValueType::Unsigned => raw as f64,
    };
    Ok(numeric * signal.factor + signal.offset)
}

pub fn encode_signal(
    frame: &mut SimCanFrame,
    signal: &DbcSignalDef,
    value: f64,
) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!(
            "invalid value for CAN signal '{}': {value}",
            signal.name
        ));
    }
    if value < signal.min || value > signal.max {
        return Err(format!(
            "value {value} is out of DBC range [{}, {}] for CAN signal '{}'",
            signal.min, signal.max, signal.name
        ));
    }
    if signal.factor == 0.0 {
        return Err(format!(
            "DBC signal '{}' has zero factor, cannot encode",
            signal.name
        ));
    }

    let raw_float = (value - signal.offset) / signal.factor;
    let raw_i64 = raw_float.round() as i64;
    let raw_u64 = match signal.value_type {
        ValueType::Signed => {
            let min = -(1_i128 << (signal.size - 1));
            let max = (1_i128 << (signal.size - 1)) - 1;
            let val = i128::from(raw_i64);
            if val < min || val > max {
                return Err(format!(
                    "encoded raw value {raw_i64} exceeds signed {}-bit range for signal '{}'",
                    signal.size, signal.name
                ));
            }
            let mask = if signal.size == 64 {
                u64::MAX
            } else {
                (1_u64 << signal.size) - 1
            };
            (raw_i64 as i128 as u64) & mask
        }
        ValueType::Unsigned => {
            if raw_i64 < 0 {
                return Err(format!(
                    "encoded raw value {raw_i64} is negative for unsigned signal '{}'",
                    signal.name
                ));
            }
            let max = if signal.size == 64 {
                u64::MAX
            } else {
                (1_u64 << signal.size) - 1
            };
            let raw = raw_i64 as u64;
            if raw > max {
                return Err(format!(
                    "encoded raw value {raw} exceeds unsigned {}-bit range for signal '{}'",
                    signal.size, signal.name
                ));
            }
            raw
        }
    };

    insert_raw(frame, signal, raw_u64)?;
    if signal.message_size > frame.len {
        frame.len = signal.message_size;
    }
    if frame.len > 8 {
        frame.flags |= CAN_FLAG_FD;
    } else {
        frame.flags &= !CAN_FLAG_FD;
    }
    if signal.extended {
        frame.flags |= CAN_FLAG_EXTENDED;
    } else {
        frame.flags &= !CAN_FLAG_EXTENDED;
    }
    Ok(())
}

pub fn frame_key_from_frame(frame: &SimCanFrame) -> u32 {
    frame_key(frame.arb_id, (frame.flags & CAN_FLAG_EXTENDED) != 0)
}

fn frame_key(arb_id: u32, extended: bool) -> u32 {
    if extended { arb_id | (1 << 31) } else { arb_id }
}

fn extract_raw(frame: &SimCanFrame, signal: &DbcSignalDef) -> Result<u64, String> {
    if signal.size > 64 {
        return Err(format!(
            "signal '{}' size {} exceeds 64 bits",
            signal.name, signal.size
        ));
    }
    match signal.byte_order {
        ByteOrder::LittleEndian => {
            let mut raw = 0_u64;
            for idx in 0..signal.size {
                let frame_bit = signal.start_bit + idx;
                let bit = get_bit(&frame.data, frame_bit)?;
                raw |= u64::from(bit) << idx;
            }
            Ok(raw)
        }
        ByteOrder::BigEndian => {
            let mut raw = 0_u64;
            let mut bit_pos = signal.start_bit as i64;
            for _ in 0..signal.size {
                let bit = get_bit(&frame.data, bit_pos as u64)?;
                raw = (raw << 1) | u64::from(bit);
                bit_pos = next_motorola_bit(bit_pos);
            }
            Ok(raw)
        }
    }
}

fn insert_raw(frame: &mut SimCanFrame, signal: &DbcSignalDef, raw: u64) -> Result<(), String> {
    match signal.byte_order {
        ByteOrder::LittleEndian => {
            for idx in 0..signal.size {
                let frame_bit = signal.start_bit + idx;
                let bit = ((raw >> idx) & 1) as u8;
                set_bit(&mut frame.data, frame_bit, bit)?;
            }
        }
        ByteOrder::BigEndian => {
            let mut bit_pos = signal.start_bit as i64;
            for idx in 0..signal.size {
                let shift = signal.size - 1 - idx;
                let bit = ((raw >> shift) & 1) as u8;
                set_bit(&mut frame.data, bit_pos as u64, bit)?;
                bit_pos = next_motorola_bit(bit_pos);
            }
        }
    }
    Ok(())
}

fn get_bit(data: &[u8; 64], index: u64) -> Result<u8, String> {
    let byte_index = usize::try_from(index / 8).map_err(|_| "bit index overflow".to_string())?;
    if byte_index >= data.len() {
        return Err(format!("bit index {index} is out of bounds"));
    }
    let bit_index = (index % 8) as u8;
    Ok((data[byte_index] >> bit_index) & 1)
}

fn set_bit(data: &mut [u8; 64], index: u64, bit: u8) -> Result<(), String> {
    let byte_index = usize::try_from(index / 8).map_err(|_| "bit index overflow".to_string())?;
    if byte_index >= data.len() {
        return Err(format!("bit index {index} is out of bounds"));
    }
    let mask = 1_u8 << (index % 8);
    if bit == 0 {
        data[byte_index] &= !mask;
    } else {
        data[byte_index] |= mask;
    }
    Ok(())
}

fn next_motorola_bit(current: i64) -> i64 {
    if current % 8 == 0 {
        current + 15
    } else {
        current - 1
    }
}

fn signed_from_raw(raw: u64, bits: u64) -> i64 {
    if bits == 0 {
        return 0;
    }
    if bits >= 64 {
        return raw as i64;
    }
    let sign_bit = 1_u64 << (bits - 1);
    if (raw & sign_bit) == 0 {
        raw as i64
    } else {
        (raw as i64) - ((1_u64 << bits) as i64)
    }
}
