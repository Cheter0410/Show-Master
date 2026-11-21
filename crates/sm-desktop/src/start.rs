//! The start screen (new show, open, show package, recent shows) over a
//! banner, and the setup wizard a new show runs through – audio, outputs,
//! timecode, remote – before the main window opens. Same layout as Lantern's
//! start screen, in Show Master's green.

use super::{App, Modal, Pending, View};
use crate::setup::Page as SetupPage;
use egui::{
    pos2, Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke,
    TextureHandle, Vec2,
};
use serde_json::json;
use std::path::{Path, PathBuf};

/// What the window shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Page {
    Start,
    Wizard(Step),
    #[default]
    Main,
}

/// The steps of the setup wizard, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Audio,
    Outputs,
    Timecode,
    Remote,
    Ready,
}

const STEPS: [(Step, &str); 5] = [
    (Step::Audio, "Audio"),
    (Step::Outputs, "Outputs"),
    (Step::Timecode, "Timecode"),
    (Step::Remote, "Remote"),
    (Step::Ready, "Ready"),
];

impl Step {
    fn index(self) -> usize {
        STEPS.iter().position(|(s, _)| *s == self).unwrap_or(0)
    }
    fn next(self) -> Option<Step> {
        STEPS.get(self.index() + 1).map(|(s, _)| *s)
    }
    fn back(self) -> Option<Step> {
        self.index().checked_sub(1).map(|i| STEPS[i].0)
    }
}

/// The new-show form of the start screen.
#[derive(Default)]
pub struct StartState {
    /// The "New show" form is open.
    pub form: bool,
    pub name: String,
    pub location: PathBuf,
    pub error: Option<String>,
    /// A new show is being made (waits for the runtime, then the wizard).
    pub(super) creating: Option<PathBuf>,
    banner: Option<TextureHandle>,
}

// Colours of the start screen (the logo's green on a dark stage).
const GREEN: Color32 = Color32::from_rgb(34, 197, 94);
const BG: Color32 = Color32::from_rgb(12, 14, 17);
const LINE: Color32 = Color32::from_rgb(44, 49, 58);
const TEXT: Color32 = Color32::from_rgb(232, 235, 239);
const MUTED: Color32 = Color32::from_rgb(140, 146, 160);
const DANGER: Color32 = Color32::from_rgb(239, 68, 68);
/// A card on the start screen: dark glass, a stroke that lights up on hover.
const GLASS: Color32 = Color32::from_rgba_premultiplied(16, 18, 22, 228);
/// Ink on the green buttons.
const ON_GREEN: Color32 = Color32::from_rgb(6, 24, 12);

/// Most shows shown under "Recent".
const RECENT: usize = 8;

/// Where new shows go unless another place is chosen: Documents/Show Master.
pub fn default_location() -> PathBuf {
    let home = if cfg!(target_os = "windows") {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    };
    home.map(|h| PathBuf::from(h).join("Documents").join("Show Master"))
        .unwrap_or_default()
}

/// A show name usable as a folder and file name.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .trim()
        .to_string()
}

impl StartState {
    /// The show file a new show gets: `<location>/<name>/<name>.smshow`.
    pub fn show_file(&self) -> PathBuf {
        let n = clean_name(&self.name);
        self.location
            .join(&n)
            .join(format!("{n}.{}", super::SHOW_EXTENSION))
    }

    /// Why the form cannot be sent yet.
    pub fn problem(&self) -> Option<String> {
        if clean_name(&self.name).is_empty() {
            return Some("Give the show a name.".into());
        }
        if self.location.as_os_str().is_empty() {
            return Some("Choose where the show folder goes.".into());
        }
        let dir = self.show_file().parent().map(Path::to_path_buf)?;
        if dir.exists() {
            return Some(format!("{} exists already.", dir.display()));
        }
        None
    }
}

impl App {
    /// The start screen again (File menu).
    pub fn back_to_start(&mut self) {
        if self.kiosk_locked() {
            return;
        }
        if self.is_live() {
            self.toast_error("Switch to Edit first.");
            return;
        }
        self.start.form = false;
        self.start.error = None;
        self.page = Page::Start;
    }

