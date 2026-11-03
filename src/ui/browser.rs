use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, Rounding, Sense, Stroke};

use crate::engine::index::{FileRow, FileSort};
use crate::engine::EngineHandle;
use crate::ui::theme::{self, PALETTE};

/// How many rows we pull from the index for a given drill. If the user needs
/// to go past this, they probably want to filter / change sort rather than
/// scroll a mile.
const BROWSER_ROW_CAP: u32 = 10_000;
const ROW_HEIGHT: f32 = 30.0;

pub struct BrowserState {
    pub sort: FileSort,
    pub descending: bool,
    pub filter: String,
    pub rows: Vec<FileRow>,
    /// (root, sort, descending) that `rows` was fetched for. Filter is applied
    /// client-side so it isn't part of this key.
    pub loaded_for: Option<(PathBuf, FileSort, bool)>,
    /// P13: totals cached alongside rows; the browser header no longer runs
    /// `totals_under` every repaint.
    pub totals: Option<(u64, u64)>,
    /// P10: memoized filtered index into `rows`, keyed by the (loaded_for,
    /// filter) combo so re-renders don't re-lowercase every path.
    pub filtered: Vec<usize>,
    pub filter_key: Option<(PathBuf, FileSort, bool, String)>,
    pub last_error: Option<String>,
    pub last_trashed: Option<PathBuf>,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            sort: FileSort::Size,
            descending: true,
            filter: String::new(),
            rows: Vec::new(),
            loaded_for: None,
            totals: None,
            filtered: Vec::new(),
            filter_key: None,
            last_error: None,
            last_trashed: None,
        }
    }
}

/// Render the drill-in file browser for `root`. Returns `true` while the
/// browser is active; the caller clears their drill_path when the back
/// button inside the view is hit (we signal via `on_back`).
pub fn show(
    ui: &mut egui::Ui,
    state: &mut BrowserState,
    engine: &EngineHandle,
    root: &Path,
    on_back: &mut dyn FnMut(),
) {
    ensure_loaded(state, engine, root);

    // Header strip: back + path + totals + refresh.
    egui::Frame::none()
        .fill(PALETTE.bg_card)
        .stroke(Stroke::new(1.0_f32, PALETTE.border))
        .rounding(Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let back = egui::Button::new(
                    egui::RichText::new("←  back").color(PALETTE.text),
                )
                .fill(PALETTE.bg_elevated)
                .rounding(Rounding::same(6.0));
                if ui.add(back).clicked() {
                    on_back();
                    return;
                }
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new(root.display().to_string())
                            .color(PALETTE.text)
                            .monospace()
                            .size(13.0),
                    ));
                    let (count, bytes) = state.totals.unwrap_or((0, 0));
                    ui.add(egui::Label::new(
                        egui::RichText::new(format!(
                            "{} files · {} total",
                            count,
                            humansize::format_size(bytes, humansize::DECIMAL),
                        ))
                        .color(PALETTE.text_dim)
                        .size(11.5),
                    ));
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Refresh").clicked() {
                        state.loaded_for = None;
                        state.filter_key = None;
                    }
                });
            });
        });

    ui.add_space(8.0);

    // Filter row.
    egui::Frame::none()
        .fill(PALETTE.bg_card)
        .stroke(Stroke::new(1.0_f32, PALETTE.border))
        .rounding(Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new("Filter").color(PALETTE.text_dim).size(12.0),
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .desired_width(260.0)
                        .hint_text("substring of path / name"),
                );
                if !state.filter.is_empty() && ui.button("Clear").clicked() {
                    state.filter.clear();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if state.rows.len() as u32 == BROWSER_ROW_CAP {
                        ui.colored_label(
                            PALETTE.warning,
                            format!("showing first {BROWSER_ROW_CAP} — tighten sort/filter to see more"),
                        );
                    }
                });
            });
        });

    if let Some(err) = &state.last_error {
        ui.add_space(4.0);
        ui.colored_label(PALETTE.danger, format!("Error: {err}"));
    }
    if let Some(trashed) = &state.last_trashed {
        ui.add_space(4.0);
        ui.colored_label(
            PALETTE.success,
            format!("Trashed: {}", trashed.display()),
        );
    }

    ui.add_space(8.0);

    // Header row.
    header_row(ui, state);
    ui.add_space(2.0);

    // P10: rebuild `state.filtered` only when (loaded_for, filter) changes.
    let want_filter_key = state
        .loaded_for
        .as_ref()
        .map(|(p, s, d)| (p.clone(), *s, *d, state.filter.clone()));
    if want_filter_key != state.filter_key {
        let filter_lower = state.filter.to_lowercase();
        state.filtered = if filter_lower.is_empty() {
            (0..state.rows.len()).collect()
        } else {
            state
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.path.to_string_lossy().to_lowercase().contains(&filter_lower))
                .map(|(i, _)| i)
                .collect()
        };
        state.filter_key = want_filter_key;
    }

    if state.filtered.is_empty() {
        egui::Frame::none()
            .fill(PALETTE.bg_card)
            .stroke(Stroke::new(1.0_f32, PALETTE.border))
            .rounding(Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(14.0, 14.0))
            .show(ui, |ui| {
                ui.colored_label(PALETTE.text_dim, "No files match. Try clearing the filter.");
            });
        return;
    }

    // Virtualized scroll area: egui only renders the visible rows (plus a
    // small overscan), so a 10,000-file browse stays snappy.
    let total = state.filtered.len();
    let mut trash_target: Option<PathBuf> = None;
    let mut reveal_target: Option<PathBuf> = None;

    egui::Frame::none()
        .fill(PALETTE.bg_card)
        .stroke(Stroke::new(1.0_f32, PALETTE.border))
        .rounding(Rounding::same(8.0))
        .inner_margin(egui::Margin::symmetric(2.0, 2.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(ui.available_height().max(300.0))
                .show_rows(ui, ROW_HEIGHT, total, |ui, visible| {
                    for i in visible {
                        let idx = state.filtered[i];
                        let row = &state.rows[idx];
                        row_view(ui, row, root, &mut trash_target, &mut reveal_target);
                    }
                });
        });

    // Apply side-effects AFTER the scroll area finishes so we don't mutate
    // state.rows while iterating a borrowed view.
    if let Some(p) = reveal_target {
        crate::osx::reveal_in_finder(&p);
    }
    if let Some(p) = trash_target {
        match crate::osx::move_to_trash(&p) {
            Ok(()) => {
                let _ = engine.index.delete_path(&p);
                state.rows.retain(|r| r.path != p);
                state.last_trashed = Some(p);
                state.last_error = None;
                // Invalidate the memoized filter so the trashed row disappears.
                state.filter_key = None;
                // Totals need a refresh too.
                state.totals = engine.index.totals_under(root).ok();
            }
            Err(e) => {
                state.last_error = Some(format!("trash failed: {e}"));
            }
        }
    }
}

