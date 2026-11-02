use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, Rounding};

use crate::engine::config::{Action, Match, Rule, Tier};
use crate::engine::EngineHandle;
use crate::ui::theme::{self, card, section_header, PALETTE};

pub struct RulesState {
    pub editing: Option<Draft>,
    pub show_raw: bool,
    pub raw_buffer: String,
}

impl Default for RulesState {
    fn default() -> Self {
        Self {
            editing: None,
            show_raw: false,
            raw_buffer: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct Draft {
    pub index: Option<usize>, // None = new
    pub name: String,
    pub extensions: String,     // comma-separated
    pub name_glob: String,      // comma-separated
    pub path_glob: String,
    pub source_prefix: String,
    pub older_than: String,     // humantime, e.g. "90d"
    pub in_tier: Option<Tier>,
    pub min_size_bytes: String,
    pub max_size_bytes: String,
    pub action_kind: ActionKind,
    pub action_to: String,
    pub action_tier: Tier,
    pub action_delete_source: bool,
    pub action_delete_after: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Move,
    Retier,
    Archive,
    DeleteAfter,
}

impl ActionKind {
    const ALL: [ActionKind; 4] = [Self::Move, Self::Retier, Self::Archive, Self::DeleteAfter];
    fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Retier => "Retier",
            Self::Archive => "Archive",
            Self::DeleteAfter => "Delete after",
        }
    }
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            index: None,
            name: String::new(),
            extensions: String::new(),
            name_glob: String::new(),
            path_glob: String::new(),
            source_prefix: String::new(),
            older_than: String::new(),
            in_tier: None,
            min_size_bytes: String::new(),
            max_size_bytes: String::new(),
            action_kind: ActionKind::Move,
            action_to: String::new(),
            action_tier: Tier::Cold,
            action_delete_source: true,
            action_delete_after: String::new(),
            enabled: true,
        }
    }
}

impl Draft {
    fn from_rule(idx: usize, r: &Rule) -> Self {
        let m = &r.r#match;
        let (ak, to, tier, del_src, del_after) = match &r.action {
            Action::Move { to } => (ActionKind::Move, to.display().to_string(), Tier::Cold, true, String::new()),
            Action::Retier { to } => (ActionKind::Retier, String::new(), *to, true, String::new()),
            Action::Archive { to, delete_source } => (ActionKind::Archive, to.display().to_string(), Tier::Frozen, *delete_source, String::new()),
            Action::DeleteAfter { after } => (ActionKind::DeleteAfter, String::new(), Tier::Frozen, false, humantime::format_duration(*after).to_string()),
        };
        Self {
            index: Some(idx),
            name: r.name.clone(),
            extensions: m.extension.join(", "),
            name_glob: m.name_glob.join(", "),
            path_glob: m.path_glob.join(", "),
            source_prefix: m.source_prefix.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            older_than: m.older_than.map(|d| humantime::format_duration(d).to_string()).unwrap_or_default(),
            in_tier: m.in_tier,
            min_size_bytes: m.min_size.map(|v| v.to_string()).unwrap_or_default(),
            max_size_bytes: m.max_size.map(|v| v.to_string()).unwrap_or_default(),
            action_kind: ak,
            action_to: to,
            action_tier: tier,
            action_delete_source: del_src,
            action_delete_after: del_after,
            enabled: r.enabled.unwrap_or(true),
        }
    }

    fn to_rule(&self) -> Result<Rule, String> {
        let mut m = Match::default();
        if !self.extensions.trim().is_empty() {
            m.extension = split_csv(&self.extensions);
        }
        if !self.name_glob.trim().is_empty() {
            m.name_glob = split_csv(&self.name_glob);
        }
        if !self.path_glob.trim().is_empty() {
            m.path_glob = split_csv(&self.path_glob);
        }
        if !self.source_prefix.trim().is_empty() {
            m.source_prefix = Some(PathBuf::from(self.source_prefix.trim()));
        }
        if !self.older_than.trim().is_empty() {
            m.older_than = Some(parse_dur(&self.older_than)?);
        }
        m.in_tier = self.in_tier;
        if !self.min_size_bytes.trim().is_empty() {
            m.min_size = Some(self.min_size_bytes.trim().parse().map_err(|e: std::num::ParseIntError| e.to_string())?);
        }
        if !self.max_size_bytes.trim().is_empty() {
            m.max_size = Some(self.max_size_bytes.trim().parse().map_err(|e: std::num::ParseIntError| e.to_string())?);
        }
        let action = match self.action_kind {
            ActionKind::Move => Action::Move {
                to: PathBuf::from(self.action_to.trim()),
            },
            ActionKind::Retier => Action::Retier { to: self.action_tier },
            ActionKind::Archive => Action::Archive {
                to: PathBuf::from(self.action_to.trim()),
                delete_source: self.action_delete_source,
            },
            ActionKind::DeleteAfter => Action::DeleteAfter {
                after: parse_dur(&self.action_delete_after)?,
            },
        };
        if self.name.trim().is_empty() {
            return Err("Rule needs a name.".into());
        }
        Ok(Rule {
            name: self.name.trim().to_string(),
            enabled: Some(self.enabled),
            r#match: m,
            action,
        })
    }
}

