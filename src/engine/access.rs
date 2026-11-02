use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

/// Best-effort last-access timestamp. Combines our own signals with
/// `kMDItemLastUsedDate` (when LaunchServices has seen the file opened) and
/// falls back to `st_atime`. See [[project-heighliner-architecture]].
pub fn best_last_access(path: &Path) -> Option<i64> {
    spotlight_last_used(path).or_else(|| atime_unix(path))
}

/// Best-effort read of the file's last-access unix timestamp. On macOS this is
/// `st_atime`; we treat the result as advisory because:
///   - noatime mounts return the mount-time atime forever
///   - Spotlight / Time Machine / anti-virus scans bump atime
///   - APFS has quirks around when atime is persisted
pub fn atime_unix(path: &Path) -> Option<i64> {
    let md = std::fs::metadata(path).ok()?;
    Some(md.atime())
}

pub fn mtime_unix(path: &Path) -> Option<i64> {
    let md = std::fs::metadata(path).ok()?;
    Some(md.mtime())
}

pub fn btime_unix(path: &Path) -> Option<i64> {
    let md = std::fs::metadata(path).ok()?;
    md.created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Shell out to `mdls -raw -name kMDItemLastUsedDate <path>`. Returns the
/// parsed unix timestamp when LaunchServices has seen the file opened, else
/// `None` (which is common for files only touched by scripts/CLIs).
#[cfg(target_os = "macos")]
pub fn spotlight_last_used(path: &Path) -> Option<i64> {
    let output = Command::new("mdls")
        .arg("-raw")
        .arg("-name")
        .arg("kMDItemLastUsedDate")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed == "(null)" || trimmed.is_empty() {
        return None;
    }
    // Format examples:
    //   "2024-01-15 09:12:08 +0000"
    //   "2026-10-30 18:04:11 -0700"
    let dt = chrono::DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S %z").ok()?;
    Some(dt.timestamp())
}

#[cfg(not(target_os = "macos"))]
pub fn spotlight_last_used(_path: &Path) -> Option<i64> {
    None
}
