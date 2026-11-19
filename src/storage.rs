mod migrations;

const MILLIS_PER_MINUTE: i64 = 60_000;

use crate::model::{CurrentEntry, HistoryEntry, current_week, iso_week_key, week_start};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc, Weekday};
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

pub struct ActiveTimer {
    pub id: i64,
    pub topic: String,
    pub started_at: i64,
}

impl ActiveTimer {
    pub fn elapsed_seconds(&self, now: DateTime<Utc>) -> i64 {
        now.timestamp_millis()
            .saturating_sub(self.started_at)
            .max(0)
            / 1000
    }
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
            migrations::initialize_new_database(&mut connection)
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
        migrations::upgrade_existing_database(&mut connection)?;

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
            "SELECT daily_entries.date, topics.name, COALESCE(minutes * 60000, ended_at - started_at)
             FROM daily_entries JOIN topics ON topics.id = daily_entries.topic_id
             WHERE minutes IS NOT NULL OR ended_at IS NOT NULL
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
            let (date, topic, duration_ms) = row?;
            Ok(CurrentEntry {
                date: parse_date(&date)?,
                topic,
                duration_ms: u64::try_from(duration_ms).context("Invalid study duration")?,
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

        transaction.execute(
            "INSERT INTO daily_entries(date, topic_id, minutes) VALUES (?1, ?2, ?3)",
            params![date.to_string(), topic_id, minutes],
        )?;
        finalize_completed_weeks(&transaction, today)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn active_timer(&self) -> Result<Option<ActiveTimer>> {
        Ok(self
            .connection
            .query_row(
                "SELECT daily_entries.id, topics.name, started_at
             FROM daily_entries JOIN topics ON topics.id = daily_entries.topic_id
             WHERE minutes IS NULL AND ended_at IS NULL",
                [],
                |row| {
                    Ok(ActiveTimer {
                        id: row.get(0)?,
                        topic: row.get(1)?,
                        started_at: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn start_timer(&self, topic: &str, now: DateTime<Utc>) -> Result<()> {
        let inserted = self.connection.execute(
            "INSERT INTO daily_entries(date, topic_id, started_at)
             SELECT ?3, id, ?2 FROM topics WHERE name = ?1
             ON CONFLICT DO NOTHING",
            params![
                topic,
                now.timestamp_millis(),
                now.with_timezone(&Local).date_naive().to_string()
            ],
        )?;
        if inserted == 0 {
            if self.active_timer()?.is_some() {
                bail!("A study timer is already running");
            }
            bail!("Could not start a timer for \"{topic}\"");
        }
        Ok(())
    }

    pub fn discard_timer(&self, id: i64) -> Result<()> {
        self.connection.execute(
            "DELETE FROM daily_entries WHERE id = ?1 AND minutes IS NULL AND ended_at IS NULL",
            [id],
        )?;
        Ok(())
    }

    pub fn stop_timer(&mut self, id: i64, now: DateTime<Utc>) -> Result<Option<i64>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some((topic_id, started_at, date)) = transaction
            .query_row(
                "SELECT topic_id, started_at, date FROM daily_entries
             WHERE id = ?1 AND minutes IS NULL AND ended_at IS NULL",
                [id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
        else {
            return Ok(None);
        };
        let ended_at = now.timestamp_millis();
        let elapsed = ended_at
            .checked_sub(started_at)
            .filter(|&ms| ms >= 0)
            .context("The system clock is before the timer start")?;
        let today = now.with_timezone(&Local).date_naive();
        let start_date = parse_date(&date)?;
        let mut date = start_date;
        let mut portion_start = started_at;
        loop {
            let next_monday = week_start(date) + chrono::Duration::weeks(1);
            let portion_end = if next_monday <= today {
                local_week_boundary(next_monday)?
                    .timestamp_millis()
                    .clamp(portion_start, ended_at)
            } else {
                ended_at
            };
            if date == start_date {
                transaction.execute(
                    "UPDATE daily_entries SET ended_at = ?1 WHERE id = ?2",
                    params![portion_end, id],
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO daily_entries(date, topic_id, started_at, ended_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![date.to_string(), topic_id, portion_start, portion_end],
                )?;
            }
            if portion_end == ended_at {
                break;
            }
            date = next_monday;
            portion_start = portion_end;
        }
        finalize_completed_weeks(&transaction, today)?;
        transaction.commit()?;
        Ok(Some(elapsed))
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

fn local_week_boundary(monday: NaiveDate) -> Result<DateTime<Local>> {
    let midnight = monday.and_hms_opt(0, 0, 0).unwrap();
    // Some time zones skip or repeat local midnight.
    (0..86_400)
        .find_map(|seconds| {
            Local
                .from_local_datetime(&(midnight + chrono::Duration::seconds(seconds)))
                .earliest()
        })
        .context("Could not resolve the start of the local week")
}

fn configure_connection(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
    Ok(())
}

fn finalize_completed_weeks(transaction: &Transaction<'_>, today: NaiveDate) -> Result<()> {
    let cutoff = current_week(today).to_string();
    // Keep any week that can still receive time from the active timer unrounded.
    let active = transaction
        .query_row(
            "SELECT topic_id, date FROM daily_entries WHERE minutes IS NULL AND ended_at IS NULL",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let (protected_topic, protected_since) = match active {
        Some((topic, date)) => (
            Some(topic),
            Some(week_start(parse_date(&date)?).to_string()),
        ),
        None => (None, None),
    };
    let archive_filter = "WHERE date < ?1 AND (minutes IS NOT NULL OR ended_at IS NOT NULL)
                          AND (topic_id IS NOT ?2 OR date < ?3)";
    let entries = params![cutoff, protected_topic, protected_since];
    let stale_entries = {
        let mut statement = transaction.prepare(&format!(
            "SELECT date, topic_id, COALESCE(minutes * 60000, ended_at - started_at)
             FROM daily_entries {archive_filter}"
        ))?;
        let rows = statement.query_map(entries, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut totals = BTreeMap::<(i32, u32, i64), i64>::new();
    for (date, topic_id, duration_ms) in stale_entries {
        let (iso_year, iso_week) = iso_week_key(parse_date(&date)?);
        let total = totals.entry((iso_year, iso_week, topic_id)).or_default();
        *total = total
            .checked_add(duration_ms)
            .context("Weekly study duration is out of range")?;
    }

    for ((year, week, topic_id), duration_ms) in totals {
        let minutes = duration_ms / MILLIS_PER_MINUTE
            + i64::from(duration_ms % MILLIS_PER_MINUTE >= MILLIS_PER_MINUTE / 2);
        if minutes > 0 {
            transaction.execute(
                "INSERT INTO weekly_totals(iso_year, iso_week, topic_id, minutes)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(iso_year, iso_week, topic_id)
                 DO UPDATE SET minutes = weekly_totals.minutes + excluded.minutes",
                params![year, week, topic_id, minutes],
            )?;
        }
    }
    transaction.execute(
        &format!("DELETE FROM daily_entries {archive_filter}"),
        entries,
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
pub(crate) mod tests {
    use super::*;
    use chrono::Duration as DateDuration;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn temporary_directory() -> PathBuf {
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
    fn timer_survives_reopening_and_credits_both_weeks_once() {
        let (mut store, directory) = temporary_store();
        store.add_topic("rust").unwrap();
        store.add_topic("systems").unwrap();
        let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let boundary = local_week_boundary(monday).unwrap().with_timezone(&Utc);
        let start = boundary - DateDuration::hours(1);
        let end = boundary + DateDuration::hours(1);
        store.start_timer("rust", start).unwrap();
        drop(store);

        let database = directory.join("kronos.sqlite3");
        let mut store = Store::open_existing(&database).unwrap();
        let other = Store::open_existing(&database).unwrap();
        let timer = store.active_timer().unwrap().unwrap();
        assert_eq!(timer.topic, "rust");
        assert_eq!(timer.elapsed_seconds(end), 7200);
        assert!(other.start_timer("systems", start).is_err());

        assert_eq!(store.stop_timer(timer.id, end).unwrap(), Some(7_200_000));
        other.start_timer("systems", end).unwrap();
        assert_eq!(store.stop_timer(timer.id, end).unwrap(), None);
        assert_eq!(store.active_timer().unwrap().unwrap().topic, "systems");
        let entries = store.load_current().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, end.with_timezone(&Local).date_naive());
        assert_eq!(entries[0].duration_ms, 3_600_000);
        let history = store.load_history().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!((history[0].iso_year, history[0].iso_week), (2026, 46));
        assert_eq!(history[0].minutes, 60);

        drop(other);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn timer_keeps_exact_durations_until_weekly_rollover() {
        let (mut store, directory) = temporary_store();
        let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let start = local_week_boundary(monday).unwrap().with_timezone(&Utc);
        for (topic, durations) in [
            ("short", vec![27_000, 2_999]),
            ("half", vec![15_000, 15_000]),
            ("manual and timer", vec![27_000, 4_000]),
        ] {
            store.add_topic(topic).unwrap();
            for milliseconds in durations {
                store.start_timer(topic, start).unwrap();
                let id = store.active_timer().unwrap().unwrap().id;
                let end = start + DateDuration::milliseconds(milliseconds);
                assert_eq!(store.stop_timer(id, end).unwrap(), Some(milliseconds));
            }
        }
        store
            .connection
            .execute(
                "INSERT INTO daily_entries(date, topic_id, minutes)
             SELECT ?1, id, 1 FROM topics WHERE name = 'manual and timer'",
                [monday.to_string()],
            )
            .unwrap();

        let totals = crate::model::aggregate_current(&store.load_current().unwrap());
        assert_eq!(totals["short"], 29_999);
        assert_eq!(totals["half"], 30_000);
        assert_eq!(totals["manual and timer"], 91_000);

        store
            .rollover_if_needed(monday + DateDuration::weeks(1))
            .unwrap();
        assert!(store.load_current().unwrap().is_empty());
        let totals = crate::model::aggregate_history(&store.load_history().unwrap());
        assert!(!totals.contains_key("short"));
        assert_eq!(totals["half"], 1);
        assert_eq!(totals["manual and timer"], 2);

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rollover_waits_for_the_active_timer_before_archiving_its_topic() {
        let (mut store, directory) = temporary_store();
        store.add_topic("rust").unwrap();
        let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let boundary = local_week_boundary(monday).unwrap().with_timezone(&Utc);
        let first_start = boundary - DateDuration::hours(1);
        store.start_timer("rust", first_start).unwrap();
        let id = store.active_timer().unwrap().unwrap().id;
        store
            .stop_timer(id, first_start + DateDuration::seconds(20))
            .unwrap();

        let start = boundary - DateDuration::seconds(20);
        store.start_timer("rust", start).unwrap();
        let id = store.active_timer().unwrap().unwrap().id;
        store.rollover_if_needed(monday).unwrap();
        assert_eq!(store.active_timer().unwrap().unwrap().id, id);
        assert!(store.load_history().unwrap().is_empty());
        assert_eq!(store.load_current().unwrap()[0].duration_ms, 20_000);

        store
            .stop_timer(id, boundary + DateDuration::seconds(20))
            .unwrap();
        assert_eq!(store.load_history().unwrap()[0].minutes, 1);
        assert_eq!(store.load_current().unwrap()[0].duration_ms, 20_000);

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discarding_a_timer_deletes_only_the_matching_open_session() {
        let (mut store, directory) = temporary_store();
        store.add_topic("rust").unwrap();
        let start = Utc::now();
        store.start_timer("rust", start).unwrap();
        let completed = store.active_timer().unwrap().unwrap().id;
        store.stop_timer(completed, start).unwrap();

        store.start_timer("rust", start).unwrap();
        let discarded = store.active_timer().unwrap().unwrap().id;
        store.discard_timer(discarded).unwrap();
        store.start_timer("rust", start).unwrap();
        let replacement = store.active_timer().unwrap().unwrap().id;
        store.discard_timer(discarded).unwrap();
        store.discard_timer(completed).unwrap();

        assert_eq!(store.active_timer().unwrap().unwrap().id, replacement);
        let count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM daily_entries WHERE id IN (?1, ?2)",
                params![discarded, completed],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unsuccessful_timer_credit_leaves_the_timer_running() {
        let (mut store, directory) = temporary_store();
        store.add_topic("rust").unwrap();
        let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let start = local_week_boundary(monday).unwrap().with_timezone(&Utc);
        let start = start - DateDuration::seconds(40);
        store.start_timer("rust", start).unwrap();
        let timer = store.active_timer().unwrap().unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_timer_credit BEFORE INSERT ON weekly_totals
             BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
            )
            .unwrap();

        assert!(
            store
                .stop_timer(timer.id, start + DateDuration::minutes(1))
                .is_err()
        );
        assert_eq!(store.active_timer().unwrap().unwrap().id, timer.id);
        assert!(store.load_current().unwrap().is_empty());

        let end = start + DateDuration::seconds(27);
        assert_eq!(store.stop_timer(timer.id, end).unwrap(), Some(27_000));
        assert!(store.active_timer().unwrap().is_none());
        assert_eq!(store.load_current().unwrap()[0].duration_ms, 27_000);

        drop(store);
        fs::remove_dir_all(directory).unwrap();
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
