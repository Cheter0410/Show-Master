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
    /// Shown the first time the window is closed: the show keeps running.
    RunsInBackground,
    /// Name for a new template from the selected cues.
    SaveTemplate(String),
    /// Kiosk unlock: PIN typed so far, and what to do afterwards (quit).
    Unlock(String, bool),
    Message {
        title: String,
        text: String,
    },
}

struct Toast {
    text: String,
    error: bool,
    until: Instant,
}

/// Result of work done on a background thread (show packages).
pub enum JobDone {
    Toast(String),
    Error(String),
    Open(PathBuf),
}

pub struct App {
    /// Background jobs (show packages) the window waits for.
    jobs: Vec<std::sync::mpsc::Receiver<JobDone>>,
    /// Cues copied in this window (also put on the system clipboard).
    clip: Option<String>,
    /// Text to hand to the system clipboard in the next pass.
    to_clipboard: Option<String>,
    /// Kiosk mode was unlocked in this session.
    kiosk_unlocked: bool,
    /// Kiosk start-up (switch to Live) done.
    kiosk_started: bool,
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
    /// Style last applied: (high contrast, touch mode).
    style_key: Option<(bool, bool)>,
    /// Per-machine preferences (touch mode).
    pub prefs: crate::prefs::Prefs,
    /// System-wide hotkeys of this machine (inactive unless started by `run`).
    pub global_hotkeys: crate::global_hotkeys::GlobalHotkeys,
    /// A touch event was seen this session (drives touch mode "Auto").
    pub touch_seen: bool,
    /// Size class of the window this frame.
    pub screen: crate::responsive::Screen,
    recovery_asked: bool,
    pub goto_text: String,
    focus_goto: bool,
    /// A text field had keyboard focus last frame (egui drops focus on Escape
    /// before `logic` runs, so this frame alone cannot tell).
    was_typing: bool,
    /// Path of a showfile to open at start (command line / file association).
    open_at_start: Option<PathBuf>,
    pub recent: crate::recent::Recent,
    last_show_path: Option<String>,
    /// After stepping the playhead in Edit mode, select the new playhead cue.
    select_playhead: bool,
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
            jobs: Vec::new(),
            clip: None,
            to_clipboard: None,
            kiosk_unlocked: false,
            kiosk_started: false,
            style_key: None,
            prefs: crate::prefs::Prefs::default(),
            global_hotkeys: Default::default(),
            touch_seen: false,
            screen: crate::responsive::Screen::default(),
            recovery_asked: false,
            goto_text: String::new(),
            focus_goto: false,
            was_typing: false,
            open_at_start,
            // Not persisted unless the real app loads it (tests must not touch the user's list).
            recent: crate::recent::Recent::load_from(None),
            last_show_path: None,
            select_playhead: false,
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

