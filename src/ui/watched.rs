use std::path::PathBuf;

use eframe::egui::{self, Rounding};

use crate::engine::config::Watch;
use crate::engine::EngineHandle;
use crate::ui::theme::{self, card, section_header, PALETTE};

#[derive(Default)]
pub struct WatchedState {
    pending_remove: Option<usize>,
}

/// A snapshot row grabbed under the config lock so we can render + hit SQL
/// without holding `engine.config.lock()` across the whole panel (F7).
struct WatchRow {
    index: usize,
    path: PathBuf,
    recursive: bool,
}

pub fn show(ui: &mut egui::Ui, state: &mut WatchedState, engine: &EngineHandle) {
    section_header(
        ui,
        "Watched directories",
        "Heighliner observes these roots and feeds their files into the rules engine.",
    );

    let add_clicked = ui
        .horizontal(|ui| {
            let btn = egui::Button::new(
                egui::RichText::new("+  Add directory").color(PALETTE.bg_window).strong(),
            )
            .fill(PALETTE.accent)
            .rounding(Rounding::same(6.0));
            ui.add(btn).clicked()
        })
        .inner;

    if add_clicked {
        if let Some(folder) = rfd::FileDialog::new().set_title("Watch directory").pick_folder() {
            handle_add(engine, folder);
        }
    }

    ui.add_space(10.0);

    // F7: snapshot the watches so the lock drops before we start rendering
    // cards and running SQL per watch.
    let rows: Vec<WatchRow> = {
        let cfg = engine.config.lock();
        cfg.watches
            .iter()
            .enumerate()
            .map(|(i, w)| WatchRow {
                index: i,
                path: w.path.clone(),
                recursive: w.recursive,
            })
            .collect()
    };

    if rows.is_empty() {
        card(ui, |ui| {
            ui.add(egui::Label::new(
                egui::RichText::new("No directories watched yet.")
                    .color(PALETTE.text_dim)
                    .size(13.0),
            ));
            ui.add(egui::Label::new(
                egui::RichText::new("Pick one above and Heighliner will index it and start applying rules.")
                    .color(PALETTE.text_faint)
                    .size(11.5),
            ));
        });
        return;
    }

    let mut dirty = false;
    for row in &rows {
        let mut recursive = row.recursive;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new("📁").size(18.0),
                ));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new(row.path.display().to_string())
                            .color(PALETTE.text)
                            .strong(),
                    ));
                    let meta = if recursive { "recursive" } else { "top-level only" };
                    ui.add(egui::Label::new(
                        egui::RichText::new(meta).size(11.5).color(PALETTE.text_dim),
                    ));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Remove").clicked() {
                        state.pending_remove = Some(row.index);
                    }
                    ui.add_space(4.0);
                    if ui.checkbox(&mut recursive, "Recursive").changed() {
                        // Reacquire lock just for the mutation.
                        let mut cfg = engine.config.lock();
                        if let Some(w) = cfg.watches.get_mut(row.index) {
                            w.recursive = recursive;
                            dirty = true;
                        }
                    }
                });
            });
            // SQL outside the config lock.
            {
                // P3: cached per-root breakdown. Even with 3 watched roots
                // this panel used to run three LIKE scans twice a second.
                let breakdown = engine.cached_bytes_per_child(&row.path);
                let total: u64 = breakdown.iter().map(|c| c.bytes).sum();
                let count: u64 = breakdown.iter().map(|c| c.files).sum();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    theme::pill(
                        ui,
                        &format!("{} files", count),
                        PALETTE.accent_dim,
                    );
                    theme::pill(
                        ui,
                        &humansize::format_size(total, humansize::DECIMAL),
                        PALETTE.bg_elevated,
                    );
                });
            }
        });
        ui.add_space(10.0);
    }

    if let Some(i) = state.pending_remove.take() {
        let mut cfg = engine.config.lock();
        if i < cfg.watches.len() {
            cfg.watches.remove(i);
            dirty = true;
        }
    }

    if dirty {
        // Snapshot-and-save, no lock across I/O (same pattern as F8).
        let snapshot = engine.config.lock().clone();
        match snapshot.save(&engine.paths.config_file) {
            Ok(()) => {
                engine.clear_error();
                engine.reload_config();
            }
            Err(e) => engine.set_error(format!("save config: {e}")),
        }
    }
}

fn handle_add(engine: &EngineHandle, folder: PathBuf) {
    let snapshot_to_save: Option<crate::engine::config::Config> = {
        let mut cfg = engine.config.lock();
        if cfg.watches.iter().any(|w| w.path == folder) {
            None
        } else {
            cfg.watches.push(Watch {
                path: folder.clone(),
                recursive: true,
            });
            tracing::info!(path = %folder.display(), total = cfg.watches.len(), "added watched directory");
            Some(cfg.clone())
        }
    };
    if let Some(snapshot) = snapshot_to_save {
        match snapshot.save(&engine.paths.config_file) {
            Ok(()) => {
                engine.clear_error();
                engine.reload_config();
            }
            Err(e) => {
                tracing::error!(error = %e, "save config failed");
                engine.set_error(format!("save config: {e}"));
            }
        }
    }
}
