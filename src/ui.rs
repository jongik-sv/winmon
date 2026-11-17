//! Rendering. CPU graphs (per core or per core kind) on top, power / GPU·NPU / memory / temperatures below.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph, Sparkline, Wrap};
use ratatui::Frame;
use std::ops::Range;

use crate::app::{App, CpuView, Series};
use crate::metrics::CoreSample;
use crate::topology::CoreKind;

/// Minimum width of one per-core cell in the grid.
const MIN_CELL_WIDTH: u16 = 26;
/// Percent graphs are stored ×10 for sub-percent resolution.
const PCT_SCALE: f64 = 10.0;
const GB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let rails = power_rails(app);
    let engines = engine_rows(app);
    let row1 = rails.len().max(engines.len()) as u16 + 2;
    let row2 = 4;
    // Give the CPU grid priority on short terminals: drop the lower panels before squashing cores.
    let cpu_min = match app.view {
        CpuView::PerCore => 3 + app.topo.cpus.len().div_ceil(4) as u16,
        // Borders of the panel and of one cell, plus a single graph line.
        CpuView::ByKind => 5,
    };
    let (row1, row2) = if area.height >= 1 + cpu_min + row1 + row2 {
        (row1, row2)
    } else if area.height >= 1 + cpu_min + row1 {
        (row1, 0)
    } else {
        (0, 0)
    };

    let [header, cpu, bottom1, bottom2] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(row1),
        Constraint::Length(row2),
    ])
    .areas(area);

    draw_header(f, header, app);
    draw_cpu(f, cpu, app);
    if row1 > 0 {
        let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom1);
        draw_rows(f, left, " Power ", &rails, Color::Magenta);
        draw_rows(f, right, " GPU · NPU ", &engines, Color::Blue);
    }
    if row2 > 0 {
        let [left, right] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom2);
        draw_memory(f, left, app);
        draw_temps(f, right, app);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let topo = &app.topo;
    let mix = [CoreKind::Performance, CoreKind::Efficient, CoreKind::LowPowerEfficient, CoreKind::Uniform]
        .into_iter()
        .filter_map(|k| match topo.count(k) {
            0 => None,
            n if k == CoreKind::Uniform => Some(format!("{n} cores")),
            n => Some(format!("{n}{}", k.prefix())),
        })
        .collect::<Vec<_>>()
        .join("+");
    let line = Line::from(vec![
        Span::styled(" winmon ", Style::new().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {} · {} · {} ms", topo.cpu_name, mix, app.interval_ms)),
        Span::styled("   q quit  +/- interval  v view", Style::new().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_cpu(f: &mut Frame, area: Rect, app: &App) {
    let n = app.cores.len();
    let avg = if n == 0 { 0.0 } else { app.last.cores.iter().map(|c| c.util).sum::<f64>() / n as f64 };
    let block = Block::bordered()
        .title(Line::from(vec![" CPU ".bold(), Span::styled(format!("{avg:.0}% "), load_style(avg))]))
        .border_style(Style::new().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if n == 0 || inner.width == 0 || inner.height == 0 {
        return;
    }
    match app.view {
        CpuView::PerCore => draw_per_core(f, inner, app),
        CpuView::ByKind => draw_by_kind(f, inner, app),
    }
}

/// One graph per core kind, always side by side (P left, E right).
fn draw_by_kind(f: &mut Frame, inner: Rect, app: &App) {
    let rects = grid(inner, 1, app.kinds.len());
    for (group, rect) in app.kinds.iter().zip(rects) {
        if inner.height >= 3 {
            let label = format!("{} ×{}", kind_name(group.kind), group.range.len());
            draw_cell(f, rect, &GraphCell { label: &label, kind: group.kind, sample: group.last, series: &group.series });
        } else {
            let cell = GraphCell { label: group.kind.prefix(), kind: group.kind, sample: group.last, series: &group.series };
            draw_line(f, rect, &cell);
        }
    }
}

fn draw_per_core(f: &mut Frame, inner: Rect, app: &App) {
    let n = app.cores.len();
    // Graph cells: each core kind gets its own block of columns (P left, E right).
    let max_cols = (inner.width / MIN_CELL_WIDTH).max(1) as usize;
    let mut groups = app.topo.kind_groups();
    if groups.len() > max_cols {
        groups = vec![0..n];
    }
    let group_cols = if groups.len() == 1 {
        // Largest column count that divides the cores evenly and keeps cells readable.
        vec![(1..=max_cols.min(n)).rev().find(|c| n.is_multiple_of(*c)).unwrap_or(1)]
    } else {
        // Fewest rows whose per-kind column counts still fit the width.
        let longest = groups.iter().map(|g| g.len()).max().unwrap_or(1);
        let rows = (1..=longest)
            .find(|r| groups.iter().map(|g| g.len().div_ceil(*r)).sum::<usize>() <= max_cols)
            .unwrap_or(longest);
        groups.iter().map(|g| g.len().div_ceil(rows)).collect()
    };
    let p = place(&groups, &group_cols);

    if inner.height / p.rows as u16 >= 3 {
        let rects = grid(inner, p.rows, p.cols);
        for (i, (r, c)) in p.cells.iter().enumerate() {
            draw_cell(f, rects[r * p.cols + c], &core_cell(app, i));
        }
    } else {
        // Compact mode: one line per core, kinds still side by side.
        let groups = app.topo.kind_groups();
        let longest = groups.iter().map(|g| g.len()).max().unwrap_or(1);
        let rows = longest.min(inner.height as usize).max(1);
        let group_cols: Vec<usize> = groups.iter().map(|g| g.len().div_ceil(rows)).collect();
        let p = place(&groups, &group_cols);
        let rects = grid(Rect { height: p.rows as u16, ..inner }, p.rows, p.cols);
        for (i, (r, c)) in p.cells.iter().enumerate() {
            draw_line(f, rects[r * p.cols + c], &core_cell(app, i));
        }
    }
}

/// Grid position of every core: each group fills its own `group_cols` columns row by row,
/// groups placed left to right.
struct Placement {
    rows: usize,
    cols: usize,
    /// `(row, col)` per core index.
    cells: Vec<(usize, usize)>,
}

fn place(groups: &[Range<usize>], group_cols: &[usize]) -> Placement {
    let rows = groups.iter().zip(group_cols).map(|(g, c)| g.len().div_ceil(*c)).max().unwrap_or(1).max(1);
    let mut cells = Vec::new();
    let mut offset = 0;
    for (g, &gc) in groups.iter().zip(group_cols) {
        cells.extend((0..g.len()).map(|k| (k / gc, offset + k % gc)));
        offset += gc;
    }
    Placement { rows, cols: offset.max(1), cells }
}

/// Splits `area` into `rows`×`cols` cells, row-major.
fn grid(area: Rect, rows: usize, cols: usize) -> Vec<Rect> {
    let row_rects = Layout::vertical(vec![Constraint::Ratio(1, rows as u32); rows]).split(area);
    row_rects
        .iter()
        .flat_map(|r| Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols]).split(*r).to_vec())
        .collect()
}

/// One utilisation graph: a single core, or all cores of a kind.
struct GraphCell<'a> {
    label: &'a str,
    kind: CoreKind,
    sample: CoreSample,
    series: &'a Series,
}

fn core_cell(app: &App, i: usize) -> GraphCell<'_> {
    let cpu = &app.topo.cpus[i];
    GraphCell {
        label: &cpu.label,
        kind: cpu.kind,
        sample: app.last.cores.get(i).copied().unwrap_or_default(),
        series: &app.cores[i],
    }
}

fn draw_cell(f: &mut Frame, area: Rect, cell: &GraphCell) {
    let s = cell.sample;
    let color = kind_color(cell.kind);
    let block = Block::bordered()
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Line::from(Span::styled(format!(" {} ", cell.label), Style::new().fg(color).bold())))
        .title(
            Line::from(vec![
                Span::styled(format!("{:>3.0}%", s.util), load_style(s.util)),
                Span::styled(format!(" {} ", ghz(s.mhz)), Style::new().fg(Color::Gray)),
            ])
            .right_aligned(),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let data = cell.series.tail(inner.width as usize, PCT_SCALE);
    f.render_widget(Sparkline::default().data(&data).max((100.0 * PCT_SCALE) as u64).style(Style::new().fg(color)), inner);
}

fn draw_line(f: &mut Frame, area: Rect, cell: &GraphCell) {
    let s = cell.sample;
    let color = kind_color(cell.kind);
    let area = Rect { height: 1, ..area };
    let [label, graph, value] =
        Layout::horizontal([Constraint::Length(5), Constraint::Min(1), Constraint::Length(14)]).areas(area);
    f.render_widget(Paragraph::new(Span::styled(format!(" {:<3}", cell.label), Style::new().fg(color).bold())), label);
    let data = cell.series.tail(graph.width as usize, PCT_SCALE);
    f.render_widget(Sparkline::default().data(&data).max((100.0 * PCT_SCALE) as u64).style(Style::new().fg(color)), graph);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {:>3.0}%", s.util), load_style(s.util)),
            Span::styled(format!(" {}", ghz(s.mhz)), Style::new().fg(Color::Gray)),
        ])),
        value,
    );
}

