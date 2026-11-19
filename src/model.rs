use chrono::{Datelike, Duration, NaiveDate};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentEntry {
    pub date: NaiveDate,
    pub topic: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub iso_year: i32,
    pub iso_week: u32,
    pub topic: String,
    pub minutes: u32,
}

#[derive(Clone, Copy)]
pub struct WeeklyGoal {
    pub starts_on: NaiveDate,
    pub minutes_per_week: u32,
}

pub enum GoalProgress {
    NoGoal,
    Scheduled(WeeklyGoal),
    Started {
        starts_on: NaiveDate,
        current_goal: WeeklyGoal,
        next_goal: Option<WeeklyGoal>,
        studied_ms: u64,
        expected_ms: u64,
    },
}

pub fn summarize_goals(
    today: NaiveDate,
    goals: &[WeeklyGoal],
    daily_entries: &[CurrentEntry],
    weekly_totals: &[HistoryEntry],
) -> GoalProgress {
    let Some(first_goal) = goals.first() else {
        return GoalProgress::NoGoal;
    };
    if first_goal.starts_on > today {
        return GoalProgress::Scheduled(*first_goal);
    }

    const MS_PER_MINUTE: u64 = 60_000;
    let mut current_goal = *first_goal;
    let mut expected_weekly_ms_days = 0;
    for (index, goal) in goals
        .iter()
        .enumerate()
        .take_while(|(_, goal)| goal.starts_on <= today)
    {
        let days = match goals.get(index + 1).filter(|next| next.starts_on <= today) {
            Some(next) => (next.starts_on - goal.starts_on).num_days(),
            None => (today - goal.starts_on).num_days() + 1,
        };
        expected_weekly_ms_days += days as u64 * u64::from(goal.minutes_per_week) * MS_PER_MINUTE;
        current_goal = *goal;
    }

    let first_week = iso_week_key(first_goal.starts_on);
    let current_week = iso_week_key(today);
    let archived_ms: u64 = weekly_totals
        .iter()
        .filter(|entry| (first_week..current_week).contains(&(entry.iso_year, entry.iso_week)))
        .map(|entry| u64::from(entry.minutes) * MS_PER_MINUTE)
        .sum();
    let daily_ms: u64 = daily_entries
        .iter()
        .filter(|entry| (first_goal.starts_on..=today).contains(&entry.date))
        .map(|entry| entry.duration_ms)
        .sum();

    GoalProgress::Started {
        starts_on: first_goal.starts_on,
        current_goal,
        next_goal: goals.iter().find(|goal| goal.starts_on > today).copied(),
        studied_ms: archived_ms + daily_ms,
        expected_ms: expected_weekly_ms_days / 7,
    }
}

pub fn week_start(date: NaiveDate) -> NaiveDate {
    let days_since_monday = date.weekday().num_days_from_monday() as i64;
    date - Duration::days(days_since_monday)
}

pub fn iso_week_key(date: NaiveDate) -> (i32, u32) {
    let week = date.iso_week();
    (week.year(), week.week())
}

pub fn current_week(today: NaiveDate) -> NaiveDate {
    week_start(today)
}

pub fn aggregate_current(entries: &[CurrentEntry]) -> BTreeMap<String, u64> {
    let mut totals = BTreeMap::new();
    for entry in entries {
        *totals.entry(entry.topic.clone()).or_insert(0) += entry.duration_ms;
    }
    totals
}

pub fn aggregate_history(entries: &[HistoryEntry]) -> BTreeMap<String, u32> {
    let mut totals = BTreeMap::new();
    for entry in entries {
        *totals.entry(entry.topic.clone()).or_insert(0) += entry.minutes;
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    #[test]
    fn progress_includes_empty_weeks_and_prorates_the_current_goal() {
        let date = |month, day| NaiveDate::from_ymd_opt(2026, month, day).unwrap();
        let goals = [
            WeeklyGoal {
                starts_on: date(1, 5),
                minutes_per_week: 420,
            },
            WeeklyGoal {
                starts_on: date(1, 19),
                minutes_per_week: 840,
            },
            WeeklyGoal {
                starts_on: date(1, 26),
                minutes_per_week: 600,
            },
        ];
        let archive = [
            HistoryEntry {
                iso_year: 2026,
                iso_week: 1,
                topic: "rust".into(),
                minutes: 30,
            },
            HistoryEntry {
                iso_year: 2026,
                iso_week: 2,
                topic: "rust".into(),
                minutes: 90,
            },
        ];
        let daily = [
            CurrentEntry {
                date: date(1, 11),
                topic: "rust".into(),
                duration_ms: 25_000,
            },
            CurrentEntry {
                date: date(1, 21),
                topic: "rust".into(),
                duration_ms: 3_600_000,
            },
        ];
        let GoalProgress::Started {
            studied_ms,
            expected_ms,
            current_goal,
            next_goal,
            ..
        } = summarize_goals(date(1, 21), &goals, &daily, &archive)
        else {
            panic!("the goal plan has already started");
        };
        assert_eq!(studied_ms, (90 + 60) * 60_000 + 25_000);
        assert_eq!(expected_ms, (2 * 420 + 3 * 120) * 60_000);
        assert_eq!(current_goal.minutes_per_week, 840);
        assert_eq!(next_goal.unwrap().minutes_per_week, 600);
    }

    #[test]
    fn weeks_start_on_monday_even_across_year_boundary() {
        let sunday = NaiveDate::from_ymd_opt(2026, 1, 4).unwrap();
        assert_eq!(
            NaiveDate::from_ymd_opt(2025, 12, 29).unwrap(),
            week_start(sunday)
        );
        assert_eq!(Weekday::Mon, week_start(sunday).weekday());
    }

    #[test]
    fn iso_week_key_uses_the_iso_year_at_calendar_year_boundaries() {
        let monday = NaiveDate::from_ymd_opt(2024, 12, 30).unwrap();
        assert_eq!((2025, 1), iso_week_key(monday));
        assert_eq!((2025, 1), iso_week_key(monday + Duration::days(6)));
        assert_eq!((2025, 2), iso_week_key(monday + Duration::days(7)));
    }

    #[test]
    fn current_entries_aggregate_by_topic() {
        let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let entries = vec![
            CurrentEntry {
                date: monday,
                topic: "java".into(),
                duration_ms: 90,
            },
            CurrentEntry {
                date: monday + Duration::days(1),
                topic: "java".into(),
                duration_ms: 30,
            },
            CurrentEntry {
                date: monday,
                topic: "design".into(),
                duration_ms: 45,
            },
        ];
        let totals = aggregate_current(&entries);
        assert_eq!(totals["java"], 120);
        assert_eq!(totals["design"], 45);
    }
}
