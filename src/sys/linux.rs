//! Linux backend: reads /proc directly.
//!
//! Memory is reported as PSS (proportional set size) from smaps_rollup, so
//! pages shared between e.g. Electron/Chromium children are split fairly and
//! summing a process tree gives a real number instead of RSS double counting.

use std::collections::HashMap;
use std::fs;
use std::time::Instant;

use crate::proc::{MemInfo, Proc, Snapshot, Zram, describe, kib, tidy};

pub struct Sampler {
    prev_ticks: HashMap<(i32, u64), u64>,
    prev_cpu: Vec<(u64, u64)>,
    prev_time: Option<Instant>,
    users: HashMap<u32, String>,
    clk_tck: f64,
}

impl Sampler {
    pub fn new() -> Self {
        let clk = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        Self {
            prev_ticks: HashMap::new(),
            prev_cpu: Vec::new(),
            prev_time: None,
            users: read_users(),
            clk_tck: if clk > 0 { clk as f64 } else { 100.0 },
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed = self
            .prev_time
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);
        self.prev_time = Some(now);

        let (cpu_total, cores) = self.sample_cpu();
        let mut procs = Vec::with_capacity(self.prev_ticks.len().max(256));
        let mut next_ticks = HashMap::with_capacity(procs.capacity());

        if let Ok(dir) = fs::read_dir("/proc") {
            for entry in dir.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<i32>().ok())
                else {
                    continue;
                };
                // Processes can vanish mid-scan; just skip them.
                let Some(mut p) = self.read_proc(pid) else {
                    continue;
                };
                let key = (pid, p.start);
                if let Some(prev) = self.prev_ticks.get(&key)
                    && elapsed > 0.0
                {
                    p.cpu = (p.ticks.saturating_sub(*prev) as f64 / self.clk_tck / elapsed * 100.0)
                        as f32;
                }
                next_ticks.insert(key, p.ticks);
                procs.push(p);
            }
        }
        self.prev_ticks = next_ticks;

        let threads = procs.iter().map(|p| p.threads as usize).sum();
        let running = procs.iter().filter(|p| p.state == 'R').count();
        Snapshot {
            procs,
            cpu_total,
            cores,
            mem: read_meminfo(),
            load: read_loadavg(),
            uptime: fs::read_to_string("/proc/uptime")
                .ok()
                .and_then(|s| s.split_whitespace().next()?.parse().ok())
                .unwrap_or(0.0),
            threads,
            running,
            self_pid: std::process::id() as i32,
            zram: read_zram(),
            detailed: true,
            tmpfs: read_tmpfs(),
        }
    }

    fn sample_cpu(&mut self) -> (f32, Vec<f32>) {
        let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
        let mut cur = Vec::new();
        for line in stat.lines() {
            if !line.starts_with("cpu") {
                break;
            }
            let v: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .take(8)
                .filter_map(|x| x.parse().ok())
                .collect();
            let total: u64 = v.iter().sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            cur.push((total.saturating_sub(idle), total));
        }
        let pct: Vec<f32> = cur
            .iter()
            .enumerate()
            .map(|(i, &(busy, total))| match self.prev_cpu.get(i) {
                Some(&(pb, pt)) if total > pt => {
                    busy.saturating_sub(pb) as f32 / (total - pt) as f32 * 100.0
                }
                _ => 0.0,
            })
            .collect();
        self.prev_cpu = cur;
        let total = pct.first().copied().unwrap_or(0.0);
        let cores = pct.get(1..).map(<[f32]>::to_vec).unwrap_or_default();
        (total, cores)
    }

    fn read_proc(&self, pid: i32) -> Option<Proc> {
        let base = format!("/proc/{pid}");
        let stat = fs::read_to_string(format!("{base}/stat")).ok()?;
        // comm may contain spaces and parens: split on the *last* ')'.
        let open = stat.find('(')?;
        let close = stat.rfind(')')?;
        let comm = stat[open + 1..close].to_string();
        let f: Vec<&str> = stat.get(close + 2..)?.split_whitespace().collect();
        if f.len() < 20 {
            return None;
        }
        let num = |i: usize| f[i].parse::<u64>().unwrap_or(0);
        const PF_KTHREAD: u64 = 0x0020_0000;

        let mut p = Proc {
            pid,
            ppid: f[1].parse().unwrap_or(0),
            state: f[0].chars().next().unwrap_or('?'),
            kernel: num(6) & PF_KTHREAD != 0,
            ticks: num(11) + num(12),
            threads: num(17) as u32,
            start: num(19),
            comm,
            ..Default::default()
        };

        let mut uid = 0;
        let mut vm_swap = 0;
        if let Ok(status) = fs::read_to_string(format!("{base}/status")) {
            for line in status.lines() {
                if let Some(v) = line.strip_prefix("Uid:") {
                    uid = v
                        .split_whitespace()
                        .next()
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("VmRSS:") {
                    p.rss = kib(v);
                } else if let Some(v) = line.strip_prefix("VmSwap:") {
                    vm_swap = kib(v);
                }
            }
        }
        p.uid = uid;
        p.user = self
            .users
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| uid.to_string());

        if p.kernel {
            p.name = format!("[{}]", p.comm);
            p.role = "kernel thread".into();
            p.mem_exact = true;
            return Some(p);
        }

        let raw = fs::read(format!("{base}/cmdline")).unwrap_or_default();
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        p.cmdline = if args.is_empty() {
            format!("[{}]", p.comm)
        } else {
            args.join(" ")
        };
        let (name, role) = describe(&p.comm, &args);
        p.name = name;
        p.role = tidy(&role, 60);
        p.cmdline = tidy(&p.cmdline, 4000);

        if let Ok(smaps) = fs::read_to_string(format!("{base}/smaps_rollup")) {
            let mut private = 0;
            for line in smaps.lines() {
                let Some((k, v)) = line.split_once(':') else {
                    continue;
                };
                match k {
                    "Pss" => p.pss = kib(v),
                    "Pss_Anon" => p.pss_anon = kib(v),
                    "Pss_File" => p.pss_file = kib(v),
                    "Pss_Shmem" => p.pss_shmem = kib(v),
                    "Private_Clean" | "Private_Dirty" => private += kib(v),
                    "SwapPss" => p.swap = kib(v),
                    _ => {}
                }
            }
            p.uss = private;
            p.mem_exact = true;
        } else {
            p.pss = p.rss;
            p.swap = vm_swap;
        }
        Some(p)
    }
}

