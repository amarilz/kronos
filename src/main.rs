mod config;
mod model;
mod setup;
mod storage;

use anyhow::{Context, Result, bail};
use chrono::{Local, Months, NaiveDate};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use model::{
    CurrentEntry, HistoryEntry, aggregate_current, aggregate_history, iso_week_key, week_start,
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
use storage::Store;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Screen {
    Dashboard,
    TopicSelect,
    DateSelect,
    Duration,
    AddTopic,
    History,
}

struct App {
    store: Store,
    topics: Vec<String>,
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
    message: String,
    quit: bool,
}

impl App {
    fn new(store: Store) -> Result<Self> {
        let today = Local::now().date_naive();
        let mut app = Self {
            store,
            topics: Vec::new(),
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
            message: String::new(),
            quit: false,
        };
        app.reload()?;
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        let today = Local::now().date_naive();
        self.store.rollover_if_needed(today)?;
        self.topics = self.store.topics()?;
        self.current = self.store.load_current()?;
        let totals = aggregate_current(&self.current);
        // Stable sorting keeps the store's alphabetical order for equal totals.
        self.topics
            .sort_by_key(|topic| Reverse(totals.get(topic).copied().unwrap_or(0)));
        self.history = self.store.load_history()?;
        self.history_weeks = self
            .history
            .iter()
            .map(|entry| (entry.iso_year, entry.iso_week))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .rev()
            .collect();
        self.selected = self.selected.min(self.topics.len().saturating_sub(1));
        self.history_index = self
            .history_index
            .min(self.history_weeks.len().saturating_sub(1));
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        self.message.clear();
        match self.screen {
            Screen::Dashboard => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
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
                KeyCode::Char('h') => {
                    self.history_index = 0;
                    self.screen = Screen::History;
                }
                KeyCode::Down => self.move_selection(1, self.topics.len()),
                KeyCode::Up => self.move_selection(-1, self.topics.len()),
                _ => {}
            },
            Screen::TopicSelect => match key.code {
                KeyCode::Esc => self.screen = Screen::Dashboard,
                KeyCode::Up => self.move_selection(-1, self.filtered_topics().count()),
                KeyCode::Down => self.move_selection(1, self.filtered_topics().count()),
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
            Screen::History => match key.code {
                KeyCode::Esc | KeyCode::Char('h') => self.screen = Screen::Dashboard,
                KeyCode::Left => self.move_history(1),
                KeyCode::Right => self.move_history(-1),
                _ => {}
            },
        }
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

    let title = Paragraph::new(concat!(" kronos v", env!("CARGO_PKG_VERSION"), " "))
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, areas[0]);

    match app.screen {
        Screen::Dashboard => draw_dashboard(frame, app, areas[1]),
        Screen::TopicSelect => draw_topic_select(frame, app, areas[1]),
        Screen::DateSelect => draw_date_select(frame, app, areas[1]),
        Screen::Duration => draw_duration(frame, app, areas[1]),
        Screen::AddTopic => draw_input(
            frame,
            "New topic · Enter to save",
            &app.topic_input,
            areas[1],
        ),
        Screen::History => draw_history(frame, app, areas[1]),
    }

    let help = help_text(app);
    let footer = Paragraph::new(help)
        .style(Style::default().fg(Color::Gray))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, areas[2]);
}

fn draw_dashboard(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let today = Local::now().date_naive();
    let start = week_start(today);
    let (iso_year, iso_week) = iso_week_key(today);
    let totals = aggregate_current(&app.current);
    let total: u32 = totals.values().sum();
    let rows = app
        .topics
        .iter()
        .map(|topic| {
            let minutes = totals.get(topic).copied().unwrap_or_default();
            ListItem::new(format!(
                "  {topic:<20} {:>3}h {:02}m",
                minutes / 60,
                minutes % 60
            ))
        })
        .collect::<Vec<_>>();
    let rows = if rows.is_empty() {
        vec![ListItem::new("  No topics yet: press `t` to add one.")]
    } else {
        rows
    };
    let list = List::new(rows)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " ISO week {iso_year}-W{iso_week:02} · Monday {start} · Total {}h {:02}m ",
            total / 60,
            total % 60
        )))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default();
    if !app.topics.is_empty() {
        state.select(Some(app.selected.min(app.topics.len() - 1)));
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

    let topics: Vec<_> = app.filtered_topics().collect();
    if topics.is_empty() {
        let message = if app.topics.is_empty() {
            "No topics yet."
        } else {
            "No matching topics."
        };
        frame.render_widget(
            Paragraph::new(message).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Select a topic"),
            ),
            results,
        );
    } else {
        draw_list(frame, "Select a topic", &topics, app.selected, results);
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
        Screen::Dashboard => {
            if app.message.is_empty() {
                " a Add study time   t Add topic   h History   q Quit ".to_owned()
            } else {
                format!(" {} ", app.message)
            }
        }
        Screen::TopicSelect => " Type to filter   ↑↓ Navigate   Enter Confirm   Esc Back ".into(),
        Screen::DateSelect => " ←→ Day/Month/Year   ↑↓ Adjust   Enter Confirm   Esc Back ".into(),
        Screen::AddTopic => " Type   Backspace Delete   Enter Confirm   Esc Cancel ".into(),
        Screen::Duration => {
            " ←→ Field   ↑↓ Adjust   s Step 1/5/15m   Enter Save   Esc Back ".into()
        }
        Screen::History => " ←→ Previous/next week   Esc Back to dashboard ".into(),
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
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;

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
        assert_eq!(aggregate_current(&app.current)["chemistry"], 120);

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
        assert_eq!(aggregate_current(&app.current)["biology"], 80);

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
