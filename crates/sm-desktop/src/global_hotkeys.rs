//! System-wide ("global") hotkeys: they work while another application has
//! the keyboard focus, e.g. GO from a keyboard while a lighting or video
//! application is in front.
//!
//! * Opt-in and per machine: the list lives in the preferences
//!   (`desktop.json`, see `prefs.rs`), never in the show file, and is empty
//!   by default.
//! * Every combination needs a modifier (Ctrl / Cmd / Alt / Super, or Shift
//!   with a function key): a plain Space or Esc would be taken away from
//!   every other application on the computer.
//! * Backends (`global-hotkey` crate): Windows `RegisterHotKey`, macOS Carbon
//!   `RegisterEventHotKey` (no Accessibility permission needed), Linux
//!   `XGrabKey` on the X11 root window. Wayland compositors do not allow
//!   global key grabs, so on Wayland the feature is off and Setup says why.
//! * Presses arrive on the platform's event thread and go through a channel
//!   (and wake the window); [`process`], called from `App::logic`, runs the
//!   actions on the UI thread through `App::run_action`, so GO debounce and
//!   the PANIC double-tap setting apply exactly as for window hotkeys.
//! * Registration failures (combination taken by another application) are
//!   shown in Setup and logged; they are never fatal.
//! * A press of a registered combination while the window has focus is
//!   handled here once, and a second report of the same press from the other
//!   source (window key event + system-wide event) is ignored.

use crate::app::App;
use global_hotkey::hotkey::{Code, Modifiers};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// Actions that can be bound system-wide. Editing actions (undo, delete, ...)
/// make no sense while another application is in front.
pub const GLOBAL_ACTIONS: [&str; 9] = [
    "go",
    "panic",
    "stop_all",
    "toggle_pause",
    "next",
    "previous",
    "reset",
    "preview",
    "save",
];

/// One physical press reported by both the window and the system-wide hook
/// within this time counts once.
pub const DEDUPE: Duration = Duration::from_millis(300);

/// Why a combination cannot be used system-wide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No modifier, or only Shift together with a typing key.
    NeedsModifier,
    /// The key has no system-wide equivalent.
    UnsupportedKey,
    /// The action cannot be bound system-wide.
    UnsupportedAction,
    /// Already used by this other system-wide action.
    Conflict(String),
}

impl Refusal {
    /// User-facing explanation for `combo`.
    pub fn text(&self, combo: &str) -> String {
        let shown = format_combo(combo);
        match self {
            Refusal::NeedsModifier => format!(
                "{shown} cannot be a system-wide key: it would be taken away from every \
                 other application. Use a combination with Ctrl, Alt or {} \
                 (Shift alone only with F1-F24), e.g. {}.",
                if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Super"
                },
                format_combo("Ctrl+Alt+KeyG")
            ),
            Refusal::UnsupportedKey => {
                format!("{shown} cannot be registered as a system-wide key.")
            }
            Refusal::UnsupportedAction => "This action cannot be used system-wide.".to_string(),
            Refusal::Conflict(other) => format!(
                "{shown} is already used system-wide by \"{}\". Not assigned - clear that \
                 binding first.",
                action_label(other)
            ),
        }
    }
}

/// Human-readable combination: `Ctrl+Shift+KeyZ` -> `Ctrl + Shift + Z`
/// (`Cmd + Shift + Z` on macOS, where the stored "Ctrl" means Cmd).
pub fn format_combo(combo: &str) -> String {
    let normalized = App::normalize_combo(combo);
    let mut parts: Vec<&str> = normalized.split('+').collect();
    let code = parts.pop().unwrap_or_default();
    let mac = cfg!(target_os = "macos");
    let mut out: Vec<String> = parts
        .iter()
        .map(|m| {
            match (*m, mac) {
                ("Ctrl", true) => "Cmd",
                ("Alt", true) => "Option",
                ("Meta", true) => "Ctrl",
                ("Meta", false) => "Super",
                (other, _) => other,
            }
            .to_string()
        })
        .collect();
    let key = match code {
        "Escape" => "Esc".to_string(),
        "ArrowUp" => "Up".to_string(),
        "ArrowDown" => "Down".to_string(),
        "ArrowLeft" => "Left".to_string(),
        "ArrowRight" => "Right".to_string(),
        "Delete" => "Del".to_string(),
        c => c
            .strip_prefix("Key")
            .or_else(|| c.strip_prefix("Digit"))
            .unwrap_or(c)
            .to_string(),
    };
    out.push(key);
    out.join(" + ")
}

