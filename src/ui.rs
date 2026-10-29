use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Clear, HighlightSpacing, Padding, Paragraph, Row as TRow, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};

use crate::app::{App, Modal, SortKey, View};
use crate::clean::{STALE_SECS, Target};

// Catppuccin Mocha
mod c {
    use ratatui::style::Color;
    pub const TEXT: Color = Color::Rgb(205, 214, 244);
    pub const SUBTEXT: Color = Color::Rgb(166, 173, 200);
    pub const OVERLAY: Color = Color::Rgb(108, 112, 134);
    pub const SURFACE2: Color = Color::Rgb(88, 91, 112);
    pub const SURFACE1: Color = Color::Rgb(69, 71, 90);
    pub const SURFACE0: Color = Color::Rgb(49, 50, 68);
    pub const BASE: Color = Color::Rgb(30, 30, 46);
    pub const MAUVE: Color = Color::Rgb(203, 166, 247);
    pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
    pub const BLUE: Color = Color::Rgb(137, 180, 250);
    pub const SAPPHIRE: Color = Color::Rgb(116, 199, 236);
    pub const TEAL: Color = Color::Rgb(148, 226, 213);
    pub const GREEN: Color = Color::Rgb(166, 227, 161);
    pub const YELLOW: Color = Color::Rgb(249, 226, 175);
    pub const PEACH: Color = Color::Rgb(250, 179, 135);
    pub const RED: Color = Color::Rgb(243, 139, 168);
    pub const PINK: Color = Color::Rgb(245, 194, 231);
}

fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

pub fn human(kib: u64) -> String {
    let k = kib as f64;
    if kib < 1024 {
        format!("{kib}K")
    } else if k < 1024.0 * 1024.0 {
        let m = k / 1024.0;
        if m >= 100.0 {
            format!("{m:.0}M")
        } else {
            format!("{m:.1}M")
        }
    } else {
        let g = k / 1024.0 / 1024.0;
        if g >= 100.0 {
            format!("{g:.0}G")
        } else if g >= 10.0 {
            format!("{g:.1}G")
        } else {
            format!("{g:.2}G")
        }
    }
}

fn mem_color(kib: u64) -> Color {
    match kib {
        0 => c::SURFACE2,
        k if k < 100 * 1024 => c::TEXT,
        k if k < 500 * 1024 => c::YELLOW,
        k if k < 1024 * 1024 => c::PEACH,
        _ => c::RED,
    }
}

fn pct_color(p: f32) -> Color {
    match p {
        p if p < 30.0 => c::GREEN,
        p if p < 60.0 => c::YELLOW,
        p if p < 85.0 => c::PEACH,
        _ => c::RED,
    }
}

/// Thin stacked bar: `━━━━━━━━━━` with colored segments over a dim track.
fn bar(width: usize, segs: &[(f64, Color)]) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for &(frac, color) in segs {
        let cells = ((frac.clamp(0.0, 1.0) * width as f64).round() as usize).min(width - used);
        if cells > 0 {
            out.push(Span::styled("━".repeat(cells), fg(color)));
            used += cells;
        }
    }
    if used < width {
        out.push(Span::styled("━".repeat(width - used), fg(c::SURFACE0)));
    }
    out
}

// Colors used for the RAM breakdown everywhere.
const C_APPS: Color = c::TEAL;
const C_ZRAM: Color = c::MAUVE;
const C_KERNEL: Color = c::PEACH;
const C_TMPFS: Color = c::YELLOW;
const C_OTHER: Color = c::BLUE;

fn ram_segments(b: &crate::proc::Breakdown, total: f64) -> Vec<(f64, Color)> {
    vec![
        (b.apps as f64 / total, C_APPS),
        (b.zram as f64 / total, C_ZRAM),
        (b.kernel as f64 / total, C_KERNEL),
        (b.tmpfs as f64 / total, C_TMPFS),
        (b.other as f64 / total, C_OTHER),
    ]
}