    /// Make the show of the form: a fresh show, saved into its own folder,
    /// then the wizard. Asks first when the open show has unsaved changes.
    pub fn create_show(&mut self) {
        if let Some(p) = self.start.problem() {
            self.start.error = Some(p);
            return;
        }
        self.start.error = None;
        self.start.creating = Some(self.start.show_file());
        self.cmd_then(
            json!({ "cmd": "new_show", "force": false }),
            |app, r| match r {
                Ok(_) => app.save_new_show(),
                Err(e) if e == "unsaved_changes" => app.modal = Some(Modal::Unsaved(Pending::New)),
                Err(e) => {
                    app.start.creating = None;
                    app.start.error = Some(App::error_text(&e));
                }
            },
        );
    }

    /// Second half of [`App::create_show`], also after the unsaved-changes
    /// question was answered.
    pub(super) fn save_new_show(&mut self) {
        let Some(path) = self.start.creating.clone() else {
            return;
        };
        self.view = View::default();
        self.cmd_then(
            json!({ "cmd": "save_show", "path": path, "folder": true }),
            |app, r| {
                app.start.creating = None;
                match r {
                    Ok(_) => {
                        app.start.form = false;
                        app.page = Page::Wizard(Step::Audio);
                    }
                    Err(e) => {
                        app.start.error =
                            Some(format!("Could not save the show: {}", App::error_text(&e)))
                    }
                }
            },
        );
    }

    /// True while a new show is being made (the reply to `new_show` goes on
    /// with saving it).
    pub(super) fn creating_show(&self) -> bool {
        self.start.creating.is_some()
    }

    pub fn finish_wizard(&mut self) {
        self.page = Page::Main;
        if self.client.state.show_path.is_some() {
            self.save(None);
        }
    }

    // ------------------------------------------------------------ background

    fn backdrop(&mut self, ui: &mut egui::Ui, dim: f32) -> Rect {
        let rect = ui.max_rect();
        let tex = self.start.banner.get_or_insert_with(|| {
            ui.ctx().load_texture(
                "sm_banner",
                banner_image(1280, 720),
                egui::TextureOptions::LINEAR,
            )
        });
        // Cover: fill the window, crop what is too much; keep the right side.
        let [tw, th] = tex.size().map(|v| v as f32);
        let scale = (rect.width() / tw).max(rect.height() / th);
        let size = Vec2::new(tw * scale, th * scale);
        let min = pos2(rect.right() - size.x, rect.center().y - size.y / 2.0);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let p = ui.painter();
        p.rect_filled(rect, 0.0, BG);
        p.image(tex.id(), Rect::from_min_size(min, size), uv, Color32::WHITE);
        if dim > 0.0 {
            p.rect_filled(rect, 0.0, Color32::from_black_alpha((dim * 255.0) as u8));
        }
        rect
    }

    // ------------------------------------------------------------ start screen

