//! Per-machine preferences of the desktop window (not part of the show file):
//! kept in `desktop.json` next to `recent.json` in the configuration folder.
//! A touch laptop and the studio desktop editing the same show can each have
//! their own setting.
//!
//! ```json
//! {
//!   "touch": "auto",
//!   "global_hotkeys": { "go": "Ctrl+Alt+KeyG", "panic": "Ctrl+Alt+Escape" }
//! }
//! ```
//!
//! `global_hotkeys` maps a hotkey action (see `app::ACTIONS`) to one
//! system-wide key combination in the format of `show.hotkeys`
//! (`Ctrl+Shift+Alt+Meta+<KeyboardEvent.code>`, "Ctrl" = Cmd on macOS).
//! Empty by default: system-wide hotkeys are opt-in per machine.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// "Touch-friendly controls" setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchPref {
    /// On as soon as a touch event was seen in this session.
    #[default]
    Auto,
    On,
    Off,
}

impl TouchPref {
    pub const ALL: [(TouchPref, &'static str); 3] = [
        (TouchPref::Auto, "Auto"),
        (TouchPref::On, "On"),
        (TouchPref::Off, "Off"),
    ];

    /// Touch mode for this setting, given whether a touch screen was used.
    pub fn resolve(self, touch_seen: bool) -> bool {
        match self {
            TouchPref::Auto => touch_seen,
            TouchPref::On => true,
            TouchPref::Off => false,
        }
    }
}

/// Window size and position (points, outer position) of the last session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub maximized: bool,
}

impl WindowGeometry {
    /// Sizes and positions a window could plausibly be restored to (a
    /// monitor that is gone again must not leave the window out of reach).
    pub fn sane(&self) -> bool {
        let finite = [self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite());
        finite
            && (300.0..=10_000.0).contains(&self.w)
            && (200.0..=10_000.0).contains(&self.h)
            && (-200.0..=8_000.0).contains(&self.x)
            && (-50.0..=6_000.0).contains(&self.y)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    touch: TouchPref,
    global_hotkeys: BTreeMap<String, String>,
    background_hint_seen: bool,
    window: Option<WindowGeometry>,
    open_last: bool,
    kiosk: bool,
    kiosk_pin: String,
}

pub struct Prefs {
    file: Option<PathBuf>,
    pub touch: TouchPref,
    /// System-wide hotkeys of this machine: action -> combination.
    pub global_hotkeys: BTreeMap<String, String>,
    /// The "keeps running in the background" note was confirmed once.
    pub background_hint_seen: bool,
    pub window: Option<WindowGeometry>,
    /// Open the most recent show when started without one.
    pub open_last: bool,
    /// Kiosk: start in Live mode with editing, Setup and Quit locked.
    pub kiosk: bool,
    /// PIN that unlocks kiosk mode ("" = unlock asks for confirmation only).
    pub kiosk_pin: String,
}

impl Default for Prefs {
    /// Not persisted (tests must not touch the user's settings).
    fn default() -> Self {
        Self::load_from(None)
    }
}

impl Prefs {
    pub fn load() -> Self {
        Self::load_from(crate::recent::config_dir().map(|d| d.join("desktop.json")))
    }

    pub fn load_from(file: Option<PathBuf>) -> Self {
        let stored = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
            .unwrap_or_default();
        Self {
            file,
            touch: stored.touch,
            global_hotkeys: stored.global_hotkeys,
            background_hint_seen: stored.background_hint_seen,
            window: stored.window.filter(WindowGeometry::sane),
            open_last: stored.open_last,
            kiosk: stored.kiosk,
            kiosk_pin: stored.kiosk_pin,
        }
    }

    /// Replace the system-wide hotkeys (saved when they changed).
    pub fn set_global_hotkeys(&mut self, map: BTreeMap<String, String>) {
        if self.global_hotkeys != map {
            self.global_hotkeys = map;
            self.save();
        }
    }

    pub fn set_background_hint_seen(&mut self) {
        if !self.background_hint_seen {
            self.background_hint_seen = true;
            self.save();
        }
    }

