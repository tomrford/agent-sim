use crate::sim::types::SimCanFrame;
use crate::sim::types::{CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RTR};
use peak_can_sys::{
    BYTE, DWORD, PEAK_BAUD_1M, PEAK_BAUD_125K, PEAK_BAUD_250K, PEAK_BAUD_500K, PEAK_BAUD_800K,
    PEAK_ERROR_ILLOPERATION, PEAK_ERROR_INITIALIZE, PEAK_ERROR_OK, PEAK_ERROR_QRCVEMPTY,
    PEAK_ERROR_QXMTFULL, PEAK_MESSAGE_BRS, PEAK_MESSAGE_ESI, PEAK_MESSAGE_EXTENDED,
    PEAK_MESSAGE_FD, PEAK_MESSAGE_RTR, PEAK_MESSAGE_STANDARD, PEAK_PCIBUS1, PEAK_PCIBUS2,
    PEAK_PCIBUS3, PEAK_PCIBUS4, PEAK_PCIBUS5, PEAK_PCIBUS6, PEAK_PCIBUS7, PEAK_PCIBUS8,
    PEAK_PCIBUS9, PEAK_PCIBUS10, PEAK_PCIBUS11, PEAK_PCIBUS12, PEAK_PCIBUS13, PEAK_PCIBUS14,
    PEAK_PCIBUS15, PEAK_PCIBUS16, PEAK_USBBUS1, PEAK_USBBUS2, PEAK_USBBUS3, PEAK_USBBUS4,
    PEAK_USBBUS5, PEAK_USBBUS6, PEAK_USBBUS7, PEAK_USBBUS8, PEAK_USBBUS9, PEAK_USBBUS10,
    PEAK_USBBUS11, PEAK_USBBUS12, PEAK_USBBUS13, PEAK_USBBUS14, PEAK_USBBUS15, PEAK_USBBUS16, Pcan,
    TPEAKMsg, TPEAKMsgFD, TPEAKTimestamp, WORD,
};

pub(crate) struct PlatformCanSocket {
    iface: String,
    channel: WORD,
    pcan: Pcan,
    fd_capable: bool,
}

impl std::fmt::Debug for PlatformCanSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformCanSocket")
            .field("iface", &self.iface)
            .field("channel", &self.channel)
            .field("fd_capable", &self.fd_capable)
            .finish()
    }
}

impl PlatformCanSocket {
    pub(crate) fn open(
        iface: &str,
        bitrate: u32,
        bitrate_data: u32,
        fd_capable: bool,
    ) -> Result<Self, String> {
        let channel = parse_channel(iface)?;
        let pcan = unsafe { Pcan::new("PCANBasic.dll") }
            .or_else(|_| unsafe { Pcan::new("PCANBasic") })
            .map_err(|err| format!("failed to load PCANBasic for '{iface}': {err}"))?;

        let status = if fd_capable {
            let profile = std::ffi::CString::new(fd_bitrate_profile(bitrate, bitrate_data)?)
                .expect("fd bitrate profile must not contain NUL");
            unsafe { pcan.CAN_InitializeFD(channel, profile.as_ptr() as *mut i8) }
        } else {
            let baud = classic_baudrate_constant(bitrate)?;
            unsafe { pcan.CAN_Initialize(channel, baud, 0, 0, 0) }
        };
        if status != PEAK_ERROR_OK {
            return Err(status_message(iface, "initialize", status));
        }

        Ok(Self {
            iface: iface.to_string(),
            channel,
            pcan,
            fd_capable,
        })
    }

    pub(crate) fn iface(&self) -> &str {
        &self.iface
    }

    pub(crate) fn recv_all(&self) -> Result<Vec<SimCanFrame>, String> {
        let mut frames = Vec::new();
        loop {
            let status = if self.fd_capable {
                let mut raw = TPEAKMsgFD {
                    ID: 0,
                    MSGTYPE: 0,
                    DLC: 0,
                    DATA: [0; 64],
                };
                let status = unsafe {
                    self.pcan
                        .CAN_ReadFD(self.channel, &mut raw, std::ptr::null_mut())
                };
                if status == PEAK_ERROR_OK {
                    frames.push(decode_fd_frame(raw)?);
                }
                status
            } else {
                let mut raw = TPEAKMsg {
                    ID: 0,
                    MSGTYPE: 0,
                    LEN: 0,
                    DATA: [0; 8],
                };
                let status = unsafe {
                    self.pcan.CAN_Read(
                        self.channel,
                        &mut raw,
                        std::ptr::null_mut::<TPEAKTimestamp>(),
                    )
                };
                if status == PEAK_ERROR_OK {
                    frames.push(decode_classic_frame(raw));
                }
                status
            };

            match status {
                PEAK_ERROR_OK => continue,
                PEAK_ERROR_QRCVEMPTY => break,
                other => return Err(status_message(&self.iface, "read", other)),
            }
        }
        Ok(frames)
    }

