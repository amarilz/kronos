use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Transaction, TransactionBehavior};

const DATABASE_APPLICATION_ID: i64 = 0x5354_5544;

const V1_0_3_TABLES: &str = "CREATE TABLE topics (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE
);
CREATE TABLE daily_entries (
    date TEXT NOT NULL,
    topic_id INTEGER NOT NULL,
    minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 4294967295),
    PRIMARY KEY (date, topic_id),
    FOREIGN KEY (topic_id) REFERENCES topics(id)
);
CREATE TABLE weekly_totals (
    iso_year INTEGER NOT NULL,
    iso_week INTEGER NOT NULL CHECK (iso_week BETWEEN 1 AND 53),
    topic_id INTEGER NOT NULL,
    minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 4294967295),
    PRIMARY KEY (iso_year, iso_week, topic_id),
    FOREIGN KEY (topic_id) REFERENCES topics(id)
);";

type TableColumns = &'static [(&'static str, &'static [(&'static str, &'static str)])];

const V1_0_1_COLUMNS: TableColumns = &[
    ("topics", &[("name", "TEXT")]),
    (
        "daily_entries",
        &[("date", "TEXT"), ("topic", "TEXT"), ("minutes", "INTEGER")],
    ),
    (
        "weekly_totals",
        &[
            ("iso_year", "INTEGER"),
            ("iso_week", "INTEGER"),
            ("topic", "TEXT"),
            ("minutes", "INTEGER"),
        ],
    ),
];

const V1_0_2_COLUMNS: TableColumns = &[
    ("topics", &[("id", "INTEGER"), ("name", "TEXT")]),
    (
        "daily_entries",
        &[
            ("date", "TEXT"),
            ("topic_id", "INTEGER"),
            ("minutes", "INTEGER"),
        ],
    ),
    (
        "weekly_totals",
        &[
            ("iso_year", "INTEGER"),
            ("iso_week", "INTEGER"),
            ("topic_id", "INTEGER"),
            ("minutes", "INTEGER"),
        ],
    ),
];

pub(super) fn create_schema(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(V1_0_3_TABLES)?;
    transaction.pragma_update(None, "application_id", DATABASE_APPLICATION_ID)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn apply_pending(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_database(&transaction)?;

    migrate_to_v1_0_2(&transaction).context("Could not migrate Kronos topic IDs to v1.0.2")?;

    verify_columns(&transaction, V1_0_2_COLUMNS)?;
    transaction.commit()?;
    Ok(())
}

fn migrate_to_v1_0_2(transaction: &Transaction<'_>) -> Result<()> {
    let topic_columns = table_columns(transaction, "topics")?;
    if topic_columns != [("name".to_owned(), "TEXT".to_owned())] {
        return Ok(());
    }

    verify_columns(transaction, V1_0_1_COLUMNS)?;
    transaction.execute_batch(
        "ALTER TABLE daily_entries RENAME TO old_daily_entries;
         ALTER TABLE weekly_totals RENAME TO old_weekly_totals;
         ALTER TABLE topics RENAME TO old_topics;",
    )?;
    transaction.execute_batch(V1_0_3_TABLES)?;
    transaction.execute_batch(
        "INSERT INTO topics(name) SELECT name FROM old_topics ORDER BY name;
         -- Recover names referenced by older databases with orphaned entries.
         INSERT INTO topics(name)
             SELECT topic FROM old_daily_entries
             UNION SELECT topic FROM old_weekly_totals
             ORDER BY topic
             ON CONFLICT(name) DO NOTHING;
         INSERT INTO daily_entries(date, topic_id, minutes)
             SELECT entry.date, (SELECT id FROM topics WHERE name = entry.topic), entry.minutes
             FROM old_daily_entries AS entry;
         INSERT INTO weekly_totals(iso_year, iso_week, topic_id, minutes)
             SELECT entry.iso_year, entry.iso_week,
                    (SELECT id FROM topics WHERE name = entry.topic), entry.minutes
             FROM old_weekly_totals AS entry;
         DROP TABLE old_daily_entries;
         DROP TABLE old_weekly_totals;
         DROP TABLE old_topics;",
    )?;
    Ok(())
}

fn verify_database(connection: &Connection) -> Result<()> {
    let quick_check: String =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if quick_check != "ok" {
        bail!("Database integrity check failed: {quick_check}");
    }

    let application_id: i64 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if application_id != DATABASE_APPLICATION_ID {
        bail!("This file is not a Kronos database (application ID {application_id})");
    }

    let expected_tables = ["daily_entries", "topics", "weekly_totals"];
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    if tables.iter().map(String::as_str).collect::<Vec<_>>() != expected_tables {
        bail!("This file does not contain the expected Kronos tables");
    }

    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns)
}

