use crate::model::{HistoryEntry, aggregate_history};
use ratatui::{
    Frame,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::cmp::Reverse;

pub(super) fn draw(
    frame: &mut Frame<'_>,
    entries: &[HistoryEntry],
    week: Option<(i32, u32)>,
    area: Rect,
) {
    let Some((iso_year, iso_week)) = week else {
        frame.render_widget(
            Paragraph::new("No history available.")
                .block(Block::default().borders(Borders::ALL).title("History")),
            area,
        );
        return;
    };
    let entries: Vec<HistoryEntry> = entries
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
