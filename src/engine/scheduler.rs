// Periodic sweeps run from the engine thread on a cadence.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::{Datelike, TimeZone, Timelike};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime as ZipDateTime, ZipWriter};

use super::actions;
use super::config::{Action, ArchiveSchedule, Config, Tier};
use super::index::{ActionLog, Index};
use super::EngineHandle;

const SWEEP_ARCHIVE: &str = "archive";

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
                let rule_name = f.managed_by_rule.clone().unwrap_or_else(|| "retier-sweep".into());
                let _ = h.index.commit_managed_move(&f.path, &new_path, target_tier, &rule_name);
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
/// into `cfg.archive.destination`. Compressed → single zip; uncompressed →
/// mirrored tree. Cadence honored via sweep_runs.last_run_at (F10).
pub fn archive_sweep(h: &EngineHandle) -> Result<()> {
    let cfg = h.config.lock().clone();
    let Some(archive_dest) = cfg.archive.destination.clone() else {
        return Ok(());
    };
    let Some(schedule) = cfg.archive.schedule else {
        return Ok(());
    };
    if !archive_cadence_elapsed(schedule, &h.index)? {
        return Ok(());
    }

    fs::create_dir_all(&archive_dest)
        .with_context(|| format!("create {}", archive_dest.display()))?;

    let frozen_dirs = collect_frozen_dirs(&cfg)?;
    let mut any_ok = false;
    for frozen in frozen_dirs {
        match archive_tree(&frozen, &archive_dest, cfg.archive.compress, cfg.general.dry_run, &h.index) {
            Ok(()) => any_ok = true,
            Err(e) => tracing::warn!(path = %frozen.display(), error = %e, "archive failed"),
        }
    }
    if any_ok && !cfg.general.dry_run {
        h.index.record_sweep(SWEEP_ARCHIVE)?;
    }
    Ok(())
}

fn archive_cadence_elapsed(schedule: ArchiveSchedule, index: &Index) -> Result<bool> {
    let interval = match schedule {
        ArchiveSchedule::Daily => Duration::from_secs(24 * 60 * 60),
        ArchiveSchedule::Weekly => Duration::from_secs(7 * 24 * 60 * 60),
        ArchiveSchedule::Monthly => Duration::from_secs(30 * 24 * 60 * 60),
    };
    let Some(last) = index.last_sweep_at(SWEEP_ARCHIVE)? else {
        return Ok(true);
    };
    let now = chrono::Utc::now().timestamp();
    Ok((now - last) >= interval.as_secs() as i64)
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

/// F2: Apply `DeleteAfter` rule actions. For each managed file whose matching
/// rule is `DeleteAfter { after }`, if its last-access age ≥ `after`, delete
/// it from disk and from the index.
pub fn delete_after_sweep(h: &EngineHandle) -> Result<()> {
    let cfg = h.config.lock().clone();
    let now = chrono::Utc::now().timestamp();
    let files = h.index.all_managed_files()?;
    let mut deleted = 0u64;

    for f in files {
        let Some(rule_name) = f.managed_by_rule.as_deref() else { continue };
        let Some(rule) = cfg.rules.iter().find(|r| r.name == rule_name) else { continue };
        let Action::DeleteAfter { after } = &rule.action else { continue };
        let last = f.last_user_access.unwrap_or(now);
        let age = Duration::from_secs((now - last).max(0) as u64);
        if age < *after {
            continue;
        }

        if cfg.general.dry_run {
            let _ = h.index.log_action(&ActionLog {
                kind: "delete_after_dry_run".into(),
                from_path: Some(f.path.clone()),
                to_path: None,
                rule_name: Some(rule_name.to_string()),
                success: true,
                error: None,
            });
            continue;
        }

        match fs::remove_file(&f.path) {
            Ok(()) => {
                let _ = h.index.delete_path(&f.path);
                let _ = h.index.log_action(&ActionLog {
                    kind: "delete_after".into(),
                    from_path: Some(f.path.clone()),
                    to_path: None,
                    rule_name: Some(rule_name.to_string()),
                    success: true,
                    error: None,
                });
                deleted += 1;
            }
            Err(e) => {
                let _ = h.index.log_action(&ActionLog {
                    kind: "delete_after".into(),
                    from_path: Some(f.path.clone()),
                    to_path: None,
                    rule_name: Some(rule_name.to_string()),
                    success: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    if deleted > 0 {
        tracing::info!(deleted, "delete_after sweep complete");
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
    let unique = actions::unique_sibling(&dest)?;

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
    let options = file_options_from(src)?;
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

    let mut uncompressed = 0u64;
    let mut file_count = 0u64;
    for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        // F3: propagate strip_prefix failure instead of silently writing
        // absolute-path zip entries.
        let rel = entry
            .path()
            .strip_prefix(src)
            .with_context(|| {
                format!(
                    "walkdir yielded a path outside of its root: {} not under {}",
                    entry.path().display(),
                    src.display()
                )
            })?;
        let name = rel.to_string_lossy().into_owned();
        let options = file_options_from(entry.path())?;
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

/// F18: build zip options with source's actual unix permissions + mtime so
/// archived files round-trip with their mode and timestamps intact.
fn file_options_from(src: &Path) -> Result<SimpleFileOptions> {
    let md = fs::metadata(src)?;
    let mode = md.permissions().mode() & 0o7777;
    let mut options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .unix_permissions(mode);
    if let Ok(mtime) = md.modified() {
        if let Some(zdt) = system_time_to_zip_datetime(mtime) {
            options = options.last_modified_time(zdt);
        }
    }
    Ok(options)
}

fn system_time_to_zip_datetime(t: SystemTime) -> Option<ZipDateTime> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let dt = chrono::Local.timestamp_opt(secs, 0).single()?;
    ZipDateTime::from_date_and_time(
        dt.year() as u16,
        dt.month() as u8,
        dt.day() as u8,
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
    .ok()
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
    let unique_dest = actions::unique_sibling(&dest)?;

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
            // F3: propagate strip_prefix failure.
            let rel = entry.path().strip_prefix(src).with_context(|| {
                format!(
                    "walkdir yielded a path outside of its root: {} not under {}",
                    entry.path().display(),
                    src.display()
                )
            })?;
            let dst = unique_dest.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            // F4: on cross-device fallback, copy must be followed by an
            // explicit remove — otherwise the source stays and the next
            // scan re-ingests it, duplicating every archive cycle.
            match fs::rename(entry.path(), &dst) {
                Ok(()) => {}
                Err(_) => {
                    fs::copy(entry.path(), &dst)?;
                    fs::remove_file(entry.path())?;
                }
            }
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
