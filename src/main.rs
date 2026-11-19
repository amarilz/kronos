mod config;
mod model;
mod setup;
mod storage;

use anyhow::{Context, Result, bail};
use chrono::{Local, Months, NaiveDate, Utc};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use model::{
    CurrentEntry, GoalProgress, HistoryEntry, WeeklyGoal, aggregate_current, aggregate_history,
    iso_week_key, week_start,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    cmp::Reverse,
    io::{self, Stdout},
    path::PathBuf,
    time::Duration as StdDuration,
};
use storage::{ActiveTimer, Store};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Screen {
    Dashboard,
    ResumeTimer,
    TopicSelect,
    TimerTopicSelect,
    DateSelect,
    Duration,
    AddTopic,
    History,
    GoalProgress,
    EditGoal,
}

struct App {
    store: Store,
    active_timer: Option<ActiveTimer>,
    topics: Vec<String>,
    studied_topic_count: usize,
    current: Vec<CurrentEntry>,
    history: Vec<HistoryEntry>,
    screen: Screen,
    selected: usize,
    date_field: usize,
    study_date: NaiveDate,
    topic_input: String,
    topic_filter: String,
    hours: u32,
    minutes: u32,
    duration_field: usize,
    step: u32,
    history_weeks: Vec<(i32, u32)>,
    history_index: usize,
    goal_progress: GoalProgress,
    snapshot_date: NaiveDate,
    goal_input: String,
    message: String,
    quit: bool,
}

