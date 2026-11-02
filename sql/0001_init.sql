CREATE TABLE IF NOT EXISTS files (
    path                TEXT PRIMARY KEY,
    size                INTEGER NOT NULL,
    created             INTEGER,
    modified            INTEGER,
    atime               INTEGER,
    last_user_access    INTEGER,
    tier                TEXT,
    managed_by_rule     TEXT
);

CREATE INDEX IF NOT EXISTS files_tier_idx ON files(tier);
CREATE INDEX IF NOT EXISTS files_rule_idx ON files(managed_by_rule);

CREATE TABLE IF NOT EXISTS access_events (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    path    TEXT NOT NULL,
    ts      INTEGER NOT NULL,
    source  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS access_events_path_idx ON access_events(path);
CREATE INDEX IF NOT EXISTS access_events_ts_idx   ON access_events(ts);

CREATE TABLE IF NOT EXISTS actions_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    from_path   TEXT,
    to_path     TEXT,
    rule_name   TEXT,
    success     INTEGER NOT NULL,
    error       TEXT
);

CREATE INDEX IF NOT EXISTS actions_log_ts_idx ON actions_log(ts);

CREATE TABLE IF NOT EXISTS tier_moves (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT NOT NULL,
    from_tier   TEXT,
    to_tier     TEXT NOT NULL,
    ts          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS tier_moves_path_idx ON tier_moves(path);

CREATE TABLE IF NOT EXISTS sweep_runs (
    kind         TEXT PRIMARY KEY,
    last_run_at  INTEGER NOT NULL
);
