use crate::sim::types::SimCanFrame;
use crate::sim::types::{CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RTR};
use std::mem::size_of;
use std::os::fd::RawFd;

const SOL_CAN_RAW: i32 = 101;
const CAN_RAW_RECV_OWN_MSGS: i32 = 4;
const CAN_RAW_FD_FRAMES: i32 = 5;
const CAN_EFF_FLAG: u32 = 0x8000_0000;
const CAN_RTR_FLAG: u32 = 0x4000_0000;
const CAN_SFF_MASK: u32 = 0x0000_07FF;
const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;
const CANFD_BRS: u8 = 0x01;
const CANFD_ESI: u8 = 0x02;

#[derive(Debug)]
pub(crate) struct PlatformCanSocket {
    iface: String,
    fd: RawFd,
}

impl PlatformCanSocket {
    pub(crate) fn open(
        iface: &str,
        _bitrate: u32,
        _bitrate_data: u32,
        fd_capable: bool,
    ) -> Result<Self, String> {
        let if_name = std::ffi::CString::new(iface.as_bytes())
            .map_err(|_| format!("invalid CAN interface name '{iface}'"))?;
        let fd = unsafe {
            libc::socket(
                libc::AF_CAN,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK,
                libc::CAN_RAW,
            )
        };
        if fd < 0 {
            return Err(format!(
                "failed to open AF_CAN socket: {}",
                std::io::Error::last_os_error()
            ));
        }

        let if_index = unsafe { libc::if_nametoindex(if_name.as_ptr()) };
        if if_index == 0 {
            unsafe { libc::close(fd) };
            return Err(format!(
                "unknown CAN interface '{iface}': {}",
                std::io::Error::last_os_error()
            ));
        }

        let recv_own: libc::c_int = 0;
        let set_recv_own = unsafe {
            libc::setsockopt(
                fd,
                SOL_CAN_RAW,
                CAN_RAW_RECV_OWN_MSGS,
                &recv_own as *const _ as *const libc::c_void,
                size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if set_recv_own < 0 {
            unsafe { libc::close(fd) };
            return Err(format!(
                "failed to disable CAN loopback on '{iface}': {}",
                std::io::Error::last_os_error()
            ));
        }

        if fd_capable {
            let enable_fd: libc::c_int = 1;
            let set_fd = unsafe {
                libc::setsockopt(
                    fd,
                    SOL_CAN_RAW,
                    CAN_RAW_FD_FRAMES,
                    &enable_fd as *const _ as *const libc::c_void,
                    size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            if set_fd < 0 {
                unsafe { libc::close(fd) };
                return Err(format!(
                    "failed to enable CAN FD mode on '{iface}': {}",
                    std::io::Error::last_os_error()
                ));
            }
        }

        let addr = libc::sockaddr_can {
            can_family: libc::AF_CAN as libc::sa_family_t,
            can_ifindex: if_index as i32,
            ..unsafe { std::mem::zeroed() }
        };
        let bind_status = unsafe {
            libc::bind(
                fd,
                (&addr as *const libc::sockaddr_can).cast(),
                size_of::<libc::sockaddr_can>() as libc::socklen_t,
            )
        };
        if bind_status < 0 {
            unsafe { libc::close(fd) };
            return Err(format!(
                "failed to bind CAN socket to '{iface}': {}",
                std::io::Error::last_os_error()
            ));
        }

        Ok(Self {
            iface: iface.to_string(),
            fd,
        })
    }

    pub(crate) fn iface(&self) -> &str {
        &self.iface
    }

    pub(crate) fn recv_all(&self) -> Result<Vec<SimCanFrame>, String> {
        let mut frames = Vec::new();
        loop {
            let mut raw = LinuxCanFdFrame::default();
            let read = unsafe {
                libc::recv(
                    self.fd,
                    (&mut raw as *mut LinuxCanFdFrame).cast(),
                    size_of::<LinuxCanFdFrame>(),
                    0,
                )
            };
            if read < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    break;
                }
                return Err(format!(
                    "failed reading CAN frame from '{}': {err}",
                    self.iface
                ));
            }

            if read as usize == size_of::<LinuxCanFrame>() {
                let mut data = [0_u8; 64];
                data[..8].copy_from_slice(&raw.data[..8]);
                frames.push(SimCanFrame {
                    arb_id: decode_arb_id(raw.can_id),
                    len: raw.len.min(8),
                    flags: decode_common_flags(raw.can_id),
                    data,
                });
                continue;
            }

            if read as usize == size_of::<LinuxCanFdFrame>() {
                let mut flags = decode_common_flags(raw.can_id) | CAN_FLAG_FD;
                if (raw.flags & CANFD_BRS) != 0 {
                    flags |= CAN_FLAG_BRS;
                }
                if (raw.flags & CANFD_ESI) != 0 {
                    flags |= CAN_FLAG_ESI;
                }
                frames.push(SimCanFrame {
                    arb_id: decode_arb_id(raw.can_id),
                    len: raw.len.min(64),
                    flags,
                    data: raw.data,
                });
                continue;
            }

            return Err(format!(
                "received unexpected CAN frame size {} bytes on '{}'",
                read, self.iface
            ));
        }
        Ok(frames)
    }

    pub(crate) fn send(&self, frame: &SimCanFrame) -> Result<(), String> {
        let can_id = encode_can_id(frame.arb_id, frame.flags);
        if (frame.flags & CAN_FLAG_FD) != 0 || frame.len > 8 {
            let raw = LinuxCanFdFrame {
                can_id,
                len: frame.len,
                flags: encode_fd_flags(frame.flags),
                __res0: 0,
                __res1: 0,
                data: frame.data,
            };
            let written = unsafe {
                libc::send(
                    self.fd,
                    (&raw as *const LinuxCanFdFrame).cast(),
                    size_of::<LinuxCanFdFrame>(),
                    0,
                )
            };
            if written < 0 {
                return Err(format!(
                    "failed to send CAN FD frame on '{}': {}",
                    self.iface,
                    std::io::Error::last_os_error()
                ));
            }
            return Ok(());
        }

        let mut payload = [0_u8; 8];
        payload[..usize::from(frame.len)].copy_from_slice(&frame.data[..usize::from(frame.len)]);
        let raw = LinuxCanFrame {
            can_id,
            can_dlc: frame.len,
            __pad: 0,
            __res0: 0,
            __res1: 0,
            data: payload,
        };
        let written = unsafe {
            libc::send(
                self.fd,
                (&raw as *const LinuxCanFrame).cast(),
                size_of::<LinuxCanFrame>(),
                0,
            )
        };
        if written < 0 {
            return Err(format!(
                "failed to send CAN frame on '{}': {}",
                self.iface,
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

impl Drop for PlatformCanSocket {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxCanFrame {
    can_id: u32,
    can_dlc: u8,
    __pad: u8,
    __res0: u8,
    __res1: u8,
    data: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxCanFdFrame {
    can_id: u32,
    len: u8,
    flags: u8,
    __res0: u8,
    __res1: u8,
    data: [u8; 64],
}

impl Default for LinuxCanFdFrame {
    fn default() -> Self {
        Self {
            can_id: 0,
            len: 0,
            flags: 0,
            __res0: 0,
            __res1: 0,
            data: [0; 64],
        }
    }
}

fn decode_common_flags(can_id: u32) -> u8 {
    let mut flags = 0_u8;
    if (can_id & CAN_EFF_FLAG) != 0 {
        flags |= CAN_FLAG_EXTENDED;
    }
    if (can_id & CAN_RTR_FLAG) != 0 {
        flags |= CAN_FLAG_RTR;
    }
    flags
}

fn decode_arb_id(can_id: u32) -> u32 {
    if (can_id & CAN_EFF_FLAG) != 0 {
        can_id & CAN_EFF_MASK
    } else {
        can_id & CAN_SFF_MASK
    }
}

fn encode_can_id(arb_id: u32, flags: u8) -> u32 {
    let mut can_id = if (flags & CAN_FLAG_EXTENDED) != 0 {
        arb_id & CAN_EFF_MASK
    } else {
        arb_id & CAN_SFF_MASK
    };
    if (flags & CAN_FLAG_EXTENDED) != 0 {
        can_id |= CAN_EFF_FLAG;
    }
    if (flags & CAN_FLAG_RTR) != 0 {
        can_id |= CAN_RTR_FLAG;
    }
    can_id
}

fn encode_fd_flags(flags: u8) -> u8 {
    let mut fd_flags = 0_u8;
    if (flags & CAN_FLAG_BRS) != 0 {
        fd_flags |= CANFD_BRS;
    }
    if (flags & CAN_FLAG_ESI) != 0 {
        fd_flags |= CANFD_ESI;
    }
    fd_flags
}
