//! SQLite schema migrations, embedded in the binary in ascending schema order.
//!
//! Name new scripts `NNNN_vX_Y_Z_description.sql` and append them to `SCHEMA_MIGRATIONS`.
//! The prefix maps to SQLite's `user_version`; the app version records the release
//! that introduced the change. Keep released scripts unchanged. The legacy scripts
//! only import databases from before Kronos tracked schema versions.

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use rusqlite_migration::{M, Migrations};
use std::sync::LazyLock;

const DATABASE_APPLICATION_ID: i64 = 0x5354_5544;
const V1_0_2_BASELINE_SCHEMA_VERSION: i64 = 1;
const V1_0_3_TIMED_STUDY_SCHEMA_VERSION: i64 = 2;

const V1_0_2_CREATE_INITIAL_STUDY_TABLES_SQL: &str =
    include_str!("../../migrations/0001_v1_0_2_create_initial_study_tables.sql");
const V1_0_3_SUPPORT_TIMED_STUDY_ENTRIES_SQL: &str =
    include_str!("../../migrations/0002_v1_0_3_support_timed_study_entries.sql");
static SCHEMA_MIGRATIONS: LazyLock<Migrations<'static>> = LazyLock::new(|| {
    Migrations::new(vec![
        M::up(V1_0_2_CREATE_INITIAL_STUDY_TABLES_SQL),
        M::up(V1_0_3_SUPPORT_TIMED_STUDY_ENTRIES_SQL),
    ])
});

const V1_0_3_TIMED_STUDY_COLUMNS: TableColumns = &[
    ("topics", &[("id", "INTEGER"), ("name", "TEXT")]),
    (
        "daily_entries",
        &[
            ("id", "INTEGER"),
            ("date", "TEXT"),
            ("topic_id", "INTEGER"),
            ("minutes", "INTEGER"),
            ("started_at", "INTEGER"),
            ("ended_at", "INTEGER"),
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

type TableColumns = &'static [(&'static str, &'static [(&'static str, &'static str)])];

const V1_0_1_NAME_KEY_COLUMNS: TableColumns = &[
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

const V1_0_2_MANUAL_STUDY_COLUMNS: TableColumns = &[
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

pub(super) fn initialize_new_database(connection: &mut Connection) -> Result<()> {
    SCHEMA_MIGRATIONS.to_latest(connection)?;
    verify_columns(connection, V1_0_3_TIMED_STUDY_COLUMNS)
}

pub(super) fn upgrade_existing_database(connection: &mut Connection) -> Result<()> {
    register_legacy_database_at_v1_0_2_baseline(connection)?;
    if let Err(error) = SCHEMA_MIGRATIONS.to_latest(connection) {
        // Another opener can finish the migration after the runner reads user_version.
        if database_schema_version(connection)? != V1_0_3_TIMED_STUDY_SCHEMA_VERSION {
            return Err(error).context("Could not apply Kronos schema migrations");
        }
    }
    verify_columns(connection, V1_0_3_TIMED_STUDY_COLUMNS)
}

fn register_legacy_database_at_v1_0_2_baseline(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    verify_database(&transaction)?;
    match database_schema_version(&transaction)? {
        0 => {
            if table_columns(&transaction, "topics")? == [("name".into(), "TEXT".into())] {
                import_v1_0_1_name_key_database_into_v1_0_2_baseline(&transaction)
                    .context("Could not migrate Kronos topic IDs")?;
            }
            verify_columns(&transaction, V1_0_2_MANUAL_STUDY_COLUMNS)?;
            transaction.pragma_update(None, "user_version", V1_0_2_BASELINE_SCHEMA_VERSION)?;
        }
        V1_0_2_BASELINE_SCHEMA_VERSION => {
            verify_columns(&transaction, V1_0_2_MANUAL_STUDY_COLUMNS)?
        }
        V1_0_3_TIMED_STUDY_SCHEMA_VERSION => {
            // An older file can already have user_version = 2 with the manual-only schema.
            // Only reclassify it if all its study tables match the v1.0.2 baseline.
            if verify_columns(&transaction, V1_0_2_MANUAL_STUDY_COLUMNS).is_ok() {
                transaction.pragma_update(None, "user_version", V1_0_2_BASELINE_SCHEMA_VERSION)?;
            } else {
                verify_columns(&transaction, V1_0_3_TIMED_STUDY_COLUMNS)?;
            }
        }
        version => bail!("Unsupported Kronos database schema version {version}"),
    }
    transaction.commit()?;
    Ok(())
}

fn import_v1_0_1_name_key_database_into_v1_0_2_baseline(
    transaction: &Transaction<'_>,
) -> Result<()> {
    verify_columns(transaction, V1_0_1_NAME_KEY_COLUMNS)?;
    transaction.execute_batch(include_str!(
        "../../migrations/legacy/v1_0_1_to_v1_0_2_rename_name_key_tables.sql"
    ))?;
    transaction.execute_batch(V1_0_2_CREATE_INITIAL_STUDY_TABLES_SQL)?;
    transaction.execute_batch(include_str!(
        "../../migrations/legacy/v1_0_1_to_v1_0_2_copy_entries_with_topic_ids.sql"
    ))?;
    Ok(())
}

fn database_schema_version(connection: &Connection) -> Result<i64> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
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
    let tables = tables.iter().map(String::as_str).collect::<Vec<_>>();
    if tables != expected_tables {
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
    fn new_database_records_the_latest_schema_version() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_new_database(&mut connection).unwrap();
        assert_eq!(
            database_schema_version(&connection).unwrap(),
            V1_0_3_TIMED_STUDY_SCHEMA_VERSION
        );
        verify_database(&connection).unwrap();
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
            assert_eq!(daily[0].duration_ms, 15 * 60_000);
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
    fn opening_v1_0_2_preserves_daily_entries_and_weekly_totals() {
        assert_v1_0_2_data_survives_upgrade(0);
    }

    #[test]
    fn opening_v1_0_2_with_a_conflicting_user_version_still_migrates() {
        assert_v1_0_2_data_survives_upgrade(V1_0_3_TIMED_STUDY_SCHEMA_VERSION);
    }

    fn assert_v1_0_2_data_survives_upgrade(legacy_user_version: i64) {
        let directory = temporary_directory();
        let path = directory.join("kronos.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "application_id", DATABASE_APPLICATION_ID)
            .unwrap();
        connection
            .execute_batch(V1_0_2_CREATE_INITIAL_STUDY_TABLES_SQL)
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO topics VALUES (1, 'rust');
             INSERT INTO daily_entries VALUES ('2026-11-18', 1, 15);
             INSERT INTO weekly_totals VALUES (2026, 46, 1, 60);",
            )
            .unwrap();
        connection
            .pragma_update(None, "user_version", legacy_user_version)
            .unwrap();
        drop(connection);

        for _ in 0..2 {
            let store = Store::open_existing(&path).unwrap();
            assert_eq!(store.load_current().unwrap()[0].duration_ms, 15 * 60_000);
            assert_eq!(store.load_history().unwrap()[0].minutes, 60);
            assert!(store.active_timer().unwrap().is_none());
            verify_columns(&store.connection, V1_0_3_TIMED_STUDY_COLUMNS).unwrap();
            assert_eq!(
                database_schema_version(&store.connection).unwrap(),
                V1_0_3_TIMED_STUDY_SCHEMA_VERSION
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
        assert_eq!(database_schema_version(&connection).unwrap(), 0);
        verify_columns(&connection, V1_0_1_NAME_KEY_COLUMNS).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM weekly_totals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);

        drop(connection);
        fs::remove_dir_all(directory).unwrap();
    }
}