/// Label of an action (as in the hotkey list).
pub fn action_label(action: &str) -> &str {
    crate::app::ACTIONS
        .iter()
        .find(|(a, _)| *a == action)
        .map_or(action, |(_, l)| l)
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Modifiers and key of a stored combination for the hotkey backend.
/// "Ctrl" is Cmd on macOS (as in `show.hotkeys`) and "Meta" the Ctrl key
/// there; elsewhere "Meta" is the Super / Windows key.
pub fn to_mods_and_code(combo: &str) -> Option<(Modifiers, Code)> {
    let n = App::normalize_combo(combo.trim());
    let mut parts: Vec<&str> = n.split('+').collect();
    let code: Code = parts.pop()?.parse().ok()?;
    if matches!(code, Code::Unidentified) {
        return None;
    }
    let mac = cfg!(target_os = "macos");
    let mut mods = Modifiers::empty();
    for p in parts {
        mods |= match (p, mac) {
            ("Ctrl", true) | ("Meta", false) => Modifiers::META,
            ("Ctrl", false) | ("Meta", true) => Modifiers::CONTROL,
            ("Shift", _) => Modifiers::SHIFT,
            ("Alt", _) => Modifiers::ALT,
            _ => return None,
        };
    }
    Some((mods, code))
}

/// Check a combination for system-wide use; returns it normalised.
pub fn validate(combo: &str) -> Result<String, Refusal> {
    let n = App::normalize_combo(combo.trim());
    let mut parts: Vec<&str> = n.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    if key.is_empty() || to_mods_and_code(&n).is_none() {
        return Err(Refusal::UnsupportedKey);
    }
    if parts.is_empty() || (parts == ["Shift"] && !is_function_key(key)) {
        return Err(Refusal::NeedsModifier);
    }
    Ok(n)
}

/// Bind `combo` to `action` in `map` (action -> combination), replacing the
/// action's previous combination. Refuses plain keys and conflicts.
pub fn assign(
    map: &BTreeMap<String, String>,
    action: &str,
    combo: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    if !GLOBAL_ACTIONS.contains(&action) {
        return Err(Refusal::UnsupportedAction);
    }
    let n = validate(combo)?;
    if let Some((other, _)) = map
        .iter()
        .find(|(a, c)| a.as_str() != action && App::normalize_combo(c) == n)
    {
        return Err(Refusal::Conflict(other.clone()));
    }
    let mut next = map.clone();
    next.insert(action.to_string(), n);
    Ok(next)
}

/// Why system-wide hotkeys cannot work in this session (`None` = they can).
/// `env` reads an environment variable.
pub fn unsupported_reason(os: &str, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    if matches!(os, "windows" | "macos") {
        return None;
    }
    let set = |k: &str| env(k).is_some_and(|v| !v.trim().is_empty());
    let wayland = env("XDG_SESSION_TYPE").is_some_and(|t| t.eq_ignore_ascii_case("wayland"))
        || set("WAYLAND_DISPLAY");
    if wayland {
        return Some(
            "This is a Wayland session. Wayland does not allow applications to register \
             system-wide keys, so they are off. Log in with an X11 session (e.g. \"GNOME on \
             Xorg\" or \"Plasma (X11)\") to use them, or use the window hotkeys, MIDI or OSC."
                .to_string(),
        );
    }
    if !set("DISPLAY") {
        return Some(
            "No X11 display (DISPLAY is not set): system-wide hotkeys are off.".to_string(),
        );
    }
    None
}

/// Shown under the system-wide list on every platform.
pub const HIDDEN_NOTE: &str = "While the window is closed (Show Master keeps running in the \
     background) or minimized, presses take up to about 0.1 s longer.";

/// Fixed platform notes shown under the system-wide list.
pub fn platform_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS: no special permission is needed (no Accessibility access). Combinations that \
         macOS or another application already reserved (e.g. Cmd + Space, Cmd + Tab) cannot \
         be registered."
    } else if cfg!(target_os = "windows") {
        "Windows: combinations that another application or Windows itself already uses \
         (e.g. Win + L, Ctrl + Alt + Del) cannot be registered."
    } else {
        "Linux: works in X11 sessions only - Wayland compositors do not allow system-wide \
         keys. Presses are picked up within about 50 ms."
    }
}

