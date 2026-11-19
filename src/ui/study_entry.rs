use crate::{App, Screen};
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

pub(super) fn draw_add_topic(frame: &mut Frame<'_>, value: &str, area: Rect) {
    let input = Paragraph::new(format!("{value}_"))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("New topic · Enter to save"),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(input, area);
}

pub(super) fn draw_date_select(frame: &mut Frame<'_>, app: &App, area: Rect) {
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

pub(super) fn draw_duration(frame: &mut Frame<'_>, app: &App, area: Rect) {
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
