mod migrations;

use crate::model::{CurrentEntry, HistoryEntry, current_week, iso_week_key, week_start};
use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate, Weekday};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn default_database_path() -> Result<PathBuf> {
        Ok(dirs::data_local_dir()
            .context("Could not locate the user's data directory")?
            .join("kronos")
            .join("kronos.sqlite3"))
    }

    pub fn create_new(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            bail!("The database path must be absolute");
        }
        if path.exists() {
            bail!(
                "A file already exists at {}. Choose the existing-database option or use another path",
                path.display()
            );
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("Could not create database directory {}", parent.display()))?;

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("Could not create database file {}", path.display()))?;

        let result = (|| {
            let mut connection =
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .with_context(|| format!("Could not open database {}", path.display()))?;
            configure_connection(&connection)?;
            migrations::create_schema(&mut connection)
                .context("Could not initialize the SQLite database")?;
            Ok(Self { connection })
        })();
        match result {
            Ok(store) => Ok(store),
            Err(error) => {
                if let Err(cleanup_error) = fs::remove_file(path) {
                    return Err(error).context(format!(
                        "Could not remove incomplete database {}: {cleanup_error}",
                        path.display()
                    ));
                }
                Err(error)
            }
        }
    }

    pub fn open_existing(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            bail!("The database path must be absolute");
        }
        if !path.is_file() {
            bail!("Database file does not exist: {}", path.display());
        }
        let mut connection =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                .with_context(|| format!("Could not open database {}", path.display()))?;
        configure_connection(&connection)?;
        migrations::apply_pending(&mut connection)?;

        Ok(Self { connection })
    }

    pub fn topics(&self) -> Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT name FROM topics ORDER BY name COLLATE NOCASE")?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn add_topic(&mut self, topic: &str) -> Result<()> {
        let topic = topic.trim().to_lowercase();
        if topic.is_empty() {
            bail!("Topic name cannot be empty");
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = transaction.execute(
            "INSERT INTO topics(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            [&topic],
        )?;
        if inserted == 0 {
            bail!("Topic \"{topic}\" already exists");
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_current(&self) -> Result<Vec<CurrentEntry>> {
        let mut statement = self.connection.prepare(
            "SELECT daily_entries.date, topics.name, daily_entries.minutes
             FROM daily_entries JOIN topics ON topics.id = daily_entries.topic_id
             ORDER BY daily_entries.date, topics.name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (date, topic, minutes) = row?;
            Ok(CurrentEntry {
                date: parse_date(&date)?,
                topic,
                minutes: parse_minutes(minutes)?,
            })
        })
        .collect()
    }

    pub fn load_history(&self) -> Result<Vec<HistoryEntry>> {
        let mut statement = self.connection.prepare(
            "SELECT weekly_totals.iso_year, weekly_totals.iso_week, topics.name, weekly_totals.minutes
             FROM weekly_totals JOIN topics ON topics.id = weekly_totals.topic_id
             ORDER BY weekly_totals.iso_year DESC, weekly_totals.iso_week DESC, topics.name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (iso_year, iso_week, topic, minutes) = row?;
            let (iso_year, iso_week) = parse_iso_week_key(iso_year, iso_week)?;
            Ok(HistoryEntry {
                iso_year,
                iso_week,
                topic,
                minutes: parse_minutes(minutes)?,
            })
        })
        .collect()
    }

    pub fn add_study(&mut self, date: NaiveDate, topic: &str, minutes: u32) -> Result<()> {
        if minutes == 0 {
            bail!("Duration must be greater than zero");
        }
        let today = Local::now().date_naive();
        if date > today {
            bail!("The date cannot be in the future");
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let topic_id: i64 = transaction
            .query_row("SELECT id FROM topics WHERE name = ?1", [topic], |row| {
                row.get(0)
            })
            .optional()?
            .with_context(|| format!("Topic \"{topic}\" does not exist"))?;

        finalize_completed_weeks(&transaction, today)?;
        let minutes = i64::from(minutes);
        if week_start(date) == current_week(today) {
            transaction.execute(
                "INSERT INTO daily_entries(date, topic_id, minutes)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(date, topic_id)
                 DO UPDATE SET minutes = daily_entries.minutes + excluded.minutes",
                params![date.to_string(), topic_id, minutes],
            )?;
        } else {
            add_weekly_minutes(&transaction, iso_week_key(date), topic_id, minutes)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn rollover_if_needed(&mut self, today: NaiveDate) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        finalize_completed_weeks(&transaction, today)?;
        transaction.commit()?;
        Ok(())
    }
}

fn configure_connection(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
    Ok(())
}

fn finalize_completed_weeks(transaction: &Transaction<'_>, today: NaiveDate) -> Result<()> {
    let cutoff = current_week(today).to_string();
    let stale_entries = {
        let mut statement = transaction.prepare(
            "SELECT date, topic_id, minutes FROM daily_entries WHERE date < ?1 ORDER BY date, topic_id",
        )?;
        let rows = statement.query_map([&cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut totals = BTreeMap::<(i32, u32, i64), u64>::new();
    for (date, topic_id, minutes) in stale_entries {
        let date = parse_date(&date)?;
        let minutes = u64::try_from(minutes).context("Invalid daily minute total")?;
        let (iso_year, iso_week) = iso_week_key(date);
        let total = totals.entry((iso_year, iso_week, topic_id)).or_default();
        *total = total
            .checked_add(minutes)
            .context("Weekly minute total is out of range")?;
    }

    for ((iso_year, iso_week, topic_id), minutes) in totals {
        let minutes = i64::try_from(minutes).context("Weekly minute total is out of range")?;
        add_weekly_minutes(transaction, (iso_year, iso_week), topic_id, minutes)?;
    }
    transaction.execute("DELETE FROM daily_entries WHERE date < ?1", [&cutoff])?;
    Ok(())
}

fn add_weekly_minutes(
    transaction: &Transaction<'_>,
    (iso_year, iso_week): (i32, u32),
    topic_id: i64,
    minutes: i64,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO weekly_totals(iso_year, iso_week, topic_id, minutes)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(iso_year, iso_week, topic_id)
         DO UPDATE SET minutes = weekly_totals.minutes + excluded.minutes",
        params![iso_year, iso_week, topic_id, minutes],
    )?;
    Ok(())
}

fn parse_iso_week_key(year: i64, week: i64) -> Result<(i32, u32)> {
    let year = i32::try_from(year).context("ISO week year is out of range")?;
    let week = u32::try_from(week).context("ISO week number is out of range")?;
    NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)
        .context("Invalid ISO year/week combination")?;
    Ok((year, week))
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("Invalid date in database: {value}"))
}

fn parse_minutes(minutes: i64) -> Result<u32> {
    u32::try_from(minutes).context("Minute total in database is out of range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as DateDuration;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    pub(super) fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kronos-sqlite-test-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn temporary_store() -> (Store, PathBuf) {
        let directory = temporary_directory();
        let store = Store::create_new(&directory.join("kronos.sqlite3")).unwrap();
        (store, directory)
    }

    #[test]
    fn creates_database_parent_directories_and_empty_tables() {
        let directory = temporary_directory().join("nested").join("data");
        let database = directory.join("kronos.sqlite3");
        let store = Store::create_new(&database).unwrap();

        assert!(database.exists());
        assert!(directory.exists());
        assert!(store.topics().unwrap().is_empty());
        assert!(store.load_current().unwrap().is_empty());
        assert!(store.load_history().unwrap().is_empty());

        drop(store);
        fs::remove_dir_all(directory.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn existing_database_is_opened_only_after_schema_validation() {
        let directory = temporary_directory();
        let database = directory.join("kronos.sqlite3");
        drop(Store::create_new(&database).unwrap());

        let store = Store::open_existing(&database).unwrap();
        assert!(store.topics().unwrap().is_empty());

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn opening_a_missing_or_unrelated_database_never_creates_app_tables() {
        let directory = temporary_directory();
        let missing = directory.join("missing.sqlite3");
        assert!(Store::open_existing(&missing).is_err());
        assert!(!missing.exists());

        let unrelated = directory.join("unrelated.sqlite3");
        let connection = Connection::open(&unrelated).unwrap();
        connection
            .execute("CREATE TABLE unrelated(value TEXT)", [])
            .unwrap();
        drop(connection);
        assert!(Store::open_existing(&unrelated).is_err());

        let connection = Connection::open(unrelated).unwrap();
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'topics'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);

        drop(connection);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn new_topics_are_lowercase_and_reject_case_insensitive_duplicates() {
        let (mut store, directory) = temporary_store();
        store.add_topic("  RuSt  ").unwrap();

        assert!(store.add_topic("RUST").is_err());
        assert_eq!(store.topics().unwrap(), vec!["rust"]);

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn study_entries_keep_their_topic_when_its_name_changes() {
        let (mut store, directory) = temporary_store();
        let today = Local::now().date_naive();
        store.add_topic("rust").unwrap();
        store.add_study(today, "rust", 30).unwrap();
        store
            .add_study(today - DateDuration::weeks(1), "rust", 45)
            .unwrap();

        store
            .connection
            .execute("UPDATE topics SET name = 'systems' WHERE name = 'rust'", [])
            .unwrap();

        assert_eq!(store.load_current().unwrap()[0].topic, "systems");
        assert_eq!(store.load_history().unwrap()[0].topic, "systems");

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rollover_aggregates_every_stale_week_atomically() {
        let (mut store, directory) = temporary_store();
        store.add_topic("java").unwrap();
        let first_monday = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        let second_monday = NaiveDate::from_ymd_opt(2026, 8, 24).unwrap();
        for (date, minutes) in [
            (first_monday, 60),
            (first_monday + DateDuration::days(1), 30),
            (second_monday, 45),
        ] {
            store
                .connection
                .execute(
                    "INSERT INTO daily_entries(date, topic_id, minutes) SELECT ?1, id, ?2 FROM topics WHERE name = 'java'",
                    params![date.to_string(), minutes],
                )
                .unwrap();
        }

        store
            .rollover_if_needed(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap())
            .unwrap();

        assert!(store.load_current().unwrap().is_empty());
        assert_eq!(
            store.load_history().unwrap(),
            vec![
                HistoryEntry {
                    iso_year: 2026,
                    iso_week: 35,
                    topic: "java".into(),
                    minutes: 45,
                },
                HistoryEntry {
                    iso_year: 2026,
                    iso_week: 34,
                    topic: "java".into(),
                    minutes: 90,
                },
            ]
        );
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn adding_study_from_three_months_ago_updates_its_original_week() {
        let (mut store, directory) = temporary_store();
        store.add_topic("java").unwrap();
        let old_monday = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();

        store.add_study(old_monday, "java", 60).unwrap();
        store
            .add_study(old_monday + DateDuration::days(2), "java", 45)
            .unwrap();

        assert!(store.load_current().unwrap().is_empty());
        assert_eq!(
            store.load_history().unwrap(),
            vec![HistoryEntry {
                iso_year: 2026,
                iso_week: 34,
                topic: "java".into(),
                minutes: 105,
            }]
        );
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_add_rolls_back_rollover_and_preserves_daily_rows() {
        let (mut store, directory) = temporary_store();
        store.add_topic("java").unwrap();
        let old_monday = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        store
            .connection
            .execute(
                "INSERT INTO daily_entries(date, topic_id, minutes) SELECT ?1, id, 60 FROM topics WHERE name = 'java'",
                [old_monday.to_string()],
            )
            .unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_weekly_insert
                 BEFORE INSERT ON weekly_totals
                 BEGIN
                     SELECT RAISE(ABORT, 'test failure');
                 END;",
            )
            .unwrap();

        assert!(
            store
                .add_study(Local::now().date_naive(), "java", 15)
                .is_err()
        );
        assert_eq!(store.load_current().unwrap().len(), 1);
        assert!(store.load_history().unwrap().is_empty());

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }
}
