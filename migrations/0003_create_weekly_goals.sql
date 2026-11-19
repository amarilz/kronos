-- Store dated weekly targets independently of recorded study time.
CREATE TABLE weekly_goals (
    starts_on TEXT PRIMARY KEY NOT NULL
        CHECK (date(starts_on) IS NOT NULL AND date(starts_on) = starts_on
               AND strftime('%w', starts_on) = '1'),
    minutes_per_week INTEGER NOT NULL CHECK (minutes_per_week BETWEEN 1 AND 10080)
);
