use std::path::PathBuf;

use eframe::egui::{self, Color32, Rect, Rounding, Sense, Stroke, Vec2};

use crate::engine::EngineHandle;
use crate::ui::theme::{self, PALETTE};

#[derive(Default)]
pub struct TreemapState {
    hovered_index: Option<usize>,
    selected_root: usize,
    pub drill_path: Option<PathBuf>,
}

pub fn show(ui: &mut egui::Ui, state: &mut TreemapState, engine: &EngineHandle) {
    let watches: Vec<std::path::PathBuf> = engine
        .config
        .lock()
        .watches
        .iter()
        .map(|w| w.path.clone())
        .collect();

    if watches.is_empty() {
        empty_state(ui, "Add a watched directory to see its space breakdown here.");
        return;
    }

    ui.horizontal_wrapped(|ui| {
        for (i, w) in watches.iter().enumerate() {
            let selected = i == state.selected_root;
            let display = w
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_else(|| w.to_str().unwrap_or(""));
            let text = egui::RichText::new(display)
                .color(if selected { PALETTE.bg_window } else { PALETTE.text })
                .size(12.0);
            let btn = egui::Button::new(text)
                .fill(if selected { PALETTE.accent } else { PALETTE.bg_elevated })
                .stroke(Stroke::new(1.0_f32, PALETTE.border))
                .rounding(Rounding::same(999.0));
            if ui.add(btn).clicked() {
                state.selected_root = i;
            }
        }
    });
    ui.add_space(6.0);

    let Some(root) = watches
        .get(state.selected_root)
        .or_else(|| watches.first())
    else {
        return;
    };
    let items = match engine.index.bytes_per_child(root) {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => {
            empty_state(ui, "No files indexed yet. The scanner runs at startup and on 'Scan now'.");
            return;
        }
        Err(e) => {
            ui.colored_label(PALETTE.danger, format!("treemap query failed: {e}"));
            return;
        }
    };

    let total: u64 = items.iter().map(|i| i.bytes).sum();
    if total == 0 {
        empty_state(ui, "Indexed files have zero total size.");
        return;
    }

    let desired = Vec2::new(ui.available_width(), 340.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(8.0), PALETTE.bg_window);

    let layout_rects = squarify(rect.shrink(4.0), &items.iter().map(|c| c.bytes).collect::<Vec<_>>(), total);
    let hover = response.hover_pos();
    let click_pos = if response.clicked() { response.interact_pointer_pos() } else { None };
    state.hovered_index = None;
    let mut clicked_cell: Option<usize> = None;

    for (i, (cell, r)) in items.iter().zip(layout_rects.iter()).enumerate() {
        let color = theme::tier_color(cell.dominant_tier);
        let hovered = hover.map(|p| r.contains(p)).unwrap_or(false);
        if hovered {
            state.hovered_index = Some(i);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if let Some(p) = click_pos {
            if r.contains(p) {
                clicked_cell = Some(i);
            }
        }
        let fill = if hovered { color } else { color.linear_multiply(0.75) };
        painter.rect_filled(*r, Rounding::same(4.0), fill);
        painter.rect_stroke(*r, Rounding::same(4.0), Stroke::new(1.0_f32, PALETTE.bg_window));

        if r.width() > 70.0 && r.height() > 36.0 {
            let name = &cell.name;
            let size_str = humansize::format_size(cell.bytes, humansize::DECIMAL);
            painter.text(
                r.left_top() + Vec2::new(8.0, 6.0),
                egui::Align2::LEFT_TOP,
                name,
                egui::FontId::proportional(12.5),
                Color32::from_rgb(16, 20, 30),
            );
            painter.text(
                r.left_top() + Vec2::new(8.0, 22.0),
                egui::Align2::LEFT_TOP,
                size_str,
                egui::FontId::proportional(11.0),
                Color32::from_rgba_unmultiplied(16, 20, 30, 180),
            );
        }
    }

    if let Some(i) = state.hovered_index {
        if let Some(c) = items.get(i) {
            response.clone().on_hover_text(format!(
                "{}\n{} · {} files · click to drill in",
                c.name,
                humansize::format_size(c.bytes, humansize::DECIMAL),
                c.files,
            ));
        }
    }

    if let Some(i) = clicked_cell {
        if let Some(c) = items.get(i) {
            state.drill_path = Some(root.join(&c.name));
        }
    }

    // Drill-in listing below the treemap.
    if let Some(drill) = state.drill_path.clone() {
        ui.add_space(10.0);
        draw_drill_in(ui, state, engine, &drill);
    }
}

fn draw_drill_in(
    ui: &mut egui::Ui,
    state: &mut TreemapState,
    engine: &EngineHandle,
    drill: &std::path::Path,
) {
    egui::Frame::none()
        .fill(PALETTE.bg_card)
        .stroke(Stroke::new(1.0_f32, PALETTE.border))
        .rounding(Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let back = egui::Button::new(
                    egui::RichText::new("←  back to treemap").color(PALETTE.text),
                )
                .fill(PALETTE.bg_elevated)
                .rounding(Rounding::same(6.0));
                if ui.add(back).clicked() {
                    state.drill_path = None;
                }
                ui.add_space(8.0);
                ui.add(egui::Label::new(
                    egui::RichText::new(drill.display().to_string())
                        .color(PALETTE.text)
                        .monospace()
                        .size(12.5),
                ));
            });
            ui.add_space(8.0);

            let files = match engine.index.files_under(drill, 500) {
                Ok(v) => v,
                Err(e) => {
                    ui.colored_label(PALETTE.danger, format!("query failed: {e}"));
                    return;
                }
            };
            if files.is_empty() {
                ui.colored_label(
                    PALETTE.text_dim,
                    "No files indexed under this subfolder yet (scan may still be in progress).",
                );
                return;
            }

            let total: u64 = files.iter().map(|f| f.size).sum();
            ui.add(egui::Label::new(
                egui::RichText::new(format!(
                    "{} files · {} · largest first",
                    files.len(),
                    humansize::format_size(total, humansize::DECIMAL),
                ))
                .color(PALETTE.text_dim)
                .size(11.5),
            ));
            ui.add_space(4.0);

            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for f in files {
                    file_row(ui, drill, &f);
                }
            });
        });
}