fn panel(title: Line<'static>) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(c::SURFACE1))
        .title(title)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let ncores = app.snap.cores.len().max(1);
    let core_rows = ncores.min(4) as u16;
    let header_h = core_rows.max(4) + 2;
    let detail_h = if app.show_detail && app.view.is_table() {
        9
    } else {
        0
    };
    let [header, tabs, body, detail, footer] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(detail_h),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);
    draw_tabs(f, app, tabs);
    match app.view {
        View::Ram => draw_ram(f, app, body),
        View::Clean => draw_clean(f, app, body),
        _ => {
            draw_table(f, app, body);
            if app.show_detail {
                draw_detail(f, app, detail);
            }
        }
    }
    draw_footer(f, app, footer);

    match &app.modal {
        Some(Modal::Kill(_)) => draw_kill(f, app, area),
        Some(Modal::Help) => draw_help(f, area),
        Some(Modal::Clean(paths, procs, size)) => {
            draw_clean_confirm(f, area, *paths, *procs, *size)
        }
        None => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.snap;
    let title = Line::from(vec![
        Span::styled(" ◉ ", fg(c::MAUVE)),
        Span::styled("ramwho ", fg(c::MAUVE).add_modifier(Modifier::BOLD)),
    ]);
    let up = s.uptime as u64;
    let uptime = format!(
        " up {}d {}h {}m ",
        up / 86400,
        up % 86400 / 3600,
        up % 3600 / 60
    );
    let block = panel(title)
        .title_top(Line::from(Span::styled(uptime, fg(c::OVERLAY))).right_aligned())
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [cores_area, stats_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(52)])
            .spacing(3)
            .areas(inner);

    // Per-core CPU bars in a grid.
    let ncores = s.cores.len().max(1);
    let rows = ncores.min(4);
    let cols = ncores.div_ceil(rows);
    let col_w = (cores_area.width as usize / cols.max(1)).max(12);
    let bar_w = col_w.saturating_sub(3 + 5 + 2).max(3);
    let mut lines = Vec::new();
    for r in 0..rows {
        let mut spans = Vec::new();
        for col in 0..cols {
            let i = col * rows + r;
            let Some(&p) = s.cores.get(i) else { continue };
            spans.push(Span::styled(format!("{i:>2} "), fg(c::OVERLAY)));
            spans.extend(bar(bar_w, &[(p as f64 / 100.0, pct_color(p))]));
            spans.push(Span::styled(format!("{p:>4.0}%"), fg(c::SUBTEXT)));
            spans.push(Span::raw("  "));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), cores_area);

    // Memory / swap / load.
    let m = &s.mem;
    let total = m.total.max(1) as f64;
    let b = s.breakdown();
    let swap_f = m.swap_used() as f64 / m.swap_total.max(1) as f64;
    let bw = 22;
    let label = |t: &'static str, col: Color| Span::styled(t, fg(col).add_modifier(Modifier::BOLD));
    let mut mem = vec![label("RAM  ", c::GREEN)];
    mem.extend(bar(bw, &ram_segments(&b, total)));
    mem.push(Span::styled(
        format!(" {:>6}", human(m.used())),
        fg(c::TEXT),
    ));
    mem.push(Span::styled(
        format!(" / {}", human(m.total)),
        fg(c::OVERLAY),
    ));
    let mut swap = vec![label("SWAP ", c::MAUVE)];
    swap.extend(bar(
        bw,
        &[(swap_f, if swap_f > 0.8 { c::RED } else { c::MAUVE })],
    ));
    swap.push(Span::styled(
        format!(" {:>6}", human(m.swap_used())),
        fg(c::TEXT),
    ));
    swap.push(Span::styled(
        format!(" / {}", human(m.swap_total)),
        fg(c::OVERLAY),
    ));
    let mut cpu = vec![label("CPU  ", c::BLUE)];
    cpu.extend(bar(
        bw,
        &[(s.cpu_total as f64 / 100.0, pct_color(s.cpu_total))],
    ));
    cpu.push(Span::styled(format!(" {:>5.1}%", s.cpu_total), fg(c::TEXT)));
    cpu.push(Span::styled(
        format!("  {} cores", s.cores.len()),
        fg(c::OVERLAY),
    ));
    let nproc = s.procs.iter().filter(|p| !p.kernel).count();
    let info = Line::from(vec![
        label("LOAD ", c::PEACH),
        Span::styled(
            format!("{:.2} {:.2} {:.2}", s.load[0], s.load[1], s.load[2]),
            fg(c::TEXT),
        ),
        Span::styled("  ·  ", fg(c::SURFACE2)),
        Span::styled(format!("{nproc}"), fg(c::TEXT)),
        Span::styled(" procs ", fg(c::OVERLAY)),
        Span::styled(format!("{}", s.threads), fg(c::TEXT)),
        Span::styled(" thr ", fg(c::OVERLAY)),
        Span::styled(format!("{}", s.running), fg(c::GREEN)),
        Span::styled(" run", fg(c::OVERLAY)),
    ]);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(mem),
            Line::from(swap),
            Line::from(cpu),
            info,
        ]),
        stats_area,
    );
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for (i, v) in View::ALL.iter().enumerate() {
        let label = format!(" {} {} ", i + 1, v.title());
        if *v == app.view {
            spans.push(Span::styled(
                label,
                Style::new()
                    .bg(c::MAUVE)
                    .fg(c::BASE)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, fg(c::OVERLAY)));
        }
        spans.push(Span::raw(" "));
    }
    let hint = match app.view {
        View::Apps => "grouped by app · →/enter to look inside",
        View::Tree => "full process tree",
        View::List => "every process, flat",
        View::Ram => "where every GB of RAM goes",
        View::Clean => "leftovers nobody is using",
    };
    spans.push(Span::styled(
        format!(" {hint}"),
        fg(c::SURFACE2).add_modifier(Modifier::ITALIC),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    let mut right = Vec::new();
    if app.filter_mode || !app.filter.is_empty() {
        right.push(Span::styled(" / ", Style::new().bg(c::YELLOW).fg(c::BASE)));
        right.push(Span::styled(
            format!(" {}{} ", app.filter, if app.filter_mode { "▏" } else { "" }),
            fg(c::YELLOW),
        ));
        right.push(Span::raw("  "));
    }
    if app.view.is_table() {
        right.push(Span::styled("sort ", fg(c::OVERLAY)));
        right.push(Span::styled(
            format!("{} ▼", app.sort.title()),
            fg(c::LAVENDER),
        ));
    }
    right.push(Span::styled(
        format!("  ⟳ {:.1}s ", app.interval_ms as f64 / 1000.0),
        fg(c::OVERLAY),
    ));
    f.render_widget(Paragraph::new(Line::from(right)).right_aligned(), area);
}

fn draw_table(f: &mut Frame, app: &mut App, area: Rect) {
    let tree = app.view != View::List;
    let sorted = |label: &str, key: SortKey| -> Cell<'static> {
        let style = if app.sort == key {
            fg(c::LAVENDER).add_modifier(Modifier::BOLD)
        } else {
            fg(c::SUBTEXT)
        };
        let text = if app.sort == key {
            format!("{label}▼")
        } else {
            label.to_string()
        };
        Cell::from(Line::from(Span::styled(text, style)).right_aligned())
    };
    let plain = |label: &str| Cell::from(Span::styled(label.to_string(), fg(c::SUBTEXT)));

    let max_fp = app
        .rows
        .iter()
        .map(|r| app.agg[r.idx].pss + app.agg[r.idx].swap)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let bar_w = 14usize;

    let mut header = vec![sorted("PID", SortKey::Pid)];
    if !tree {
        header.push(plain("USER"));
    }
    header.push(Cell::from(Span::styled(
        if app.sort == SortKey::Name {
            "NAME▼"
        } else {
            "NAME"
        },
        if app.sort == SortKey::Name {
            fg(c::LAVENDER).add_modifier(Modifier::BOLD)
        } else {
            fg(c::SUBTEXT)
        },
    )));
    header.push(sorted("CPU%", SortKey::Cpu));
    header.push(sorted(if tree { "Σ MEM" } else { "MEM" }, SortKey::Mem));
    header.push(sorted(if tree { "Σ SWAP" } else { "SWAP" }, SortKey::Swap));
    header.push(Cell::from(
        Line::from(Span::styled(
            if tree { "SELF" } else { "RSS" },
            fg(c::SUBTEXT),
        ))
        .right_aligned(),
    ));
    if !tree {
        header.push(Cell::from(
            Line::from(Span::styled("THR", fg(c::SUBTEXT))).right_aligned(),
        ));
    }
    header.push(Cell::from(Line::from(vec![
        Span::styled("ram", fg(c::TEAL)),
        Span::styled("+", fg(c::OVERLAY)),
        Span::styled("swap", fg(c::MAUVE)),
    ])));

    let rows: Vec<TRow> = app
        .rows
        .iter()
        .map(|r| {
            let p = &app.snap.procs[r.idx];
            let a = &app.agg[r.idx];
            let mut cells = vec![Cell::from(
                Line::from(Span::styled(p.pid.to_string(), fg(c::OVERLAY))).right_aligned(),
            )];
            if !tree {
                let user_col = if p.user == "root" { c::RED } else { c::SUBTEXT };
                cells.push(Cell::from(Span::styled(p.user.clone(), fg(user_col))));
            }

            let mut name = Vec::new();
            if tree {
                name.push(Span::styled(r.prefix.clone(), fg(c::SURFACE2)));
                name.push(match (r.has_children, r.expanded) {
                    (true, true) => Span::styled("▾ ", fg(c::MAUVE)),
                    (true, false) => Span::styled("▸ ", fg(c::MAUVE)),
                    _ => Span::raw("  "),
                });
            }
            let root = tree && r.prefix.is_empty();
            let name_style = if p.kernel {
                fg(c::OVERLAY)
            } else if root {
                fg(c::TEXT).add_modifier(Modifier::BOLD)
            } else {
                fg(c::TEXT)
            };
            name.push(Span::styled(p.name.clone(), name_style));
            if r.has_children && !r.expanded {
                name.push(Span::styled(format!(" ×{}", a.count), fg(c::TEAL)));
            }
            if !p.role.is_empty() {
                name.push(Span::styled(
                    format!("  {}", p.role),
                    fg(c::OVERLAY).add_modifier(Modifier::ITALIC),
                ));
            }
            cells.push(Cell::from(Line::from(name)));

            let num = |s: String, col: Color| {
                Cell::from(Line::from(Span::styled(s, fg(col))).right_aligned())
            };
            let cpu = a.cpu;
            cells.push(num(
                format!("{cpu:.1}"),
                if cpu < 0.05 {
                    c::SURFACE2
                } else {
                    pct_color(cpu.min(100.0))
                },
            ));
            let approx = if p.mem_exact || !app.snap.detailed {
                ""
            } else {
                "~"
            };
            cells.push(num(format!("{approx}{}", human(a.pss)), mem_color(a.pss)));
            cells.push(num(
                if a.swap == 0 {
                    "·".into()
                } else {
                    human(a.swap)
                },
                if a.swap == 0 { c::SURFACE2 } else { c::MAUVE },
            ));
            if tree {
                cells.push(num(human(p.pss), c::OVERLAY));
            } else {
                cells.push(num(human(p.rss), c::OVERLAY));
                cells.push(num(
                    if app.snap.detailed {
                        p.threads.to_string()
                    } else {
                        "–".into()
                    },
                    c::OVERLAY,
                ));
            }
            cells.push(Cell::from(Line::from(bar(
                bar_w,
                &[
                    (a.pss as f64 / max_fp, c::TEAL),
                    (a.swap as f64 / max_fp, c::MAUVE),
                ],
            ))));
            TRow::new(cells)
        })
        .collect();

    let mut widths = vec![Constraint::Length(8)];
    if !tree {
        widths.push(Constraint::Length(10));
    }
    widths.push(Constraint::Fill(1));
    widths.extend([
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(7),
    ]);
    if !tree {
        widths.push(Constraint::Length(4));
    }
    widths.push(Constraint::Length(bar_w as u16));

    let groups = app.rows.iter().filter(|r| r.prefix.is_empty()).count();
    let title = match app.view {
        View::List => format!(" {} processes ", app.rows.len()),
        _ => format!(" {} groups · {} rows ", groups, app.rows.len()),
    };
    let pos = format!(
        " {}/{} ",
        app.table.selected().map(|i| i + 1).unwrap_or(0),
        app.rows.len()
    );
    let block = panel(Line::from(Span::styled(title, fg(c::SUBTEXT))))
        .title_bottom(Line::from(Span::styled(pos, fg(c::OVERLAY))).right_aligned());

    let table = Table::new(rows, widths)
        .header(TRow::new(header).height(1).bottom_margin(0))
        .block(block)
        .column_spacing(1)
        .row_highlight_style(Style::new().bg(c::SURFACE0).add_modifier(Modifier::BOLD))
        .highlight_symbol(Span::styled("▌", fg(c::MAUVE)))
        .highlight_spacing(HighlightSpacing::Always);

    app.page = area.height.saturating_sub(4).max(1) as usize;
    f.render_stateful_widget(table, area, &mut app.table);

    let mut sb = ScrollbarState::new(app.rows.len()).position(app.table.selected().unwrap_or(0));
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(fg(c::SURFACE0))
            .thumb_style(fg(c::SURFACE2)),
        area.inner(ratatui::layout::Margin {
            vertical: 2,
            horizontal: 0,
        }),
        &mut sb,
    );
}

