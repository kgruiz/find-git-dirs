use anyhow::Result;
use clap::Parser;
use crossbeam_channel::{bounded, select, tick, Sender};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ignore::{DirEntry, WalkBuilder, WalkState};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Wrap},
};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    io::{self, stdout, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(about = "TUI scanner for all .git directories")]
struct Args {
    /// Output JSON after exit (default)
    #[arg(long, action = clap::ArgAction::SetTrue, conflicts_with = "plain")]
    json: bool,

    /// Follow symlinks (use --no-follow-links to disable)
    #[arg(long = "no-follow-links", action = clap::ArgAction::SetFalse, default_value_t = true)]
    follow_links: bool,

    /// Extra root(s) to scan via flag (can be repeated)
    #[arg(long, value_name = "PATH")]
    root: Vec<PathBuf>,

    /// Write the final results to a file instead of stdout
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Output newline-delimited paths instead of JSON
    #[arg(long, action = clap::ArgAction::SetTrue)]
    plain: bool,

    /// Root path(s) to scan as positional arguments
    #[arg(value_name = "PATH", num_args = 0.., trailing_var_arg = true)]
    paths: Vec<PathBuf>,
}

#[derive(Clone)]
struct RootState {
    path: PathBuf,
    scanned: u64,
    found: u64,
    done: bool,
    current: Option<PathBuf>,
}

impl RootState {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            scanned: 0,
            found: 0,
            done: false,
            current: None,
        }
    }
}

enum Msg {
    Scanned { root_idx: usize },
    Progress { root_idx: usize, path: PathBuf },
    Found { root_idx: usize, path: PathBuf },
    Done { root_idx: usize },
}

struct App {
    start: Instant,
    roots: Vec<RootState>,
    recent: Vec<PathBuf>,
    all_found: Vec<PathBuf>,
    seen_found: HashSet<PathBuf>,
}

impl App {
    fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            start: Instant::now(),
            roots: roots.into_iter().map(RootState::new).collect(),
            recent: Vec::new(),
            all_found: Vec::new(),
            seen_found: HashSet::new(),
        }
    }

    fn total_scanned(&self) -> u64 {
        self.roots.iter().map(|r| r.scanned).sum()
    }

    fn total_found(&self) -> u64 {
        self.roots.iter().map(|r| r.found).sum()
    }

    fn all_done(&self) -> bool {
        self.roots.iter().all(|r| r.done)
    }

    fn push_recent(&mut self, p: PathBuf) {
        self.recent.push(p);
        if self.recent.len() > 12 {
            let over = self.recent.len() - 12;
            self.recent.drain(0..over);
        }
    }
}

fn main() -> Result<()> {
    let Args {
        json,
        follow_links,
        root,
        output,
        plain,
        paths,
    } = Args::parse();

    let json_output = !plain || json;

    let mut roots = if paths.is_empty() && root.is_empty() {
        os_roots()
    } else {
        paths
    };
    roots.extend(root);
    roots.sort();
    roots.dedup();
    roots.retain(|p| p.is_dir());
    if roots.is_empty() {
        eprintln!("No valid roots to scan.");
        return Ok(());
    }

    let (tx, rx) = bounded::<Msg>(1024);
    spawn_scanners(&roots, follow_links, tx)?;

    let mut live_output = match output.as_ref() {
        Some(dest) => Some(LiveOutput::new(dest, json_output)?),
        None => None,
    };

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;

    let mut app = App::new(roots);
    let tick_rate = tick(Duration::from_millis(100));

    // Event loop
    loop {
        // Drain messages fast before drawing
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Scanned { root_idx } => {
                    app.roots[root_idx].scanned = app.roots[root_idx].scanned.saturating_add(1);
                }
                Msg::Progress { root_idx, path } => {
                    app.roots[root_idx].current = Some(path);
                }
                Msg::Found { root_idx, path } => {
                    if app.seen_found.insert(path.clone()) {
                        app.roots[root_idx].found = app.roots[root_idx].found.saturating_add(1);
                        app.push_recent(path.clone());
                        app.all_found.push(path.clone());
                        if let Some(writer) = live_output.as_mut() {
                            writer.record(&path)?;
                        }
                    }
                }
                Msg::Done { root_idx } => {
                    app.roots[root_idx].done = true;
                    app.roots[root_idx].current = None;
                }
            }
        }

        terminal.draw(|f| draw(f, &app))?;

        // Exit if user quits or all done and user hits Enter
        select! {
            recv(tick_rate) -> _ => {},
            default => {}
        }

        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if k.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        break
                    }
                    _ => {}
                }
            }
        }

        if app.all_done() && app.total_scanned() > 0 {
            // Give user a moment to view, then exit on auto-complete if no key press
            // Non-blocking; continue loop until user quits.
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, LeaveAlternateScreen)?;

    // Output results
    if let Some(writer) = live_output.as_mut() {
        writer.finalize()?;
    } else {
        emit_results(&app.all_found, json_output, None)?;
    }

    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    let header_height = 3;
    let min_recent_height = 4;
    let desired_root_height = if app.roots.len() <= 1 {
        4
    } else {
        app.roots.len() as u16 + 3
    };
    let available_for_roots = area
        .height
        .saturating_sub(header_height + min_recent_height)
        .max(3);
    let root_height = desired_root_height.min(available_for_roots);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(root_height),
            Constraint::Min(min_recent_height),
        ])
        .split(area);

    // Header
    let elapsed = app.start.elapsed().as_secs_f64();
    let scanned = app.total_scanned();
    let found = app.total_found();
    let rate = if elapsed > 0.0 {
        scanned as f64 / elapsed
    } else {
        0.0
    };
    let status = if app.all_done() { "done" } else { "scanning" };
    let header = Paragraph::new(format!(
        "state: {}   roots: {}   scanned: {}   found: {}   rate: {:.0}/s   elapsed: {:.1}s   quit: q",
        status,
        app.roots.len(),
        scanned,
        found,
        rate,
        elapsed
    ))
    .block(Block::default().borders(Borders::ALL).title("find-git-dirs"));
    f.render_widget(header, chunks[0]);

    render_root_panel(f, app, chunks[1]);
    render_recent(f, app, chunks[2]);
}