fn file_row(ui: &mut egui::Ui, root: &std::path::Path, f: &crate::engine::index::FileRow) {
    egui::Frame::none()
        .fill(PALETTE.bg_elevated)
        .rounding(Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let tier_c = f.tier.map(theme::tier_color).unwrap_or(PALETTE.text_faint);
                let tier_g = f.tier.map(theme::tier_glyph).unwrap_or("·");
                ui.colored_label(tier_c, tier_g);
                let rel = f.path.strip_prefix(root).unwrap_or(&f.path);
                ui.add(egui::Label::new(
                    egui::RichText::new(rel.display().to_string())
                        .color(PALETTE.text)
                        .monospace()
                        .size(12.0),
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reveal").clicked() {
                        crate::osx::reveal_in_finder(&f.path);
                    }
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(
                        egui::RichText::new(humansize::format_size(f.size, humansize::DECIMAL))
                            .color(PALETTE.text_dim)
                            .monospace()
                            .size(12.0),
                    ));
                });
            });
        });
    ui.add_space(2.0);
}

fn empty_state(ui: &mut egui::Ui, msg: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 180.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(8.0), PALETTE.bg_window);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        msg,
        egui::FontId::proportional(13.0),
        PALETTE.text_faint,
    );
}

/// Classic squarified treemap (Bruls, Huijsen, van Wijk). Takes pre-sorted
/// (descending) weights and lays them out inside `area`.
fn squarify(area: Rect, values: &[u64], total: u64) -> Vec<Rect> {
    let n = values.len();
    let mut out = vec![Rect::NOTHING; n];
    if total == 0 || n == 0 {
        return out;
    }

    // Scale weights to area units so the ratios map directly to pixels.
    let scale = (area.width() * area.height()) / total as f32;
    let scaled: Vec<f32> = values.iter().map(|v| (*v as f32) * scale).collect();

    layout(&scaled, (0..n).collect(), area, &mut out);
    out
}

fn layout(scaled: &[f32], indices: Vec<usize>, mut area: Rect, out: &mut [Rect]) {
    let mut remaining = indices;
    while !remaining.is_empty() {
        let (row, rest) = greedy_row(scaled, &remaining, shortest_side(area));
        place_row(scaled, &row, &mut area, out);
        remaining = rest;
    }
}

fn greedy_row(scaled: &[f32], indices: &[usize], w: f32) -> (Vec<usize>, Vec<usize>) {
    let mut row = Vec::new();
    let mut best_ratio = f32::INFINITY;
    for (i, idx) in indices.iter().enumerate() {
        let mut trial = row.clone();
        trial.push(*idx);
        let ratio = worst_ratio(scaled, &trial, w);
        if ratio > best_ratio {
            let rest = indices[i..].to_vec();
            return (row, rest);
        }
        row.push(*idx);
        best_ratio = ratio;
    }
    (row, Vec::new())
}

fn worst_ratio(scaled: &[f32], row: &[usize], w: f32) -> f32 {
    let s: f32 = row.iter().map(|i| scaled[*i]).sum();
    let (rmax, rmin) = row.iter().fold((0.0_f32, f32::INFINITY), |(mx, mn), i| {
        let v = scaled[*i];
        (mx.max(v), mn.min(v))
    });
    let w2 = w * w;
    let s2 = s * s;
    (w2 * rmax / s2.max(1e-6)).max(s2 / (w2 * rmin).max(1e-6))
}

fn shortest_side(r: Rect) -> f32 {
    r.width().min(r.height())
}

fn place_row(scaled: &[f32], row: &[usize], area: &mut Rect, out: &mut [Rect]) {
    let s: f32 = row.iter().map(|i| scaled[*i]).sum();
    let horizontal = area.width() >= area.height();
    if horizontal {
        let h = (s / area.width()).min(area.height());
        let mut x = area.left();
        for &i in row {
            let w = scaled[i] / h.max(1e-6);
            out[i] = Rect::from_min_size(
                egui::pos2(x, area.top()),
                egui::vec2(w, h),
            );
            x += w;
        }
        area.set_top(area.top() + h);
    } else {
        let w = (s / area.height()).min(area.width());
        let mut y = area.top();
        for &i in row {
            let h = scaled[i] / w.max(1e-6);
            out[i] = Rect::from_min_size(
                egui::pos2(area.left(), y),
                egui::vec2(w, h),
            );
            y += h;
        }
        area.set_left(area.left() + w);
    }
}

