use crate::{App, Screen};
use anyhow::{Result, bail};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

pub(super) fn draw_topic_select(frame: &mut Frame<'_>, app: &App, area: Rect) {
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
    let topics: Vec<_> = app.filtered_topics().chain(["+ Create topic"]).collect();
    draw_list(frame, title, &topics, app.selected, results);
}

fn draw_list(
    frame: &mut Frame<'_>,
    title: &str,
    options: &[impl AsRef<str>],
    selected: usize,
    area: Rect,
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

pub(super) fn draw_add_topic(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = match app.topic_creation_origin {
        Screen::TimerTopicSelect => "Create topic and start timer",
        Screen::TopicSelect => "Create topic and log time",
        _ => "Create topic",
    };
    let input = Paragraph::new(format!("Topic name: {}_", app.topic_input))
        .block(Block::bordered().title(title))
        .wrap(Wrap { trim: true });
    frame.render_widget(input, area);
}

pub(super) fn draw_date_select(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let selected_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let date = app.date_draft;
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

pub(super) fn draw_duration(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let selected = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default();
    let date = if app.study_date == chrono::Local::now().date_naive() {
        format!("Today ({})", app.study_date)
    } else {
        app.study_date.to_string()
    };
    let cursor = if app.date_focused { "" } else { "_" };
    let duration = format!("Duration: {}{cursor}", app.duration_input);
    let change_date = format!("Change date: {date}");
    let content = vec![
        Line::from(format!("Topic: {}", app.study_topic)),
        Line::styled(duration, if app.date_focused { normal } else { selected }),
        Line::styled(
            change_date,
            if app.date_focused { selected } else { normal },
        ),
        Line::from("Enter minutes or a duration like 1h 30m."),
    ];
    frame.render_widget(
        Paragraph::new(content)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title("Log study time")),
        area,
    );
}

pub(crate) fn parse_duration(input: &str) -> Result<u32> {
    let input = input.trim().to_ascii_lowercase();
    let total = if let Some((hours, minutes)) = input.split_once('h') {
        let hours = hours.trim().parse::<u32>().ok();
        let minutes = if minutes.trim().is_empty() {
            Some(0)
        } else {
            minutes
                .trim()
                .strip_suffix('m')
                .and_then(|value| value.trim().parse::<u32>().ok())
        };
        hours
            .and_then(|hours| hours.checked_mul(60))
            .zip(minutes)
            .and_then(|(hours, minutes)| hours.checked_add(minutes))
    } else {
        input
            .strip_suffix('m')
            .unwrap_or(&input)
            .trim()
            .parse::<u32>()
            .ok()
    };
    match total {
        Some(minutes) if (1..=99 * 60 + 59).contains(&minutes) => Ok(minutes),
        _ => bail!("Enter a duration from 1m to 99h 59m, such as 45m or 1h 30m."),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn duration_accepts_minutes_and_hours_and_rejects_invalid_input() {
        for (input, minutes) in [
            ("45", 45),
            ("45m", 45),
            ("1h", 60),
            ("1H30M", 90),
            ("1h 30m", 90),
            ("99h 59m", 5999),
        ] {
            assert_eq!(parse_duration(input).unwrap(), minutes);
        }
        for input in ["", "0m", "100h", "45mm", "1h potato", "4294967295h 1m"] {
            assert!(parse_duration(input).is_err(), "{input}");
        }
    }
}
