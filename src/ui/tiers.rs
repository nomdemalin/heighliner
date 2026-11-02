use std::time::Duration;

use eframe::egui::{self, Rounding};

use crate::engine::config::Tier;
use crate::engine::EngineHandle;
use crate::ui::theme::{self, card, section_header, PALETTE};

pub struct TiersState {
    pub selected: Tier,
    pub hot_input: String,
    pub warm_input: String,
    pub cold_input: String,
    initialized: bool,
}

impl Default for TiersState {
    fn default() -> Self {
        Self {
            selected: Tier::Hot,
            hot_input: String::new(),
            warm_input: String::new(),
            cold_input: String::new(),
            initialized: false,
        }
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut TiersState, engine: &EngineHandle) {
    section_header(
        ui,
        "Tiers",
        "Access-age thresholds for hot/warm/cold/frozen. Files move between subdirs automatically.",
    );

    let (hot, warm, cold) = {
        let cfg = engine.config.lock();
        (cfg.tiers.hot_within, cfg.tiers.warm_within, cfg.tiers.cold_within)
    };
    if !state.initialized {
        state.hot_input = humantime::format_duration(hot).to_string();
        state.warm_input = humantime::format_duration(warm).to_string();
        state.cold_input = humantime::format_duration(cold).to_string();
        state.initialized = true;
    }

    card(ui, |ui| {
        ui.add(egui::Label::new(
            egui::RichText::new("Thresholds").color(PALETTE.text).strong().size(14.0),
        ));
        ui.add(egui::Label::new(
            egui::RichText::new(
                "A file is hot while its age ≤ hot_within, warm up to warm_within, cold up to cold_within, else frozen.",
            )
            .size(11.5)
            .color(PALETTE.text_dim),
        ));
        ui.add_space(6.0);

        egui::Grid::new("thresholds").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            threshold_row(ui, Tier::Hot, &mut state.hot_input);
            ui.end_row();
            threshold_row(ui, Tier::Warm, &mut state.warm_input);
            ui.end_row();
            threshold_row(ui, Tier::Cold, &mut state.cold_input);
            ui.end_row();
            ui.horizontal(|ui| {
                ui.colored_label(theme::tier_color(Tier::Frozen), theme::tier_glyph(Tier::Frozen));
                ui.label(egui::RichText::new("Frozen").color(PALETTE.text));
            });
            ui.label(egui::RichText::new("everything older than cold_within").color(PALETTE.text_dim));
            ui.label("");
            ui.end_row();
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let save = egui::Button::new(egui::RichText::new("Save thresholds").color(PALETTE.bg_window).strong())
                .fill(PALETTE.accent)
                .rounding(Rounding::same(6.0));
            if ui.add(save).clicked() {
                match (
                    humantime::parse_duration(state.hot_input.trim()),
                    humantime::parse_duration(state.warm_input.trim()),
                    humantime::parse_duration(state.cold_input.trim()),
                ) {
                    (Ok(h), Ok(w), Ok(c)) if h <= w && w <= c => {
                        let mut cfg = engine.config.lock();
                        cfg.tiers.hot_within = h;
                        cfg.tiers.warm_within = w;
                        cfg.tiers.cold_within = c;
                        if let Err(e) = cfg.save(&engine.paths.config_file) {
                            engine.set_error(format!("save: {e}"));
                        } else {
                            engine.clear_error();
                            drop(cfg);
                            engine.reload_config();
                        }
                    }
                    (Ok(_), Ok(_), Ok(_)) => {
                        engine.set_error("Thresholds must satisfy hot ≤ warm ≤ cold.");
                    }
                    _ => engine.set_error("Could not parse duration. Try e.g. 7d, 30d, 6mo."),
                }
            }
            if ui.button("Reset").clicked() {
                state.initialized = false;
            }
        });
    });

    ui.add_space(12.0);

    // Tier picker row.
    ui.horizontal(|ui| {
        for t in Tier::ALL {
            let selected = state.selected == t;
            let fill = if selected { theme::tier_color(t) } else { PALETTE.bg_elevated };
            let text_color = if selected { PALETTE.bg_window } else { PALETTE.text };
            let btn = egui::Button::new(
                egui::RichText::new(format!("{} {}", theme::tier_glyph(t), t.dir_name()))
                    .color(text_color)
                    .strong(),
            )
            .fill(fill)
            .rounding(Rounding::same(999.0));
            if ui.add(btn).clicked() {
                state.selected = t;
            }
        }
    });
    ui.add_space(8.0);

    let files = engine.index.files_in_tier(state.selected, 500).unwrap_or_default();
    if files.is_empty() {
        card(ui, |ui| {
            ui.colored_label(
                PALETTE.text_dim,
                format!("No files in {} yet.", state.selected.dir_name()),
            );
        });
        return;
    }

    card(ui, |ui| {
        ui.add(egui::Label::new(
            egui::RichText::new(format!("{} members · showing up to 500", state.selected.dir_name()))
                .color(PALETTE.text_dim)
                .size(11.5),
        ));
        ui.add_space(4.0);
        egui::Grid::new("tier_files").striped(true).num_columns(4).show(ui, |ui| {
            ui.strong("Path");
            ui.strong("Size");
            ui.strong("Last access");
            ui.strong("");
            ui.end_row();
            let now = chrono::Utc::now().timestamp();
            for f in files {
                ui.monospace(f.path.display().to_string());
                ui.monospace(humansize::format_size(f.size, humansize::DECIMAL));
                let age_str = match f.last_user_access {
                    Some(ts) => humantime::format_duration(Duration::from_secs(
                        (now - ts).max(0) as u64,
                    ))
                    .to_string(),
                    None => "—".into(),
                };
                ui.monospace(age_str);
                if ui.button("Reveal").clicked() {
                    crate::osx::reveal_in_finder(&f.path);
                }
                ui.end_row();
            }
        });
    });
}

fn threshold_row(ui: &mut egui::Ui, t: Tier, input: &mut String) {
    ui.horizontal(|ui| {
        ui.colored_label(theme::tier_color(t), theme::tier_glyph(t));
        ui.label(egui::RichText::new(t.dir_name()).color(PALETTE.text));
    });
    ui.add(
        egui::TextEdit::singleline(input)
            .desired_width(140.0)
            .hint_text("e.g. 7d, 1mo"),
    );
    ui.label(egui::RichText::new("duration").color(PALETTE.text_faint).size(11.0));
}
