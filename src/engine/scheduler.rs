// Periodic sweeps run from the engine thread on a cadence.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::actions;
use super::config::{ArchiveSchedule, Config, Tier};
use super::index::{ActionLog, Index};
use super::EngineHandle;

/// Walk every managed file; if its age bucket differs from its stored tier,
/// move it between hot/warm/cold/frozen subdirs under its managed destination.
pub fn retier_sweep(h: &EngineHandle) -> Result<()> {
    let cfg = h.config.lock().clone();
    let files = h.index.all_managed_files()?;
    let now = chrono::Utc::now().timestamp();
    let mut moved = 0u64;

    for f in files {
        let last = f.last_user_access.unwrap_or(now);
        let age = Duration::from_secs((now - last).max(0) as u64);
        let target_tier = cfg.tiers.tier_for_age(age);
        let Some(current) = f.tier else { continue };
        if current == target_tier {
            continue;
        }

        // Figure out where the managed destination root is: the file lives in
        // <root>/<tier>/<name>. Strip the current tier dir to find <root>.
        let Some(parent) = f.path.parent() else { continue };
        let Some(parent_name) = parent.file_name().and_then(|n| n.to_str()) else { continue };
        if parent_name != current.dir_name() {
            // File hasn't been placed into a tier subdir — just update its
            // record so future queries show the right tier.
            let _ = h.index.set_tier(&f.path, target_tier);
            continue;
        }
        let Some(root) = parent.parent() else { continue };
        let dest = root.join(target_tier.dir_name()).join(
            f.path.file_name().unwrap_or_default(),
        );

        match actions::safe_move(&f.path, &dest, cfg.general.dry_run, &h.index, Some("retier-sweep")) {
            Ok(new_path) => {
                let _ = h.index.update_path(&f.path, &new_path);
                let _ = h.index.set_tier(&new_path, target_tier);
                moved += 1;
            }
            Err(e) => tracing::warn!(path = %f.path.display(), error = %e, "retier move failed"),
        }
    }

    if moved > 0 {
        tracing::info!(moved, "retier sweep complete");
    }
    Ok(())
}

/// Archive the contents of every `frozen/` subdir under managed destinations
/// into `cfg.archive.destination`. If `compress` is true, write a single
/// `<rootname>-YYYY-MM-DD.zip`; otherwise just copy preserving structure.
pub fn archive_sweep(h: &EngineHandle) -> Result<()> {
    let cfg = h.config.lock().clone();
    let Some(archive_dest) = cfg.archive.destination.clone() else {
        return Ok(());
    };
    let Some(schedule) = cfg.archive.schedule else {
        return Ok(());
    };
    if !archive_cadence_elapsed(schedule, h)? {
        return Ok(());
    }

    fs::create_dir_all(&archive_dest)
        .with_context(|| format!("create {}", archive_dest.display()))?;

    let frozen_dirs = collect_frozen_dirs(&cfg)?;
    for frozen in frozen_dirs {
        if let Err(e) = archive_tree(&frozen, &archive_dest, cfg.archive.compress, cfg.general.dry_run, &h.index) {
            tracing::warn!(path = %frozen.display(), error = %e, "archive failed");
        }
    }
    Ok(())
}

fn archive_cadence_elapsed(schedule: ArchiveSchedule, _h: &EngineHandle) -> Result<bool> {
    // Simple heuristic: always fire. The engine ticker gates us at 24h so
    // Daily is correct; Weekly/Monthly fire more often than ideal in v1. Add
    // a last_archive timestamp table when we care enough.
    let _ = schedule;
    Ok(true)
}

/// Delete frozen files older than `cfg.archive.purge_frozen_after`.
pub fn purge_sweep(h: &EngineHandle) -> Result<()> {
    let cfg = h.config.lock().clone();
    let Some(cutoff) = cfg.archive.purge_frozen_after else {
        return Ok(());
    };
    let now = chrono::Utc::now().timestamp();
    let cutoff_secs = cutoff.as_secs() as i64;

    let files = h.index.files_in_tier(Tier::Frozen, 100_000)?;
    let mut deleted = 0u64;
    for f in files {
        let last = f.last_user_access.unwrap_or(now);
        if (now - last) < cutoff_secs {
            continue;
        }
        if cfg.general.dry_run {
            let _ = h.index.log_action(&ActionLog {
                kind: "purge_dry_run".into(),
                from_path: Some(f.path.clone()),
                to_path: None,
                rule_name: Some("purge-sweep".into()),
                success: true,
                error: None,
            });
            continue;
        }
        if let Err(e) = fs::remove_file(&f.path) {
            let _ = h.index.log_action(&ActionLog {
                kind: "purge".into(),
                from_path: Some(f.path.clone()),
                to_path: None,
                rule_name: Some("purge-sweep".into()),
                success: false,
                error: Some(e.to_string()),
            });
            continue;
        }
        let _ = h.index.delete_path(&f.path);
        let _ = h.index.log_action(&ActionLog {
            kind: "purge".into(),
            from_path: Some(f.path.clone()),
            to_path: None,
            rule_name: Some("purge-sweep".into()),
            success: true,
            error: None,
        });
        deleted += 1;
    }
    if deleted > 0 {
        tracing::info!(deleted, "purge sweep complete");
    }
    Ok(())
}