impl App {
    fn new(store: Store) -> Result<Self> {
        let today = Local::now().date_naive();
        let mut app = Self {
            store,
            active_timer: None,
            topics: Vec::new(),
            studied_topic_count: 0,
            current: Vec::new(),
            history: Vec::new(),
            screen: Screen::Dashboard,
            selected: 0,
            date_field: 0,
            study_date: today,
            topic_input: String::new(),
            topic_filter: String::new(),
            hours: 0,
            minutes: 0,
            duration_field: 0,
            step: 1,
            history_weeks: Vec::new(),
            history_index: 0,
            goal_progress: GoalProgress::NoGoal,
            snapshot_date: today,
            goal_input: String::new(),
            message: String::new(),
            quit: false,
        };
        app.reload()?;
        if app.active_timer.is_some() {
            app.screen = Screen::ResumeTimer;
        }
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        let today = Local::now().date_naive();
        self.store.rollover_if_needed(today)?;
        self.active_timer = self.store.active_timer()?;
        self.topics = self.store.topics()?;
        self.current = self.store.load_current()?;
        let monday = week_start(today);
        self.current.retain(|entry| entry.date >= monday);
        let totals = aggregate_current(&self.current);
        self.studied_topic_count = totals
            .values()
            .filter(|&&duration_ms| duration_ms > 0)
            .count();
        // Stable sorting keeps the store's alphabetical order for equal totals.
        self.topics
            .sort_by_key(|topic| Reverse(totals.get(topic).copied().unwrap_or(0)));
        self.history = self.store.load_history()?;
        self.goal_progress = self.store.goal_progress(today)?;
        self.snapshot_date = today;
        self.history_weeks = self
            .history
            .iter()
            .map(|entry| (entry.iso_year, entry.iso_week))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .rev()
            .collect();
        self.selected = self
            .selected
            .min(self.studied_topic_count.saturating_sub(1));
        self.history_index = self
            .history_index
            .min(self.history_weeks.len().saturating_sub(1));
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        self.message.clear();
        match self.screen {
            Screen::ResumeTimer => match key.code {
                KeyCode::Char('y' | 'Y' | 'n' | 'N') if key.kind == KeyEventKind::Press => {
                    let pending_id = self.active_timer.as_ref().map(|timer| timer.id);
                    let discard = matches!(key.code, KeyCode::Char('n' | 'N'));
                    if discard && let Some(id) = pending_id {
                        self.store.discard_timer(id)?;
                    }
                    self.reload()?;
                    if self.active_timer.is_none()
                        || (!discard
                            && self.active_timer.as_ref().map(|timer| timer.id) == pending_id)
                    {
                        self.screen = Screen::Dashboard;
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Screen::Dashboard => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                KeyCode::Char('m') if key.kind == KeyEventKind::Press => {
                    if let Some(timer) = &self.active_timer {
                        let result = self.store.stop_timer(timer.id, Utc::now())?;
                        self.reload()?;
                        self.message = match result {
                            Some(duration_ms) => {
                                format!("Timer saved · {}", format_duration(duration_ms as u64))
                            }
                            None => "This timer was already stopped".into(),
                        };
                    } else {
                        self.reload()?;
                        if self.active_timer.is_none() {
                            self.topic_filter.clear();
                            self.selected = 0;
                            self.screen = Screen::TimerTopicSelect;
                        }
                    }
                }
                KeyCode::Char('a') => {
                    self.reload()?;
                    self.topic_filter.clear();
                    self.selected = 0;
                    self.screen = Screen::TopicSelect;
                }
                KeyCode::Char('t') => {
                    self.topic_input.clear();
                    self.screen = Screen::AddTopic;
                }
                KeyCode::Char('g') => self.open_goal_progress()?,
                KeyCode::Char('h') => {
                    self.history_index = 0;
                    self.screen = Screen::History;
                }
                KeyCode::Down => self.move_selection(1, self.studied_topic_count),
                KeyCode::Up => self.move_selection(-1, self.studied_topic_count),
                _ => {}
            },
            Screen::TopicSelect | Screen::TimerTopicSelect => match key.code {
                KeyCode::Esc => self.screen = Screen::Dashboard,
                KeyCode::Up => self.move_selection(-1, self.filtered_topics().count()),
                KeyCode::Down => self.move_selection(1, self.filtered_topics().count()),
                KeyCode::Enter
                    if self.screen == Screen::TimerTopicSelect
                        && self.filtered_topics().next().is_some() =>
                {
                    let topic = self
                        .filtered_topics()
                        .nth(self.selected)
                        .context("No topic selected")?
                        .to_owned();
                    let result = self.store.start_timer(&topic, Utc::now());
                    self.screen = Screen::Dashboard;
                    self.reload()?;
                    result?;
                }
                KeyCode::Enter if self.filtered_topics().next().is_some() => {
                    self.study_date = Local::now().date_naive();
                    self.date_field = 0;
                    self.screen = Screen::DateSelect;
                }
                KeyCode::Backspace => {
                    self.topic_filter.pop();
                    self.selected = 0;
                }
                KeyCode::Char(ch) if !ch.is_control() => {
                    self.topic_filter.push(ch);
                    self.selected = 0;
                }
                _ => {}
            },
            Screen::DateSelect => match key.code {
                KeyCode::Esc => self.screen = Screen::TopicSelect,
                KeyCode::Left => self.date_field = self.date_field.saturating_sub(1),
                KeyCode::Right => self.date_field = (self.date_field + 1).min(2),
                KeyCode::Up => self.adjust_study_date(true),
                KeyCode::Down => self.adjust_study_date(false),
                KeyCode::Enter => self.begin_duration(),
                _ => {}
            },
            Screen::Duration => match key.code {
                KeyCode::Esc => self.screen = Screen::DateSelect,
                KeyCode::Left => self.duration_field = 0,
                KeyCode::Right => self.duration_field = 1,
                KeyCode::Up => self.adjust_duration(1),
                KeyCode::Down => self.adjust_duration(-1),
                KeyCode::Char('s') => self.step = next_step(self.step),
                KeyCode::Enter => {
                    let total = self.hours * 60 + self.minutes;
                    if total == 0 {
                        bail!("Duration must be greater than zero");
                    }
                    let topic = self
                        .filtered_topics()
                        .nth(self.selected)
                        .context("No topic selected")?
                        .to_owned();
                    self.store.add_study(self.study_date, &topic, total)?;
                    self.reload()?;
                    self.message = format!(
                        "Added {}h {}m to {} on {}",
                        self.hours, self.minutes, topic, self.study_date
                    );
                    self.screen = Screen::Dashboard;
                }
                _ => {}
            },
            Screen::AddTopic => match key.code {
                KeyCode::Esc => self.screen = Screen::Dashboard,
                KeyCode::Backspace => {
                    self.topic_input.pop();
                }
                KeyCode::Enter => {
                    self.store.add_topic(&self.topic_input)?;
                    self.reload()?;
                    self.message = format!("Topic \"{}\" added", self.topic_input.trim());
                    self.screen = Screen::Dashboard;
                }
                KeyCode::Char(ch) if !ch.is_control() => self.topic_input.push(ch),
                _ => {}
            },
            Screen::GoalProgress => match key.code {
                KeyCode::Esc | KeyCode::Char('g') => self.screen = Screen::Dashboard,
                KeyCode::Char('e') => {
                    self.goal_input.clear();
                    self.screen = Screen::EditGoal;
                }
                _ => {}
            },
            Screen::EditGoal => match key.code {
                KeyCode::Esc => self.screen = Screen::GoalProgress,
                KeyCode::Backspace => {
                    self.goal_input.pop();
                }
                KeyCode::Enter => {
                    let fields: Vec<_> = self.goal_input.split_whitespace().collect();
                    let [date, minutes] = fields.as_slice() else {
                        bail!("Enter a Monday date and weekly minutes");
                    };
                    let starts_on = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                        .context("Enter the start date as YYYY-MM-DD")?;
                    let minutes_per_week =
                        minutes.parse().context("Enter whole minutes per week")?;
                    self.store.save_weekly_goal(WeeklyGoal {
                        starts_on,
                        minutes_per_week,
                    })?;
                    self.open_goal_progress()?;
                }
                KeyCode::Char(ch) if !ch.is_control() => self.goal_input.push(ch),
                _ => {}
            },
            Screen::History => match key.code {
                KeyCode::Esc | KeyCode::Char('h') => self.screen = Screen::Dashboard,
                KeyCode::Left => self.move_history(1),
                KeyCode::Right => self.move_history(-1),
                _ => {}
            },
        }
        Ok(())
    }

    fn open_goal_progress(&mut self) -> Result<()> {
        self.goal_progress = self.store.goal_progress(Local::now().date_naive())?;
        self.screen = Screen::GoalProgress;
        Ok(())
    }

    fn filtered_topics(&self) -> impl Iterator<Item = &str> {
        let query = self.topic_filter.to_lowercase();
        self.topics
            .iter()
            .map(String::as_str)
            .filter(move |topic| topic.contains(&query))
    }

    fn begin_duration(&mut self) {
        self.hours = 0;
        self.minutes = 0;
        self.duration_field = 0;
        self.step = 1;
        self.screen = Screen::Duration;
    }

    fn adjust_duration(&mut self, direction: i64) {
        let total = self.hours.saturating_mul(60).saturating_add(self.minutes);
        let delta = if self.duration_field == 0 {
            60
        } else {
            self.step
        } as i64
            * direction;
        let updated = (total as i64 + delta).clamp(0, 99 * 60 + 59) as u32;
        self.hours = updated / 60;
        self.minutes = updated % 60;
    }

    fn move_selection(&mut self, direction: isize, length: usize) {
        if length == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + direction).rem_euclid(length as isize) as usize;
    }

    fn adjust_study_date(&mut self, forward: bool) {
        let date = self.study_date;
        let adjusted = if self.date_field == 0 {
            if forward {
                date.succ_opt()
            } else {
                date.pred_opt()
            }
        } else {
            let months = Months::new(if self.date_field == 1 { 1 } else { 12 });
            if forward {
                date.checked_add_months(months)
            } else {
                date.checked_sub_months(months)
            }
        };
        if let Some(adjusted) = adjusted.filter(|date| *date <= Local::now().date_naive()) {
            self.study_date = adjusted;
        }
    }

    fn move_history(&mut self, direction: isize) {
        self.history_index = self
            .history_index
            .saturating_add_signed(direction)
            .min(self.history_weeks.len().saturating_sub(1));
    }
}

#[derive(Parser)]
#[command(name = "kronos", about = "Offline weekly learning tracker")]
struct Cli {
    #[arg(
        long = "db",
        env = "KRONOS_DB_PATH",
        value_name = "PATH",
        global = true
    )]
    database: Option<PathBuf>,
    #[arg(long, help = "Choose or change the saved database path")]
    configure_db: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = resolve_store(cli.database, cli.configure_db)?;
    let app = App::new(store)?;
    run(app)
}

