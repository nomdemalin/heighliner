use eframe::egui::{self, style, Color32, FontId, Margin, Rounding, Shadow, Stroke, TextStyle};

use crate::engine::config::Tier;

pub struct Palette {
    pub bg_window: Color32,
    pub bg_sidebar: Color32,
    pub bg_panel: Color32,
    pub bg_card: Color32,
    pub bg_elevated: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub tier_hot: Color32,
    pub tier_warm: Color32,
    pub tier_cold: Color32,
    pub tier_frozen: Color32,
    #[allow(dead_code)]
    pub categorical: [Color32; 8],
}

pub const PALETTE: Palette = Palette {
    bg_window: Color32::from_rgb(0x0B, 0x10, 0x1A),
    bg_sidebar: Color32::from_rgb(0x0F, 0x16, 0x22),
    bg_panel: Color32::from_rgb(0x13, 0x1B, 0x2A),
    bg_card: Color32::from_rgb(0x18, 0x22, 0x33),
    bg_elevated: Color32::from_rgb(0x1F, 0x2A, 0x3F),
    border: Color32::from_rgb(0x24, 0x30, 0x45),
    border_strong: Color32::from_rgb(0x33, 0x44, 0x5F),
    text: Color32::from_rgb(0xE8, 0xEE, 0xF7),
    text_dim: Color32::from_rgb(0xA6, 0xB2, 0xC6),
    text_faint: Color32::from_rgb(0x6C, 0x7A, 0x92),
    accent: Color32::from_rgb(0x5E, 0xEA, 0xD4),
    accent_dim: Color32::from_rgb(0x2C, 0x8A, 0x7C),
    danger: Color32::from_rgb(0xFF, 0x6B, 0x6B),
    success: Color32::from_rgb(0x7E, 0xE0, 0x9D),
    warning: Color32::from_rgb(0xFF, 0xC1, 0x6A),
    tier_hot: Color32::from_rgb(0xFF, 0x7A, 0x59),
    tier_warm: Color32::from_rgb(0xFF, 0xB2, 0x4D),
    tier_cold: Color32::from_rgb(0x6B, 0xB7, 0xFF),
    tier_frozen: Color32::from_rgb(0xA7, 0xC0, 0xDC),
    categorical: [
        Color32::from_rgb(0x5E, 0xEA, 0xD4),
        Color32::from_rgb(0x6B, 0xB7, 0xFF),
        Color32::from_rgb(0xFF, 0xB2, 0x4D),
        Color32::from_rgb(0xFF, 0x7A, 0x59),
        Color32::from_rgb(0xB4, 0xA7, 0xFF),
        Color32::from_rgb(0x7E, 0xE0, 0x9D),
        Color32::from_rgb(0xFF, 0x8E, 0xC7),
        Color32::from_rgb(0xA7, 0xC0, 0xDC),
    ],
};

pub fn tier_color(t: Tier) -> Color32 {
    match t {
        Tier::Hot => PALETTE.tier_hot,
        Tier::Warm => PALETTE.tier_warm,
        Tier::Cold => PALETTE.tier_cold,
        Tier::Frozen => PALETTE.tier_frozen,
    }
}

