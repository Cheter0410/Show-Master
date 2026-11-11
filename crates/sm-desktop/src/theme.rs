//! Colours and visual style. Dark by default, optional high contrast.
//! The colour meaning is the same everywhere (and the same as the web UI):
//! next/playhead = green, running = blue, paused = amber, error = red,
//! loaded = cyan, disarmed = dimmed.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const NEXT: Color32 = Color32::from_rgb(34, 197, 94);
pub const NEXT_BG: Color32 = Color32::from_rgb(20, 60, 36);
pub const RUNNING: Color32 = Color32::from_rgb(59, 130, 246);
pub const RUNNING_BG: Color32 = Color32::from_rgb(23, 42, 82);
pub const PAUSED: Color32 = Color32::from_rgb(245, 158, 11);
pub const ERROR: Color32 = Color32::from_rgb(239, 68, 68);
pub const ERROR_BG: Color32 = Color32::from_rgb(80, 22, 22);
pub const LOADED: Color32 = Color32::from_rgb(34, 211, 238);
pub const WARN: Color32 = Color32::from_rgb(250, 204, 21);
pub const MUTED: Color32 = Color32::from_rgb(140, 146, 160);
pub const GO: Color32 = Color32::from_rgb(22, 163, 74);
pub const PANIC: Color32 = Color32::from_rgb(185, 28, 28);
pub const LIVE: Color32 = Color32::from_rgb(220, 38, 38);
pub const EDIT: Color32 = Color32::from_rgb(37, 99, 235);

/// Cue colour names used in the show file -> swatch.
pub fn cue_color(name: &str) -> Option<Color32> {
    Some(match name {
        "red" => Color32::from_rgb(239, 68, 68),
        "orange" => Color32::from_rgb(249, 115, 22),
        "yellow" => Color32::from_rgb(234, 179, 8),
        "green" => Color32::from_rgb(34, 197, 94),
        "blue" => Color32::from_rgb(59, 130, 246),
        "purple" => Color32::from_rgb(168, 85, 247),
        _ => return None,
    })
}

pub const CUE_COLORS: [&str; 7] = ["", "red", "orange", "yellow", "green", "blue", "purple"];

pub fn apply(ctx: &egui::Context, high_contrast: bool) {
    let mut style = (*ctx.global_style()).clone();
    let mut v = Visuals::dark();
    if high_contrast {
        v.panel_fill = Color32::BLACK;
        v.window_fill = Color32::BLACK;
        v.extreme_bg_color = Color32::BLACK;
        v.faint_bg_color = Color32::from_gray(20);
        v.override_text_color = Some(Color32::WHITE);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.5, Color32::from_gray(200));
        v.widgets.inactive.bg_stroke = Stroke::new(1.5, Color32::from_gray(220));
        v.widgets.inactive.weak_bg_fill = Color32::from_gray(30);
    } else {
        v.panel_fill = Color32::from_rgb(17, 19, 24);
        v.window_fill = Color32::from_rgb(24, 27, 34);
        v.extreme_bg_color = Color32::from_rgb(10, 11, 14);
        v.faint_bg_color = Color32::from_rgb(28, 31, 39);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(38, 42, 52);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 57, 70);
    }
    v.window_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = Color32::from_rgb(37, 72, 140);
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(20.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.5, FontFamily::Monospace),
    );
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Large tabular digits for clocks.
pub fn clock_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