// ----------------------------------------------------------------------
// Backend
// ----------------------------------------------------------------------

/// The platform hook, behind a trait so tests run without a display.
pub trait Backend {
    /// Grab `combo` (normalised) system-wide. Err = user-facing reason.
    fn register(&mut self, combo: &str) -> Result<(), String>;
    fn unregister(&mut self, combo: &str);
    /// Combinations pressed since the last call (key-down only, no repeats).
    fn take_pressed(&mut self) -> Vec<String>;
}

type Starter = fn(&egui::Context) -> Result<Box<dyn Backend>, String>;

/// Where a press came from (for de-duplication).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Window,
    System,
}

/// State of one configured action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Registered and working.
    Active,
    /// Could not be registered (reason).
    Failed(String),
    /// Paused while a key is being captured in Setup.
    Paused,
    /// Not running here (reason).
    Off(String),
}

/// Registered system-wide hotkeys of this window.
#[derive(Default)]
pub struct GlobalHotkeys {
    starter: Option<Starter>,
    started: bool,
    backend: Option<Box<dyn Backend>>,
    /// Why the feature is off on this machine / session.
    pub unsupported: Option<String>,
    /// Registered: combination -> action.
    active: BTreeMap<String, String>,
    /// Last list asked for (action -> combination).
    wanted: Option<BTreeMap<String, String>>,
    paused: bool,
    /// Registration problems: action -> reason.
    errors: BTreeMap<String, String>,
    last: HashMap<String, (Source, Instant)>,
}

impl GlobalHotkeys {
    /// The real platform hook. It is started lazily, on the UI thread, the
    /// first time a system-wide hotkey is configured (nothing is grabbed and
    /// no X11 connection is opened while the list is empty).
    pub fn native() -> Self {
        let mut s = Self::default();
        s.starter = Some(native::start);
        // Known up front (from the environment), so Setup can say it at once.
        s.unsupported = unsupported_reason(std::env::consts::OS, |k| std::env::var(k).ok());
        s
    }

    /// A given backend (tests).
    pub fn with_backend(backend: Box<dyn Backend>) -> Self {
        let mut s = Self::default();
        s.backend = Some(backend);
        s.started = true;
        s
    }

    fn ensure_started(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        let Some(start) = self.starter else {
            return;
        };
        self.started = true;
        if let Some(reason) = &self.unsupported {
            tracing::info!("system-wide hotkeys off: {reason}");
            return;
        }
        match start(ctx) {
            Ok(b) => self.backend = Some(b),
            Err(e) => {
                tracing::warn!("system-wide hotkeys could not be started: {e}");
                self.unsupported = Some(format!("System-wide hotkeys could not be started: {e}"));
            }
        }
    }

