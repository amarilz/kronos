mod dashboard;
pub(crate) mod goals;
mod history;
pub(crate) mod setup;
mod study_entry;

use crate::{App, Screen};
use chrono::Utc;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
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
        Screen::Dashboard => dashboard::draw(frame, app, areas[1]),
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
        Screen::TopicSelect | Screen::TimerTopicSelect => {
            study_entry::draw_topic_select(frame, app, areas[1])
        }
        Screen::DateSelect => study_entry::draw_date_select(frame, app, areas[1]),
        Screen::Duration => study_entry::draw_duration(frame, app, areas[1]),
        Screen::AddTopic => study_entry::draw_add_topic(frame, &app.topic_input, areas[1]),
        Screen::History => history::draw(frame, app, areas[1]),
        Screen::GoalProgress => goals::draw_progress(frame, &app.goal_progress, areas[1]),
        Screen::EditGoal => goals::draw_editor(frame, &app.goal_input, areas[1]),
    }

    let help = help_text(app);
    let footer = Paragraph::new(help)
        .style(Style::default().fg(Color::Gray))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, areas[2]);
}

pub(crate) fn format_duration(duration_ms: u64) -> String {
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