fn resolve_store(database: Option<PathBuf>, configure_db: bool) -> Result<Store> {
    if configure_db {
        return setup::run(&Store::default_database_path()?, None);
    }

    if let Some(path) = database {
        let path = absolute_path(path)?;
        return Store::open_existing(&path).with_context(|| {
            "The --db path must point to an existing, valid Kronos database. \
             Run `kronos --configure-db` to select or create one"
        });
    }

    match config::load_database_path()? {
        Some(path) => match Store::open_existing(&path) {
            Ok(store) => Ok(store),
            Err(error) => setup::run(
                &path,
                Some(format!("The saved database could not be opened: {error}")),
            ),
        },
        None => setup::run(&Store::default_database_path()?, None),
    }
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("The database path must be absolute");
    }
    Ok(path)
}

fn run(mut app: App) -> Result<()> {
    enable_raw_mode().context("Could not enable raw terminal mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    while !app.quit {
        if matches!(app.screen, Screen::Dashboard | Screen::GoalProgress)
            && app.snapshot_date != Local::now().date_naive()
        {
            app.reload()?;
        }
        terminal.draw(|frame| draw(frame, app))?;
        if event::poll(StdDuration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && let Err(error) = app.handle_key(key)
        {
            app.message = error.to_string();
        }
    }
    Ok(())
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let running = app.active_timer.is_some() && app.screen != Screen::ResumeTimer;
    let title = match &app.active_timer {
        Some(timer) if running => {
            let seconds = timer.elapsed_seconds(Utc::now());
            format!(
                " ● Timer running {:02}:{:02}:{:02} · {} ",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60,
                timer.topic
            )
        }
        _ => concat!(" kronos v", env!("CARGO_PKG_VERSION"), " ").into(),
    };
    let title_style = if running {
        Style::default().fg(Color::Black).bg(Color::LightRed)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let title = Paragraph::new(title)
        .style(title_style.add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, areas[0]);

    match app.screen {
        Screen::Dashboard => draw_dashboard(frame, app, areas[1]),
        Screen::ResumeTimer => {
            if let Some(timer) = &app.active_timer {
                let prompt = Paragraph::new(format!(
                    "Continue the running timer for \"{}\"?\nDiscarding deletes the session without adding study time.",
                    timer.topic
                ))
                .wrap(Wrap { trim: true })
                .block(Block::bordered().title("Resume study timer"));
                frame.render_widget(prompt, areas[1]);
            }
        }
        Screen::TopicSelect | Screen::TimerTopicSelect => draw_topic_select(frame, app, areas[1]),
        Screen::DateSelect => draw_date_select(frame, app, areas[1]),
        Screen::Duration => draw_duration(frame, app, areas[1]),
        Screen::AddTopic => draw_input(
            frame,
            "New topic · Enter to save",
            &app.topic_input,
            areas[1],
        ),
        Screen::History => draw_history(frame, app, areas[1]),
        Screen::GoalProgress => draw_goal_progress(frame, &app.goal_progress, areas[1]),
        Screen::EditGoal => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("Enter a Monday (YYYY-MM-DD) and minutes per week."),
                    Line::from("Example: 2026-11-16 420"),
                    Line::from(format!("{}_", app.goal_input)),
                ])
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title("Set weekly goal")),
                areas[1],
            );
        }
    }

    let help = help_text(app);
    let footer = Paragraph::new(help)
        .style(Style::default().fg(Color::Gray))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, areas[2]);
}

