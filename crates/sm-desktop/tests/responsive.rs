//! Small screens and touch: every essential control exists and lies inside
//! the window at all supported sizes; touch mode makes targets larger.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use sm_desktop::App;

const SIZES: [(f32, f32); 5] = [
    (1440.0, 900.0),
    (1280.0, 800.0),
    (1024.0, 700.0),
    (800.0, 600.0),
    (760.0, 540.0),
];

/// Assert that the control labelled `label` exists and lies inside the window.
fn on_screen(h: &Harness<'static, App>, size: egui::Vec2, label: &str) -> egui::Rect {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size).expand(0.5);
    let rect = h
        .query_by_label(label)
        .unwrap_or_else(|| panic!("{label:?} missing at {size:?}"))
        .rect();
    assert!(
        screen.contains_rect(rect),
        "{label:?} at {rect:?} is outside the {size:?} window"
    );
    assert!(
        rect.width() >= 8.0 && rect.height() >= 8.0,
        "{label:?} too small: {rect:?}"
    );
    rect
}

/// `on_screen`, then click it.
fn tap(h: &mut Harness<'static, App>, size: egui::Vec2, label: &str) {
    on_screen(h, size, label);
    h.get_by_label(label).click();
}

fn go_live(env: &Env, h: &mut Harness<'static, App>) {
    h.get_by_label("LIVE").click();
    settle(h);
    if h.query_by_label("Go live anyway").is_some() {
        h.get_by_label("Go live anyway").click();
    }
    env.wait(h, "live mode", |s| s["mode"] == "live");
}

fn cue_count(s: &serde_json::Value) -> usize {
    s["show"]["cue_lists"][0]["cues"].as_array().unwrap().len()
}

#[test]
fn edit_view_controls_fit_every_size() {
    for (w, hgt) in SIZES {
        let size = egui::vec2(w, hgt);
        let env = Env::demo();
        let mut h = env.harness_sized(size);
        for label in [
            "EDIT",
            "LIVE",
            "Quit",
            "Setup",
            "GO",
            "Stop all",
            "PANIC",
            "Delete selected cues",
        ] {
            on_screen(&h, size, label);
        }
        // Select a cue: the inspector opens and the table keeps some rows.
        h.get_by_label("Cue 1 Intro").click();
        settle(&mut h);
        on_screen(&h, size, "Delete this cue");
        on_screen(&h, size, "Cue 1 Intro");
        on_screen(&h, size, "Delete selected cues");
        on_screen(&h, size, "PANIC");
        // Adding a cue: a button on wide screens, an "Add cue" menu otherwise.
        let before = cue_count(&env.state());
        if h.query_by_label("Add memo cue").is_none() {
            tap(&mut h, size, "Add cue");
            settle(&mut h);
        }
        tap(&mut h, size, "Add memo cue");
        env.wait(&mut h, "memo added", |s| cue_count(s) == before + 1);
    }
}

#[test]
fn compact_tables_hide_minor_columns_but_keep_the_essentials() {
    let env = Env::demo();
    let mut h = env.harness_sized(egui::vec2(1440.0, 900.0));
    assert!(h.query_by_label("Armed 1 Intro").is_some());
    let mut h2 = env.harness_sized(egui::vec2(800.0, 600.0));
    // Armed moved to the inspector, the playhead gutter stays.
    assert!(h2.query_by_label("Armed 1 Intro").is_none());
    on_screen(
        &h2,
        egui::vec2(800.0, 600.0),
        "Set playhead to 2 Fade intro",
    );
    h2.get_by_label("Cue 1 Intro").click();
    settle(&mut h2);
    assert!(h2.query_by_label("Cue armed").is_some());
    settle(&mut h);
}

#[test]
fn inspector_folds_on_small_screens() {
    let env = Env::demo();
    let size = egui::vec2(1024.0, 700.0);
    let mut h = env.harness_sized(size);
    h.get_by_label("Cue 4 Long").click();
    settle(&mut h);
    tap(&mut h, size, "Hide inspector");
    settle(&mut h);
    assert!(h.query_by_label("Delete this cue").is_none());
    tap(&mut h, size, "Show inspector");
    settle(&mut h);
    on_screen(&h, size, "Delete this cue");
    // Large screens have no fold button: everything stays as it was.
    let mut big = env.harness();
    big.get_by_label("Cue 4 Long").click();
    settle(&mut big);
    assert!(big.query_by_label("Hide inspector").is_none());
}

#[test]
fn live_view_keeps_go_and_panic_reachable_at_every_size() {
    for (w, hgt) in SIZES {
        let size = egui::vec2(w, hgt);
        let env = Env::demo();
        let mut h = env.harness_sized(size);
        go_live(&env, &mut h);
        for label in [
            "GO", "PANIC", "Stop all", "Pause", "Prev", "Next", "Reset", "EDIT", "Quit",
        ] {
            on_screen(&h, size, label);
        }
        let go = h.get_by_label("GO").rect();
        assert!(go.height() >= 44.0, "GO too small at {size:?}: {go:?}");
        // The GO button really fires at this size.
        h.get_by_label("GO").click();
        env.wait(&mut h, "cue running", |_| {
            !env.handle.tick()["active"].as_array().unwrap().is_empty()
        });
        h.get_by_label("PANIC").click();
        env.wait(&mut h, "all stopped", |_| {
            env.handle.tick()["active"].as_array().unwrap().is_empty()
        });
    }
}

