use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;

pub mod access;
pub mod actions;
pub mod config;
pub mod index;
pub mod paths;
pub mod rules;
pub mod scanner;
pub mod scheduler;
pub mod tiering;
pub mod watcher;

use config::Config;
use index::Index;
use paths::AppPaths;

#[derive(Clone, Copy, Default, Debug)]
pub struct StatsSnapshot {
    pub file_count: u64,
    pub actions_run: u64,
    pub bytes_managed: u64,
}

#[derive(Default)]
struct SharedState {
    stats: StatsSnapshot,
    last_error: Option<String>,
    last_activity: Option<String>,
    scanning: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum SweepKind {
    Rescan,
    Retier,
    Archive,
    Purge,
}

pub enum EngineMsg {
    Fs(watcher::FsSignal),
    Tick(SweepKind),
    ScanNow,
    ReloadConfig,
}

#[derive(Clone)]
pub struct EngineHandle {
    pub paths: AppPaths,
    pub config: Arc<Mutex<Config>>,
    pub index: Arc<Index>,
    shared: Arc<Mutex<SharedState>>,
    msgs: Sender<EngineMsg>,
}

impl EngineHandle {
    pub fn stats_snapshot(&self) -> StatsSnapshot {
        self.shared.lock().stats
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.lock().last_error.clone()
    }

    pub fn last_activity(&self) -> Option<String> {
        self.shared.lock().last_activity.clone()
    }

    pub fn scanning(&self) -> bool {
        self.shared.lock().scanning
    }

    pub fn set_error(&self, msg: impl Into<String>) {
        self.shared.lock().last_error = Some(msg.into());
    }

    pub fn clear_error(&self) {
        self.shared.lock().last_error = None;
    }

    pub fn request_scan_now(&self) {
        let _ = self.msgs.send(EngineMsg::ScanNow);
    }

    pub fn reload_config(&self) {
        let _ = self.msgs.send(EngineMsg::ReloadConfig);
    }

    /// Flip `general.dry_run` and persist. Returns the new value.
    pub fn toggle_dry_run(&self) -> Result<bool> {
        let new_val = {
            let mut cfg = self.config.lock();
            cfg.general.dry_run = !cfg.general.dry_run;
            cfg.save(&self.paths.config_file)?;
            cfg.general.dry_run
        };
        Ok(new_val)
    }
}

pub struct EngineInit {
    pub paths: AppPaths,
    pub config: Config,
    pub index: Arc<Index>,
}

pub struct Engine;

impl Engine {
    pub fn spawn(init: EngineInit) -> EngineHandle {
        let (tx, rx) = unbounded::<EngineMsg>();
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let config = Arc::new(Mutex::new(init.config));
        let handle = EngineHandle {
            paths: init.paths,
            config: config.clone(),
            index: init.index.clone(),
            shared: shared.clone(),
            msgs: tx.clone(),
        };

        let th = handle.clone();
        thread::Builder::new()
            .name("heighliner-engine".into())
            .spawn(move || {
                if let Err(e) = run(th.clone(), rx) {
                    tracing::error!(error = %e, "engine thread exited with error");
                    th.set_error(e.to_string());
                }
            })
            .expect("spawn engine thread");

        start_ticker(tx, handle.clone());
        handle
    }
}

fn run(h: EngineHandle, rx: Receiver<EngineMsg>) -> Result<()> {
    tracing::info!("engine thread starting");
    push_activity(&h, "Starting up");
    rescan_once(&h);
    update_stats(&h);

    let mut _watcher = rebuild_watcher(&h);

    loop {
        let msg = match rx.recv() {
            Ok(m) => m,
            Err(_) => break,
        };
        match msg {
            EngineMsg::Fs(sig) => on_fs(&h, sig),
            EngineMsg::Tick(SweepKind::Rescan) => rescan_once(&h),
            EngineMsg::Tick(SweepKind::Retier) => {
                if let Err(e) = scheduler::retier_sweep(&h) {
                    push_error(&h, format!("retier sweep: {e}"));
                }
            }
            EngineMsg::Tick(SweepKind::Archive) => {
                if let Err(e) = scheduler::archive_sweep(&h) {
                    push_error(&h, format!("archive sweep: {e}"));
                }
            }
            EngineMsg::Tick(SweepKind::Purge) => {
                if let Err(e) = scheduler::purge_sweep(&h) {
                    push_error(&h, format!("purge sweep: {e}"));
                }
            }
            EngineMsg::ScanNow => rescan_once(&h),
            EngineMsg::ReloadConfig => {
                _watcher = rebuild_watcher(&h);
                push_activity(&h, "Reloaded config");
                rescan_once(&h);
            }
        }
        update_stats(&h);
    }
    Ok(())
}

fn rebuild_watcher(h: &EngineHandle) -> Option<watcher::WatcherHandle> {
    let cfg = h.config.lock().clone();
    if cfg.watches.is_empty() {
        return None;
    }
    let tx = h.msgs.clone();
    match watcher::spawn(&cfg, move |sig| {
        let _ = tx.send(EngineMsg::Fs(sig));
    }) {
        Ok(w) => {
            push_activity(h, &format!("Watching {} root(s)", cfg.watches.len()));
            Some(w)
        }
        Err(e) => {
            push_error(h, format!("watcher: {e}"));
            None
        }
    }
}

fn rescan_once(h: &EngineHandle) {
    h.shared.lock().scanning = true;
    let cfg = h.config.lock().clone();
    let mut seen = 0u64;
    let mut errors = 0u64;
    for w in &cfg.watches {
        if !w.path.exists() {
            continue;
        }
        let walker = if w.recursive {
            walkdir::WalkDir::new(&w.path).follow_links(false)
        } else {
            walkdir::WalkDir::new(&w.path).max_depth(1).follow_links(false)
        };
        for entry in walker.into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Err(e) = ingest_and_apply(h, entry.path()) {
                tracing::warn!(path = %entry.path().display(), error = %e, "scan ingest failed");
                errors += 1;
                continue;
            }
            seen += 1;
        }
    }
    push_activity(h, &format!("Scan: {seen} files · {errors} errors"));
    h.shared.lock().scanning = false;
}

