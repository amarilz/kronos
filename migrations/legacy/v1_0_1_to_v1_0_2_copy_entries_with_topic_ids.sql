INSERT INTO topics(name) SELECT name FROM old_topics ORDER BY name;
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
DROP TABLE old_topics;
