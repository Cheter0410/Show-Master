//! Setup window: audio device, output patch, MIDI, timecode, remote access,
//! hotkeys, general safety settings and the about/update page.
//!
//! Every change is sent as a command (`update_show`, `apply_layout`, ...).
//! Text fields keep a local draft while focused and commit on Enter or when
//! they lose focus; nothing is sent per frame.

#[path = "setup_audio.rs"]
mod audio;
#[path = "setup_general.rs"]
mod general;
#[path = "setup_hotkeys.rs"]
mod hotkeys;
#[path = "setup_midi.rs"]
mod midi;
#[path = "setup_remote.rs"]
mod remote;

use crate::app::App;
use crate::model::UiState;
use crate::theme;
use egui::{Color32, Id, Key, RichText};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Audio,
    Patch,
    Midi,
    Timecode,
    Remote,
    Hotkeys,
    General,
    About,
}

const TABS: [(Tab, &str); 8] = [
    (Tab::Audio, "Audio"),
    (Tab::Patch, "Outputs / Patch"),
    (Tab::Midi, "MIDI"),
    (Tab::Timecode, "Timecode"),
    (Tab::Remote, "Remote"),
    (Tab::Hotkeys, "Hotkeys"),
    (Tab::General, "General"),
    (Tab::About, "About / Updates"),
];

/// Something the window wants the app to do after drawing.
pub(super) enum Out {
    /// Send a command (errors become a toast).
    Cmd(Value),
    /// Send a command and show `ok` as a toast when it succeeds.
    CmdToast(Value, String),
    Error(String),
    /// Save the diagnostics bundle (system dialog).
    ExportDiagnostics,
    /// Show the log folder in the file manager.
    OpenLogFolder,
    /// Per-machine touch-friendly controls setting (not in the show).
    SetTouch(crate::prefs::TouchPref),
}

#[derive(Default)]
pub struct SetupState {
    pub open: bool,
    tab: Tab,
    /// Text of focused fields, by field key.
    drafts: HashMap<String, String>,
    outbox: Vec<Out>,
    /// Pending (not yet applied) audio device choice.
    audio: Option<audio::Draft>,
    /// Layout template waiting for confirmation.
    confirm_layout: Option<String>,
    /// New RTP-MIDI peer address being typed.
    peer_addr: String,
    /// Hotkey action waiting for a key press.
    capture: Option<String>,
    /// Last hotkey assignment problem: (action, text).
    hotkey_warning: Option<(String, String)>,
    /// QR code textures per URL (`None` = cannot be encoded).
    qr: HashMap<String, Option<egui::TextureHandle>>,
    /// Touch-friendly controls setting of this machine (mirrors `App::prefs`).
    touch_pref: crate::prefs::TouchPref,
    /// Touch mode is on right now (for the hint next to the setting).
    touch_active: bool,
}

impl SetupState {
    pub(super) fn outbox_push(&mut self, out: Out) {
        self.outbox.push(out);
    }

    /// True while the hotkey editor waits for a key press (global hotkeys pause).
    pub fn capturing_hotkey(&self) -> bool {
        self.open && self.capture.is_some()
    }

    /// True when a QR code texture for `url` has been made (Remote tab).
    pub fn has_qr_code(&self, url: &str) -> bool {
        self.qr.get(url).is_some_and(Option::is_some)
    }

    fn send(&mut self, cmd: Value) {
        self.outbox.push(Out::Cmd(cmd));
    }

    fn send_toast(&mut self, cmd: Value, ok: impl Into<String>) {
        self.outbox.push(Out::CmdToast(cmd, ok.into()));
    }

    /// RFC 7396 merge patch on the show document.
    fn update_show(&mut self, patch: Value) {
        self.send(json!({ "cmd": "update_show", "patch": patch }));
    }

    fn update_settings(&mut self, patch: Value) {
        self.update_show(json!({ "settings": patch }));
    }

    fn error(&mut self, text: impl Into<String>) {
        self.outbox.push(Out::Error(text.into()));
    }
}

/// Take a key press for the hotkey editor before any view sees the input.
/// Called from `App::logic` ahead of the global hotkeys.
pub fn capture_keys(app: &mut App, ctx: &egui::Context) {
    if app.setup.open && !app.is_live() {
        hotkeys::capture_keys(app, ctx);
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.setup.open {
        app.setup.capture = None;
        return;
    }
    let live = app.is_live();
    if live {
        // Settings are locked in live mode; never keep the keyboard captured.
        app.setup.capture = None;
    }

    let mut open = true;
    {
        app.setup.touch_pref = app.prefs.touch;
        app.setup.touch_active = app.touch_mode();
        let st = &mut app.setup;
        let s = &app.client.state;
        // Never larger than the main window: on small screens the window
        // fills the content area (minus a margin) and its body scrolls.
        let area = ctx.content_rect().shrink(8.0);
        let size = egui::vec2(860.0_f32.min(area.width()), 600.0_f32.min(area.height()));
        let min = egui::vec2(640.0_f32.min(area.width()), 420.0_f32.min(area.height()));
        egui::Window::new("Setup")
            .id(Id::unique("setup_window"))
            .open(&mut open)
            .default_size(size)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .min_size(min)
            .max_size(area.size())
            .constrain_to(area)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                // Narrow window: the tab column becomes a row of tabs on top.
                let narrow = ui.available_width() < 800.0;
                let content = |ui: &mut egui::Ui, st: &mut SetupState| {
                    if live {
                        banner(
                            ui,
                            theme::WARN,
                            "Live mode: settings are locked. Switch to Edit to change them.",
                        );
                    }
                    egui::ScrollArea::both()
                        .id_salt("setup_scroll")
                        .auto_shrink(false)
                        .scroll_source(crate::responsive::scroll_source(ui.ctx()))
                        .show(ui, |ui| {
                            let enabled = !live || st.tab == Tab::About;
                            ui.add_enabled_ui(enabled, |ui| body(ui, st, s));
                        });
                };
                if narrow {
                    tab_row(ui, st);
                    ui.separator();
                    content(ui, st);
                } else {
                    ui.horizontal_top(|ui| {
                        tab_column(ui, st);
                        ui.separator();
                        ui.vertical(|ui| content(ui, st));
                    });
                }
            });
    }
    if !open {
        app.setup.open = false;
        app.setup.capture = None;
        app.setup.confirm_layout = None;
    }
    flush(app);
}