fn read_users() -> HashMap<u32, String> {
    fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut it = l.split(':');
            let name = it.next()?;
            let uid = it.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

fn read_meminfo() -> MemInfo {
    let mut m = MemInfo::default();
    for line in fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
    {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = kib(v);
        match k {
            "MemTotal" => m.total = v,
            "MemAvailable" => m.available = v,
            "MemFree" => m.free = v,
            "AnonPages" => m.anon = v,
            "Shmem" => m.shmem = v,
            "SUnreclaim" => m.slab_unreclaimable = v,
            "KernelStack" => m.kernel_stack = v,
            "PageTables" | "SecPageTables" => m.page_tables += v,
            "VmallocUsed" => m.vmalloc = v,
            "Percpu" => m.percpu = v,
            "GPUActive" => m.gpu = v,
            "Buffers" => m.buffers = v,
            "Cached" => m.cached = v,
            "SwapTotal" => m.swap_total = v,
            "SwapFree" => m.swap_free = v,
            _ => {}
        }
    }
    m
}

fn read_loadavg() -> [f32; 3] {
    let s = fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let mut it = s.split_whitespace().map(|x| x.parse().unwrap_or(0.0));
    [
        it.next().unwrap_or(0.0),
        it.next().unwrap_or(0.0),
        it.next().unwrap_or(0.0),
    ]
}

fn read_zram() -> Option<Zram> {
    let mut z = Zram::default();
    let mut found = false;
    for e in fs::read_dir("/sys/block").ok()?.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().starts_with("zram") {
            continue;
        }
        let Ok(s) = fs::read_to_string(e.path().join("mm_stat")) else {
            continue;
        };
        let v: Vec<u64> = s
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        if v.len() >= 3 {
            z.stored += v[0] / 1024;
            z.compressed += v[1] / 1024;
            z.ram += v[2] / 1024;
            found = true;
        }
    }
    found.then_some(z)
}

fn read_tmpfs() -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    for line in fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
    {
        let mut it = line.split_whitespace();
        let (Some(_), Some(mnt), Some(fstype)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if fstype != "tmpfs" {
            continue;
        }
        // Mount points escape spaces as \040.
        let mnt = mnt.replace("\\040", " ");
        let Ok(c) = std::ffi::CString::new(mnt.clone()) else {
            continue;
        };
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            continue;
        }
        let bs = st.f_frsize as u64;
        let used = (st.f_blocks as u64).saturating_sub(st.f_bfree as u64) * bs / 1024;
        out.push((mnt, used, st.f_blocks as u64 * bs / 1024));
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.1));
    out
}
