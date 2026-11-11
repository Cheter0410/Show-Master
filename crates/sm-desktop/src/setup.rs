//! Setup window. (Placeholder, see sm-desktop plan.)

use crate::app::App;

#[derive(Default)]
pub struct SetupState {
    pub open: bool,
}

impl SetupState {
    /// True while the hotkey editor waits for a key press (global hotkeys pause).
    pub fn capturing_hotkey(&self) -> bool {
        false
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut open = app.setup.open;
    egui::Window::new("Setup").open(&mut open).show(ctx, |ui| {
        ui.label("Setup");
    });
    app.setup.open = open;
}
