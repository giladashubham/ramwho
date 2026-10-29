// Some model fields are only filled by the Linux backend.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod app;
mod clean;
mod proc;
mod sys;
mod ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::{App, View};
use sys::Sampler;

/// Set by SIGTERM/SIGHUP so the loop exits through `ratatui::restore()`.
static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

const USAGE: &str = "ramwho — which apps are really using your RAM

USAGE:
    ramwho [OPTIONS]

OPTIONS:
    -i, --interval <ms>   refresh interval (default 2000)
    --once                print one snapshot as text and exit
    --tree | --list       view for --once (default: apps, fully expanded)
    -V, --version         print version
    -h, --help            show this help";

fn main() -> anyhow::Result<()> {
    let mut interval_ms = 2000u64;
    let mut once = false;
    let mut view = View::Apps;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-i" | "--interval" => {
                interval_ms = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(interval_ms)
                    .max(250)
            }
            "--once" => once = true,
            "--tree" => view = View::Tree,
            "--list" => view = View::List,
            "-V" | "--version" => {
                println!("ramwho {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}\n\n{USAGE}"),
        }
    }

    if once {
        print_once(view);
        return Ok(());
    }

    // Sampler runs on its own thread so scanning /proc never blocks input.
    let (snap_tx, snap_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    let interval = Arc::new(AtomicU64::new(interval_ms));
    let interval_bg = interval.clone();
    let mut sampler = Sampler::new();
    sampler.sample();
    thread::sleep(Duration::from_millis(250));
    let first = sampler.sample();
    thread::spawn(move || {
        loop {
            match wake_rx.recv_timeout(Duration::from_millis(interval_bg.load(Ordering::Relaxed))) {
                Err(RecvTimeoutError::Disconnected) => break,
                _ => {
                    if snap_tx.send(sampler.sample()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
    }
    let mut app = App::new(first, interval_ms);
    // ratatui::init sets up raw mode + alt screen and a panic hook that restores the terminal.
    let mut terminal = ratatui::init();
    let res = (|| -> anyhow::Result<()> {
        let mut dirty = true;
        while !app.quit {
            if app.poll_clean() {
                dirty = true;
            }
            while let Ok(snap) = snap_rx.try_recv() {
                app.update(snap);
                dirty = true;
            }
            if dirty {
                terminal.draw(|f| ui::draw(f, &mut app))?;
                dirty = false;
            }
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if app.handle_key(k) {
                            let _ = wake_tx.send(());
                        }
                        interval.store(app.interval_ms, Ordering::Relaxed);
                    }
                    Event::Resize(..) => {}
                    _ => continue,
                }
                dirty = true;
            } else if app.clean_busy.is_some() {
                // animate the spinner while scanning/cleaning
                dirty = true;
            } else if app
                .status
                .as_ref()
                .is_some_and(|(_, at, _)| at.elapsed().as_secs() >= 5)
            {
                // status toast expired: clear it and redraw once
                app.status = None;
                dirty = true;
            }
            if STOP.load(Ordering::Relaxed) {
                app.quit = true;
            }
        }
        Ok(())
    })();
    ratatui::restore();
    res
}

fn print_once(view: View) {
    let mut s = Sampler::new();
    s.sample();
    thread::sleep(Duration::from_millis(500));
    let mut app = App::new(s.sample(), 0);
    app.view = view;
    app.expand_all(true);
    println!(
        "{:>8} {:>6} {:>8} {:>8} {:>8}  NAME",
        "PID", "CPU%", "MEM", "SWAP", "SELF"
    );
    for r in &app.rows {
        let p = &app.snap.procs[r.idx];
        let a = &app.agg[r.idx];
        println!(
            "{:>8} {:>6.1} {:>8} {:>8} {:>8}  {}{}{}",
            p.pid,
            a.cpu,
            ui::human(a.pss),
            ui::human(a.swap),
            ui::human(p.pss),
            r.prefix,
            p.name,
            if p.role.is_empty() {
                String::new()
            } else {
                format!("  [{}]", p.role)
            }
        );
    }
    let m = &app.snap.mem;
    let total_pss: u64 = app.snap.procs.iter().map(|p| p.pss).sum();
    eprintln!(
        "\nsum of process memory ({}) {} · used RAM (total-available) {} · swap used {}",
        if app.snap.detailed { "PSS" } else { "RSS" },
        ui::human(total_pss),
        ui::human(m.used()),
        ui::human(m.swap_used())
    );
}
