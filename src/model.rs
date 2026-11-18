use chrono::{Datelike, Duration, NaiveDate};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentEntry {
    pub date: NaiveDate,
    pub topic: String,
    pub minutes: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub iso_year: i32,
    pub iso_week: u32,
    pub topic: String,
    pub minutes: u32,
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

pub fn aggregate_current(entries: &[CurrentEntry]) -> BTreeMap<String, u32> {
    let mut totals = BTreeMap::new();
    for entry in entries {
        *totals.entry(entry.topic.clone()).or_insert(0) += entry.minutes;
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
                minutes: 90,
            },
            CurrentEntry {
                date: monday + Duration::days(1),
                topic: "java".into(),
                minutes: 30,
            },
            CurrentEntry {
                date: monday,
                topic: "design".into(),
                minutes: 45,
            },
        ];
        let totals = aggregate_current(&entries);
        assert_eq!(totals["java"], 120);
        assert_eq!(totals["design"], 45);
    }
}
