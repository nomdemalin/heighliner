use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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
use index::{ActionRow, ChildBreakdown, Index};
use paths::AppPaths;
use rules::CompiledRule;

/// Capacity for the engine command channel. Bounded (F16): FS event bursts
/// and sweep ticks block briefly on backpressure instead of growing memory
/// without limit.
const ENGINE_CHANNEL_CAP: usize = 4096;

/// P3: how long a cached UI query is served before we re-run the SQL. 800ms
/// matches the UI's typical repaint cadence and bounds staleness tightly.
const CACHE_TTL: Duration = Duration::from_millis(800);

/// P2: how many rows the rescan loop buffers between DB commits.
const SCAN_BATCH: usize = 2048;

/// P11: how often `rescan_once` pushes a fresh stats snapshot while walking.
const SCAN_STATS_EVERY: Duration = Duration::from_millis(750);

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

#[derive(Default)]
struct Caches {
    bytes_per_tier: Option<(Instant, Vec<(config::Tier, u64)>)>,
    bytes_per_child: HashMap<PathBuf, (Instant, Vec<ChildBreakdown>)>,
    recent_actions: Option<(Instant, Vec<ActionRow>)>,
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
    /// P3: TTL cache for the repeat UI queries.
    caches: Arc<Mutex<Caches>>,
    /// P18: bumped on each push_activity / push_error / stats update. UI
    /// reads this to know when to repaint; idle app → no repaints.
    activity_version: Arc<AtomicU64>,
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

    pub fn activity_version(&self) -> u64 {
        self.activity_version.load(Ordering::Relaxed)
    }

    pub fn set_error(&self, msg: impl Into<String>) {
        self.shared.lock().last_error = Some(msg.into());
        self.bump_activity();
    }

    pub fn clear_error(&self) {
        self.shared.lock().last_error = None;
        self.bump_activity();
    }

    fn bump_activity(&self) {
        self.activity_version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn request_scan_now(&self) {
        let _ = self.msgs.send(EngineMsg::ScanNow);
    }

    pub fn reload_config(&self) {
        let _ = self.msgs.send(EngineMsg::ReloadConfig);
    }

    /// F8: flip the flag under the config lock, clone the config, drop the
    /// lock, then save.
    pub fn toggle_dry_run(&self) -> Result<()> {
        let snapshot = {
            let mut cfg = self.config.lock();
            cfg.general.dry_run = !cfg.general.dry_run;
            cfg.clone()
        };
        snapshot.save(&self.paths.config_file)?;
        Ok(())
    }

    pub fn compiled_rules(&self) -> Arc<Vec<CompiledRule>> {
        self.compiled.lock().clone()
    }

    fn rebuild_rules(&self) -> Result<()> {
        let rules = self.config.lock().rules.clone();
        let compiled = rules::compile(&rules)?;
        *self.compiled.lock() = Arc::new(compiled);
        Ok(())
    }

    // ---- P3: cached queries for the UI ----

    pub fn cached_bytes_per_tier(&self) -> Vec<(config::Tier, u64)> {
        {
            let c = self.caches.lock();
            if let Some((ts, v)) = &c.bytes_per_tier {
                if ts.elapsed() < CACHE_TTL {
                    return v.clone();
                }
            }
        }
        let fresh = self.index.bytes_per_tier().unwrap_or_default();
        let mut c = self.caches.lock();
        c.bytes_per_tier = Some((Instant::now(), fresh.clone()));
        fresh
    }

    pub fn cached_bytes_per_child(&self, root: &std::path::Path) -> Vec<ChildBreakdown> {
        {
            let c = self.caches.lock();
            if let Some((ts, v)) = c.bytes_per_child.get(root) {
                if ts.elapsed() < CACHE_TTL {
                    return v.clone();
                }
            }
        }
        let fresh = self.index.bytes_per_child(root).unwrap_or_default();
        let mut c = self.caches.lock();
        c.bytes_per_child
            .insert(root.to_path_buf(), (Instant::now(), fresh.clone()));
        fresh
    }

    pub fn cached_recent_actions(&self, limit: u32) -> Vec<ActionRow> {
        {
            let c = self.caches.lock();
            if let Some((ts, v)) = &c.recent_actions {
                if ts.elapsed() < CACHE_TTL {
                    return v.clone();
                }
            }
        }
        let fresh = self.index.recent_actions(limit).unwrap_or_default();
        let mut c = self.caches.lock();
        c.recent_actions = Some((Instant::now(), fresh.clone()));
        fresh
    }

    /// Called after any state-changing operation so the next UI read goes
    /// straight to the DB instead of serving stale cached data.
    fn invalidate_caches(&self) {
        let mut c = self.caches.lock();
        c.bytes_per_tier = None;
        c.bytes_per_child.clear();
        c.recent_actions = None;
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
        let caches = Arc::new(Mutex::new(Caches::default()));
        let activity_version = Arc::new(AtomicU64::new(0));
        let config = Arc::new(Mutex::new(init.config));
        let compiled: Arc<Mutex<Arc<Vec<CompiledRule>>>> = Arc::new(Mutex::new(Arc::new(Vec::new())));
        let handle = EngineHandle {
            paths: init.paths,
            config: config.clone(),
            index: init.index.clone(),
            compiled: compiled.clone(),
            caches,
            activity_version,
            shared: shared.clone(),
            msgs: tx.clone(),
        };

        if let Err(e) = handle.rebuild_rules() {
            handle.set_error(format!("compile rules: {e}"));
        }

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

/// P12: hash the set of watch paths + their recursive flags. The watcher is
/// only rebuilt when this hash changes, so toggling `dry_run` or editing
/// unrelated rules doesn't thrash FSEvents registration.
fn watch_set_hash(watches: &[config::Watch]) -> u64 {
    let mut h = DefaultHasher::new();
    for w in watches {
        w.path.hash(&mut h);
        w.recursive.hash(&mut h);
    }
    h.finish()
}

fn run(h: EngineHandle, rx: Receiver<EngineMsg>) -> Result<()> {
    tracing::info!("engine thread starting");
    push_activity(&h, "Starting up");

    let mut current_watch_hash = watch_set_hash(&h.config.lock().watches);
    let mut watcher_handle = rebuild_watcher(&h);
    rescan_once(&h);
    update_stats(&h);

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
                let new_hash = watch_set_hash(&h.config.lock().watches);
                if new_hash != current_watch_hash {
                    watcher_handle = rebuild_watcher(&h);
                    current_watch_hash = new_hash;
                    push_activity(&h, "Reloaded config (watches changed)");
                    rescan_once(&h);
                } else {
                    push_activity(&h, "Reloaded config (rules/thresholds)");
                }
            }
        }
        update_stats(&h);
        h.invalidate_caches();
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
    h.bump_activity();

    // P5: pull only the scalars the hot path uses.
    let (watches, tier_policy, dry, compress) = {
        let cfg = h.config.lock();
        (
            cfg.watches.clone(),
            cfg.tiers.clone(),
            cfg.general.dry_run,
            cfg.archive.compress,
        )
    };
    let compiled = h.compiled_rules();

    let mut seen: u64 = 0;
    let mut errors: u64 = 0;
    let mut buf: Vec<index::FileRow> = Vec::with_capacity(SCAN_BATCH);
    let mut last_stats_push = Instant::now();

    for w in &watches {
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
            // P1: one metadata() per file.
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(path = %entry.path().display(), error = %e, "metadata failed");
                    errors += 1;
                    continue;
                }
            };
            if !md.is_file() {
                continue;
            }
            buf.push(scanner::build_row(entry.path(), &md, &tier_policy));
            seen += 1;

