//! The application shell: top bar, status bar, mode switch, file handling,
//! hotkeys, dialogs and toasts. The two main views live in `edit.rs` and
//! `live.rs`, the setup window in `setup.rs`.

use crate::client::Client;
use crate::model::{Issue, Mode, Severity};
use crate::theme;
use egui::{Align2, Color32, Id, Key, Modifiers, RichText, ViewportCommand};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Callback for a command result.
pub type Reply = Box<dyn FnOnce(&mut App, Result<Value, String>)>;

pub const AUDIO_EXTENSIONS: [&str; 11] = [
    "wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "oga", "m4a", "aac", "caf",
];
pub const SHOW_EXTENSION: &str = "smshow";

/// Hotkey actions (values of `show.hotkeys`).
pub const ACTIONS: [(&str, &str); 15] = [
    ("go", "GO"),
    ("panic", "Panic"),
    ("stop_all", "Stop all"),
    ("toggle_pause", "Pause / resume"),
    ("next", "Playhead down"),
    ("previous", "Playhead up"),
    ("reset", "Reset"),
    ("preview", "Preview selected cue"),
    ("toggle_mode", "Edit / Live"),
    ("undo", "Undo"),
    ("redo", "Redo"),
    ("save", "Save"),
    ("duplicate", "Duplicate cues"),
    ("delete", "Delete cues"),
    ("find", "Go to cue number"),
];

/// UI state shared by all views.
#[derive(Default)]
pub struct View {
    /// Selected cue ids (any depth), in click order.
    pub selection: Vec<u64>,
    /// The cue list shown in the main area.
    pub list: Option<u64>,
}

enum Pending {
    New,
    Open(PathBuf),
    Close,
}

enum Modal {
    Preflight(Vec<Issue>),
    LeaveLive,
    Unsaved(Pending),
    Recovery,
    Update,
    CloseWhileRunning,
    Message { title: String, text: String },
}

struct Toast {
    text: String,
    error: bool,
    until: Instant,
}

pub struct App {
    pub client: Client,
    pub view: View,
    pub edit: crate::edit::EditState,
    pub live: crate::live::LiveState,
    pub setup: crate::setup::SetupState,
    pending: HashMap<u64, Reply>,
    modal: Option<Modal>,
    toasts: Vec<Toast>,
    last_go: Option<Instant>,
    panic_tap: Option<Instant>,
    allow_close: bool,
    high_contrast: Option<bool>,
    recovery_asked: bool,
    pub goto_text: String,
    focus_goto: bool,
    /// Path of a showfile to open at start (command line / file association).
    open_at_start: Option<PathBuf>,
}

