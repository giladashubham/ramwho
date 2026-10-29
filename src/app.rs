use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::clean::{self, Candidate, CleanMsg};
use crate::proc::{Proc, Snapshot};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Processes grouped under the app that launched them (collapsed).
    Apps,
    /// The full parent/child tree.
    Tree,
    /// Flat list, one row per process.
    List,
    /// Where all used RAM goes (apps, zram, kernel, tmpfs…).
    Ram,
    /// Leftover tmp files and orphaned processes that can be cleared.
    Clean,
}

impl View {
    pub const ALL: [View; 5] = [View::Apps, View::Tree, View::List, View::Ram, View::Clean];
    pub fn title(self) -> &'static str {
        match self {
            View::Apps => "Apps",
            View::Tree => "Tree",
            View::List => "List",
            View::Ram => "RAM",
            View::Clean => "Clean up",
        }
    }
    pub fn is_table(self) -> bool {
        matches!(self, View::Apps | View::Tree | View::List)
    }
    /// Which process layout to compute (RAM / Clean reuse the Apps grouping).
    fn layout(self) -> View {
        if self.is_table() { self } else { View::Apps }
    }
    fn idx(self) -> usize {
        self.layout() as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Mem,
    Cpu,
    Swap,
    Pid,
    Name,
}

impl SortKey {
    pub fn title(self) -> &'static str {
        match self {
            SortKey::Mem => "memory",
            SortKey::Cpu => "cpu",
            SortKey::Swap => "swap",
            SortKey::Pid => "pid",
            SortKey::Name => "name",
        }
    }
}

/// Totals for a process plus everything below it in the current view.
#[derive(Clone, Copy, Default, Debug)]
pub struct Agg {
    pub pss: u64,
    pub swap: u64,
    pub cpu: f32,
    pub count: usize,
}

pub struct Row {
    pub idx: usize,
    pub prefix: String,
    pub has_children: bool,
    pub expanded: bool,
}

pub struct KillDialog {
    pub pid: i32,
    pub name: String,
    pub role: String,
    /// Real descendants (by ppid), deepest first.
    pub descendants: Vec<i32>,
    pub footprint: u64,
    pub choice: usize,
}

impl KillDialog {
    pub fn options(&self) -> Vec<(&'static str, String)> {
        let mut o = vec![
            ("SIGTERM", "ask it to exit gracefully".to_string()),
            ("SIGKILL", "force kill immediately".to_string()),
        ];
        if !self.descendants.is_empty() {
            let n = self.descendants.len() + 1;
            o.push(("SIGTERM", format!("whole tree ({n} processes)")));
            o.push(("SIGKILL", format!("whole tree ({n} processes)")));
        }
        o
    }
}

pub enum Modal {
    Kill(KillDialog),
    Help,
    /// Confirm clearing: (count of paths, count of processes, KiB).
    Clean(usize, usize, u64),
}

pub struct App {
    pub snap: Snapshot,
    pub view: View,
    pub sort: SortKey,
    pub rows: Vec<Row>,
    pub agg: Vec<Agg>,
    pub table: TableState,
    selected_pid: Option<i32>,
    /// Per view: pids whose expansion differs from the view's default.
    toggled: [HashSet<i32>; 3],
    pub filter: String,
    pub filter_mode: bool,
    pub show_kernel: bool,
    pub show_detail: bool,
    pub modal: Option<Modal>,
    pub status: Option<(String, Instant, bool)>,
    pub interval_ms: u64,
    pub quit: bool,
    pub page: usize,
    pub clean: Vec<Candidate>,
    pub clean_table: TableState,
    clean_rx: Option<Receiver<CleanMsg>>,
    /// "scanning" / "cleaning" while the background job runs.
    pub clean_busy: Option<&'static str>,
}

