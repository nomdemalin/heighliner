use eframe::egui::{self, Color32, Rounding, Stroke};
use humansize::{format_size, DECIMAL};

use crate::engine::config::Tier;
use crate::engine::EngineHandle;
use crate::ui::theme::{self, card, section_header, PALETTE};
use crate::ui::treemap;

#[derive(Default)]
pub struct DashboardState {
    pub treemap: treemap::TreemapState,
}

pub fn show(ui: &mut egui::Ui, state: &mut DashboardState, engine: &EngineHandle) {
    section_header(ui, "Dashboard", "What's happening in your watched directories");
    let stats = engine.stats_snapshot();

    ui.horizontal(|ui| {
        tile(ui, "Files indexed", format!("{}", stats.file_count), "across all watched roots");
        ui.add_space(10.0);
        tile(ui, "Bytes managed", format_size(stats.bytes_managed, DECIMAL), "total on disk");
        ui.add_space(10.0);
        tile(ui, "Actions run", format!("{}", stats.actions_run), "all-time, successful");
        ui.add_space(10.0);
        let dry = engine.config.lock().general.dry_run;
        tile(
            ui,
            "Mode",
            if dry { "DRY RUN".into() } else { "LIVE".into() },
            if dry { "no files are touched" } else { "actions apply immediately" },
        );
    });

    ui.add_space(16.0);

    card(ui, |ui| {
        ui.add(egui::Label::new(
            egui::RichText::new("Tier breakdown").size(14.0).strong().color(PALETTE.text),
        ));
        ui.add(egui::Label::new(
            egui::RichText::new("Bytes grouped by access-age tier")
                .size(11.5)
                .color(PALETTE.text_dim),
        ));
        ui.add_space(8.0);
        let per_tier = engine.index.bytes_per_tier().unwrap_or_default();
        draw_tier_bar(ui, &per_tier);
    });

    ui.add_space(16.0);

    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(
                egui::RichText::new("Space treemap").size(14.0).strong().color(PALETTE.text),
            ));
            ui.add_space(6.0);
            ui.add(egui::Label::new(
                egui::RichText::new("colored by dominant tier")
                    .size(11.5)
                    .color(PALETTE.text_dim),
            ));
        });
        ui.add_space(6.0);
        treemap::show(ui, &mut state.treemap, engine);
    });
}

fn tile(ui: &mut egui::Ui, label: &str, value: String, hint: &str) {
    let w = (ui.available_width() - 36.0) / 4.0;
    ui.allocate_ui_with_layout(
        egui::vec2(w, 96.0),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            egui::Frame::none()
                .fill(PALETTE.bg_card)
                .stroke(Stroke::new(1.0_f32, PALETTE.border))
                .rounding(Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_min_width(w - 28.0);
                    ui.set_min_height(72.0);
                    ui.add(egui::Label::new(
                        egui::RichText::new(label)
                            .size(10.5)
                            .color(PALETTE.text_faint)
                            .strong(),
                    ));
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(
                        egui::RichText::new(value).size(24.0).strong().color(PALETTE.text),
                    ));
                    ui.add(egui::Label::new(
                        egui::RichText::new(hint).size(11.0).color(PALETTE.text_dim),
                    ));
                });
        },
    );
}

fn draw_tier_bar(ui: &mut egui::Ui, per_tier: &[(Tier, u64)]) {
    let total: u64 = per_tier.iter().map(|(_, b)| *b).sum::<u64>().max(1);
    let width = ui.available_width();
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(6.0), PALETTE.bg_elevated);

    let mut x = rect.left();
    for t in Tier::ALL {
        let bytes = per_tier.iter().find(|(tt, _)| *tt == t).map(|(_, b)| *b).unwrap_or(0);
        if bytes == 0 {
            continue;
        }
        let w = (bytes as f32 / total as f32) * rect.width();
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, rect.height()));
        painter.rect_filled(seg, Rounding::same(6.0), theme::tier_color(t));
        if w > 50.0 {
            painter.text(
                seg.center(),
                egui::Align2::CENTER_CENTER,
                humansize::format_size(bytes, DECIMAL),
                egui::FontId::proportional(11.0),
                Color32::from_rgb(10, 15, 25),
            );
        }
        x += w;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for t in Tier::ALL {
            let bytes = per_tier.iter().find(|(tt, _)| *tt == t).map(|(_, b)| *b).unwrap_or(0);
            egui::Frame::none()
                .fill(PALETTE.bg_elevated)
                .stroke(Stroke::new(1.0_f32, PALETTE.border))
                .rounding(Rounding::same(999.0))
                .inner_margin(egui::Margin::symmetric(10.0, 3.0))
                .show(ui, |ui| {
                    ui.colored_label(theme::tier_color(t), theme::tier_glyph(t));
                    ui.label(egui::RichText::new(t.dir_name()).color(PALETTE.text).size(12.0));
                    ui.label(
                        egui::RichText::new(humansize::format_size(bytes, DECIMAL))
                            .color(PALETTE.text_dim)
                            .size(11.5),
                    );
                });
            ui.add_space(6.0);
        }
    });
}
