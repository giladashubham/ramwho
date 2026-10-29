use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Candidate, CleanMsg, Target, default_pick};
use crate::proc::Proc;

pub const ROOTS: &[&str] = &["/tmp", "/dev/shm"];

/// Session / system entries that must never be offered for deletion.
const PROTECTED: &[&str] = &[
    ".X11-unix",
    ".ICE-unix",
    ".XIM-unix",
    ".font-unix",
    ".Test-unix",
    ".X",
    "systemd-private-",
    "tmux-",
    "ssh-",
    "pulse-",
    "dbus-",
    "krb5cc",
    "claude-",
    "snap-private-tmp",
    ".mount_",
    "libpod_rootless_lock",
    "lttng-",
    "sem.",
    ".org.chromium.",
    "xauth",
    "gdm",
    "orbit-",
    "kitty-",
    "zellij-",
    "nvim.",
];

const DEV_PROCS: &[&str] = &[
    "node",
    "bun",
    "deno",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "uv",
    "esbuild",
    "vite",
    "tsserver",
    "tsc",
    "next-server",
    "rust-analyzer",
    "gopls",
    "cargo",
    "java",
    "ruby",
    "php",
    "claude",
    "codex",
    "gemini",
    "python",
    "python3",
    "pyright",
    "typescript-language-server",
];

fn my_uid() -> u32 {
    unsafe { libc::getuid() }
}

/// Top-level tmpfs entry ("/tmp/foo") that a path lives under.
fn top_entry(p: &str) -> Option<String> {
    for root in ROOTS {
        if let Some(rest) = p.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
            let first = rest.split('/').next().filter(|s| !s.is_empty())?;
            return Some(format!("{root}/{first}"));
        }
    }
    None
}

/// Map of top-level tmpfs entry → "comm (pid)" for everything currently in use.
pub fn collect_used() -> HashMap<String, String> {
    let mut used = HashMap::new();
    let mut add = |path: &str, who: &dyn Fn() -> String| {
        let path = path.trim_end_matches(" (deleted)");
        if let Some(top) = top_entry(path) {
            used.entry(top).or_insert_with(who);
        }
    };
    let Ok(dir) = fs::read_dir("/proc") else {
        return used;
    };
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        let base = e.path();
        let comm = fs::read_to_string(base.join("comm")).unwrap_or_default();
        let who = || format!("{} ({pid})", comm.trim());
        for link in ["cwd", "exe", "root"] {
            if let Ok(t) = fs::read_link(base.join(link)) {
                add(&t.to_string_lossy(), &who);
            }
        }
        if let Ok(fds) = fs::read_dir(base.join("fd")) {
            for fd in fds.flatten() {
                if let Ok(t) = fs::read_link(fd.path()) {
                    add(&t.to_string_lossy(), &who);
                }
            }
        }
        if let Ok(maps) = fs::read_to_string(base.join("maps")) {
            for line in maps.lines() {
                if let Some(i) = line.find('/') {
                    add(&line[i..], &who);
                }
            }
        }
    }
    // Bound unix sockets (tmux, ssh-agent, language servers…).
    for line in fs::read_to_string("/proc/net/unix")
        .unwrap_or_default()
        .lines()
        .skip(1)
    {
        if let Some(path) = line.split_whitespace().nth(7) {
            add(path, &|| "socket".to_string());
        }
    }
    used
}

/// Allocated size (KiB) and newest mtime of a tree, staying on one filesystem.
fn du(path: &Path, dev: u64) -> (u64, i64) {
    let mut size = 0;
    let mut newest = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(md) = fs::symlink_metadata(&p) else {
            continue;
        };
        if md.dev() != dev {
            continue;
        }
        size += md.blocks() * 512 / 1024;
        newest = newest.max(md.mtime());
        if md.is_dir()
            && let Ok(rd) = fs::read_dir(&p)
        {
            stack.extend(rd.flatten().map(|e| e.path()));
        }
    }
    (size, newest)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn scan_paths(used: &HashMap<String, String>) -> Vec<Candidate> {
    let uid = my_uid();
    let now = now_secs();
    let mut out = Vec::new();
    for root in ROOTS {
        let Ok(root_md) = fs::metadata(root) else {
            continue;
        };
        let Ok(rd) = fs::read_dir(root) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if PROTECTED.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            let path = e.path();
            let Ok(md) = fs::symlink_metadata(&path) else {
                continue;
            };
            // Other users' files can't be removed; mount points aren't files.
            if md.uid() != uid || md.dev() != root_md.dev() {
                continue;
            }
            let ft = md.file_type();
            if ft.is_socket() || ft.is_fifo() || ft.is_block_device() || ft.is_char_device() {
                continue;
            }
            let (size, newest) = du(&path, root_md.dev());
            let full = path.to_string_lossy().into_owned();
            let in_use_by = used.get(&full).cloned();
            let idle_secs = (now - newest).max(0) as u64;
            let mut c = Candidate {
                selected: false,
                target: Target::Path(path),
                name,
                kind: if ft.is_dir() {
                    "folder"
                } else if ft.is_symlink() {
                    "link"
                } else {
                    "file"
                },
                root: root.to_string(),
                size,
                idle_secs,
                in_use_by,
            };
            c.selected = default_pick(&c);
            out.push(c);
        }
    }
    out
}

