//! Setup tab "Hotkeys": one key combination per action, captured from the
//! keyboard, with conflict detection.

use super::{banner, heading, hint, SetupState};
use crate::app::{App, ACTIONS};
use crate::model::UiState;
use crate::theme;
use egui::{Key, RichText};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

fn action_label(action: &str) -> &str {
    ACTIONS
        .iter()
        .find(|(a, _)| *a == action)
        .map_or(action, |(_, l)| l)
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

/// Bind `combo` to `action`, replacing the action's previous key(s).
/// Returns the conflicting action when the combination is taken.
pub fn assign(
    hotkeys: &BTreeMap<String, String>,
    action: &str,
    combo: &str,
) -> Result<BTreeMap<String, String>, String> {
    let n = App::normalize_combo(combo);
    if let Some((_, other)) = hotkeys
        .iter()
        .find(|(k, a)| a.as_str() != action && App::normalize_combo(k) == n)
    {
        return Err(other.clone());
    }
    let mut next: BTreeMap<String, String> = hotkeys
        .iter()
        .filter(|(k, a)| a.as_str() != action && App::normalize_combo(k) != n)
        .map(|(k, a)| (k.clone(), a.clone()))
        .collect();
    next.insert(n, action.to_string());
    Ok(next)
}

/// Merge patch turning `old` into `new` (removed keys are sent as null).
pub fn diff_patch(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Value {
    let mut p = Map::new();
    for k in old.keys() {
        if !new.contains_key(k) {
            p.insert(k.clone(), Value::Null);
        }
    }
    for (k, v) in new {
        if old.get(k) != Some(v) {
            p.insert(k.clone(), Value::String(v.clone()));
        }
    }
    Value::Object(p)
}

fn commit(st: &mut SetupState, old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) {
    let patch = diff_patch(old, new);
    if patch.as_object().is_some_and(|p| !p.is_empty()) {
        st.update_show(serde_json::json!({ "hotkeys": patch }));
    }
}

fn try_assign(st: &mut SetupState, s: &UiState, action: &str, combo: &str) {
    match assign(&s.show.hotkeys, action, combo) {
        Ok(next) => {
            commit(st, &s.show.hotkeys, &next);
            st.hotkey_warning = None;
        }
        Err(other) => {
            st.hotkey_warning = Some((
                action.to_string(),
                format!(
                    "{} is already used by \"{}\". Not assigned - clear that binding first.",
                    format_combo(combo),
                    action_label(&other)
                ),
            ));
        }
    }
    st.capture = None;
}

/// While an action waits for its key: take the next key press out of the
/// input (so nothing else reacts to it) and assign it. Escape cancels.
pub(super) fn capture_keys(app: &mut App, ctx: &egui::Context) {
    let Some(action) = app.setup.capture.clone() else {
        return;
    };
    let mut pressed: Option<(Key, egui::Modifiers)> = None;
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if pressed.is_none() {
                    pressed = Some((*key, *modifiers));
                }
                false
            }
            egui::Event::Key { .. } | egui::Event::Text(_) => false,
            _ => true,
        });
    });
    let Some((key, mods)) = pressed else {
        return;
    };
    if key == Key::Escape && !mods.any() {
        app.setup.capture = None;
        return;
    }
    match App::combo(key, mods) {
        Some(combo) => {
            let st = &mut app.setup;
            try_assign(st, &app.client.state, &action, &combo);
            super::flush(app);
        }
        None => {
            app.setup.hotkey_warning = Some((
                action,
                format!("The key {} cannot be used as a hotkey.", key.name()),
            ));
            app.setup.capture = None;
        }
    }
}