pub fn tier_glyph(t: Tier) -> &'static str {
    match t {
        Tier::Hot => "●",
        Tier::Warm => "◉",
        Tier::Cold => "◌",
        Tier::Frozen => "❄",
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0));
    style.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
    style.text_styles.insert(TextStyle::Button, FontId::proportional(13.5));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
    style.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5));

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.window_margin = Margin::same(0.0);
    style.spacing.menu_margin = Margin::same(6.0);
    style.spacing.icon_spacing = 6.0;
    style.spacing.scroll.bar_width = 10.0;

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(PALETTE.text);
    visuals.panel_fill = PALETTE.bg_panel;
    visuals.window_fill = PALETTE.bg_card;
    visuals.extreme_bg_color = PALETTE.bg_window;
    visuals.faint_bg_color = PALETTE.bg_sidebar;
    visuals.code_bg_color = PALETTE.bg_sidebar;
    visuals.hyperlink_color = PALETTE.accent;
    visuals.selection.bg_fill = PALETTE.accent_dim.linear_multiply(0.6);
    visuals.selection.stroke = Stroke::new(1.0_f32, PALETTE.accent);
    visuals.window_rounding = Rounding::same(8.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.window_shadow = Shadow {
        offset: egui::vec2(0.0, 2.0),
        blur: 12.0,
        spread: 0.0,
        color: Color32::from_black_alpha(140),
    };
    visuals.popup_shadow = Shadow {
        offset: egui::vec2(0.0, 2.0),
        blur: 10.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    };
    visuals.widgets.noninteractive = widget_style(PALETTE.bg_card, PALETTE.text, PALETTE.border, 0.0);
    visuals.widgets.inactive = widget_style(PALETTE.bg_card, PALETTE.text, PALETTE.border, 0.0);
    visuals.widgets.hovered = widget_style(PALETTE.bg_elevated, PALETTE.text, PALETTE.border_strong, 1.0);
    visuals.widgets.active = widget_style(PALETTE.accent_dim, PALETTE.bg_window, PALETTE.accent, 2.0);
    visuals.widgets.open = widget_style(PALETTE.bg_elevated, PALETTE.text, PALETTE.border_strong, 1.0);

    style.visuals = visuals;
    ctx.set_style(style);
}

fn widget_style(bg: Color32, fg: Color32, border: Color32, expansion: f32) -> style::WidgetVisuals {
    style::WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0_f32, border),
        fg_stroke: Stroke::new(1.0_f32, fg),
        rounding: Rounding::same(6.0),
        expansion,
    }
}

/// Card container: rounded panel with a subtle border, inner padding.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(PALETTE.bg_card)
        .stroke(Stroke::new(1.0_f32, PALETTE.border))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, add)
        .inner
}

pub fn section_header(ui: &mut egui::Ui, text: &str, hint: &str) {
    ui.vertical(|ui| {
        ui.add(egui::Label::new(
            egui::RichText::new(text).size(22.0).color(PALETTE.text).strong(),
        ));
        if !hint.is_empty() {
            ui.add(egui::Label::new(
                egui::RichText::new(hint).color(PALETTE.text_dim).size(13.0),
            ));
        }
    });
    ui.add_space(10.0);
}

pub fn pill(ui: &mut egui::Ui, label: &str, color: Color32) {
    let text = egui::RichText::new(label).color(PALETTE.bg_window).strong().size(11.0);
    egui::Frame::none()
        .fill(color)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(8.0, 2.0))
        .show(ui, |ui| {
            ui.add(egui::Label::new(text));
        });
}

/// Heighliner mark: an upward wedge of 4 tier-colored slats. The hot band is
/// the small apex (bright, recent), the frozen band is the wide base (muted,
/// cold storage). Reads as a stylized cargo hull / lifted ziggurat.
pub fn paint_logo(painter: &egui::Painter, center: egui::Pos2, size: f32) {
    let w = size * 0.92;
    let h = size * 0.92;
    let gap = (size * 0.03).max(0.75);
    // Keep the apex band from degenerating to a line at small sizes.
    let apex_min_width_frac = 0.10_f32;

    let bands = [
        (0.00_f32, 0.22, PALETTE.tier_hot),
        (0.22_f32, 0.48, PALETTE.tier_warm),
        (0.48_f32, 0.74, PALETTE.tier_cold),
        (0.74_f32, 1.00, PALETTE.tier_frozen),
    ];

    let top_y = center.y - h / 2.0;
    for (t_top, t_bot, color) in bands {
        let wt = (w * t_top).max(w * apex_min_width_frac);
        let wb = w * t_bot;
        let y_top = top_y + h * t_top + gap;
        let y_bot = top_y + h * t_bot - gap;
        if y_bot <= y_top {
            continue;
        }
        let pts = vec![
            egui::pos2(center.x - wt / 2.0, y_top),
            egui::pos2(center.x + wt / 2.0, y_top),
            egui::pos2(center.x + wb / 2.0, y_bot),
            egui::pos2(center.x - wb / 2.0, y_bot),
        ];
        painter.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    }
}