impl App {
    pub fn new(ctx: &egui::Context, mut client: Client, open_at_start: Option<PathBuf>) -> Self {
        client.set_repaint_context(ctx.clone());
        let mut app = Self {
            client,
            view: View::default(),
            edit: Default::default(),
            live: Default::default(),
            setup: Default::default(),
            pending: HashMap::new(),
            modal: None,
            toasts: Vec::new(),
            last_go: None,
            panic_tap: None,
            allow_close: false,
            high_contrast: None,
            recovery_asked: false,
            goto_text: String::new(),
            focus_goto: false,
            open_at_start,
        };
        // The theme is applied on the first frame (`logic`), on the context that draws.
        let _ = &mut app;
        app
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    /// Send a command; failures are shown as a toast.
    pub fn cmd(&mut self, cmd: Value) {
        self.client.send(cmd);
    }

    /// Send a command and handle its result.
    pub fn cmd_then(
        &mut self,
        cmd: Value,
        then: impl FnOnce(&mut App, Result<Value, String>) + 'static,
    ) {
        let id = self.client.send(cmd);
        self.pending.insert(id, Box::new(then));
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            error: false,
            until: Instant::now() + Duration::from_secs(3),
        });
    }

    pub fn toast_error(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            error: true,
            until: Instant::now() + Duration::from_secs(6),
        });
    }

    pub fn message(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.modal = Some(Modal::Message {
            title: title.into(),
            text: text.into(),
        });
    }

    /// Human-readable text for an error code from the runtime.
    pub fn error_text(code: &str) -> String {
        match code {
            "live_mode" => "Not possible in Live mode - switch to Edit first.".into(),
            "cues_running" => "Cues are running.".into(),
            "unsaved_changes" => "The show has unsaved changes.".into(),
            "forbidden" => "Not allowed.".into(),
            "path_required" => "Choose where to save the show.".into(),
            other => other.to_string(),
        }
    }

    fn handle_replies(&mut self) {
        for (id, result) in self.client.poll() {
            match self.pending.remove(&id) {
                Some(f) => f(self, result),
                None => {
                    if let Err(e) = result {
                        self.toast_error(Self::error_text(&e));
                    }
                }
            }
        }
    }

    pub fn mode(&self) -> Mode {
        self.client.state.mode
    }

    pub fn is_live(&self) -> bool {
        self.mode() == Mode::Live
    }

    /// The cue list shown in the main area.
    pub fn current_list(&self) -> Option<u64> {
        let lists = &self.client.state.show.cue_lists;
        self.view
            .list
            .filter(|l| lists.iter().any(|x| x.id == *l))
            .or_else(|| lists.first().map(|l| l.id))
    }

    // ------------------------------------------------------------------
    // Transport (shared by toolbar, live dashboard and hotkeys)
    // ------------------------------------------------------------------

    /// Time GO stays locked after a press (double-click protection).
    pub fn go_locked(&self) -> bool {
        let ms = u64::from(self.client.state.show.settings.go_debounce_ms);
        self.last_go
            .is_some_and(|t| t.elapsed() < Duration::from_millis(ms))
    }

    pub fn go(&mut self) {
        if self.go_locked() {
            return;
        }
        self.last_go = Some(Instant::now());
        let list = self.current_list();
        self.cmd(json!({ "cmd": "go", "list": list }));
    }

    /// Panic, honouring the double-tap safety setting. Returns true when it fired.
    pub fn panic(&mut self) -> bool {
        let settings = &self.client.state.show.settings;
        if settings.panic_double_tap {
            let window = Duration::from_millis(u64::from(settings.double_tap_ms));
            match self.panic_tap {
                Some(t) if t.elapsed() <= window => self.panic_tap = None,
                _ => {
                    self.panic_tap = Some(Instant::now());
                    self.toast("Press PANIC again to confirm");
                    return false;
                }
            }
        }
        self.cmd(json!({ "cmd": "panic" }));
        true
    }

    /// True while a first panic tap waits for its confirmation.
    pub fn panic_armed(&self) -> bool {
        let ms = u64::from(self.client.state.show.settings.double_tap_ms);
        self.panic_tap
            .is_some_and(|t| t.elapsed() <= Duration::from_millis(ms))
    }

    pub fn stop_all(&mut self) {
        self.cmd(json!({ "cmd": "stop_all" }));
    }

    pub fn toggle_pause(&mut self) {
        self.cmd(json!({ "cmd": "toggle_pause" }));
    }

    pub fn step_playhead(&mut self, down: bool) {
        let list = self.current_list();
        self.cmd(json!({ "cmd": if down { "next" } else { "previous" }, "list": list }));
    }

    pub fn reset(&mut self) {
        self.cmd(json!({ "cmd": "reset" }));
    }

    pub fn goto_number(&mut self, number: &str) {
        let number = number.trim().to_string();
        if number.is_empty() {
            return;
        }
        let n = number.clone();
        self.cmd_then(
            json!({ "cmd": "goto_number", "number": number }),
            move |app, r| match r {
                Ok(_) => app.toast(format!("Next: cue {n}")),
                Err(e) => app.toast_error(e),
            },
        );
    }

    // ------------------------------------------------------------------
    // Modes
    // ------------------------------------------------------------------

    pub fn request_mode(&mut self, mode: Mode) {
        match mode {
            Mode::Live if !self.is_live() => {
                self.cmd_then(json!({ "cmd": "run_preflight" }), |app, r| match r {
                    Ok(v) => {
                        let issues: Vec<Issue> =
                            serde_json::from_value(v["issues"].clone()).unwrap_or_default();
                        if issues.iter().any(|i| i.severity != Severity::Info) {
                            app.modal = Some(Modal::Preflight(issues));
                        } else {
                            app.cmd(json!({ "cmd": "set_mode", "mode": "live" }));
                        }
                    }
                    Err(e) => app.toast_error(App::error_text(&e)),
                });
            }
            Mode::Edit if self.is_live() => {
                self.cmd_then(json!({ "cmd": "set_mode", "mode": "edit" }), |app, r| {
                    if let Err(e) = r {
                        if e == "cues_running" {
                            app.modal = Some(Modal::LeaveLive);
                        } else {
                            app.toast_error(App::error_text(&e));
                        }
                    }
                });
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Files (native system dialogs)
    // ------------------------------------------------------------------

    fn show_dir(&self) -> Option<PathBuf> {
        self.client
            .state
            .show_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .map(Path::to_path_buf)
    }

    /// Native "open" dialog for audio files (multiple selection).
    pub fn pick_audio_files(&self) -> Vec<PathBuf> {
        let mut d = rfd::FileDialog::new()
            .set_title("Add audio files")
            .add_filter("Audio", &AUDIO_EXTENSIONS);
        if let Some(dir) = self.show_dir() {
            d = d.set_directory(dir);
        }
        d.pick_files().unwrap_or_default()
    }

    /// Native "open" dialog for one audio file.
    pub fn pick_audio_file(&self) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new()
            .set_title("Choose audio file")
            .add_filter("Audio", &AUDIO_EXTENSIONS);
        if let Some(dir) = self.show_dir() {
            d = d.set_directory(dir);
        }
        d.pick_file()
    }

    /// Add audio cues for files (after the selection), selecting the last new one.
    pub fn add_audio_cues(&mut self, files: Vec<PathBuf>) {
        if self.is_live() {
            self.toast_error(Self::error_text("live_mode"));
            return;
        }
        let list = self.current_list();
        let after = self.view.selection.last().copied();
        // Each cue is inserted right after the anchor, so go backwards to keep the file order.
        for f in files.into_iter().rev() {
            let cmd = json!({ "cmd": "add_cue", "kind": "audio", "file": f, "list": list, "after": after });
            self.cmd_then(cmd, |app, r| match r {
                Ok(v) => {
                    if let Some(id) = v["cue"].as_u64() {
                        app.view.selection = vec![id];
                    }
                }
                Err(e) => app.toast_error(App::error_text(&e)),
            });
        }
    }

    pub fn new_show(&mut self) {
        self.file_action(Pending::New, false);
    }

    pub fn open_show_dialog(&mut self) {
        let mut d = rfd::FileDialog::new()
            .set_title("Open show")
            .add_filter("Show Master show", &[SHOW_EXTENSION]);
        if let Some(dir) = self.show_dir() {
            d = d.set_directory(dir);
        }
        if let Some(path) = d.pick_file() {
            self.file_action(Pending::Open(path), false);
        }
    }

    pub fn open_show(&mut self, path: PathBuf) {
        self.file_action(Pending::Open(path), false);
    }

    fn file_action(&mut self, action: Pending, force: bool) {
        let cmd = match &action {
            Pending::New => json!({ "cmd": "new_show", "force": force }),
            Pending::Open(p) => json!({ "cmd": "open_show", "path": p, "force": force }),
            Pending::Close => return,
        };
        self.cmd_then(cmd, move |app, r| match r {
            Ok(_) => app.view = View::default(),
            Err(e) if e == "unsaved_changes" => app.modal = Some(Modal::Unsaved(action)),
            Err(e) => app.toast_error(App::error_text(&e)),
        });
    }

    /// Save; asks for a location when the show has never been saved.
    pub fn save(&mut self, then: Option<Reply>) {
        if self.client.state.show_path.is_none() {
            self.save_as(then);
            return;
        }
        self.cmd_then(json!({ "cmd": "save_show" }), move |app, r| match r {
            Ok(v) => {
                app.toast("Show saved");
                if let Some(f) = then {
                    f(app, Ok(v));
                }
            }
            Err(e) => app.toast_error(format!("Save failed: {}", App::error_text(&e))),
        });
    }

    pub fn save_as(&mut self, then: Option<Reply>) {
        let name = format!("{}.{SHOW_EXTENSION}", self.client.state.show.name);
        let mut d = rfd::FileDialog::new()
            .set_title("Save show as")
            .add_filter("Show Master show", &[SHOW_EXTENSION])
            .set_file_name(name);
        if let Some(dir) = self.show_dir() {
            d = d.set_directory(dir);
        }
        let Some(path) = d.save_file() else { return };
        self.cmd_then(
            json!({ "cmd": "save_show", "path": path }),
            move |app, r| match r {
                Ok(v) => {
                    app.toast("Show saved");
                    if let Some(f) = then {
                        f(app, Ok(v));
                    }
                }
                Err(e) => app.toast_error(format!("Save failed: {}", App::error_text(&e))),
            },
        );
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if files.is_empty() {
            return;
        }
        let ext = |p: &Path| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default()
        };
        if let Some(show) = files.iter().find(|p| ext(p) == SHOW_EXTENSION) {
            self.open_show(show.clone());
            return;
        }
        let audio: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| AUDIO_EXTENSIONS.contains(&ext(p).as_str()))
            .collect();
        if audio.is_empty() {
            self.toast_error("Only audio files and .smshow files can be dropped here.");
        } else {
            self.add_audio_cues(audio);
        }
    }

    // ------------------------------------------------------------------
    // Hotkeys
    // ------------------------------------------------------------------

    /// `KeyboardEvent.code`-style name of a key (the format of `show.hotkeys`).
    pub fn key_code(key: Key) -> Option<String> {
        let name = key.name();
        Some(match key {
            Key::ArrowDown | Key::ArrowUp | Key::ArrowLeft | Key::ArrowRight => {
                format!("Arrow{name}")
            }
            Key::Num0
            | Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9 => format!("Digit{name}"),
            k if name.len() == 1
                && name.chars().all(|c| c.is_ascii_uppercase())
                && k != Key::Space =>
            {
                format!("Key{name}")
            }
            Key::Space
            | Key::Escape
            | Key::Enter
            | Key::Tab
            | Key::Backspace
            | Key::Delete
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown => name.to_string(),
            _ if name.starts_with('F') && name[1..].chars().all(|c| c.is_ascii_digit()) => {
                name.to_string()
            }
            _ => return None,
        })
    }

    /// `Ctrl+Shift+Alt+KeyZ` style combination. "Ctrl" is Cmd on macOS.
    pub fn combo(key: Key, m: Modifiers) -> Option<String> {
        let mut s = String::new();
        if m.command {
            s.push_str("Ctrl+");
        }
        if m.shift {
            s.push_str("Shift+");
        }
        if m.alt {
            s.push_str("Alt+");
        }
        if m.mac_cmd && !m.command || (cfg!(target_os = "macos") && m.ctrl) {
            s.push_str("Meta+");
        }
        Some(s + &Self::key_code(key)?)
    }

    /// Normalise a stored combination (modifier order may differ).
    pub fn normalize_combo(combo: &str) -> String {
        let parts: Vec<&str> = combo.split('+').collect();
        let Some((key, mods)) = parts.split_last() else {
            return combo.to_string();
        };
        let mut out = String::new();
        for m in ["Ctrl", "Shift", "Alt", "Meta"] {
            if mods.contains(&m) {
                out.push_str(m);
                out.push('+');
            }
        }
        out + key
    }

    fn hotkeys(&mut self, ctx: &egui::Context) {
        if self.modal.is_some() || self.setup.capturing_hotkey() {
            return;
        }
        let map: HashMap<String, String> = self
            .client
            .state
            .show
            .hotkeys
            .iter()
            .map(|(k, v)| (Self::normalize_combo(k), v.clone()))
            .collect();
        let typing = ctx.egui_wants_keyboard_input();
        let live = self.is_live();
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            i.events.retain(|e| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = e
                else {
                    return true;
                };
                let Some(combo) = Self::combo(*key, *modifiers) else {
                    return true;
                };
                let Some(action) = map.get(&combo) else {
                    return true;
                };
                // While typing only Escape (panic) counts, and only in live mode.
                if typing && !(*key == Key::Escape && live) {
                    return true;
                }
                if !*repeat {
                    fired.push(action.clone());
                }
                false // consumed: a focused button must not also react to Space/Enter
            });
        });
        for action in fired {
            self.run_action(&action);
        }
    }

    pub fn run_action(&mut self, action: &str) {
        let editing = !self.is_live();
        match action {
            "go" => self.go(),
            "panic" => {
                self.panic();
            }
            "stop_all" => self.stop_all(),
            "toggle_pause" => self.toggle_pause(),
            "next" => self.step_playhead(true),
            "previous" => self.step_playhead(false),
            "reset" => self.reset(),
            "preview" => {
                if let Some(cue) = self.view.selection.last().copied() {
                    self.cmd(json!({ "cmd": "start_cue", "cue": cue }));
                }
            }
            "toggle_mode" => self.request_mode(if self.is_live() {
                Mode::Edit
            } else {
                Mode::Live
            }),
            "undo" if editing => self.cmd(json!({ "cmd": "undo" })),
            "redo" if editing => self.cmd(json!({ "cmd": "redo" })),
            "save" => self.save(None),
            "duplicate" if editing && !self.view.selection.is_empty() => {
                let cues = self.view.selection.clone();
                self.cmd_then(
                    json!({ "cmd": "duplicate_cues", "cues": cues }),
                    |app, r| match r {
                        Ok(v) => {
                            if let Some(ids) = v["cues"].as_array() {
                                app.view.selection = ids.iter().filter_map(Value::as_u64).collect();
                            }
                        }
                        Err(e) => app.toast_error(App::error_text(&e)),
                    },
                );
            }
            "delete" if editing && !self.view.selection.is_empty() => {
                let cues = std::mem::take(&mut self.view.selection);
                self.cmd(json!({ "cmd": "delete_cues", "cues": cues }));
            }
            "find" => self.focus_goto = true,
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Frame
    // ------------------------------------------------------------------

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let hc = self.client.state.show.settings.high_contrast;
        if self.high_contrast != Some(hc) {
            ctx.set_theme(egui::Theme::Dark);
            theme::apply(ctx, hc);
            self.high_contrast = Some(hc);
        }
    }

    /// Everything that is not drawing: state, replies, signals, input.
    pub fn logic(&mut self, ctx: &egui::Context) {
        self.client.refresh();
        self.handle_replies();
        self.apply_theme(ctx);
        let handle = self.client.handle.clone();
        if handle.raise.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if handle.quit_requested.load(Ordering::SeqCst) && !self.allow_close {
            // Quit button, remote quit or an installed update: close without asking.
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if let Some(path) = self.open_at_start.take() {
            self.open_show(path);
        }
        if self.client.state.recovery_available && !self.recovery_asked && self.modal.is_none() {
            self.recovery_asked = true;
            self.modal = Some(Modal::Recovery);
        }
        self.handle_close_request(ctx);
        self.handle_dropped_files(ctx);
        crate::setup::capture_keys(self, ctx);
        self.hotkeys(ctx);
        self.toasts.retain(|t| t.until > Instant::now());
        // Clocks and meters move continuously.
        ctx.request_repaint_after(Duration::from_millis(40));
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.allow_close {
            return;
        }
        let s = &self.client.state;
        let running = !self.client.tick.active.is_empty();
        let locked = s.mode == Mode::Live && s.show.settings.quit_lock;
        if running || locked {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.modal = Some(Modal::CloseWhileRunning);
        } else if s.dirty {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.modal = Some(Modal::Unsaved(Pending::Close));
        } else {
            self.allow_close = true;
        }
    }

    fn close_now(&mut self, ctx: &egui::Context) {
        self.allow_close = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    /// Draw the whole window.
    pub fn ui_root(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            if self.is_live() {
                crate::live::show(self, ui);
            } else {
                crate::edit::show(self, ui);
            }
        });
        crate::setup::show(self, &ctx);
        self.modals(&ctx);
        self.draw_toasts(&ctx);
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::unique("drop"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(160));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Drop audio files to add cues, or a .smshow file to open it",
                egui::FontId::proportional(22.0),
                Color32::WHITE,
            );
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let live = self.is_live();
        ui.horizontal(|ui| {
            if live {
                ui.label(
                    RichText::new(" LIVE ")
                        .strong()
                        .color(Color32::WHITE)
                        .background_color(theme::LIVE)
                        .size(16.0),
                );
            }
            let s = &self.client.state;
            let title = if s.dirty {
                format!("{} *", s.show.name)
            } else {
                s.show.name.clone()
            };
            ui.label(RichText::new(title).strong().size(17.0))
                .on_hover_text(
                    s.show_path
                        .clone()
                        .unwrap_or_else(|| "Not saved yet".into()),
                );
            ui.separator();
            if !live {
                ui.menu_button("File", |ui| {
                    if ui.button("New show").clicked() {
                        self.new_show();
                    }
                    if ui.button("Open…").clicked() {
                        self.open_show_dialog();
                    }
                    ui.separator();
                    if ui.button("Save").clicked() {
                        self.save(None);
                    }
                    if ui.button("Save as…").clicked() {
                        self.save_as(None);
                    }
                    ui.separator();
                    if ui
                        .button("Collect media into show folder")
                        .on_hover_text("Copy all audio files next to the showfile")
                        .clicked()
                    {
                        self.cmd_then(json!({ "cmd": "collect_media" }), |app, r| match r {
                            Ok(v) => app.toast(format!(
                                "Collected {} file(s)",
                                v["copied"].as_u64().unwrap_or(0)
                            )),
                            Err(e) => app.toast_error(App::error_text(&e)),
                        });
                    }
                });
                if ui.button("Save").clicked() {
                    self.save(None);
                }
                let (can_undo, can_redo) = (self.client.state.can_undo, self.client.state.can_redo);
                if ui
                    .add_enabled(can_undo, egui::Button::new("Undo"))
                    .on_hover_text("Undo")
                    .clicked()
                {
                    self.cmd(json!({ "cmd": "undo" }));
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new("Redo"))
                    .on_hover_text("Redo")
                    .clicked()
                {
                    self.cmd(json!({ "cmd": "redo" }));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Quit")
                    .on_hover_text("Quit Show Master")
                    .clicked()
                {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
                if !live && ui.button("Setup").clicked() {
                    self.setup.open = true;
                }
                self.mode_switch(ui);
                self.update_button(ui);
                let goto = ui.add(
                    egui::TextEdit::singleline(&mut self.goto_text)
                        .hint_text("#")
                        .desired_width(56.0)
                        .id(Id::unique("goto_box")),
                );
                if self.focus_goto {
                    goto.request_focus();
                    self.focus_goto = false;
                }
                if goto.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let n = std::mem::take(&mut self.goto_text);
                    self.goto_number(&n);
                }
                ui.label("Go to cue");
            });
        });
    }

    fn mode_switch(&mut self, ui: &mut egui::Ui) {
        let live = self.is_live();
        let pill = |text: &str, on: bool, color: Color32| {
            let t = RichText::new(text).strong();
            if on {
                egui::Button::new(t.color(Color32::WHITE)).fill(color)
            } else {
                egui::Button::new(t)
            }
        };
        // Right-to-left layout: LIVE is added first.
        if ui
            .add(pill("LIVE", live, theme::LIVE))
            .on_hover_text("Switch to Live mode (runs the pre-flight check)")
            .clicked()
            && !live
        {
            self.request_mode(Mode::Live);
        }
        if ui.add(pill("EDIT", !live, theme::EDIT)).clicked() && live {
            self.request_mode(Mode::Edit);
        }
    }

    fn update_button(&mut self, ui: &mut egui::Ui) {
        let u = &self.client.state.update;
        let label = match u.state.as_str() {
            "available" | "failed" => format!("Update {}", u.version.clone().unwrap_or_default()),
            "downloading" => format!(
                "Downloading {}%",
                (100 * u.done).checked_div(u.total).unwrap_or(0)
            ),
            "installing" => "Installing…".into(),
            "restarting" => "Restarting…".into(),
            _ => return,
        };
        let b = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
            .fill(Color32::from_rgb(21, 128, 61));
        if ui
            .add(b)
            .on_hover_text("A new version of Show Master is available")
            .clicked()
        {
            self.modal = Some(Modal::Update);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let s = &self.client.state;
        let t = &self.client.tick;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let small = |t: String| RichText::new(t).small();
            if let Some(err) = &s.audio.error {
                ui.label(small(format!("Audio: {err}")).color(theme::ERROR)).on_hover_text(err);
            } else if s.audio.is_null {
                ui.label(small("No audio device (silent)".into()).color(theme::WARN));
            }
            ui.label(small(format!("{} · {}", s.audio.backend, s.audio.device)));
            ui.label(small(format!("{} Hz", s.audio.sample_rate)));
            if let Some(b) = s.audio.buffer_frames {
                ui.label(small(format!("{b} frames")));
            }
            ui.label(small(format!("{} out", s.audio.channels)));
            ui.label(small(format!("CPU {:.0}%", t.cpu * 100.0)));
            let under = small(format!("Underruns {}", t.underruns));
            ui.label(if t.underruns > 0 { under.color(theme::ERROR) } else { under });
            ui.label(small(format!("Voices {}", t.voices)));
            ui.label(small(format!("MIDI ports {}", s.midi.ports.len())));
            if s.midi.rtp.enabled {
                let up = s.midi.rtp.peers.iter().filter(|p| p.state == "connected").count();
                ui.label(small(format!("RTP-MIDI {up}/{}", s.midi.rtp.peers.len())));
            }
            ui.label(small(format!("Remotes {}", s.remote.clients.len())));
            if let Some(url) = s.remote.urls.first() {
                ui.label(small(format!("{url}/remote"))).on_hover_text("Open this address on a tablet in the same network (Setup → Remote shows a QR code)");
            }
        });
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                for t in &self.toasts {
                    let (fill, stroke) = if t.error {
                        (theme::ERROR_BG, theme::ERROR)
                    } else {
                        (Color32::from_rgb(30, 41, 59), theme::RUNNING)
                    };
                    egui::Frame::new()
                        .fill(fill)
                        .stroke(egui::Stroke::new(1.0, stroke))
                        .corner_radius(6.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.label(RichText::new(&t.text).color(Color32::WHITE));
                        });
                    ui.add_space(6.0);
                }
            });
    }

    // ------------------------------------------------------------------
    // Modal dialogs
    // ------------------------------------------------------------------

    fn modals(&mut self, ctx: &egui::Context) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        let mut keep = true;
        let mut next: Option<Modal> = None;
        let resp = egui::Modal::new(Id::unique("app_modal")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            match &modal {
                Modal::Preflight(issues) => {
                    ui.heading("Pre-flight check");
                    ui.label("Before going live, please look at these points:");
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for i in issues {
                            let (tag, color) = match i.severity {
                                Severity::Error => ("ERROR", theme::ERROR),
                                Severity::Warning => ("WARNING", theme::WARN),
                                Severity::Info => ("INFO", theme::MUTED),
                            };
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(tag).strong().color(color));
                                if let Some(cue) = i.cue {
                                    if ui.link(&i.message).on_hover_text("Select this cue").clicked() {
                                        self.view.selection = vec![cue];
                                        keep = false;
                                    }
                                } else {
                                    ui.label(&i.message);
                                }
                            });
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Back to Edit").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("Go live anyway").strong().color(Color32::WHITE)).fill(theme::LIVE)).clicked() {
                            self.cmd(json!({ "cmd": "set_mode", "mode": "live" }));
                            keep = false;
                        }
                    });
                }
                Modal::LeaveLive => {
                    ui.heading("Cues are running");
                    ui.label("Switch to Edit mode anyway? The cues keep playing.");
                    ui.horizontal(|ui| {
                        if ui.button("Stay live").clicked() {
                            keep = false;
                        }
                        if ui.button("Switch to Edit").clicked() {
                            self.cmd(json!({ "cmd": "set_mode", "mode": "edit", "force": true }));
                            keep = false;
                        }
                    });
                }
                Modal::Unsaved(action) => {
                    ui.heading("Unsaved changes");
                    ui.label("The show has changes that are not saved.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.button("Don't save").clicked() {
                            keep = false;
                            match action {
                                Pending::Close => self.close_now(ui.ctx()),
                                Pending::New => self.file_action(Pending::New, true),
                                Pending::Open(p) => self.file_action(Pending::Open(p.clone()), true),
                            }
                        }
                        if ui.add(egui::Button::new(RichText::new("Save").strong()).fill(theme::EDIT)).clicked() {
                            keep = false;
                            let ctx2 = ui.ctx().clone();
                            let then: Reply = match action {
                                Pending::Close => Box::new(move |app: &mut App, _| app.close_now(&ctx2)),
                                Pending::New => Box::new(|app: &mut App, _| app.file_action(Pending::New, true)),
                                Pending::Open(p) => {
                                    let p = p.clone();
                                    Box::new(move |app: &mut App, _| app.file_action(Pending::Open(p), true))
                                }
                            };
                            self.save(Some(then));
                        }
                    });
                }
                Modal::Recovery => {
                    ui.heading("Recover unsaved changes?");
                    ui.label("Show Master was not closed normally. An automatic backup with newer changes than the saved show exists.");
                    ui.horizontal(|ui| {
                        if ui.button("Discard backup").clicked() {
                            self.cmd(json!({ "cmd": "recover", "accept": false }));
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("Recover").strong()).fill(theme::EDIT)).clicked() {
                            self.cmd(json!({ "cmd": "recover", "accept": true }));
                            keep = false;
                        }
                    });
                }
                Modal::Update => {
                    let u = self.client.state.update.clone();
                    ui.heading(format!("Show Master {} is available", u.version.clone().unwrap_or_default()));
                    ui.label(RichText::new(format!("Installed: {}", u.current)).color(theme::MUTED));
                    if let Some(notes) = u.notes.as_ref().filter(|n| !n.is_empty()) {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            ui.label(notes);
                        });
                    }
                    if u.state == "downloading" {
                        let p = if u.total > 0 { u.done as f32 / u.total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(p).show_percentage());
                    }
                    if let Some(e) = &u.error {
                        ui.label(RichText::new(e).color(theme::ERROR));
                    }
                    if u.automatic == Some(false) {
                        ui.label("This copy cannot update itself. Get the new installer from the download page.");
                    }
                    ui.label(RichText::new("Show Master restarts after the update. Save the show and stop all cues first.").small().color(theme::MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Later").clicked() {
                            keep = false;
                        }
                        ui.hyperlink_to("Download page", &u.download_page);
                        let busy = matches!(u.state.as_str(), "downloading" | "installing" | "restarting");
                        if u.automatic == Some(true) && ui.add_enabled(!busy, egui::Button::new(RichText::new("Update and restart").strong()).fill(Color32::from_rgb(21, 128, 61))).clicked() {
                            self.cmd_then(json!({ "cmd": "install_update" }), |app, r| {
                                if let Err(e) = r {
                                    app.toast_error(match e.as_str() {
                                        "unsaved_changes" => "Save the show before updating.".to_string(),
                                        "cues_running" => "Stop all cues before updating.".to_string(),
                                        other => App::error_text(other),
                                    });
                                }
                            });
                        }
                    });
                }
                Modal::CloseWhileRunning => {
                    ui.heading("Quit Show Master?");
                    let running = !self.client.tick.active.is_empty();
                    ui.label(if running { "Cues are playing. Quitting stops all audio immediately." } else { "Show Master is in Live mode." });
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add(egui::Button::new(RichText::new("Quit").strong().color(Color32::WHITE)).fill(theme::PANIC)).clicked() {
                            keep = false;
                            if self.client.state.dirty {
                                next = Some(Modal::Unsaved(Pending::Close));
                            } else {
                                self.close_now(ui.ctx());
                            }
                        }
                    });
                }
                Modal::Message { title, text } => {
                    ui.heading(title);
                    ui.label(text);
                    if ui.button("OK").clicked() {
                        keep = false;
                    }
                }
            }
        });
        if resp.should_close() && !matches!(modal, Modal::Recovery) {
            keep = false;
        }
        if let Some(n) = next {
            self.modal = Some(n);
        } else if keep && self.modal.is_none() {
            self.modal = Some(modal);
        }
    }
}
