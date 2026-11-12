//! Setup > MIDI input (listen, mapping, learn) and Setup > Timecode source
//! (chase MTC / LTC), with fake MIDI ports and a virtual audio input.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use sm_audio::input::ManualInputBackend;
use sm_core::model::Show;
use sm_desktop::model::TcInfo;
use sm_desktop::App;
use sm_io::midi_in::{FakeMidiInput, FakeMidiInputFeed};
use sm_server::runtime::{self, Config};
use std::time::{Duration, Instant};

/// A runtime with fake MIDI inputs and a virtual audio input; four memo cues.
fn env() -> (Env, FakeMidiInputFeed) {
    let mut show = Show::default();
    show.settings.audio.backend = Some("null".into());
    show.settings.go_debounce_ms = 0;
    for n in 1..=4 {
        let id = show.alloc_id();
        show.cue_lists[0].cues.push(sm_core::model::Cue {
            id,
            number: n.to_string(),
            name: format!("Cue {n}"),
            ..Default::default()
        });
    }
    let (midi, feed) = FakeMidiInput::new(&["Pad", "Keys"]);
    let (audio, _feed) = ManualInputBackend::new("Line In", 2, 48_000);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (handle, _t) = runtime::spawn(Config {
        show: Some(show),
        midi_backend: Some(Box::new(sm_io::midi::NoMidi)),
        midi_input_backend: Some(Box::new(midi)),
        audio_input_backend: Some(Box::new(audio)),
        ..Config::default()
    });
    let dir = tempfile::tempdir().unwrap();
    (Env { rt, handle, dir }, feed)
}

fn open_setup(h: &mut Harness<'static, App>, tab: &str) {
    h.get_by_label("Setup").click();
    settle(h);
    h.get_by_label(tab).click();
    settle(h);
}

/// Pick `option` in the combo box labelled `label`.
fn choose(h: &mut Harness<'static, App>, label: &str, option: &str) {
    h.get_by_label(label).scroll_to_me();
    settle(h);
    h.get_by_label(label).click();
    settle(h);
    // Long lists scroll inside the popup.
    h.get_by_label(option).scroll_to_me();
    settle(h);
    h.get_by_label(option).click();
    settle(h);
}