fn state_name(s: char) -> &'static str {
    match s {
        'R' => "running",
        'S' => "sleeping",
        'D' => "disk wait",
        'Z' => "zombie",
        'T' | 't' => "stopped",
        'I' => "idle",
        _ => "?",
    }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(row) = app.selected_row() else {
        f.render_widget(panel(Line::from(" details ")), area);
        return;
    };
    let p = &app.snap.procs[row.idx];
    let a = &app.agg[row.idx];
    let title = Line::from(vec![
        Span::styled(" ", fg(c::TEXT)),
        Span::styled(p.name.clone(), fg(c::MAUVE).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" · pid {} ", p.pid), fg(c::OVERLAY)),
    ]);
    let block = panel(title).padding(Padding::horizontal(1));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(46)])
        .spacing(3)
        .areas(inner);

    let k = |s: &'static str| Span::styled(format!("{s:<8}"), fg(c::OVERLAY));
    let v = |s: String| Span::styled(s, fg(c::TEXT));
    let sep = || Span::styled("  ·  ", fg(c::SURFACE2));
    let parent = app
        .snap
        .procs
        .iter()
        .find(|q| q.pid == p.ppid)
        .map(|q| format!("{} ({})", q.name, p.ppid))
        .unwrap_or_else(|| p.ppid.to_string());
    let mut lines = vec![
        Line::from(vec![
            k("user"),
            v(p.user.clone()),
            sep(),
            Span::styled("state ", fg(c::OVERLAY)),
            v(state_name(p.state).into()),
            sep(),
            Span::styled("threads ", fg(c::OVERLAY)),
            v(p.threads.to_string()),
            sep(),
            Span::styled("parent ", fg(c::OVERLAY)),
            v(parent),
        ]),
        Line::from(vec![
            k("role"),
            Span::styled(
                if p.role.is_empty() {
                    "—".into()
                } else {
                    p.role.clone()
                },
                fg(c::SAPPHIRE),
            ),
        ]),
    ];
    if row.has_children {
        lines.push(Line::from(vec![
            k("group"),
            Span::styled(format!("{} processes", a.count), fg(c::TEAL)),
            sep(),
            Span::styled("ram ", fg(c::OVERLAY)),
            Span::styled(
                human(a.pss),
                fg(mem_color(a.pss)).add_modifier(Modifier::BOLD),
            ),
            sep(),
            Span::styled("swap ", fg(c::OVERLAY)),
            Span::styled(human(a.swap), fg(c::MAUVE)),
            sep(),
            Span::styled("cpu ", fg(c::OVERLAY)),
            v(format!("{:.1}%", a.cpu)),
        ]));
    }
    lines.push(Line::from(vec![
        k("cmd"),
        Span::styled(p.cmdline.clone(), fg(c::SUBTEXT)),
    ]));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), left);

    if !app.snap.detailed {
        let lines = vec![
            Line::from(vec![
                Span::styled("resident ", fg(c::OVERLAY)),
                Span::styled(
                    human(p.rss),
                    fg(mem_color(p.rss)).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::raw(""),
            Line::from(Span::styled(
                "Shared memory is counted in every process",
                fg(c::SURFACE2).add_modifier(Modifier::ITALIC),
            )),
            Line::from(Span::styled(
                "that maps it. Exact PSS/swap needs Linux.",
                fg(c::SURFACE2).add_modifier(Modifier::ITALIC),
            )),
        ];
        f.render_widget(Paragraph::new(lines), right);
        return;
    }

    // Memory breakdown for this process itself.
    let pss = p.pss.max(1) as f64;
    let mut mbar = vec![Span::styled("self ", fg(c::OVERLAY))];
    mbar.extend(bar(
        28,
        &[
            (p.pss_anon as f64 / pss, c::TEAL),
            (p.pss_file as f64 / pss, c::BLUE),
            (p.pss_shmem as f64 / pss, c::PINK),
        ],
    ));
    mbar.push(Span::styled(
        format!(" {:>6}", human(p.pss)),
        fg(mem_color(p.pss)).add_modifier(Modifier::BOLD),
    ));
    let legend = |col: Color, name: &'static str, val: u64| {
        vec![
            Span::styled("● ", fg(col)),
            Span::styled(format!("{name} "), fg(c::OVERLAY)),
            Span::styled(format!("{:<7}", human(val)), fg(c::TEXT)),
        ]
    };
    let mut l2 = vec![Span::raw("     ")];
    l2.extend(legend(c::TEAL, "heap", p.pss_anon));
    l2.extend(legend(c::BLUE, "file", p.pss_file));
    l2.extend(legend(c::PINK, "shm", p.pss_shmem));
    let kv = |name: &'static str, val: String, col: Color| {
        vec![
            Span::styled(format!("{name} "), fg(c::OVERLAY)),
            Span::styled(format!("{val:<8}"), fg(col)),
        ]
    };
    let mut l3 = vec![Span::raw("     ")];
    l3.extend(kv("unique", human(p.uss), c::TEXT));
    l3.extend(kv("rss", human(p.rss), c::TEXT));
    l3.extend(kv(
        "swap",
        human(p.swap),
        if p.swap > 0 { c::MAUVE } else { c::SURFACE2 },
    ));
    let note = if p.mem_exact {
        Line::from(Span::styled(
            "     PSS: shared pages split between users",
            fg(c::SURFACE2).add_modifier(Modifier::ITALIC),
        ))
    } else {
        Line::from(Span::styled(
            "     ≈ RSS only (other user's process)",
            fg(c::YELLOW).add_modifier(Modifier::ITALIC),
        ))
    };
    f.render_widget(
        Paragraph::new(vec![Line::from(mbar), Line::from(l2), Line::from(l3), note]),
        right,
    );
}