    pub fn start_screen(&mut self, ui: &mut egui::Ui) {
        let rect = self.backdrop(ui, 0.0);
        let pad = (rect.width() * 0.06).clamp(28.0, 96.0);
        let inner = rect.shrink2(Vec2::new(pad, pad * 0.8));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        let ui = &mut child;

        ui.add_space((inner.height() * 0.05).min(36.0));
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::hover());
            show_master_mark(ui.painter(), r);
            ui.add_space(8.0);
            ui.label(RichText::new("Show Master").size(46.0).strong().color(TEXT));
        });
        ui.add_space(2.0);
        ui.label(
            RichText::new("Cue-based playback and show control")
                .size(18.0)
                .color(Color32::from_rgb(196, 200, 210)),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Version {}  ·  Audio, MIDI, OSC, Timecode  ·  Tablet remote",
                env!("CARGO_PKG_VERSION")
            ))
            .size(12.5)
            .color(MUTED),
        );
        ui.add_space((inner.height() * 0.06).clamp(16.0, 48.0));

        if !self.start.form {
            if let Some(e) = self.start.error.clone() {
                notice(ui, DANGER, &e);
                ui.add_space(10.0);
            }
            self.start_home(ui);
        } else {
            self.new_form(ui);
        }

        let foot = pos2(inner.left(), rect.bottom() - pad * 0.45);
        ui.painter().text(
            foot,
            egui::Align2::LEFT_CENTER,
            "Drop a .smshow file anywhere to open it",
            egui::FontId::proportional(12.5),
            MUTED,
        );
    }

    fn start_home(&mut self, ui: &mut egui::Ui) {
        let gap = 16.0;
        let w = ((ui.available_width() - 2.0 * gap) / 3.0).clamp(190.0, 300.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if action_card(
                ui,
                w,
                Icon::New,
                "New show",
                "A show folder for cues and audio, then set up the interface, outputs, timecode and remote.",
                true,
            )
            .clicked()
            {
                self.start.form = true;
                self.start.error = None;
                self.start.name.clear();
                if self.start.location.as_os_str().is_empty() {
                    self.start.location = default_location();
                }
            }
            if action_card(
                ui,
                w,
                Icon::Open,
                "Open show",
                "A .smshow file from this computer, a USB stick or a shared folder.",
                false,
            )
            .clicked()
            {
                self.open_show_dialog();
            }
            if action_card(
                ui,
                w,
                Icon::Package,
                "Open show package",
                "A .smpack from another computer: the show and all its audio in one file.",
                false,
            )
            .clicked()
            {
                self.import_package(None);
            }
        });

        let recent: Vec<PathBuf> = self.recent.paths.iter().take(RECENT).cloned().collect();
        if recent.is_empty() {
            return;
        }
        ui.add_space(28.0);
        let width = 3.0 * w + 2.0 * gap;
        ui.label(
            RichText::new("RECENT SHOWS")
                .size(11.5)
                .strong()
                .color(MUTED),
        );
        ui.add_space(6.0);
        let mut open = None;
        Frame::new()
            .fill(GLASS)
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(width - 12.0);
                let rows = ((ui.available_height() - 30.0) / 44.0).floor().max(1.0) as usize;
                for p in recent.iter().take(rows) {
                    if recent_row(ui, p).clicked() {
                        open = Some(p.clone());
                    }
                }
            });
        if let Some(p) = open {
            if p.is_file() {
                self.start.error = None;
                self.open_show(p);
            } else {
                self.start.error = Some(format!("Not there any more: {}", p.display()));
            }
        }
    }

    fn new_form(&mut self, ui: &mut egui::Ui) {
        form_card(ui, |ui| {
            ui.label(RichText::new("New show").size(24.0).strong().color(TEXT));
            ui.label(
                RichText::new(
                    "Show Master makes a folder for the show. Audio added later can be copied into \
                     it (File › Collect media), so the folder can be backed up or taken to another \
                     computer as one piece.",
                )
                .color(MUTED),
            );
            ui.add_space(16.0);
            let st = &mut self.start;
            egui::Grid::new("sm_new_show")
                .num_columns(2)
                .spacing([16.0, 12.0])
                .min_col_width(110.0)
                .show(ui, |ui| {
                    let l = ui.label(RichText::new("Show name").color(MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut st.name)
                            .hint_text("e.g. Spring Concert 2026")
                            .desired_width(360.0),
                    )
                    .labelled_by(l.id);
                    ui.end_row();
                    ui.label(RichText::new("Location").color(MUTED));
                    ui.horizontal(|ui| {
                        path_field(ui, &st.location, "No place chosen");
                        if ui.button("Choose…").clicked() {
                            let mut d = rfd::FileDialog::new();
                            if st.location.is_dir() {
                                d = d.set_directory(&st.location);
                            }
                            if let Some(p) = d.pick_folder() {
                                st.location = p;
                            }
                        }
                    });
                    ui.end_row();
                });
            ui.add_space(10.0);
            if !clean_name(&st.name).is_empty() && !st.location.as_os_str().is_empty() {
                ui.label(
                    RichText::new(format!("Creates  {}", st.show_file().display()))
                        .size(12.5)
                        .color(MUTED),
                );
            }
            let problem = st.problem();
            ui.add_space(14.0);
            if let Some(e) = &st.error {
                ui.label(RichText::new(e).color(DANGER));
                ui.add_space(6.0);
            }
            let mut go = false;
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Cancel").size(15.0))
                            .corner_radius(8)
                            .min_size(Vec2::new(110.0, 38.0)),
                    )
                    .clicked()
                {
                    st.form = false;
                    st.error = None;
                }
                ui.add_space(6.0);
                let busy = st.creating.is_some();
                go = ui
                    .add_enabled(
                        problem.is_none() && !busy,
                        egui::Button::new(
                            RichText::new("Create and set up")
                                .size(15.0)
                                .strong()
                                .color(ON_GREEN),
                        )
                        .fill(GREEN)
                        .corner_radius(8)
                        .min_size(Vec2::new(200.0, 38.0)),
                    )
                    .clicked();
                if busy {
                    ui.spinner();
                } else if let Some(p) = &problem {
                    ui.add_space(8.0);
                    ui.label(RichText::new(p).color(MUTED));
                }
            });
            if go || (problem.is_none() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                self.create_show();
            }
        });
    }

    // ------------------------------------------------------------ wizard

    pub fn wizard(&mut self, ui: &mut egui::Ui, step: Step) {
        let rect = self.backdrop(ui, 0.55);
        let size = Vec2::new(
            (rect.width() - 60.0)
                .clamp(560.0, 900.0)
                .min(rect.width() - 16.0),
            (rect.height() - 60.0)
                .clamp(440.0, 700.0)
                .min(rect.height() - 16.0),
        );
        let card = Rect::from_center_size(rect.center(), size);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(card)
                .layout(Layout::top_down(Align::Min)),
        );
        let ui = &mut child;
        Frame::new()
            .fill(Color32::from_rgba_premultiplied(18, 20, 24, 246))
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::symmetric(30, 24))
            .show(ui, |ui| {
                ui.set_min_size(card.size() - Vec2::new(60.0, 48.0));
                let name = self.client.state.show.name.clone();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("SETUP").size(11.5).strong().color(GREEN));
                    ui.label(RichText::new(&name).size(11.5).strong().color(MUTED));
                });
                ui.add_space(10.0);
                steps_bar(ui, step);
                ui.add_space(16.0);
                let (title, about) = match step {
                    Step::Audio => (
                        "Which interface plays the show?",
                        "Choose the audio interface, its sample rate and buffer. A larger buffer is \
                         safer, a smaller one reacts faster. The meters at the bottom of the main \
                         window show that it runs.",
                    ),
                    Step::Outputs => (
                        "How are the outputs patched?",
                        "Name the outputs of the interface – PA left and right, monitors, sub – or \
                         start from a layout. Cues play to these names, so the show moves to \
                         another interface by changing the patch only.",
                    ),
                    Step::Timecode => (
                        "Does the show use timecode?",
                        "Show Master can send timecode to lights and video, or follow timecode from \
                         the sound desk or show control (MTC or LTC). Leave it as it is to run the \
                         show with GO alone.",
                    ),
                    Step::Remote => (
                        "Control from a tablet or phone?",
                        "With the remote on, a tablet in the same network runs GO, panic and the cue \
                         list in a browser. Pair a device with its QR code; a PIN keeps others out.",
                    ),
                    Step::Ready => (
                        "Ready for the show",
                        "This is how the show is set up. Everything can be changed later under \
                         Setup.",
                    ),
                };
                ui.label(RichText::new(title).size(24.0).strong().color(TEXT));
                ui.add_space(4.0);
                ui.label(RichText::new(about).color(MUTED));
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                let footer = 54.0;
                egui::ScrollArea::vertical()
                    .id_salt("sm_wizard")
                    .max_height((ui.available_height() - footer).max(80.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| match step {
                        Step::Audio => crate::setup::page(self, ui, SetupPage::Audio),
                        Step::Outputs => crate::setup::page(self, ui, SetupPage::Outputs),
                        Step::Timecode => crate::setup::page(self, ui, SetupPage::Timecode),
                        Step::Remote => crate::setup::page(self, ui, SetupPage::Remote),
                        Step::Ready => self.wizard_summary(ui),
                    });
                ui.add_space(8.0);
                self.wizard_footer(ui, step);
            });
    }

    fn wizard_summary(&mut self, ui: &mut egui::Ui) {
        let s = &self.client.state;
        let folder = s
            .show_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Not saved yet".into());
        let audio = if let Some(e) = &s.audio.error {
            (format!("Not running – {e}"), DANGER)
        } else if s.audio.is_null || s.audio.device.is_empty() {
            ("No interface – silent".into(), MUTED)
        } else {
            (
                format!(
                    "{}  ·  {} channels  ·  {}",
                    s.audio.device,
                    s.audio.channels,
                    crate::util::fmt_khz(s.audio.sample_rate)
                ),
                GREEN,
            )
        };
        let outs = &s.show.patch.outputs;
        let outputs = if outs.is_empty() {
            "None".to_string()
        } else {
            let names: Vec<&str> = outs.iter().map(|o| o.name.as_str()).take(6).collect();
            let more = if outs.len() > 6 { ", …" } else { "" };
            format!("{}: {}{more}", outs.len(), names.join(", "))
        };
        let t = &s.show.timecode;
        let tc = match t.source {
            sm_core::model::TcSource::Generator => {
                let rate = crate::setup::rate_label(t.rate);
                if rate.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    format!("Sent by Show Master at {rate} fps")
                } else {
                    format!("Sent by Show Master at {rate}")
                }
            }
            sm_core::model::TcSource::External => match t.input.kind {
                sm_core::model::TcInputKind::Mtc => format!(
                    "Follows MTC from {}",
                    if t.input.mtc_port.is_empty() {
                        "any MIDI input"
                    } else {
                        &t.input.mtc_port
                    }
                ),
                sm_core::model::TcInputKind::Ltc => format!(
                    "Follows LTC on {} channel {}",
                    t.input.ltc_device.as_deref().unwrap_or("the default input"),
                    t.input.ltc_channel + 1
                ),
            },
        };
        let r = &s.show.settings.remote;
        let remote = if !r.enabled {
            "Off".to_string()
        } else {
            let at = s
                .remote
                .urls
                .first()
                .map(|u| crate::setup::remote::short_address(u).to_string())
                .unwrap_or_else(|| format!("port {}", r.port));
            let pin = if r.control_pin.is_empty() {
                "no PIN"
            } else {
                "with PIN"
            };
            format!("On at {at}, {pin}")
        };
        egui::Grid::new("sm_summary")
            .num_columns(2)
            .spacing([22.0, 10.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Show folder").color(MUTED));
                ui.label(RichText::new(folder).strong());
                ui.end_row();
                ui.label(RichText::new("Interface").color(MUTED));
                ui.label(RichText::new(audio.0).strong().color(audio.1));
                ui.end_row();
                for (k, v) in [("Outputs", outputs), ("Timecode", tc), ("Remote", remote)] {
                    ui.label(RichText::new(k).color(MUTED));
                    ui.label(RichText::new(v).strong());
                    ui.end_row();
                }
            });
        ui.add_space(14.0);
        ui.label(
            RichText::new(
                "Next: add cues with the + button or drop audio files onto the cue list.",
            )
            .color(MUTED),
        );
    }

    fn wizard_footer(&mut self, ui: &mut egui::Ui, step: Step) {
        let big = |text: &str, primary: bool| {
            let b = egui::Button::new(
                RichText::new(text.to_string())
                    .size(15.0)
                    .strong()
                    .color(if primary { ON_GREEN } else { TEXT }),
            )
            .corner_radius(8)
            .min_size(Vec2::new(130.0, 38.0));
            if primary {
                b.fill(GREEN)
            } else {
                b
            }
        };
        let w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(w, 38.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                if step != Step::Ready
                    && ui
                        .add(
                            egui::Button::new(RichText::new("Skip setup").color(MUTED))
                                .frame(false),
                        )
                        .on_hover_text("Open the show now; set it up later under Setup")
                        .clicked()
                {
                    self.finish_wizard();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match step.next() {
                        Some(n) => {
                            if ui.add(big("Next", true)).clicked() {
                                self.page = Page::Wizard(n);
                            }
                        }
                        None => {
                            if ui.add(big("Open show", true)).clicked() {
                                self.finish_wizard();
                            }
                        }
                    }
                    if let Some(b) = step.back() {
                        if ui.add(big("Back", false)).clicked() {
                            self.page = Page::Wizard(b);
                        }
                    }
                });
            },
        );
    }
}