/// One labelled row of a lower panel: `label  value  ▁▂▅▇` graph.
struct Row<'a> {
    label: &'static str,
    value: String,
    series: &'a Series,
    /// Graph ceiling in the series' unit (100 for percent, auto for watts).
    max: f64,
}

fn power_rails(app: &App) -> Vec<Row<'_>> {
    let p = app.last.power;
    [
        ("Package", p.package, &app.package_w),
        ("CPU cores", p.cores, &app.cores_w),
        ("GPU", p.gpu, &app.gpu_w),
        ("DRAM", p.dram, &app.dram_w),
    ]
    .into_iter()
    .filter(|(_, _, s)| s.seen)
    .map(|(label, v, series)| Row {
        label,
        value: v.map_or("-".into(), |w| format!("{w:.2} W")),
        series,
        // Auto-scale to the visible peak (with headroom) so small rails still show their shape.
        max: (series.max_tail(1024) * 1.2).max(0.5),
    })
    .collect()
}

fn engine_rows(app: &App) -> Vec<Row<'_>> {
    let e = app.last.engines;
    [
        ("GPU 3D", e.gpu_3d, &app.gpu_3d),
        ("GPU compute", e.compute, &app.compute),
        ("GPU video", e.video, &app.video),
        ("NPU", e.npu, &app.npu),
    ]
    .into_iter()
    .filter(|(_, _, s)| s.seen)
    .map(|(label, v, series)| Row { label, value: format!("{:.0}%", v.unwrap_or(0.0)), series, max: 100.0 })
    .collect()
}

