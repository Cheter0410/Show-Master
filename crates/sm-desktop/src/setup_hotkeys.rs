//! Setup tab "Hotkeys": one key combination per action, captured from the
//! keyboard, with conflict detection. A second, per-machine list holds the
//! system-wide hotkeys (see `global_hotkeys.rs`).

use super::{banner, heading, hint, SetupState};
use crate::app::{App, ACTIONS};
use crate::global_hotkeys::{self as global, Edit, Status};
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

pub use crate::global_hotkeys::format_combo;

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
    if global::panel(ctx).capture.is_some() {
        capture_global(ctx);
        return;
    }
    let Some(action) = app.setup.capture.clone() else {
        return;
    };
    let Some((key, mods)) = take_key_press(ctx) else {
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

/// Take the first key press of this frame out of the input (with all other
/// key and text events, so nothing else reacts to them).
fn take_key_press(ctx: &egui::Context) -> Option<(Key, egui::Modifiers)> {
    let mut pressed: Option<(Key, egui::Modifiers)> = None;
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                // Modifier keys alone are part of the combination being typed.
                if pressed.is_none() && !is_modifier_key(*key) {
                    pressed = Some((*key, *modifiers));
                }
                false
            }
            egui::Event::Key { .. } | egui::Event::Text(_) => false,
            _ => true,
        });
    });
    pressed
}

/// Shift, Ctrl, Alt or Super pressed on its own (egui reports these as keys).
fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// Capture for the system-wide list: plain keys and conflicts are refused.
fn capture_global(ctx: &egui::Context) {
    let Some((key, mods)) = take_key_press(ctx) else {
        return;
    };
    let mut p = global::panel(ctx);
    let Some(action) = p.capture.take() else {
        return;
    };
    if key == Key::Escape && !mods.any() {
        global::store_panel(ctx, p);
        return;
    }
    let result = match App::combo(key, mods) {
        Some(combo) => global::assign(&p.combos, &action, &combo).map_err(|r| r.text(&combo)),
        None => Err(format!(
            "The key {} cannot be used as a hotkey.",
            key.name()
        )),
    };
    match result {
        Ok(next) => {
            if let Some(combo) = next.get(&action) {
                p.edits.push(Edit::Set(action.clone(), combo.clone()));
            }
            p.combos = next;
            p.status.insert(action, Status::Paused);
            p.warning = None;
        }
        Err(text) => p.warning = Some((action, text)),
    }
    global::store_panel(ctx, p);
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
    global_section(ui, st, s);
}