/// Archive one file. When `compress` is true, writes a single-entry
/// `<name>.zip` under `dest_root`; otherwise copies the file.
pub fn archive_file_with(
    src: &Path,
    dest_root: &Path,
    delete_source: bool,
    dry_run: bool,
    compress: bool,
    index: &Index,
    rule_name: Option<&str>,
) -> Result<()> {
    let name = src
        .file_name()
        .map(|n| n.to_owned())
        .context("file has no name")?;
    let dest = if compress {
        dest_root.join(format!(
            "{}.zip",
            Path::new(&name).display()
        ))
    } else {
        dest_root.join(&name)
    };
    if dry_run {
        index.log_action(&ActionLog {
            kind: if compress { "archive_zip_dry_run".into() } else { "archive_dry_run".into() },
            from_path: Some(src.to_path_buf()),
            to_path: Some(dest),
            rule_name: rule_name.map(str::to_string),
            success: true,
            error: None,
        })?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let unique = actions::unique_sibling(&dest);

    let bytes_written = if compress {
        write_single_file_zip(src, &unique)?
    } else {
        fs::copy(src, &unique)?
    };
    tracing::debug!(
        src = %src.display(),
        dest = %unique.display(),
        bytes_written,
        "archive_file complete"
    );

    index.log_action(&ActionLog {
        kind: if compress { "archive_zip".into() } else { "archive".into() },
        from_path: Some(src.to_path_buf()),
        to_path: Some(unique.clone()),
        rule_name: rule_name.map(str::to_string),
        success: true,
        error: None,
    })?;
    if delete_source {
        fs::remove_file(src)?;
        let _ = index.delete_path(src);
    }
    Ok(())
}

fn write_single_file_zip(src: &Path, zip_path: &Path) -> Result<u64> {
    let file = fs::File::create(zip_path)
        .with_context(|| format!("create {}", zip_path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .unix_permissions(0o644);
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .context("non-utf8 filename")?;
    zip.start_file(name.to_string(), options)?;
    let mut buf = Vec::with_capacity(64 * 1024);
    fs::File::open(src)?.read_to_end(&mut buf)?;
    zip.write_all(&buf)?;
    let w = zip.finish()?;
    Ok(w.metadata()?.len())
}

/// Compress every file under `src` into a single zip, preserving relative paths.
fn write_tree_zip(src: &Path, zip_path: &Path) -> Result<ZipTreeReport> {
    let file = fs::File::create(zip_path)
        .with_context(|| format!("create {}", zip_path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .unix_permissions(0o644);

    let mut uncompressed = 0u64;
    let mut file_count = 0u64;
    for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
        let name = rel.to_string_lossy().into_owned();
        zip.start_file(name, options)?;
        let mut f = fs::File::open(entry.path())?;
        let mut buf = Vec::with_capacity(64 * 1024);
        f.read_to_end(&mut buf)?;
        zip.write_all(&buf)?;
        uncompressed += buf.len() as u64;
        file_count += 1;
    }
    let w = zip.finish()?;
    let compressed = w.metadata()?.len();
    Ok(ZipTreeReport {
        file_count,
        uncompressed,
        compressed,
    })
}

#[derive(Debug, Clone, Copy)]
struct ZipTreeReport {
    file_count: u64,
    uncompressed: u64,
    compressed: u64,
}

fn collect_frozen_dirs(cfg: &Config) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for r in &cfg.rules {
        if let super::config::Action::Move { to } = &r.action {
            let p = to.join("frozen");
            if p.is_dir() {
                out.push(p);
            }
        }
    }
    Ok(out)
}

fn archive_tree(src: &Path, dest_root: &Path, compress: bool, dry_run: bool, index: &Index) -> Result<()> {
    let stamp = chrono::Local::now().format("%Y-%m-%d");
    let base_name = src
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("frozen");
    let dest = if compress {
        dest_root.join(format!("{base_name}-{stamp}.zip"))
    } else {
        dest_root.join(format!("{base_name}-{stamp}"))
    };

    if dry_run {
        index.log_action(&ActionLog {
            kind: if compress { "archive_tree_zip_dry_run".into() } else { "archive_tree_dry_run".into() },
            from_path: Some(src.to_path_buf()),
            to_path: Some(dest),
            rule_name: Some("archive-sweep".into()),
            success: true,
            error: None,
        })?;
        return Ok(());
    }

    fs::create_dir_all(dest_root)?;
    let unique_dest = actions::unique_sibling(&dest);

    if compress {
        let report = write_tree_zip(src, &unique_dest)?;
        let ratio = if report.uncompressed > 0 {
            (report.compressed as f64) / (report.uncompressed as f64)
        } else {
            0.0
        };
        tracing::info!(
            files = report.file_count,
            uncompressed = report.uncompressed,
            compressed = report.compressed,
            ratio = ratio,
            "archive_tree zipped"
        );
        // After successful zip, remove source files so `frozen/` is actually flushed.
        for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
            if entry.file_type().is_file() {
                let _ = fs::remove_file(entry.path());
                let _ = index.delete_path(entry.path());
            }
        }
        index.log_action(&ActionLog {
            kind: "archive_tree_zip".into(),
            from_path: Some(src.to_path_buf()),
            to_path: Some(unique_dest),
            rule_name: Some("archive-sweep".into()),
            success: true,
            error: None,
        })?;
    } else {
        fs::create_dir_all(&unique_dest)?;
        for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(src).unwrap_or(entry.path());
            let dst = unique_dest.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(entry.path(), &dst).or_else(|_| fs::copy(entry.path(), &dst).map(|_| ()))?;
            let _ = index.update_path(entry.path(), &dst);
        }
        index.log_action(&ActionLog {
            kind: "archive_tree".into(),
            from_path: Some(src.to_path_buf()),
            to_path: Some(unique_dest),
            rule_name: Some("archive-sweep".into()),
            success: true,
            error: None,
        })?;
    }
    Ok(())
}
