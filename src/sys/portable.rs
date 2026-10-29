//! Portable backend (macOS, Windows, BSDs) built on `sysinfo`.
//!
//! These platforms don't expose PSS or per-process swap cheaply, so memory
//! is resident memory (RSS / working set). Shared pages are counted in every
//! process that maps them, so group totals can overstate real usage.

use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind, Users};

use crate::proc::{MemInfo, Proc, Snapshot, describe, tidy};

pub struct Sampler {
    sys: Option<System>,
    users: Option<Users>,
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            sys: System::new().ok(),
            users: Users::new_with_refreshed_list().ok(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let Some(sys) = self.sys.as_mut() else {
            return Snapshot::default();
        };
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu()
                .with_user(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );

        let mut procs = Vec::with_capacity(sys.processes().len());
        for (pid, p) in sys.processes() {
            let comm = p.name().to_string_lossy().into_owned();
            let args: Vec<String> = p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .filter(|a| !a.is_empty())
                .collect();
            let (mut name, role) = describe(&comm, &args);
            for ext in [".exe", ".EXE", ".Exe"] {
                if let Some(n) = name.strip_suffix(ext) {
                    name = n.to_string();
                }
            }
            let user = p
                .user_id()
                .and_then(|u| self.users.as_ref()?.get_user_by_id(u))
                .map(|u| u.name().to_string())
                .unwrap_or_default();
            let rss = p.memory() / 1024;
            let cmdline = if args.is_empty() {
                comm.clone()
            } else {
                args.join(" ")
            };
            procs.push(Proc {
                pid: pid.as_u32() as i32,
                ppid: p.parent().map(|x| x.as_u32() as i32).unwrap_or(0),
                name,
                role: tidy(&role, 60),
                cmdline: tidy(&cmdline, 4000),
                user,
                state: match p.status() {
                    ProcessStatus::Run => 'R',
                    ProcessStatus::Idle => 'I',
                    ProcessStatus::Stop => 'T',
                    ProcessStatus::Zombie => 'Z',
                    _ => 'S',
                },
                start: p.start_time(),
                cpu: p.cpu_usage(),
                rss,
                pss: rss,
                comm,
                ..Default::default()
            });
        }

        let running = procs.iter().filter(|p| p.state == 'R').count();
        let load = System::load_average()
            .map(|l| [l.one as f32, l.five as f32, l.fifteen as f32])
            .unwrap_or_default();
        Snapshot {
            procs,
            cpu_total: sys.global_cpu_usage(),
            cores: sys.cpus().iter().map(|c| c.usage()).collect(),
            mem: MemInfo {
                total: sys.total_memory() / 1024,
                available: sys.available_memory() / 1024,
                free: sys.free_memory() / 1024,
                swap_total: sys.total_swap() / 1024,
                swap_free: sys.total_swap().saturating_sub(sys.used_swap()) / 1024,
                ..Default::default()
            },
            load,
            uptime: System::uptime().unwrap_or(0) as f64,
            threads: 0,
            running,
            self_pid: std::process::id() as i32,
            zram: None,
            tmpfs: Vec::new(),
            detailed: false,
        }
    }
}
