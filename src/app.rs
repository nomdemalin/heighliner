use eframe::egui::{self, Color32, Margin, Rounding, Stroke};

use crate::engine::EngineHandle;
use crate::ui;
use crate::ui::theme::{self, PALETTE};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Watched,
    Rules,
    Tiers,
    Log,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Dashboard, Tab::Watched, Tab::Rules, Tab::Tiers, Tab::Log];

    fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Watched => "Watched",
            Tab::Rules => "Rules",
            Tab::Tiers => "Tiers",
            Tab::Log => "Log",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Tab::Dashboard => "◱",
            Tab::Watched => "◉",
            Tab::Rules => "⚙",
            Tab::Tiers => "❄",
            Tab::Log => "≡",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Dashboard => "Overview and space usage",
            Tab::Watched => "Directories Heighliner observes",
            Tab::Rules => "Match files, pick actions",
            Tab::Tiers => "Hot / warm / cold / frozen policy",
            Tab::Log => "Everything the engine has done",
        }
    }
}

pub struct HeighlinerApp {
    pub engine: EngineHandle,
    pub tab: Tab,
    pub dashboard: ui::dashboard::DashboardState,
    pub watched: ui::watched::WatchedState,
    pub rules: ui::rules::RulesState,
    pub tiers: ui::tiers::TiersState,
    pub log: ui::log::LogState,
    theme_applied: bool,
}

impl HeighlinerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, engine: EngineHandle) -> Self {
        theme::apply(&cc.egui_ctx);
        Self {
            engine,
            tab: Tab::Dashboard,
            dashboard: ui::dashboard::DashboardState::default(),
            watched: ui::watched::WatchedState::default(),
            rules: ui::rules::RulesState::default(),
            tiers: ui::tiers::TiersState::default(),
            log: ui::log::LogState::default(),
            theme_applied: true,
        }
    }
}

impl eframe::App for HeighlinerApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let [r, g, b, _] = PALETTE.bg_window.to_array();
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }

        self.header(ctx);
        self.sidebar(ctx);
        self.status_bar(ctx);
        self.body(ctx);

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl HeighlinerApp {
    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .exact_height(52.0)
            .frame(
                egui::Frame::none()
                    .fill(PALETTE.bg_sidebar)
                    .stroke(Stroke::new(1.0_f32, PALETTE.border))
                    .inner_margin(Margin::symmetric(16.0, 8.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // Painted brand mark — the Heighliner wedge.
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(30.0, 32.0),
                        egui::Sense::hover(),
                    );
                    theme::paint_logo(ui.painter(), rect.center(), 30.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(
                            egui::RichText::new("Heighliner").size(16.0).strong().color(PALETTE.text),
                        ));
                        ui.add(egui::Label::new(
                            egui::RichText::new("Keep your filesystem in order")
                                .size(10.5)
                                .color(PALETTE.text_faint),
                        ));
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let cfg = self.engine.config.lock();
                        let dry = cfg.general.dry_run;
                        drop(cfg);

                        if ui
                            .add(egui::Button::new(egui::RichText::new("Scan now").color(PALETTE.text)))
                            .on_hover_text("Re-walk every watched root")
                            .clicked()
                        {
                            self.engine.request_scan_now();
                        }

                        ui.add_space(4.0);

                        let dry_text = if dry { "Dry run · ON" } else { "Dry run · off" };
                        let dry_color = if dry { PALETTE.warning } else { PALETTE.text_dim };
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new(dry_text).color(dry_color).strong(),
                            ).fill(if dry { PALETTE.bg_elevated } else { PALETTE.bg_card }))
                            .on_hover_text("When ON, actions are logged but not applied")
                            .clicked()
                        {
                            if let Err(e) = self.engine.toggle_dry_run() {
                                self.engine.set_error(format!("toggle dry-run: {e}"));
                            }
                        }
                    });
                });
            });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(190.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(PALETTE.bg_sidebar)
                    .stroke(Stroke::new(1.0_f32, PALETTE.border))
                    .inner_margin(Margin::symmetric(10.0, 14.0)),
            )
            .show(ctx, |ui| {
                for t in Tab::ALL {
                    nav_item(ui, t, self.tab == t, || self.tab = t);
                    ui.add_space(4.0);
                }
                ui.add_space(20.0);
                ui.add(egui::Separator::default().spacing(2.0));
                ui.add_space(10.0);
                ui.label(egui::RichText::new("TIERS").size(10.0).color(PALETTE.text_faint).strong());
                for t in crate::engine::config::Tier::ALL {
                    ui.horizontal(|ui| {
                        ui.colored_label(theme::tier_color(t), theme::tier_glyph(t));
                        ui.label(egui::RichText::new(t.dir_name()).color(PALETTE.text_dim));
                    });
                }
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(28.0)
            .frame(
                egui::Frame::none()
                    .fill(PALETTE.bg_sidebar)
                    .stroke(Stroke::new(1.0_f32, PALETTE.border))
                    .inner_margin(Margin::symmetric(14.0, 4.0)),
            )
            .show(ctx, |ui| {
                let stats = self.engine.stats_snapshot();
                ui.horizontal_centered(|ui| {
                    let scanning = self.engine.scanning();
                    ui.colored_label(
                        if scanning { PALETTE.warning } else { PALETTE.success },
                        if scanning { "● scanning" } else { "● idle" },
                    );
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(format!(
                        "{} files · {} · {} actions",
                        humansize::format_size(stats.file_count as u64, humansize::DECIMAL)
                            .trim_end_matches(" B").to_string()
                            .parse::<f64>()
                            .map(|_| stats.file_count.to_string())
                            .unwrap_or_else(|_| stats.file_count.to_string()),
                        humansize::format_size(stats.bytes_managed, humansize::DECIMAL),
                        stats.actions_run,
                    )).color(PALETTE.text_dim).size(12.0));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(err) = self.engine.last_error() {
                            ui.colored_label(PALETTE.danger, format!("⚠ {err}"));
                        } else if let Some(act) = self.engine.last_activity() {
                            ui.colored_label(PALETTE.text_dim, act);
                        }
                    });
                });
            });
    }

    fn body(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(PALETTE.bg_panel)
                    .inner_margin(Margin::symmetric(22.0, 18.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Dashboard => ui::dashboard::show(ui, &mut self.dashboard, &self.engine),
                    Tab::Watched => ui::watched::show(ui, &mut self.watched, &self.engine),
                    Tab::Rules => ui::rules::show(ui, &mut self.rules, &self.engine),
                    Tab::Tiers => ui::tiers::show(ui, &mut self.tiers, &self.engine),
                    Tab::Log => ui::log::show(ui, &mut self.log, &self.engine),
                });
            });
    }
}

fn nav_item(ui: &mut egui::Ui, tab: Tab, selected: bool, mut on_click: impl FnMut()) {
    let (fill, fg) = if selected {
        (PALETTE.bg_elevated, PALETTE.accent)
    } else {
        (Color32::TRANSPARENT, PALETTE.text_dim)
    };
    let resp = egui::Frame::none()
        .fill(fill)
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::symmetric(10.0, 7.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new(tab.glyph()).color(fg).size(15.0),
                ));
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new(tab.label())
                            .color(if selected { PALETTE.text } else { PALETTE.text_dim })
                            .strong(),
                    ));
                    ui.add(egui::Label::new(
                        egui::RichText::new(tab.hint()).size(10.5).color(PALETTE.text_faint),
                    ));
                });
            });
        })
        .response
        .interact(egui::Sense::click());
    if resp.clicked() {
        on_click();
    }
    if resp.hovered() && !selected {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}
