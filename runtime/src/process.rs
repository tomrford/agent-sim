use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::time::sleep;

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const STALE_INVALID_LOCK_AGE: Duration = Duration::from_secs(1);

#[cfg(unix)]
pub fn kill_pid(pid: u32) -> std::io::Result<()> {
    let result = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
pub fn pid_exists(pid: u32) -> std::io::Result<bool> {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::ESRCH => Ok(false),
        Some(code) if code == libc::EPERM => Ok(true),
        _ => Err(err),
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

#[cfg(windows)]
pub fn pid_exists(pid: u32) -> std::io::Result<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, SYNCHRONIZE,
    };

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(code) if code == ERROR_ACCESS_DENIED as i32 => Ok(true),
            Some(_) => Ok(false),
            None => Err(err),
        };
    }

    unsafe {
        CloseHandle(handle);
    }
    Ok(true)
}

#[cfg(not(any(unix, windows)))]
compile_error!("process helpers are only implemented for Unix and Windows targets");

pub struct StartupLock {
    path: PathBuf,
}

impl StartupLock {
    pub async fn acquire(path: PathBuf, timeout: Duration) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let deadline = std::time::Instant::now() + timeout;
        loop {
            match try_create_lock(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if clear_stale_lock(&path)? {
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timed out waiting for startup lock '{}'", path.display()),
                        ));
                    }
                    sleep(LOCK_POLL_INTERVAL).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
}

impl Drop for StartupLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn try_create_lock(path: &Path) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_all()?;
    Ok(())
}

fn clear_stale_lock(path: &Path) -> std::io::Result<bool> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(false);
    };

    if let Ok(owner_pid) = contents.trim().parse::<u32>() {
        if !pid_exists(owner_pid)? {
            std::fs::remove_file(path)?;
            return Ok(true);
        }
        return Ok(false);
    }

    let modified_at = path
        .metadata()?
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let age = SystemTime::now()
        .duration_since(modified_at)
        .unwrap_or(Duration::ZERO);
    if age >= STALE_INVALID_LOCK_AGE {
        std::fs::remove_file(path)?;
        return Ok(true);
    }

    Ok(false)
}