fn on_fs(h: &EngineHandle, sig: watcher::FsSignal) {
    use watcher::FsSignal;
    match sig {
        FsSignal::Created(p) | FsSignal::Modified(p) => {
            if let Err(e) = ingest_and_apply(h, &p) {
                tracing::warn!(path = %p.display(), error = %e, "ingest_and_apply failed");
            }
        }
        FsSignal::Renamed { from, to } => {
            let _ = h.index.update_path(&from, &to);
            if let Err(e) = ingest_and_apply(h, &to) {
                tracing::warn!(path = %to.display(), error = %e, "ingest_and_apply failed");
            }
        }
        FsSignal::Removed(p) => {
            let _ = h.index.delete_path(&p);
        }
    }
}

fn ingest_and_apply(h: &EngineHandle, path: &std::path::Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let cfg = h.config.lock().clone();
    scanner::ingest_file(path, &cfg, &h.index)?;

    let md = std::fs::metadata(path)?;
    let last_access = access::best_last_access(path).unwrap_or_else(|| chrono::Utc::now().timestamp());
    let now = chrono::Utc::now().timestamp();
    let age = Duration::from_secs((now - last_access).max(0) as u64);
    let tier = cfg.tiers.tier_for_age(age);

    let compiled = rules::compile(&cfg.rules)?;
    let cand = rules::Candidate {
        path,
        size: md.len(),
        tier,
        age,
    };
    if let Some((rule, action)) = rules::first_match(&compiled, &cand) {
        let dry = cfg.general.dry_run;
        match action {
            rules::Resolved::MoveTo(dest_root) => {
                let dest = rules::move_destination(&dest_root, tier, path);
                let moved = actions::safe_move(path, &dest, dry, &h.index, Some(&rule.name))?;
                let _ = h.index.update_path(path, &moved);
                let _ = h.index.set_managed_rule(&moved, &rule.name);
                let _ = h.index.set_tier(&moved, tier);
                push_activity(h, &format!("[{}] → {}", rule.name, moved.display()));
            }
            rules::Resolved::Retier(target) => {
                let _ = h.index.set_tier(path, target);
                push_activity(h, &format!("[{}] retier {} → {}", rule.name, path.display(), target.dir_name()));
            }
            rules::Resolved::Archive { to, delete_source } => {
                let compress = cfg.archive.compress;
                scheduler::archive_file_with(
                    path,
                    &to,
                    delete_source,
                    dry,
                    compress,
                    &h.index,
                    Some(&rule.name),
                )?;
            }
            rules::Resolved::DeleteAfter(_) => {
                // Deferred to the periodic scheduler; marking the rule here so
                // the scheduler finds the file next sweep.
                let _ = h.index.set_managed_rule(path, &rule.name);
            }
        }
    }
    Ok(())
}

fn start_ticker(tx: Sender<EngineMsg>, h: EngineHandle) {
    thread::Builder::new()
        .name("heighliner-ticker".into())
        .spawn(move || {
            let mut last_rescan = std::time::Instant::now();
            let mut last_retier = std::time::Instant::now();
            let mut last_archive = std::time::Instant::now();
            let mut last_purge = std::time::Instant::now();
            loop {
                thread::sleep(Duration::from_secs(30));
                let cfg = h.config.lock().clone();
                if last_rescan.elapsed() >= cfg.general.rescan_interval {
                    last_rescan = std::time::Instant::now();
                    let _ = tx.send(EngineMsg::Tick(SweepKind::Rescan));
                }
                if last_retier.elapsed() >= cfg.general.retier_interval {
                    last_retier = std::time::Instant::now();
                    let _ = tx.send(EngineMsg::Tick(SweepKind::Retier));
                }
                if last_archive.elapsed() >= Duration::from_secs(24 * 60 * 60) {
                    last_archive = std::time::Instant::now();
                    let _ = tx.send(EngineMsg::Tick(SweepKind::Archive));
                }
                if last_purge.elapsed() >= Duration::from_secs(24 * 60 * 60) {
                    last_purge = std::time::Instant::now();
                    let _ = tx.send(EngineMsg::Tick(SweepKind::Purge));
                }
            }
        })
        .expect("spawn ticker thread");
}

fn update_stats(h: &EngineHandle) {
    let file_count = h.index.file_count().unwrap_or(0);
    let bytes_managed = h.index.total_bytes().unwrap_or(0);
    let actions_run = h.index.action_count().unwrap_or(0);
    h.shared.lock().stats = StatsSnapshot {
        file_count,
        actions_run,
        bytes_managed,
    };
}

fn push_activity(h: &EngineHandle, msg: &str) {
    tracing::info!("{}", msg);
    h.shared.lock().last_activity = Some(msg.into());
}

fn push_error(h: &EngineHandle, msg: String) {
    tracing::warn!("{}", msg);
    h.shared.lock().last_error = Some(msg);
}