/// Session/compositor processes: in the Apps view their children become
/// top-level groups instead of all collapsing under "systemd".
fn is_launcher(p: &Proc) -> bool {
    p.pid == 1
        || p.kernel
        || matches!(
            p.comm.as_str(),
            "systemd"
                | "init"
                | "gnome-shell"
                | "gnome-session-b"
                | "kwin_wayland"
                | "kwin_x11"
                | "plasmashell"
                | "ksmserver"
                | "startplasma-wa"
                | "sway"
                | "Hyprland"
                | "niri"
                | "cosmic-session"
                | "dbus-daemon"
                | "dbus-broker"
                | "dbus-broker-lau"
                | "sddm"
                | "gdm"
                | "gdm-wayland-ses"
                | "login"
                // macOS
                | "launchd"
                | "loginwindow"
                // Windows
                | "System"
                | "smss.exe"
                | "csrss.exe"
                | "wininit.exe"
                | "winlogon.exe"
                | "services.exe"
                | "svchost.exe"
                | "explorer.exe"
                | "Explorer.EXE"
        )
}

impl App {
    pub fn new(snap: Snapshot, interval_ms: u64) -> Self {
        let mut app = App {
            snap,
            view: View::Apps,
            sort: SortKey::Mem,
            rows: Vec::new(),
            agg: Vec::new(),
            table: TableState::default().with_selected(0),
            selected_pid: None,
            toggled: Default::default(),
            filter: String::new(),
            filter_mode: false,
            show_kernel: false,
            show_detail: true,
            modal: None,
            status: None,
            interval_ms,
            quit: false,
            page: 20,
            clean: Vec::new(),
            clean_table: TableState::default().with_selected(0),
            clean_rx: None,
            clean_busy: None,
        };
        app.rebuild();
        app
    }

    pub fn update(&mut self, snap: Snapshot) {
        self.snap = snap;
        self.rebuild();
    }

