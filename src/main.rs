//! winmon — a sudoless performance monitor for Windows, inspired by macmon.
//! Everything is read from built-in performance counters (PDH); no driver or admin rights needed.

#[cfg(not(windows))]
compile_error!("winmon only supports Windows");

mod app;
mod metrics;
mod pdh;
mod topology;
mod ui;

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use app::App;
use metrics::Sampler;
use topology::Topology;

const DEFAULT_INTERVAL_MS: u64 = 1000;
const MIN_INTERVAL_MS: u64 = 250;
const MAX_INTERVAL_MS: u64 = 10_000;
const INTERVAL_STEP_MS: u64 = 250;

const USAGE: &str = "\
winmon — performance monitor for Windows (per-core CPU, power, GPU/NPU, memory, temperature)

Usage: winmon [-i <ms>] [--once [<cols>x<rows>]]

Options:
  -i, --interval <ms>  sampling interval in milliseconds (default 1000, min 250)
      --once [WxH]     sample for a few intervals, print one frame as plain text and exit
                       (default size 120x40)
  -h, --help           show this help
  -V, --version        show version

Keys: q / Esc quit, + / - change the interval";

struct Args {
    interval_ms: u64,
    /// `--once`: render a single frame of this size to stdout.
    once: Option<(u16, u16)>,
}

fn main() {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return,
        Err(msg) => {
            eprintln!("winmon: {msg}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let interval_ms = args.interval_ms;

    let topo = Topology::detect();
    let Some(sampler) = Sampler::new() else {
        eprintln!("winmon: could not open a performance counter query (PDH)");
        std::process::exit(1);
    };

    if let Some(size) = args.once {
        if let Err(e) = print_once(sampler, App::new(topo, interval_ms), size) {
            eprintln!("winmon: {e}");
            std::process::exit(1);
        }
        return;
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, sampler, App::new(topo, interval_ms));
    ratatui::restore();
    if let Err(e) = result {
        eprintln!("winmon: {e}");
        std::process::exit(1);
    }
}

/// `Ok(None)` means help/version was printed and the program should exit.
fn parse_args() -> Result<Option<Args>, String> {
    let mut interval = DEFAULT_INTERVAL_MS;
    let mut once = None;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("winmon {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-i" | "--interval" => {
                let v = args.next().ok_or("--interval needs a value")?;
                interval = v.parse().map_err(|_| format!("invalid interval: {v}"))?;
            }
            "--once" => {
                let size = match args.peek() {
                    Some(v) if !v.starts_with('-') => {
                        let v = args.next().unwrap();
                        let (w, h) = v.split_once('x').ok_or_else(|| format!("invalid size: {v}"))?;
                        let parse = |s: &str| s.parse::<u16>().ok().filter(|n| *n >= 10);
                        parse(w).zip(parse(h)).ok_or_else(|| format!("invalid size: {v}"))?
                    }
                    _ => (120, 40),
                };
                once = Some(size);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(Args { interval_ms: interval.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS), once }))
}

/// Samples a few times so the graphs have some history, then prints one frame without colours.
fn print_once(mut sampler: Sampler, mut app: App, (w, h): (u16, u16)) -> std::io::Result<()> {
    use ratatui::backend::TestBackend;

    const SAMPLES: usize = 5;
    // Shorter than the live interval so `--once` returns in about a second.
    let step = Duration::from_millis(MIN_INTERVAL_MS);
    for _ in 0..SAMPLES {
        std::thread::sleep(step);
        let snap = sampler.sample(&app.topo);
        app.push(snap);
    }

    let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h))?;
    terminal.draw(|f| ui::draw(f, &app))?;
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        let line: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    print!("{out}");
    Ok(())
}

fn run(terminal: &mut ratatui::DefaultTerminal, mut sampler: Sampler, mut app: App) -> std::io::Result<()> {
    // The first rate-counter reading needs some time after the baseline collection.
    let mut next_sample = Instant::now() + Duration::from_millis(MIN_INTERVAL_MS);
    loop {
        let now = Instant::now();
        if now >= next_sample {
            let snap = sampler.sample(&app.topo);
            app.push(snap);
            next_sample += Duration::from_millis(app.interval_ms);
            if next_sample < now {
                // Fell behind (e.g. the machine slept): resync instead of sampling in a burst.
                next_sample = now + Duration::from_millis(app.interval_ms);
            }
        }

        terminal.draw(|f| ui::draw(f, &app))?;

        if event::poll(next_sample.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        app.interval_ms = (app.interval_ms + INTERVAL_STEP_MS).min(MAX_INTERVAL_MS);
                    }
                    KeyCode::Char('-') | KeyCode::Char('_') => {
                        app.interval_ms = app.interval_ms.saturating_sub(INTERVAL_STEP_MS).max(MIN_INTERVAL_MS);
                    }
                    _ => {}
                },
                // Resize and other events just fall through to a redraw.
                _ => {}
            }
        }
    }
}