fn is_launcher(p: &Proc) -> bool {
    p.pid == 1 || p.comm == "systemd" || p.comm == "init"
}

/// Dev tooling processes that got re-parented to systemd/init (the editor,
/// terminal or app that started them is gone), have no children and are idle.
pub fn scan_procs(procs: &[Proc]) -> Vec<Candidate> {
    let uid = my_uid();
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let parents: HashSet<i32> = procs.iter().map(|p| p.ppid).collect();
    let now_ticks = {
        let up: f64 = fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0);
        up * unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64
    };
    let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let mut out = Vec::new();
    for p in procs {
        if p.kernel || p.uid != uid || parents.contains(&p.pid) || p.cpu >= 1.0 {
            continue;
        }
        let dev = DEV_PROCS.contains(&p.name.as_str())
            || DEV_PROCS.contains(&p.comm.as_str())
            || p.name.starts_with("python3.");
        if !dev || !by_pid.get(&p.ppid).is_some_and(|pp| is_launcher(pp)) {
            continue;
        }
        // Started by a systemd service on purpose → not an orphan.
        let cg = fs::read_to_string(format!("/proc/{}/cgroup", p.pid)).unwrap_or_default();
        if cg.trim_end().ends_with(".service") {
            continue;
        }
        let age = ((now_ticks - p.start as f64) / tck).max(0.0) as u64;
        out.push(Candidate {
            target: Target::Proc {
                pid: p.pid,
                start: p.start,
            },
            name: if p.role.is_empty() {
                p.name.clone()
            } else {
                format!("{}  {}", p.name, p.role)
            },
            kind: "process",
            root: format!("pid {}", p.pid),
            size: p.pss + p.swap,
            idle_secs: age,
            in_use_by: None,
            selected: false,
        });
    }
    out
}

pub fn scan(procs: &[Proc]) -> Vec<Candidate> {
    let used = collect_used();
    let mut v = scan_procs(procs);
    // A detached process that is holding files in /tmp is doing work: lock it.
    for c in v.iter_mut() {
        if let Target::Proc { pid, .. } = c.target {
            let tag = format!("({pid})");
            if let Some((path, _)) = used.iter().find(|(_, who)| who.ends_with(&tag)) {
                c.in_use_by = Some(format!("working in {path}"));
            }
        }
    }
    v.extend(scan_paths(&used));
    // Cleanable first, biggest first.
    v.sort_by(|a, b| {
        a.in_use_by
            .is_some()
            .cmp(&b.in_use_by.is_some())
            .then(b.size.cmp(&a.size))
    });
    v
}

pub fn clean(items: Vec<Candidate>) -> CleanMsg {
    // Re-check usage right before deleting: things may have started since the scan.
    let used = collect_used();
    let (mut removed, mut freed, mut failed) = (0, 0, Vec::new());
    for c in items {
        match &c.target {
            Target::Path(p) => {
                let full = p.to_string_lossy().into_owned();
                let parent_ok = p
                    .parent()
                    .is_some_and(|d| ROOTS.iter().any(|r| d == Path::new(r)));
                if !parent_ok {
                    failed.push(format!("{full}: outside /tmp"));
                    continue;
                }
                if let Some(who) = used.get(&full) {
                    failed.push(format!("{}: now in use by {who}", c.name));
                    continue;
                }
                let md = fs::symlink_metadata(p);
                let res = match md {
                    Ok(m) if m.is_dir() => fs::remove_dir_all(p),
                    Ok(_) => fs::remove_file(p),
                    Err(e) => Err(e),
                };
                match res {
                    Ok(()) => {
                        removed += 1;
                        freed += c.size;
                    }
                    Err(e) => failed.push(format!("{}: {e}", c.name)),
                }
            }
            Target::Proc { pid, start } => {
                // Make sure the pid wasn't reused by another process.
                let same = fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .is_some_and(|s| {
                        s.rfind(')').and_then(|i| {
                            s[i + 2..].split_whitespace().nth(19)?.parse::<u64>().ok()
                        }) == Some(*start)
                    });
                if !same {
                    continue;
                }
                if unsafe { libc::kill(*pid, libc::SIGTERM) } == 0 {
                    removed += 1;
                    freed += c.size;
                } else {
                    failed.push(format!("{}: {}", c.name, std::io::Error::last_os_error()));
                }
            }
        }
    }
    CleanMsg::Done {
        removed,
        freed,
        failed,
    }
}
