//! Platform backends. Linux reads /proc directly for exact per-process
//! memory (PSS, swap) and the full RAM breakdown; macOS, Windows and other
//! systems use the portable `sysinfo` backend (resident memory per process).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Sampler;

#[cfg(not(target_os = "linux"))]
mod portable;
#[cfg(not(target_os = "linux"))]
pub use portable::Sampler;

pub const PLATFORM: &str = std::env::consts::OS;

/// Stop a process. `force` = SIGKILL (unix) / TerminateProcess (windows);
/// otherwise a graceful SIGTERM where the platform has one.
#[cfg(unix)]
pub fn terminate(pid: i32, force: bool) -> std::io::Result<()> {
    let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
    if unsafe { libc::kill(pid, sig) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Windows has no SIGTERM: both options end the process.
#[cfg(not(unix))]
pub fn terminate(pid: i32, _force: bool) -> std::io::Result<()> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid as u32);
    let mut sys = System::new().map_err(|e| std::io::Error::other(e.to_string()))?;
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    match sys.process(pid) {
        Some(p) if p.kill() => Ok(()),
        Some(_) => Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        None => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
    }
}

/// True for errors that mean "this process belongs to someone else".
pub fn is_permission_error(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
}