    pub(crate) fn send(&self, frame: &SimCanFrame) -> Result<(), String> {
        let status = if self.fd_capable && ((frame.flags & CAN_FLAG_FD) != 0 || frame.len > 8) {
            let mut raw = TPEAKMsgFD {
                ID: frame.arb_id,
                MSGTYPE: encode_message_type(frame.flags)?,
                DLC: fd_len_to_dlc(frame.len)?,
                DATA: frame.data,
            };
            unsafe { self.pcan.CAN_WriteFD(self.channel, &mut raw) }
        } else {
            let mut raw = TPEAKMsg {
                ID: frame.arb_id,
                MSGTYPE: encode_message_type(frame.flags)?,
                LEN: frame.len,
                DATA: {
                    let mut payload = [0_u8; 8];
                    payload[..usize::from(frame.len)]
                        .copy_from_slice(&frame.data[..usize::from(frame.len)]);
                    payload
                },
            };
            unsafe { self.pcan.CAN_Write(self.channel, &mut raw) }
        };

        match status {
            PEAK_ERROR_OK => Ok(()),
            PEAK_ERROR_QXMTFULL => Err(format!("PCAN transmit queue is full on '{}'", self.iface)),
            other => Err(status_message(&self.iface, "write", other)),
        }
    }
}

impl Drop for PlatformCanSocket {
    fn drop(&mut self) {
        let _ = unsafe { self.pcan.CAN_Uninitialize(self.channel) };
    }
}

fn parse_channel(iface: &str) -> Result<WORD, String> {
    let normalized = iface.trim().to_ascii_lowercase();
    let handle = match normalized.as_str() {
        "usb1" | "peak_usbbus1" => PEAK_USBBUS1,
        "usb2" | "peak_usbbus2" => PEAK_USBBUS2,
        "usb3" | "peak_usbbus3" => PEAK_USBBUS3,
        "usb4" | "peak_usbbus4" => PEAK_USBBUS4,
        "usb5" | "peak_usbbus5" => PEAK_USBBUS5,
        "usb6" | "peak_usbbus6" => PEAK_USBBUS6,
        "usb7" | "peak_usbbus7" => PEAK_USBBUS7,
        "usb8" | "peak_usbbus8" => PEAK_USBBUS8,
        "usb9" | "peak_usbbus9" => PEAK_USBBUS9,
        "usb10" | "peak_usbbus10" => PEAK_USBBUS10,
        "usb11" | "peak_usbbus11" => PEAK_USBBUS11,
        "usb12" | "peak_usbbus12" => PEAK_USBBUS12,
        "usb13" | "peak_usbbus13" => PEAK_USBBUS13,
        "usb14" | "peak_usbbus14" => PEAK_USBBUS14,
        "usb15" | "peak_usbbus15" => PEAK_USBBUS15,
        "usb16" | "peak_usbbus16" => PEAK_USBBUS16,
        "pci1" | "peak_pcibus1" => PEAK_PCIBUS1,
        "pci2" | "peak_pcibus2" => PEAK_PCIBUS2,
        "pci3" | "peak_pcibus3" => PEAK_PCIBUS3,
        "pci4" | "peak_pcibus4" => PEAK_PCIBUS4,
        "pci5" | "peak_pcibus5" => PEAK_PCIBUS5,
        "pci6" | "peak_pcibus6" => PEAK_PCIBUS6,
        "pci7" | "peak_pcibus7" => PEAK_PCIBUS7,
        "pci8" | "peak_pcibus8" => PEAK_PCIBUS8,
        "pci9" | "peak_pcibus9" => PEAK_PCIBUS9,
        "pci10" | "peak_pcibus10" => PEAK_PCIBUS10,
        "pci11" | "peak_pcibus11" => PEAK_PCIBUS11,
        "pci12" | "peak_pcibus12" => PEAK_PCIBUS12,
        "pci13" | "peak_pcibus13" => PEAK_PCIBUS13,
        "pci14" | "peak_pcibus14" => PEAK_PCIBUS14,
        "pci15" | "peak_pcibus15" => PEAK_PCIBUS15,
        "pci16" | "peak_pcibus16" => PEAK_PCIBUS16,
        _ => {
            return Err(format!(
                "unsupported PEAK CAN channel '{iface}'; use usb1..usb16 or pci1..pci16"
            ));
        }
    };
    WORD::try_from(handle).map_err(|_| format!("invalid PEAK channel handle for '{iface}'"))
}

fn classic_baudrate_constant(bitrate: u32) -> Result<WORD, String> {
    let baud = match bitrate {
        125_000 => PEAK_BAUD_125K,
        250_000 => PEAK_BAUD_250K,
        500_000 => PEAK_BAUD_500K,
        800_000 => PEAK_BAUD_800K,
        1_000_000 => PEAK_BAUD_1M,
        _ => {
            return Err(format!(
                "unsupported PEAK classic CAN bitrate {bitrate}; supported values are 125k, 250k, 500k, 800k, 1M"
            ));
        }
    };
    WORD::try_from(baud).map_err(|_| format!("invalid PEAK baudrate constant for {bitrate}"))
}

