use crate::{
    App, HOME_ACTIONS,
    model::{aggregate_current, iso_week_key, week_start},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use super::{format_duration, goals};

pub(super) fn draw(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let [menu, summary] = Layout::vertical([
        Constraint::Length(HOME_ACTIONS.len() as u16),
        Constraint::Min(0),
    ])
    .areas(area);
    let actions =
        HOME_ACTIONS.map(|(label, shortcut)| ListItem::new(format!("{label} [{shortcut}]")));
    let mut state = ListState::default().with_selected(Some(app.home_selection));
    frame.render_stateful_widget(
        List::new(actions)
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> "),
        menu,
        &mut state,
    );
    let today = app.snapshot_date;
    let start = week_start(today);
    let (iso_year, iso_week) = iso_week_key(today);
    let totals = aggregate_current(&app.current);
    let total: u64 = totals.values().sum();
    let area = goals::draw_weekly_balance(frame, &app.goal_progress, today, total, summary);
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
    let block = Block::default().borders(Borders::ALL).title(format!(
        " This week · Total {} · {iso_year}-W{iso_week:02} · {start} ",
        format_duration(total)
    ));
    let mut state = ListState::default().with_offset(app.dashboard_scroll);
    frame.render_stateful_widget(List::new(rows).block(block), area, &mut state);
}