// ---------------------------------------------------------------- widgets

#[derive(Clone, Copy)]
enum Icon {
    New,
    Open,
    Package,
}

fn action_card(
    ui: &mut egui::Ui,
    w: f32,
    icon: Icon,
    title: &str,
    about: &str,
    primary: bool,
) -> egui::Response {
    let h = 168.0;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title));
    let hot = r.hovered();
    let p = ui.painter();
    let stroke = if primary {
        GREEN.gamma_multiply(if hot { 1.0 } else { 0.7 })
    } else if hot {
        Color32::from_rgb(90, 98, 112)
    } else {
        LINE
    };
    p.rect(
        rect,
        CornerRadius::same(14),
        if hot {
            Color32::from_rgba_premultiplied(26, 29, 34, 236)
        } else {
            GLASS
        },
        Stroke::new(if primary { 1.5 } else { 1.0 }, stroke),
        egui::StrokeKind::Inside,
    );
    let tile = Rect::from_min_size(rect.min + Vec2::new(20.0, 20.0), Vec2::splat(40.0));
    p.rect_filled(
        tile,
        CornerRadius::same(10),
        if primary {
            GREEN
        } else {
            Color32::from_rgb(40, 45, 54)
        },
    );
    let ink = if primary { ON_GREEN } else { TEXT };
    draw_icon(p, tile.shrink(11.0), icon, ink);
    p.text(
        rect.min + Vec2::new(20.0, 78.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(18.0),
        TEXT,
    );
    let galley = p.layout(
        about.to_string(),
        egui::FontId::proportional(13.0),
        MUTED,
        w - 40.0,
    );
    p.galley(rect.min + Vec2::new(20.0, 106.0), galley, MUTED);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn draw_icon(p: &egui::Painter, r: Rect, icon: Icon, ink: Color32) {
    let s = Stroke::new(2.2, ink);
    match icon {
        Icon::New => {
            p.line_segment([r.center_top(), r.center_bottom()], s);
            p.line_segment([r.left_center(), r.right_center()], s);
        }
        Icon::Open => {
            let body = Rect::from_min_max(r.min + Vec2::new(0.0, 4.0), r.max);
            p.rect_stroke(body, CornerRadius::same(2), s, egui::StrokeKind::Middle);
            p.line_segment(
                [r.min + Vec2::new(0.0, 4.0), r.min + Vec2::new(6.0, 0.0)],
                s,
            );
            p.line_segment(
                [r.min + Vec2::new(6.0, 0.0), r.min + Vec2::new(10.0, 4.0)],
                s,
            );
        }
        Icon::Package => {
            // A box with a lid line.
            let body = Rect::from_min_max(r.min + Vec2::new(0.0, 3.0), r.max);
            p.rect_stroke(body, CornerRadius::same(2), s, egui::StrokeKind::Middle);
            let y = body.top() + 5.0;
            p.line_segment([pos2(body.left(), y), pos2(body.right(), y)], s);
            p.line_segment(
                [
                    pos2(r.center().x - 3.0, y + 4.0),
                    pos2(r.center().x + 3.0, y + 4.0),
                ],
                s,
            );
        }
    }
}

/// The Show Master mark: a play triangle and three cue bars, the top one lit.
fn show_master_mark(p: &egui::Painter, r: Rect) {
    p.rect(
        r,
        CornerRadius::same(10),
        Color32::from_rgb(20, 23, 28),
        Stroke::new(1.0, Color32::from_rgb(42, 47, 56)),
        egui::StrokeKind::Inside,
    );
    let u = r.width() / 1024.0;
    let at = |x: f32, y: f32| pos2(r.left() + x * u, r.top() + y * u);
    p.add(egui::Shape::convex_polygon(
        vec![at(236.0, 300.0), at(338.0, 366.0), at(236.0, 432.0)],
        GREEN,
        Stroke::NONE,
    ));
    for (y, w, c) in [
        (326.0, 404.0, GREEN),
        (472.0, 328.0, Color32::from_rgb(91, 100, 114)),
        (618.0, 368.0, Color32::from_rgb(58, 65, 76)),
    ] {
        let bar = Rect::from_min_max(at(384.0, y), at(384.0 + w, y + 80.0));
        p.rect_filled(bar, CornerRadius::same((40.0 * u) as u8), c);
    }
}

fn recent_row(ui: &mut egui::Ui, p: &Path) -> egui::Response {
    let w = ui.available_width();
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::click());
    let name = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let there = p.is_file();
    let painter = ui.painter();
    if r.hovered() {
        painter.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(32, 36, 43));
    }
    let dot = pos2(rect.left() + 18.0, rect.center().y);
    painter.circle_filled(dot, 4.0, if there { GREEN } else { LINE });
    painter.text(
        pos2(rect.left() + 34.0, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        &name,
        egui::FontId::proportional(15.0),
        if there { TEXT } else { MUTED },
    );
    let place = p
        .parent()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let place = if there {
        place
    } else {
        format!("Missing – {place}")
    };
    painter.text(
        pos2(rect.left() + 34.0, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        place,
        egui::FontId::proportional(12.0),
        MUTED,
    );
    let when = std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    painter.text(
        pos2(rect.right() - 14.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        when,
        egui::FontId::proportional(12.0),
        MUTED,
    );
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn path_field(ui: &mut egui::Ui, p: &Path, empty: &str) {
    let (text, color) = if p.as_os_str().is_empty() {
        (empty.to_string(), MUTED)
    } else {
        (p.display().to_string(), TEXT)
    };
    Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(300.0);
            ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
        })
        .response
        .on_hover_text(p.display().to_string());
}

fn form_card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(Color32::from_rgba_premultiplied(18, 20, 24, 242))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(28, 24))
        .show(ui, |ui| {
            ui.set_max_width(760.0);
            add(ui);
        });
}

