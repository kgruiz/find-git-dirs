use anyhow::Result;
use clap::Parser;
use crossbeam_channel::{bounded, select, tick, Sender};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ignore::{DirEntry, WalkBuilder};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table},
};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs,
    io::{self, stdout},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(about = "TUI scanner for all .git directories")]
struct Args {
    /// Output JSON after exit
    #[arg(long)]
    json: bool,

    /// Follow symlinks (use --no-follow-links to disable)
    #[arg(long = "no-follow-links", action = clap::ArgAction::SetFalse, default_value_t = true)]
    follow_links: bool,

    /// Extra root(s) to scan via flag (can be repeated)
    #[arg(long, value_name = "PATH")]
    root: Vec<PathBuf>,

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
}

impl RootState {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            scanned: 0,
            found: 0,
            done: false,
        }
    }
}

enum Msg {
    Scanned { root_idx: usize },
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
        paths,
    } = Args::parse();

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
        loop {
            match rx.try_recv() {
                Ok(Msg::Scanned { root_idx }) => {
                    app.roots[root_idx].scanned = app.roots[root_idx].scanned.saturating_add(1);
                }
                Ok(Msg::Found { root_idx, path }) => {
                    if app.seen_found.insert(path.clone()) {
                        app.roots[root_idx].found = app.roots[root_idx].found.saturating_add(1);
                        app.push_recent(path.clone());
                        app.all_found.push(path);
                    }
                }
                Ok(Msg::Done { root_idx }) => {
                    app.roots[root_idx].done = true;
                }
                Err(_) => break,
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
    if json {
        print_json(&app.all_found)?;
    } else {
        for p in &app.all_found {
            println!("{}", p.display());
        }
    }

    Ok(())
}

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(8),
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

    // Per-root table
    let rows: Vec<Row> = app
        .roots
        .iter()
        .map(|r| {
            let s = if r.done { "done" } else { "…" };
            Row::new(vec![
                r.path.display().to_string(),
                r.scanned.to_string(),
                r.found.to_string(),
                s.to_string(),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(60),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(vec!["root", "scanned", "found", "status"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title("roots"));
    f.render_widget(table, chunks[1]);

    // Recent finds
    let start = if app.recent.len() > 12 {
        app.recent.len() - 12
    } else {
        0
    };
    let items: Vec<ListItem> = app.recent[start..]
        .iter()
        .rev()
        .take(12)
        .map(|p| ListItem::new(p.display().to_string()))
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("recent .git found (newest first)"),
    );
    f.render_widget(list, chunks[2]);
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

    for ent in wb.build() {
        match ent {
            Ok(e) => {
                let _ = tx.send(Msg::Scanned { root_idx });
                if is_git_dir(&e) {
                    let path = canonical_dir(e.path()).unwrap_or_else(|_| e.path().to_path_buf());
                    let _ = tx.send(Msg::Found { root_idx, path });
                }
            }
            Err(_) => {
                // ignore permission or IO errors, just keep going
                let _ = tx.send(Msg::Scanned { root_idx });
            }
        }
    }

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

fn print_json(paths: &[PathBuf]) -> Result<()> {
    let mut out = String::from("[");
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let s = p
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        out.push('"');
        out.push_str(&s);
        out.push('"');
    }
    out.push(']');
    println!("{}", out);
    Ok(())
}
