//! Live mode dashboard: transport, cue window, notes, master fader.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use serde_json::Value;
use sm_desktop::App;

/// Switch to Live mode through the UI (the null audio output makes the
/// pre-flight check ask).
fn go_live(env: &Env, h: &mut Harness<'static, App>) {
    h.get_by_label("LIVE").click();
    settle(h);
    if h.query_by_label("Go live anyway").is_some() {
        h.get_by_label("Go live anyway").click();
    }
    env.wait(h, "live mode", |s| s["mode"] == "live");
}

fn active(env: &Env) -> Vec<Value> {
    env.handle.tick()["active"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Id of the top-level cue with `number` in the first list.
fn cue_id(env: &Env, number: &str) -> u64 {
    env.state()["show"]["cue_lists"][0]["cues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["number"] == number)
        .and_then(|c| c["id"].as_u64())
        .unwrap()
}

fn playhead(s: &Value) -> Value {
    let list = s["show"]["cue_lists"][0]["id"].as_u64().unwrap();
    s["playheads"][list.to_string()].clone()
}

/// Two clicks in one frame (kittest queues each event in its own, slow frame).
fn double_click(h: &mut Harness<'static, App>, pos: egui::Pos2) {
    let events = &mut h.input_mut().events;
    events.push(egui::Event::PointerMoved(pos));
    for _ in 0..2 {
        for pressed in [true, false] {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
    }
    h.step();
}

#[test]
fn go_button_fires_the_next_cue_and_moves_the_playhead() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    let first = cue_id(&env, "1");
    assert_eq!(playhead(&env.state()), first);
    h.get_by_label("GO").click();
    env.wait(&mut h, "playhead moved", |s| playhead(s) != first);
    env.wait(&mut h, "cue running", |_| !active(&env).is_empty());
    assert!(active(&env).iter().any(|a| a["cue"] == first));
}

#[test]
fn pause_button_toggles_pause_all() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    h.get_by_label("GO").click();
    env.wait(&mut h, "cue running", |_| !active(&env).is_empty());
    h.get_by_label("Pause").click();
    env.wait(&mut h, "paused", |s| s["paused_all"] == true);
    // The button now offers to resume.
    h.get_by_label("Resume").click();
    env.wait(&mut h, "resumed", |s| s["paused_all"] == false);
}

#[test]
fn panic_stops_everything() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    h.get_by_label("GO").click();
    env.wait(&mut h, "cue running", |_| !active(&env).is_empty());
    h.get_by_label("PANIC").click();
    env.wait(&mut h, "all stopped", |_| active(&env).is_empty());
}

#[test]
fn double_click_on_a_row_sets_the_playhead_single_click_does_not() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    let first = cue_id(&env, "1");
    let long = cue_id(&env, "4");
    let row = h.get_by_label("Cue 4 Long");
    row.click();
    settle(&mut h);
    assert_eq!(
        playhead(&env.state()),
        first,
        "a single click must not move the playhead"
    );
    assert!(active(&env).is_empty(), "a click must never fire a cue");
    let pos = h.get_by_label("Cue 4 Long").rect().center();
    double_click(&mut h, pos);
    env.wait(&mut h, "playhead on cue 4", |s| playhead(s) == long);
    assert!(
        active(&env).is_empty(),
        "a double-click must not fire a cue"
    );
}

#[test]
fn prev_next_and_reset_with_confirmation() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    let first = cue_id(&env, "1");
    let second = cue_id(&env, "2");
    h.get_by_label("Next").click();
    env.wait(&mut h, "playhead down", |s| playhead(s) == second);
    h.get_by_label("Prev").click();
    env.wait(&mut h, "playhead up", |s| playhead(s) == first);
    h.get_by_label("GO").click();
    env.wait(&mut h, "cue running", |_| !active(&env).is_empty());
    h.get_by_label("Reset").click();
    settle(&mut h);
    // Nothing happens before the confirmation.
    assert!(!active(&env).is_empty());
    h.get_by_label("Reset show").click();
    env.wait(&mut h, "reset", |s| {
        playhead(s) == first && active(&env).is_empty()
    });
}

#[test]
fn notes_reach_the_server() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    h.get_by_label("Show notes").click();
    settle(&mut h);
    h.get_by_label("Show notes").type_text("Interval 20 min");
    settle(&mut h);
    // Sent after a short pause, without leaving the field.
    env.wait(&mut h, "notes saved", |s| {
        s["show"]["notes"] == "Interval 20 min"
    });
}

#[test]
fn master_fader_sets_the_master_level() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    h.get_by_label("Master level").set_value_accesskit(-10.0);
    env.wait(&mut h, "master at -10 dB", |s| {
        (s["master_db"].as_f64().unwrap() + 10.0).abs() < 0.01
    });
    // Bottom of the fader = silence.
    h.get_by_label("Master level").set_value_accesskit(-60.0);
    env.wait(&mut h, "master silent", |s| {
        s["master_db"].as_f64().unwrap() < -100.0
    });
}

#[test]
fn small_window_and_many_cues_render() {
    use sm_core::model::{Cue, CueKind, Show};
    let mut show = Show::default();
    let mut cues = Vec::new();
    for i in 0..600 {
        let id = show.alloc_id();
        cues.push(Cue {
            id,
            number: format!("{}", i + 1),
            name: format!("Cue number {}", i + 1),
            kind: CueKind::Memo,
            ..Cue::default()
        });
    }
    show.cue_lists[0].cues = cues;
    let env = Env::start(show);
    let mut h = env.harness_sized(egui::vec2(1024.0, 700.0));
    go_live(&env, &mut h);
    // Virtualised: only visible rows exist.
    assert!(h.query_by_label("Cue 1 Cue number 1").is_some());
    assert!(h.query_by_label("Cue 500 Cue number 500").is_none());
    // Following the playhead scrolls it into view.
    let target = env.state()["show"]["cue_lists"][0]["cues"][499]["id"]
        .as_u64()
        .unwrap();
    let list = env.state()["show"]["cue_lists"][0]["id"].as_u64().unwrap();
    env.rt.block_on(async {
        let cmd = sm_server::runtime::parse_command(
            serde_json::json!({ "cmd": "set_playhead", "cue": target, "list": list }),
        )
        .unwrap();
        env.handle
            .command(cmd, sm_server::runtime::Role::Admin, "test")
            .await
            .unwrap();
    });
    env.wait(&mut h, "playhead set", |s| playhead(s) == target);
    settle(&mut h);
    assert!(h.query_by_label("Cue 500 Cue number 500").is_some());
}

#[test]
fn panic_holds_the_timecode_and_the_clock_menu_resumes_it() {
    let env = Env::demo();
    let mut h = env.harness();
    go_live(&env, &mut h);
    let tc_running = |_: &Value| env.handle.tick()["tc"]["running"].as_bool() == Some(true);
    let tc_held = |_: &Value| env.handle.tick()["tc"]["running"].as_bool() == Some(false);
    // Start the generator through the clock menu (right-click / long-press).
    h.get_by_label("Timecode controls").click_secondary();
    settle(&mut h);
    h.get_by_label("Start from the beginning").click();
    env.wait(&mut h, "timecode running", tc_running);
    // PANIC holds it.
    h.get_by_label("PANIC").click();
    env.wait(&mut h, "timecode held by panic", tc_held);
    h.get_by_label("Timecode controls").click_secondary();
    settle(&mut h);
    h.get_by_label("Resume timecode").click();
    env.wait(&mut h, "timecode running again", tc_running);
}
