use crate::{
    App,
    model::{aggregate_current, iso_week_key, week_start},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use super::{format_duration, goals};

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let today = app.snapshot_date;
    let start = week_start(today);
    let (iso_year, iso_week) = iso_week_key(today);
    let totals = aggregate_current(&app.current);
    let total: u64 = totals.values().sum();
    let area = goals::draw_weekly_balance(frame, &app.goal_progress, today, total, area);
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
        let message_area = Rect {
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
