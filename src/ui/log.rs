use chrono::{DateTime, Local, TimeZone};
use eframe::egui::{self, Rounding};

use crate::engine::EngineHandle;
use crate::ui::theme::{card, section_header, PALETTE};

#[derive(Default)]
pub struct LogState {
    pub filter: String,
    pub only_errors: bool,
}

pub fn show(ui: &mut egui::Ui, state: &mut LogState, engine: &EngineHandle) {
    section_header(
        ui,
        "Activity log",
        "Every move, retier, archive, and purge Heighliner has run — most recent first.",
    );

    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .desired_width(220.0)
                .hint_text("substring match"),
        );
        ui.checkbox(&mut state.only_errors, "Errors only");
    });
    ui.add_space(8.0);

    // P3: cached_recent_actions is served from the engine's TTL cache; the
    // panel can render twice a second without re-running the SQL each time.
    let rows = engine.cached_recent_actions(1000);

    let f_lower = state.filter.to_lowercase();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| {
            if state.only_errors && r.success {
                return false;
            }
            if f_lower.is_empty() {
                return true;
            }
            let hay = format!(
                "{} {} {} {}",
                r.kind,
                r.from_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                r.to_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                r.rule_name.clone().unwrap_or_default(),
            )
            .to_lowercase();
            hay.contains(&f_lower)
        })
        .collect();

    if rows.is_empty() {
        card(ui, |ui| {
            ui.colored_label(PALETTE.text_dim, "No matching actions. Add watched dirs and rules and the log will fill in.");
        });
        return;
    }

    // P16: virtualize. row_card height varies with number of lines (from/to/err),
    // so use a conservative fixed estimate — ScrollArea::show_rows requires one.
    const ROW_H: f32 = 86.0;
    card(ui, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(540.0)
            .show_rows(ui, ROW_H, rows.len(), |ui, visible| {
                for i in visible {
                    row_card(ui, &rows[i]);
                    ui.add_space(4.0);
                }
            });
    });
}

fn row_card(ui: &mut egui::Ui, row: &crate::engine::index::ActionRow) {
    let (badge_color, badge_text) = if row.success {
        (PALETTE.success, "OK")
    } else {
        (PALETTE.danger, "FAIL")
    };
    let kind_color = match row.kind.as_str() {
        "move" => PALETTE.accent,
        "move_dry_run"
        | "archive_dry_run"
        | "archive_zip_dry_run"
        | "archive_tree_dry_run"
        | "archive_tree_zip_dry_run"
        | "purge_dry_run"
        | "delete_after_dry_run" => PALETTE.warning,
        "archive" | "archive_zip" | "archive_tree" | "archive_tree_zip" => PALETTE.tier_cold,
        "purge" | "delete_after" => PALETTE.danger,
        "skip_in_place" => PALETTE.text_faint,
        _ => PALETTE.text_dim,
    };

    egui::Frame::none()
        .fill(PALETTE.bg_elevated)
        .rounding(Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                crate::ui::theme::pill(ui, badge_text, badge_color);
                ui.add(egui::Label::new(
                    egui::RichText::new(&row.kind).color(kind_color).strong().size(12.0),
                ));
                let when = Local
                    .timestamp_opt(row.ts, 0)
                    .single()
                    .map(|dt: DateTime<Local>| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| row.ts.to_string());
                ui.add(egui::Label::new(
                    egui::RichText::new(when).color(PALETTE.text_dim).size(11.5).monospace(),
                ));
                if let Some(rule) = &row.rule_name {
                    ui.add(egui::Label::new(
                        egui::RichText::new(format!("rule: {rule}")).color(PALETTE.text_faint).size(11.0),
                    ));
                }
            });
            if let Some(from) = &row.from_path {
                ui.add(egui::Label::new(
                    egui::RichText::new(format!("from  {}", from.display()))
                        .monospace()
                        .color(PALETTE.text_dim)
                        .size(11.5),
                ));
            }
            if let Some(to) = &row.to_path {
                ui.add(egui::Label::new(
                    egui::RichText::new(format!("  to  {}", to.display()))
                        .monospace()
                        .color(PALETTE.text)
                        .size(11.5),
                ));
            }
            if let Some(err) = &row.error {
                ui.add(egui::Label::new(
                    egui::RichText::new(err).color(PALETTE.danger).size(11.5),
                ));
            }
        });
}
