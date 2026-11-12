//! Per-machine preferences of the desktop window (not part of the show file):
//! kept in `desktop.json` next to `recent.json` in the configuration folder.
//! A touch laptop and the studio desktop editing the same show can each have
//! their own setting.

use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    touch: TouchPref,
}

pub struct Prefs {
    file: Option<PathBuf>,
    pub touch: TouchPref,
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
        let Ok(text) = serde_json::to_string_pretty(&Stored { touch: self.touch }) else {
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
}
