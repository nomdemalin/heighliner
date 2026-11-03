use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use super::config::{Tier, TierPolicy};
use super::index::{FileRow, Index};

/// P1: build a FileRow from a path and a *pre-read* Metadata, so callers that
/// already have the metadata (ingest_and_apply) don't incur a second
/// `fs::metadata` syscall. One stat() call per file across the whole hot path.
///
/// P17: `best_last_access` → `atime` is the fallback chain; the old
/// `.or(atime)` outer-OR was a no-op after F6 dropped Spotlight from the hot
/// path.
pub fn build_row(path: &Path, md: &Metadata, tiers: &TierPolicy) -> FileRow {
    let atime = md.atime();
    let mtime = md.mtime();
    let btime = md
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let last_access = atime; // single stat already gives this; no extra signal in the hot path
    let now = chrono::Utc::now().timestamp();
    let age = Duration::from_secs((now - last_access).max(0) as u64);
    let tier = tiers.tier_for_age(age);
    FileRow {
        path: path.to_path_buf(),
        size: md.len(),
        created: btime,
        modified: Some(mtime),
        atime: Some(atime),
        last_user_access: Some(last_access),
        tier: Some(tier),
        managed_by_rule: None,
    }
}

#[allow(dead_code)]
fn _unused(_: Tier, _: &Index) {}