pub(super) fn hotkeys_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let hotkeys = &s.show.hotkeys;
    heading(ui, "Hotkeys");
    hint(
        ui,
        "Hotkeys are stored in the show file. Click \"Set…\" and press the new key combination.",
    );
    if let Some((_, text)) = &st.hotkey_warning {
        banner(ui, theme::WARN, &format!("Warning: {text}"));
    }
    ui.add_space(4.0);
    egui::Grid::new("hotkeys_grid")
        .num_columns(3)
        .striped(true)
        .spacing([20.0, 6.0])
        .min_col_width(170.0)
        .show(ui, |ui| {
            for h in ["Action", "Key", ""] {
                ui.label(RichText::new(h).color(theme::MUTED));
            }
            ui.end_row();
            for (action, label) in ACTIONS {
                let keys: Vec<&String> = hotkeys
                    .iter()
                    .filter(|(_, a)| a.as_str() == action)
                    .map(|(k, _)| k)
                    .collect();
                let warn = st.hotkey_warning.as_ref().is_some_and(|(a, _)| a == action);
                let mut name = RichText::new(label);
                if warn {
                    name = name.color(theme::WARN);
                }
                ui.label(name);
                let capturing = st.capture.as_deref() == Some(action);
                if capturing {
                    ui.label(
                        RichText::new("Press a key…  (Esc cancels)")
                            .strong()
                            .color(theme::LOADED),
                    );
                } else if keys.is_empty() {
                    ui.label(RichText::new("not set").color(theme::MUTED));
                } else {
                    let text: Vec<String> = keys.iter().map(|k| format_combo(k)).collect();
                    ui.label(RichText::new(text.join(", ")).monospace().strong());
                }
                ui.horizontal(|ui| {
                    if capturing {
                        if ui
                            .button("Cancel")
                            .on_hover_text(format!("Stop waiting for a key for {label}"))
                            .clicked()
                        {
                            st.capture = None;
                        }
                        if ui
                            .button("Use Esc")
                            .on_hover_text(format!("Assign the Escape key to {label}"))
                            .clicked()
                        {
                            try_assign(st, s, action, "Escape");
                        }
                    } else {
                        let set = ui
                            .button("Set…")
                            .on_hover_text(format!("Set the key for {label}"));
                        if set.clicked() {
                            set.surrender_focus();
                            st.hotkey_warning = None;
                            st.capture = Some(action.to_string());
                        }
                        if ui
                            .add_enabled(!keys.is_empty(), egui::Button::new("Clear"))
                            .on_hover_text(format!("Remove the key of {label}"))
                            .clicked()
                        {
                            let next = hotkeys
                                .iter()
                                .filter(|(_, a)| a.as_str() != action)
                                .map(|(k, a)| (k.clone(), a.clone()))
                                .collect();
                            commit(st, hotkeys, &next);
                            st.hotkey_warning = None;
                        }
                    }
                });
                ui.end_row();
            }
        });
    ui.add_space(8.0);
    if ui.button("Reset to defaults").clicked() {
        commit(st, hotkeys, &sm_core::model::default_hotkeys());
        st.hotkey_warning = None;
        st.capture = None;
    }
    if cfg!(target_os = "macos") {
        hint(ui, "\"Cmd\" is shown for the command key; it is stored as Ctrl in the show file so shows work on every system.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let ctrl = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        assert_eq!(
            format_combo("Shift+Ctrl+KeyZ"),
            format!("{ctrl} + Shift + Z")
        );
        assert_eq!(format_combo("Escape"), "Esc");
        assert_eq!(format_combo("Digit1"), "1");
        assert_eq!(format_combo("ArrowDown"), "Down");
    }

    #[test]
    fn assigns_and_refuses_conflicts() {
        let map = sm_core::model::default_hotkeys();
        assert_eq!(assign(&map, "go", "KeyS"), Err("stop_all".to_string()));
        let next = assign(&map, "go", "KeyG").unwrap_or_default();
        assert_eq!(next.get("KeyG").map(String::as_str), Some("go"));
        assert!(!next.contains_key("Space"));
        // Re-assigning the same key to the same action is fine.
        assert!(assign(&map, "go", "Space").is_ok());
        let patch = diff_patch(&map, &next);
        assert_eq!(patch["Space"], Value::Null);
        assert_eq!(patch["KeyG"], "go");
        assert_eq!(patch.as_object().map(|p| p.len()), Some(2));
    }
}
