use super::{format_duration, goals};
use crate::{
    App,
    model::{HistoryEntry, aggregate_history, iso_week_key},
};
use chrono::{NaiveDate, Weekday};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::cmp::Reverse;

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(&(iso_year, iso_week)) = app.history_weeks.get(app.history_index) else {
        frame.render_widget(
            Paragraph::new("No history available.")
                .block(Block::default().borders(Borders::ALL).title("History")),
            area,
        );
        return;
    };
    let entries: Vec<HistoryEntry> = app
        .history
        .iter()
        .filter(|entry| entry.iso_year == iso_year && entry.iso_week == iso_week)
        .cloned()
        .collect();
    let mut totals: std::collections::BTreeMap<_, _> = aggregate_history(&entries)
        .into_iter()
        .map(|(topic, minutes)| (topic, u64::from(minutes) * 60_000))
        .collect();
    for entry in app
        .pending_history
        .iter()
        .filter(|entry| iso_week_key(entry.date) == (iso_year, iso_week))
    {
        *totals.entry(entry.topic.clone()).or_default() += entry.duration_ms;
    }
    let studied_ms: u64 = totals.values().sum();
    let mut ranked_topics: Vec<_> = totals.into_iter().collect();
    ranked_topics.sort_by_key(|(_, duration_ms)| Reverse(*duration_ms));
    let mut content = vec![Line::from(format!(
        "ISO week {iso_year}-W{iso_week:02} · Total {}",
        study_duration(studied_ms)
    ))];
    if let Some(monday) = NaiveDate::from_isoywd_opt(iso_year, iso_week, Weekday::Mon)
        && let Some(goal) = app
            .weekly_goals
            .iter()
            .rev()
            .find(|goal| goal.starts_on <= monday)
    {
        let expected_ms = goal.expected_ms_for_days(7);
        content.push(Line::from(format!(
            "Weekly goal  {}",
            study_duration(expected_ms)
        )));
        content.push(goals::weekly_balance_line("", studied_ms, expected_ms));
    }
    content.push(Line::default());
    content.extend(ranked_topics.into_iter().map(|(topic, duration_ms)| {
        Line::from(format!("  {topic:<20} {}", study_duration(duration_ms)))
    }));
    frame.render_widget(
        Paragraph::new(content)
            .block(Block::default().borders(Borders::ALL).title("History"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn study_duration(duration_ms: u64) -> String {
    if duration_ms.is_multiple_of(60_000) {
        goals::format_goal_duration(duration_ms, false)
    } else {
        format_duration(duration_ms)
    }
}