fn verify_columns(connection: &Connection, expected_columns: TableColumns) -> Result<()> {
    for (table, expected) in expected_columns {
        let actual = table_columns(connection, table)?;
        if actual
            .iter()
            .map(|(name, data_type)| (name.as_str(), data_type.as_str()))
            .ne(expected.iter().copied())
        {
            bail!("Database table \"{table}\" has an unexpected schema");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Store, tests::temporary_directory};
    use std::{fs, path::Path};

    fn create_name_key_database(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection
            .pragma_update(None, "application_id", DATABASE_APPLICATION_ID)
            .unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 CREATE TABLE topics(name TEXT PRIMARY KEY COLLATE NOCASE);
                 CREATE TABLE daily_entries(
                     date TEXT NOT NULL,
                     topic TEXT NOT NULL COLLATE NOCASE,
                     minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 4294967295),
                     PRIMARY KEY (date, topic),
                     FOREIGN KEY (topic) REFERENCES topics(name)
                 );
                 CREATE TABLE weekly_totals(
                     iso_year INTEGER NOT NULL,
                     iso_week INTEGER NOT NULL CHECK (iso_week BETWEEN 1 AND 53),
                     topic TEXT NOT NULL COLLATE NOCASE,
                     minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 4294967295),
                     PRIMARY KEY (iso_year, iso_week, topic),
                     FOREIGN KEY (topic) REFERENCES topics(name)
                 );
                 INSERT INTO topics VALUES ('rust'), ('unused');
                 INSERT INTO daily_entries VALUES ('2026-11-18', 'RUST', 15);
                 INSERT INTO weekly_totals VALUES (2026, 46, 'rust', 60), (2026, 46, 'systems', 30);",
            )
            .unwrap();
        connection
    }

    #[test]
    fn opening_a_name_key_database_migrates_daily_and_weekly_entries_once() {
        let directory = temporary_directory();
        let path = directory.join("kronos.sqlite3");
        drop(create_name_key_database(&path));

        for _ in 0..2 {
            let store = Store::open_existing(&path).unwrap();
            assert_eq!(store.topics().unwrap(), ["rust", "systems", "unused"]);
            let daily = store.load_current().unwrap();
            assert_eq!(daily.len(), 1);
            assert_eq!(daily[0].topic, "rust");
            assert_eq!(daily[0].minutes, 15);
            let weekly = store.load_history().unwrap();
            assert_eq!(weekly.len(), 2);
            assert_eq!((weekly[0].topic.as_str(), weekly[0].minutes), ("rust", 60));
            assert_eq!(
                (weekly[1].topic.as_str(), weekly[1].minutes),
                ("systems", 30)
            );
            assert!(
                store
                    .connection
                    .prepare("PRAGMA foreign_key_check")
                    .unwrap()
                    .exists([])
                    .is_ok_and(|has_violations| !has_violations)
            );
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_topic_id_migration_keeps_the_original_tables() {
        let directory = temporary_directory();
        let path = directory.join("kronos.sqlite3");
        let connection = create_name_key_database(&path);
        connection
            .execute("INSERT INTO topics VALUES (NULL)", [])
            .unwrap();
        drop(connection);

        let error = Store::open_existing(&path).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("Could not migrate Kronos topic IDs")
        );
        let connection = Connection::open(path).unwrap();
        verify_database(&connection).unwrap();
        verify_columns(&connection, V1_0_1_COLUMNS).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM weekly_totals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);

        drop(connection);
        fs::remove_dir_all(directory).unwrap();
    }
}
