//! Edit mode: cue table and inspector. (Placeholder, see sm-desktop plan.)

use crate::app::App;

#[derive(Default)]
pub struct EditState {}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let _ = app;
    ui.heading("Edit mode");
}