fn draw_rows(f: &mut Frame, area: Rect, title: &'static str, rows: &[Row], color: Color) {
    let block = Block::bordered().title(title.bold()).border_style(Style::new().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if rows.is_empty() {
        f.render_widget(Paragraph::new("not available on this PC".dark_gray()), inner);
        return;
    }
    let lines = Layout::vertical(vec![Constraint::Length(1); rows.len()]).split(inner);
    for (row, rect) in rows.iter().zip(lines.iter()) {
        let [label, value, graph] =
            Layout::horizontal([Constraint::Length(12), Constraint::Length(9), Constraint::Min(1)]).areas(*rect);
        f.render_widget(Paragraph::new(format!(" {}", row.label)), label);
        f.render_widget(Paragraph::new(Span::styled(format!("{:>8} ", row.value), Style::new().fg(color).bold())), value);
        // Scale so the ceiling maps to a round integer range.
        let scale = 1000.0 / row.max;
        let data = row.series.tail(graph.width as usize, scale);
        f.render_widget(Sparkline::default().data(&data).max(1000).style(Style::new().fg(color)), graph);
    }
}

fn draw_memory(f: &mut Frame, area: Rect, app: &App) {
    let m = app.last.memory;
    let block = Block::bordered().title(" Memory ".bold()).border_style(Style::new().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [ram, commit] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let ratio = |used: u64, total: u64| if total == 0 { 0.0 } else { (used as f64 / total as f64).clamp(0.0, 1.0) };
    let r = ratio(m.used, m.total);
    f.render_widget(
        Gauge::default()
            .gauge_style(load_style(r * 100.0))
            .ratio(r)
            .label(format!("RAM  {:.1} / {:.1} GB", m.used as f64 / GB, m.total as f64 / GB)),
        ram,
    );
    let c = ratio(m.commit_used, m.commit_limit);
    let swap = m.pagefile_pct.map_or(String::new(), |p| format!(" · swap {p:.0}%"));
    f.render_widget(
        Gauge::default().gauge_style(load_style(c * 100.0)).ratio(c).label(format!(
            "Commit {:.1} / {:.1} GB{swap}",
            m.commit_used as f64 / GB,
            m.commit_limit as f64 / GB
        )),
        commit,
    );
}

fn draw_temps(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Temperature ".bold()).border_style(Style::new().fg(Color::DarkGray));
    let temps = &app.last.temps;
    if temps.is_empty() {
        f.render_widget(Paragraph::new("not available on this PC".dark_gray()).block(block), area);
        return;
    }
    let spans: Vec<Span> = temps
        .iter()
        .flat_map(|(name, c)| {
            [Span::raw(format!(" {name} ")), Span::styled(format!("{c:.0}°"), temp_style(*c)), Span::raw(" ")]
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }).block(block), area);
}

fn kind_name(kind: CoreKind) -> &'static str {
    match kind {
        CoreKind::Performance => "P cores",
        CoreKind::Efficient => "E cores",
        CoreKind::LowPowerEfficient => "LP cores",
        CoreKind::Uniform => "All cores",
    }
}

