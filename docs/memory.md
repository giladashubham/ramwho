# How ramwho counts memory

## Per process (Linux)

For each process, ramwho reads `/proc/<pid>/smaps_rollup`:

| ramwho | kernel field | meaning |
|---|---|---|
| MEM / SELF | `Pss` | Proportional set size: private pages, plus each shared page divided by the number of processes sharing it |
| heap | `Pss_Anon` | Heap, stacks and other anonymous memory |
| file | `Pss_File` | Mapped files: the binary, libraries, mmapped data |
| shm | `Pss_Shmem` | Shared memory and tmpfs mappings |
| unique | `Private_Clean + Private_Dirty` | Memory freed if this process alone exits |
| swap | `SwapPss` | Swapped-out pages, shared fairly |

Because PSS splits shared pages, **adding up a group gives its real RAM use.** RSS (htop's `RES`) counts shared pages again for every process. Electron, Chromium and node apps share a lot, so RSS totals can be far too high.

`smaps_rollup` is only readable for your own processes. For other users' processes (e.g. root daemons), ramwho falls back to `VmRSS` / `VmSwap` from `/proc/<pid>/status` and marks the value with `~`.

Threads are never shown as separate rows. htop lists every thread by default (press `H` to hide them), and each thread row repeats the whole process's memory.

## The RAM tab: where "used" goes

"Used" is `MemTotal − MemAvailable`. The RAM tab splits it into parts that add up exactly:

| row | source | notes |
|---|---|---|
| Apps | `AnonPages` | Heap and stacks of all processes |
| zram | `/sys/block/zram*/mm_stat` → `mem_used_total` | Compressed swap kept **in RAM**. No process owns it, so it never shows in a process list. |
| Kernel | `SUnreclaim + KernelStack + PageTables + SecPageTables + VmallocUsed + Percpu + GPUActive` | |
| /tmp & shared mem | `Shmem` | tmpfs files and shared memory that are resident right now |
| Cache in use / other | remainder | File cache the kernel isn't counting as available |

If the estimates add up to more than "used" (some counters overlap on some kernels), they are scaled down proportionally so the rows still sum to the total.

The Apps tab's Σ MEM totals are usually ~1 GB higher than the RAM tab's "Apps" row. PSS also includes mapped files (binaries, libraries), and the kernel counts those as page cache.

### What's in swap

- **Processes**: the sum of `SwapPss` across all processes
- **/tmp files (≈)**: tmpfs usage that isn't resident (`Σ tmpfs used − Shmem`). tmpfs pages get swapped like anything else, and old files in `/tmp` often end up filling swap.
- **Other**: the remainder

## macOS and Windows

These use the [`sysinfo`](https://crates.io/crates/sysinfo) crate, which reports resident memory per process (RSS on macOS, working set on Windows). Shared pages are counted in each process, so group totals can overstate real usage. Per-process swap and the detailed breakdown aren't available. The RAM tab shows "Apps" (summed resident memory, capped at used) and "System & other".
