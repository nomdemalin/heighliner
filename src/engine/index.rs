use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};

use super::config::Tier;

/// SQLite-backed index of every file Heighliner knows about. All queries go
/// through this handle so we can swap pools or add caching later.
pub struct Index {
    conn: Mutex<Connection>,
}

impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;

        // P6: tune for the browser / drill / scan workload.
        //   - WAL + sync=NORMAL keeps writes fast with crash safety.
        //   - 64MB page cache turns full-table scans into memory walks on an
        //     index of our size.
        //   - 256MB mmap lets SQLite skip pread() syscalls on hot pages.
        //   - case_sensitive_like is required for `path LIKE 'prefix/%'` to
        //     use the files.path PRIMARY KEY index. Without this, LIKE scans
        //     the whole table even when the PK could nail the prefix.
        //     (All our queries will still migrate to range form in P7.)
        //   - temp_store MEMORY avoids spilling GROUP BY / ORDER BY sorts.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "cache_size", -65_536i64)?; // 64 MiB
        conn.pragma_update(None, "mmap_size", 268_435_456i64)?; // 256 MiB
        conn.pragma_update(None, "case_sensitive_like", true)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;

        let idx = Self { conn: Mutex::new(conn) };
        idx.migrate()?;
        Ok(idx)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock();
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(include_str!("../../sql/0001_init.sql"))?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            conn.execute_batch(include_str!("../../sql/0002_perf.sql"))?;
            conn.pragma_update(None, "user_version", 2)?;
        }
        Ok(())
    }

    /// P4: O(1) stats reads via the counters table (maintained by triggers).
    pub fn quick_stats(&self) -> Result<(u64, u64, u64)> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT value FROM stats_counters WHERE key = ?1")?;
        let file_count: i64 = stmt.query_row(params!["file_count"], |r| r.get(0))?;
        let bytes: i64 = stmt.query_row(params!["bytes_managed"], |r| r.get(0))?;
        let actions: i64 = stmt.query_row(params!["actions_run"], |r| r.get(0))?;
        Ok((file_count.max(0) as u64, bytes.max(0) as u64, actions.max(0) as u64))
    }

    /// Insert or update a file row from a freshly observed metadata snapshot.
    pub fn upsert_file(&self, row: &FileRow) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO files (path, size, created, modified, atime, last_user_access, tier, managed_by_rule)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET
                size = excluded.size,
                modified = excluded.modified,
                atime = excluded.atime,
                last_user_access = COALESCE(excluded.last_user_access, files.last_user_access),
                tier = excluded.tier,
                managed_by_rule = COALESCE(excluded.managed_by_rule, files.managed_by_rule)",
            params![
                row.path.to_string_lossy(),
                row.size as i64,
                row.created,
                row.modified,
                row.atime,
                row.last_user_access,
                row.tier.map(|t| t.dir_name()),
                row.managed_by_rule,
            ],
        )?;
        Ok(())
    }

    /// P2: batch upsert for the scanner. One transaction, one prepared
    /// statement, all rows inserted at once. 10–50× faster on an initial
    /// scan than looping through `upsert_file` because every autocommit
    /// otherwise fsyncs the WAL.
    pub fn upsert_batch(&self, rows: &[FileRow]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let txn = conn.transaction()?;
        {
            let mut stmt = txn.prepare_cached(
                "INSERT INTO files (path, size, created, modified, atime, last_user_access, tier, managed_by_rule)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(path) DO UPDATE SET
                    size = excluded.size,
                    modified = excluded.modified,
                    atime = excluded.atime,
                    last_user_access = COALESCE(excluded.last_user_access, files.last_user_access),
                    tier = excluded.tier,
                    managed_by_rule = COALESCE(excluded.managed_by_rule, files.managed_by_rule)",
            )?;
            for row in rows {
                stmt.execute(params![
                    row.path.to_string_lossy(),
                    row.size as i64,
                    row.created,
                    row.modified,
                    row.atime,
                    row.last_user_access,
                    row.tier.map(|t| t.dir_name()),
                    row.managed_by_rule,
                ])?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    pub fn log_action(&self, entry: &ActionLog) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO actions_log (ts, kind, from_path, to_path, rule_name, success, error)
             VALUES (strftime('%s','now'), ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.kind,
                entry.from_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                entry.to_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                entry.rule_name,
                entry.success as i32,
                entry.error.as_deref(),
            ],
        )?;
        Ok(())
    }

    /// Totals bytes per tier across the whole index.
    pub fn bytes_per_tier(&self) -> Result<Vec<(Tier, u64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT tier, COALESCE(SUM(size), 0) FROM files WHERE tier IS NOT NULL GROUP BY tier",
        )?;
        let mut out = Vec::new();
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (name, bytes) = row?;
            if let Some(t) = parse_tier(&name) {
                out.push((t, bytes as u64));
            }
        }
        Ok(out)
    }

    /// Breakdown by the top-level directory immediately under the given root.
    ///
    /// P7+P9: uses a range query on the `files.path` PK index (not LIKE) and
    /// pushes the child-name extraction + aggregation into SQLite. The query
    /// returns one row per (child, tier) — at most O(num_children × 4) rows
    /// — instead of one row per file. On a 100k-file subtree this collapses
    /// the FFI boundary crossing from 100k rows to a few dozen.
    pub fn bytes_per_child(&self, root: &Path) -> Result<Vec<ChildBreakdown>> {
        let (lo, hi) = prefix_range(root);
        let prefix_chars = lo.chars().count() as i64;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "WITH per_tier AS (
                SELECT
                    CASE
                        WHEN instr(substr(path, ?1 + 1), '/') > 0
                        THEN substr(path, ?1 + 1, instr(substr(path, ?1 + 1), '/') - 1)
                        ELSE substr(path, ?1 + 1)
                    END AS child_name,
                    COALESCE(tier, 'frozen') AS tier_str,
                    size
                FROM files
                WHERE path >= ?2 AND path < ?3
             )
             SELECT child_name, tier_str, COALESCE(SUM(size), 0), COUNT(*)
             FROM per_tier
             WHERE child_name != ''
             GROUP BY child_name, tier_str",
        )?;

        use std::collections::HashMap;
        let mut agg: HashMap<String, ChildAggregate> = HashMap::new();
        let rows = stmt.query_map(params![prefix_chars, lo, hi], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (child, tier_str, size_sum, file_cnt) = row?;
            let tier = parse_tier(&tier_str).unwrap_or(Tier::Frozen);
            let entry = agg.entry(child).or_default();
            entry.total_bytes += size_sum as u64;
            entry.file_count += file_cnt as u64;
            *entry.by_tier.entry(tier).or_default() += size_sum as u64;
        }

        let mut out: Vec<ChildBreakdown> = agg
            .into_iter()
            .map(|(name, a)| {
                let dominant = a
                    .by_tier
                    .iter()
                    .max_by_key(|(_, b)| **b)
                    .map(|(t, _)| *t)
                    .unwrap_or(Tier::Frozen);
                ChildBreakdown {
                    name,
                    bytes: a.total_bytes,
                    files: a.file_count,
                    dominant_tier: dominant,
                }
            })
            .collect();
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        Ok(out)
    }

    /// Files under a prefix, with explicit sort. P7: uses a range form on
    /// the files.path PK index instead of LIKE.
    pub fn files_under_sorted(
        &self,
        prefix: &Path,
        sort: FileSort,
        descending: bool,
        limit: u32,
    ) -> Result<Vec<FileRow>> {
        let (lo, hi) = prefix_range(prefix);
        let conn = self.conn.lock();
        let order_col = match sort {
            FileSort::Size => "size",
            FileSort::Modified => "COALESCE(modified, 0)",
            FileSort::LastAccess => "COALESCE(last_user_access, 0)",
            FileSort::Path => "path",
        };
        let dir = if descending { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT path, size, created, modified, atime, last_user_access, tier, managed_by_rule
             FROM files WHERE path >= ?1 AND path < ?2 ORDER BY {order_col} {dir} LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params![lo, hi, limit], |r| {
                Ok(FileRow {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    size: r.get::<_, i64>(1)? as u64,
                    created: r.get(2)?,
                    modified: r.get(3)?,
                    atime: r.get(4)?,
                    last_user_access: r.get(5)?,
                    tier: r.get::<_, Option<String>>(6)?.and_then(|s| parse_tier(&s)),
                    managed_by_rule: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Total bytes + file count under a prefix. P7: range form on the PK.
    pub fn totals_under(&self, prefix: &Path) -> Result<(u64, u64)> {
        let (lo, hi) = prefix_range(prefix);
        let conn = self.conn.lock();
        let row: (i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM files WHERE path >= ?1 AND path < ?2",
            rusqlite::params![lo, hi],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((row.0 as u64, row.1 as u64))
    }

    pub fn files_in_tier(&self, tier: Tier, limit: u32) -> Result<Vec<FileRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT path, size, created, modified, atime, last_user_access, tier, managed_by_rule
             FROM files WHERE tier = ?1 ORDER BY last_user_access ASC NULLS FIRST LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![tier.dir_name(), limit], |r| {
                Ok(FileRow {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    size: r.get::<_, i64>(1)? as u64,
                    created: r.get(2)?,
                    modified: r.get(3)?,
                    atime: r.get(4)?,
                    last_user_access: r.get(5)?,
                    tier: r.get::<_, Option<String>>(6)?.and_then(|s| parse_tier(&s)),
                    managed_by_rule: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_path(&self, from: &Path, to: &Path) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE files SET path = ?1 WHERE path = ?2",
            params![to.to_string_lossy(), from.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn delete_path(&self, path: &Path) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn set_tier(&self, path: &Path, tier: Tier) -> Result<()> {
        let conn = self.conn.lock();
        let current: Option<String> = conn
            .query_row(
                "SELECT tier FROM files WHERE path = ?1",
                params![path.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?;
        conn.execute(
            "UPDATE files SET tier = ?1 WHERE path = ?2",
            params![tier.dir_name(), path.to_string_lossy()],
        )?;
        conn.execute(
            "INSERT INTO tier_moves (path, from_tier, to_tier, ts) VALUES (?1, ?2, ?3, strftime('%s','now'))",
            params![path.to_string_lossy(), current, tier.dir_name()],
        )?;
        Ok(())
    }

    /// F20: atomic commit of a managed move. Updates files.path+tier+rule and
    /// writes tier_moves in a single transaction so UI readers never see
    /// torn state.
    pub fn commit_managed_move(
        &self,
        from: &Path,
        to: &Path,
        tier: Tier,
        rule_name: &str,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let txn = conn.transaction()?;
        let current: Option<String> = txn
            .query_row(
                "SELECT tier FROM files WHERE path = ?1",
                params![from.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?;
        txn.execute(
            "UPDATE files SET path = ?1, tier = ?2, managed_by_rule = ?3 WHERE path = ?4",
            params![
                to.to_string_lossy(),
                tier.dir_name(),
                rule_name,
                from.to_string_lossy()
            ],
        )?;
        txn.execute(
            "INSERT INTO tier_moves (path, from_tier, to_tier, ts) VALUES (?1, ?2, ?3, strftime('%s','now'))",
            params![to.to_string_lossy(), current, tier.dir_name()],
        )?;
        txn.commit()?;
        Ok(())
    }

    /// F10: unix-ts of the last successful run of a named sweep, or None.
    pub fn last_sweep_at(&self, kind: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock();
        let ts: Option<i64> = conn
            .query_row(
                "SELECT last_run_at FROM sweep_runs WHERE kind = ?1",
                params![kind],
                |r| r.get(0),
            )
            .optional()?;
        Ok(ts)
    }

    pub fn record_sweep(&self, kind: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO sweep_runs (kind, last_run_at) VALUES (?1, strftime('%s','now'))
             ON CONFLICT(kind) DO UPDATE SET last_run_at = strftime('%s','now')",
            params![kind],
        )?;
        Ok(())
    }

    pub fn set_managed_rule(&self, path: &Path, rule_name: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE files SET managed_by_rule = ?1 WHERE path = ?2",
            params![rule_name, path.to_string_lossy()],
        )?;
        Ok(())
    }

    pub fn all_managed_files(&self) -> Result<Vec<FileRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT path, size, created, modified, atime, last_user_access, tier, managed_by_rule
             FROM files",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(FileRow {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    size: r.get::<_, i64>(1)? as u64,
                    created: r.get(2)?,
                    modified: r.get(3)?,
                    atime: r.get(4)?,
                    last_user_access: r.get(5)?,
                    tier: r.get::<_, Option<String>>(6)?.and_then(|s| parse_tier(&s)),
                    managed_by_rule: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn recent_actions(&self, limit: u32) -> Result<Vec<ActionRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT ts, kind, from_path, to_path, rule_name, success, error
             FROM actions_log ORDER BY ts DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok(ActionRow {
                    ts: r.get(0)?,
                    kind: r.get(1)?,
                    from_path: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
                    to_path: r.get::<_, Option<String>>(3)?.map(PathBuf::from),
                    rule_name: r.get(4)?,
                    success: r.get::<_, i32>(5)? != 0,
                    error: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// Bounds for a prefix range query: `path >= lo AND path < hi`. The upper
/// bound is `lo` with the final `/` (0x2F) swapped for `0` (0x30, the next
/// ASCII codepoint), so the whole subtree sits between the two strings.
fn prefix_range(prefix: &Path) -> (String, String) {
    let mut lo = prefix.to_string_lossy().into_owned();
    if !lo.ends_with('/') {
        lo.push('/');
    }
    let mut hi = lo.clone();
    hi.pop();
    hi.push('0');
    (lo, hi)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSort {
    Size,
    Modified,
    LastAccess,
    Path,
}

#[derive(Debug, Clone)]
pub struct FileRow {
    pub path: PathBuf,
    pub size: u64,
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub atime: Option<i64>,
    pub last_user_access: Option<i64>,
    pub tier: Option<Tier>,
    pub managed_by_rule: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ActionLog {
    pub kind: String,
    pub from_path: Option<PathBuf>,
    pub to_path: Option<PathBuf>,
    pub rule_name: Option<String>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChildBreakdown {
    pub name: String,
    pub bytes: u64,
    pub files: u64,
    pub dominant_tier: Tier,
}

#[derive(Default)]
struct ChildAggregate {
    total_bytes: u64,
    file_count: u64,
    by_tier: std::collections::HashMap<Tier, u64>,
}

fn parse_tier(s: &str) -> Option<Tier> {
    match s {
        "hot" => Some(Tier::Hot),
        "warm" => Some(Tier::Warm),
        "cold" => Some(Tier::Cold),
        "frozen" => Some(Tier::Frozen),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ActionRow {
    pub ts: i64,
    pub kind: String,
    pub from_path: Option<PathBuf>,
    pub to_path: Option<PathBuf>,
    pub rule_name: Option<String>,
    pub success: bool,
    pub error: Option<String>,
}