fn notice(ui: &mut egui::Ui, color: Color32, text: &str) {
    Frame::new()
        .fill(Color32::from_rgba_premultiplied(40, 14, 14, 230))
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color));
        });
}

/// 1 Audio — 2 Outputs — … — 5 Ready; done steps ticked.
fn steps_bar(ui: &mut egui::Ui, now: Step) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::hover());
    let p = ui.painter();
    let n = STEPS.len();
    let seg = w / n as f32;
    let y = rect.top() + 12.0;
    for (i, (s, label)) in STEPS.iter().enumerate() {
        let x = rect.left() + seg * i as f32 + 12.0;
        let c = pos2(x, y);
        let done = i < now.index();
        let here = *s == now;
        if done {
            p.circle_filled(c, 11.0, GREEN.gamma_multiply(0.85));
            let s = Stroke::new(2.0, ON_GREEN);
            p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(-1.5, 4.0)], s);
            p.line_segment([c + Vec2::new(-1.5, 4.0), c + Vec2::new(5.0, -4.0)], s);
        } else if here {
            p.circle_filled(c, 11.0, GREEN);
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                egui::FontId::proportional(13.0),
                ON_GREEN,
            );
        } else {
            p.circle_stroke(c, 10.0, Stroke::new(1.5, LINE));
            p.text(
                c,
                egui::Align2::CENTER_CENTER,
                (i + 1).to_string(),
                egui::FontId::proportional(13.0),
                MUTED,
            );
        }
        let text = p.text(
            pos2(x + 18.0, y),
            egui::Align2::LEFT_CENTER,
            *label,
            egui::FontId::proportional(14.0),
            if here || done { TEXT } else { MUTED },
        );
        if i + 1 < n {
            let (a, b) = (text.right() + 12.0, x + seg - 16.0);
            if b > a {
                p.line_segment(
                    [pos2(a, y), pos2(b, y)],
                    Stroke::new(
                        2.0,
                        if done {
                            GREEN.gamma_multiply(0.7)
                        } else {
                            LINE
                        },
                    ),
                );
            }
        }
    }
}

