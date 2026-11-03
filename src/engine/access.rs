// macOS-specific last-access signals.
//
// P1/P17: the scanner's hot path reads st_atime from a single `fs::metadata`
// call now; the atime/mtime/btime helpers that used to live here were just
// extra `fs::metadata()` syscalls. They're gone. What stays is the Spotlight
// `kMDItemLastUsedDate` reader (F6), reserved for a future targeted call site
// — e.g. right before promoting a file to the frozen tier — where paying one
// fork+exec per candidate is acceptable.

use std::path::Path;
use std::process::Command;

/// Shell out to `mdls -raw -name kMDItemLastUsedDate <path>`. Returns the
/// parsed unix timestamp when LaunchServices has seen the file opened, else
/// `None` (which is common for files only touched by scripts/CLIs).
#[allow(dead_code)]
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
    let dt = chrono::DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S %z").ok()?;
    Some(dt.timestamp())
}

#[allow(dead_code)]
#[cfg(not(target_os = "macos"))]
pub fn spotlight_last_used(_path: &Path) -> Option<i64> {
    None
}
