mod backend;
pub mod dbc;

use crate::sim::types::SimCanFrame;
use crate::sim::types::{
    CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RESERVED_MASK,
    CAN_FLAG_RTR,
};

#[derive(Debug)]
pub struct CanSocket {
    inner: backend::PlatformCanSocket,
}

impl CanSocket {
    pub fn open(
        iface: &str,
        bitrate: u32,
        bitrate_data: u32,
        fd_capable: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            inner: backend::PlatformCanSocket::open(iface, bitrate, bitrate_data, fd_capable)?,
        })
    }

    pub fn iface(&self) -> &str {
        self.inner.iface()
    }

    pub fn recv_all(&self) -> Result<Vec<SimCanFrame>, String> {
        self.inner.recv_all()
    }

    pub fn send(&self, frame: &SimCanFrame) -> Result<(), String> {
        self.inner.send(frame)
    }
}

pub fn parse_data_hex(raw: &str) -> Result<Vec<u8>, String> {
    let compact = raw
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect::<String>();
    if compact.len() % 2 != 0 {
        return Err(format!(
            "invalid CAN payload hex '{raw}': expected an even number of hex characters"
        ));
    }
    if compact.len() / 2 > 64 {
        return Err(format!(
            "invalid CAN payload hex '{raw}': payload exceeds 64 bytes"
        ));
    }
    let mut payload = Vec::with_capacity(compact.len() / 2);
    let bytes = compact.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let pair = format!("{}{}", bytes[idx] as char, bytes[idx + 1] as char);
        let value = u8::from_str_radix(&pair, 16)
            .map_err(|_| format!("invalid CAN payload hex '{raw}': bad byte '{pair}'"))?;
        payload.push(value);
        idx += 2;
    }
    Ok(payload)
}

pub fn validate_frame(bus_name: &str, fd_capable: bool, frame: &SimCanFrame) -> Result<(), String> {
    if (frame.flags & CAN_FLAG_RESERVED_MASK) != 0 {
        return Err(format!(
            "CAN frame for bus '{}' has reserved flag bits set",
            bus_name
        ));
    }
    if (frame.flags & CAN_FLAG_EXTENDED) != 0 {
        if frame.arb_id > 0x1FFF_FFFF {
            return Err(format!(
                "CAN frame for bus '{}' has invalid extended arbitration id 0x{:X}",
                bus_name, frame.arb_id
            ));
        }
    } else if frame.arb_id > 0x7FF {
        return Err(format!(
            "CAN frame for bus '{}' has invalid standard arbitration id 0x{:X}",
            bus_name, frame.arb_id
        ));
    }
    if frame.len > 64 {
        return Err(format!(
            "CAN frame for bus '{}' has invalid payload length {}",
            bus_name, frame.len
        ));
    }

    let fd_requested =
        (frame.flags & CAN_FLAG_FD) != 0 || (frame.flags & (CAN_FLAG_BRS | CAN_FLAG_ESI)) != 0;
    if fd_requested {
        if !fd_capable {
            return Err(format!(
                "CAN bus '{}' is classic-only and cannot carry FD frames",
                bus_name
            ));
        }
        if !matches!(frame.len, 0..=8 | 12 | 16 | 20 | 24 | 32 | 48 | 64) {
            return Err(format!(
                "CAN FD frame for bus '{}' has invalid length {}; valid lengths are 0-8,12,16,20,24,32,48,64",
                bus_name, frame.len
            ));
        }
        if (frame.flags & CAN_FLAG_RTR) != 0 {
            return Err(format!(
                "CAN FD frame for bus '{}' cannot set RTR flag",
                bus_name
            ));
        }
    } else if frame.len > 8 {
        return Err(format!(
            "classic CAN frame for bus '{}' has invalid length {}",
            bus_name, frame.len
        ));
    }

    Ok(())
}
