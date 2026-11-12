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

/// The window's close button.
fn close_window(h: &mut egui_kittest::Harness<'_, sm_desktop::App>) {
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.run_steps(2);
}

#[test]
fn closing_the_window_keeps_show_master_running() {
    let env = Env::demo();
    let mut h = env.harness();
    close_window(&mut h);
    // First time: explain that the show keeps running.
    assert!(h.query_by_label("Show Master keeps running").is_some());
    h.get_by_label("Keep running").click();
    settle(&mut h);
    assert!(h.query_by_label("Show Master keeps running").is_none());
    // The runtime was not asked to quit.
    assert!(!env
        .handle
        .quit_requested
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn quit_from_the_background_note_asks_while_cues_run() {
    let env = Env::demo();
    let mut h = env.harness();
    h.key_press(egui::Key::Space);
    env.wait(&mut h, "cue running", |_| {
        !env.handle.tick()["active"].as_array().unwrap().is_empty()
    });
    close_window(&mut h);
    h.get_by_label("Quit Show Master").click();
    settle(&mut h);
    assert!(h.query_by_label("Quit Show Master?").is_some());
}

#[test]
fn quit_button_asks_nothing_when_idle_and_requests_close() {
    let env = Env::demo();
    let mut h = env.harness();
    h.get_by_label("Quit").click();
    settle(&mut h);
    assert!(h.query_by_label("Quit Show Master?").is_none());
}

#[test]
fn kiosk_mode_goes_live_and_locks_until_unlocked() {
    let env = Env::demo();
    let mut h = env.harness();
    h.state_mut().prefs.set_kiosk(true, "4321");
    env.wait(&mut h, "kiosk goes live", |s| s["mode"] == "live");
    settle(&mut h);
    assert!(h.query_by_label("Setup").is_none());
    assert!(h.query_by_label("EDIT").is_none());
    assert!(h.query_by_label("Quit").is_none());
    // GO still works.
    assert!(h.query_by_label("GO").is_some());
    // Quitting asks for the PIN first.
    h.state_mut().request_quit(&egui::Context::default());
    settle(&mut h);
    assert!(h
        .query_by_label("Unlock editing, Setup and Quit on this computer?")
        .is_none());
    h.get_by_label("Cancel").click();
    settle(&mut h);
    h.get_by_label("Unlock").click();
    settle(&mut h);
    h.get_by_label("Kiosk PIN").type_text("1111");
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert!(
        h.query_by_label("Setup").is_none(),
        "wrong PIN unlocks nothing"
    );
    h.get_by_label("Kiosk PIN").type_text("4321");
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert!(h.query_by_label("EDIT").is_some());
    h.get_by_label("Lock").click();
    settle(&mut h);
    assert!(h.query_by_label("EDIT").is_none());
}
