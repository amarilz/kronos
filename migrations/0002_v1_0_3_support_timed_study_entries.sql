-- Preserve manual entries while adding persistent timers to the daily table.
ALTER TABLE daily_entries RENAME TO old_daily_entries;

CREATE TABLE daily_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL,
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    minutes INTEGER,
    started_at INTEGER,
    ended_at INTEGER,
    CHECK ((minutes IS NOT NULL AND minutes BETWEEN 1 AND 4294967295
            AND started_at IS NULL AND ended_at IS NULL)
        OR (minutes IS NULL AND started_at IS NOT NULL
            AND (ended_at IS NULL OR ended_at >= started_at)))
);
CREATE INDEX daily_entries_by_date ON daily_entries(date);
CREATE UNIQUE INDEX one_active_daily_timer
    ON daily_entries ((1)) WHERE minutes IS NULL AND ended_at IS NULL;

INSERT INTO daily_entries(date, topic_id, minutes)
    SELECT date, topic_id, minutes FROM old_daily_entries;
DROP TABLE old_daily_entries;