            if buf.len() >= SCAN_BATCH {
                flush_batch(h, &mut buf, &compiled, dry, compress, &mut errors);
            }
            // P11: live stats so the status bar actually ticks during long scans.
            if last_stats_push.elapsed() >= SCAN_STATS_EVERY {
                update_stats(h);
                last_stats_push = Instant::now();
            }
        }
    }

    flush_batch(h, &mut buf, &compiled, dry, compress, &mut errors);

    push_activity(h, &format!("Scan: {seen} files · {errors} errors"));
    h.shared.lock().scanning = false;
    update_stats(h);

    h.bump_activity();
    h.invalidate_caches();
}

/// P2: commit the accumulated upsert batch, then run rules against each row
/// in the batch using the already-known metadata. The upserts land first so
/// `commit_managed_move`'s UPDATE finds the row.
fn flush_batch(
    h: &EngineHandle,
    buf: &mut Vec<index::FileRow>,
    compiled: &[CompiledRule],
    dry: bool,
    compress: bool,
    errors: &mut u64,
) {
    if buf.is_empty() {
        return;
    }
    if let Err(e) = h.index.upsert_batch(buf) {
        tracing::warn!(error = %e, "batch upsert failed");
        *errors += buf.len() as u64;
        buf.clear();
        return;
    }
    let now = chrono::Utc::now().timestamp();
    for row in buf.drain(..) {
        apply_rule_to_row(h, &row, compiled, dry, compress, now);
    }
}

