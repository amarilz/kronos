use crate::{config, storage::Store};
use anyhow::{Context, Result, bail};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    io::{self, Stdout},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy)]
enum SetupAction {
    OpenExisting,
    CreateNew,
}

struct SetupState {
    action: SetupAction,
    selected: usize,
    entering_path: bool,
    path: String,
    message: String,
}

pub fn run(suggested_path: &Path, message: Option<String>) -> Result<Store> {
    let mut state = SetupState {
        action: SetupAction::OpenExisting,
        selected: 0,
        entering_path: false,
        path: suggested_path.display().to_string(),
        message: message.unwrap_or_default(),
    };

    enable_raw_mode().context("Could not enable raw terminal mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, &mut state, suggested_path);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut SetupState,
    suggested_path: &Path,
) -> Result<Store> {
    loop {
        terminal.draw(|frame| draw(frame, state, suggested_path))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                if let Some(store) = handle_key(state, key)? {
                    return Ok(store);
                }
            }
            Event::Paste(text) if state.entering_path => {
                state.path.push_str(&text);
                state.message.clear();
            }
            _ => {}
        }
    }
}

fn handle_key(state: &mut SetupState, key: KeyEvent) -> Result<Option<Store>> {
    if !state.entering_path {
        match key.code {
            KeyCode::Up | KeyCode::Down => state.selected = 1 - state.selected,
            KeyCode::Enter => {
                state.action = if state.selected == 0 {
                    SetupAction::OpenExisting
                } else {
                    SetupAction::CreateNew
                };
                state.entering_path = true;
                state.message.clear();
            }
            KeyCode::Esc | KeyCode::Char('q') => bail!("Database setup cancelled"),
            _ => {}
        }
        return Ok(None);
    }

    match key.code {
        KeyCode::Esc => {
            state.entering_path = false;
            state.message.clear();
        }
        KeyCode::Backspace => {
            state.path.pop();
            state.message.clear();
        }
        KeyCode::Char(ch) if !ch.is_control() => {
            state.path.push(ch);
            state.message.clear();
        }
        KeyCode::Enter => {
            let path = PathBuf::from(state.path.trim());
            if !path.is_absolute() {
                state.message = "Enter an absolute file path.".to_owned();
                return Ok(None);
            }

            let result = match state.action {
                SetupAction::OpenExisting => Store::open_existing(&path),
                SetupAction::CreateNew => Store::create_new(&path),
            };
            match result {
                Ok(store) => match config::save_database_path(&path) {
                    Ok(()) => return Ok(Some(store)),
                    Err(error) => state.message = error.to_string(),
                },
                Err(error) => state.message = error.to_string(),
            }
        }
        _ => {}
    }
    Ok(None)
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &SetupState, suggested_path: &Path) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(3),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(" Kronos · Database setup ")
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::ALL)),
        areas[0],
    );

    if state.entering_path {
        let action = match state.action {
            SetupAction::OpenExisting => "Open an existing SQLite database",
            SetupAction::CreateNew => "Create a new SQLite database",
        };
        let content = format!("{action}\n\nAbsolute database file path:\n{}_", state.path);
        frame.render_widget(
            Paragraph::new(content)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Database path"),
                )
                .wrap(Wrap { trim: true }),
            areas[1],
        );
    } else {
        let options = [
            "Open an existing SQLite database",
            "Create a new SQLite database",
        ];
        let items = options
            .iter()
            .map(|option| ListItem::new(format!("  {option}")))
            .collect::<Vec<_>>();
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected));
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Choose an option"),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> "),
            areas[1],
            &mut list_state,
        );
        let suggestion = ratatui::layout::Rect {
            x: areas[1].x.saturating_add(2),
            y: areas[1].y.saturating_add(areas[1].height.saturating_sub(2)),
            width: areas[1].width.saturating_sub(4),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(format!("Suggested path: {}", suggested_path.display()))
                .style(Style::default().fg(Color::Gray)),
            suggestion,
        );
    }

    let footer_text = if state.message.is_empty() {
        if state.entering_path {
            "Type or paste the absolute path · Enter Continue · Esc Back"
        } else {
            "↑↓ Navigate · Enter Select · q Quit"
        }
    } else {
        &state.message
    };
    frame.render_widget(
        Paragraph::new(footer_text)
            .style(Style::default().fg(if state.message.is_empty() {
                Color::Gray
            } else {
                Color::Red
            }))
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        areas[2],
    );
}
