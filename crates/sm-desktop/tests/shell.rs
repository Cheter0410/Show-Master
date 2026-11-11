//! The application shell: mode switch with pre-flight, hotkeys, quit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui_kittest::kittest::Queryable;

#[test]
fn switches_to_live_through_the_preflight_dialog_and_back() {
    let env = Env::demo();
    let mut h = env.harness();
    assert_eq!(env.state()["mode"], "edit");
    h.get_by_label("LIVE").click();
    settle(&mut h);
    // Null audio output -> pre-flight reports an error and asks.
    h.get_by_label("Go live anyway").click();
    env.wait(&mut h, "live mode", |s| s["mode"] == "live");
    h.get_by_label("EDIT").click();
    env.wait(&mut h, "edit mode", |s| s["mode"] == "edit");
}

#[test]
fn space_is_go_and_escape_is_panic() {
    let env = Env::demo();
    let mut h = env.harness();
    let before = env.state()["playheads"].clone();
    h.key_press(egui::Key::Space);
    env.wait(&mut h, "playhead moved", |s| s["playheads"] != before);
    env.wait(&mut h, "cue running", |_| {
        !env.handle.tick()["active"].as_array().unwrap().is_empty()
    });
    h.key_press(egui::Key::Escape);
    env.wait(&mut h, "all stopped", |_| {
        env.handle.tick()["active"].as_array().unwrap().is_empty()
    });
}

#[test]
fn quit_button_asks_nothing_when_idle_and_requests_close() {
    let env = Env::demo();
    let mut h = env.harness();
    h.get_by_label("Quit").click();
    settle(&mut h);
    assert!(h.query_by_label("Quit Show Master?").is_none());
}