fn format_duration(duration_ms: u64) -> String {
    let seconds = duration_ms / 1000;
    let fractional = duration_ms % 1000;
    let clock = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    if fractional == 0 {
        clock
    } else {
        format!("{clock}.{fractional:03}")
    }
}

fn format_goal_duration(duration_ms: u64, show_days: bool) -> String {
    let total_minutes = duration_ms / 60_000;
    if total_minutes == 0 && duration_ms > 0 {
        return "<1m".into();
    }
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if show_days && hours >= 24 {
        format!("{}d {}h {minutes:02}m", hours / 24, hours % 24)
    } else {
        format!("{hours}h {minutes:02}m")
    }
}

fn goal_balance(studied_ms: u64, expected_ms: u64) -> (&'static str, Color) {
    match studied_ms.cmp(&expected_ms) {
        std::cmp::Ordering::Greater => ("Ahead by", Color::Green),
        std::cmp::Ordering::Less => ("Behind by", Color::LightRed),
        std::cmp::Ordering::Equal => ("On track", Color::Green),
    }
}

fn draw_goal_progress(
    frame: &mut ratatui::Frame<'_>,
    progress: &GoalProgress,
    area: ratatui::layout::Rect,
) {
    let lines = match progress {
        GoalProgress::NoGoal => vec![Line::from("No weekly goal. Press e to set one.")],
        GoalProgress::Scheduled(goal) => vec![Line::from(format!(
            "Plan starts on {} · {} per week",
            goal.starts_on,
            format_goal_duration(u64::from(goal.minutes_per_week) * 60_000, false)
        ))],
        GoalProgress::Started {
            starts_on,
            current_goal,
            next_goal,
            studied_ms,
            expected_ms,
        } => {
            let weekly_goal_ms = u64::from(current_goal.minutes_per_week) * 60_000;
            let difference_ms = studied_ms.abs_diff(*expected_ms);
            let mut difference = format_goal_duration(difference_ms, false);
            if difference_ms >= weekly_goal_ms {
                let equivalent_weeks = difference_ms as f64 / weekly_goal_ms as f64;
                difference.push_str(&format!(" (~{equivalent_weeks:.1} weeks at current goal)"));
            }
            let (label, color) = goal_balance(*studied_ms, *expected_ms);
            let status = if difference_ms == 0 {
                label.to_owned()
            } else {
                format!("{label} {difference}")
            };
            let mut lines = vec![
                Line::from(format!("Plan since {starts_on}")),
                Line::from(format!(
                    "{:<24} {}",
                    "Current weekly goal",
                    format_goal_duration(weekly_goal_ms, false)
                )),
                Line::from(format!(
                    "{:<24} {}",
                    "Total studied",
                    format_goal_duration(*studied_ms, true)
                )),
                Line::from(format!(
                    "{:<24} {}",
                    "Expected through today",
                    format_goal_duration(*expected_ms, true)
                )),
                Line::styled(
                    status,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ];
            if let Some(goal) = next_goal {
                lines.push(Line::from(format!(
                    "{:<24} {} from {}",
                    "Next weekly goal",
                    format_goal_duration(u64::from(goal.minutes_per_week) * 60_000, false),
                    goal.starts_on,
                )));
            }
            lines
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title("Goal progress · 1d = 24h")),
        area,
    );
}

fn draw_dashboard(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let today = app.snapshot_date;
    let start = week_start(today);
    let (iso_year, iso_week) = iso_week_key(today);
    let totals = aggregate_current(&app.current);
    let total: u64 = totals.values().sum();
    let area = if let GoalProgress::Started { current_goal, .. } = app.goal_progress {
        let days_elapsed = (today - start).num_days() as u64 + 1;
        let expected_ms = u64::from(current_goal.minutes_per_week) * 60_000 * days_elapsed / 7;
        let (label, color) = goal_balance(total, expected_ms);
        let balance = if total == expected_ms {
            format!("  This week · {label}")
        } else {
            let difference = format_goal_duration(total.abs_diff(expected_ms), false);
            format!("  This week · {label} {difference}")
        };
        let [status, study_list] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(
            Paragraph::new(balance).style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
            status,
        );
        study_list
    } else {
        area
    };
    let rows = app
        .topics
        .iter()
        .take(app.studied_topic_count)
        .map(|topic| ListItem::new(format!("  {topic:<20} {}", format_duration(totals[topic]))))
        .collect::<Vec<_>>();
    let rows = if rows.is_empty() {
        vec![ListItem::new("  No study time recorded this week.")]
    } else {
        rows
    };
    let list = List::new(rows)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " ISO week {iso_year}-W{iso_week:02} · Monday {start} · Total {} ",
            format_duration(total)
        )))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default();
    if app.studied_topic_count > 0 {
        state.select(Some(app.selected.min(app.studied_topic_count - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
    if !app.message.is_empty() {
        let message_area = ratatui::layout::Rect {
            x: area.x.saturating_add(2),
            y: area.y.saturating_add(area.height.saturating_sub(2)),
            width: area.width.saturating_sub(4),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(app.message.as_str()).style(Style::default().fg(Color::Green)),
            message_area,
        );
    }
}

fn draw_topic_select(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let [search, results] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(format!(" Filter: {}_", app.topic_filter)),
        search,
    );

    let title = if app.screen == Screen::TimerTopicSelect {
        "Select a topic to start the timer"
    } else {
        "Select a topic"
    };
    let topics: Vec<_> = app.filtered_topics().collect();
    if topics.is_empty() {
        let message = if app.topics.is_empty() {
            "No topics yet."
        } else {
            "No matching topics."
        };
        frame.render_widget(
            Paragraph::new(message).block(Block::default().borders(Borders::ALL).title(title)),
            results,
        );
    } else {
        draw_list(frame, title, &topics, app.selected, results);
    }
}

fn draw_list(
    frame: &mut ratatui::Frame<'_>,
    title: &str,
    options: &[impl AsRef<str>],
    selected: usize,
    area: ratatui::layout::Rect,
) {
    let items = options
        .iter()
        .map(|option| ListItem::new(format!("  {}", option.as_ref())))
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !options.is_empty() {
        state.select(Some(selected.min(options.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> "),
        area,
        &mut state,
    );
}

fn draw_input(
    frame: &mut ratatui::Frame<'_>,
    title: &str,
    value: &str,
    area: ratatui::layout::Rect,
) {
    let input = Paragraph::new(format!("{value}_"))
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: true });
    frame.render_widget(input, area);
}

fn draw_date_select(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let selected_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let date = app.study_date;
    let fields = ["%d", "%m", "%Y"];
    let mut values = Line::default();
    for (index, format) in fields.into_iter().enumerate() {
        if index > 0 {
            values.push_span(" / ");
        }
        let style = if app.date_field == index {
            selected_style
        } else {
            Style::default()
        };
        values.push_span(Span::styled(date.format(format).to_string(), style));
    }
    frame.render_widget(
        Paragraph::new(vec![values, Line::from(date.format("%A").to_string())])
            .centered()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("When did you study?"),
            ),
        area,
    );
}

fn draw_duration(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let selected_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let hour_style = if app.duration_field == 0 {
        selected_style
    } else {
        Style::default()
    };
    let minute_style = if app.duration_field == 1 {
        selected_style
    } else {
        Style::default()
    };
    let content = vec![
        Line::from(vec![Span::raw("             Hours     Minutes")]),
        Line::from(vec![
            Span::raw("             "),
            Span::styled(format!("{:02}", app.hours), hour_style),
            Span::raw("   :   "),
            Span::styled(format!("{:02}", app.minutes), minute_style),
        ]),
        Line::from(format!(
            "\n     Date: {} · Minute step: {}",
            app.study_date, app.step
        )),
    ];
    frame.render_widget(
        Paragraph::new(content).block(Block::default().borders(Borders::ALL).title("Duration")),
        area,
    );
}

fn draw_history(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    if app.history_weeks.is_empty() {
        frame.render_widget(
            Paragraph::new("No history available.")
                .block(Block::default().borders(Borders::ALL).title("History")),
            area,
        );
        return;
    }
    let (iso_year, iso_week) = app.history_weeks[app.history_index];
    let entries: Vec<HistoryEntry> = app
        .history
        .iter()
        .filter(|entry| entry.iso_year == iso_year && entry.iso_week == iso_week)
        .cloned()
        .collect();
    let totals = aggregate_history(&entries);
    let total: u32 = totals.values().sum();
    let mut ranked_topics: Vec<_> = totals.into_iter().collect();
    ranked_topics.sort_by_key(|(_, minutes)| Reverse(*minutes));
    let lines = ranked_topics
        .into_iter()
        .map(|(topic, minutes)| {
            Line::from(format!(
                "  {topic:<20} {:>3}h {:02}m",
                minutes / 60,
                minutes % 60
            ))
        })
        .collect::<Vec<_>>();
    let content = std::iter::once(Line::from(format!(
        "ISO week {iso_year}-W{iso_week:02} · Total {}h {:02}m\n",
        total / 60,
        total % 60
    )))
    .chain(lines)
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(content)
            .block(Block::default().borders(Borders::ALL).title("History"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn help_text(app: &App) -> String {
    if !app.message.is_empty() && app.screen != Screen::Dashboard {
        return format!(" Error: {} ", app.message);
    }
    match app.screen {
        Screen::ResumeTimer => " y Continue   n Discard   Esc Quit ".into(),
        Screen::Dashboard => {
            if app.message.is_empty() {
                let timer_action = if app.active_timer.is_some() {
                    "Stop"
                } else {
                    "Start"
                };
                format!(
                    " m {timer_action} timer   a Add time   t Add topic   h History   g Goals   q Quit "
                )
            } else {
                format!(" {} ", app.message)
            }
        }
        Screen::TopicSelect | Screen::TimerTopicSelect => {
            " Type to filter   ↑↓ Navigate   Enter Confirm   Esc Back ".into()
        }
        Screen::DateSelect => " ←→ Day/Month/Year   ↑↓ Adjust   Enter Confirm   Esc Back ".into(),
        Screen::AddTopic => " Type   Backspace Delete   Enter Confirm   Esc Cancel ".into(),
        Screen::Duration => {
            " ←→ Field   ↑↓ Adjust   s Step 1/5/15m   Enter Save   Esc Back ".into()
        }
        Screen::History => " ←→ Previous/next week   Esc Back to dashboard ".into(),
        Screen::GoalProgress => " e Set weekly goal   Esc Back ".into(),
        Screen::EditGoal => " Enter Save   Esc Cancel ".into(),
    }
}

fn next_step(step: u32) -> u32 {
    match step {
        1 => 5,
        5 => 15,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;

    #[test]
    fn dashboard_colors_the_weekly_balance_independently_of_the_plan_balance() -> Result<()> {
        let directory = storage::tests::temporary_directory();
        let mut store = Store::create_new(&directory.join("kronos.sqlite3"))?;
        let today = Local::now().date_naive();
        let previous_monday = week_start(today) - Duration::weeks(1);
        store.add_topic("rust")?;
        store.save_weekly_goal(WeeklyGoal {
            starts_on: previous_monday,
            minutes_per_week: 420,
        })?;
        store.add_study(previous_monday, "rust", 900)?;
        let mut app = App::new(store)?;
        let mut terminal = Terminal::new(TestBackend::new(80, 16))?;

        assert!(
            matches!(app.goal_progress, GoalProgress::Started { studied_ms, expected_ms, .. }
            if studied_ms > expected_ms)
        );
        terminal.draw(|frame| draw(frame, &app))?;
        let status = &terminal.backend().buffer().content()[240..320];
        let text: String = status.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("This week · Behind by"));
        assert_eq!(status[2].fg, Color::LightRed);

        app.store.add_study(today, "rust", 480)?;
        app.reload()?;
        terminal.draw(|frame| draw(frame, &app))?;
        let status = &terminal.backend().buffer().content()[240..320];
        let text: String = status.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("This week · Ahead by"));
        assert_eq!(status[2].fg, Color::Green);

        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn weekly_goal_can_be_configured_from_progress_and_survives_reopening() -> Result<()> {
        let directory = storage::tests::temporary_directory();
        let database = directory.join("kronos.sqlite3");
        let monday = week_start(Local::now().date_naive());
        let mut app = App::new(Store::create_new(&database)?)?;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.handle_key(key(KeyCode::Char('g')))?;
        assert!(matches!(app.goal_progress, GoalProgress::NoGoal));
        app.handle_key(key(KeyCode::Char('e')))?;
        app.goal_input = format!("{} 420", monday + Duration::days(1));
        assert!(app.handle_key(key(KeyCode::Enter)).is_err());
        assert!(app.screen == Screen::EditGoal);
        app.goal_input = format!("{monday} 420");
        app.handle_key(key(KeyCode::Enter))?;
        assert!(app.screen == Screen::GoalProgress);
        let mut terminal = Terminal::new(TestBackend::new(80, 16))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("Total studied"));
        assert!(rendered.contains("Expected through today"));
        assert!(rendered.contains("Behind by"));
        drop(app);

        let mut app = App::new(Store::open_existing(&database)?)?;
        app.handle_key(key(KeyCode::Char('g')))?;
        assert!(matches!(
            app.goal_progress,
            GoalProgress::Started {
                studied_ms: 0,
                current_goal: WeeklyGoal {
                    minutes_per_week: 420,
                    ..
                },
                ..
            }
        ));
        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn dashboard_can_start_and_stop_a_recovered_timer() -> Result<()> {
        let directory = storage::tests::temporary_directory();
        let database = directory.join("kronos.sqlite3");
        let mut store = Store::create_new(&database)?;
        store.add_topic("rust")?;
        let mut app = App::new(store)?;
        let timer_key = KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE);
        app.handle_key(timer_key)?;
        assert!(app.screen == Screen::TimerTopicSelect);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))?;
        assert!(app.screen == Screen::Dashboard);
        let id = app.active_timer.as_ref().unwrap().id;
        drop(app);

        let mut app = App::new(Store::open_existing(&database)?)?;
        assert_eq!(app.active_timer.as_ref().unwrap().id, id);
        assert!(app.screen == Screen::ResumeTimer);
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))?;
        assert!(app.screen == Screen::Dashboard);
        assert_eq!(app.active_timer.as_ref().unwrap().id, id);
        let mut terminal = Terminal::new(TestBackend::new(80, 16))?;
        terminal.draw(|frame| draw(frame, &app))?;
        let title = terminal.backend().buffer().content()[80..160]
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(title.contains("Timer running") && title.contains("rust"));
        assert_eq!(
            terminal.backend().buffer().content()[82].bg,
            Color::LightRed
        );

        app.handle_key(timer_key)?;
        assert!(app.active_timer.is_none());
        assert!(app.store.active_timer()?.is_none());

        app.store
            .start_timer("rust", Utc::now() - Duration::minutes(10))?;
        let before_discard = app.store.load_current()?;
        drop(app);
        let mut app = App::new(Store::open_existing(&database)?)?;
        assert!(app.screen == Screen::ResumeTimer);
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))?;
        assert!(app.screen == Screen::Dashboard);
        assert!(app.store.active_timer()?.is_none());
        assert_eq!(app.store.load_current()?, before_discard);

        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn study_menu_and_history_rank_topics_by_weekly_duration() -> Result<()> {
        let database = std::env::temp_dir().join(format!(
            "kronos-topic-order-{}-{}.sqlite3",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut store = Store::create_new(&database)?;
        let today = Local::now().date_naive();
        let last_week = today - Duration::weeks(1);
        for (topic, current, previous) in [
            ("algebra", 20, 40),
            ("biology", 20, 40),
            ("chemistry", 60, 10),
            ("design", 0, 90),
        ] {
            store.add_topic(topic)?;
            if current > 0 {
                store.add_study(today, topic, current)?;
            }
            store.add_study(last_week, topic, previous)?;
        }
        store.add_study(today - Duration::weeks(2), "algebra", 30)?;

        let mut app = App::new(store)?;
        let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
        let press = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));

        terminal.draw(|frame| draw(frame, &app))?;
        assert_topic_rows(&terminal, &["chemistry", "algebra", "biology"]);
        let dashboard: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!dashboard.contains("design"));

        press(&mut app, KeyCode::Char('a'))?;
        terminal.draw(|frame| draw(frame, &app))?;
        assert_topic_rows(&terminal, &["chemistry", "algebra", "biology", "design"]);

        press(&mut app, KeyCode::Enter)?;
        assert_eq!(app.study_date, today);
        press(&mut app, KeyCode::Up)?;
        assert_eq!(app.study_date, today);

        app.study_date = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
        press(&mut app, KeyCode::Up)?;
        assert_eq!(app.study_date, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
        press(&mut app, KeyCode::Down)?;
        assert_eq!(
            app.study_date,
            NaiveDate::from_ymd_opt(2023, 12, 31).unwrap()
        );
        press(&mut app, KeyCode::Right)?;
        press(&mut app, KeyCode::Up)?;
        assert_eq!(
            app.study_date,
            NaiveDate::from_ymd_opt(2024, 1, 31).unwrap()
        );
        press(&mut app, KeyCode::Up)?;
        assert_eq!(
            app.study_date,
            NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()
        );
        press(&mut app, KeyCode::Right)?;
        press(&mut app, KeyCode::Down)?;
        assert_eq!(
            app.study_date,
            NaiveDate::from_ymd_opt(2023, 2, 28).unwrap()
        );

        for key in [
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Enter,
        ] {
            press(&mut app, key)?;
        }
        assert_eq!(aggregate_current(&app.current)["chemistry"], 120 * 60_000);

        press(&mut app, KeyCode::Char('a'))?;
        for key in [KeyCode::Char('I'), KeyCode::Char('x'), KeyCode::Enter] {
            press(&mut app, key)?;
        }
        assert!(matches!(app.screen, Screen::TopicSelect));
        assert_eq!(app.filtered_topics().count(), 0);

        press(&mut app, KeyCode::Backspace)?;
        assert_eq!(
            app.filtered_topics().collect::<Vec<_>>(),
            ["chemistry", "biology", "design"]
        );
        terminal.draw(|frame| draw(frame, &app))?;
        assert_topic_rows(&terminal, &["chemistry", "biology", "design"]);
        for key in [
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::Up,
            KeyCode::Enter,
        ] {
            press(&mut app, key)?;
        }
        assert_eq!(aggregate_current(&app.current)["biology"], 80 * 60_000);

        press(&mut app, KeyCode::Char('h'))?;
        terminal.draw(|frame| draw(frame, &app))?;
        assert_topic_rows(&terminal, &["design", "algebra", "biology", "chemistry"]);

        press(&mut app, KeyCode::Right)?;
        assert_eq!(app.history_index, 0);
        press(&mut app, KeyCode::Left)?;
        assert_eq!(app.history_index, 1);
        press(&mut app, KeyCode::Left)?;
        assert_eq!(app.history_index, 1);
        press(&mut app, KeyCode::Right)?;
        assert_eq!(app.history_index, 0);

        drop(app);
        std::fs::remove_file(database)?;
        Ok(())
    }

    fn assert_topic_rows(terminal: &Terminal<TestBackend>, expected: &[&str]) {
        let buffer = terminal.backend().buffer();
        let visible_topics: Vec<_> = buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .filter_map(|row| {
                let text: String = row.iter().map(|cell| cell.symbol()).collect();
                expected.iter().copied().find(|topic| text.contains(topic))
            })
            .collect();
        assert_eq!(visible_topics, expected);
    }

    #[test]
    fn duration_step_cycles_through_one_five_and_fifteen_minutes() {
        assert_eq!(next_step(1), 5);
        assert_eq!(next_step(5), 15);
        assert_eq!(next_step(15), 1);
    }
}
