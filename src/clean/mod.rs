//! Finds leftovers that hold RAM without being needed:
//! - files/folders in RAM-backed tmpfs (/tmp, /dev/shm) that no running
//!   process is using (open fd, cwd, mmap, bound unix socket)
//! - orphaned dev processes (node, python, …) whose parent app has exited.
//!
//! Nothing is removed without the user selecting it and confirming.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{clean, scan};

use std::path::PathBuf;

#[cfg(not(target_os = "linux"))]
use crate::proc::Proc;

/// Clean-up relies on /proc to prove nothing uses a file; Linux only for now.
pub const SUPPORTED: bool = cfg!(target_os = "linux");
/// Untouched for this long and unused → pre-selected.
pub const STALE_SECS: u64 = 24 * 3600;

#[derive(Clone, Debug)]
pub enum Target {
    Path(PathBuf),
    Proc { pid: i32, start: u64 },
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub target: Target,
    pub name: String,
    pub kind: &'static str,
    pub root: String,
    /// KiB (for processes: RAM + swap).
    pub size: u64,
    pub idle_secs: u64,
    /// Why it can't be cleaned right now.
    pub in_use_by: Option<String>,
    pub selected: bool,
}

pub enum CleanMsg {
    Scanned(Vec<Candidate>),
    Done {
        removed: usize,
        freed: u64,
        failed: Vec<String>,
    },
}

/// Pre-selection rule: unused, untouched for a day, and big enough to matter
/// (tiny lock files / empty IPC dirs free nothing and may belong to live apps).
pub fn default_pick(c: &Candidate) -> bool {
    matches!(c.target, Target::Path(_))
        && c.in_use_by.is_none()
        && c.idle_secs >= STALE_SECS
        && c.size >= 1024
        && !c.name.ends_with(".lock")
}

#[cfg(not(target_os = "linux"))]
pub fn scan(_procs: &[Proc]) -> Vec<Candidate> {
    Vec::new()
}

#[cfg(not(target_os = "linux"))]
pub fn clean(_items: Vec<Candidate>) -> CleanMsg {
    CleanMsg::Done {
        removed: 0,
        freed: 0,
        failed: vec!["clean-up is not supported on this platform".into()],
    }
}