fn wait_port(feed: &FakeMidiInputFeed, port: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !feed.is_open(port) {
        assert!(Instant::now() < deadline, "{port} not opened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn playhead_number(state: &serde_json::Value) -> Option<String> {
    let list = &state["show"]["cue_lists"][0];
    let head = state["playheads"][list["id"].to_string()].as_u64()?;
    list["cues"]
        .as_array()?
        .iter()
        .find(|c| c["id"] == head)
        .and_then(|c| c["number"].as_str().map(String::from))
}

#[test]
fn add_a_mapping_with_learn_and_use_it() {
    let (env, feed) = env();
    let mut h = env.harness();
    open_setup(&mut h, "MIDI");
    assert!(h.query_by_label("No MIDI received yet").is_some());
    assert!(h
        .query_by_label_contains("No mappings: incoming MIDI does nothing")
        .is_some());

    choose(&mut h, "Listen to", "All ports");
    env.wait(&mut h, "listen all", |s| {
        s["show"]["midi_input"]["mode"] == "all"
    });
    wait_port(&feed, "Pad");

    h.get_by_label("Add mapping").scroll_to_me();
    settle(&mut h);
    h.get_by_label("Add mapping").click();
    env.wait(&mut h, "mapping added", |s| {
        s["show"]["midi_input"]["mappings"].as_array().map(Vec::len) == Some(1)
    });

    // Learn: the next message fills the trigger.
    h.get_by_label("Learn").click();
    settle(&mut h);
    assert!(h.query_by_label("Waiting for MIDI... (cancel)").is_some());
    feed.send("Pad", &[0xB2, 21, 127]);
    env.wait(&mut h, "learned", |s| {
        let m = &s["show"]["midi_input"]["mappings"][0];
        m["kind"] == "control_change"
            && m["channel"] == 3
            && m["number"] == 21
            && m["port"] == "Pad"
    });
    assert!(h.query_by_label("Learn").is_some());
    assert!(h.query_by_label_contains("CC ch 3 #21 = 127").is_some());

    // Action: playhead down.
    choose(&mut h, "Mapping 1 action", "Next (playhead down)");
    env.wait(&mut h, "action", |s| {
        s["show"]["midi_input"]["mappings"][0]["action"] == "next"
    });
    assert_eq!(playhead_number(&env.state()).as_deref(), Some("1"));
    feed.send("Pad", &[0xB2, 21, 0]);
    feed.send("Pad", &[0xB2, 21, 127]);
    env.wait(&mut h, "playhead moved", |s| {
        playhead_number(s).as_deref() == Some("2")
    });
    // Another port / channel does nothing.
    feed.send("Keys", &[0xB0, 21, 0]);
    feed.send("Keys", &[0xB0, 21, 127]);
    std::thread::sleep(Duration::from_millis(150));
    settle(&mut h);
    assert_eq!(playhead_number(&env.state()).as_deref(), Some("2"));

    // Go-to-cue needs a number; an unknown one is flagged.
    choose(&mut h, "Mapping 1 action", "Go to cue (playhead)");
    env.wait(&mut h, "goto", |s| {
        s["show"]["midi_input"]["mappings"][0]["action"] == "goto_number"
    });
    h.get_by_label("Mapping 1 cue number").click();
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_label("Mapping 1 cue number").type_text("9");
    h.key_press(Key::Enter);
    env.wait(&mut h, "cue 9", |s| {
        s["show"]["midi_input"]["mappings"][0]["cue_number"] == "9"
    });
    assert!(h.query_by_label("no such cue").is_some());

    // The only "Remove" on the tab (no aliases, no peers).
    h.get_by_label("Remove").click();
    env.wait(&mut h, "removed", |s| {
        s["show"]["midi_input"]["mappings"].as_array().map(Vec::len) == Some(0)
    });
}

#[test]
fn msc_receive_settings() {
    let (env, _feed) = env();
    let mut h = env.harness();
    open_setup(&mut h, "MIDI");
    h.get_by_label("Answer MIDI Show Control").click();
    env.wait(&mut h, "msc on", |s| {
        s["show"]["midi_input"]["msc"]["enabled"] == true
    });
    choose(&mut h, "MSC format", "All types (7F)");
    env.wait(&mut h, "format", |s| {
        s["show"]["midi_input"]["msc"]["command_format"] == 127
    });
}

#[test]
fn choose_an_external_ltc_timecode_source() {
    let (env, _feed) = env();
    let mut h = env.harness();
    open_setup(&mut h, "Timecode");
    assert!(h.query_by_label("Input").is_none());
    choose(&mut h, "Timecode source", "External (chase MTC / LTC)");
    env.wait(&mut h, "external", |s| {
        s["show"]["timecode"]["source"] == "external"
    });
    env.wait(&mut h, "no signal", |s| {
        s["inputs"]["tc_status"] == "no_signal"
    });
    assert!(h.query_by_label("No signal").is_some());
    assert!(h.query_by_label("MTC input port").is_some());

    choose(&mut h, "Input", "LTC (audio input)");
    env.wait(&mut h, "ltc", |s| {
        s["show"]["timecode"]["input"]["kind"] == "ltc"
    });
    env.wait(&mut h, "devices", |s| {
        s["inputs"]["audio_inputs"][0]["id"] == "Line In"
    });
    choose(&mut h, "Audio input", "Line In (2 ch)");
    env.wait(&mut h, "device", |s| {
        s["show"]["timecode"]["input"]["ltc_device"] == "Line In"
    });
    env.wait(&mut h, "ltc open", |s| s["inputs"]["ltc_open"] == true);
    h.get_by_label("Freewheel").click();
    settle(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.get_by_label("Freewheel").type_text("3000");
    h.key_press(Key::Enter);
    env.wait(&mut h, "freewheel", |s| {
        s["show"]["timecode"]["input"]["freewheel_ms"] == 3000
    });
    // The live clock shows the source and its state.
    env.wait(&mut h, "tick", |_| {
        env.handle.tick()["tc"]["source"] == "ltc"
    });
    let tc: TcInfo = serde_json::from_value(env.handle.tick()["tc"].clone()).unwrap();
    let (text, _, _) = sm_desktop::live::clocks::tc_source_badge(&tc).unwrap();
    assert_eq!(text, "LTC NO SIGNAL");
    assert!(sm_desktop::live::clocks::tc_source_badge(&TcInfo::default()).is_none());

    // Back to the generator.
    choose(&mut h, "Timecode source", "Internal generator");
    env.wait(&mut h, "generator", |s| {
        s["show"]["timecode"]["source"] == "generator" && s["inputs"]["tc_status"] == "generator"
    });
}