/// "System-wide" list: per machine (preferences), opt-in, combinations with
/// a modifier only.
fn global_section(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let ctx = ui.ctx().clone();
    let mut p = global::panel(&ctx);
    p.drawn_pass = ctx.cumulative_pass_nr();
    ui.add_space(14.0);
    ui.separator();
    heading(
        ui,
        "System-wide (work when Show Master is in the background)",
    );
    hint(
        ui,
        "Stored on this computer only, not in the show. Off until you set a key. Each \
         combination needs Ctrl, Alt or Cmd / Super and is taken away from every other \
         application while Show Master runs.",
    );
    hint(ui, global::platform_hint());
    hint(ui, global::HIDDEN_NOTE);
    if let Some(reason) = &p.unsupported {
        banner(ui, theme::WARN, reason);
    }
    if let Some((_, text)) = &p.warning {
        banner(ui, theme::WARN, &format!("Warning: {text}"));
    }
    ui.add_space(4.0);
    egui::Grid::new("global_hotkeys_grid")
        .num_columns(4)
        .striped(true)
        .spacing([20.0, 6.0])
        .min_col_width(120.0)
        .show(ui, |ui| {
            for h in ["Action", "System-wide key", "Status", ""] {
                ui.label(RichText::new(h).color(theme::MUTED));
            }
            ui.end_row();
            for action in global::GLOBAL_ACTIONS {
                let label = global::action_label(action);
                let combo = p.combos.get(action).cloned();
                let warn = p.warning.as_ref().is_some_and(|(a, _)| a == action);
                let mut name = RichText::new(label);
                if warn {
                    name = name.color(theme::WARN);
                }
                ui.label(name);
                let capturing = p.capture.as_deref() == Some(action);
                if capturing {
                    ui.label(
                        RichText::new("Press a combination…  (Esc cancels)")
                            .strong()
                            .color(theme::LOADED),
                    );
                } else if let Some(c) = &combo {
                    ui.label(RichText::new(format_combo(c)).monospace().strong());
                } else {
                    ui.label(RichText::new("not set").color(theme::MUTED));
                }
                let failure = status_label(ui, combo.as_deref(), p.status.get(action), s, action);
                ui.horizontal(|ui| {
                    if capturing {
                        if ui
                            .button("Cancel")
                            .on_hover_text(format!(
                                "Stop waiting for a system-wide key for {label}"
                            ))
                            .clicked()
                        {
                            p.capture = None;
                        }
                    } else {
                        let set = ui
                            .button("Set…")
                            .on_hover_text(format!("Set the system-wide key for {label}"));
                        if set.clicked() {
                            set.surrender_focus();
                            p.warning = None;
                            p.capture = Some(action.to_string());
                            st.capture = None;
                            st.hotkey_warning = None;
                        }
                        if ui
                            .add_enabled(combo.is_some(), egui::Button::new("Clear"))
                            .on_hover_text(format!("Remove the system-wide key of {label}"))
                            .clicked()
                        {
                            p.combos.remove(action);
                            p.edits.push(Edit::Clear(action.to_string()));
                            p.warning = None;
                        }
                    }
                });
                ui.end_row();
                // Failures are written out, not only shown on hover.
                if let Some(text) = failure {
                    ui.label("");
                    ui.label(RichText::new(text).small().color(theme::WARN));
                    ui.end_row();
                }
            }
        });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let failed = p.status.values().any(|s| matches!(s, Status::Failed(_)));
        if ui
            .add_enabled(failed, egui::Button::new("Try again"))
            .on_hover_text("Register the combinations that failed again")
            .clicked()
        {
            p.edits.push(Edit::Retry);
        }
        if ui
            .add_enabled(
                !p.combos.is_empty(),
                egui::Button::new("Remove all system-wide keys"),
            )
            .clicked()
        {
            p.combos.clear();
            p.edits.push(Edit::ClearAll);
            p.capture = None;
            p.warning = None;
        }
    });
    if st.capture.is_some() {
        p.capture = None;
    }
    global::store_panel(&ctx, p);
}

/// Status cell; returns the failure reason to write out below the row.
fn status_label(
    ui: &mut egui::Ui,
    combo: Option<&str>,
    status: Option<&Status>,
    s: &UiState,
    action: &str,
) -> Option<String> {
    let Some(combo) = combo else {
        ui.label(RichText::new("-").color(theme::MUTED));
        return None;
    };
    let (text, color, tip) = match status {
        Some(Status::Active) => {
            // Same combination as a window hotkey of another action: the
            // system-wide one wins (the system takes the key first).
            let n = App::normalize_combo(combo);
            let shadowed = s
                .show
                .hotkeys
                .iter()
                .find(|(k, a)| App::normalize_combo(k) == n && a.as_str() != action);
            match shadowed {
                Some((_, other)) => (
                    "active".to_string(),
                    theme::WARN,
                    Some(format!(
                        "Also the window key of \"{}\" in this show; the system-wide action wins.",
                        global::action_label(other)
                    )),
                ),
                None => ("active".to_string(), theme::LOADED, None),
            }
        }
        Some(Status::Failed(e)) => ("not registered".to_string(), theme::WARN, Some(e.clone())),
        Some(Status::Paused) | None => ("paused".to_string(), theme::MUTED, None),
        Some(Status::Off(reason)) => ("off".to_string(), theme::MUTED, Some(reason.clone())),
    };
    let r = ui.label(RichText::new(text).color(color));
    let tip = tip?;
    r.on_hover_text(tip.clone());
    matches!(status, Some(Status::Failed(_))).then_some(tip)
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