fn ensure_loaded(state: &mut BrowserState, engine: &EngineHandle, root: &Path) {
    let want = (root.to_path_buf(), state.sort, state.descending);
    if state.loaded_for.as_ref() == Some(&want) {
        return;
    }
    match engine
        .index
        .files_under_sorted(root, state.sort, state.descending, BROWSER_ROW_CAP)
    {
        Ok(rows) => {
            state.rows = rows;
            state.loaded_for = Some(want);
            state.filter_key = None; // force filtered rebuild
            state.last_error = None;
        }
        Err(e) => {
            state.rows.clear();
            state.last_error = Some(format!("index query failed: {e}"));
        }
    }
    // P13: refresh totals exactly when the rowset reloads.
    state.totals = engine.index.totals_under(root).ok();
}

fn header_row(ui: &mut egui::Ui, state: &mut BrowserState) {
    egui::Frame::none()
        .fill(PALETTE.bg_sidebar)
        .stroke(Stroke::new(1.0_f32, PALETTE.border))
        .rounding(Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Tier dot column (fixed width, no sort target).
                ui.add_sized([18.0, 18.0], egui::Label::new(""));
                // Path header — flexible.
                sort_header(ui, state, FileSort::Path, "Path", true);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_sized([80.0, 18.0], egui::Label::new("")); // actions column
                    ui.add_space(6.0);
                    sort_header_right(ui, state, FileSort::LastAccess, "Last used", 110.0);
                    sort_header_right(ui, state, FileSort::Modified, "Modified", 110.0);
                    sort_header_right(ui, state, FileSort::Size, "Size", 100.0);
                });
            });
        });
}

fn sort_header(ui: &mut egui::Ui, state: &mut BrowserState, col: FileSort, label: &str, flex: bool) {
    let selected = state.sort == col;
    let arrow = if selected { if state.descending { " ▼" } else { " ▲" } } else { "" };
    let text = egui::RichText::new(format!("{label}{arrow}"))
        .color(if selected { PALETTE.accent } else { PALETTE.text_dim })
        .size(11.5)
        .strong();
    let btn = egui::Button::new(text)
        .fill(Color32::TRANSPARENT)
        .frame(false)
        .min_size(egui::vec2(if flex { 0.0 } else { 100.0 }, 18.0));
    let resp = ui.add(btn.sense(Sense::click()));
    if resp.clicked() {
        toggle_sort(state, col);
    }
}