fn render_root_panel(f: &mut Frame, app: &App, area: Rect) {
    if app.roots.len() == 1 {
        render_single_root(f, &app.roots[0], area);
    } else {
        render_root_table(f, &app.roots, area);
    }
}

fn render_single_root(f: &mut Frame, root: &RootState, area: Rect) {
    let status = if root.done { "done" } else { "scanning" };
    let current = if root.done {
        "complete".to_string()
    } else {
        root.current
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "starting…".to_string())
    };

    let lines = vec![
        Line::from(root.path.display().to_string()),
        Line::from(format!(
            "status: {}   scanned: {}   found: {}",
            status, root.scanned, root.found
        )),
        Line::from(format!("current: {}", current)),
    ];

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("root"))
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_root_table(f: &mut Frame, roots: &[RootState], area: Rect) {
    let rows: Vec<Row> = roots
        .iter()
        .map(|r| {
            let status = if r.done { "done" } else { "…" };
            let current = if r.done {
                "complete".to_string()
            } else {
                r.current
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "…".to_string())
            };
            Row::new(vec![
                r.path.display().to_string(),
                r.scanned.to_string(),
                r.found.to_string(),
                status.to_string(),
                current,
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Percentage(40),
        ],
    )
    .header(
        Row::new(vec!["root", "scanned", "found", "status", "current path"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title("roots"));
    f.render_widget(table, area);
}

fn render_recent(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    if area.height <= 5 {
        render_current_paths(f, app, area);
        return;
    }

    let current_height = area.height.clamp(3, 5);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(current_height), Constraint::Min(3)])
        .split(area);

    render_current_paths(f, app, chunks[0]);
    render_recent_list(f, app, chunks[1]);
}

fn render_current_paths(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    if app.roots.is_empty() {
        lines.push(Line::from("no roots queued"));
    } else {
        for root in &app.roots {
            let marker = if root.done { "✓" } else { "▶" };
            let current = if root.done {
                "complete".to_string()
            } else {
                root.current
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "starting scan".to_string())
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}", root.path.display()),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::raw(marker),
                Span::raw(" → "),
                Span::raw(current),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("current traversal"),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn render_recent_list(f: &mut Frame, app: &App, area: Rect) {
    if area.height < 3 {
        return;
    }

    let capacity = area.height.saturating_sub(2) as usize;
    let window = capacity.clamp(1, 12);
    let start = app.recent.len().saturating_sub(window);
    let items: Vec<ListItem> = app.recent[start..]
        .iter()
        .rev()
        .take(window)
        .map(|p| ListItem::new(p.display().to_string()))
        .collect();

    if items.is_empty() {
        let placeholder = Paragraph::new("no .git directories found yet").block(
            Block::default()
                .borders(Borders::ALL)
                .title("recent .git found"),
        );
        f.render_widget(placeholder, area);
    } else {
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("recent .git found (newest first)"),
        );
        f.render_widget(list, area);
    }
}

fn spawn_scanners(roots: &[PathBuf], follow_links: bool, tx: Sender<Msg>) -> Result<()> {
    // Spawn one thread per root to avoid blocking the UI
    for (idx, root) in roots.iter().cloned().enumerate() {
        let txc = tx.clone();
        thread::spawn(move || scan_root(idx, &root, follow_links, txc));
    }

    Ok(())
}

fn scan_root(root_idx: usize, root: &Path, follow_links: bool, tx: Sender<Msg>) {
    let mut wb = WalkBuilder::new(root);
    wb.standard_filters(false)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .follow_links(follow_links);

    let _ = tx.send(Msg::Progress {
        root_idx,
        path: root.to_path_buf(),
    });
    let throttle = Duration::from_millis(120);
    let last_progress = Arc::new(Mutex::new(Instant::now()));

    wb.build_parallel().run(|| {
        let txc = tx.clone();
        let last_progress = Arc::clone(&last_progress);
        Box::new(move |result| {
            match result {
                Ok(entry) => {
                    let _ = txc.send(Msg::Scanned { root_idx });
                    let now = Instant::now();
                    let should_report = {
                        if let Ok(mut last) = last_progress.lock() {
                            if now.duration_since(*last) >= throttle {
                                *last = now;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };

                    let path_buf = entry.path().to_path_buf();
                    if should_report {
                        let _ = txc.send(Msg::Progress {
                            root_idx,
                            path: path_buf.clone(),
                        });
                    }

                    if is_git_dir(&entry) {
                        let path = canonical_dir(entry.path()).unwrap_or_else(|_| path_buf.clone());
                        let _ = txc.send(Msg::Found { root_idx, path });
                    }
                }
                Err(_) => {
                    // ignore permission or IO errors, just keep going
                    let _ = txc.send(Msg::Scanned { root_idx });
                }
            }
            WalkState::Continue
        })
    });

    let _ = tx.send(Msg::Done { root_idx });
}

fn is_git_dir(entry: &DirEntry) -> bool {
    entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
        && entry
            .path()
            .file_name()
            .and_then(OsStr::to_str)
            .map(|n| n.eq_ignore_ascii_case(".git"))
            .unwrap_or(false)
}

fn canonical_dir(p: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(p) {
        Ok(c) => Ok(c),
        Err(_) => Ok(p.to_path_buf()),
    }
}

#[cfg(target_os = "windows")]
fn os_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for drive in b'A'..=b'Z' {
        let root = format!("{}:\\", drive as char);
        let p = PathBuf::from(&root);
        if fs::read_dir(&p).is_ok() {
            v.push(p);
        }
    }
    v
}

#[cfg(not(target_os = "windows"))]
fn os_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

fn emit_results(paths: &[PathBuf], json: bool, output: Option<&Path>) -> Result<()> {
    match output {
        Some(dest) => write_results(dest, json, paths),
        None if json => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            write_json(&mut handle, paths)
        }
        None => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            for p in paths {
                writeln!(handle, "{}", p.display())?;
            }
            Ok(())
        }
    }
}

fn write_results(path: &Path, json: bool, paths: &[PathBuf]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    if json {
        write_json(&mut writer, paths)?;
    } else {
        for p in paths {
            writeln!(writer, "{}", p.display())?;
        }
    }
    writer.flush()?;
    Ok(())
}

fn write_json<W: Write>(mut writer: W, paths: &[PathBuf]) -> Result<()> {
    writer.write_all(b"[")?;
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            writer.write_all(b",")?;
        }
        writer.write_all(b"\"")?;
        writer.write_all(escape_json_path(p).as_bytes())?;
        writer.write_all(b"\"")?;
    }
    writer.write_all(b"]\n")?;
    Ok(())
}