// ---------------------------------------------------------------- banner

/// The start screen's banner: a dark control room, the lit cue list of the
/// logo glowing on the right, a waveform running through and soft green
/// light from above. Made here, so no picture file.
pub fn banner_image(w: usize, h: usize) -> egui::ColorImage {
    fn lin(c: [f32; 3]) -> [f32; 3] {
        c.map(|v| (v / 255.0).powf(2.2))
    }
    fn hash(x: u32, y: u32) -> f32 {
        let mut n = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
        n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
        ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
    }
    fn smooth(a: f32, b: f32, x: f32) -> f32 {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    /// Distance to a rounded bar from (x0, y) to (x1, y) with radius r.
    fn bar(px: f32, py: f32, x0: f32, x1: f32, y: f32, r: f32) -> f32 {
        let cx = px.clamp(x0, x1);
        ((px - cx).powi(2) + (py - y).powi(2)).sqrt() - r
    }
    let top = lin([14.0, 18.0, 22.0]);
    let bottom = lin([4.0, 5.0, 6.0]);
    let green = lin([34.0, 197.0, 94.0]);
    let mint = lin([170.0, 255.0, 200.0]);
    let grey = lin([91.0, 100.0, 114.0]);
    let teal = lin([40.0, 120.0, 140.0]);
    let a = w as f32 / h as f32;
    // The cue list: (x0, x1, y, colour, strength); x in units of the height.
    let x0 = 0.64 * a;
    let bars = [
        (x0, x0 + 0.42, 0.40, green, 1.0),
        (x0, x0 + 0.34, 0.52, grey, 0.45),
        (x0, x0 + 0.38, 0.64, grey, 0.28),
    ];
    let tri = (x0 - 0.10, 0.40);
    // Light from above: (origin x of the width, angle, spread, colour, strength).
    let beams = [
        (0.70, 0.18, 0.07, green, 0.45),
        (0.88, -0.22, 0.06, mint, 0.30),
        (0.50, 0.45, 0.05, teal, 0.30),
    ];
    let mut pixels = Vec::with_capacity(w * h);
    for py in 0..h {
        let v = py as f32 / h as f32;
        for px in 0..w {
            let u = px as f32 / w as f32;
            let x = u * a;
            let mut c = [0.0f32; 3];
            let base = smooth(0.0, 1.0, v);
            for k in 0..3 {
                c[k] = top[k] + (bottom[k] - top[k]) * base;
            }
            let haze = 0.65
                + 0.2 * (u * 6.0 + v * 3.0).sin() * (v * 5.0 - u * 2.0).cos()
                + 0.15 * (u * 15.0 - v * 9.0).sin();
            for (ox, ang, spread, col, k) in beams {
                let dx = (u - ox) * a;
                let dy = v + 0.08;
                let an = dx.atan2(dy) - ang;
                let fall = (-(an / spread).powi(2)).exp();
                let len = (dx * dx + dy * dy).sqrt();
                let fade = (-len * 1.2).exp() * smooth(0.0, 0.12, dy);
                let b = fall * fade * k * haze;
                for i in 0..3 {
                    c[i] += col[i] * b * 0.45;
                }
            }
            // The cue bars: a solid core with a soft glow.
            for (b0, b1, y, col, k) in bars {
                let d = bar(x, v, b0, b1, y, 0.035);
                let core = smooth(0.004, -0.004, d);
                let glow =
                    (-(d.max(0.0) / 0.05).powi(2)).exp() * 0.35 + (-d.max(0.0) / 0.18).exp() * 0.12;
                for i in 0..3 {
                    c[i] += col[i] * (core * 0.9 + glow) * k;
                }
            }
            // The play triangle in front of the lit cue.
            let (tx, ty) = (x - tri.0, v - tri.1);
            let inside = tx > -0.045 && tx < 0.045 && ty.abs() < (0.045 - tx) * 0.65;
            let tglow = (-(tx * tx + ty * ty) / 0.01).exp() * 0.5;
            for i in 0..3 {
                c[i] += green[i] * (if inside { 0.95 } else { 0.0 } + tglow * 0.5);
            }
            // A waveform running through below the list.
            let wave_y = 0.82
                + 0.025
                    * ((u * 41.0).sin() * (u * 7.3).cos())
                    * (0.4 + 0.6 * (u * 3.1 + 1.0).sin().abs());
            let amp =
                0.004 + 0.016 * ((u * 23.0).sin() * (u * 5.0).sin()).abs() * smooth(0.2, 0.9, u);
            let dw = (v - wave_y).abs() - amp;
            let wv =
                (-(dw.max(0.0) / 0.003).powi(2)).exp() * 0.14 + (-dw.max(0.0) / 0.03).exp() * 0.025;
            for i in 0..3 {
                c[i] += green[i] * wv * smooth(0.05, 0.6, u);
            }
            // Floor reflection of the cue list.
            let floor = smooth(0.86, 0.9, v);
            if floor > 0.0 {
                let ry = 2.0 * 0.88 - v;
                let d = bar(x, ry, bars[0].0, bars[0].1, bars[0].2 + 0.30, 0.04);
                let refl = (-d.max(0.0) / 0.08).exp() * 0.08 * floor;
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - 0.25 * floor) + green[i] * refl;
                }
            }
            let left = 0.45 + 0.55 * smooth(0.12, 0.62, u);
            let (cx, cy) = (u - 0.62, v - 0.5);
            let vig = 1.0 - 0.6 * (cx * cx * 1.2 + cy * cy * 1.6).min(1.0);
            let dither = (hash(px as u32 + 7, py as u32 + 13) - 0.5) / 255.0;
            let rgb = c.map(|x| {
                let x = x * left * vig;
                let t = 1.0 - (-x * 1.6).exp();
                ((t.powf(1.0 / 2.2) + dither).clamp(0.0, 1.0) * 255.0) as u8
            });
            pixels.push(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        }
    }
    egui::ColorImage::new([w, h], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_safe_folder_names() {
        assert_eq!(clean_name("  Gala: 2026/27 "), "Gala- 2026-27");
        assert_eq!(clean_name("..."), "");
        let st = StartState {
            name: "Gala".into(),
            location: PathBuf::from("/shows"),
            ..Default::default()
        };
        assert_eq!(st.show_file(), PathBuf::from("/shows/Gala/Gala.smshow"));
    }

    #[test]
    fn wizard_steps_go_both_ways() {
        assert_eq!(Step::Audio.back(), None);
        assert_eq!(Step::Audio.next(), Some(Step::Outputs));
        assert_eq!(Step::Ready.next(), None);
        assert_eq!(Step::Ready.back(), Some(Step::Remote));
    }
}