fn tab_column(ui: &mut egui::Ui, st: &mut SetupState) {
    let w = if crate::responsive::get(ui.ctx()).touch {
        170.0
    } else {
        140.0
    };
    ui.vertical(|ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 4.0;
        for (tab, label) in TABS {
            let b = egui::Button::selectable(st.tab == tab, label).min_size(egui::vec2(w, 30.0));
            if ui.add(b).clicked() && st.tab != tab {
                st.tab = tab;
                st.capture = None;
                st.confirm_layout = None;
            }
        }
    });
}

/// Tabs as a wrapping row (narrow setup window).
fn tab_row(ui: &mut egui::Ui, st: &mut SetupState) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for (tab, label) in TABS {
            if ui
                .add(egui::Button::selectable(st.tab == tab, label))
                .clicked()
                && st.tab != tab
            {
                st.tab = tab;
                st.capture = None;
                st.confirm_layout = None;
            }
        }
    });
}

fn body(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    ui.add_space(2.0);
    match st.tab {
        Tab::Audio => audio::audio_tab(ui, st, s),
        Tab::Patch => audio::patch_tab(ui, st, s),
        Tab::Midi => midi::midi_tab(ui, st, s),
        Tab::Timecode => midi::timecode_tab(ui, st, s),
        Tab::Remote => remote::remote_tab(ui, st, s),
        Tab::Hotkeys => hotkeys::hotkeys_tab(ui, st, s),
        Tab::General => general::general_tab(ui, st, s),
        Tab::About => general::about_tab(ui, s, st),
    }
}

/// Execute what the window queued while drawing.
fn flush(app: &mut App) {
    for out in std::mem::take(&mut app.setup.outbox) {
        match out {
            Out::Cmd(cmd) => app.cmd(cmd),
            Out::CmdToast(cmd, ok) => app.cmd_then(cmd, move |app, r| match r {
                Ok(_) => app.toast(ok),
                Err(e) => app.toast_error(App::error_text(&e)),
            }),
            Out::Error(t) => app.toast_error(t),
            Out::ExportDiagnostics => app.export_diagnostics(),
            Out::OpenLogFolder => app.open_log_folder(),
            Out::SetTouch(t) => app.prefs.set_touch(t),
        }
    }
}

// ----------------------------------------------------------------------
// Shared building blocks
// ----------------------------------------------------------------------

fn field_id(key: &str) -> Id {
    Id::unique(("setup_field", key))
}

/// A section heading.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong().size(16.0));
    ui.add_space(2.0);
}

/// Small explanatory text.
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::MUTED));
}

/// A coloured notice box.
fn banner(ui: &mut egui::Ui, color: Color32, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(color));
        })
        .response
}

/// Two-column form: label | widgets. Unnamed inputs on the row take the
/// label as their accessible name.
fn form(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 8.0])
        .min_col_width(150.0)
        .show(ui, add);
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let l = ui.label(label);
    ui.label_inputs_by(l.id, |ui| {
        ui.horizontal(add);
    });
    ui.end_row();
}

/// Single-line text field that returns the new text on Enter / lost focus
/// (only when it differs). Escape reverts.
fn text_field(
    ui: &mut egui::Ui,
    drafts: &mut HashMap<String, String>,
    key: &str,
    current: &str,
    width: f32,
) -> Option<String> {
    let id = field_id(key);
    let mut text = drafts
        .get(key)
        .cloned()
        .unwrap_or_else(|| current.to_string());
    let resp = ui.add(
        egui::TextEdit::singleline(&mut text)
            .id(id)
            .desired_width(width)
            .min_size(egui::vec2(width, 0.0)),
    );
    if resp.has_focus() {
        drafts.insert(key.to_string(), text);
        return None;
    }
    drafts.remove(key);
    if resp.lost_focus() && !ui.input(|i| i.key_pressed(Key::Escape)) && text != current {
        return Some(text);
    }
    None
}

/// Numeric text field committing a value clamped to `min..=max`.
/// Unparsable input reverts.
fn num_field<T>(
    ui: &mut egui::Ui,
    drafts: &mut HashMap<String, String>,
    key: &str,
    current: T,
    min: T,
    max: T,
    width: f32,
) -> Option<T>
where
    T: FromStr + Display + PartialOrd + Copy,
{
    let text = text_field(ui, drafts, key, &current.to_string(), width)?;
    let v: T = text.trim().replace(',', ".").parse().ok()?;
    let v = if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    };
    (v != current).then_some(v)
}

/// A labelled unit after a field.
fn unit(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED));
}

/// Small "Remove" style button.
fn small_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).small())
}
