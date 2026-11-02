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
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

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
        Ok(())
    }

    pub fn file_count(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn action_count(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM actions_log WHERE success = 1", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn total_bytes(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn
            .query_row("SELECT COALESCE(SUM(size), 0) FROM files", [], |r| r.get(0))?;
        Ok(n as u64)
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
    /// If `root = /Users/alice/Downloads`, returns rows for each entry under
    /// that path with the sum of sizes and the "dominant" tier (max bytes).
    pub fn bytes_per_child(&self, root: &Path) -> Result<Vec<ChildBreakdown>> {
        let conn = self.conn.lock();
        let root_s = {
            let mut s = root.to_string_lossy().into_owned();
            if !s.ends_with('/') {
                s.push('/');
            }
            s
        };
        let like = format!("{}%", root_s);
        let mut stmt = conn.prepare(
            "SELECT path, size, COALESCE(tier, 'frozen') FROM files WHERE path LIKE ?1",
        )?;
        let rows = stmt.query_map([like], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;

        use std::collections::HashMap;
        let mut agg: HashMap<String, ChildAggregate> = HashMap::new();
        for row in rows {
            let (path, size, tier_str) = row?;
            let rest = &path[root_s.len()..];
            let child = match rest.split_once('/') {
                Some((head, _)) => head.to_string(),
                None => rest.to_string(),
            };
            if child.is_empty() {
                continue;
            }
            let tier = parse_tier(&tier_str).unwrap_or(Tier::Frozen);
            let entry = agg.entry(child).or_default();
            entry.total_bytes += size as u64;
            entry.file_count += 1;
            *entry.by_tier.entry(tier).or_default() += size as u64;
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

    /// Files under a given path prefix, sorted by size descending.
    /// Used by the treemap's drill-in view so the user can see which
    /// individual files are eating a given subfolder's bytes.
    pub fn files_under(&self, prefix: &Path, limit: u32) -> Result<Vec<FileRow>> {
        let conn = self.conn.lock();
        let mut p = prefix.to_string_lossy().into_owned();
        if !p.ends_with('/') {
            p.push('/');
        }
        let like = format!("{}%", p);
        let mut stmt = conn.prepare(
            "SELECT path, size, created, modified, atime, last_user_access, tier, managed_by_rule
             FROM files WHERE path LIKE ?1 ORDER BY size DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![like, limit], |r| {
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