    /// Save the diagnostics bundle (log, system, state) via the system dialog.
    pub fn export_diagnostics(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export diagnostics")
            .add_filter("Text", &["txt"])
            .set_file_name("showmaster-diagnostics.txt")
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, self.client.handle.diagnostics()) {
            Ok(()) => self.toast(format!("Diagnostics saved to {}", path.display())),
            Err(e) => self.toast_error(format!("Could not save diagnostics: {e}")),
        }
    }

    /// Save the cue sheet (`pdf` or `csv`) through the system dialog.
    pub fn export_cue_sheet(&mut self, pdf: bool) {
        let show = &self.client.state.show;
        let stem: String = show
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stem = if stem.trim().is_empty() {
            "show".to_string()
        } else {
            stem.trim().to_string()
        };
        let (ext, label) = if pdf {
            ("pdf", "PDF")
        } else {
            ("csv", "CSV (spreadsheet)")
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export cue sheet")
            .add_filter(label, &[ext])
            .set_file_name(format!("{stem} - cue sheet.{ext}"))
            .save_file()
        else {
            return;
        };
        let bytes = if pdf {
            let date = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            sm_core::cuesheet::to_pdf(show, &date)
        } else {
            sm_core::cuesheet::to_csv(show).into_bytes()
        };
        match std::fs::write(&path, bytes) {
            Ok(()) => self.toast(format!("Cue sheet saved to {}", path.display())),
            Err(e) => self.toast_error(format!("Could not save the cue sheet: {e}")),
        }
    }

    /// Show the folder with the log file in the system file manager.
    pub fn open_log_folder(&mut self) {
        let Some(dir) = self
            .client
            .state
            .log_file
            .as_deref()
            .and_then(|f| Path::new(f).parent())
            .map(Path::to_path_buf)
        else {
            self.toast_error("The log file location is not known.");
            return;
        };
        let opener = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(opener).arg(&dir).spawn() {
            self.toast_error(format!("Could not open {}: {e}", dir.display()));
        }
    }

    /// Run `work` on a background thread; its result is handled in `logic`.
    fn spawn_job(&mut self, what: &str, work: impl FnOnce() -> JobDone + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("sm-job".into())
            .spawn(move || {
                let _ = tx.send(work());
            }) {
            Ok(_) => {
                self.jobs.push(rx);
                self.toast(format!("{what}..."));
            }
            Err(e) => self.toast_error(format!("{what} failed: {e}")),
        }
    }

    fn poll_jobs(&mut self) {
        let mut done = Vec::new();
        self.jobs.retain(|rx| match rx.try_recv() {
            Ok(r) => {
                done.push(r);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        for r in done {
            match r {
                JobDone::Toast(t) => self.toast(t),
                JobDone::Error(e) => self.toast_error(e),
                JobDone::Open(p) => self.open_show(p),
            }
        }
    }

    /// Save the show and all its audio as one `.smpack` file.
    pub fn export_package(&mut self) {
        let show = self.client.state.show.clone();
        let dir = self
            .client
            .state
            .show_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .map(Path::to_path_buf);
        let name = if show.name.trim().is_empty() {
            "Show".to_string()
        } else {
            show.name.trim().to_string()
        };
        let Some(out) = rfd::FileDialog::new()
            .set_title("Export show package")
            .add_filter("Show Master package", &[sm_core::package::EXTENSION])
            .set_file_name(format!("{name}.{}", sm_core::package::EXTENSION))
            .save_file()
        else {
            return;
        };
        self.spawn_job("Packing the show", move || {
            match sm_core::package::export(&show, dir.as_deref(), &out) {
                Ok(r) if r.missing.is_empty() => JobDone::Toast(format!(
                    "Show package saved: {} audio files, {:.1} MB",
                    r.files,
                    r.bytes as f64 / 1e6
                )),
                Ok(r) => JobDone::Error(format!(
                    "Package saved, but {} file(s) were missing: {}",
                    r.missing.len(),
                    r.missing.join(", ")
                )),
                Err(e) => JobDone::Error(format!("Could not save the package: {e}")),
            }
        });
    }

    /// Unpack a `.smpack` into a folder and open the show.
    pub fn import_package(&mut self, package: Option<PathBuf>) {
        let package = match package {
            Some(p) => p,
            None => {
                let Some(p) = rfd::FileDialog::new()
                    .set_title("Open show package")
                    .add_filter("Show Master package", &[sm_core::package::EXTENSION])
                    .pick_file()
                else {
                    return;
                };
                p
            }
        };
        let Some(dest) = rfd::FileDialog::new()
            .set_title("Unpack the show into which folder?")
            .pick_folder()
        else {
            return;
        };
        self.spawn_job(
            "Unpacking the show",
            move || match sm_core::package::import(&package, &dest) {
                Ok(show) => JobDone::Open(show),
                Err(e) => JobDone::Error(format!("Could not open the package: {e}")),
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
        if let Some(pkg) = files.iter().find(|p| ext(p) == sm_core::package::EXTENSION) {
            self.import_package(Some(pkg.clone()));
            return;
        }
        let audio: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| AUDIO_EXTENSIONS.contains(&ext(p).as_str()))
            .collect();
        if audio.is_empty() {
            self.toast_error("Only audio files, .smshow and .smpack files can be dropped here.");
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
        let typing = ctx.egui_wants_keyboard_input() || self.was_typing;
        let live = self.is_live();
        let delete_action = "delete".to_string();
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
                // Delete and Backspace (the Mac "delete" key) always delete cues in
                // Edit mode, even if the show's hotkey list does not name them.
                let builtin =
                    (*key == Key::Delete || *key == Key::Backspace) && modifiers.is_none() && !live;
                let Some(action) =
                    map.get(&combo)
                        .or(if builtin { Some(&delete_action) } else { None })
                else {
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
        // Copy / cut / paste of cues (Ctrl/Cmd + C, X, V), Edit mode, not while typing.
        if !typing && !live {
            let mut clip_events = Vec::new();
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Copy => {
                        clip_events.push(("copy", None));
                        false
                    }
                    egui::Event::Cut => {
                        clip_events.push(("cut", None));
                        false
                    }
                    egui::Event::Paste(t) => {
                        clip_events.push(("paste", Some(t.clone())));
                        false
                    }
                    _ => true,
                });
            });
            for (what, text) in clip_events {
                match (what, text) {
                    ("paste", Some(t)) => self.paste_cues(&t),
                    (a, _) => self.run_action(a),
                }
            }
        }
    }

    /// Copy the selected cues (to this window and the system clipboard).
    pub fn copy_selection(&mut self, cut: bool) {
        let dir = self
            .client
            .state
            .show_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .map(Path::to_path_buf);
        let ids = self.view.selection.clone();
        let Some(text) =
            crate::clipboard::copy_payload(&self.client.state.show, dir.as_deref(), &ids)
        else {
            return;
        };
        self.clip = Some(text.clone());
        self.to_clipboard = Some(text);
        let n = ids.len();
        if cut {
            self.cmd(json!({ "cmd": "delete_cues", "cues": ids }));
            self.view.selection.clear();
            self.toast(format!("Cut {n} cue{}", if n == 1 { "" } else { "s" }));
        } else {
            self.toast(format!(
                "Copied {n} cue{} - paste in any show",
                if n == 1 { "" } else { "s" }
            ));
        }
    }

    /// Paste cues from clipboard / template text after the selection.
    pub fn paste_cues(&mut self, text: &str) {
        let list = self.current_list();
        let after = self.view.selection.last().copied();
        match crate::clipboard::paste_command(text, list, after) {
            Some(cmd) => self.cmd_then(cmd, |app, r| match r {
                Ok(v) => {
                    if let Some(ids) = v["cues"].as_array() {
                        app.view.selection = ids.iter().filter_map(Value::as_u64).collect();
                        app.edit.table.scroll_to = app.view.selection.first().copied();
                    }
                }
                Err(e) => app.toast_error(App::error_text(&e)),
            }),
            None => self.toast_error("The clipboard holds no Show Master cues."),
        }
    }

    pub fn run_action(&mut self, action: &str) {
        let editing = !self.is_live();
        match action {
            "copy" if editing => self.copy_selection(false),
            "cut" if editing => self.copy_selection(true),
            "paste" if editing => {
                if let Some(t) = self.clip.clone() {
                    self.paste_cues(&t);
                }
            }
            "save_template" if editing && !self.view.selection.is_empty() => {
                let name = self
                    .view
                    .selection
                    .first()
                    .and_then(|id| self.client.state.show.cue(*id))
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.modal = Some(Modal::SaveTemplate(name));
            }
            "go" => self.go(),
            "panic" => {
                self.panic();
            }
            "stop_all" => self.stop_all(),
            "toggle_pause" => self.toggle_pause(),
            "next" | "previous" => {
                self.step_playhead(action == "next");
                // In Edit mode the selection (and inspector) follows the playhead.
                self.select_playhead = editing;
            }
            "reset" => self.reset(),
            "preview" => {
                if let Some(cue) = self.view.selection.last().copied() {
                    self.cmd(json!({ "cmd": "start_cue", "cue": cue }));
                }
            }
            "toggle_mode" if self.kiosk_locked() => {}
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
                let n = cues.len();
                let undo = if cfg!(target_os = "macos") {
                    "Cmd+Z"
                } else {
                    "Ctrl+Z"
                };
                self.cmd_then(
                    json!({ "cmd": "delete_cues", "cues": cues }),
                    move |app, r| match r {
                        Ok(_) => app.toast(format!(
                            "Deleted {n} cue{} - undo with {undo}",
                            if n == 1 { "" } else { "s" }
                        )),
                        Err(e) => app.toast_error(App::error_text(&e)),
                    },
                );
            }
            "find" => self.focus_goto = true,
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Frame
    // ------------------------------------------------------------------

    /// Touch-friendly controls are on (setting, or "Auto" and a touch was seen).
    pub fn touch_mode(&self) -> bool {
        self.prefs.touch.resolve(self.touch_seen)
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if !self.touch_seen && ctx.input(|i| i.has_touch_screen()) {
            self.touch_seen = true;
        }
        let touch = self.touch_mode();
        let key = (self.client.state.show.settings.high_contrast, touch);
        if self.style_key != Some(key) {
            ctx.set_theme(egui::Theme::Dark);
            theme::apply(ctx, key.0, key.1);
            self.style_key = Some(key);
        }
        self.screen = crate::responsive::Screen::new(ctx.content_rect().size(), touch);
        crate::responsive::store(ctx, self.screen);
    }

    /// Everything that is not drawing: state, replies, signals, input.
    pub fn logic(&mut self, ctx: &egui::Context) {
        let changed = self.client.refresh();
        self.handle_replies();
        if changed {
            let path = self.client.state.show_path.clone();
            if path != self.last_show_path {
                if let Some(p) = &path {
                    self.recent.add(Path::new(p));
                }
                self.last_show_path = path;
            }
            if self.select_playhead {
                self.select_playhead = false;
                if let Some(cue) = self
                    .current_list()
                    .and_then(|l| self.client.state.playhead(l))
                {
                    self.view.selection = vec![cue];
                }
            }
        }
        self.apply_theme(ctx);
        let handle = self.client.handle.clone();
        // Second start of the program, tray icon, Dock icon: show the window.
        if handle.raise.swap(false, Ordering::SeqCst) | crate::background::take_show() {
            self.show_window(ctx);
        }
        if crate::background::take_hide() {
            self.hide_window(ctx);
        }
        if crate::background::take_quit() {
            self.show_window(ctx);
            self.request_quit(ctx);
        }
        if handle.quit_requested.load(Ordering::SeqCst) && !self.allow_close {
            // Quit button, remote quit or an installed update: close without asking.
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if let Some(path) = self.open_at_start.take() {
            self.open_show(path);
        }
        // Kiosk: go live once the show is there (no pre-flight dialog to answer).
        if self.prefs.kiosk && !self.kiosk_started && !self.client.state.version.is_empty() {
            self.kiosk_started = true;
            if !self.is_live() {
                self.cmd(json!({ "cmd": "set_mode", "mode": "live" }));
            }
        }
        // Files the OS asked us to open (double-click, Open with, Dock).
        let requested = crate::open_files::take();
        if !requested.is_empty() {
            let ext = |p: &Path| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default()
            };
            if let Some(show) = requested.iter().find(|p| ext(p) == SHOW_EXTENSION) {
                self.open_show(show.clone());
            } else if let Some(pkg) = requested
                .iter()
                .find(|p| ext(p) == sm_core::package::EXTENSION)
            {
                self.import_package(Some(pkg.clone()));
            } else {
                let audio: Vec<PathBuf> = requested
                    .into_iter()
                    .filter(|p| AUDIO_EXTENSIONS.contains(&ext(p).as_str()))
                    .collect();
                if !audio.is_empty() {
                    self.add_audio_cues(audio);
                }
            }
        }
        if self.client.state.recovery_available && !self.recovery_asked && self.modal.is_none() {
            self.recovery_asked = true;
            self.modal = Some(Modal::Recovery);
        }
        self.handle_close_request(ctx);
        self.poll_jobs();
        self.handle_dropped_files(ctx);
        crate::setup::capture_keys(self, ctx);
        let hidden = ctx.input(|i| {
            let v = i.viewport();
            v.visible() == Some(false) || v.minimized == Some(true)
        });
        crate::global_hotkeys::fast::update(&self.client.handle, hidden, self.current_list());
        crate::global_hotkeys::process(self, ctx);
        self.hotkeys(ctx);
        self.toasts.retain(|t| t.until > Instant::now());
        // Clocks, meters and running cues move continuously; when nothing
        // happens a few frames per second are enough (saves CPU and battery).
        let t = &self.client.tick;
        let busy = !t.active.is_empty()
            || t.meters.peak.iter().any(|p| *p > 0.000_1)
            || matches!(
                self.client.state.update.state.as_str(),
                "downloading" | "installing"
            )
            || self.panic_armed();
        ctx.request_repaint_after(Duration::from_millis(if busy { 40 } else { 250 }));
    }

    /// The window's close button only hides the window: the show, the tablet
    /// server and everything else keep running (see [`crate::background`]).
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.allow_close {
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        if self.prefs.background_hint_seen {
            self.hide_window(ctx);
        } else {
            self.modal = Some(Modal::RunsInBackground);
        }
    }

    /// Bring the (hidden or minimized) window to the front.
    pub fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    /// Hide the window and keep running; minimize where a hidden window
    /// could not be found again (no tray icon, Wayland).
    pub fn hide_window(&mut self, ctx: &egui::Context) {
        if crate::background::can_hide() {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        } else {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    }

    /// The short address for tablets (`showmaster.local`), if known.
    pub fn tablet_address(&self) -> Option<String> {
        let r = &self.client.state.remote;
        r.mdns
            .as_ref()
            .filter(|m| m.enabled)
            .and_then(|m| m.url.as_deref())
            .map(|u| crate::setup::remote::short_address(u).to_string())
            .or_else(|| r.urls.first().map(|u| format!("{u}/view")))
    }

    /// Kiosk mode is on and locked: no editing, Setup or Quit.
    pub fn kiosk_locked(&self) -> bool {
        self.prefs.kiosk && !self.kiosk_unlocked
    }

    /// Really quit Show Master (Quit button, Cmd+Q, tray menu): asks first
    /// when cues are running, in Live mode with quit protection, or with
    /// unsaved changes.
    pub fn request_quit(&mut self, ctx: &egui::Context) {
        if self.allow_close {
            return;
        }
        if self.kiosk_locked() {
            self.modal = Some(Modal::Unlock(String::new(), true));
            return;
        }
        let s = &self.client.state;
        let running = !self.client.tick.active.is_empty();
        let locked = s.mode == Mode::Live && s.show.settings.quit_lock;
        if running || locked {
            self.modal = Some(Modal::CloseWhileRunning);
        } else if s.dirty {
            self.modal = Some(Modal::Unsaved(Pending::Close));
        } else {
            self.close_now(ctx);
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
        if let Some(t) = self.to_clipboard.take() {
            ctx.copy_text(t);
        }
        self.modals(&ctx);
        self.draw_toasts(&ctx);
        self.was_typing = ctx.egui_wants_keyboard_input();
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

    /// File menu entries (also the folded "Menu" on narrow screens).
    fn file_menu_items(&mut self, ui: &mut egui::Ui) {
        if ui.button("New show").clicked() {
            self.new_show();
        }
        if ui.button("Open…").clicked() {
            self.open_show_dialog();
        }
        let recent: Vec<PathBuf> = self.recent.existing().cloned().collect();
        ui.add_enabled_ui(!recent.is_empty(), |ui| {
            ui.menu_button("Open recent", |ui| {
                for p in &recent {
                    let name = p
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("show")
                        .to_string();
                    if ui
                        .button(name)
                        .on_hover_text(p.display().to_string())
                        .clicked()
                    {
                        self.open_show(p.clone());
                    }
                }
                ui.separator();
                if ui.button("Clear list").clicked() {
                    self.recent.clear();
                }
            });
        });
        ui.separator();
        if ui.button("Save").clicked() {
            self.save(None);
        }
        if ui.button("Save as…").clicked() {
            self.save_as(None);
        }
        ui.separator();
        if ui
            .button("Export show package…")
            .on_hover_text("The show and all its audio in one file, to move it to another computer")
            .clicked()
        {
            self.export_package();
        }
        if ui
            .button("Open show package…")
            .on_hover_text("Unpack a .smpack file into a folder and open it")
            .clicked()
        {
            self.import_package(None);
        }
        ui.separator();
        if ui
            .button("Export cue sheet (PDF)…")
            .on_hover_text("Printable list of all cues with notes, for stage management and crew")
            .clicked()
        {
            self.export_cue_sheet(true);
        }
        if ui
            .button("Export cue sheet (CSV)…")
            .on_hover_text("All cues as a spreadsheet (Excel, Numbers, LibreOffice)")
            .clicked()
        {
            self.export_cue_sheet(false);
        }
        ui.separator();
        if ui
            .button("Export diagnostics…")
            .on_hover_text("Log, system and show state in one text file, for support")
            .clicked()
        {
            self.export_diagnostics();
        }
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
    }

    /// Width of a button with `text` (for fitting toolbars).
    pub fn button_width(ui: &egui::Ui, text: &str) -> f32 {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let w = ui
            .painter()
            .layout_no_wrap(text.to_string(), font, Color32::WHITE)
            .size()
            .x;
        w + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let live = self.is_live();
        let screen = self.screen;
        // Narrow screens (or compact + touch): File, Save, Undo and Redo fold
        // into one "Menu"; the "Go to cue" caption is dropped.
        let fold = screen.fold_toolbar();
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
            if self.kiosk_locked() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Unlock")
                        .on_hover_text("Kiosk mode: unlock editing, Setup and Quit")
                        .clicked()
                    {
                        self.modal = Some(Modal::Unlock(String::new(), false));
                    }
                    ui.label(RichText::new("KIOSK").small().color(theme::MUTED));
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.client.state.show.name)
                                    .strong()
                                    .size(17.0),
                            )
                            .truncate(),
                        );
                    });
                });
                return;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.prefs.kiosk
                    && ui
                        .button("Lock")
                        .on_hover_text("Back to kiosk mode")
                        .clicked()
                {
                    self.kiosk_unlocked = false;
                }
                if ui
                    .button("Quit")
                    .on_hover_text(
                        "Quit Show Master: stops the show and the tablet remote. \
                         Closing the window only hides it.",
                    )
                    .clicked()
                {
                    self.request_quit(ui.ctx());
                }
                if !live && ui.button("Setup").clicked() {
                    self.setup.open = true;
                }
                self.mode_switch(ui);
                self.update_button(ui);
                let goto = ui.add(
                    egui::TextEdit::singleline(&mut self.goto_text)
                        .hint_text(if screen.compact() { "Cue #" } else { "#" })
                        .desired_width(match (screen.touch, screen.compact()) {
                            (true, _) => 76.0,
                            (false, true) => 60.0,
                            (false, false) => 56.0,
                        })
                        .id(Id::unique("goto_box")),
                );
                ui.ctx()
                    .accesskit_node_builder(goto.id, |b| b.set_label("Go to cue number"));
                if self.focus_goto {
                    goto.request_focus();
                    self.focus_goto = false;
                }
                if goto.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let n = std::mem::take(&mut self.goto_text);
                    self.goto_number(&n);
                }
                if !screen.compact() {
                    ui.label("Go to cue");
                }
                // The rest, left to right: show title (truncated) and file buttons.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let tools: f32 = if live {
                        0.0
                    } else if fold {
                        Self::button_width(ui, "Menu") + 12.0
                    } else {
                        ["File", "Save", "Undo", "Redo"]
                            .iter()
                            .map(|t| Self::button_width(ui, t))
                            .sum::<f32>()
                            + 12.0
                    };
                    let title_w = (ui.available_width() - tools).max(40.0);
                    let s = &self.client.state;
                    let title = if s.dirty {
                        format!("{} *", s.show.name)
                    } else {
                        s.show.name.clone()
                    };
                    let path = s
                        .show_path
                        .clone()
                        .unwrap_or_else(|| "Not saved yet".into());
                    let t = ui
                        .scope(|ui| {
                            ui.set_max_width(title_w);
                            ui.add(
                                egui::Label::new(RichText::new(title).strong().size(17.0))
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            )
                        })
                        .inner;
                    // Tap alternative to the hover text (touch screens have no hover).
                    if t.on_hover_text(&path).clicked() {
                        self.toast(path);
                    }
                    if live {
                        return;
                    }
                    ui.separator();
                    let (can_undo, can_redo) =
                        (self.client.state.can_undo, self.client.state.can_redo);
                    if fold {
                        ui.menu_button("Menu", |ui| {
                            if ui.button("Save").clicked() {
                                self.save(None);
                            }
                            if ui
                                .add_enabled(can_undo, egui::Button::new("Undo"))
                                .clicked()
                            {
                                self.cmd(json!({ "cmd": "undo" }));
                            }
                            if ui
                                .add_enabled(can_redo, egui::Button::new("Redo"))
                                .clicked()
                            {
                                self.cmd(json!({ "cmd": "redo" }));
                            }
                            ui.separator();
                            self.file_menu_items(ui);
                        })
                        .response
                        .on_hover_text("File, save, undo and redo");
                        return;
                    }
                    ui.menu_button("File", |ui| self.file_menu_items(ui));
                    if ui.button("Save").clicked() {
                        self.save(None);
                    }
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
                });
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

    /// Status bar items: (text, colour, priority, hint). Lower priority
    /// numbers are kept longest when the bar is too narrow.
    fn status_items(&self) -> Vec<(String, Option<Color32>, u8, Option<&'static str>)> {
        let s = &self.client.state;
        let t = &self.client.tick;
        let mut items = Vec::new();
        if let Some(err) = &s.audio.error {
            items.push((format!("Audio: {err}"), Some(theme::ERROR), 0, None));
        } else if s.audio.is_null {
            items.push((
                "No audio device (silent)".to_string(),
                Some(theme::WARN),
                0,
                None,
            ));
        }
        items.push((
            format!("{} · {}", s.audio.backend, s.audio.device),
            None,
            3,
            None,
        ));
        if let Some(l) = s.remote.link.as_ref().filter(|l| l.role != "off") {
            let (text, color) = match (l.role.as_str(), l.connected) {
                ("main", true) => ("Backup: in sync".to_string(), theme::NEXT),
                ("main", false) => ("Backup: NOT REACHABLE".to_string(), theme::ERROR),
                (_, true) => ("BACKUP - following the main".to_string(), theme::NEXT),
                (_, false) => (
                    "BACKUP - main not heard - running on its own".to_string(),
                    theme::ERROR,
                ),
            };
            items.push((
                text,
                Some(color),
                0,
                Some("Main / backup computer link: Setup > Remote > Backup computer"),
            ));
        }
        items.push((format!("{} Hz", s.audio.sample_rate), None, 4, None));
        if let Some(b) = s.audio.buffer_frames {
            items.push((format!("{b} frames"), None, 5, None));
        }
        items.push((format!("{} out", s.audio.channels), None, 5, None));
        items.push((format!("CPU {:.0}%", t.cpu * 100.0), None, 1, None));
        items.push((
            format!("Underruns {}", t.underruns),
            (t.underruns > 0).then_some(theme::ERROR),
            1,
            None,
        ));
        items.push((format!("Voices {}", t.voices), None, 4, None));
        items.push((format!("MIDI ports {}", s.midi.ports.len()), None, 6, None));
        if s.midi.rtp.enabled {
            let up = s
                .midi
                .rtp
                .peers
                .iter()
                .filter(|p| p.state == "connected")
                .count();
            items.push((
                format!("RTP-MIDI {up}/{}", s.midi.rtp.peers.len()),
                None,
                6,
                None,
            ));
        }
        items.push((format!("Remotes {}", s.remote.clients.len()), None, 6, None));
        let by_name = s
            .remote
            .mdns
            .as_ref()
            .filter(|m| m.enabled)
            .and_then(|m| m.url.as_deref())
            .map(|u| crate::setup::remote::short_address(u).to_string());
        if let Some(addr) = by_name.or_else(|| s.remote.urls.first().map(|u| format!("{u}/remote")))
        {
            items.push((
                addr,
                None,
                7,
                Some("Open this address on a tablet in the same network (Setup > Remote shows a QR code)"),
            ));
        }
        items
    }

    /// One line. Items that do not fit (lowest priority first) move into a
    /// "Details" popup that opens on click / tap; the audio error is shortened.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let items = self.status_items();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let small = |t: &str| RichText::new(t).small();
            let font = egui::TextStyle::Small.resolve(ui.style());
            let gap = ui.spacing().item_spacing.x;
            let widths: Vec<f32> = items
                .iter()
                .map(|(t, ..)| {
                    ui.painter()
                        .layout_no_wrap(t.clone(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                        + gap
                })
                .collect();
            let avail = ui.available_width();
            let total: f32 = widths.iter().sum();
            let mut shown = vec![true; items.len()];
            let mut error_w = None;
            if total > avail {
                let details_w = Self::button_width(ui, "Details") + 4.0;
                let mut budget = avail - details_w;
                // The audio problem stays, shortened to at most 40 % of the bar.
                if items.first().is_some_and(|i| i.2 == 0) {
                    let w = widths[0].min(avail * 0.4);
                    error_w = Some(w - gap);
                    budget -= w;
                }
                let mut order: Vec<usize> = (0..items.len()).filter(|i| items[*i].2 > 0).collect();
                order.sort_by_key(|i| items[*i].2);
                shown = vec![false; items.len()];
                if error_w.is_some() {
                    shown[0] = true;
                }
                for i in order {
                    if widths[i] <= budget {
                        budget -= widths[i];
                        shown[i] = true;
                    } else {
                        // Keep the display order stable: stop at the first misfit.
                        break;
                    }
                }
            }
            for (i, (text, color, prio, hint)) in items.iter().enumerate() {
                if !shown[i] {
                    continue;
                }
                let mut rt = small(text);
                if let Some(c) = color {
                    rt = rt.color(*c);
                }
                let resp = if *prio == 0 && error_w.is_some() {
                    ui.scope(|ui| {
                        ui.set_max_width(error_w.unwrap_or(200.0));
                        ui.add(egui::Label::new(rt).truncate())
                    })
                    .inner
                } else {
                    ui.label(rt)
                };
                if *prio == 0 {
                    resp.on_hover_text(text.as_str());
                } else if let Some(h) = hint {
                    resp.on_hover_text(*h);
                }
            }
            if shown.iter().any(|s| !s) || error_w.is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button(small("Details"), |ui| {
                        ui.set_max_width(self.screen.dialog_width(420.0));
                        for (text, color, _, hint) in &items {
                            let mut rt = RichText::new(text);
                            if let Some(c) = color {
                                rt = rt.color(*c);
                            }
                            ui.add(egui::Label::new(rt).wrap());
                            if let Some(h) = hint {
                                ui.label(RichText::new(*h).small().color(theme::MUTED));
                            }
                        }
                    })
                    .response
                    .on_hover_text("All audio, MIDI and network status");
                });
            }
        });
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let max_w = self.screen.dialog_width(380.0);
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
                            ui.set_max_width(max_w);
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
        let max_w = self.screen.dialog_width(520.0);
        let max_h = (self.screen.height * 0.4).clamp(120.0, 320.0);
        let resp = egui::Modal::new(Id::unique("app_modal")).show(ctx, |ui| {
            ui.set_max_width(max_w);
            match &modal {
                Modal::Preflight(issues) => {
                    ui.heading("Pre-flight check");
                    ui.label("Before going live, please look at these points:");
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
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
                        egui::ScrollArea::vertical().max_height(max_h.min(220.0)).show(ui, |ui| {
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
                Modal::RunsInBackground => {
                    ui.heading("Show Master keeps running");
                    ui.label("Closing the window does not stop the show: audio, timecode, MIDI and the tablet remote keep running in the background.");
                    ui.add_space(4.0);
                    ui.label(if crate::background::can_hide() {
                        if cfg!(target_os = "macos") {
                            "Open the window again from the Dock, by starting Show Master again or from the menu bar icon. Quit with Quit (Cmd+Q) or from the menu bar icon."
                        } else {
                            "Open the window again by starting Show Master again or by clicking the tray icon. Quit with Quit or from the tray icon."
                        }
                    } else {
                        "There is no tray icon on this desktop, so the window is minimized instead. Quit with the Quit button."
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.button("Quit Show Master").clicked() {
                            keep = false;
                            self.request_quit(ui.ctx());
                        }
                        if ui.add(egui::Button::new(RichText::new("Keep running").strong())).clicked() {
                            keep = false;
                            self.prefs.set_background_hint_seen();
                            self.hide_window(ui.ctx());
                        }
                    });
                }
                Modal::Unlock(typed, quit) => {
                    ui.heading("Unlock");
                    let has_pin = !self.prefs.kiosk_pin.is_empty();
                    ui.label(if has_pin {
                        "Enter the kiosk PIN to unlock editing, Setup and Quit."
                    } else {
                        "Unlock editing, Setup and Quit on this computer?"
                    });
                    let mut text = typed.clone();
                    let mut enter = false;
                    if has_pin {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .password(true)
                                .hint_text("PIN")
                                .desired_width(160.0),
                        );
                        ui.ctx().accesskit_node_builder(resp.id, |b| b.set_label("Kiosk PIN"));
                        enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if !enter && !resp.has_focus() && typed.is_empty() {
                            resp.request_focus();
                        }
                        if text != *typed {
                            next = Some(Modal::Unlock(text.clone(), *quit));
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                            next = None;
                        }
                        if ui.button("Unlock").clicked() || enter {
                            if !has_pin || text == self.prefs.kiosk_pin {
                                keep = false;
                                next = None;
                                self.kiosk_unlocked = true;
                                if *quit {
                                    self.request_quit(ui.ctx());
                                }
                            } else {
                                next = Some(Modal::Unlock(String::new(), *quit));
                                self.toast_error("Wrong PIN");
                            }
                        }
                    });
                }
                Modal::SaveTemplate(name) => {
                    ui.heading("Save as template");
                    ui.label("The selected cues can then be inserted into any show (Add cue > Templates).");
                    let mut text = name.clone();
                    let resp = ui.add(egui::TextEdit::singleline(&mut text).hint_text("Template name").desired_width(280.0));
                    if text != *name {
                        next = Some(Modal::SaveTemplate(text.clone()));
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                        if ui.add_enabled(!text.trim().is_empty(), egui::Button::new("Save")).clicked() || (enter && !text.trim().is_empty()) {
                            keep = false;
                            next = None;
                            let dir = self.client.state.show_path.as_deref().and_then(|p| Path::new(p).parent()).map(Path::to_path_buf);
                            let ids = self.view.selection.clone();
                            match crate::clipboard::copy_payload(&self.client.state.show, dir.as_deref(), &ids)
                                .ok_or_else(|| "nothing selected".to_string())
                                .and_then(|p| crate::clipboard::save_template(&text, &p))
                            {
                                Ok(_) => self.toast(format!("Template \"{}\" saved", text.trim())),
                                Err(e) => self.toast_error(format!("Could not save the template: {e}")),
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
