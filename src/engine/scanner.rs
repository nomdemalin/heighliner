use std::path::Path;

use anyhow::Result;

use super::access;
use super::config::{Config, Tier};
use super::index::{FileRow, Index};

/// Index a single file: read its metadata and upsert into the DB. Called by
/// both the initial walk (in `engine::rescan_once`) and the FS event handler.
pub fn ingest_file(path: &Path, config: &Config, index: &Index) -> Result<()> {
    let md = std::fs::metadata(path)?;
    if !md.is_file() {
        return Ok(());
    }
    let atime = access::atime_unix(path);
    let last_access = access::best_last_access(path)
        .or(atime)
        .unwrap_or_else(|| chrono::Utc::now().timestamp());
    let now = chrono::Utc::now().timestamp();
    let age = std::time::Duration::from_secs((now - last_access).max(0) as u64);
    let tier = tier_for(config, age);
    let row = FileRow {
        path: path.to_path_buf(),
        size: md.len(),
        created: access::btime_unix(path),
        modified: access::mtime_unix(path),
        atime,
        last_user_access: Some(last_access),
        tier: Some(tier),
        managed_by_rule: None,
    };
    index.upsert_file(&row)?;
    Ok(())
}

fn tier_for(config: &Config, age: std::time::Duration) -> Tier {
    config.tiers.tier_for_age(age)
}
