use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::index::{ActionLog, Index};

/// Move a file, preferring an atomic rename on the same volume; falling back
/// to copy + fsync + unlink. If `dest` already exists, pick a unique sibling
/// (`foo.pdf` → `foo (1).pdf`) rather than clobbering.
pub fn safe_move(src: &Path, dest: &Path, dry_run: bool, index: &Index, rule_name: Option<&str>) -> Result<PathBuf> {
    let final_dest = unique_sibling(dest);
    if dry_run {
        index.log_action(&ActionLog {
            kind: "move_dry_run".into(),
            from_path: Some(src.to_path_buf()),
            to_path: Some(final_dest.clone()),
            rule_name: rule_name.map(str::to_string),
            success: true,
            error: None,
        })?;
        return Ok(final_dest);
    }
    if let Some(parent) = final_dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let moved = match fs::rename(src, &final_dest) {
        Ok(()) => final_dest.clone(),
        Err(e) if is_cross_device(&e) => copy_then_delete(src, &final_dest)?,
        Err(e) => {
            let msg = e.to_string();
            index.log_action(&ActionLog {
                kind: "move".into(),
                from_path: Some(src.to_path_buf()),
                to_path: Some(final_dest.clone()),
                rule_name: rule_name.map(str::to_string),
                success: false,
                error: Some(msg),
            })?;
            return Err(e.into());
        }
    };

    index.log_action(&ActionLog {
        kind: "move".into(),
        from_path: Some(src.to_path_buf()),
        to_path: Some(moved.clone()),
        rule_name: rule_name.map(str::to_string),
        success: true,
        error: None,
    })?;
    Ok(moved)
}

fn is_cross_device(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        if let Some(code) = e.raw_os_error() {
            return code == libc_exdev();
        }
    }
    false
}

#[cfg(unix)]
fn libc_exdev() -> i32 {
    18
}

fn copy_then_delete(src: &Path, dest: &Path) -> Result<PathBuf> {
    fs::copy(src, dest).with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
    // Best effort fsync so a crash doesn't leave a torn file.
    if let Ok(f) = std::fs::File::open(dest) {
        let _ = f.sync_all();
    }
    fs::remove_file(src).with_context(|| format!("remove {}", src.display()))?;
    Ok(dest.to_path_buf())
}

/// Pick `path`, or `path (1)`, `path (2)`, …, whichever doesn't exist.
pub fn unique_sibling(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|s| s.to_str());
    for i in 1..10_000 {
        let candidate = match ext {
            Some(e) => parent.join(format!("{stem} ({i}).{e}")),
            None => parent.join(format!("{stem} ({i})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}
