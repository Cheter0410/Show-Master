//! The show runtime: one thread that owns the document, the cue engine and
//! all I/O. Every action in the system - hotkey, desktop UI, iPad, Stream
//! Deck, OSC - arrives here as a command on one bus, and every state change
//! leaves as a published state diff, tick or log entry.

use crate::diff::diff;
use crate::doc::{Document, EditCommand, MediaInfo, MediaProbe};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sm_core::engine::{Command, Engine, Event, EventKind};
use sm_core::model::{Show, LAYOUTS};
use sm_core::preflight::{self, Issue};
use sm_core::showfile;
use sm_io::Io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, oneshot};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    View,
    Control,
    Admin,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Edit,
    Live,
}

/// Application-level commands.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum SysCommand {
    SetMode {
        mode: Mode,
        #[serde(default)]
        force: bool,
    },
    RunPreflight,
    NewShow {
        #[serde(default)]
        force: bool,
    },
    OpenShow {
        path: String,
        #[serde(default)]
        force: bool,
    },
    SaveShow {
        #[serde(default)]
        path: Option<String>,
    },
    CollectMedia,
    Recover {
        accept: bool,
    },
    KickClient {
        id: u64,
    },
    RtpConnect {
        addr: String,
    },
    ClearLog,
    SetNotes {
        text: String,
    },
    ResetClip,
    /// Ask the update server for a newer version now.
    CheckUpdate,
    /// Download and install the available update, then restart.
    InstallUpdate,
    /// Bring the desktop window to the front (sent by a second program start).
    RaiseWindow,
    /// Shut the application down.
    Quit {
        #[serde(default)]
        force: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnyCommand {
    Engine(Command),
    Edit(EditCommand),
    Sys(SysCommand),
}

/// Parse a command document from any transport (WS, HTTP, OSC, hotkey).
pub fn parse_command(v: Value) -> Result<AnyCommand, String> {
    let name = v
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or("missing field 'cmd'")?
        .to_string();
    fn unknown(e: &serde_json::Error) -> bool {
        e.to_string().starts_with("unknown variant")
    }
    match serde_json::from_value::<Command>(v.clone()) {
        Ok(c) => return Ok(AnyCommand::Engine(c)),
        Err(e) if !unknown(&e) => return Err(format!("{name}: {e}")),
        Err(_) => {}
    }
    match serde_json::from_value::<EditCommand>(v.clone()) {
        Ok(c) => return Ok(AnyCommand::Edit(c)),
        Err(e) if !unknown(&e) => return Err(format!("{name}: {e}")),
        Err(_) => {}
    }
    match serde_json::from_value::<SysCommand>(v) {
        Ok(c) => Ok(AnyCommand::Sys(c)),
        Err(e) if !unknown(&e) => Err(format!("{name}: {e}")),
        Err(_) => Err(format!("unknown command '{name}'")),
    }
}

impl AnyCommand {
    /// Minimum role needed. Enforced here, on the server, for every transport.
    pub fn required_role(&self) -> Role {
        match self {
            AnyCommand::Engine(_) => Role::Control,
            AnyCommand::Sys(SysCommand::SetNotes { .. } | SysCommand::ResetClip) => Role::Control,
            AnyCommand::Edit(_) | AnyCommand::Sys(_) => Role::Admin,
        }
    }
}

pub struct Request {
    pub command: AnyCommand,
    pub role: Role,
    pub origin: String,
    pub reply: Option<oneshot::Sender<Result<Value, String>>>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ClientInfo {
    pub id: u64,
    pub role: Role,
    pub addr: String,
    pub since_ms: u64,
    pub agent: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct LogEntry {
    pub time_ms: u64,
    pub level: &'static str,
    pub message: String,
}

/// Messages fanned out to all connected clients.
#[derive(Clone, Debug)]
pub enum Outbound {
    /// Pre-serialised `patch` message and its revision; `public` has the
    /// remote PINs removed and is what non-admin clients receive.
    Patch {
        rev: u64,
        admin: Arc<str>,
        public: Arc<str>,
    },
    Tick(Arc<str>),
    Log(Arc<str>),
    Kick(u64),
}

#[derive(Default)]
pub struct Published {
    pub rev: u64,
    pub state: Value,
    pub tick: Value,
    pub log_tail: Vec<LogEntry>,
}

/// Handle used by the server (and tests) to talk to the runtime.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Request>,
    pub published: Arc<RwLock<Published>>,
    pub events: broadcast::Sender<Outbound>,
    pub clients: Arc<Mutex<Vec<ClientInfo>>>,
    /// Signalled when the application should exit (Quit command, update installed).
    pub quit: Arc<tokio::sync::Notify>,
    /// Set together with `quit`; polled by the desktop window.
    pub quit_requested: Arc<std::sync::atomic::AtomicBool>,
    /// Set when another program start asks the window to come to the front.
    pub raise: Arc<std::sync::atomic::AtomicBool>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Handle {
    /// Send a command and wait for its result.
    pub async fn command(
        &self,
        command: AnyCommand,
        role: Role,
        origin: &str,
    ) -> Result<Value, String> {
        if role < command.required_role() {
            return Err("forbidden".into());
        }
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request {
                command,
                role,
                origin: origin.to_string(),
                reply: Some(reply),
            })
            .map_err(|_| "runtime stopped".to_string())?;
        rx.await.map_err(|_| "runtime stopped".to_string())?
    }

    /// Fire-and-forget (OSC, hotkeys from non-async contexts).
    pub fn post(&self, command: AnyCommand, role: Role, origin: &str) -> Result<(), String> {
        if role < command.required_role() {
            return Err("forbidden".into());
        }
        self.tx
            .send(Request {
                command,
                role,
                origin: origin.to_string(),
                reply: None,
            })
            .map_err(|_| "runtime stopped".to_string())
    }

    /// Ask the whole application to exit (window, server, engine).
    pub fn request_quit(&self) {
        self.quit_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.quit.notify_one();
    }

    /// Stop the runtime thread (it autosaves on the way out). Join it afterwards.
    pub fn shutdown(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn state(&self) -> Value {
        self.published
            .read()
            .map(|p| p.state.clone())
            .unwrap_or(Value::Null)
    }

    pub fn tick(&self) -> Value {
        self.published
            .read()
            .map(|p| p.tick.clone())
            .unwrap_or(Value::Null)
    }
}

#[derive(Default)]
pub struct Config {
    pub show_path: Option<PathBuf>,
    /// Port override; `None` = the show's `settings.remote.port`.
    pub port: Option<u16>,
    /// MIDI backend override (tests).
    pub midi_backend: Option<Box<dyn sm_io::midi::MidiBackend>>,
    /// Use this show instead of loading `show_path` (tests, recovery).
    pub show: Option<Show>,
    /// Look for a newer version at start-up (the only internet access there is).
    pub update_check: bool,
}

struct Probe;
impl MediaProbe for Probe {
    fn probe(&self, path: &Path) -> Option<MediaInfo> {
        let info = sm_audio::probe(path).ok()?;
        Some(MediaInfo {
            duration: info.duration_seconds,
            channels: info.channels as u16,
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const PIN_PARENT: [&str; 3] = ["show", "settings", "remote"];
const PIN_KEYS: [&str; 2] = ["control_pin", "view_pin"];

/// Blank the remote PINs inside `value`, which sits at `path` in the state document.
fn redact_at(path: &[Value], value: &mut Value) {
    if path.len() > PIN_PARENT.len()
        || !path
            .iter()
            .zip(PIN_PARENT)
            .all(|(a, b)| a.as_str() == Some(b))
    {
        return;
    }
    let mut cur = value;
    for key in PIN_PARENT.iter().skip(path.len()) {
        match cur.get_mut(*key) {
            Some(next) => cur = next,
            None => return,
        }
    }
    for k in PIN_KEYS {
        if let Some(pin) = cur.get_mut(k) {
            // Clients only need to know whether a PIN is set.
            *pin = Value::from(if pin.as_str().is_some_and(|s| !s.is_empty()) {
                "****"
            } else {
                ""
            });
        }
    }
}

/// The state document as non-admin clients may see it.
pub fn redact_state(mut state: Value) -> Value {
    redact_at(&[], &mut state);
    state
}

fn redact_ops(ops: Vec<Value>) -> Vec<Value> {
    ops.into_iter()
        .map(|mut op| {
            let path: Vec<Value> = op
                .get("p")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let is_pin = path.len() == 4
                && path
                    .last()
                    .and_then(Value::as_str)
                    .is_some_and(|k| PIN_KEYS.contains(&k))
                && path
                    .iter()
                    .zip(PIN_PARENT)
                    .all(|(a, b)| a.as_str() == Some(b));
            if let Some(v) = op.get_mut("v") {
                if is_pin {
                    *v = Value::from(if v.as_str().is_some_and(|s| !s.is_empty()) {
                        "****"
                    } else {
                        ""
                    });
                } else {
                    redact_at(&path, v);
                }
            }
            op
        })
        .collect()
}

const LOG_CAP: usize = 2000;
const LOG_TAIL: usize = 300;
const TICK_INTERVAL: Duration = Duration::from_millis(50);
const LOOP_INTERVAL: Duration = Duration::from_millis(2);

struct Runtime {
    doc: Document,
    engine: Engine,
    io: Io,
    mode: Mode,
    preflight: Vec<Issue>,
    recovery: Option<Show>,
    updater: Option<crate::update::Updater>,
    log: Vec<LogEntry>,
    new_log: Vec<LogEntry>,
    handle: Handle,
    port_override: Option<u16>,
    /// The port the server actually listens on (set by the binary after bind).
    urls: (Option<Instant>, Vec<String>),
    state_dirty: bool,
    last_state: Value,
    rev: u64,
    lookahead: u64,
}

/// Start the runtime thread. Returns the handle and the join handle.
pub fn spawn(config: Config) -> (Handle, std::thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<Request>();
    let (events, _) = broadcast::channel(1024);
    let handle = Handle {
        tx,
        published: Arc::new(RwLock::new(Published::default())),
        events,
        clients: Arc::new(Mutex::new(Vec::new())),
        quit: Arc::new(tokio::sync::Notify::new()),
        quit_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        raise: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let h = handle.clone();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("sm-engine".into())
        .spawn(move || {
            let mut rt = Runtime::new(config, h);
            rt.publish(true);
            let _ = ready_tx.send(());
            rt.run(rx);
        });
    // Wait until the first state is published so clients never see an empty document.
    let _ = ready_rx.recv_timeout(Duration::from_secs(20));
    match thread {
        Ok(t) => (handle, t),
        Err(e) => {
            tracing::error!("cannot start engine thread: {e}");
            (handle, std::thread::spawn(|| {}))
        }
    }
}

impl Runtime {
    fn new(mut config: Config, handle: Handle) -> Self {
        let mut log = Vec::new();
        let mut note = |level: &'static str, message: String| {
            log.push(LogEntry {
                time_ms: now_ms(),
                level,
                message,
            })
        };
        let mut recovery = None;
        let (show, path) = match (config.show.take(), config.show_path.take()) {
            (Some(show), path) => (show, path),
            (None, Some(path)) => match showfile::load(&path) {
                Ok(show) => {
                    recovery = showfile::recoverable(&path);
                    note("info", format!("Opened {}", path.display()));
                    (show, Some(path))
                }
                Err(e) => {
                    note("error", format!("Cannot open {}: {e}", path.display()));
                    // A missing file becomes a new show at that path; a broken one is never overwritten silently.
                    let keep = !path.exists();
                    (Show::default(), keep.then_some(path))
                }
            },
            (None, None) => (Show::default(), None),
        };
        let doc = Document::new(show, path);
        let dir = doc.show_dir().map(Path::to_path_buf);
        let io = match config.midi_backend.take() {
            Some(m) => Io::with_midi_backend(&doc.show, dir, m),
            None => Io::new(&doc.show, dir),
        };
        let engine = Engine::new(doc.show.clone(), io.sample_rate());
        let mut rt = Self {
            doc,
            engine,
            io,
            mode: Mode::Edit,
            preflight: Vec::new(),
            recovery,
            updater: config
                .update_check
                .then(|| crate::update::Updater::new(VERSION)),
            log,
            new_log: Vec::new(),
            handle,
            port_override: config.port,
            urls: (None, Vec::new()),
            state_dirty: true,
            last_state: Value::Null,
            rev: 0,
            lookahead: 0,
        };
        rt.update_lookahead();
        let status = rt.io.audio_status();
        if let Some(e) = &status.error {
            rt.note("warn", format!("Audio: {e}"));
        }
        rt.note(
            "info",
            format!(
                "Audio output: {} / {} @ {} Hz, {} ch",
                status.backend, status.device, status.sample_rate, status.channels
            ),
        );
        rt
    }

    fn update_lookahead(&mut self) {
        let status = self.io.audio_status();
        let sr = u64::from(status.sample_rate.max(1));
        // One device buffer plus scheduling slack for this thread.
        self.lookahead = u64::from(status.buffer_frames.unwrap_or(512)) + sr * 12 / 1000;
    }

    fn note(&mut self, level: &'static str, message: String) {
        match level {
            "error" => tracing::error!("{message}"),
            "warn" => tracing::warn!("{message}"),
            _ => tracing::info!("{message}"),
        }
        let entry = LogEntry {
            time_ms: now_ms(),
            level,
            message,
        };
        self.new_log.push(entry.clone());
        self.log.push(entry);
        if self.log.len() > LOG_CAP {
            self.log.drain(..LOG_CAP / 4);
        }
    }

    fn run(&mut self, rx: mpsc::Receiver<Request>) {
        let mut last_tick = Instant::now();
        let mut last_autosave = Instant::now();
        let mut last_slow = Instant::now();
        loop {
            let first = match rx.recv_timeout(LOOP_INTERVAL) {
                Ok(r) => Some(r),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if self.handle.stop.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            // A bug must never take the command bus down mid-show: contain a
            // panic to the request that caused it and keep running.
            let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let now = self.io.now() + self.lookahead;
                self.engine.advance(now);
                let mut pending = first;
                while let Some(req) = pending {
                    let result = self.handle_request(req.command, req.role, &req.origin);
                    if let Some(reply) = req.reply {
                        let _ = reply.send(result);
                    }
                    self.state_dirty = true;
                    pending = rx.try_recv().ok();
                }
                self.pump();
            }));
            if step.is_err() {
                self.note(
                    "error",
                    "Internal error in the engine thread - recovered, see log file".into(),
                );
                self.state_dirty = true;
            }
            if last_slow.elapsed() >= Duration::from_secs(1) {
                last_slow = Instant::now();
                self.state_dirty = true; // peers, clients, device errors
                if self
                    .updater
                    .as_ref()
                    .is_some_and(|u| u.get() == crate::update::State::Quit)
                {
                    self.note("info", "Update installed - restarting".into());
                    self.publish(false);
                    self.handle.request_quit();
                    self.updater = None;
                }
            }
            if last_tick.elapsed() >= TICK_INTERVAL {
                last_tick = Instant::now();
                self.publish(false);
            }
            if last_autosave.elapsed() >= Duration::from_secs(5) {
                last_autosave = Instant::now();
                self.autosave();
            }
        }
        self.autosave();
    }

    /// Move engine output to the I/O layer and collect events.
    fn pump(&mut self) {
        // Voice failures feed back into the engine, which may emit more actions.
        for _ in 0..4 {
            let actions = self.engine.drain_actions();
            if actions.is_empty() {
                break;
            }
            for (voice, message) in self.io.apply(actions) {
                self.engine.voice_error(voice, message);
            }
        }
        let (failed, messages) = self.io.tick(self.engine.now());
        for (voice, message) in failed {
            self.engine.voice_error(voice, message);
        }
        for m in messages {
            let level = if m.contains("UNDERRUN") {
                "error"
            } else {
                "warn"
            };
            self.note(level, m);
        }
        let events = self.engine.drain_events();
        if !events.is_empty() {
            self.state_dirty = true;
        }
        for ev in events {
            self.on_event(ev);
        }
    }

    fn cue_label(&self, id: u64) -> String {
        match self.doc.show.cue(id) {
            Some(c) => match (c.number.is_empty(), c.name.is_empty()) {
                (false, false) => format!("{} \"{}\"", c.number, c.name),
                (false, true) => c.number.clone(),
                (true, false) => format!("\"{}\"", c.name),
                (true, true) => format!("#{id}"),
            },
            None => format!("#{id}"),
        }
    }

    fn on_event(&mut self, ev: Event) {
        let (level, text): (&'static str, String) = match ev.kind {
            EventKind::Go { cue } => ("info", format!("GO cue {}", self.cue_label(cue))),
            EventKind::GoIgnored { reason } => ("warn", format!("GO ignored ({reason})")),
            EventKind::CueTriggered { .. } => return,
            EventKind::CueStarted { cue } => {
                ("info", format!("Cue {} started", self.cue_label(cue)))
            }
            EventKind::CueEnded { cue } => {
                ("info", format!("Cue {} finished", self.cue_label(cue)))
            }
            EventKind::CueStopped { cue } => {
                ("info", format!("Cue {} stopped", self.cue_label(cue)))
            }
            EventKind::CuePaused { cue } => ("info", format!("Cue {} paused", self.cue_label(cue))),
            EventKind::CueResumed { cue } => {
                ("info", format!("Cue {} resumed", self.cue_label(cue)))
            }
            EventKind::CueLoaded { cue, time } => (
                "info",
                format!("Cue {} loaded to {time:.2} s", self.cue_label(cue)),
            ),
            EventKind::CueError { cue, message } => (
                "error",
                format!("Cue {} ERROR: {message}", self.cue_label(cue)),
            ),
            EventKind::CueArmed { cue, armed } => {
                // Runtime arm/disarm is part of the show: keep the document in sync.
                self.doc.touch(|show| {
                    if let Some(c) = show.cue_mut(cue) {
                        c.armed = armed;
                    }
                });
                (
                    "info",
                    format!(
                        "Cue {} {}",
                        self.cue_label(cue),
                        if armed { "armed" } else { "disarmed" }
                    ),
                )
            }
            EventKind::Playhead { .. } => return,
            EventKind::StopAll => ("warn", "STOP ALL".into()),
            EventKind::Panic => ("error", "PANIC".into()),
            EventKind::PausedAll => ("info", "Paused all".into()),
            EventKind::ResumedAll => ("info", "Resumed all".into()),
            EventKind::Reset => ("info", "Reset".into()),
            EventKind::Timecode { action, position } => {
                ("info", format!("Timecode {action} at {position}"))
            }
            EventKind::Master { .. } => return,
            EventKind::Info { message } => ("warn", message),
        };
        self.note(level, text);
    }

    fn any_running(&self) -> bool {
        !self.engine.snapshot().active.is_empty()
    }

    fn run_preflight(&mut self) {
        let status = self.io.audio_status();
        let ports = self.io.midi.ports();
        let dir = self.doc.show_dir().map(Path::to_path_buf);
        let env = preflight::Environment {
            show_dir: dir.as_deref(),
            audio_device: (!status.is_null).then(|| status.device.clone()),
            audio_channels: status.channels,
            midi_ports: ports,
        };
        self.preflight = preflight::check(&self.doc.show, &env);
    }

    /// Push the edited show into engine and I/O.
    fn show_changed(&mut self) {
        if self
            .io
            .audio_settings_changed(&self.doc.show.settings.audio)
        {
            let _ = self.engine.command(Command::StopAll);
            self.pump();
            let rate_changed = self.io.reopen_audio(&self.doc.show);
            if rate_changed {
                self.engine = Engine::new(self.doc.show.clone(), self.io.sample_rate());
            }
            self.update_lookahead();
            let status = self.io.audio_status();
            self.note(
                "info",
                format!(
                    "Audio output: {} / {} @ {} Hz, {} ch",
                    status.backend, status.device, status.sample_rate, status.channels
                ),
            );
            if let Some(e) = status.error {
                self.note("warn", format!("Audio: {e}"));
            }
        }
        self.engine.set_show(self.doc.show.clone());
        for w in self.io.configure(&self.doc.show) {
            self.note("warn", w);
        }
    }

    /// Replace the whole document (new / open / recover).
    fn load_document(&mut self, doc: Document) {
        let _ = self.engine.command(Command::Panic);
        self.pump();
        self.doc = doc;
        self.io
            .set_show_dir(self.doc.show_dir().map(Path::to_path_buf));
        if self
            .io
            .audio_settings_changed(&self.doc.show.settings.audio)
        {
            self.io.reopen_audio(&self.doc.show);
            self.update_lookahead();
        }
        self.engine = Engine::new(self.doc.show.clone(), self.io.sample_rate());
        // Engine time must continue from the device clock, not restart at zero.
        let now = self.io.now() + self.lookahead;
        self.engine.advance(now);
        for w in self.io.configure(&self.doc.show) {
            self.note("warn", w);
        }
        self.preflight.clear();
        self.mode = Mode::Edit;
    }

    fn autosave(&mut self) {
        if !self.doc.dirty {
            return;
        }
        if let Some(path) = self.doc.path.clone() {
            if let Err(e) = showfile::autosave(&self.doc.show, &path) {
                self.note("error", format!("Autosave failed: {e}"));
            }
        }
    }

    fn handle_request(
        &mut self,
        command: AnyCommand,
        role: Role,
        origin: &str,
    ) -> Result<Value, String> {
        if role < command.required_role() {
            return Err("forbidden".into());
        }
        match command {
            AnyCommand::Engine(cmd) => {
                if !matches!(
                    cmd,
                    Command::SetMaster { .. }
                        | Command::SetPlayhead { .. }
                        | Command::Next { .. }
                        | Command::Previous { .. }
                ) {
                    tracing::debug!(?cmd, origin, "command");
                }
                let r = self.engine.command(cmd).map(|_| Value::Null);
                self.pump();
                r
            }
            AnyCommand::Edit(cmd) => {
                if self.mode == Mode::Live {
                    return Err("live_mode".into());
                }
                let result = self.doc.apply(cmd, &Probe)?;
                self.show_changed();
                Ok(result)
            }
            AnyCommand::Sys(cmd) => self.sys(cmd),
        }
    }

    fn sys(&mut self, cmd: SysCommand) -> Result<Value, String> {
        match cmd {
            SysCommand::SetMode { mode, force } => {
                if mode == self.mode {
                    return Ok(Value::Null);
                }
                match mode {
                    Mode::Live => {
                        self.run_preflight();
                        self.mode = Mode::Live;
                        let errors = self
                            .preflight
                            .iter()
                            .filter(|i| i.severity == preflight::Severity::Error)
                            .count();
                        self.note(
                            if errors > 0 { "warn" } else { "info" },
                            format!(
                                "LIVE mode - pre-flight: {errors} error(s), {} item(s) total",
                                self.preflight.len()
                            ),
                        );
                    }
                    Mode::Edit => {
                        if self.any_running() && !force {
                            return Err("cues_running".into());
                        }
                        self.mode = Mode::Edit;
                        self.note("info", "EDIT mode".into());
                    }
                }
                Ok(Value::Null)
            }
            SysCommand::RunPreflight => {
                self.io.midi.rescan();
                self.run_preflight();
                Ok(json!({ "issues": self.preflight }))
            }
            SysCommand::NewShow { force } => {
                self.guard_structure(force)?;
                self.load_document(Document::default());
                self.recovery = None;
                self.note("info", "New show".into());
                Ok(Value::Null)
            }
            SysCommand::OpenShow { path, force } => {
                self.guard_structure(force)?;
                let path = PathBuf::from(path);
                let show = showfile::load(&path).map_err(|e| e.to_string())?;
                self.recovery = showfile::recoverable(&path);
                self.load_document(Document::new(show, Some(path.clone())));
                self.note("info", format!("Opened {}", path.display()));
                Ok(Value::Null)
            }
            SysCommand::SaveShow { path } => {
                let path = match path.filter(|p| !p.is_empty()) {
                    Some(p) => {
                        let mut p = PathBuf::from(p);
                        if p.extension().is_none() {
                            p.set_extension(showfile::EXTENSION);
                        }
                        p
                    }
                    None => self.doc.path.clone().ok_or("path_required")?,
                };
                // Saving to a new location: media paths stay valid by making them absolute first.
                let old_dir = self.doc.show_dir().map(Path::to_path_buf);
                let new_dir = path.parent().map(Path::to_path_buf);
                if old_dir != new_dir {
                    self.doc.show.for_each_cue_mut(|c| {
                        if let sm_core::model::CueKind::Audio(a) = &mut c.kind {
                            if !a.file.is_empty() {
                                let abs = showfile::resolve_media(old_dir.as_deref(), &a.file);
                                a.file = showfile::relativize_media(new_dir.as_deref(), &abs);
                            }
                        }
                    });
                }
                showfile::save(&self.doc.show, &path).map_err(|e| e.to_string())?;
                self.doc.path = Some(path.clone());
                self.doc.dirty = false;
                self.recovery = None;
                self.io.set_show_dir(new_dir);
                self.engine.set_show(self.doc.show.clone());
                self.note("info", format!("Saved {}", path.display()));
                Ok(json!({ "path": path }))
            }
            SysCommand::CollectMedia => {
                if self.mode == Mode::Live {
                    return Err("live_mode".into());
                }
                let dir = self
                    .doc
                    .show_dir()
                    .map(Path::to_path_buf)
                    .ok_or("save the show first")?;
                let (copied, errors) = showfile::collect_media(&mut self.doc.show, &dir);
                self.doc.dirty = true;
                self.engine.set_show(self.doc.show.clone());
                for e in &errors {
                    self.note("error", format!("Collect: {e}"));
                }
                self.note(
                    "info",
                    format!(
                        "Collected media: {copied} file(s) copied, {} problem(s)",
                        errors.len()
                    ),
                );
                Ok(json!({ "copied": copied, "errors": errors }))
            }
            SysCommand::Recover { accept } => {
                let recovered = self.recovery.take().ok_or("nothing to recover")?;
                if accept {
                    let path = self.doc.path.clone();
                    let mut doc = Document::new(recovered, path);
                    doc.dirty = true;
                    self.load_document(doc);
                    self.note("warn", "Recovered unsaved changes from autosave".into());
                } else if let Some(p) = &self.doc.path {
                    let _ = std::fs::remove_file(showfile::autosave_path(p));
                }
                Ok(Value::Null)
            }
            SysCommand::KickClient { id } => {
                let _ = self.handle.events.send(Outbound::Kick(id));
                Ok(Value::Null)
            }
            SysCommand::RtpConnect { addr } => {
                self.io.midi.rtp_connect(&addr)?;
                Ok(Value::Null)
            }
            SysCommand::ClearLog => {
                self.log.clear();
                Ok(Value::Null)
            }
            SysCommand::SetNotes { text } => {
                self.doc.touch(|s| s.notes = text);
                Ok(Value::Null)
            }
            SysCommand::CheckUpdate => {
                match &self.updater {
                    Some(u) if !u.busy() => u.recheck(),
                    Some(_) => return Err("an update is being installed".into()),
                    // Switched off at start-up: an explicit request turns it on.
                    None => self.updater = Some(crate::update::Updater::new(VERSION)),
                }
                Ok(Value::Null)
            }
            SysCommand::InstallUpdate => {
                // Never replace the program under a running show.
                if self.mode == Mode::Live {
                    return Err("live_mode".into());
                }
                if self.any_running() {
                    return Err("cues_running".into());
                }
                if self.doc.dirty {
                    return Err("unsaved_changes".into());
                }
                self.updater
                    .as_ref()
                    .ok_or("update check is switched off")?
                    .start()?;
                self.note("info", "Installing update".into());
                Ok(Value::Null)
            }
            SysCommand::RaiseWindow => {
                self.handle
                    .raise
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Value::Null)
            }
            SysCommand::Quit { force } => {
                if !force && self.any_running() {
                    return Err("cues_running".into());
                }
                if !force && self.doc.dirty {
                    return Err("unsaved_changes".into());
                }
                self.note("info", "Quit requested".into());
                self.handle.request_quit();
                Ok(Value::Null)
            }
            SysCommand::ResetClip => {
                self.io.reset_clip();
                Ok(Value::Null)
            }
        }
    }

    fn guard_structure(&self, force: bool) -> Result<(), String> {
        if self.mode == Mode::Live {
            return Err("live_mode".into());
        }
        if self.doc.dirty && !force {
            return Err("unsaved_changes".into());
        }
        Ok(())
    }

    fn build_state(&mut self) -> Value {
        let snap = self.engine.snapshot();
        let audio = self.io.audio_status();
        let peers: Vec<Value> = self
            .io
            .midi
            .rtp_peers()
            .iter()
            .map(|p| {
                json!({
                    "name": p.name, "addr": p.addr.to_string(),
                    "state": format!("{:?}", p.state).to_lowercase(),
                    "latency_ms": p.latency_us.map(|u| u as f64 / 1000.0),
                })
            })
            .collect();
        let clients = self
            .handle
            .clients
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default();
        let remote = &self.doc.show.settings.remote;
        let port = self.port_override.unwrap_or(remote.port);
        if self
            .urls
            .0
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            let urls = if remote.enabled {
                crate::http::local_urls(port)
            } else {
                Vec::new()
            };
            self.urls = (Some(Instant::now()), urls);
        }
        let rtp = &self.doc.show.rtp_midi;
        json!({
            "version": VERSION,
            "mode": self.mode,
            "show": self.doc.show,
            "show_path": self.doc.path,
            "dirty": self.doc.dirty,
            "can_undo": self.doc.can_undo(),
            "can_redo": self.doc.can_redo(),
            "recovery_available": self.recovery.is_some(),
            "playheads": snap.lists.iter().map(|l| (l.id.to_string(), json!(l.playhead))).collect::<serde_json::Map<_, _>>(),
            "errors": snap.errors.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
            "loaded": snap.loaded.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
            "paused_all": snap.paused_all,
            "master_db": snap.master_db,
            "audio": audio,
            "midi": {
                "ports": self.io.midi.ports(),
                "rtp": { "enabled": rtp.enabled, "name": rtp.session_name, "port": self.io.midi.rtp_local_port().unwrap_or(rtp.port), "peers": peers },
            },
            "remote": { "urls": self.urls.1, "port": port, "clients": clients },
            "preflight": self.preflight,
            "layouts": LAYOUTS,
            "update": self.updater.as_ref().map(|u| u.json()).unwrap_or_else(|| crate::update::disabled_json(VERSION)),
        })
    }

    fn build_tick(&mut self) -> Value {
        let snap = self.engine.snapshot();
        let meters = self.io.meters();
        let status = self.io.status();
        let tc = &self.doc.show.timecode;
        let alarms: Vec<&str> = match snap.show_elapsed {
            Some(t) => self
                .doc
                .show
                .settings
                .timer_alarms
                .iter()
                .filter(|a| t >= a.at && t < a.at + 10.0)
                .map(|a| a.label.as_str())
                .collect(),
            None => Vec::new(),
        };
        json!({
            "t": "tick",
            "frame": snap.frame,
            "wall_ms": now_ms(),
            "show_elapsed": snap.show_elapsed,
            "tc": { "position": snap.tc_position, "running": snap.tc.running, "rate": snap.tc.rate, "ltc": tc.ltc_enabled, "mtc": tc.mtc_enabled },
            "active": snap.active,
            "next_auto": snap.next_auto,
            "meters": { "peak": meters.peak, "rms": meters.rms, "clip": meters.clip },
            "cpu": status.cpu, "underruns": status.underruns, "voices": status.voices,
            "alarms": alarms,
        })
    }

    fn publish(&mut self, force: bool) {
        let tick = self.build_tick();
        let tick_text: Arc<str> = tick.to_string().into();
        let mut patch = None;
        if self.state_dirty || force {
            self.state_dirty = false;
            let state = self.build_state();
            let ops = diff(&self.last_state, &state);
            if !ops.is_empty() {
                self.rev += 1;
                let admin: Arc<str> = json!({ "t": "patch", "rev": self.rev, "ops": ops })
                    .to_string()
                    .into();
                let public: Arc<str> =
                    json!({ "t": "patch", "rev": self.rev, "ops": redact_ops(ops) })
                        .to_string()
                        .into();
                patch = Some(Outbound::Patch {
                    rev: self.rev,
                    admin,
                    public,
                });
                self.last_state = state;
            }
        }
        let log = std::mem::take(&mut self.new_log);
        if let Ok(mut p) = self.handle.published.write() {
            p.tick = tick;
            if patch.is_some() {
                p.rev = self.rev;
                p.state = self.last_state.clone();
            }
            if !log.is_empty() || p.log_tail.len() > self.log.len() {
                let from = self.log.len().saturating_sub(LOG_TAIL);
                p.log_tail = self
                    .log
                    .get(from..)
                    .map(<[LogEntry]>::to_vec)
                    .unwrap_or_default();
            }
        }
        // Order matters for clients: patch first, then the tick that refers to it.
        if let Some(p) = patch {
            let _ = self.handle.events.send(p);
        }
        if !log.is_empty() {
            let text: Arc<str> = json!({ "t": "log", "entries": log }).to_string().into();
            let _ = self.handle.events.send(Outbound::Log(text));
        }
        let _ = self.handle.events.send(Outbound::Tick(tick_text));
    }
}