fn parse_dur(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s.trim()).map_err(|e| e.to_string())
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

pub fn show(ui: &mut egui::Ui, state: &mut RulesState, engine: &EngineHandle) {
    section_header(
        ui,
        "Rules",
        "Declarative matchers → actions. Backed by rules.toml.",
    );

    ui.horizontal(|ui| {
        let btn = egui::Button::new(
            egui::RichText::new("+  New rule").color(PALETTE.bg_window).strong(),
        )
        .fill(PALETTE.accent)
        .rounding(Rounding::same(6.0));
        if ui.add(btn).clicked() {
            state.editing = Some(Draft::default());
            state.show_raw = false;
        }
        ui.add_space(6.0);
        let raw_label = if state.show_raw { "Hide raw TOML" } else { "View raw TOML" };
        if ui.button(raw_label).clicked() {
            state.show_raw = !state.show_raw;
            if state.show_raw {
                if let Ok(s) = std::fs::read_to_string(&engine.paths.config_file) {
                    state.raw_buffer = s;
                }
            }
        }
    });

    ui.add_space(10.0);

    if let Some(draft) = state.editing.clone() {
        draw_editor(ui, state, engine, draft);
        ui.add_space(10.0);
    }

    // List existing rules.
    let cfg = engine.config.lock();
    if cfg.rules.is_empty() {
        card(ui, |ui| {
            ui.colored_label(PALETTE.text_dim, "No rules yet. Click +New rule to add one.");
        });
    } else {
        for (i, r) in cfg.rules.iter().enumerate() {
            draw_rule_card(ui, i, r, &mut state.editing);
            ui.add_space(8.0);
        }
    }
    drop(cfg);

    if state.show_raw {
        ui.add_space(12.0);
        card(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("Raw: {}", engine.paths.config_file.display()))
                    .color(PALETTE.text_dim)
                    .size(11.5),
            );
            ui.add(
                egui::TextEdit::multiline(&mut state.raw_buffer)
                    .code_editor()
                    .desired_rows(14)
                    .desired_width(f32::INFINITY),
            );
            if ui.button("Save raw TOML").clicked() {
                let parsed = toml::from_str::<crate::engine::config::Config>(&state.raw_buffer);
                match parsed {
                    Ok(parsed) => match parsed.save(&engine.paths.config_file) {
                        Ok(()) => {
                            *engine.config.lock() = parsed;
                            engine.clear_error();
                            engine.reload_config();
                        }
                        Err(e) => engine.set_error(format!("save: {e}")),
                    },
                    Err(e) => engine.set_error(format!("parse rules.toml: {e}")),
                }
            }
        });
    }
}