fn fd_bitrate_profile(bitrate: u32, bitrate_data: u32) -> Result<&'static str, String> {
    match (bitrate, bitrate_data) {
        (250_000, 2_000_000) => Ok(
            "f_clock=80000000,nom_brp=20,nom_tseg1=13,nom_tseg2=2,nom_sjw=1,data_brp=4,data_tseg1=7,data_tseg2=2,data_sjw=1",
        ),
        (500_000, 2_000_000) => Ok(
            "f_clock=80000000,nom_brp=10,nom_tseg1=13,nom_tseg2=2,nom_sjw=1,data_brp=4,data_tseg1=7,data_tseg2=2,data_sjw=1",
        ),
        (500_000, 4_000_000) => Ok(
            "f_clock=80000000,nom_brp=10,nom_tseg1=13,nom_tseg2=2,nom_sjw=1,data_brp=2,data_tseg1=7,data_tseg2=2,data_sjw=1",
        ),
        (1_000_000, 2_000_000) => Ok(
            "f_clock=80000000,nom_brp=5,nom_tseg1=13,nom_tseg2=2,nom_sjw=1,data_brp=4,data_tseg1=7,data_tseg2=2,data_sjw=1",
        ),
        _ => Err(format!(
            "unsupported PEAK CAN FD bitrate pair nominal={bitrate} data={bitrate_data}; add a supported profile"
        )),
    }
}

fn decode_classic_frame(raw: TPEAKMsg) -> SimCanFrame {
    let mut data = [0_u8; 64];
    let len = raw.LEN.min(8);
    data[..usize::from(len)].copy_from_slice(&raw.DATA[..usize::from(len)]);
    SimCanFrame {
        arb_id: raw.ID,
        len,
        flags: decode_message_type(raw.MSGTYPE),
        data,
    }
}

fn decode_fd_frame(raw: TPEAKMsgFD) -> Result<SimCanFrame, String> {
    let len = fd_dlc_to_len(raw.DLC)?;
    Ok(SimCanFrame {
        arb_id: raw.ID,
        len,
        flags: decode_message_type(raw.MSGTYPE),
        data: raw.DATA,
    })
}

fn decode_message_type(msgtype: BYTE) -> u8 {
    let raw = u32::from(msgtype);
    let mut flags = 0_u8;
    if (raw & PEAK_MESSAGE_EXTENDED) != 0 {
        flags |= CAN_FLAG_EXTENDED;
    }
    if (raw & PEAK_MESSAGE_RTR) != 0 {
        flags |= CAN_FLAG_RTR;
    }
    if (raw & PEAK_MESSAGE_FD) != 0 {
        flags |= CAN_FLAG_FD;
    }
    if (raw & PEAK_MESSAGE_BRS) != 0 {
        flags |= CAN_FLAG_BRS;
    }
    if (raw & PEAK_MESSAGE_ESI) != 0 {
        flags |= CAN_FLAG_ESI;
    }
    flags
}

fn encode_message_type(flags: u8) -> Result<BYTE, String> {
    let mut msgtype: DWORD = PEAK_MESSAGE_STANDARD;
    if (flags & CAN_FLAG_EXTENDED) != 0 {
        msgtype |= PEAK_MESSAGE_EXTENDED;
    }
    if (flags & CAN_FLAG_RTR) != 0 {
        msgtype |= PEAK_MESSAGE_RTR;
    }
    if (flags & CAN_FLAG_FD) != 0 {
        msgtype |= PEAK_MESSAGE_FD;
    }
    if (flags & CAN_FLAG_BRS) != 0 {
        msgtype |= PEAK_MESSAGE_BRS;
    }
    if (flags & CAN_FLAG_ESI) != 0 {
        msgtype |= PEAK_MESSAGE_ESI;
    }
    BYTE::try_from(msgtype).map_err(|_| {
        format!("invalid PEAK message type bitfield 0x{msgtype:X} derived from flags 0x{flags:02X}")
    })
}

fn fd_dlc_to_len(dlc: BYTE) -> Result<u8, String> {
    match dlc {
        0..=8 => Ok(dlc),
        9 => Ok(12),
        10 => Ok(16),
        11 => Ok(20),
        12 => Ok(24),
        13 => Ok(32),
        14 => Ok(48),
        15 => Ok(64),
        other => Err(format!("invalid PEAK CAN FD DLC {}", other)),
    }
}

fn fd_len_to_dlc(len: u8) -> Result<BYTE, String> {
    let dlc = match len {
        0..=8 => len,
        12 => 9,
        16 => 10,
        20 => 11,
        24 => 12,
        32 => 13,
        48 => 14,
        64 => 15,
        other => return Err(format!("invalid PEAK CAN FD payload length {}", other)),
    };
    Ok(dlc)
}

fn status_message(iface: &str, context: &str, status: DWORD) -> String {
    match status {
        PEAK_ERROR_INITIALIZE => {
            format!("failed to {context} PEAK CAN channel '{iface}': channel is not initialized")
        }
        PEAK_ERROR_ILLOPERATION => {
            format!("failed to {context} PEAK CAN channel '{iface}': illegal operation")
        }
        other => format!("failed to {context} PEAK CAN channel '{iface}': status 0x{other:X}"),
    }
}
