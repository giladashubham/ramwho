//! Platform-neutral process/system model, plus helpers that turn long
//! command lines into short labels. Sampling lives in `crate::sys`.

/// All memory values are KiB.
#[derive(Clone, Default, Debug)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub comm: String,
    /// Short display name (binary basename).
    pub name: String,
    /// What this process is doing: electron role, script path, subcommand…
    pub role: String,
    pub cmdline: String,
    pub user: String,
    pub uid: u32,
    pub state: char,
    pub threads: u32,
    pub ticks: u64,
    pub start: u64,
    /// % of one core (htop style, can exceed 100).
    pub cpu: f32,
    pub rss: u64,
    pub pss: u64,
    pub pss_anon: u64,
    pub pss_file: u64,
    pub pss_shmem: u64,
    pub uss: u64,
    pub swap: u64,
    /// false when smaps_rollup was unreadable (other user's process) and
    /// pss/swap fall back to RSS/VmSwap.
    pub mem_exact: bool,
    pub kernel: bool,
}

#[derive(Clone, Default, Debug)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub free: u64,
    /// Anonymous memory of all processes (heap, stacks).
    pub anon: u64,
    /// tmpfs + shared memory currently resident in RAM.
    pub shmem: u64,
    pub slab_unreclaimable: u64,
    pub kernel_stack: u64,
    pub page_tables: u64,
    pub vmalloc: u64,
    pub percpu: u64,
    pub gpu: u64,
    pub buffers: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl MemInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

#[derive(Clone, Default, Debug)]
pub struct Snapshot {
    pub procs: Vec<Proc>,
    pub cpu_total: f32,
    pub cores: Vec<f32>,
    pub mem: MemInfo,
    pub load: [f32; 3],
    pub uptime: f64,
    pub threads: usize,
    pub running: usize,
    pub self_pid: i32,
    pub zram: Option<Zram>,
    /// true when the backend reports PSS/swap per process and the full RAM
    /// breakdown (Linux). Other platforms report resident memory (RSS).
    pub detailed: bool,
    /// (mountpoint, used KiB, size KiB) for every tmpfs mount.
    pub tmpfs: Vec<(String, u64, u64)>,
}

/// zram devices summed. KiB.
#[derive(Clone, Default, Debug)]
pub struct Zram {
    /// Uncompressed data stored.
    pub stored: u64,
    pub compressed: u64,
    /// Real RAM used by zram (compressed data + allocator overhead).
    pub ram: u64,
}

/// Where used RAM goes; parts add up to `used` (KiB).
#[derive(Clone, Default, Debug)]
pub struct Breakdown {
    pub apps: u64,
    pub zram: u64,
    pub kernel: u64,
    pub tmpfs: u64,
    pub other: u64,
    pub used: u64,
    pub reclaimable: u64,
    pub free: u64,
    pub swap_procs: u64,
    pub swap_tmpfs: u64,
    pub swap_other: u64,
}

impl Snapshot {
    pub fn breakdown(&self) -> Breakdown {
        let m = &self.mem;
        let used = m.used();
        let zram = self.zram.as_ref().map(|z| z.ram).unwrap_or(0);
        let kernel =
            m.slab_unreclaimable + m.kernel_stack + m.page_tables + m.vmalloc + m.percpu + m.gpu;
        // Scale down proportionally if the estimates overshoot `used`.
        // Without PSS (non-Linux) the best "apps" estimate is summed RSS.
        let apps = if self.detailed {
            m.anon
        } else {
            self.procs.iter().map(|p| p.rss).sum::<u64>().min(used)
        };
        let mut parts = [apps, zram, kernel, m.shmem];
        let sum: u64 = parts.iter().sum();
        if sum > used && sum > 0 {
            for p in parts.iter_mut() {
                *p = (*p as u128 * used as u128 / sum as u128) as u64;
            }
        }
        let other = used.saturating_sub(parts.iter().sum());

        let swap_used = m.swap_used();
        let swap_procs = self
            .procs
            .iter()
            .map(|p| p.swap)
            .sum::<u64>()
            .min(swap_used);
        let tmpfs_total: u64 = self.tmpfs.iter().map(|t| t.1).sum();
        let swap_tmpfs = tmpfs_total
            .saturating_sub(m.shmem)
            .min(swap_used - swap_procs);
        Breakdown {
            apps: parts[0],
            zram: parts[1],
            kernel: parts[2],
            tmpfs: parts[3],
            other,
            used,
            reclaimable: m.available.saturating_sub(m.free),
            free: m.free,
            swap_procs,
            swap_tmpfs,
            swap_other: swap_used - swap_procs - swap_tmpfs,
        }
    }
}

