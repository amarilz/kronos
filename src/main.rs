mod config;
mod model;
mod storage;
mod ui;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, Months, NaiveDate, Utc};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use model::{
    CurrentEntry, GoalProgress, HistoryEntry, WeeklyGoal, aggregate_current, history_weeks,
    summarize_goals, week_start,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    cmp::Reverse,
    io::{self, Stdout},
    path::PathBuf,
    time::{Duration as StdDuration, Instant},
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

const HOME_ACTIONS: [(&str, char); 5] = [
    ("Start timer", 'm'),
    ("Log time", 'a'),
    ("History", 'h'),
    ("Goals", 'g'),
    ("Quit", 'q'),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResumeAction {
    Continue,
    Save,
    Discard,
}

struct App {
    store: Store,
    active_timer: Option<ActiveTimer>,
    topics: Vec<String>,
    studied_topic_count: usize,
    current: Vec<CurrentEntry>,
    history: Vec<HistoryEntry>,
    pending_history: Vec<CurrentEntry>,
    weekly_goals: Vec<WeeklyGoal>,
    screen: Screen,
    selected: usize,
    home_selection: usize,
    dashboard_scroll: usize,
    resume_selection: usize,
    timer_quit: bool,
    topic_creation_origin: Screen,
    study_topic: String,
    date_draft: NaiveDate,
    date_focused: bool,
    duration_input: String,
    date_field: usize,
    study_date: NaiveDate,
    topic_input: String,
    topic_filter: String,
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
            pending_history: Vec::new(),
            weekly_goals: Vec::new(),
            screen: Screen::Dashboard,
            selected: 0,
            home_selection: 0,
            dashboard_scroll: 0,
            resume_selection: 0,
            timer_quit: false,
            topic_creation_origin: Screen::Dashboard,
            study_topic: String::new(),
            date_draft: today,
            date_focused: false,
            duration_input: String::new(),
            date_field: 0,
            study_date: today,
            topic_input: String::new(),
            topic_filter: String::new(),
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

    fn resume_actions(&self) -> &[ResumeAction] {
        if self
            .active_timer
            .as_ref()
            .and_then(ActiveTimer::last_seen)
            .is_some()
        {
            &[
                ResumeAction::Continue,
                ResumeAction::Save,
                ResumeAction::Discard,
            ]
        } else {
            &[ResumeAction::Continue, ResumeAction::Discard]
        }
    }

    fn checkpoint_timer(&self, now: DateTime<Utc>) -> Result<()> {
        if self.screen != Screen::ResumeTimer
            && let Some(timer) = &self.active_timer
        {
            self.store.checkpoint_timer(timer.id, now)?;
        }
        Ok(())
    }

    fn save_timer(&mut self, id: i64, ended_at: DateTime<Utc>) -> Result<()> {
        let result = self.store.stop_timer(id, ended_at)?;
        self.reload()?;
        if self.active_timer.is_none() {
            self.screen = Screen::Dashboard;
        } else {
            self.screen = Screen::ResumeTimer;
            self.resume_selection = 0;
        }
        self.message = match result {
            Some(duration_ms) => {
                format!("Timer saved · {}", ui::format_duration(duration_ms as u64))
            }
            None => "This timer was already stopped".into(),
        };
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let today = Local::now().date_naive();
        self.store.rollover_if_needed(today)?;
        self.active_timer = self.store.active_timer()?;
        self.topics = self.store.topics()?;
        let (daily_entries, history, weekly_goals) = self.store.load_study_snapshot()?;
        self.goal_progress = summarize_goals(today, &weekly_goals, &daily_entries, &history);
        let monday = week_start(today);
        (self.current, self.pending_history) = daily_entries
            .into_iter()
            .partition(|entry| entry.date >= monday);
        self.history = history;
        self.weekly_goals = weekly_goals;
        let totals = aggregate_current(&self.current);
        self.studied_topic_count = totals
            .values()
            .filter(|&&duration_ms| duration_ms > 0)
            .count();
        // Stable sorting keeps the store's alphabetical order for equal totals.
        self.topics
            .sort_by_key(|topic| Reverse(totals.get(topic).copied().unwrap_or(0)));
        self.snapshot_date = today;
        self.history_weeks = history_weeks(today, &self.history, &self.pending_history);
        self.history_index = self
            .history_index
            .min(self.history_weeks.len().saturating_sub(1));
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release
            || (key.code == KeyCode::Enter && key.kind != KeyEventKind::Press)
        {
            return Ok(());
        }
        self.message.clear();
        let mut key = key;
        if self.screen == Screen::Dashboard
            && self.active_timer.is_none()
            && key.code == KeyCode::Enter
        {
            key.code = KeyCode::Char(HOME_ACTIONS[self.home_selection].1);
        }
        if self.screen != Screen::ResumeTimer
            && let Some(timer) = &self.active_timer
        {
            match key.code {
                KeyCode::Left | KeyCode::Up => self.timer_quit = false,
                KeyCode::Right | KeyCode::Down => self.timer_quit = true,
                KeyCode::Tab | KeyCode::BackTab => self.timer_quit = !self.timer_quit,
                KeyCode::Enter if self.timer_quit => self.quit = true,
                KeyCode::Enter | KeyCode::Char('m') if key.kind == KeyEventKind::Press => {
                    self.save_timer(timer.id, Utc::now())?;
                }
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                _ => {}
            }
            return Ok(());
        }

        match self.screen {
            Screen::ResumeTimer => match key.code {
                KeyCode::Left | KeyCode::Up | KeyCode::BackTab => {
                    let count = self.resume_actions().len();
                    self.resume_selection = (self.resume_selection + count - 1) % count;
                }
                KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                    self.resume_selection =
                        (self.resume_selection + 1) % self.resume_actions().len();
                }
                KeyCode::Enter | KeyCode::Char('y' | 'Y' | 's' | 'S' | 'n' | 'N')
                    if key.kind == KeyEventKind::Press =>
                {
                    if let Some(timer) = &self.active_timer {
                        let action = match key.code {
                            KeyCode::Char('y' | 'Y') => ResumeAction::Continue,
                            KeyCode::Char('s' | 'S') => ResumeAction::Save,
                            KeyCode::Char('n' | 'N') => ResumeAction::Discard,
                            _ => self.resume_actions()[self.resume_selection],
                        };
                        match action {
                            ResumeAction::Save => {
                                if let Some(ended_at) = timer.last_seen() {
                                    self.save_timer(timer.id, ended_at)?;
                                }
                            }
                            ResumeAction::Continue | ResumeAction::Discard => {
                                let id = timer.id;
                                if action == ResumeAction::Continue {
                                    self.store.checkpoint_timer(id, Utc::now())?;
                                } else {
                                    self.store.discard_timer(id)?;
                                }
                                self.reload()?;
                                self.resume_selection = 0;
                                if self.active_timer.is_none()
                                    || (action == ResumeAction::Continue
                                        && self.active_timer.as_ref().map(|timer| timer.id)
                                            == Some(id))
                                {
                                    self.screen = Screen::Dashboard;
                                }
                            }
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Screen::Dashboard => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
                KeyCode::Char('m') if key.kind == KeyEventKind::Press => {
                    self.reload()?;
                    if self.active_timer.is_none() {
                        self.topic_filter.clear();
                        self.selected = 0;
                        self.screen = Screen::TimerTopicSelect;
                    }
                }
                KeyCode::Char('a') => {
                    self.reload()?;
                    self.topic_filter.clear();
                    self.selected = 0;
                    self.screen = Screen::TopicSelect;
                }
                KeyCode::Char('t') => {
                    self.open_topic_creation(Screen::Dashboard);
                }
                KeyCode::Char('g') => self.open_goal_progress()?,
                KeyCode::Char('h') => {
                    self.reload()?;
                    self.history_index = 0;
                    self.screen = Screen::History;
                }
                KeyCode::PageUp => self.dashboard_scroll = self.dashboard_scroll.saturating_sub(1),
                KeyCode::PageDown => {
                    self.dashboard_scroll =
                        (self.dashboard_scroll + 1).min(self.studied_topic_count.saturating_sub(1));
                }
                KeyCode::Down | KeyCode::Tab => {
                    self.home_selection = (self.home_selection + 1) % HOME_ACTIONS.len();
                }
                KeyCode::Up | KeyCode::BackTab => {
                    self.home_selection =
                        (self.home_selection + HOME_ACTIONS.len() - 1) % HOME_ACTIONS.len();
                }
                _ => {}
            },
            Screen::TopicSelect | Screen::TimerTopicSelect => match key.code {
                KeyCode::Esc => self.screen = Screen::Dashboard,
                KeyCode::Up => self.move_selection(-1, self.filtered_topics().count() + 1),
                KeyCode::Down => self.move_selection(1, self.filtered_topics().count() + 1),
                KeyCode::Enter => {
                    let topic = self.filtered_topics().nth(self.selected).map(str::to_owned);
                    if let Some(topic) = topic {
                        self.continue_with_topic(self.screen, topic)?;
                    } else {
                        self.open_topic_creation(self.screen);
                    }
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
                KeyCode::Esc => self.screen = Screen::Duration,
                KeyCode::Left => self.date_field = self.date_field.saturating_sub(1),
                KeyCode::Right => self.date_field = (self.date_field + 1).min(2),
                KeyCode::Up => self.adjust_study_date(true),
                KeyCode::Down => self.adjust_study_date(false),
                KeyCode::Enter => {
                    self.study_date = self.date_draft;
                    self.screen = Screen::Duration;
                }
                _ => {}
            },
            Screen::Duration => match key.code {
                KeyCode::Esc => self.screen = Screen::TopicSelect,
                KeyCode::Tab | KeyCode::BackTab => self.date_focused = !self.date_focused,
                KeyCode::Up => self.date_focused = false,
                KeyCode::Down => self.date_focused = true,
                KeyCode::Enter if self.date_focused => {
                    self.date_draft = self.study_date;
                    self.date_field = 0;
                    self.screen = Screen::DateSelect;
                }
                KeyCode::Enter => {
                    let total = ui::study_entry::parse_duration(&self.duration_input)?;
                    self.store
                        .add_study(self.study_date, &self.study_topic, total)?;
                    self.reload()?;
                    self.message = format!(
                        "Logged {} for {} on {}",
                        ui::format_duration(u64::from(total) * 60_000),
                        self.study_topic,
                        self.study_date,
                    );
                    self.screen = Screen::Dashboard;
                }
                KeyCode::Backspace if !self.date_focused => {
                    self.duration_input.pop();
                }
                KeyCode::Char(ch) if !self.date_focused && !ch.is_control() => {
                    self.duration_input.push(ch);
                }
                _ => {}
            },
            Screen::AddTopic => match key.code {
                KeyCode::Esc => self.screen = self.topic_creation_origin,
                KeyCode::Backspace => {
                    self.topic_input.pop();
                }
                KeyCode::Enter => {
                    let topic = self.topic_input.trim().to_lowercase();
                    self.store.add_topic(&topic)?;
                    let origin = self.topic_creation_origin;
                    self.screen = origin;
                    self.reload()?;
                    if origin == Screen::Dashboard {
                        self.message = format!("Topic \"{topic}\" added");
                    } else {
                        self.topic_filter = topic.clone();
                        let selected = self.filtered_topics().position(|item| item == topic);
                        self.selected = selected.unwrap_or(0);
                        self.continue_with_topic(origin, topic)?;
                    }
                }
                KeyCode::Char(ch) if !ch.is_control() => self.topic_input.push(ch),
                _ => {}
            },
            Screen::GoalProgress => match key.code {
                KeyCode::Esc | KeyCode::Char('g') => self.screen = Screen::Dashboard,
                KeyCode::Enter | KeyCode::Char('e') => {
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
                    self.store
                        .save_weekly_goal(ui::goals::parse_goal(&self.goal_input)?)?;
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
        self.reload()?;
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

    fn open_topic_creation(&mut self, origin: Screen) {
        self.topic_creation_origin = origin;
        self.topic_input = if origin == Screen::Dashboard {
            String::new()
        } else {
            self.topic_filter.clone()
        };
        self.screen = Screen::AddTopic;
    }

    fn continue_with_topic(&mut self, origin: Screen, topic: String) -> Result<()> {
        match origin {
            Screen::TimerTopicSelect => {
                let result = self.store.start_timer(&topic, Utc::now());
                self.reload()?;
                if self.active_timer.is_some() {
                    self.timer_quit = false;
                    self.screen = Screen::Dashboard;
                }
                result?;
            }
            Screen::TopicSelect => {
                self.study_topic = topic;
                self.study_date = Local::now().date_naive();
                self.date_focused = false;
                self.duration_input.clear();
                self.screen = Screen::Duration;
            }
            _ => unreachable!("Only study workflows select topics"),
        }
        Ok(())
    }

    fn move_selection(&mut self, direction: isize, length: usize) {
        if length == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + direction).rem_euclid(length as isize) as usize;
    }

    fn adjust_study_date(&mut self, forward: bool) {
        let date = self.date_draft;
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
            self.date_draft = adjusted;
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
        return ui::setup::run(&Store::default_database_path()?, None);
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
            Err(error) => ui::setup::run(
                &path,
                Some(format!("The saved database could not be opened: {error}")),
            ),
        },
        None => ui::setup::run(&Store::default_database_path()?, None),
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
    let mut last_checkpoint = Instant::now();
    while !app.quit {
        if last_checkpoint.elapsed() >= StdDuration::from_secs(5) {
            last_checkpoint = Instant::now();
            if let Err(error) = app.checkpoint_timer(Utc::now()) {
                app.message = format!("Could not save timer checkpoint · {error}");
            }
        }
        if matches!(app.screen, Screen::Dashboard | Screen::GoalProgress)
            && app.snapshot_date != Local::now().date_naive()
        {
            app.reload()?;
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        if event::poll(StdDuration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && let Err(error) = app.handle_key(key)
        {
            app.message = error.to_string();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use crossterm::event::KeyModifiers;
    use ratatui::{backend::TestBackend, style::Color};

    #[test]
    fn history_shows_each_weeks_goal_and_colors_its_balance() -> Result<()> {
        let directory = storage::tests::temporary_directory();
        let mut store = Store::create_new(&directory.join("kronos.sqlite3"))?;
        let monday = week_start(Local::now().date_naive());
        let last_week = monday - Duration::weeks(1);
        let first_week = monday - Duration::weeks(3);
        store.add_topic("rust")?;
        store.save_weekly_goal(WeeklyGoal {
            starts_on: first_week,
            minutes_per_week: 300,
        })?;
        store.save_weekly_goal(WeeklyGoal {
            starts_on: last_week,
            minutes_per_week: 420,
        })?;
        store.add_study(first_week, "rust", 330)?;
        store.add_study(last_week, "rust", 60)?;
        let mut app = App::new(store)?;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        app.handle_key(key(KeyCode::Char('h')))?;
        assert_eq!(
            app.history_weeks,
            [
                model::iso_week_key(last_week),
                model::iso_week_key(first_week)
            ]
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 16))?;

        for (goal, balance, color) in [
            ("7h 00m", "Behind by 6h 00m", Color::LightRed),
            ("5h 00m", "Ahead by 0h 30m", Color::Green),
        ] {
            terminal.draw(|frame| ui::draw(frame, &app))?;
            let buffer = terminal.backend().buffer().content();
            let goal_row: String = buffer[240..320].iter().map(|cell| cell.symbol()).collect();
            let balance_row: String = buffer[320..400].iter().map(|cell| cell.symbol()).collect();
            assert!(goal_row.contains(&format!("Weekly goal  {goal}")));
            assert!(balance_row.contains(balance));
            assert_eq!(buffer[321].fg, color);
            app.handle_key(key(KeyCode::Left))?;
        }

        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

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
        terminal.draw(|frame| ui::draw(frame, &app))?;
        let row_start = (HOME_ACTIONS.len() + 1) * 80;
        let status = &terminal.backend().buffer().content()[row_start..row_start + 80];
        let text: String = status.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("This week · Behind by"));
        assert_eq!(status[2].fg, Color::LightRed);

        app.store.add_study(today, "rust", 480)?;
        app.reload()?;
        terminal.draw(|frame| ui::draw(frame, &app))?;
        let status = &terminal.backend().buffer().content()[row_start..row_start + 80];
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
        terminal.draw(|frame| ui::draw(frame, &app))?;
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
    fn recovered_timer_saves_the_last_checkpoint_without_counting_time_away() -> Result<()> {
        let directory = storage::tests::temporary_directory();
        let database = directory.join("kronos.sqlite3");
        let mut store = Store::create_new(&database)?;
        store.add_topic("rust")?;
        let started_at = Utc::now() - Duration::seconds(12);
        let checkpoint = started_at + Duration::milliseconds(5_123);
        let press = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        store.start_timer("rust", started_at)?;
        let timer = store.active_timer()?.unwrap();
        store.checkpoint_timer(timer.id, checkpoint)?;
        drop(store);

        let mut app = App::new(Store::open_existing(&database)?)?;
        assert!(app.screen == Screen::ResumeTimer);
        press(&mut app, KeyCode::Char('q'))?;
        drop(app);

        let mut app = App::new(Store::open_existing(&database)?)?;
        let timer = app.active_timer.as_ref().unwrap();
        assert_eq!(
            timer.last_seen(),
            DateTime::from_timestamp_millis(checkpoint.timestamp_millis())
        );
        press(&mut app, KeyCode::Down)?;
        press(&mut app, KeyCode::Enter)?;
        assert!(app.screen == Screen::Dashboard);
        assert!(app.active_timer.is_none());
        assert_eq!(app.message, "Timer saved · 00:00:05.123");

        drop(app);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn dashboard_can_start_and_stop_a_recovered_timer() -> Result<()> {
        let directory = storage::tests::temporary_directory();
        let database = directory.join("kronos.sqlite3");
        let store = Store::create_new(&database)?;
        let mut app = App::new(store)?;
        let press = |app: &mut App, code| app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        press(&mut app, KeyCode::Enter)?;
        assert!(app.screen == Screen::TimerTopicSelect);
        press(&mut app, KeyCode::Enter)?;
        assert!(app.screen == Screen::AddTopic);
        press(&mut app, KeyCode::Esc)?;
        assert!(app.screen == Screen::TimerTopicSelect);
        for ch in "Rust".chars() {
            press(&mut app, KeyCode::Char(ch))?;
        }
        press(&mut app, KeyCode::Enter)?;
        assert_eq!(app.topic_input, "Rust");
        press(&mut app, KeyCode::Enter)?;
        assert!(app.screen == Screen::Dashboard);
        let id = app.active_timer.as_ref().unwrap().id;
        press(&mut app, KeyCode::Right)?;
        press(&mut app, KeyCode::Enter)?;
        assert!(app.quit);
        drop(app);

        let mut app = App::new(Store::open_existing(&database)?)?;
        assert_eq!(app.active_timer.as_ref().unwrap().id, id);
        assert!(app.screen == Screen::ResumeTimer);
        let mut terminal = Terminal::new(TestBackend::new(80, 16))?;
        terminal.draw(|frame| ui::draw(frame, &app))?;
        let prompt: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(prompt.contains("Last checkpoint at"));
        let choice_rows = terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .filter(|row| ["[y]", "[s]", "[n]"].iter().any(|key| row.contains(key)))
            .count();
        assert_eq!(choice_rows, 3);
        assert!(prompt.contains("> [y] Continue timer"));
        assert!(prompt.contains("[s] Save and stop"));
        assert!(prompt.contains("[n] Discard session"));
        assert!(prompt.contains("Esc Quit"));

        press(&mut app, KeyCode::Enter)?;
        assert!(app.screen == Screen::Dashboard);
        assert_eq!(app.active_timer.as_ref().unwrap().id, id);
        terminal.draw(|frame| ui::draw(frame, &app))?;
        let rows: Vec<_> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .map(|row| row.trim().to_owned())
            .filter(|row| !row.is_empty())
            .collect();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].starts_with("Timer running · rust"));
        assert_eq!(rows[1].split(':').count(), 3);
        assert_eq!(rows[2], "m Stop   q Quit");

        for shortcut in ['a', 't', 'h', 'g', 'n'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::NONE))?;
            assert!(app.screen == Screen::Dashboard);
            assert_eq!(app.active_timer.as_ref().unwrap().id, id);
        }

        press(&mut app, KeyCode::Enter)?;
        assert!(app.active_timer.is_none());
        assert!(app.store.active_timer()?.is_none());

        app.store
            .start_timer("rust", Utc::now() - Duration::minutes(10))?;
        let before_discard = app.store.load_current()?;
        drop(app);
        let mut app = App::new(Store::open_existing(&database)?)?;
        assert!(app.screen == Screen::ResumeTimer);
        press(&mut app, KeyCode::Left)?;
        press(&mut app, KeyCode::Enter)?;
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

        terminal.draw(|frame| ui::draw(frame, &app))?;
        assert_topic_rows(&terminal, &["chemistry", "algebra", "biology"]);
        let dashboard: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!dashboard.contains("design"));

        press(&mut app, KeyCode::Down)?;
        press(&mut app, KeyCode::Enter)?;
        terminal.draw(|frame| ui::draw(frame, &app))?;
        assert_topic_rows(&terminal, &["chemistry", "algebra", "biology", "design"]);

        press(&mut app, KeyCode::Enter)?;
        assert_eq!(app.study_date, today);
        assert!(app.screen == Screen::Duration);
        assert!(press(&mut app, KeyCode::Enter).is_err());
        for ch in "1h 30m".chars() {
            press(&mut app, KeyCode::Char(ch))?;
        }
        press(&mut app, KeyCode::Tab)?;
        press(&mut app, KeyCode::Enter)?;
        assert!(app.screen == Screen::DateSelect);
        press(&mut app, KeyCode::Up)?;
        assert_eq!(app.date_draft, today);

        app.date_draft = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
        press(&mut app, KeyCode::Up)?;
        assert_eq!(app.date_draft, NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
        press(&mut app, KeyCode::Down)?;
        assert_eq!(
            app.date_draft,
            NaiveDate::from_ymd_opt(2023, 12, 31).unwrap()
        );
        press(&mut app, KeyCode::Right)?;
        press(&mut app, KeyCode::Up)?;
        assert_eq!(
            app.date_draft,
            NaiveDate::from_ymd_opt(2024, 1, 31).unwrap()
        );
        press(&mut app, KeyCode::Up)?;
        assert_eq!(
            app.date_draft,
            NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()
        );
        press(&mut app, KeyCode::Right)?;
        press(&mut app, KeyCode::Down)?;
        assert_eq!(
            app.date_draft,
            NaiveDate::from_ymd_opt(2023, 2, 28).unwrap()
        );

        press(&mut app, KeyCode::Esc)?;
        assert_eq!(app.study_date, today);
        press(&mut app, KeyCode::Tab)?;
        press(&mut app, KeyCode::Enter)?;
        assert_eq!(aggregate_current(&app.current)["chemistry"], 150 * 60_000);

        press(&mut app, KeyCode::Char('a'))?;
        for key in [KeyCode::Char('I'), KeyCode::Char('x')] {
            press(&mut app, key)?;
        }
        assert!(matches!(app.screen, Screen::TopicSelect));
        assert_eq!(app.filtered_topics().count(), 0);

        press(&mut app, KeyCode::Backspace)?;
        assert_eq!(
            app.filtered_topics().collect::<Vec<_>>(),
            ["chemistry", "biology", "design"]
        );
        terminal.draw(|frame| ui::draw(frame, &app))?;
        assert_topic_rows(&terminal, &["chemistry", "biology", "design"]);
        press(&mut app, KeyCode::Down)?;
        press(&mut app, KeyCode::Enter)?;
        for ch in "60".chars() {
            press(&mut app, KeyCode::Char(ch))?;
        }
        press(&mut app, KeyCode::Enter)?;
        assert_eq!(aggregate_current(&app.current)["biology"], 80 * 60_000);

        press(&mut app, KeyCode::Char('h'))?;
        terminal.draw(|frame| ui::draw(frame, &app))?;
        assert_topic_rows(&terminal, &["design", "algebra", "biology", "chemistry"]);

        press(&mut app, KeyCode::Right)?;
        assert_eq!(app.history_index, 0);
        press(&mut app, KeyCode::Left)?;
        assert_eq!(app.history_index, 1);
        press(&mut app, KeyCode::Left)?;
        assert_eq!(app.history_index, 1);
        press(&mut app, KeyCode::Right)?;
        assert_eq!(app.history_index, 0);

        press(&mut app, KeyCode::Esc)?;
        press(&mut app, KeyCode::Char('a'))?;
        for ch in "physics".chars() {
            press(&mut app, KeyCode::Char(ch))?;
        }
        press(&mut app, KeyCode::Enter)?;
        press(&mut app, KeyCode::Enter)?;
        assert_eq!(app.study_topic, "physics");
        for ch in "45m".chars() {
            press(&mut app, KeyCode::Char(ch))?;
        }
        press(&mut app, KeyCode::Tab)?;
        press(&mut app, KeyCode::Enter)?;
        press(&mut app, KeyCode::Down)?;
        press(&mut app, KeyCode::Enter)?;
        assert_eq!(app.study_date, today - Duration::days(1));
        assert_eq!(app.duration_input, "45m");
        press(&mut app, KeyCode::Tab)?;
        press(&mut app, KeyCode::Enter)?;
        let (entries, _, _) = app.store.load_study_snapshot()?;
        assert!(
            entries.iter().any(|entry| entry.topic == "physics"
                && entry.date == today - Duration::days(1)
                && entry.duration_ms == 45 * 60_000)
                || app.history.iter().any(|entry| entry.topic == "physics"
                    && entry.minutes == 45
                    && (entry.iso_year, entry.iso_week)
                        == model::iso_week_key(today - Duration::days(1)))
        );

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
}
