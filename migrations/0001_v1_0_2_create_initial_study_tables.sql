-- Baseline matching the released Kronos v1.0.2 schema.
PRAGMA application_id = 0x53545544;

CREATE TABLE topics (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE
);

CREATE TABLE weekly_totals (
    iso_year INTEGER NOT NULL,
    iso_week INTEGER NOT NULL CHECK (iso_week BETWEEN 1 AND 53),
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 4294967295),
    PRIMARY KEY (iso_year, iso_week, topic_id)
);

CREATE TABLE daily_entries (
    date TEXT NOT NULL,
    topic_id INTEGER NOT NULL REFERENCES topics(id),
    minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 4294967295),
    PRIMARY KEY (date, topic_id)
);