fn draw_editor(ui: &mut egui::Ui, state: &mut RulesState, engine: &EngineHandle, mut draft: Draft) {
    card(ui, |ui| {
        ui.add(egui::Label::new(
            egui::RichText::new(if draft.index.is_some() { "Edit rule" } else { "New rule" })
                .color(PALETTE.text)
                .strong()
                .size(14.0),
        ));
        ui.add_space(8.0);

        egui::Grid::new("rule_editor").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut draft.name);
            ui.end_row();

            ui.label("Enabled");
            ui.checkbox(&mut draft.enabled, "");
            ui.end_row();

            ui.label(egui::RichText::new("Match").color(PALETTE.accent).strong());
            ui.label("");
            ui.end_row();

            ui.label("Extensions");
            ui.text_edit_singleline(&mut draft.extensions);
            ui.end_row();

            ui.label("Name glob");
            ui.text_edit_singleline(&mut draft.name_glob);
            ui.end_row();

            ui.label("Path glob");
            ui.text_edit_singleline(&mut draft.path_glob);
            ui.end_row();

            ui.label("Source prefix");
            ui.text_edit_singleline(&mut draft.source_prefix);
            ui.end_row();

            ui.label("Older than");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut draft.older_than);
                ui.label(egui::RichText::new("e.g. 7d, 2mo").color(PALETTE.text_faint).size(11.0));
            });
            ui.end_row();

            ui.label("In tier");
            egui::ComboBox::from_id_salt("in_tier")
                .selected_text(match draft.in_tier {
                    Some(t) => t.dir_name(),
                    None => "any",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.in_tier, None, "any");
                    for t in Tier::ALL {
                        ui.selectable_value(&mut draft.in_tier, Some(t), t.dir_name());
                    }
                });
            ui.end_row();

            ui.label("Min size (bytes)");
            ui.text_edit_singleline(&mut draft.min_size_bytes);
            ui.end_row();

            ui.label("Max size (bytes)");
            ui.text_edit_singleline(&mut draft.max_size_bytes);
            ui.end_row();

            ui.label(egui::RichText::new("Action").color(PALETTE.accent).strong());
            ui.label("");
            ui.end_row();

            ui.label("Kind");
            egui::ComboBox::from_id_salt("action_kind")
                .selected_text(draft.action_kind.label())
                .show_ui(ui, |ui| {
                    for k in ActionKind::ALL {
                        ui.selectable_value(&mut draft.action_kind, k, k.label());
                    }
                });
            ui.end_row();

            match draft.action_kind {
                ActionKind::Move | ActionKind::Archive => {
                    ui.label("Destination");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut draft.action_to);
                        if ui.button("Pick…").clicked() {
                            if let Some(f) = rfd::FileDialog::new().pick_folder() {
                                draft.action_to = f.display().to_string();
                            }
                        }
                    });
                    ui.end_row();
                    if draft.action_kind == ActionKind::Archive {
                        ui.label("Delete source");
                        ui.checkbox(&mut draft.action_delete_source, "");
                        ui.end_row();
                    }
                }
                ActionKind::Retier => {
                    ui.label("Target tier");
                    egui::ComboBox::from_id_salt("retier_target")
                        .selected_text(draft.action_tier.dir_name())
                        .show_ui(ui, |ui| {
                            for t in Tier::ALL {
                                ui.selectable_value(&mut draft.action_tier, t, t.dir_name());
                            }
                        });
                    ui.end_row();
                }
                ActionKind::DeleteAfter => {
                    ui.label("Delete after");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut draft.action_delete_after);
                        ui.label(egui::RichText::new("e.g. 180d").color(PALETTE.text_faint).size(11.0));
                    });
                    ui.end_row();
                }
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let save_btn = egui::Button::new(
                egui::RichText::new("Save rule").color(PALETTE.bg_window).strong(),
            )
            .fill(PALETTE.accent)
            .rounding(Rounding::same(6.0));
            if ui.add(save_btn).clicked() {
                match draft.to_rule() {
                    Ok(rule) => {
                        let mut cfg = engine.config.lock();
                        match draft.index {
                            Some(i) if i < cfg.rules.len() => cfg.rules[i] = rule,
                            _ => cfg.rules.push(rule),
                        }
                        if let Err(e) = cfg.save(&engine.paths.config_file) {
                            engine.set_error(format!("save config: {e}"));
                        } else {
                            engine.clear_error();
                            drop(cfg);
                            engine.reload_config();
                            state.editing = None;
                        }
                    }
                    Err(e) => engine.set_error(e),
                }
            }
            if ui.button("Cancel").clicked() {
                state.editing = None;
                engine.clear_error();
            }
        });
    });
    // Persist drafted edits so re-render keeps user input.
    state.editing = Some(draft);
}

fn draw_rule_card(ui: &mut egui::Ui, idx: usize, r: &Rule, editing: &mut Option<Draft>) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            if r.enabled.unwrap_or(true) {
                theme::pill(ui, "ON", PALETTE.success);
            } else {
                theme::pill(ui, "OFF", PALETTE.text_faint);
            }
            ui.add(egui::Label::new(
                egui::RichText::new(&r.name).color(PALETTE.text).strong(),
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Edit").clicked() {
                    *editing = Some(Draft::from_rule(idx, r));
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(
                egui::RichText::new("match:").color(PALETTE.text_faint).size(11.5),
            ));
            ui.add(egui::Label::new(
                egui::RichText::new(summarize_match(&r.r#match)).color(PALETTE.text_dim),
            ));
        });
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(
                egui::RichText::new("action:").color(PALETTE.text_faint).size(11.5),
            ));
            ui.add(egui::Label::new(
                egui::RichText::new(summarize_action(&r.action)).color(PALETTE.accent),
            ));
        });
    });
}

fn summarize_match(m: &Match) -> String {
    let mut parts = Vec::new();
    if !m.extension.is_empty() {
        parts.push(format!("ext={}", m.extension.join("|")));
    }
    if !m.name_glob.is_empty() {
        parts.push(format!("name={}", m.name_glob.join("|")));
    }
    if !m.path_glob.is_empty() {
        parts.push(format!("path={}", m.path_glob.join("|")));
    }
    if let Some(p) = &m.source_prefix {
        parts.push(format!("in={}", p.display()));
    }
    if let Some(d) = m.older_than {
        parts.push(format!("age≥{}", humantime::format_duration(d)));
    }
    if let Some(t) = m.in_tier {
        parts.push(format!("tier={}", t.dir_name()));
    }
    if let Some(s) = m.min_size {
        parts.push(format!("≥{}B", s));
    }
    if let Some(s) = m.max_size {
        parts.push(format!("≤{}B", s));
    }
    if parts.is_empty() {
        "match-all".into()
    } else {
        parts.join(" · ")
    }
}

fn summarize_action(a: &Action) -> String {
    match a {
        Action::Move { to } => format!("move → {}", to.display()),
        Action::Retier { to } => format!("retier → {}", to.dir_name()),
        Action::Archive { to, delete_source } => {
            format!("archive → {} (del_src={})", to.display(), delete_source)
        }
        Action::DeleteAfter { after } => format!("delete after {}", humantime::format_duration(*after)),
    }
}
