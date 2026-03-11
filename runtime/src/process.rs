#[cfg(unix)]
pub fn kill_pid(pid: u32) -> std::io::Result<()> {
    let result = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub fn kill_pid(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }

    let terminated = unsafe { TerminateProcess(handle, 1) };
    let terminate_error = if terminated == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        CloseHandle(handle);
    }

    if let Some(err) = terminate_error {
        Err(err)
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("process::kill_pid is only implemented for Unix and Windows targets");
