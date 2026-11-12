//! The Setup window: settings commit through `update_show`, layout
//! templates, hotkey capture with conflict check, remote URLs / QR codes,
//! timecode rate.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use sm_desktop::App;

fn open_setup(h: &mut Harness<'static, App>, tab: &str) {
    h.get_by_label("Setup").click();
    settle(h);
    assert!(h.state().setup.open);
    h.get_by_label(tab).click();
    settle(h);
}

/// Replace the text of a field and press Enter.
fn type_into(h: &mut Harness<'static, App>, label: &str, text: &str) {
    h.get_by_label(label).click();
    settle(h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_label(label).type_text(text);
    h.key_press(Key::Enter);
    settle(h);
}

#[test]
fn go_debounce_commits_on_enter() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "General");
    type_into(&mut h, "GO debounce", "400");
    env.wait(&mut h, "debounce updated", |s| {
        s["show"]["settings"]["go_debounce_ms"] == 400
    });
}

#[test]
fn applying_the_5_1_layout_gives_six_outputs() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "Outputs / Patch");
    h.get_by_label("5.1").click();
    settle(&mut h);
    // Nothing happens before the confirmation.
    assert_eq!(
        env.state()["show"]["patch"]["outputs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    h.get_by_label("Apply layout 5.1").click();
    env.wait(&mut h, "six outputs", |s| {
        s["show"]["patch"]["outputs"].as_array().map(Vec::len) == Some(6)
    });
    // The table shows them.
    assert!(h.query_by_label("Output 6 name").is_some());
}

#[test]
fn hotkey_assignment_refuses_conflicts() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "Hotkeys");
    // Rows follow ACTIONS: 0 = GO, 1 = Panic.
    h.get_all_by_label("Set…").next().unwrap().click();
    settle(&mut h);
    assert!(h.state().setup.capturing_hotkey());
    h.key_press(Key::G);
    env.wait(&mut h, "G is GO", |s| s["show"]["hotkeys"]["KeyG"] == "go");
    assert!(!h.state().setup.capturing_hotkey());
    assert!(env.state()["show"]["hotkeys"].get("Space").is_none());

    // The same key for Panic is refused with a warning naming GO.
    h.get_all_by_label("Set…").nth(1).unwrap().click();
    settle(&mut h);
    h.key_press(Key::G);
    settle(&mut h);
    assert!(!h.state().setup.capturing_hotkey());
    assert!(h
        .query_by_label_contains("is already used by \"GO\"")
        .is_some());
    let hk = &env.state()["show"]["hotkeys"];
    assert_eq!(hk["KeyG"], "go");
    assert_eq!(hk["Escape"], "panic");

    // Escape cancels a capture without assigning anything.
    h.get_all_by_label("Set…").nth(1).unwrap().click();
    settle(&mut h);
    h.key_press(Key::Escape);
    settle(&mut h);
    assert!(!h.state().setup.capturing_hotkey());
    assert_eq!(env.state()["show"]["hotkeys"]["Escape"], "panic");
    assert!(env.handle.tick()["active"].as_array().unwrap().is_empty());

    h.get_by_label("Reset to defaults").scroll_to_me();
    settle(&mut h);
    h.get_by_label("Reset to defaults").click();
    env.wait(&mut h, "defaults back", |s| {
        s["show"]["hotkeys"]["Space"] == "go" && s["show"]["hotkeys"].get("KeyG").is_none()
    });
}

#[test]
fn remote_tab_shows_urls_and_qr_codes() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "Remote");
    let urls = env.state()["remote"]["urls"].clone();
    match urls
        .as_array()
        .and_then(|u| u.first())
        .and_then(|u| u.as_str())
    {
        Some(url) => {
            let remote = format!("{url}/remote");
            assert!(h.query_all_by_label(&remote).count() >= 1);
            assert!(h.query_by_label(&format!("QR code {remote}")).is_some());
            assert!(h.query_by_label(&format!("QR code {url}/view")).is_some());
            assert!(h.state().setup.has_qr_code(&remote));
        }
        None => {
            assert!(h.query_by_label_contains("No network address").is_some());
        }
    }
    assert!(h
        .query_by_label_contains("after Show Master is restarted")
        .is_some());
}

#[test]
fn timecode_rate_change() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "Timecode");
    h.get_by_label("Frame rate").click();
    settle(&mut h);
    h.get_by_label("29.97 drop-frame").click();
    env.wait(&mut h, "rate 29.97df", |s| {
        s["show"]["timecode"]["rate"] == "29.97df"
    });
    // Invalid start offset: refused, value unchanged.
    type_into(&mut h, "Start offset", "01:00:00:45");
    assert_eq!(env.state()["show"]["timecode"]["start"], "01:00:00:00");
    type_into(&mut h, "Start offset", "02:00:00:00");
    env.wait(&mut h, "start 2h", |s| {
        s["show"]["timecode"]["start"] == "02:00:00:00"
    });
}

#[test]
fn settings_are_locked_in_live_mode() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "General");
    h.state_mut()
        .cmd(serde_json::json!({ "cmd": "set_mode", "mode": "live", "force": true }));
    env.wait(&mut h, "live", |s| s["mode"] == "live");
    assert!(h.query_by_label_contains("settings are locked").is_some());
}

#[test]
fn every_tab_draws() {
    let env = Env::demo();
    let mut h = env.harness();
    open_setup(&mut h, "Audio");
    for tab in [
        "Outputs / Patch",
        "MIDI",
        "Timecode",
        "Remote",
        "Hotkeys",
        "General",
        "About / Updates",
        "Audio",
    ] {
        h.get_by_label(tab).click();
        settle(&mut h);
    }
    assert!(h.query_by_label("Check for updates now").is_none());
    h.get_by_label("About / Updates").click();
    settle(&mut h);
    h.get_by_label("Check for updates now").click();
    settle(&mut h);
}

#[test]
fn panic_messages_are_added_edited_and_removed() {
    let env = Env::demo();
    let mut h = env.harness();
    h.get_by_label("Setup").click();
    settle(&mut h);
    h.get_by_label("General").click();
    settle(&mut h);
    h.get_by_label("Add OSC message").scroll_to_me();
    settle(&mut h);
    h.get_by_label("Add OSC message").click();
    env.wait(&mut h, "osc panic message", |s| {
        s["show"]["settings"]["panic_send"][0]["address"] == "/blackout"
    });
    h.get_by_label("Add MIDI message").click();
    env.wait(&mut h, "midi panic message", |s| {
        s["show"]["settings"]["panic_send"][1]["kind"] == "midi"
    });
    settle(&mut h);
    // Edit the MIDI message as text.
    h.get_all_by_value("pc 1 0").next().unwrap().scroll_to_me();
    settle(&mut h);
    h.get_all_by_value("pc 1 0").next().unwrap().click();
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_all_by_value("pc 1 0")
        .next()
        .unwrap()
        .type_text("cc 1 7 0");
    h.key_press(Key::Enter);
    settle(&mut h);
    env.wait(&mut h, "midi message edited", |s| {
        s["show"]["settings"]["panic_send"][1]["message"]["msg"] == "control_change"
    });
    h.get_all_by_label("Remove").next().unwrap().scroll_to_me();
    settle(&mut h);
    h.get_all_by_label("Remove").next().unwrap().click();
    env.wait(&mut h, "first removed", |s| {
        s["show"]["settings"]["panic_send"]
            .as_array()
            .unwrap()
            .len()
            == 1
    });
}
