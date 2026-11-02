use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, FileIdMap};

use super::config::Config;

#[derive(Debug, Clone)]
pub enum FsSignal {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

pub type WatcherHandle = Debouncer<notify::RecommendedWatcher, FileIdMap>;

/// Spawn a notify watcher per configured watch root. Debounces at 1s so
/// bursts of save-on-type editors don't flood the engine. `on_event` is
/// invoked from the notify worker thread.
pub fn spawn<F>(config: &Config, on_event: F) -> Result<WatcherHandle>
where
    F: Fn(FsSignal) + Send + 'static,
{
    let mut debouncer = new_debouncer(
        Duration::from_secs(1),
        None,
        move |events: notify_debouncer_full::DebounceEventResult| match events {
            Ok(evts) => {
                for e in evts {
                    for sig in translate(&e) {
                        on_event(sig);
                    }
                }
            }
            Err(errors) => {
                for err in errors {
                    tracing::warn!(error = %err, "watcher error");
                }
            }
        },
    )?;

    for w in &config.watches {
        let mode = if w.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        if let Err(e) = debouncer.watcher().watch(&w.path, mode) {
            tracing::warn!(path = %w.path.display(), error = %e, "watch failed");
            continue;
        }
        debouncer.cache().add_root(&w.path, mode);
    }
    Ok(debouncer)
}

fn translate(e: &DebouncedEvent) -> Vec<FsSignal> {
    use notify::event::{EventKind, ModifyKind, RenameMode};
    match &e.kind {
        EventKind::Create(_) => e.paths.iter().cloned().map(FsSignal::Created).collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if e.paths.len() == 2 => {
            vec![FsSignal::Renamed {
                from: e.paths[0].clone(),
                to: e.paths[1].clone(),
            }]
        }
        EventKind::Modify(_) => e.paths.iter().cloned().map(FsSignal::Modified).collect(),
        EventKind::Remove(_) => e.paths.iter().cloned().map(FsSignal::Removed).collect(),
        _ => vec![],
    }
}
