ALTER TABLE daily_entries ADD COLUMN last_seen_at INTEGER
    CHECK (last_seen_at IS NULL OR (started_at IS NOT NULL AND last_seen_at >= started_at));
