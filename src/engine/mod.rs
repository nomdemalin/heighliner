use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;

pub mod access;
pub mod actions;
pub mod config;
pub mod index;
pub mod paths;
pub mod rules;
pub mod scanner;
pub mod scheduler;
pub mod watcher;

use config::Config;
use index::Index;
use paths::AppPaths;
use rules::CompiledRule;

/// Capacity for the engine command channel. Bounded (F16): FS event bursts
/// and sweep ticks block briefly on backpressure instead of growing memory
/// without limit.
const ENGINE_CHANNEL_CAP: usize = 4096;

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
    DeleteAfter,
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
    /// F5: compiled rules cache. Rebuilt on config load / reload so the FS
    /// hot path doesn't rerun `GlobSet::build` per event.
    pub compiled: Arc<Mutex<Arc<Vec<CompiledRule>>>>,
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

    /// F8: flip the flag under the config lock, clone the config, drop the
    /// lock, then save. No fs::write + rename under the mutex. Returns
    /// Result<()> (F24) — the caller doesn't need the new value.
    pub fn toggle_dry_run(&self) -> Result<()> {
        let snapshot = {
            let mut cfg = self.config.lock();
            cfg.general.dry_run = !cfg.general.dry_run;
            cfg.clone()
        };
        snapshot.save(&self.paths.config_file)?;
        Ok(())
    }

    /// Snapshot of the currently compiled rules. Cheap: an Arc clone.
    pub fn compiled_rules(&self) -> Arc<Vec<CompiledRule>> {
        self.compiled.lock().clone()
    }

    /// Rebuild the compiled rules from the current config.
    fn rebuild_rules(&self) -> Result<()> {
        let rules = self.config.lock().rules.clone();
        let compiled = rules::compile(&rules)?;
        *self.compiled.lock() = Arc::new(compiled);
        Ok(())
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
        let (tx, rx) = bounded::<EngineMsg>(ENGINE_CHANNEL_CAP);
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let config = Arc::new(Mutex::new(init.config));
        let compiled: Arc<Mutex<Arc<Vec<CompiledRule>>>> = Arc::new(Mutex::new(Arc::new(Vec::new())));
        let handle = EngineHandle {
            paths: init.paths,
            config: config.clone(),
            index: init.index.clone(),
            compiled: compiled.clone(),
            shared: shared.clone(),
            msgs: tx.clone(),
        };

        // Pre-build compiled rules so the first ingest doesn't pay the cost.
        if let Err(e) = handle.rebuild_rules() {
            handle.set_error(format!("compile rules: {e}"));
        }

        // F17: catch panics in the engine thread so the UI surfaces them
        // instead of silently seizing on stale "idle".
        let th = handle.clone();
        thread::Builder::new()
            .name("heighliner-engine".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    if let Err(e) = run(th.clone(), rx) {
                        tracing::error!(error = %e, "engine thread exited with error");
                        th.set_error(e.to_string());
                    }
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    tracing::error!("engine thread panicked: {msg}");
                    th.set_error(format!("engine panic: {msg}"));
                }
            })
            .expect("spawn engine thread");

        start_ticker(tx, handle.clone());
        handle
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "unknown panic".into()
    }
}

fn run(h: EngineHandle, rx: Receiver<EngineMsg>) -> Result<()> {
    tracing::info!("engine thread starting");
    push_activity(&h, "Starting up");
    rescan_once(&h);
    update_stats(&h);

    // F23: not an underscore — this binding IS load-bearing. Dropping the
    // Debouncer stops FSEvents delivery.
    let mut watcher_handle = rebuild_watcher(&h);

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
            EngineMsg::Tick(SweepKind::DeleteAfter) => {
                if let Err(e) = scheduler::delete_after_sweep(&h) {
                    push_error(&h, format!("delete_after sweep: {e}"));
                }
            }
            EngineMsg::ScanNow => rescan_once(&h),
            EngineMsg::ReloadConfig => {
                if let Err(e) = h.rebuild_rules() {
                    push_error(&h, format!("compile rules: {e}"));
                }
                watcher_handle = rebuild_watcher(&h);
                push_activity(&h, "Reloaded config");
                rescan_once(&h);
            }
        }
        update_stats(&h);
    }
    drop(watcher_handle);
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

    // F5: use the cached compiled rules.
    let compiled = h.compiled_rules();
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
                // F20: all post-move index writes atomic (txn).
                if !dry && moved != path {
                    let _ = h.index.commit_managed_move(path, &moved, tier, &rule.name);
                }
                push_activity(h, &format!("[{}] → {}", rule.name, moved.display()));
            }
            rules::Resolved::Retier(target) => {
                let _ = h.index.set_tier(path, target);
                let _ = h.index.set_managed_rule(path, &rule.name);
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
                // Deferred to delete_after_sweep (F2); mark the file so the
                // sweeper finds it on the next tick.
                let _ = h.index.set_managed_rule(path, &rule.name);
            }
        }
    }
    Ok(())
}

/// Ticker: fires sweep ticks on configured cadence.
///
/// F27 (noted, not eliminated): the 30s outer sleep is a hard floor on how
/// responsive these timers can be. A configured `rescan_interval` of less
/// than 30s rounds up to this cadence; raise the sleep for better efficiency
/// or shrink it for finer granularity.
fn start_ticker(tx: Sender<EngineMsg>, h: EngineHandle) {
    thread::Builder::new()
        .name("heighliner-ticker".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| ticker_loop(tx, &h)));
            if let Err(panic) = result {
                let msg = panic_message(&panic);
                tracing::error!("ticker thread panicked: {msg}");
                h.set_error(format!("ticker panic: {msg}"));
            }
        })
        .expect("spawn ticker thread");
}

fn ticker_loop(tx: Sender<EngineMsg>, h: &EngineHandle) {
    let mut last_rescan = std::time::Instant::now();
    let mut last_retier = std::time::Instant::now();
    let mut last_archive = std::time::Instant::now();
    let mut last_purge = std::time::Instant::now();
    let mut last_delete_after = std::time::Instant::now();
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
        if last_delete_after.elapsed() >= Duration::from_secs(60 * 60) {
            last_delete_after = std::time::Instant::now();
            let _ = tx.send(EngineMsg::Tick(SweepKind::DeleteAfter));
        }
    }
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