/// Collapse whitespace/newlines and cap length so labels stay on one line.
pub(crate) fn tidy(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        let mut t: String = flat.chars().take(max - 1).collect();
        t.push('…');
        t
    } else {
        flat
    }
}

pub(crate) fn kib(v: &str) -> u64 {
    v.split_whitespace()
        .next()
        .and_then(|x| x.parse().ok())
        .unwrap_or(0)
}

fn basename(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

/// Last `n` path components, e.g. "server/dist/bin.mjs".
fn short_path(s: &str, n: usize) -> String {
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(n)..].join("/")
}

const SCRIPT_EXT: &[&str] = &[
    ".js", ".mjs", ".cjs", ".ts", ".mts", ".py", ".rb", ".pl", ".jar", ".sh",
];
const INTERPRETERS: &[&str] = &[
    "node", "bun", "deno", "python", "python3", "ruby", "perl", "java", "electron", "bash", "sh",
    "zsh",
];

/// Derive a short (name, role) pair from argv so long Electron / node
/// command lines become readable: "t3code", "renderer" / "server/dist/bin.mjs".
pub fn describe(comm: &str, args: &[String]) -> (String, String) {
    if args.is_empty() {
        return (comm.to_string(), String::new());
    }
    // Processes that rewrite their title into a single string,
    // e.g. "postgres: checkpointer " or "sshd: user@pts/0".
    if args.len() == 1 {
        let a = args[0].trim();
        if let Some((head, tail)) = a.split_once(": ")
            && (!head.contains('/') || head.starts_with('/'))
        {
            return (basename(head).to_string(), tail.trim().to_string());
        }
        if a.contains(' ') {
            let split: Vec<String> = a.split_whitespace().map(String::from).collect();
            return describe(comm, &split);
        }
    }

    let mut name = basename(&args[0]).trim_end_matches(':').to_string();
    if name.is_empty() {
        name = comm.to_string();
    }

    // Chromium / Electron child processes.
    if let Some(t) = args.iter().find_map(|a| a.strip_prefix("--type=")) {
        let role = match t {
            "renderer" => {
                if args.iter().any(|a| a == "--extension-process") {
                    "renderer · extension".to_string()
                } else {
                    "renderer".to_string()
                }
            }
            "gpu-process" => "gpu".to_string(),
            "utility" => match args
                .iter()
                .find_map(|a| a.strip_prefix("--utility-sub-type="))
            {
                Some(sub) => format!("utility · {}", sub.rsplit('.').next().unwrap_or(sub)),
                None => "utility".to_string(),
            },
            "zygote" => {
                if args.iter().any(|a| a == "--no-zygote-sandbox") {
                    "zygote (no sandbox)".to_string()
                } else {
                    "zygote".to_string()
                }
            }
            "crashpad-handler" => "crashpad".to_string(),
            other => other.to_string(),
        };
        return (name, role);
    }

    // Interpreter / script: find the script being run, skipping --require X etc.
    let is_interp = INTERPRETERS
        .iter()
        .any(|i| name == *i || name.starts_with("python3."));
    // Shell one-liners: "zsh -c 'cmd …'" → role is the command itself.
    // Also inline code: "python3 -c '…'", "node -e '…'".
    let inline = matches!(name.as_str(), "sh" | "bash" | "zsh" | "fish" | "dash")
        || INTERPRETERS.contains(&name.as_str())
        || name.starts_with("python3.");
    if inline
        && let Some(i) = args
            .iter()
            .position(|a| matches!(a.as_str(), "-c" | "-lc" | "-e" | "--eval"))
        && let Some(cmd) = args.get(i + 1)
    {
        return (name, format!("{} {cmd}", args[i]));
    }

    let mut skip_next = false;
    let mut script = None;
    for a in &args[1..] {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(
            a.as_str(),
            "--require" | "-r" | "--import" | "--loader" | "-e" | "--eval" | "-cp" | "-classpath"
        ) {
            skip_next = true;
            continue;
        }
        if a == "-m" {
            // python -m module
            skip_next = false;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        let looks_script = SCRIPT_EXT.iter().any(|e| a.ends_with(e));
        if looks_script || (is_interp && (a.contains('/') || !a.contains(' '))) {
            script = Some(a.as_str());
            break;
        }
    }
    if let Some(s) = script {
        let role = if s.contains('/') {
            short_path(s, 3)
        } else {
            s.to_string()
        };
        return (name, role);
    }

    // Generic: leading subcommand words ("cloudflared tunnel", "opencode serve").
    let mut words = Vec::new();
    for a in &args[1..] {
        if a.starts_with('-') || words.len() >= 2 {
            break;
        }
        words.push(if a.contains('/') {
            short_path(a, 2)
        } else {
            a.clone()
        });
    }
    (name, words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(args: &[&str]) -> (String, String) {
        let v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        describe("comm", &v)
    }

    #[test]
    fn electron_roles() {
        assert_eq!(
            d(&["/opt/app/t3code", "--type=renderer", "--foo"]),
            ("t3code".into(), "renderer".into())
        );
        assert_eq!(d(&["/opt/app/t3code", "--type=gpu-process"]).1, "gpu");
        assert_eq!(
            d(&[
                "code",
                "--type=utility",
                "--utility-sub-type=network.mojom.NetworkService"
            ])
            .1,
            "utility · NetworkService"
        );
        assert_eq!(
            d(&["steamwebhelper", "--type=zygote", "--no-zygote-sandbox"]).1,
            "zygote (no sandbox)"
        );
    }

    #[test]
    fn scripts_skip_require() {
        let r = d(&[
            "/tmp/app/t3code",
            "--require",
            "/x/compileCache.cjs",
            "/x/apps/server/dist/bin.mjs",
        ]);
        assert_eq!(r, ("t3code".into(), "server/dist/bin.mjs".into()));
        assert_eq!(
            d(&["node", "/home/u/proj/node_modules/vite/bin/vite.js"]).1,
            "vite/bin/vite.js"
        );
        assert_eq!(d(&["python3", "-m", "http.server"]).1, "http.server");
    }

    #[test]
    fn inline_code_and_subcommands() {
        assert_eq!(
            d(&["bash", "-c", "sleep 1 && echo hi"]).1,
            "-c sleep 1 && echo hi"
        );
        assert_eq!(d(&["python3", "-c", "print(1)"]).1, "-c print(1)");
        assert_eq!(
            d(&["cloudflared", "tunnel", "--no-autoupdate", "run"]).1,
            "tunnel"
        );
        assert_eq!(d(&["claude", "--output-format", "stream-json"]).1, "");
    }

    #[test]
    fn rewritten_titles() {
        assert_eq!(
            d(&["postgres: checkpointer "]),
            ("postgres".into(), "checkpointer".into())
        );
    }

    #[test]
    fn windows_paths() {
        assert_eq!(
            d(&[r"C:\Program Files\App\app.exe", "--type=renderer"]).0,
            "app.exe"
        );
    }

    #[test]
    fn tidy_flattens_and_truncates() {
        assert_eq!(tidy("a\n  b\tc", 10), "a b c");
        assert_eq!(tidy("abcdefghij", 5), "abcd…");
    }
}
