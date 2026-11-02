use eframe::egui::{self, Rounding};

use crate::engine::config::Watch;
use crate::engine::EngineHandle;
use crate::ui::theme::{self, card, section_header, PALETTE};

#[derive(Default)]
pub struct WatchedState {
    pending_remove: Option<usize>,
}

pub fn show(ui: &mut egui::Ui, state: &mut WatchedState, engine: &EngineHandle) {
    section_header(
        ui,
        "Watched directories",
        "Heighliner observes these roots and feeds their files into the rules engine.",
    );

    let mut dirty = false;

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
            let path_added = folder.clone();
            let mut cfg = engine.config.lock();
            if !cfg.watches.iter().any(|w| w.path == path_added) {
                cfg.watches.push(Watch { path: folder, recursive: true });
                match cfg.save(&engine.paths.config_file) {
                    Ok(()) => {
                        tracing::info!(path = %path_added.display(), total = cfg.watches.len(), "added watched directory");
                        drop(cfg);
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
    }

    ui.add_space(10.0);

    let mut cfg = engine.config.lock();
    if cfg.watches.is_empty() {
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

    for (i, w) in cfg.watches.iter_mut().enumerate() {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new("📁").size(18.0),
                ));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new(w.path.display().to_string())
                            .color(PALETTE.text)
                            .strong(),
                    ));
                    let meta = if w.recursive { "recursive" } else { "top-level only" };
                    ui.add(egui::Label::new(
                        egui::RichText::new(meta).size(11.5).color(PALETTE.text_dim),
                    ));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Remove").clicked() {
                        state.pending_remove = Some(i);
                    }
                    ui.add_space(4.0);
                    if ui.checkbox(&mut w.recursive, "Recursive").changed() {
                        dirty = true;
                    }
                });
            });
            if let Ok(breakdown) = engine.index.bytes_per_child(&w.path) {
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
        if i < cfg.watches.len() {
            cfg.watches.remove(i);
            dirty = true;
        }
    }

    if dirty {
        if let Err(e) = cfg.save(&engine.paths.config_file) {
            engine.set_error(format!("save config: {e}"));
        } else {
            engine.clear_error();
            drop(cfg);
            engine.reload_config();
        }
    }
}