    /// Make the registrations match `wanted` (action -> combination).
    /// `paused` unregisters everything (a key is being captured in Setup).
    pub fn sync(&mut self, ctx: &egui::Context, wanted: &BTreeMap<String, String>, paused: bool) {
        if self.wanted.as_ref() == Some(wanted) && self.paused == paused {
            return;
        }
        if !wanted.is_empty() {
            self.ensure_started(ctx);
        }
        self.wanted = Some(wanted.clone());
        self.paused = paused;
        self.errors.clear();
        let mut desired: BTreeMap<String, String> = BTreeMap::new();
        if !paused {
            for (action, combo) in wanted {
                if !GLOBAL_ACTIONS.contains(&action.as_str()) {
                    self.errors.insert(
                        action.clone(),
                        Refusal::UnsupportedAction.text(combo).to_string(),
                    );
                    continue;
                }
                match validate(combo) {
                    Ok(n) => {
                        if let Some(other) = desired.get(&n) {
                            let text = Refusal::Conflict(other.clone()).text(&n);
                            self.errors.insert(action.clone(), text);
                        } else {
                            desired.insert(n, action.clone());
                        }
                    }
                    Err(r) => {
                        self.errors.insert(action.clone(), r.text(combo));
                    }
                }
            }
        }
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let stale: Vec<String> = self
            .active
            .keys()
            .filter(|c| !desired.contains_key(*c))
            .cloned()
            .collect();
        for combo in stale {
            backend.unregister(&combo);
            self.active.remove(&combo);
            tracing::info!("system-wide hotkey {combo} released");
        }
        for (combo, action) in desired {
            if let Some(a) = self.active.get_mut(&combo) {
                *a = action;
                continue;
            }
            match backend.register(&combo) {
                Ok(()) => {
                    tracing::info!("system-wide hotkey {combo} -> {action}");
                    self.active.insert(combo, action);
                }
                Err(e) => {
                    tracing::warn!("system-wide hotkey {combo} ({action}) not registered: {e}");
                    self.errors.insert(action, e);
                }
            }
        }
    }

    /// Try failed registrations again on the next sync.
    pub fn retry(&mut self) {
        self.wanted = None;
    }

    /// Release every registration (exit, or before the hook goes away).
    pub fn shutdown(&mut self) {
        if let Some(b) = self.backend.as_mut() {
            for combo in self.active.keys() {
                b.unregister(combo);
            }
        }
        self.active.clear();
        self.wanted = None;
    }

    /// Action of a registered combination.
    pub fn action_for(&self, combo: &str) -> Option<&String> {
        self.active.get(&App::normalize_combo(combo))
    }

    /// Registered combinations (combination -> action).
    pub fn active(&self) -> &BTreeMap<String, String> {
        &self.active
    }

    pub fn status(&self, action: &str) -> Status {
        if let Some(e) = self.errors.get(action) {
            return Status::Failed(e.clone());
        }
        if self.active.values().any(|a| a == action) {
            return Status::Active;
        }
        if self.paused {
            return Status::Paused;
        }
        Status::Off(
            self.unsupported
                .clone()
                .unwrap_or_else(|| "System-wide hotkeys are not running.".to_string()),
        )
    }

    /// True when `action` from `source` should run (not a second report of
    /// one press by the other source).
    pub fn fire(&mut self, action: &str, source: Source) -> bool {
        let now = Instant::now();
        if let Some((s, t)) = self.last.get(action) {
            if *s != source && now.duration_since(*t) < DEDUPE {
                return false;
            }
        }
        self.last.insert(action.to_string(), (source, now));
        true
    }

    /// Actions of system-wide presses since the last call.
    pub fn take_actions(&mut self) -> Vec<String> {
        let Some(b) = self.backend.as_mut() else {
            return Vec::new();
        };
        let pressed = b.take_pressed();
        let mut out = Vec::new();
        for combo in pressed {
            if let Some(action) = self.active.get(&combo).cloned() {
                if self.fire(&action, Source::System) {
                    out.push(action);
                }
            }
        }
        out
    }
}

impl Drop for GlobalHotkeys {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ----------------------------------------------------------------------
// Setup panel (shared with `setup_hotkeys.rs` through the egui context)
// ----------------------------------------------------------------------

/// A change requested in Setup, applied by [`process`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Set(String, String),
    Clear(String),
    ClearAll,
    Retry,
}

/// What the Setup tab shows and asks for. Stored in the egui context so
/// the Setup window needs no extra plumbing.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    /// Configured list (action -> combination).
    pub combos: BTreeMap<String, String>,
    pub status: BTreeMap<String, Status>,
    /// Why the feature is off here.
    pub unsupported: Option<String>,
    /// Action waiting for its system-wide key.
    pub capture: Option<String>,
    /// Last assignment problem: (action, text).
    pub warning: Option<(String, String)>,
    pub edits: Vec<Edit>,
    /// Pass in which the Setup section was last drawn.
    pub drawn_pass: u64,
}

fn panel_id() -> egui::Id {
    egui::Id::unique("sm_global_hotkeys_panel")
}

