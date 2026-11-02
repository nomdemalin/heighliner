// macOS-specific integrations.

use std::path::Path;

#[cfg(target_os = "macos")]
pub fn reveal_in_finder(path: &Path) {
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
}

#[cfg(not(target_os = "macos"))]
pub fn reveal_in_finder(_path: &Path) {}
