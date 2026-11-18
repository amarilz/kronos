# kronos

A small, offline terminal learning tracker. Run it with:

```sh
cargo run
```

The executable is named `kronos`. On the first run, the TUI asks whether to open an existing SQLite database or create a new one, then asks for the database file's absolute path. Existing files are checked for integrity and the expected Kronos schema. Creating a database never overwrites an existing
file.

The selected path is saved as `database.path` in `kronos.properties` under the
operating system's user configuration directory, so setup is not repeated on
later runs. The UTF-8 properties file supports additional `key=value` settings
for future configuration. Use `kronos --configure-db` to select a different
database.

For scripts or a one-time override, pass an existing database with `--db` or
set `KRONOS_DB_PATH`:

```sh
kronos --db /absolute/path/to/kronos.sqlite3
KRONOS_DB_PATH=/absolute/path/to/kronos.sqlite3 kronos
```

## Weekly storage model

The database contains:

- `topics`: user-defined topic names.
- `daily_entries`: dated entries for the open week.
- `weekly_totals`: completed and retroactively updated weekly aggregates.

Completed weeks are identified by the **ISO year and ISO week number**, stored
as `iso_year` and `iso_week`. Together they uniquely identify a week, including
at year boundaries: for example, Monday 30 December 2024 belongs to ISO week
`2025-W01`. A topic's weekly total is uniquely keyed by
`(iso_year, iso_week, topic)`.

The app rolls over every stale daily week on startup and in the same SQLite
transaction as each new study entry. An entry from several months ago updates
its original ISO week directly; intervening weeks can be empty. Failed
operations roll back without partially applying the rollover or new entry.

## Database location and backups

When creating a database, the setup screen suggests a path under the user's
local data directory:

| Operating system | Suggested path |
| --- | --- |
| macOS | `~/Library/Application Support/kronos/kronos.sqlite3` |
| Linux | `$XDG_DATA_HOME/kronos/kronos.sqlite3`, or `~/.local/share/kronos/kronos.sqlite3` when unset |
| Windows | `%LOCALAPPDATA%\kronos\kronos.sqlite3` |

Parent directories are created when needed. SQLite is embedded, so no database
server is required. Close the app before copying the database file for a
backup; do not copy or edit it while the app is running.
