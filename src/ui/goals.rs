use crate::model::{GoalProgress, WeeklyGoal};
use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};

pub(crate) fn parse_goal(input: &str) -> Result<WeeklyGoal> {
    let mut fields = input.split_whitespace();
    let (Some(date), Some(minutes), None) = (fields.next(), fields.next(), fields.next()) else {
        bail!("Enter a Monday date and weekly minutes");
    };
    let starts_on = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .context("Enter the start date as YYYY-MM-DD")?;
    let minutes_per_week = minutes.parse().context("Enter whole minutes per week")?;
    Ok(WeeklyGoal {
        starts_on,
        minutes_per_week,
    })
}

pub(crate) fn draw_editor(frame: &mut Frame<'_>, input: &str, area: Rect) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Enter a Monday (YYYY-MM-DD) and minutes per week."),
            Line::from("Example: 2026-11-16 420"),
            Line::from(format!("{input}_")),
        ])
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title("Set weekly goal")),
        area,
    );
}

fn format_goal_duration(duration_ms: u64, show_days: bool) -> String {
    let total_minutes = duration_ms / 60_000;
    if total_minutes == 0 && duration_ms > 0 {
        return "<1m".into();
    }
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if show_days && hours >= 24 {
        format!("{}d {}h {minutes:02}m", hours / 24, hours % 24)
    } else {
        format!("{hours}h {minutes:02}m")
    }
}

fn goal_balance(studied_ms: u64, expected_ms: u64) -> (&'static str, Color) {
    match studied_ms.cmp(&expected_ms) {
        std::cmp::Ordering::Greater => ("Ahead by", Color::Green),
        std::cmp::Ordering::Less => ("Behind by", Color::LightRed),
        std::cmp::Ordering::Equal => ("On track", Color::Green),
    }
}

pub(crate) fn draw_progress(frame: &mut Frame<'_>, progress: &GoalProgress, area: Rect) {
    let lines = match progress {
        GoalProgress::NoGoal => vec![Line::from("No weekly goal. Press e to set one.")],
        GoalProgress::Scheduled(goal) => vec![Line::from(format!(
            "Plan starts on {} · {} per week",
            goal.starts_on,
            format_goal_duration(u64::from(goal.minutes_per_week) * 60_000, false)
        ))],
        GoalProgress::Started {
            starts_on,
            current_goal,
            next_goal,
            studied_ms,
            expected_ms,
        } => {
            let weekly_goal_ms = u64::from(current_goal.minutes_per_week) * 60_000;
            let difference_ms = studied_ms.abs_diff(*expected_ms);
            let mut difference = format_goal_duration(difference_ms, false);
            if difference_ms >= weekly_goal_ms {
                let equivalent_weeks = difference_ms as f64 / weekly_goal_ms as f64;
                difference.push_str(&format!(" (~{equivalent_weeks:.1} weeks at current goal)"));
            }
            let (label, color) = goal_balance(*studied_ms, *expected_ms);
            let status = if difference_ms == 0 {
                label.to_owned()
            } else {
                format!("{label} {difference}")
            };
            let mut lines = vec![
                Line::from(format!("Plan since {starts_on}")),
                Line::from(format!(
                    "{:<24} {}",
                    "Current weekly goal",
                    format_goal_duration(weekly_goal_ms, false)
                )),
                Line::from(format!(
                    "{:<24} {}",
                    "Total studied",
                    format_goal_duration(*studied_ms, true)
                )),
                Line::from(format!(
                    "{:<24} {}",
                    "Expected through today",
                    format_goal_duration(*expected_ms, true)
                )),
                Line::styled(
                    status,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ];
            if let Some(goal) = next_goal {
                lines.push(Line::from(format!(
                    "{:<24} {} from {}",
                    "Next weekly goal",
                    format_goal_duration(u64::from(goal.minutes_per_week) * 60_000, false),
                    goal.starts_on,
                )));
            }
            lines
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title("Goal progress · 1d = 24h")),
        area,
    );
}

pub(crate) fn draw_weekly_balance(
    frame: &mut Frame<'_>,
    progress: &GoalProgress,
    today: NaiveDate,
    total: u64,
    area: Rect,
) -> Rect {
    if let GoalProgress::Started { current_goal, .. } = progress {
        let days_elapsed = u64::from(today.weekday().number_from_monday());
        let expected_ms = current_goal.expected_ms_for_days(days_elapsed);
        let (label, color) = goal_balance(total, expected_ms);
        let balance = if total == expected_ms {
            format!("  This week · {label}")
        } else {
            let difference = format_goal_duration(total.abs_diff(expected_ms), false);
            format!("  This week · {label} {difference}")
        };
        let [status, study_list] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(
            Paragraph::new(balance).style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
            status,
        );
        study_list
    } else {
        area
    }
}
