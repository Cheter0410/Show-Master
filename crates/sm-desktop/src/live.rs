//! Live mode dashboard. (Placeholder, see sm-desktop plan.)

use crate::app::App;

#[derive(Default)]
pub struct LiveState {}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let _ = app;
    ui.heading("Live mode");
}
