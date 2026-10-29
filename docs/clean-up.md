# Clean up: what it removes and how it stays safe

On many Linux systems `/tmp` is a **tmpfs**, so every file in it lives in RAM (or in swap, which on zram systems is also RAM). Old build folders, downloads and test output in `/tmp` quietly eat memory until reboot. The Clean up tab (`5`) finds them.

## What it lists

1. **Top-level entries in `/tmp` and `/dev/shm`** that you own, with size (allocated blocks) and when anything inside was last modified.
2. **Detached dev processes**: `node`, `bun`, `deno`, `python`, `uv`, `vite`, `esbuild`, `tsserver`, `rust-analyzer`, `claude` and similar. These are processes that were re-parented to `systemd`/`init` because the terminal, editor or app that started them exited. They must also have no children, be nearly idle (< 1% CPU), and not be part of a systemd `.service`. Some are servers you started on purpose with `nohup`/`setsid`, so check before stopping them.

## Safety rules

- **In-use items are locked (🔒) and can't be selected.** An entry counts as in use if any running process:
  - has a file inside it open (`/proc/*/fd`)
  - has its working directory, executable or root inside it
  - has a file inside it memory-mapped (`/proc/*/maps`, including deleted files)
  - has a unix socket bound inside it (`/proc/net/unix`)

  A detached process that is using files in `/tmp` is locked too.
- **Never offered:** session and system entries (`.X11-unix`, `.ICE-unix`, `systemd-private-*`, `tmux-*`, `ssh-*`, `pulse-*`, `dbus-*`, `.mount_*` AppImage mounts, …), mount points, sockets, FIFOs, devices, and anything owned by another user.
- **Pre-selection is conservative.** Only files and folders that are unused, **1 MB or larger**, untouched for **24 hours**, and not `*.lock` are ticked. Processes are never pre-selected, and `a` (select all) skips them. You tick processes one at a time with `space`.
- **A filter limits the action.** With a filter active, Clear only affects visible items, never hidden ones.
- **Confirmation first.** `enter` shows what will be deleted or stopped and how much it frees.
- **Usage is checked again right before deleting.** Anything that started using an entry since the scan is skipped. A process is only signalled if its start time still matches, so a reused PID is never hit.
- Deletion is limited to direct children of `/tmp` and `/dev/shm`, and symlinks are removed without following them.
- Processes get `SIGTERM` (a graceful stop), not `SIGKILL`.

## Expectations

Deleting swapped-out tmpfs files frees the compressed space in zram. With a typical 2× compression ratio, clearing 3 GB from `/tmp` frees about 1.5 GB of real RAM.

Clean up is Linux-only for now. It relies on `/proc` to prove nothing uses a file. On macOS and Windows the temp folder is on disk anyway, so it doesn't hold RAM.