fn escape_json_path(path: &Path) -> String {
    path.display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

struct LiveOutput {
    inner: LiveOutputKind,
}

enum LiveOutputKind {
    Json {
        writer: io::BufWriter<fs::File>,
        first: bool,
    },
    Plain {
        writer: io::BufWriter<fs::File>,
    },
}

impl LiveOutput {
    fn new(path: &Path, json: bool) -> Result<Self> {
        let file = fs::File::create(path)?;
        let mut writer = io::BufWriter::new(file);
        let inner = if json {
            writer.write_all(b"[")?;
            LiveOutputKind::Json {
                writer,
                first: true,
            }
        } else {
            LiveOutputKind::Plain { writer }
        };
        Ok(Self { inner })
    }

    fn record(&mut self, path: &Path) -> Result<()> {
        match &mut self.inner {
            LiveOutputKind::Json { writer, first } => {
                if !*first {
                    writer.write_all(b",")?;
                }
                writer.write_all(b"\n  \"")?;
                writer.write_all(escape_json_path(path).as_bytes())?;
                writer.write_all(b"\"")?;
                writer.flush()?;
                *first = false;
            }
            LiveOutputKind::Plain { writer } => {
                writeln!(writer, "{}", path.display())?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    fn finalize(&mut self) -> Result<()> {
        match &mut self.inner {
            LiveOutputKind::Json { writer, first } => {
                if *first {
                    writer.write_all(b"]\n")?;
                } else {
                    writer.write_all(b"\n]\n")?;
                }
                writer.flush()?;
            }
            LiveOutputKind::Plain { writer } => {
                writer.flush()?;
            }
        }
        Ok(())
    }
}