pub fn panel(ctx: &egui::Context) -> Panel {
    ctx.data(|d| d.get_temp(panel_id())).unwrap_or_default()
}

pub fn store_panel(ctx: &egui::Context, p: Panel) {
    ctx.data_mut(|d| d.insert_temp(panel_id(), p));
}

/// Per frame, from `App::logic` (after the Setup key capture, before the
/// window hotkeys): apply Setup edits, keep registrations in sync, take the
/// window's own presses of registered combinations and run the actions.
pub fn process(app: &mut App, ctx: &egui::Context) {
    let mut p = panel(ctx);
    let pass = ctx.cumulative_pass_nr();
    if p.capture.is_some() && (!app.setup.open || app.is_live() || p.drawn_pass + 1 < pass) {
        p.capture = None;
    }
    if !p.edits.is_empty() {
        let mut map = app.prefs.global_hotkeys.clone();
        for e in std::mem::take(&mut p.edits) {
            match e {
                Edit::Set(action, combo) => {
                    if let Ok(next) = assign(&map, &action, &combo) {
                        map = next;
                    }
                }
                Edit::Clear(action) => {
                    map.remove(&action);
                }
                Edit::ClearAll => map.clear(),
                Edit::Retry => app.global_hotkeys.retry(),
            }
        }
        app.prefs.set_global_hotkeys(map);
    }
    let paused = p.capture.is_some() || app.setup.capturing_hotkey();
    let wanted = app.prefs.global_hotkeys.clone();
    app.global_hotkeys.sync(ctx, &wanted, paused);
    p.status = wanted
        .keys()
        .map(|a| (a.clone(), app.global_hotkeys.status(a)))
        .collect();
    p.combos = wanted;
    p.unsupported = app.global_hotkeys.unsupported.clone();
    store_panel(ctx, p);

    // The window has focus and sees the press itself: handle it here once.
    let mut fired = Vec::new();
    if !app.global_hotkeys.active().is_empty() {
        let gh = &app.global_hotkeys;
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
                let Some(action) = App::combo(*key, *modifiers).and_then(|c| gh.action_for(&c))
                else {
                    return true;
                };
                if !*repeat {
                    fired.push(action.clone());
                }
                false
            });
        });
    }
    for action in fired {
        if app.global_hotkeys.fire(&action, Source::Window) {
            app.run_action(&action);
        }
    }
    for action in app.global_hotkeys.take_actions() {
        app.run_action(&action);
    }
}

// ----------------------------------------------------------------------
// Platform hook (global-hotkey crate)
// ----------------------------------------------------------------------

mod native {
    use super::{to_mods_and_code, Backend};
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    /// The crate's event handler is process-wide and can be set once.
    static STARTED: AtomicBool = AtomicBool::new(false);

    /// A key still "down" after this long is treated as released (a lost
    /// release event must not disable the hotkey).
    const STUCK: Duration = Duration::from_secs(2);

    pub struct Native {
        manager: GlobalHotKeyManager,
        keys: HashMap<String, HotKey>,
        ids: HashMap<u32, String>,
        rx: Receiver<GlobalHotKeyEvent>,
        down: HashMap<u32, Instant>,
    }