    pub fn selected(&self) -> Option<&Proc> {
        let row = self.rows.get(self.table.selected()?)?;
        self.snap.procs.get(row.idx)
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.table.selected()?)
    }

    fn default_expanded(&self) -> bool {
        self.view.layout() == View::Tree
    }

    pub fn expand_all(&mut self, expand: bool) {
        let v = self.view.idx();
        self.toggled[v].clear();
        if expand != self.default_expanded() {
            self.toggled[v] = self.snap.procs.iter().map(|p| p.pid).collect();
        }
        self.rebuild();
    }

    /// Recompute view tree, subtree totals, filtering, sorting and the flat
    /// list of rows to display. Selection follows the selected pid.
    pub fn rebuild(&mut self) {
        let procs = &self.snap.procs;
        let n = procs.len();
        let visible: Vec<bool> = procs
            .iter()
            .map(|p| self.show_kernel || !p.kernel)
            .collect();
        let by_pid: HashMap<i32, usize> =
            procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for i in 0..n {
            if !visible[i] {
                continue;
            }
            let layout = self.view.layout();
            let parent = match layout {
                View::List => None,
                _ => by_pid.get(&procs[i].ppid).copied().filter(|&j| {
                    j != i && visible[j] && !(layout == View::Apps && is_launcher(&procs[j]))
                }),
            };
            match parent {
                Some(j) => children[j].push(i),
                None => roots.push(i),
            }
        }

        let mut agg = vec![Agg::default(); n];
        fn sum(i: usize, procs: &[Proc], children: &[Vec<usize>], agg: &mut [Agg]) {
            let p = &procs[i];
            let mut a = Agg {
                pss: p.pss,
                swap: p.swap,
                cpu: p.cpu,
                count: 1,
            };
            for &c in &children[i] {
                sum(c, procs, children, agg);
                let ca = agg[c];
                a.pss += ca.pss;
                a.swap += ca.swap;
                a.cpu += ca.cpu;
                a.count += ca.count;
            }
            agg[i] = a;
        }
        for &r in &roots {
            sum(r, procs, &children, &mut agg);
        }

        // Filter: keep matches plus their ancestors.
        let needle = self.filter.to_lowercase();
        let mut keep = vec![true; n];
        if !needle.is_empty() {
            fn mark(
                i: usize,
                procs: &[Proc],
                ch: &[Vec<usize>],
                needle: &str,
                keep: &mut [bool],
            ) -> bool {
                let p = &procs[i];
                let mut hit = p.name.to_lowercase().contains(needle)
                    || p.role.to_lowercase().contains(needle)
                    || p.cmdline.to_lowercase().contains(needle)
                    || p.user.to_lowercase().contains(needle)
                    || p.pid.to_string() == needle;
                for &c in &ch[i] {
                    hit |= mark(c, procs, ch, needle, keep);
                }
                keep[i] = hit;
                hit
            }
            for &r in &roots {
                mark(r, procs, &children, &needle, &mut keep);
            }
        }

        let sort = self.sort;
        let cmp = |a: &usize, b: &usize| -> Ordering {
            let (x, y) = (&agg[*a], &agg[*b]);
            let (pa, pb) = (&procs[*a], &procs[*b]);
            match sort {
                SortKey::Mem => y.pss.cmp(&x.pss),
                SortKey::Swap => y.swap.cmp(&x.swap),
                SortKey::Cpu => y.cpu.partial_cmp(&x.cpu).unwrap_or(Ordering::Equal),
                SortKey::Pid => pa.pid.cmp(&pb.pid),
                SortKey::Name => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
            }
            .then(pa.pid.cmp(&pb.pid))
        };
        roots.retain(|&i| keep[i]);
        roots.sort_by(cmp);
        for c in children.iter_mut() {
            c.retain(|&i| keep[i]);
            c.sort_by(cmp);
        }

        let filtering = !needle.is_empty();
        let default_expanded = self.default_expanded();
        let toggled = &self.toggled[self.view.idx()];
        let mut rows = Vec::with_capacity(n);
        struct Ctx<'a> {
            procs: &'a [Proc],
            children: &'a [Vec<usize>],
            toggled: &'a HashSet<i32>,
            default_expanded: bool,
            filtering: bool,
        }
        fn walk(i: usize, lead: &str, branch: &str, cont: &str, cx: &Ctx, rows: &mut Vec<Row>) {
            let kids = &cx.children[i];
            let expanded =
                cx.filtering || (cx.default_expanded != cx.toggled.contains(&cx.procs[i].pid));
            rows.push(Row {
                idx: i,
                prefix: format!("{lead}{branch}"),
                has_children: !kids.is_empty(),
                expanded,
            });
            if expanded {
                let child_lead = format!("{lead}{cont}");
                for (k, &c) in kids.iter().enumerate() {
                    let last = k + 1 == kids.len();
                    let (b, nc) = if last {
                        ("└─ ", "   ")
                    } else {
                        ("├─ ", "│  ")
                    };
                    walk(c, &child_lead, b, nc, cx, rows);
                }
            }
        }
        let cx = Ctx {
            procs,
            children: &children,
            toggled,
            default_expanded,
            filtering,
        };
        for &r in &roots {
            walk(r, "", "", "", &cx, &mut rows);
        }

        self.rows = rows;
        self.agg = agg;
        self.restore_selection();
    }

    fn restore_selection(&mut self) {
        if self.rows.is_empty() {
            self.table.select(None);
            return;
        }
        let pos = self.selected_pid.and_then(|pid| {
            self.rows
                .iter()
                .position(|r| self.snap.procs[r.idx].pid == pid)
        });
        let i = pos.unwrap_or_else(|| self.table.selected().unwrap_or(0).min(self.rows.len() - 1));
        self.select(i);
    }

    fn select(&mut self, i: usize) {
        if self.rows.is_empty() {
            return;
        }
        let i = i.min(self.rows.len() - 1);
        self.table.select(Some(i));
        self.selected_pid = Some(self.snap.procs[self.rows[i].idx].pid);
    }

    fn move_by(&mut self, d: isize) {
        let cur = self.table.selected().unwrap_or(0) as isize;
        self.select((cur + d).max(0) as usize);
    }

    fn toggle(&mut self, expand: Option<bool>) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if !row.has_children {
            return;
        }
        let want = expand.unwrap_or(!row.expanded);
        if want == row.expanded {
            return;
        }
        let pid = self.snap.procs[row.idx].pid;
        let set = &mut self.toggled[self.view.idx()];
        if !set.remove(&pid) {
            set.insert(pid);
        }
        self.rebuild();
    }

    fn go_parent(&mut self) {
        let Some(sel) = self.table.selected() else {
            return;
        };
        let depth = |r: &Row| r.prefix.chars().count();
        let d = depth(&self.rows[sel]);
        if d == 0 {
            return;
        }
        if let Some(p) = (0..sel).rev().find(|&i| depth(&self.rows[i]) < d) {
            self.select(p);
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>, err: bool) {
        self.status = Some((msg.into(), Instant::now(), err));
    }

    fn open_kill(&mut self) {
        let Some(p) = self.selected() else { return };
        if p.pid <= 1 || p.kernel {
            self.set_status("refusing to kill init / kernel threads", true);
            return;
        }
        // Real descendants by ppid (BFS), sent leaf-first.
        let mut kids: HashMap<i32, Vec<i32>> = HashMap::new();
        for q in &self.snap.procs {
            kids.entry(q.ppid).or_default().push(q.pid);
        }
        let mut order = Vec::new();
        let mut queue = vec![p.pid];
        while let Some(x) = queue.pop() {
            for &c in kids.get(&x).map(Vec::as_slice).unwrap_or(&[]) {
                order.push(c);
                queue.push(c);
            }
        }
        order.reverse();
        let footprint = self
            .selected_row()
            .map(|r| self.agg[r.idx].pss + self.agg[r.idx].swap)
            .unwrap_or(p.pss + p.swap);
        self.modal = Some(Modal::Kill(KillDialog {
            pid: p.pid,
            name: p.name.clone(),
            role: p.role.clone(),
            descendants: order,
            footprint,
            choice: 0,
        }));
    }

    fn do_kill(&mut self, d: &KillDialog) {
        let force = d.choice % 2 == 1;
        let sig_name = if force { "SIGKILL" } else { "SIGTERM" };
        let mut targets = if d.choice >= 2 {
            d.descendants.clone()
        } else {
            Vec::new()
        };
        targets.push(d.pid);
        let (mut ok, mut err) = (0, None);
        for t in targets {
            if t <= 1 || t == self.snap.self_pid {
                continue;
            }
            match crate::sys::terminate(t, force) {
                Ok(()) => ok += 1,
                Err(e) => err = Some(e),
            }
        }
        match err {
            None => self.set_status(format!("sent {sig_name} to {ok} process(es) · {} ({})", d.name, d.pid), false),
            Some(e) if crate::sys::is_permission_error(&e) => self.set_status(
                format!("{sig_name}: sent to {ok}, permission denied for others (owned by another user — try sudo)"),
                true,
            ),
            Some(e) => self.set_status(format!("{sig_name}: sent to {ok}, error: {e}"), true),
        }
    }

    /// Returns true if the sampler should refresh immediately.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return false;
        }

        if let Some(modal) = self.modal.take() {
            match modal {
                Modal::Help => {}
                Modal::Clean(..) => {
                    if matches!(key.code, KeyCode::Enter | KeyCode::Char('y')) {
                        self.start_clean();
                    }
                }
                Modal::Kill(mut d) => {
                    let n = d.options().len();
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => d.choice = (d.choice + n - 1) % n,
                        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                            d.choice = (d.choice + 1) % n
                        }
                        KeyCode::Char('9') => {
                            d.choice = 1;
                            self.do_kill(&d);
                            return true;
                        }
                        KeyCode::Enter | KeyCode::Char('y') => {
                            self.do_kill(&d);
                            return true;
                        }
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => return false,
                        _ => {}
                    }
                    self.modal = Some(Modal::Kill(d));
                }
            }
            return false;
        }

        if self.filter_mode {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_mode = false;
                }
                KeyCode::Enter | KeyCode::Down | KeyCode::Up => self.filter_mode = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => return false,
            }
            self.clean_table.select(Some(0));
            self.rebuild();
            return false;
        }

        // Keys that work everywhere.
        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                return false;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let i = View::ALL.iter().position(|v| *v == self.view).unwrap_or(0);
                let n = View::ALL.len();
                let next = if key.code == KeyCode::Tab {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                self.set_view(View::ALL[next]);
                return false;
            }
            KeyCode::Char(c @ '1'..='5') => {
                self.set_view(View::ALL[c as usize - '1' as usize]);
                return false;
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.modal = Some(Modal::Help);
                return false;
            }
            _ => {}
        }
        match self.view {
            View::Clean => return self.clean_key(key),
            View::Ram => return key.code == KeyCode::Char('r'),
            _ => {}
        }

        match key.code {
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild();
                }
            }
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(self.page as isize),
            KeyCode::PageUp => self.move_by(-(self.page as isize)),
            KeyCode::Home | KeyCode::Char('g') => self.select(0),
            KeyCode::End | KeyCode::Char('G') => self.select(usize::MAX),
            KeyCode::Left | KeyCode::Char('h') => match self.selected_row() {
                Some(r) if r.has_children && r.expanded => self.toggle(Some(false)),
                _ => self.go_parent(),
            },
            KeyCode::Right | KeyCode::Char('l') => self.toggle(Some(true)),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle(None),
            KeyCode::Char('e') => self.expand_all(true),
            KeyCode::Char('E') => self.expand_all(false),
            KeyCode::Char('/') => self.filter_mode = true,
            KeyCode::Char('m') | KeyCode::Char('M') => self.set_sort(SortKey::Mem),
            KeyCode::Char('c') | KeyCode::Char('P') => self.set_sort(SortKey::Cpu),
            KeyCode::Char('s') => self.set_sort(SortKey::Swap),
            KeyCode::Char('p') => self.set_sort(SortKey::Pid),
            KeyCode::Char('n') => self.set_sort(SortKey::Name),
            KeyCode::Char('k') | KeyCode::Char('x') | KeyCode::Delete | KeyCode::F(9) => {
                self.open_kill()
            }
            KeyCode::Char('t') => {
                self.show_kernel = !self.show_kernel;
                self.rebuild();
            }
            KeyCode::Char('d') => self.show_detail = !self.show_detail,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.interval_ms = (self.interval_ms + 500).min(10_000);
                self.set_status(
                    format!("refresh every {:.1}s", self.interval_ms as f64 / 1000.0),
                    false,
                );
            }
            KeyCode::Char('-') => {
                self.interval_ms = self.interval_ms.saturating_sub(500).max(500);
                self.set_status(
                    format!("refresh every {:.1}s", self.interval_ms as f64 / 1000.0),
                    false,
                );
            }
            KeyCode::Char('r') => return true,
            _ => {}
        }
        false
    }

    pub fn start_scan(&mut self) {
        if self.clean_busy.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let procs = self.snap.procs.clone();
        std::thread::spawn(move || {
            let _ = tx.send(CleanMsg::Scanned(clean::scan(&procs)));
        });
        self.clean_rx = Some(rx);
        self.clean_busy = Some("scanning");
    }

    fn start_clean(&mut self) {
        // Only what's visible: a filter must never clear hidden items.
        let items: Vec<Candidate> = self
            .clean_visible()
            .into_iter()
            .map(|i| &self.clean[i])
            .filter(|c| c.selected && c.in_use_by.is_none())
            .cloned()
            .collect();
        if items.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(clean::clean(items));
        });
        self.clean_rx = Some(rx);
        self.clean_busy = Some("cleaning");
    }

    /// Poll the background scan/clean job. Returns true if something changed.
    pub fn poll_clean(&mut self) -> bool {
        let Some(rx) = &self.clean_rx else {
            return false;
        };
        let msg = match rx.try_recv() {
            Ok(msg) => msg,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.clean_rx = None;
                self.clean_busy = None;
                self.set_status("clean-up job failed unexpectedly", true);
                return true;
            }
        };
        self.clean_rx = None;
        self.clean_busy = None;
        match msg {
            CleanMsg::Scanned(v) => {
                self.clean = v;
                let sel = self
                    .clean_table
                    .selected()
                    .unwrap_or(0)
                    .min(self.clean.len().saturating_sub(1));
                self.clean_table.select(Some(sel));
            }
            CleanMsg::Done {
                removed,
                freed,
                failed,
            } => {
                if failed.is_empty() {
                    self.set_status(
                        format!(
                            "cleared {removed} item(s) · {} freed",
                            crate::ui::human(freed)
                        ),
                        false,
                    );
                } else {
                    self.set_status(
                        format!("cleared {removed}, {} failed: {}", failed.len(), failed[0]),
                        true,
                    );
                }
                self.start_scan();
            }
        }
        true
    }

    pub fn clean_selected(&self) -> (usize, usize, u64) {
        let vis = self.clean_visible();
        let sel = vis
            .iter()
            .map(|&i| &self.clean[i])
            .filter(|c| c.selected && c.in_use_by.is_none());
        let (mut paths, mut procs, mut size) = (0, 0, 0);
        for c in sel {
            if matches!(c.target, clean::Target::Proc { .. }) {
                procs += 1
            } else {
                paths += 1
            }
            size += c.size;
        }
        (paths, procs, size)
    }

    /// Indices into `clean` that match the filter.
    pub fn clean_visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        (0..self.clean.len())
            .filter(|&i| {
                let c = &self.clean[i];
                needle.is_empty()
                    || c.name.to_lowercase().contains(&needle)
                    || c.root.contains(&needle)
            })
            .collect()
    }

    fn clean_key(&mut self, key: KeyEvent) -> bool {
        let vis = self.clean_visible();
        let n = vis.len();
        let cur = self
            .clean_table
            .selected()
            .unwrap_or(0)
            .min(n.saturating_sub(1));
        let mv = |d: isize| ((cur as isize + d).max(0) as usize).min(n.saturating_sub(1));
        match key.code {
            KeyCode::Down => self.clean_table.select(Some(mv(1))),
            KeyCode::Up => self.clean_table.select(Some(mv(-1))),
            KeyCode::PageDown => self.clean_table.select(Some(mv(self.page as isize))),
            KeyCode::PageUp => self.clean_table.select(Some(mv(-(self.page as isize)))),
            KeyCode::Home | KeyCode::Char('g') => self.clean_table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.clean_table.select(Some(n.saturating_sub(1))),
            KeyCode::Char('/') => self.filter_mode = true,
            KeyCode::Esc => self.filter.clear(),
            KeyCode::Char(' ') => {
                if let Some(c) = vis.get(cur).and_then(|&i| self.clean.get_mut(i))
                    && c.in_use_by.is_none()
                {
                    c.selected = !c.selected;
                }
                self.clean_table.select(Some(mv(1)));
            }
            // Bulk actions apply to what's visible (respecting the filter).
            // Processes are only ever ticked one by one with space.
            KeyCode::Char('a') => vis.iter().for_each(|&i| {
                let c = &mut self.clean[i];
                c.selected = c.in_use_by.is_none() && matches!(c.target, clean::Target::Path(_));
            }),
            KeyCode::Char('n') => vis.iter().for_each(|&i| self.clean[i].selected = false),
            KeyCode::Char('o') => vis
                .iter()
                .for_each(|&i| self.clean[i].selected = clean::default_pick(&self.clean[i])),
            KeyCode::Char('r') => self.start_scan(),
            KeyCode::Enter | KeyCode::Char('x') | KeyCode::Delete => {
                let (paths, procs, size) = self.clean_selected();
                if paths + procs == 0 {
                    self.set_status("nothing selected (space to select)", true);
                } else if self.clean_busy.is_none() {
                    self.modal = Some(Modal::Clean(paths, procs, size));
                }
            }
            _ => {}
        }
        false
    }

    fn set_view(&mut self, v: View) {
        self.view = v;
        if v == View::Clean && self.clean.is_empty() {
            self.start_scan();
        }
        self.rebuild();
    }

    fn set_sort(&mut self, s: SortKey) {
        self.sort = s;
        self.rebuild();
    }
}
