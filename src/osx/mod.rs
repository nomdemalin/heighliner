// macOS-specific integrations.

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Result};

#[cfg(target_os = "macos")]
pub fn reveal_in_finder(path: &Path) {
    let _ = Command::new("open").arg("-R").arg(path).spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn reveal_in_finder(_path: &Path) {}

/// Move a file to the macOS Trash (undoable, native). Uses `osascript` so we
/// don't need to pull in an objc2 bridge for this one call. Blocks on the
/// AppleScript invocation (≈100ms per file); fine for click-driven use.
#[cfg(target_os = "macos")]
pub fn move_to_trash(path: &Path) -> Result<()> {
    let p = path
        .to_str()
        .ok_or_else(|| anyhow!("non-utf8 path cannot be trashed via osascript"))?;
    // Escape any embedded quotes so the AppleScript string stays valid.
    let escaped = p.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "tell application \"Finder\" to delete (POSIX file \"{escaped}\" as alias)"
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("osascript trash failed: {}", stderr.trim()));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn move_to_trash(_path: &Path) -> Result<()> {
    Err(anyhow!("move_to_trash is only implemented on macOS"))
}