    /// Remember the window geometry (saved when it changed noticeably).
    pub fn set_window(&mut self, g: WindowGeometry) {
        if !g.sane() {
            return;
        }
        let same = self.window.is_some_and(|w| {
            w.maximized == g.maximized
                && (w.x - g.x).abs() < 2.0
                && (w.y - g.y).abs() < 2.0
                && (w.w - g.w).abs() < 2.0
                && (w.h - g.h).abs() < 2.0
        });
        if !same {
            self.window = Some(g);
            self.save();
        }
    }

    pub fn set_open_last(&mut self, on: bool) {
        if self.open_last != on {
            self.open_last = on;
            self.save();
        }
    }

    pub fn set_kiosk(&mut self, on: bool, pin: &str) {
        if self.kiosk != on || self.kiosk_pin != pin {
            self.kiosk = on;
            self.kiosk_pin = pin.to_string();
            self.save();
        }
    }

    pub fn set_touch(&mut self, touch: TouchPref) {
        if self.touch != touch {
            self.touch = touch;
            self.save();
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let stored = Stored {
            touch: self.touch,
            global_hotkeys: self.global_hotkeys.clone(),
            background_hint_seen: self.background_hint_seen,
            window: self.window,
            open_last: self.open_last,
            kiosk: self.kiosk,
            kiosk_pin: self.kiosk_pin.clone(),
        };
        let Ok(text) = serde_json::to_string_pretty(&stored) else {
            return;
        };
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_pref_persists_and_resolves() {
        let dir = std::env::temp_dir().join(format!("sm-prefs-{}", std::process::id()));
        let file = dir.join("desktop.json");
        let mut p = Prefs::load_from(Some(file.clone()));
        assert_eq!(p.touch, TouchPref::Auto);
        p.set_touch(TouchPref::On);
        assert_eq!(Prefs::load_from(Some(file)).touch, TouchPref::On);
        assert!(TouchPref::Auto.resolve(true));
        assert!(!TouchPref::Auto.resolve(false));
        assert!(TouchPref::On.resolve(false));
        assert!(!TouchPref::Off.resolve(true));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn window_geometry_persists_and_insane_values_are_ignored() {
        let dir = std::env::temp_dir().join(format!("sm-prefs-win-{}", std::process::id()));
        let file = dir.join("desktop.json");
        let mut p = Prefs::load_from(Some(file.clone()));
        let g = WindowGeometry {
            x: 100.0,
            y: 50.0,
            w: 1200.0,
            h: 800.0,
            maximized: false,
        };
        p.set_window(g);
        assert_eq!(Prefs::load_from(Some(file.clone())).window, Some(g));
        // A window on a monitor that is far away / garbage is not restored.
        p.set_window(WindowGeometry { x: 90_000.0, ..g });
        assert_eq!(Prefs::load_from(Some(file.clone())).window, Some(g));
        std::fs::write(
            &file,
            r#"{"window":{"x":-5000,"y":0,"w":800,"h":600,"maximized":false}}"#,
        )
        .unwrap();
        assert_eq!(Prefs::load_from(Some(file)).window, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn global_hotkeys_round_trip() {
        let dir = std::env::temp_dir().join(format!("sm-prefs-gh-{}", std::process::id()));
        let file = dir.join("desktop.json");
        let mut p = Prefs::load_from(Some(file.clone()));
        assert!(p.global_hotkeys.is_empty(), "opt-in: empty by default");
        let mut map = BTreeMap::new();
        map.insert("go".to_string(), "Ctrl+Alt+KeyG".to_string());
        p.set_touch(TouchPref::Off);
        p.set_global_hotkeys(map.clone());
        let back = Prefs::load_from(Some(file.clone()));
        assert_eq!(back.global_hotkeys, map);
        assert_eq!(back.touch, TouchPref::Off);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        assert_eq!(v["global_hotkeys"]["go"], "Ctrl+Alt+KeyG");
        // An older file without the key still loads.
        std::fs::write(&file, r#"{"touch":"on"}"#).unwrap_or_default();
        let old = Prefs::load_from(Some(file));
        assert_eq!(old.touch, TouchPref::On);
        assert!(old.global_hotkeys.is_empty());
        // Not persisted without a file.
        let mut mem = Prefs::default();
        mem.set_global_hotkeys(map);
        let _ = std::fs::remove_dir_all(dir);
    }
}
