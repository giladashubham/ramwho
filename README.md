# ramwho

**Which apps are *really* using your RAM?** `ramwho` is a fast, good-looking terminal process monitor that answers the question `htop` makes hard.

```
╭ ◉ ramwho ──────────────────────────────────────────────────────────── up 7d 20h ╮
│  0 ━━━━━━━━━━   5%   4 ━━━━━━━━━━   3%      RAM  ━━━━━━━━━━━━━━  8.85G / 13.5G   │
│  1 ━━━━━━━━━━   0%   5 ━━━━━━━━━━   1%      SWAP ━━━━━━━━━━━━━━  6.08G / 8.00G   │
╰─────────────────────────────────────────────────────────────────────────────────╯
  1 Apps   2 Tree   3 List   4 RAM   5 Clean up
╭ 138 groups ─────────────────────────────────────────────────────────────────────╮
│      PID NAME                                    CPU%   Σ MEM▼  Σ SWAP    SELF  │
│▌ 1740029 ▾ t3code                                 5.0    1.43G    267M   86.3M  │
│  1740438 ├─ ▸ t3code ×9  server/dist/bin.mjs      4.1     962M   74.4M    222M  │
│  1740056 ├─ ▸ t3code ×3  zygote                   0.6     362M   38.4M    647K  │
│  1740055 ├─ ▸ t3code ×2  zygote (no sandbox)      0.0    56.3M   87.1M    308K  │
│  1740102 └─   t3code  utility · NetworkService    0.5    10.6M   10.8M   10.6M  │
│  2203447 ▸ firefox ×12                           37.4    1.13G       ·    235K  │
╰─────────────────────────────────────────────────────────────────────────────────╯
```

## Why not htop?

| htop | ramwho |
|---|---|
| Every **thread** gets its own row, each repeating the process's memory | One row per **process** |
| RES counts shared memory once per process, so totals overshoot | **PSS**: shared pages are split fairly, so totals add up (Linux) |
| Flat list of 40 `chrome`/`electron`/`node` rows | Processes **grouped by app**, with totals. Expand a group to see what's inside |
| Long, unreadable command lines | Short labels: `renderer`, `gpu`, `utility · NetworkService`, `server/dist/bin.mjs`, `tunnel` |
| No idea where the rest of "used" RAM went | **RAM tab**: apps, zram, kernel, tmpfs and cache, adding up to the total |
| — | **Swap per process**, plus what's in swap (processes vs `/tmp` files) |
| — | **Clean up**: finds `/tmp` leftovers and detached dev processes nobody uses |

## Features

- **5 views**: `Apps` (grouped), `Tree` (full parent/child tree), `List` (flat), `RAM` (breakdown), `Clean up`
- **Kill** a process or its whole tree (SIGTERM or SIGKILL). It refuses to kill PID 1 or kernel threads.
- **Filter** by name, command line, user or PID. **Sort** by memory, CPU, swap, PID or name.
- **Details pane**: memory split (heap / file / shared), unique memory, RSS, swap, parent, full command
- **Fast**: a single ~1 MB binary. Sampling runs on a background thread and it barely uses CPU when idle.
- **Catppuccin Mocha** colors, rounded panels, per-core CPU bars, stacked RAM bars

## Platform support

| | Linux | macOS | Windows |
|---|:-:|:-:|:-:|
| Apps / Tree / List views, CPU, filter, sort | ✅ | ✅ | ✅ |
| Kill process / whole tree | ✅ | ✅ | ✅ (both options end the process) |
| Exact per-process memory (PSS) | ✅ | resident memory (RSS) | working set (RSS) |
| Per-process swap | ✅ | — | — |
| Full RAM breakdown (zram, kernel, tmpfs) | ✅ | apps vs system | apps vs system |
| Clean up tab | ✅ | — | — |

Linux reads `/proc` directly. macOS and Windows use the [`sysinfo`](https://crates.io/crates/sysinfo) crate. On those platforms memory is resident memory, so shared pages are counted in every process that maps them.

## Install

**Prebuilt binaries:** download from [Releases](https://github.com/giladashubham/ramwho/releases) (Linux x86_64/arm64, macOS Intel/Apple Silicon, Windows x86_64), unpack, and put `ramwho` on your `PATH`.

**With Cargo** (Rust 1.85+):

```bash
cargo install --git https://github.com/giladashubham/ramwho
```

**From source:**

```bash
git clone https://github.com/giladashubham/ramwho && cd ramwho
cargo install --path .        # or: cargo build --release && ./target/release/ramwho
```

To update, run the same `cargo install` command again with `--force`.

## Usage

```bash
ramwho                  # open the TUI
ramwho -i 1000          # refresh every second (default 2000 ms)
ramwho --once           # print a text snapshot (apps view, expanded) and exit
ramwho --once --tree    # … or --list
```

### Keys

| key | action |
| --- | --- |
| `↑ ↓` `PgUp PgDn` `g G` | move |
| `→`/`l`, `←`/`h`, `enter`/`space` | expand, collapse / jump to parent, toggle |
| `e` / `E` | expand all / collapse all |
| `tab`, `1`–`5` | Apps · Tree · List · RAM · Clean up |
| `/` | filter (`esc` clears) |
| `m c s p n` | sort by mem · cpu · swap · pid · name |
| `k` `F9` `del` | kill dialog (`9` = instant SIGKILL) |
| `t` | show kernel threads |
| `d` | toggle details pane |
| `+` `-` | slower / faster refresh |
| `r` | refresh now / rescan |
| `?` | help |
| `q` `ctrl-c` | quit |

In **Clean up**: `space` toggles an item, `a` selects all files, `o` selects old items only, `n` selects none, `enter` clears the selection (with confirmation) and `r` rescans.

### Reading the numbers

- **Σ MEM / Σ SWAP**: totals for a process *and everything under it* in the current view. **SELF** is the process alone.
- **`×18`** on a collapsed row: the group contains 18 processes.
- **`~`** (Linux): the process belongs to another user, so its exact memory isn't readable. The value shown is RSS.
- On Linux with zram (Fedora's default), swap is compressed RAM, so **Σ MEM + Σ SWAP** is an app's real footprint.

More detail: [how memory is counted](docs/memory.md) · [how Clean up stays safe](docs/clean-up.md).

## How it's built

Rust + [ratatui](https://ratatui.rs). The code is laid out as follows:

```
src/
  main.rs         event loop, background sampler thread, CLI
  app.rs          state: views, grouping, subtree totals, filter/sort, kill
  ui.rs           all drawing
  proc.rs         platform-neutral model + command-line → label logic
  sys/linux.rs    /proc backend (PSS, swap, zram, tmpfs)
  sys/portable.rs sysinfo backend (macOS, Windows, BSD)
  clean/          Clean up scanner (Linux)
```

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt --check
```

## License

[MIT](LICENSE) © Shubham Gilada
