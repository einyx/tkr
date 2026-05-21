use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(unix)]
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::net::UnixListener;

#[derive(Clone)]
struct SessionState {
    commands: VecDeque<CommandEntry>,
    live_lines: VecDeque<String>,
    plugin_status: String,
    total_saved: u64,
    total_in: u64,
    command_count: u64,
}

#[derive(Clone)]
struct CommandEntry {
    name: String,
    tokens_saved: i64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            commands: VecDeque::new(),
            live_lines: VecDeque::new(),
            plugin_status: "waiting for session...".into(),
            total_saved: 0,
            total_in: 0,
            command_count: 0,
        }
    }
}

pub fn run() -> Result<()> {
    #[cfg(not(unix))]
    anyhow::bail!("tkr watch requires Unix domain sockets (not available on Windows)");

    #[cfg(unix)]
    run_unix()
}

#[cfg(unix)]
fn run_unix() -> Result<()> {
    let home = dirs::home_dir().unwrap_or_default();
    let sock_path = home.join(".tkr/session.sock");

    let state = Arc::new(Mutex::new(SessionState::default()));

    {
        let _ = std::fs::remove_file(&sock_path);
        std::fs::create_dir_all(sock_path.parent().unwrap())?;

        let listener = UnixListener::bind(&sock_path)?;
        listener.set_nonblocking(true)?;

        let state_bg = state.clone();

        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let state = state_bg.clone();
                    std::thread::spawn(move || {
                        let reader = BufReader::new(stream);
                        for line in reader.lines().map_while(Result::ok) {
                            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                                handle_event(&state, &val);
                            }
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        });
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        let s = state.lock().unwrap().clone();

        terminal.draw(|f| {
            let size = f.area();

            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(3),
                ])
                .split(size);

            let ratio_pct: f64 = if s.total_in > 0 {
                (s.total_saved as f64 / s.total_in as f64) * 100.0
            } else {
                0.0
            };
            let header_text = format!(
                " tkr watch  ·  cmds: {}  ·  saved: {} / {} tokens  ·  reduction: {:.0}%",
                s.command_count, s.total_saved, s.total_in, ratio_pct
            );
            let header = Paragraph::new(header_text)
                .style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(header, outer[0]);

            let middle = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
                .split(outer[1]);

            let cmd_items: Vec<ListItem> = s
                .commands
                .iter()
                .rev()
                .take(middle[0].height as usize)
                .map(|c| {
                    let saved_str = format!("-{}", c.tokens_saved);
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:<22}", c.name)),
                        Span::styled(format!("{saved_str:>6}"), Style::default().fg(Color::Green)),
                    ]))
                })
                .collect();
            let cmd_list = List::new(cmd_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Recent Commands "),
            );
            f.render_widget(cmd_list, middle[0]);

            let live_items: Vec<ListItem> = s
                .live_lines
                .iter()
                .rev()
                .take(middle[1].height as usize)
                .map(|l| ListItem::new(l.as_str()))
                .collect();
            let live_list = List::new(live_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Live Output Stream "),
            );
            f.render_widget(live_list, middle[1]);

            let status =
                Paragraph::new(format!(" Plugins: {}  |  Press q to quit", s.plugin_status))
                    .block(Block::default().borders(Borders::ALL));
            f.render_widget(status, outer[2]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    // Clean shutdown — remove our socket so the next process doesn't see
    // a stale endpoint. (On crash this is recovered by remove_file before bind.)
    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}

fn handle_event(state: &Arc<Mutex<SessionState>>, val: &Value) {
    let mut s = state.lock().unwrap();
    match val["event"].as_str() {
        Some("command_start") => {
            let cmd = val["cmd"].as_str().unwrap_or("?").to_string();
            let args = val["args"].as_str().unwrap_or("").to_string();
            let label = if args.is_empty() {
                cmd.clone()
            } else {
                format!("{cmd} {}", args.split_whitespace().next().unwrap_or(""))
            };
            s.live_lines.push_back(format!("> {label}"));
            if s.live_lines.len() > 200 {
                s.live_lines.pop_front();
            }
            s.command_count += 1;
        }
        Some("line_suppressed") => {
            let reason = val["reason"].as_str().unwrap_or("?");
            let line = format!("  ({reason} suppressed)");
            s.live_lines.push_back(line);
            if s.live_lines.len() > 200 {
                s.live_lines.pop_front();
            }
        }
        Some("command_end") => {
            let saved = val["tokens_saved"].as_u64().unwrap_or(0);
            let total_in = val["tokens_in"].as_u64().unwrap_or(0);
            s.total_saved += saved;
            s.total_in += total_in;
            let name = s
                .live_lines
                .iter()
                .rev()
                .find(|l| l.starts_with("> "))
                .map(|l| l.trim_start_matches("> ").to_string())
                .unwrap_or_default();
            s.commands.push_back(CommandEntry {
                name,
                tokens_saved: saved as i64,
            });
            if s.commands.len() > 50 {
                s.commands.pop_front();
            }
            s.plugin_status = "filter ✓  semantic ✓  analytics ✓".into();
        }
        _ => {}
    }
}