fn key_hint(k: &str, d: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {k} "),
            Style::new().fg(c::MAUVE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{d} "), fg(c::OVERLAY)),
    ]
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    let hints: &[(&str, &str)] = match app.view {
        View::Ram => &[
            ("tab", "view"),
            ("5", "clean up"),
            ("?", "help"),
            ("q", "quit"),
        ],
        View::Clean => &[
            ("space", "select"),
            ("/", "filter"),
            ("a", "all"),
            ("o", "old only"),
            ("n", "none"),
            ("enter", "clear selected"),
            ("r", "rescan"),
            ("tab", "view"),
            ("q", "quit"),
        ],
        _ => &[
            ("↑↓", "move"),
            ("←→", "fold"),
            ("tab", "view"),
            ("/", "filter"),
            ("k", "kill"),
            ("m c s", "sort"),
            ("d", "details"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    for &(k, d) in hints {
        spans.extend(key_hint(k, d));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    if let Some((msg, at, err)) = &app.status
        && at.elapsed().as_secs() < 5
    {
        let style = if *err {
            Style::new().fg(c::BASE).bg(c::RED)
        } else {
            Style::new().fg(c::BASE).bg(c::GREEN)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {msg} "), style))).right_aligned(),
            area,
        );
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn draw_kill(f: &mut Frame, app: &App, area: Rect) {
    let Some(Modal::Kill(d)) = &app.modal else {
        return;
    };
    let opts = d.options();
    let rect = centered(area, 62, opts.len() as u16 + 8);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(c::RED))
        .title(Line::from(Span::styled(
            " ⚠ kill process ",
            fg(c::RED).add_modifier(Modifier::BOLD),
        )))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::new().bg(c::BASE));
    let mut lines = vec![
        Line::from(vec![
            Span::styled(d.name.clone(), fg(c::TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  {}", d.role),
                fg(c::OVERLAY).add_modifier(Modifier::ITALIC),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("pid {}", d.pid), fg(c::OVERLAY)),
            Span::styled("  ·  ", fg(c::SURFACE2)),
            Span::styled(format!("{} in use", human(d.footprint)), fg(c::PEACH)),
        ]),
        Line::raw(""),
    ];
    for (i, (sig, desc)) in opts.iter().enumerate() {
        let sel = i == d.choice;
        let sig_col = if *sig == "SIGKILL" { c::RED } else { c::YELLOW };
        let bg = if sel {
            Style::new().bg(c::SURFACE0)
        } else {
            Style::new()
        };
        lines.push(
            Line::from(vec![
                Span::styled(if sel { " ❯ " } else { "   " }, fg(c::MAUVE)),
                Span::styled(
                    format!("{sig:<8}"),
                    fg(sig_col).add_modifier(Modifier::BOLD),
                ),
                Span::styled(desc.clone(), fg(if sel { c::TEXT } else { c::SUBTEXT })),
            ])
            .style(bg),
        );
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "↑↓ choose · enter send · 9 force kill · esc cancel",
        fg(c::OVERLAY),
    )));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rect = centered(area, 60, 26);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(c::MAUVE))
        .title(Line::from(Span::styled(
            " keys ",
            fg(c::MAUVE).add_modifier(Modifier::BOLD),
        )))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::new().bg(c::BASE));
    let row = |k: &'static str, d: &'static str| {
        Line::from(vec![
            Span::styled(format!("{k:<14}"), fg(c::LAVENDER)),
            Span::styled(d, fg(c::TEXT)),
        ])
    };
    let lines = vec![
        row("↑ ↓ PgUp PgDn", "move   (g / G: top / bottom)"),
        row("→  l", "expand group"),
        row("←  h", "collapse group / jump to parent"),
        row("enter space", "toggle group"),
        row("e  E", "expand all / collapse all"),
        row("tab  1-5", "Apps · Tree · List · RAM · Clean up"),
        row("/", "filter by name, cmdline, user, pid"),
        row("m c s p n", "sort by mem · cpu · swap · pid · name"),
        row("k  F9  del", "kill selected (or its whole tree)"),
        row("space a o n", "clean up: toggle · all · old · none"),
        row("t", "show kernel threads"),
        row("d", "toggle details pane"),
        row("+  -", "slower / faster refresh"),
        row("r", "refresh now"),
        row("q  ctrl-c", "quit"),
        Line::raw(""),
        Line::from(Span::styled(
            "MEM = PSS: shared memory split fairly, so a group's",
            fg(c::OVERLAY),
        )),
        Line::from(Span::styled(
            "Σ total is real RAM, not RSS double counting.",
            fg(c::OVERLAY),
        )),
        Line::from(Span::styled(
            "Threads are not listed as separate rows.",
            fg(c::OVERLAY),
        )),
        Line::from(Span::styled(
            "~ = approximate (process owned by another user).",
            fg(c::OVERLAY),
        )),
    ];
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

fn pct_of(part: u64, whole: u64) -> String {
    format!("{:>3.0}%", part as f64 * 100.0 / whole.max(1) as f64)
}

fn draw_ram(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.snap;
    let m = &s.mem;
    let b = s.breakdown();
    let total = m.total.max(1);
    let [top, bottom] = Layout::vertical([Constraint::Length(15), Constraint::Min(6)]).areas(area);
    let [left, right] = Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(top);

    // ── Where's my RAM ──────────────────────────────────────────────
    let block = panel(Line::from(vec![
        Span::styled(" where's my RAM ", fg(c::TEXT).add_modifier(Modifier::BOLD)),
        Span::styled(format!("· {} total ", human(m.total)), fg(c::OVERLAY)),
    ]))
    .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(left);
    f.render_widget(block, left);
    let w = inner.width as usize;
    let mut segs = ram_segments(&b, total as f64);
    segs.push((b.reclaimable as f64 / total as f64, c::SURFACE1));
    let mut lines = vec![Line::from(bar(w, &segs)), Line::raw("")];
    let z = s.zram.clone().unwrap_or_default();
    let ratio = if z.compressed > 0 {
        z.stored as f64 / z.compressed as f64
    } else {
        0.0
    };
    let row = |col: Color, dot: &'static str, label: &'static str, kib: u64, note: String| {
        Line::from(vec![
            Span::styled(format!("{dot} "), fg(col)),
            Span::styled(format!("{label:<22}"), fg(c::TEXT)),
            Span::styled(
                format!("{:>7}", human(kib)),
                fg(col).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}  ", pct_of(kib, total)), fg(c::OVERLAY)),
            Span::styled(note, fg(c::OVERLAY).add_modifier(Modifier::ITALIC)),
        ])
    };
    lines.push(row(
        C_APPS,
        "●",
        "Apps",
        b.apps,
        "process heap & stacks → tabs 1-3".into(),
    ));
    if !s.detailed {
        lines.pop();
        lines.push(row(
            C_APPS,
            "●",
            "Apps",
            b.apps,
            "sum of resident memory (shared pages counted per process)".into(),
        ));
        lines.push(row(
            C_OTHER,
            "●",
            "System & other",
            b.other,
            "OS, drivers, caches, compressed memory".into(),
        ));
    }
    if s.detailed {
        lines.push(row(
            C_ZRAM,
            "●",
            "zram (swap in RAM)",
            b.zram,
            if s.zram.is_some() {
                format!(
                    "{} of swapped data, compressed {ratio:.1}×",
                    human(z.stored)
                )
            } else {
                "no zram".into()
            },
        ));
        lines.push(row(
            C_KERNEL,
            "●",
            "Kernel",
            b.kernel,
            format!(
                "slab {} · vmalloc {} · page tables {}",
                human(m.slab_unreclaimable),
                human(m.vmalloc),
                human(m.page_tables)
            ),
        ));
        lines.push(row(
            C_TMPFS,
            "●",
            "/tmp & shared mem",
            b.tmpfs,
            "RAM-backed files (tmpfs) resident now".into(),
        ));
        lines.push(row(
            C_OTHER,
            "●",
            "Cache in use / other",
            b.other,
            "file cache the kernel won't drop".into(),
        ));
    }
    lines.push(Line::from(Span::styled(
        "─".repeat(w.min(70)),
        fg(c::SURFACE1),
    )));
    lines.push(Line::from(vec![
        Span::styled("  ", fg(c::TEXT)),
        Span::styled(
            format!("{:<22}", "Used"),
            fg(c::TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>7}", human(b.used)),
            fg(c::TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}  ", pct_of(b.used, total)), fg(c::OVERLAY)),
        Span::styled(
            "= sum of the rows above",
            fg(c::SURFACE2).add_modifier(Modifier::ITALIC),
        ),
    ]));
    lines.push(row(
        c::SURFACE2,
        "○",
        "Reclaimable cache",
        b.reclaimable,
        "freed automatically when apps need it".into(),
    ));
    lines.push(row(c::SURFACE2, "○", "Free", b.free, String::new()));
    f.render_widget(Paragraph::new(lines), inner);

    // ── What's in swap ──────────────────────────────────────────────
    let swap_used = m.swap_used();
    let block = panel(Line::from(vec![
        Span::styled(" what's in swap ", fg(c::TEXT).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("· {} / {} ", human(swap_used), human(m.swap_total)),
            fg(c::OVERLAY),
        ),
    ]))
    .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(right);
    f.render_widget(block, right);
    let w = inner.width as usize;
    let st = m.swap_total.max(1) as f64;
    let srow = |col: Color, label: &'static str, kib: u64| {
        Line::from(vec![
            Span::styled("● ", fg(col)),
            Span::styled(format!("{label:<18}"), fg(c::TEXT)),
            Span::styled(
                format!("{:>7}", human(kib)),
                fg(col).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}", pct_of(kib, swap_used)), fg(c::OVERLAY)),
        ])
    };
    if !s.detailed {
        let lines = vec![
            Line::from(bar(w, &[(swap_used as f64 / st, C_ZRAM)])),
            Line::raw(""),
            Line::from(vec![
                Span::styled("● ", fg(C_ZRAM)),
                Span::styled(format!("{:<18}", "Swap used"), fg(c::TEXT)),
                Span::styled(
                    format!("{:>7}", human(swap_used)),
                    fg(C_ZRAM).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::raw(""),
            Line::from(Span::styled(
                format!(
                    "Per-process swap isn't reported on {}.",
                    crate::sys::PLATFORM
                ),
                fg(c::OVERLAY),
            )),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    } else {
        let mut lines = vec![
            Line::from(bar(
                w,
                &[
                    (b.swap_procs as f64 / st, C_APPS),
                    (b.swap_tmpfs as f64 / st, C_TMPFS),
                    (b.swap_other as f64 / st, c::SURFACE2),
                ],
            )),
            Line::raw(""),
            srow(C_APPS, "Processes", b.swap_procs),
            srow(C_TMPFS, "/tmp files (≈)", b.swap_tmpfs),
            srow(c::SURFACE2, "Other", b.swap_other),
            Line::raw(""),
        ];
        if s.zram.is_some() {
            lines.push(Line::from(Span::styled(
                "This swap is zram: it lives in RAM,",
                fg(c::OVERLAY),
            )));
            lines.push(Line::from(vec![
                Span::styled("compressed to ", fg(c::OVERLAY)),
                Span::styled(human(z.ram), fg(C_ZRAM).add_modifier(Modifier::BOLD)),
                Span::styled(" of real memory.", fg(c::OVERLAY)),
            ]));
        }
        if b.swap_tmpfs > swap_used / 4 {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("➜ ", fg(c::YELLOW)),
                Span::styled("old /tmp files fill your swap. ", fg(c::YELLOW)),
                Span::styled("Press 5", fg(c::YELLOW).add_modifier(Modifier::BOLD)),
            ]));
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    // ── Top apps & tmpfs ────────────────────────────────────────────
    let [apps_area, tmp_area] =
        Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(bottom);
    let block = panel(Line::from(Span::styled(
        " biggest apps (RAM + swap) ",
        fg(c::TEXT).add_modifier(Modifier::BOLD),
    )))
    .padding(Padding::horizontal(2));
    let inner = block.inner(apps_area);
    f.render_widget(block, apps_area);
    let mut roots: Vec<_> = app
        .rows
        .iter()
        .filter(|r| r.prefix.is_empty())
        .map(|r| r.idx)
        .collect();
    roots.sort_by_key(|&i| std::cmp::Reverse(app.agg[i].pss + app.agg[i].swap));
    let max = roots
        .first()
        .map(|&i| app.agg[i].pss + app.agg[i].swap)
        .unwrap_or(1)
        .max(1) as f64;
    let bw = (inner.width as usize).saturating_sub(48).clamp(6, 40);
    let lines: Vec<Line> = roots
        .iter()
        .take(inner.height as usize)
        .map(|&i| {
            let p = &s.procs[i];
            let a = &app.agg[i];
            let mut v = vec![Span::styled(format!("{:<22.22} ", p.name), fg(c::TEXT))];
            v.push(Span::styled(
                if a.count > 1 {
                    format!("×{:<4}", a.count)
                } else {
                    "     ".into()
                },
                fg(c::TEAL),
            ));
            v.extend(bar(
                bw,
                &[(a.pss as f64 / max, C_APPS), (a.swap as f64 / max, C_ZRAM)],
            ));
            v.push(Span::styled(
                format!(" {:>7}", human(a.pss)),
                fg(mem_color(a.pss)),
            ));
            v.push(Span::styled(
                if a.swap > 0 {
                    format!(" +{} swap", human(a.swap))
                } else {
                    String::new()
                },
                fg(C_ZRAM),
            ));
            Line::from(v)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);

    let block = panel(Line::from(Span::styled(
        " RAM-backed folders (tmpfs) ",
        fg(c::TEXT).add_modifier(Modifier::BOLD),
    )))
    .padding(Padding::horizontal(2));
    let inner = block.inner(tmp_area);
    f.render_widget(block, tmp_area);
    if s.tmpfs.is_empty() {
        let msg = if s.detailed {
            "none"
        } else {
            "Linux only (tmpfs)"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, fg(c::OVERLAY)))),
            inner,
        );
        return;
    }
    let tmax = s.tmpfs.first().map(|t| t.1).unwrap_or(1).max(1) as f64;
    let bw = (inner.width as usize).saturating_sub(30).clamp(4, 30);
    let lines: Vec<Line> = s
        .tmpfs
        .iter()
        .filter(|t| t.1 >= 1024)
        .take(inner.height as usize)
        .map(|(mnt, used, _)| {
            let mut v = vec![Span::styled(format!("{mnt:<20.20}"), fg(c::TEXT))];
            v.extend(bar(bw, &[(*used as f64 / tmax, C_TMPFS)]));
            v.push(Span::styled(
                format!(" {:>7}", human(*used)),
                fg(mem_color(*used)),
            ));
            Line::from(v)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn draw_clean(f: &mut Frame, app: &mut App, area: Rect) {
    if !crate::clean::SUPPORTED {
        let block = panel(Line::from(Span::styled(
            " clean up ",
            fg(c::TEXT).add_modifier(Modifier::BOLD),
        )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let lines = vec![
            Line::from(Span::styled(
                format!("Clean up isn't available on {} yet.", crate::sys::PLATFORM),
                fg(c::TEXT),
            )),
            Line::from(Span::styled(
                "It needs /proc to prove no running process uses a file before deleting it.",
                fg(c::OVERLAY),
            )),
        ];
        f.render_widget(
            Paragraph::new(lines).centered(),
            centered(inner, inner.width, 2),
        );
        return;
    }
    let (paths, procs, size) = app.clean_selected();
    let in_use = app.clean.iter().filter(|c| c.in_use_by.is_some()).count();
    let mut title = vec![Span::styled(
        " clean up ",
        fg(c::TEXT).add_modifier(Modifier::BOLD),
    )];
    if let Some(busy) = app.clean_busy {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        title.push(Span::styled(
            format!("{} {busy}… ", SPINNER[(t / 80) as usize % SPINNER.len()]),
            fg(c::YELLOW),
        ));
    } else {
        title.push(Span::styled(
            format!("· {} items · {} in use (locked) ", app.clean.len(), in_use),
            fg(c::OVERLAY),
        ));
    }
    let summary = Line::from(vec![
        Span::styled(" selected ", fg(c::OVERLAY)),
        Span::styled(
            format!("{}", paths + procs),
            fg(c::TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", fg(c::SURFACE2)),
        Span::styled(human(size), fg(c::GREEN).add_modifier(Modifier::BOLD)),
        Span::styled(" to free ", fg(c::OVERLAY)),
    ])
    .right_aligned();
    let block = panel(Line::from(title)).title_top(summary).title_bottom(
        Line::from(Span::styled(
            " pre-selected: unused, 1 MB+, untouched for 1 day+ · 🔒 = in use by a running process ",
            fg(c::SURFACE2),
        ))
        .centered(),
    );

    let vis = app.clean_visible();
    if vis.is_empty() {
        let msg = if app.clean_busy.is_some() {
            "scanning /tmp, /dev/shm and processes…"
        } else if !app.filter.is_empty() {
            "no matches"
        } else {
            "nothing to clean 🎉"
        };
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, fg(c::OVERLAY)))).centered(),
            centered(inner, inner.width, 1),
        );
        return;
    }

    let max = app.clean.iter().map(|c| c.size).max().unwrap_or(1).max(1) as f64;
    let rows: Vec<TRow> = vis
        .iter()
        .map(|&i| {
            let cnd = &app.clean[i];
            let locked = cnd.in_use_by.is_some();
            let check = match (locked, cnd.selected) {
                (true, _) => Span::styled(" 🔒", fg(c::SURFACE2)),
                (false, true) => Span::styled(" [✓]", fg(c::GREEN).add_modifier(Modifier::BOLD)),
                (false, false) => Span::styled(" [ ]", fg(c::OVERLAY)),
            };
            let is_proc = matches!(cnd.target, Target::Proc { .. });
            let kind_col = if is_proc {
                c::RED
            } else if cnd.kind == "folder" {
                c::BLUE
            } else {
                c::SUBTEXT
            };
            let name_col = if locked { c::OVERLAY } else { c::TEXT };
            let status = match &cnd.in_use_by {
                Some(who) => Span::styled(format!("in use · {who}"), fg(c::OVERLAY)),
                None if is_proc => Span::styled(
                    format!(
                        "detached · parent exited · {}",
                        ago(cnd.idle_secs).replace(" ago", " old")
                    ),
                    fg(c::PEACH),
                ),
                None if cnd.idle_secs >= STALE_SECS => {
                    Span::styled(format!("unused · {}", ago(cnd.idle_secs)), fg(c::GREEN))
                }
                None => Span::styled(format!("unused · {}", ago(cnd.idle_secs)), fg(c::YELLOW)),
            };
            TRow::new(vec![
                Cell::from(check),
                Cell::from(Span::styled(cnd.kind, fg(kind_col))),
                Cell::from(Line::from(vec![
                    Span::styled(
                        if is_proc {
                            format!("{}  ", cnd.root)
                        } else {
                            format!("{}/", cnd.root)
                        },
                        fg(c::SURFACE2),
                    ),
                    Span::styled(cnd.name.clone(), fg(name_col)),
                ])),
                Cell::from(
                    Line::from(Span::styled(
                        human(cnd.size),
                        fg(if locked {
                            c::OVERLAY
                        } else {
                            mem_color(cnd.size)
                        }),
                    ))
                    .right_aligned(),
                ),
                Cell::from(Line::from(bar(
                    10,
                    &[(
                        cnd.size as f64 / max,
                        if locked { c::SURFACE2 } else { C_TMPFS },
                    )],
                ))),
                Cell::from(status),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(7),
            Constraint::Fill(1),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(34),
        ],
    )
    .header(TRow::new(
        ["", "TYPE", "NAME", "SIZE", "", "STATUS"]
            .map(|h| Cell::from(Span::styled(h, fg(c::SUBTEXT)))),
    ))
    .block(block)
    .column_spacing(1)
    .row_highlight_style(Style::new().bg(c::SURFACE0).add_modifier(Modifier::BOLD))
    .highlight_symbol(Span::styled("▌", fg(c::MAUVE)))
    .highlight_spacing(HighlightSpacing::Always);
    app.page = area.height.saturating_sub(4).max(1) as usize;
    f.render_stateful_widget(table, area, &mut app.clean_table);
}

fn draw_clean_confirm(f: &mut Frame, area: Rect, paths: usize, procs: usize, size: u64) {
    let rect = centered(area, 58, 11);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(c::PEACH))
        .title(Line::from(Span::styled(
            " ✦ clear selected ",
            fg(c::PEACH).add_modifier(Modifier::BOLD),
        )))
        .padding(Padding::new(2, 2, 1, 0))
        .style(Style::new().bg(c::BASE));
    let mut lines = vec![Line::from(vec![
        Span::styled("Free about ", fg(c::TEXT)),
        Span::styled(human(size), fg(c::GREEN).add_modifier(Modifier::BOLD)),
    ])];
    lines.push(Line::raw(""));
    if paths > 0 {
        lines.push(Line::from(vec![
            Span::styled("  • delete ", fg(c::SUBTEXT)),
            Span::styled(format!("{paths}"), fg(c::TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" files/folders from /tmp, /dev/shm", fg(c::SUBTEXT)),
        ]));
    }
    if procs > 0 {
        lines.push(Line::from(vec![
            Span::styled("  • stop ", fg(c::SUBTEXT)),
            Span::styled(format!("{procs}"), fg(c::TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" detached processes (SIGTERM)", fg(c::SUBTEXT)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Deleted files can't be recovered. Anything that",
        fg(c::OVERLAY),
    )));
    lines.push(Line::from(Span::styled(
        "started using them since the scan is skipped.",
        fg(c::OVERLAY),
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            " enter ",
            Style::new()
                .bg(c::PEACH)
                .fg(c::BASE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" clear   ", fg(c::TEXT)),
        Span::styled(" esc ", Style::new().bg(c::SURFACE1).fg(c::TEXT)),
        Span::styled(" cancel", fg(c::TEXT)),
    ]));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

#[cfg(test)]
mod tests {
    use super::human;

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512K");
        assert_eq!(human(1536), "1.5M");
        assert_eq!(human(250 * 1024), "250M");
        assert_eq!(human(3 * 1024 * 1024 + 300 * 1024), "3.29G");
    }
}
