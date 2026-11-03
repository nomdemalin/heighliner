-- Migration 0002: perf plumbing.
--
-- Adds a tiny stats_counters table maintained by triggers so update_stats
-- (called after every engine message) stops doing three full-table aggregates.
-- Adds a covering (tier, size) index so bytes_per_tier can answer from the
-- index alone without rowid lookups.

CREATE INDEX IF NOT EXISTS files_tier_size_idx ON files(tier, size);

CREATE TABLE IF NOT EXISTS stats_counters (
    key    TEXT PRIMARY KEY,
    value  INTEGER NOT NULL DEFAULT 0
);

-- Seed rows + backfill from current state. The INSERT OR REPLACE guarantees
-- we end up with correct values whether this migration ran on a fresh DB or
-- one that already has files/actions.
INSERT OR REPLACE INTO stats_counters (key, value) VALUES
    ('file_count',    (SELECT COUNT(*)              FROM files)),
    ('bytes_managed', (SELECT COALESCE(SUM(size),0) FROM files)),
    ('actions_run',   (SELECT COUNT(*)              FROM actions_log WHERE success = 1));

-- Keep counters consistent through INSERT / UPDATE OF size / DELETE on files.
CREATE TRIGGER IF NOT EXISTS files_ai AFTER INSERT ON files
BEGIN
    UPDATE stats_counters SET value = value + 1          WHERE key = 'file_count';
    UPDATE stats_counters SET value = value + NEW.size   WHERE key = 'bytes_managed';
END;

CREATE TRIGGER IF NOT EXISTS files_au AFTER UPDATE OF size ON files
BEGIN
    UPDATE stats_counters SET value = value + (NEW.size - OLD.size)
    WHERE key = 'bytes_managed';
END;

CREATE TRIGGER IF NOT EXISTS files_ad AFTER DELETE ON files
BEGIN
    UPDATE stats_counters SET value = value - 1          WHERE key = 'file_count';
    UPDATE stats_counters SET value = value - OLD.size   WHERE key = 'bytes_managed';
END;

-- actions_run counts successful entries only.
CREATE TRIGGER IF NOT EXISTS actions_log_ai AFTER INSERT ON actions_log
WHEN NEW.success = 1
BEGIN
    UPDATE stats_counters SET value = value + 1 WHERE key = 'actions_run';
END;