    /// Create the hook. Must run on the UI (main) thread: macOS needs the main
    /// run loop, Windows the thread that pumps the window messages.
    pub fn start(ctx: &egui::Context) -> Result<Box<dyn Backend>, String> {
        if STARTED.swap(true, Ordering::SeqCst) {
            return Err("already running in this process".to_string());
        }
        let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            // Never blocks: unbounded channel, then wake the window.
            let _ = tx.send(e);
            ctx.request_repaint();
        }));
        Ok(Box::new(Native {
            manager,
            keys: HashMap::new(),
            ids: HashMap::new(),
            rx,
            down: HashMap::new(),
        }))
    }

    impl Backend for Native {
        fn register(&mut self, combo: &str) -> Result<(), String> {
            let (mods, code) = to_mods_and_code(combo)
                .ok_or_else(|| "this key cannot be used system-wide".to_string())?;
            let hk = HotKey::new(Some(mods), code);
            self.manager.register(hk).map_err(|e| match e {
                global_hotkey::Error::AlreadyRegistered(_) => {
                    "already used by another application or by the system".to_string()
                }
                other => other.to_string(),
            })?;
            self.ids.insert(hk.id(), combo.to_string());
            self.keys.insert(combo.to_string(), hk);
            Ok(())
        }

        fn unregister(&mut self, combo: &str) {
            if let Some(hk) = self.keys.remove(combo) {
                self.ids.remove(&hk.id());
                self.down.remove(&hk.id());
                if let Err(e) = self.manager.unregister(hk) {
                    tracing::warn!("system-wide hotkey {combo}: {e}");
                }
            }
        }

        fn take_pressed(&mut self) -> Vec<String> {
            let mut out = Vec::new();
            while let Ok(e) = self.rx.try_recv() {
                match e.state {
                    HotKeyState::Pressed => {
                        if self.down.get(&e.id).is_some_and(|t| t.elapsed() < STUCK) {
                            continue; // auto-repeat
                        }
                        self.down.insert(e.id, Instant::now());
                        if let Some(c) = self.ids.get(&e.id) {
                            out.push(c.clone());
                        }
                    }
                    HotKeyState::Released => {
                        self.down.remove(&e.id);
                    }
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn validates_and_normalises() {
        assert_eq!(validate("Alt+Ctrl+KeyG"), Ok("Ctrl+Alt+KeyG".to_string()));
        assert_eq!(validate("Shift+F5"), Ok("Shift+F5".to_string()));
        assert_eq!(validate("Ctrl+Space"), Ok("Ctrl+Space".to_string()));
        assert_eq!(validate("Meta+Digit1"), Ok("Meta+Digit1".to_string()));
        for plain in ["Space", "Escape", "KeyG", "F5", "Enter"] {
            assert_eq!(validate(plain), Err(Refusal::NeedsModifier), "{plain}");
        }
        // Shift + a typing key would steal capital letters system-wide.
        assert_eq!(validate("Shift+KeyG"), Err(Refusal::NeedsModifier));
        assert_eq!(validate("Shift+Space"), Err(Refusal::NeedsModifier));
        assert_eq!(validate("Ctrl+Nonsense"), Err(Refusal::UnsupportedKey));
        assert_eq!(validate(""), Err(Refusal::UnsupportedKey));
        assert!(Refusal::NeedsModifier
            .text("Space")
            .contains("cannot be a system-wide key"));
    }

    #[test]
    fn maps_modifiers_for_the_platform() {
        let (m, c) = to_mods_and_code("Ctrl+Shift+Alt+KeyZ")
            .unwrap_or((Modifiers::empty(), Code::Unidentified));
        assert_eq!(c, Code::KeyZ);
        assert!(m.contains(Modifiers::SHIFT | Modifiers::ALT));
        if cfg!(target_os = "macos") {
            assert!(m.contains(Modifiers::META)); // Cmd
        } else {
            assert!(m.contains(Modifiers::CONTROL));
        }
        assert!(to_mods_and_code("Ctrl+ArrowDown").is_some());
        assert!(to_mods_and_code("Ctrl+PageUp").is_some());
        assert!(to_mods_and_code("Hyper+KeyA").is_some_and(|(m, _)| m.is_empty()));
    }

    #[test]
    fn assigns_and_refuses_conflicts() {
        let empty = BTreeMap::new();
        assert_eq!(assign(&empty, "go", "Space"), Err(Refusal::NeedsModifier));
        assert_eq!(
            assign(&empty, "undo", "Ctrl+Alt+KeyZ"),
            Err(Refusal::UnsupportedAction)
        );
        let map = assign(&empty, "go", "Alt+Ctrl+KeyG").unwrap_or_default();
        assert_eq!(map.get("go").map(String::as_str), Some("Ctrl+Alt+KeyG"));
        assert_eq!(
            assign(&map, "panic", "Ctrl+Alt+KeyG"),
            Err(Refusal::Conflict("go".to_string()))
        );
        assert!(Refusal::Conflict("go".into())
            .text("Ctrl+Alt+KeyG")
            .contains("\"GO\""));
        // Re-assigning replaces the action's previous combination.
        let map = assign(&map, "go", "Ctrl+Alt+KeyH").unwrap_or_default();
        assert_eq!(map.len(), 1);
        assert!(assign(&map, "panic", "Ctrl+Alt+KeyG").is_ok());
    }

    #[test]
    fn detects_wayland_and_missing_display() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(unsupported_reason("linux", env(&[("DISPLAY", ":0")])).is_none());
        let w = unsupported_reason(
            "linux",
            env(&[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")]),
        );
        assert!(w.is_some_and(|t| t.contains("Wayland")));
        assert!(unsupported_reason("linux", env(&[("XDG_SESSION_TYPE", "wayland")])).is_some());
        assert!(
            unsupported_reason("linux", env(&[("XDG_SESSION_TYPE", "x11")]))
                .is_some_and(|t| t.contains("DISPLAY"))
        );
        assert!(unsupported_reason("macos", env(&[])).is_none());
        assert!(unsupported_reason("windows", env(&[])).is_none());
    }

    #[derive(Default)]
    struct Log {
        registered: Vec<String>,
        refuse: Vec<String>,
        pressed: Vec<String>,
    }

    struct Fake(Rc<RefCell<Log>>);

    impl Backend for Fake {
        fn register(&mut self, combo: &str) -> Result<(), String> {
            let mut l = self.0.borrow_mut();
            if l.refuse.iter().any(|c| c == combo) {
                return Err("taken".into());
            }
            l.registered.push(combo.to_string());
            Ok(())
        }
        fn unregister(&mut self, combo: &str) {
            self.0.borrow_mut().registered.retain(|c| c != combo);
        }
        fn take_pressed(&mut self) -> Vec<String> {
            std::mem::take(&mut self.0.borrow_mut().pressed)
        }
    }

    #[test]
    fn syncs_registrations_and_dedupes() {
        let log = Rc::new(RefCell::new(Log::default()));
        log.borrow_mut().refuse.push("Ctrl+Alt+KeyP".into());
        let mut gh = GlobalHotkeys::with_backend(Box::new(Fake(log.clone())));
        let ctx = egui::Context::default();
        let mut want = BTreeMap::new();
        want.insert("go".to_string(), "Alt+Ctrl+KeyG".to_string());
        want.insert("panic".to_string(), "Ctrl+Alt+KeyP".to_string());
        want.insert("stop_all".to_string(), "KeyS".to_string());
        gh.sync(&ctx, &want, false);
        assert_eq!(log.borrow().registered, vec!["Ctrl+Alt+KeyG".to_string()]);
        assert_eq!(gh.status("go"), Status::Active);
        assert_eq!(gh.status("panic"), Status::Failed("taken".into()));
        assert!(matches!(gh.status("stop_all"), Status::Failed(_)));

        // A system-wide press runs once; the window's report of the same
        // press right after is ignored, a later system press is not.
        log.borrow_mut().pressed.push("Ctrl+Alt+KeyG".into());
        assert_eq!(gh.take_actions(), vec!["go".to_string()]);
        assert!(!gh.fire("go", Source::Window));
        assert!(gh.fire("go", Source::System));

        // Pausing (key capture) releases everything; resuming grabs again.
        gh.sync(&ctx, &want, true);
        assert!(log.borrow().registered.is_empty());
        assert_eq!(gh.status("go"), Status::Paused);
        gh.sync(&ctx, &want, false);
        assert_eq!(log.borrow().registered.len(), 1);

        // Removing the entry releases it; shutdown releases the rest.
        want.remove("go");
        gh.sync(&ctx, &want, false);
        assert!(log.borrow().registered.is_empty());
        want.insert("go".to_string(), "Ctrl+Alt+KeyG".to_string());
        gh.sync(&ctx, &want, false);
        drop(gh);
        assert!(log.borrow().registered.is_empty());
    }

    #[test]
    fn default_never_starts_a_real_hook() {
        let mut gh = GlobalHotkeys::default();
        let mut want = BTreeMap::new();
        want.insert("go".to_string(), "Ctrl+Alt+KeyG".to_string());
        gh.sync(&egui::Context::default(), &want, false);
        assert!(gh.active().is_empty());
        assert!(matches!(gh.status("go"), Status::Off(_)));
    }
}
