#[cfg(target_os = "windows")]
mod peak;
#[cfg(target_os = "linux")]
mod socketcan;

#[cfg(target_os = "windows")]
pub(crate) use peak::PlatformCanSocket;
#[cfg(target_os = "linux")]
pub(crate) use socketcan::PlatformCanSocket;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[derive(Debug)]
pub(crate) struct PlatformCanSocket {
    iface: String,
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl PlatformCanSocket {
    pub(crate) fn open(
        iface: &str,
        _bitrate: u32,
        _bitrate_data: u32,
        _fd_capable: bool,
    ) -> Result<Self, String> {
        Err(format!(
            "CAN transport requires Linux SocketCAN or Windows Peak CAN (requested '{iface}')"
        ))
    }

    pub(crate) fn iface(&self) -> &str {
        &self.iface
    }

    pub(crate) fn recv_all(&self) -> Result<Vec<crate::sim::types::SimCanFrame>, String> {
        Err("CAN transport requires Linux SocketCAN or Windows Peak CAN".to_string())
    }

    pub(crate) fn send(&self, _frame: &crate::sim::types::SimCanFrame) -> Result<(), String> {
        Err("CAN transport requires Linux SocketCAN or Windows Peak CAN".to_string())
    }
}
