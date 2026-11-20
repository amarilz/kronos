mod dashboard;
pub(crate) mod goals;
mod history;
pub(crate) mod setup;
pub(crate) mod study_entry;

use crate::{App, ResumeAction, Screen};
use chrono::{DateTime, Local, Utc};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
    if app.screen != Screen::ResumeTimer
        && let Some(timer) = &app.active_timer
    {
        let seconds = timer.elapsed_seconds(Utc::now());
        let clock = format_duration(seconds * 1000);
        let mut lines = vec![
            Line::from(format!("Timer running · {}", timer.topic)),
            Line::from(clock),
            Line::default(),
            choice_line("m Stop", "q Quit", app.timer_quit),
        ];
        if !app.message.is_empty() {
            lines.push(Line::from(app.message.as_str()));
        }
        let timer_view = Paragraph::new(lines).centered().wrap(Wrap { trim: true });
        frame.render_widget(timer_view, frame.area());
        return;
    }

    let [title, content, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::from(!app.message.is_empty())),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("Kronos").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        title,
    );

    match app.screen {
        Screen::Dashboard => dashboard::draw(frame, app, content),
        Screen::ResumeTimer => {
            if let Some(timer) = &app.active_timer {
                let now = Utc::now();
                let local_time = |time: DateTime<Utc>| {
                    time.with_timezone(&Local)
                        .format("%Y-%m-%d %H:%M:%S")
                        .to_string()
                };
                let mut lines = vec![Line::from(format!("Study timer for \"{}\"", timer.topic))];
                if let Some(start) = DateTime::from_timestamp_millis(timer.started_at) {
                    lines.push(Line::from(format!("Started at {}", local_time(start))));
                }
                if let Some(last_seen) = timer.last_seen() {
                    lines.push(Line::from(format!(
                        "Last checkpoint at {} · {} elapsed",
                        local_time(last_seen),
                        format_duration(timer.elapsed_ms(last_seen)),
                    )));
                } else {
                    lines.push(Line::from("No previous checkpoint available."));
                }
                lines.push(Line::from(format!(
                    "Now {} · {} elapsed",
                    local_time(now),
                    format_duration(timer.elapsed_seconds(now) * 1000),
                )));
                lines.push(Line::from(
                    "Continuing includes time while Kronos was closed.",
                ));
                lines.push(Line::default());
                let mut choices = Vec::new();
                for (index, action) in app.resume_actions().iter().enumerate() {
                    if index > 0 {
                        choices.push(Span::raw("   "));
                    }
                    let label = match action {
                        ResumeAction::Continue => "y Continue",
                        ResumeAction::Save => "s Save checkpoint",
                        ResumeAction::Discard => "n Discard",
                    };
                    let style = if index == app.resume_selection {
                        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    choices.push(Span::styled(label, style));
                }
                lines.push(Line::from(choices));
                if !app.message.is_empty() {
                    lines.push(Line::from(app.message.as_str()));
                }
                let prompt = Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .block(Block::bordered().title("Resume study timer"));
                frame.render_widget(prompt, content);
            }
        }
        Screen::TopicSelect | Screen::TimerTopicSelect => {
            study_entry::draw_topic_select(frame, app, content)
        }
        Screen::DateSelect => study_entry::draw_date_select(frame, app, content),
        Screen::Duration => study_entry::draw_duration(frame, app, content),
        Screen::AddTopic => study_entry::draw_add_topic(frame, app, content),
        Screen::History => history::draw(frame, app, content),
        Screen::GoalProgress => goals::draw_progress(frame, &app.goal_progress, content),
        Screen::EditGoal => goals::draw_editor(frame, &app.goal_input, content),
    }

    if !app.message.is_empty() {
        frame.render_widget(
            Paragraph::new(app.message.as_str()).style(Style::default().fg(Color::Yellow)),
            status,
        );
    }
    frame.render_widget(
        Paragraph::new(help_text(app.screen))
            .style(Style::default().fg(Color::Gray))
            .wrap(Wrap { trim: true }),
        footer,
    );
}

fn choice_line<'a>(left: &'a str, right: &'a str, right_selected: bool) -> Line<'a> {
    let selected = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let normal = Style::default();
    Line::from(vec![
        Span::styled(left, if right_selected { normal } else { selected }),
        Span::raw("   "),
        Span::styled(right, if right_selected { selected } else { normal }),
    ])
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

fn help_text(screen: Screen) -> &'static str {
    match screen {
        Screen::ResumeTimer => "←→ Select · Enter Confirm · Esc Quit",
        Screen::Dashboard => "↑↓ Select · Enter Open · PgUp/PgDn Scroll study time",
        Screen::TimerTopicSelect => "Type to search · ↑↓ Select · Enter Start or create · Esc Back",
        Screen::TopicSelect => "Type to search · ↑↓ Select · Enter Open · Esc Back",
        Screen::DateSelect => "←→ Day, month, year · ↑↓ Adjust · Enter Apply · Esc Cancel",
        Screen::AddTopic => "Enter Confirm · Esc Back",
        Screen::Duration => "Tab Change field · Enter Confirm · Esc Back",
        Screen::History => "← Older week · → Newer week · Esc Back",
        Screen::GoalProgress => "Enter Set weekly goal · Esc Back",
        Screen::EditGoal => "Enter Save · Esc Back",
    }
}