fn kind_color(kind: CoreKind) -> Color {
    match kind {
        CoreKind::Performance | CoreKind::Uniform => Color::Green,
        CoreKind::Efficient => Color::Cyan,
        CoreKind::LowPowerEfficient => Color::Blue,
    }
}

fn load_style(pct: f64) -> Style {
    Style::new().fg(match pct {
        p if p >= 80.0 => Color::Red,
        p if p >= 50.0 => Color::Yellow,
        _ => Color::Green,
    })
}

fn temp_style(c: f64) -> Style {
    Style::new().fg(match c {
        c if c >= 85.0 => Color::Red,
        c if c >= 65.0 => Color::Yellow,
        _ => Color::Green,
    })
}

fn ghz(mhz: f64) -> String {
    if mhz <= 0.0 {
        "  -  ".into()
    } else {
        format!("{:.2} GHz", mhz / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Snapshot;
    use crate::topology::{LogicalCpu, Topology};
    use ratatui::backend::TestBackend;

    fn app(view: CpuView) -> App {
        let cpu = |number, kind, label: &str| LogicalCpu { group: 0, number, kind, label: label.into() };
        let topo = Topology {
            cpu_name: "Test CPU".into(),
            cpus: vec![
                cpu(0, CoreKind::Performance, "P0"),
                cpu(1, CoreKind::Performance, "P1"),
                cpu(2, CoreKind::Efficient, "E0"),
                cpu(3, CoreKind::Efficient, "E1"),
            ],
        };
        let mut app = App::new(topo, 1000);
        app.view = view;
        let core = |util| CoreSample { util, mhz: 2000.0 };
        app.push(Snapshot { cores: vec![core(80.0), core(60.0), core(10.0), core(20.0)], ..Snapshot::default() });
        app
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|p| buf[p].symbol()).collect()
    }

    #[test]
    fn by_kind_view_replaces_core_graphs_with_one_graph_per_kind() {
        let frame = render(&app(CpuView::ByKind), 120, 40);
        assert!(frame.contains("P cores ×2") && frame.contains("70%"), "{frame}");
        assert!(frame.contains("E cores ×2") && frame.contains("15%"), "{frame}");
        assert!(!frame.contains(" P0 ") && !frame.contains(" E1 "), "{frame}");
    }

    #[test]
    fn by_kind_view_keeps_p_left_of_e_even_when_narrow() {
        let frame = render(&app(CpuView::ByKind), 44, 40);
        let row = frame.chars().collect::<Vec<_>>().chunks(44).map(|r| r.iter().collect::<String>()).find(|r| r.contains("P cores"));
        let row = row.expect("P graph is drawn");
        assert!(row.find("P cores") < row.find("E cores") && row.contains("E cores"), "{frame}");
    }

    #[test]
    fn per_core_view_is_the_default_and_shows_every_core() {
        let frame = render(&App { view: CpuView::default(), ..app(CpuView::ByKind) }, 120, 40);
        assert!(frame.contains(" P0 ") && frame.contains(" E1 "), "{frame}");
        assert!(!frame.contains("P cores"), "{frame}");
    }

    #[test]
    fn by_kind_view_survives_small_terminals() {
        let app = app(CpuView::ByKind);
        for (w, h) in [(120, 8), (40, 40), (40, 8), (40, 5), (20, 4), (10, 3), (10, 1)] {
            render(&app, w, h);
        }
    }
}
