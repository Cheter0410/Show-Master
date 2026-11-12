//! System-wide hotkeys: the Setup section, refusal of plain keys, conflict
//! check, registration failures, and presses running the action once. The
//! platform hook is replaced by a fake, so no display / X server is needed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use serde_json::Value;
use sm_desktop::app::ACTIONS;
use sm_desktop::global_hotkeys::{self as global, Backend, GlobalHotkeys};
use sm_desktop::App;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Log {
    registered: Vec<String>,
    refuse: Vec<String>,
    pressed: Vec<String>,
}

struct Fake(Arc<Mutex<Log>>);

impl Backend for Fake {
    fn register(&mut self, combo: &str) -> Result<(), String> {
        let mut l = self.0.lock().unwrap();
        if l.refuse.iter().any(|c| c == combo) {
            return Err("already used by another application or by the system".into());
        }
        l.registered.push(combo.to_string());
        Ok(())
    }
    fn unregister(&mut self, combo: &str) {
        self.0.lock().unwrap().registered.retain(|c| c != combo);
    }
    fn take_pressed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().pressed)
    }
}

fn playhead(s: &Value) -> Value {
    let list = s["show"]["cue_lists"][0]["id"].as_u64().unwrap();
    s["playheads"][list.to_string()].clone()
}

fn cue_id(env: &Env, number: &str) -> Value {
    env.state()["show"]["cue_lists"][0]["cues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["number"] == number)
        .map(|c| c["id"].clone())
        .unwrap()
}

/// "Set…" button of the system-wide row of `action` (after the window rows).
fn set_button(h: &mut Harness<'static, App>, action: &str) {
    let row = global::GLOBAL_ACTIONS
        .iter()
        .position(|a| *a == action)
        .unwrap();
    let n = ACTIONS.len() + row;
    h.get_all_by_label("Set…").nth(n).unwrap().scroll_to_me();
    settle(h);
    h.get_all_by_label("Set…").nth(n).unwrap().click();
    settle(h);
}

fn setup(env: &Env) -> (Harness<'static, App>, Arc<Mutex<Log>>) {
    let log = Arc::new(Mutex::new(Log::default()));
    let mut h = env.harness();
    h.state_mut().global_hotkeys = GlobalHotkeys::with_backend(Box::new(Fake(log.clone())));
    h.get_by_label("Setup").click();
    settle(&mut h);
    h.get_by_label("Hotkeys").click();
    settle(&mut h);
    (h, log)
}

#[test]
fn setup_section_refuses_plain_keys_and_conflicts() {
    let env = Env::demo();
    let (mut h, log) = setup(&env);
    assert!(h
        .query_by_label("System-wide (work when Show Master is in the background)")
        .is_some());
    // Opt-in: nothing configured, nothing grabbed.
    assert!(h.state().prefs.global_hotkeys.is_empty());
    assert!(log.lock().unwrap().registered.is_empty());

    // A plain Space is refused.
    set_button(&mut h, "go");
    assert_eq!(global::panel(&h.ctx).capture.as_deref(), Some("go"));
    h.key_press(Key::Space);
    settle(&mut h);
    assert!(global::panel(&h.ctx).capture.is_none());
    assert!(h
        .query_by_label_contains("cannot be a system-wide key")
        .is_some());
    assert!(h.state().prefs.global_hotkeys.is_empty());
    // ... and so is Shift + a letter (would steal capitals system-wide).
    set_button(&mut h, "go");
    h.key_press_modifiers(Modifiers::SHIFT, Key::G);
    settle(&mut h);
    assert!(h.state().prefs.global_hotkeys.is_empty());

    // Ctrl+Alt+G is accepted, stored per machine and registered. The
    // modifier keys arrive first as key presses of their own: still waiting.
    set_button(&mut h, "go");
    h.key_press(Key::ControlLeft);
    h.key_press(Key::AltLeft);
    settle(&mut h);
    assert_eq!(global::panel(&h.ctx).capture.as_deref(), Some("go"));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::G);
    settle(&mut h);
    assert_eq!(
        h.state().prefs.global_hotkeys.get("go").map(String::as_str),
        Some("Ctrl+Alt+KeyG")
    );
    assert_eq!(log.lock().unwrap().registered, vec!["Ctrl+Alt+KeyG"]);
    assert!(h.query_by_label("active").is_some());
    // Nothing went into the show.
    assert!(env.state()["show"]["hotkeys"]
        .get("Ctrl+Alt+KeyG")
        .is_none());

    // The same combination for Panic is refused with a warning naming GO.
    set_button(&mut h, "panic");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::G);
    settle(&mut h);
    assert!(h
        .query_by_label_contains("already used system-wide by \"GO\"")
        .is_some());
    assert!(!h.state().prefs.global_hotkeys.contains_key("panic"));

    // Escape cancels a capture without assigning anything.
    set_button(&mut h, "panic");
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(global::panel(&h.ctx).capture.is_none());
    assert!(!h.state().prefs.global_hotkeys.contains_key("panic"));
    assert!(env.handle.tick()["active"].as_array().unwrap().is_empty());

    // A combination another application holds: shown, not fatal.
    log.lock().unwrap().refuse.push("Ctrl+Alt+KeyS".into());
    set_button(&mut h, "stop_all");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::S);
    settle(&mut h);
    assert!(h.query_by_label("not registered").is_some());
    assert!(h
        .query_by_label_contains("already used by another application")
        .is_some());
    assert_eq!(log.lock().unwrap().registered, vec!["Ctrl+Alt+KeyG"]);

    // Clear releases the grab.
    h.get_by_label("Remove all system-wide keys").scroll_to_me();
    settle(&mut h);
    h.get_by_label("Remove all system-wide keys").click();
    settle(&mut h);
    assert!(h.state().prefs.global_hotkeys.is_empty());
    assert!(log.lock().unwrap().registered.is_empty());
}

#[test]
fn a_system_wide_press_runs_the_action_once() {
    let env = Env::demo();
    let (mut h, log) = setup(&env);
    set_button(&mut h, "go");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::G);
    settle(&mut h);
    assert_eq!(log.lock().unwrap().registered, vec!["Ctrl+Alt+KeyG"]);
    h.state_mut().setup.open = false;
    settle(&mut h);
    assert!(!h.state().setup.open);

    let first = playhead(&env.state());
    assert_eq!(first, cue_id(&env, "1"));
    // The platform reports the press, and the focused window sees the same
    // key press: GO runs once (playhead on cue 2, not on the group).
    log.lock().unwrap().pressed.push("Ctrl+Alt+KeyG".into());
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::G);
    env.wait(&mut h, "GO from the system-wide key", |s| {
        playhead(s) != first
    });
    settle(&mut h);
    std::thread::sleep(std::time::Duration::from_millis(100));
    settle(&mut h);
    assert_eq!(playhead(&env.state()), cue_id(&env, "2"));

    // A later press works again.
    std::thread::sleep(global::DEDUPE);
    log.lock().unwrap().pressed.push("Ctrl+Alt+KeyG".into());
    env.wait(&mut h, "second GO", |s| playhead(s) == cue_id_in(s, "3"));
}

fn cue_id_in(s: &Value, number: &str) -> Value {
    s["show"]["cue_lists"][0]["cues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["number"] == number)
        .map(|c| c["id"].clone())
        .unwrap()
}