#[test]
fn narrow_live_view_shows_tools_as_tabs() {
    let env = Env::demo();
    let size = egui::vec2(800.0, 600.0);
    let mut h = env.harness_sized(size);
    go_live(&env, &mut h);
    // Folded by default: the master fader appears when the Meters tab opens.
    assert!(h.query_by_label("Master level").is_none());
    tap(&mut h, size, "Meters");
    settle(&mut h);
    on_screen(&h, size, "Master level");
    tap(&mut h, size, "Notes");
    settle(&mut h);
    on_screen(&h, size, "Show notes");
    on_screen(&h, size, "PANIC");
}

#[test]
fn setup_window_fits_small_screens() {
    for (w, hgt) in SIZES {
        let size = egui::vec2(w, hgt);
        let env = Env::demo();
        let mut h = env.harness_sized(size);
        h.get_by_label("Setup").click();
        settle(&mut h);
        tap(&mut h, size, "About / Updates");
        settle(&mut h);
        on_screen(&h, size, "Check for updates now");
        tap(&mut h, size, "General");
        settle(&mut h);
        on_screen(&h, size, "Touch-friendly controls");
    }
}

#[test]
fn narrow_top_bar_folds_file_actions_into_a_menu() {
    let env = Env::demo();
    let size = egui::vec2(800.0, 600.0);
    let mut h = env.harness_sized(size);
    assert!(h.query_by_label("File").is_none());
    tap(&mut h, size, "Menu");
    settle(&mut h);
    on_screen(&h, size, "Save as…");
    on_screen(&h, size, "Undo");
    on_screen(&h, size, "Go to cue number");
}

#[test]
fn touch_mode_makes_rows_and_buttons_larger() {
    let env = Env::demo();
    let size = egui::vec2(1280.0, 800.0);
    let mut h = env.harness_sized(size);
    let row = h.get_by_label("Cue 1 Intro").rect().height();
    let save = h.get_by_label("Save").rect().height();
    assert!(row < 40.0 && save < 40.0);
    assert!(h.query_by_label("More actions for 1 Intro").is_none());

    h.state_mut()
        .prefs
        .set_touch(sm_desktop::prefs::TouchPref::On);
    settle(&mut h);
    assert!(h.state().touch_mode());
    let row = h.get_by_label("Cue 1 Intro").rect().height();
    assert!(row >= 44.0, "touch row height {row}");
    for label in ["Save", "GO", "PANIC", "Setup", "Quit"] {
        let r = on_screen(&h, size, label);
        assert!(
            r.height() >= 43.5,
            "{label} is {} high in touch mode",
            r.height()
        );
    }
    // Touch has no right mouse button: the "More" button opens the row menu,
    // which also offers inline rename.
    tap(&mut h, size, "More actions for 1 Intro");
    settle(&mut h);
    h.get_by_label("Rename").click();
    settle(&mut h);
    assert!(h.query_by_label("Inline cue name").is_some());
    // Reordering has its own handle.
    on_screen(&h, size, "Move 1 Intro");
}

#[test]
fn a_touch_event_turns_touch_mode_on_automatically() {
    let env = Env::demo();
    let mut h = env.harness_sized(egui::vec2(1024.0, 700.0));
    assert!(!h.state().touch_mode());
    let pos = egui::pos2(400.0, 300.0);
    for phase in [egui::TouchPhase::Start, egui::TouchPhase::End] {
        h.input_mut().events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(1),
            id: egui::TouchId(1),
            phase,
            pos,
            force: None,
        });
        h.step();
    }
    settle(&mut h);
    assert!(h.state().touch_mode());
    let row = h.get_by_label("Cue 1 Intro").rect().height();
    assert!(row >= 44.0, "touch row height {row}");
    // "Off" wins over a detected touch screen.
    h.state_mut()
        .prefs
        .set_touch(sm_desktop::prefs::TouchPref::Off);
    settle(&mut h);
    assert!(!h.state().touch_mode());
}

#[test]
fn wide_but_short_screens_put_the_inspector_at_the_side() {
    let env = Env::demo();
    let size = egui::vec2(1366.0, 660.0);
    let mut h = env.harness_sized(size);
    h.get_by_label("Cue 4 Long").click();
    settle(&mut h);
    let delete = on_screen(&h, size, "Delete this cue");
    assert!(
        delete.left() > size.x / 2.0,
        "inspector not at the side: {delete:?}"
    );
    // The table keeps its full height: the last cue is still visible.
    on_screen(&h, size, "Cue 7 Note");
}