fn apply_rule_to_row(
    h: &EngineHandle,
    row: &index::FileRow,
    compiled: &[CompiledRule],
    dry: bool,
    compress: bool,
    now: i64,
) {
    let tier = row.tier.unwrap_or(config::Tier::Hot);
    let age = Duration::from_secs(
        (now - row.last_user_access.unwrap_or(now)).max(0) as u64,
    );
    let cand = rules::Candidate {
        path: &row.path,
        size: row.size,
        tier,
        age,
    };
    let Some((rule, action)) = rules::first_match(compiled, &cand) else {
        return;
    };
    match action {
        rules::Resolved::MoveTo(dest_root) => {
            let dest = rules::move_destination(&dest_root, tier, &row.path);
            match actions::safe_move(&row.path, &dest, dry, &h.index, Some(&rule.name)) {
                Ok(moved) => {
                    if !dry && moved != row.path {
                        let _ = h.index.commit_managed_move(&row.path, &moved, tier, &rule.name);
                    }
                    push_activity(h, &format!("[{}] → {}", rule.name, moved.display()));
                }
                Err(e) => tracing::warn!(path = %row.path.display(), error = %e, "move failed"),
            }
        }
        rules::Resolved::Retier(target) => {
            let _ = h.index.set_tier(&row.path, target);
            let _ = h.index.set_managed_rule(&row.path, &rule.name);
            push_activity(h, &format!("[{}] retier {} → {}", rule.name, row.path.display(), target.dir_name()));
        }
        rules::Resolved::Archive { to, delete_source } => {
            if let Err(e) = scheduler::archive_file_with(
                &row.path, &to, delete_source, dry, compress, &h.index, Some(&rule.name),
            ) {
                tracing::warn!(path = %row.path.display(), error = %e, "archive failed");
            }
        }
        rules::Resolved::DeleteAfter(_) => {
            let _ = h.index.set_managed_rule(&row.path, &rule.name);
        }
    }
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
    // P1: one metadata() for the whole function. is_file() comes from it.
    let md = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if !md.is_file() {
        return Ok(());
    }

    // P5: pull only scalar config values; no deep clone of Vec<Rule>+Vec<Watch>.
    let (dry, tier_policy, compress) = {
        let cfg = h.config.lock();
        (cfg.general.dry_run, cfg.tiers.clone(), cfg.archive.compress)
    };

    let row = scanner::build_row(path, &md, &tier_policy);
    h.index.upsert_file(&row)?;

    let tier = row.tier.unwrap_or(config::Tier::Hot);
    let now = chrono::Utc::now().timestamp();
    let age = Duration::from_secs(
        (now - row.last_user_access.unwrap_or(now)).max(0) as u64,
    );

    let compiled = h.compiled_rules();
    let cand = rules::Candidate {
        path,
        size: row.size,
        tier,
        age,
    };
    if let Some((rule, action)) = rules::first_match(&compiled, &cand) {
        match action {
            rules::Resolved::MoveTo(dest_root) => {
                let dest = rules::move_destination(&dest_root, tier, path);
                let moved = actions::safe_move(path, &dest, dry, &h.index, Some(&rule.name))?;
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
                let _ = h.index.set_managed_rule(path, &rule.name);
            }
        }
    }
    Ok(())
}

/// Ticker: fires sweep ticks on configured cadence.
///
/// F27: 30s outer sleep is a hard floor on how responsive these timers can
/// be. A configured `rescan_interval` of less than 30s rounds up to this
/// cadence.
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
    let mut last_rescan = Instant::now();
    let mut last_retier = Instant::now();
    let mut last_archive = Instant::now();
    let mut last_purge = Instant::now();
    let mut last_delete_after = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(30));
        let (rescan_i, retier_i) = {
            let cfg = h.config.lock();
            (cfg.general.rescan_interval, cfg.general.retier_interval)
        };
        if last_rescan.elapsed() >= rescan_i {
            last_rescan = Instant::now();
            let _ = tx.send(EngineMsg::Tick(SweepKind::Rescan));
        }
        if last_retier.elapsed() >= retier_i {
            last_retier = Instant::now();
            let _ = tx.send(EngineMsg::Tick(SweepKind::Retier));
        }
        if last_archive.elapsed() >= Duration::from_secs(24 * 60 * 60) {
            last_archive = Instant::now();
            let _ = tx.send(EngineMsg::Tick(SweepKind::Archive));
        }
        if last_purge.elapsed() >= Duration::from_secs(24 * 60 * 60) {
            last_purge = Instant::now();
            let _ = tx.send(EngineMsg::Tick(SweepKind::Purge));
        }
        if last_delete_after.elapsed() >= Duration::from_secs(60 * 60) {
            last_delete_after = Instant::now();
            let _ = tx.send(EngineMsg::Tick(SweepKind::DeleteAfter));
        }
    }
}

/// P4: three counter rows out of stats_counters instead of three full-table
/// aggregates.
fn update_stats(h: &EngineHandle) {
    let (file_count, bytes_managed, actions_run) =
        h.index.quick_stats().unwrap_or((0, 0, 0));
    let mut shared = h.shared.lock();
    let changed = shared.stats.file_count != file_count
        || shared.stats.bytes_managed != bytes_managed
        || shared.stats.actions_run != actions_run;
    shared.stats = StatsSnapshot {
        file_count,
        actions_run,
        bytes_managed,
    };
    drop(shared);
    if changed {
        h.bump_activity();
    }
}

fn push_activity(h: &EngineHandle, msg: &str) {
    tracing::info!("{}", msg);
    h.shared.lock().last_activity = Some(msg.into());
    h.bump_activity();
}

fn push_error(h: &EngineHandle, msg: String) {
    tracing::warn!("{}", msg);
    h.shared.lock().last_error = Some(msg);
    h.bump_activity();
}