fn sort_header_right(ui: &mut egui::Ui, state: &mut BrowserState, col: FileSort, label: &str, w: f32) {
    let selected = state.sort == col;
    let arrow = if selected { if state.descending { " ▼" } else { " ▲" } } else { "" };
    let text = egui::RichText::new(format!("{label}{arrow}"))
        .color(if selected { PALETTE.accent } else { PALETTE.text_dim })
        .size(11.5)
        .strong();
    let btn = egui::Button::new(text)
        .fill(Color32::TRANSPARENT)
        .frame(false)
        .min_size(egui::vec2(w, 18.0));
    let resp = ui.add(btn.sense(Sense::click()));
    if resp.clicked() {
        toggle_sort(state, col);
    }
}

fn toggle_sort(state: &mut BrowserState, col: FileSort) {
    if state.sort == col {
        state.descending = !state.descending;
    } else {
        state.sort = col;
        state.descending = matches!(col, FileSort::Size | FileSort::Modified | FileSort::LastAccess);
    }
    state.loaded_for = None;
}

fn row_view(
    ui: &mut egui::Ui,
    row: &FileRow,
    root: &Path,
    trash: &mut Option<PathBuf>,
    reveal: &mut Option<PathBuf>,
) {
    ui.horizontal(|ui| {
        let tier_c = row.tier.map(theme::tier_color).unwrap_or(PALETTE.text_faint);
        let tier_g = row.tier.map(theme::tier_glyph).unwrap_or("·");
        ui.add_sized([18.0, 18.0], egui::Label::new(egui::RichText::new(tier_g).color(tier_c)));

        let rel = row.path.strip_prefix(root).unwrap_or(&row.path);
        let rel_s = rel.display().to_string();
        let name_label = egui::Label::new(
            egui::RichText::new(rel_s)
                .color(PALETTE.text)
                .monospace()
                .size(12.5),
        )
        .truncate();
        ui.add(name_label);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Actions.
            let trash_btn = egui::Button::new(
                egui::RichText::new("Trash").color(PALETTE.danger),
            )
            .fill(PALETTE.bg_elevated)
            .rounding(Rounding::same(5.0));
            if ui.add(trash_btn).on_hover_text("Move to macOS Trash").clicked() {
                *trash = Some(row.path.clone());
            }
            ui.add_space(2.0);
            let reveal_btn = egui::Button::new(
                egui::RichText::new("Reveal").color(PALETTE.text),
            )
            .fill(PALETTE.bg_elevated)
            .rounding(Rounding::same(5.0));
            if ui.add(reveal_btn).on_hover_text("Open the enclosing folder").clicked() {
                *reveal = Some(row.path.clone());
            }
            ui.add_space(10.0);

            // Last access.
            let last_s = relative_time(row.last_user_access);
            ui.add_sized(
                [110.0, 18.0],
                egui::Label::new(
                    egui::RichText::new(last_s)
                        .color(PALETTE.text_dim)
                        .monospace()
                        .size(11.5),
                ),
            );
            // Modified.
            let mod_s = relative_time(row.modified);
            ui.add_sized(
                [110.0, 18.0],
                egui::Label::new(
                    egui::RichText::new(mod_s)
                        .color(PALETTE.text_dim)
                        .monospace()
                        .size(11.5),
                ),
            );
            // Size.
            ui.add_sized(
                [100.0, 18.0],
                egui::Label::new(
                    egui::RichText::new(humansize::format_size(row.size, humansize::DECIMAL))
                        .color(PALETTE.text)
                        .monospace()
                        .size(12.0),
                ),
            );
        });
    });
}

fn relative_time(ts: Option<i64>) -> String {
    let Some(ts) = ts else {
        return "—".into();
    };
    let now = chrono::Utc::now().timestamp();
    let secs = (now - ts).max(0) as u64;
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 86_400 * 60 {
        format!("{}d ago", secs / 86_400)
    } else {
        let dt: Option<chrono::DateTime<chrono::Local>> =
            chrono::Local.timestamp_opt(ts, 0).single();
        match dt {
            Some(d) => d.format("%Y-%m-%d").to_string(),
            None => "—".into(),
        }
    }
}

// Silence unused-import warnings on cfg-gated items.
#[allow(dead_code)]
fn _marker(_: SystemTime, _: Duration, _: Color32) {}

use chrono::TimeZone;
