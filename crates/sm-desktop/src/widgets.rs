//! Widgets used by more than one view: transport buttons, clocks, meters.

use crate::app::App;
use crate::theme;
use egui::{Color32, RichText, Sense, Vec2};

/// The big green GO button. Fires on press (not release) for minimum latency.
pub fn go_button(app: &mut App, ui: &mut egui::Ui, size: Vec2) {
    let locked = app.go_locked();
    let text = RichText::new("GO")
        .strong()
        .size((size.y * 0.42).clamp(14.0, 64.0))
        .color(Color32::WHITE);
    let fill = if locked {
        Color32::from_rgb(20, 83, 45)
    } else {
        theme::GO
    };
    let resp = ui.add_sized(
        size,
        egui::Button::new(text).fill(fill).sense(Sense::click()),
    );
    if resp.is_pointer_button_down_on() && !locked && ui.input(|i| i.pointer.any_pressed()) {
        app.go();
    }
    resp.on_hover_text("GO: fire the next cue (Space)");
}

/// PANIC: stops everything (double tap when that safety is on).
pub fn panic_button(app: &mut App, ui: &mut egui::Ui, size: Vec2) {
    let armed = app.panic_armed();
    let label = if armed { "PANIC: press again" } else { "PANIC" };
    let text = RichText::new(label)
        .strong()
        .size((size.y * 0.32).clamp(13.0, 30.0))
        .color(Color32::WHITE);
    let fill = if armed { theme::ERROR } else { theme::PANIC };
    if ui
        .add_sized(size, egui::Button::new(text).fill(fill))
        .on_hover_text("Stop everything immediately (Esc)")
        .clicked()
    {
        app.panic();
    }
}

/// A normal transport button.
pub fn transport_button(ui: &mut egui::Ui, size: Vec2, label: &str, hint: &str) -> bool {
    ui.add_sized(
        size,
        egui::Button::new(
            RichText::new(label)
                .strong()
                .size((size.y * 0.3).clamp(12.0, 22.0)),
        ),
    )
    .on_hover_text(hint)
    .clicked()
}
