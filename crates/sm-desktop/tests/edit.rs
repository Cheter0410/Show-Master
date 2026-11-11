//! Edit mode: cue table, add row and inspector against a real runtime.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use sm_desktop::App;
use serde_json::Value;
use sm_core::model::CueList;
use sm_desktop::edit::table::{move_command, DropPos, DropTarget};

/// Top-level cues of the first list: (id, number, name, type).
fn cues(env: &Env) -> Vec<(u64, String, String, String)> {
    env.state()["show"]["cue_lists"][0]["cues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["id"].as_u64().unwrap(),
                c["number"].as_str().unwrap().to_string(),
                c["name"].as_str().unwrap().to_string(),
                c["kind"]["type"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn id_of(env: &Env, number: &str) -> u64 {
    cues(env)
        .into_iter()
        .find(|c| c.1 == number)
        .map(|c| c.0)
        .unwrap()
}

/// A harness that draws missing glyphs as boxes instead of panicking.
/// Needed once the show is dirty: the shell's title marker (`app.rs`) uses a
/// glyph the default fonts lack. The edit view's own glyphs are checked with
/// the panicking policy in `every_inspector_tab_draws_with_the_default_fonts`.
fn harness(env: &Env) -> Harness<'static, App> {
    let h = env.harness();
    h.ctx
        .set_missing_glyph_policy(egui::MissingGlyphPolicy::Tofu);
    h
}

fn find_cue(s: &Value, id: u64) -> Option<Value> {
    fn rec(cues: &[Value], id: u64) -> Option<Value> {
        for c in cues {
            if c["id"].as_u64() == Some(id) {
                return Some(c.clone());
            }
            if let Some(ch) = c["kind"]["children"].as_array() {
                if let Some(f) = rec(ch, id) {
                    return Some(f);
                }
            }
        }
        None
    }
    s["show"]["cue_lists"]
        .as_array()?
        .iter()
        .find_map(|l| rec(l["cues"].as_array()?, id))
}

#[test]
fn selecting_a_row_shows_the_inspector() {
    let env = Env::demo();
    let mut h = harness(&env);
    assert!(h.query_by_label("Cue name").is_none());
    h.get_by_label("Cue 7 Note").click();
    settle(&mut h);
    assert_eq!(h.state().view.selection, vec![id_of(&env, "7")]);
    h.get_by_label("Cue name");
    h.get_by_label("Basic tab");
    h.get_by_label("Timing tab");
    // Clicking the empty space below the rows clears the selection.
    let rect = h.get_by_label("Cue 7 Note").rect();
    h.hover_at(rect.center() + egui::vec2(0.0, 120.0));
    h.drag_at(rect.center() + egui::vec2(0.0, 120.0));
    h.drop_at(rect.center() + egui::vec2(0.0, 120.0));
    settle(&mut h);
    assert!(h.state().view.selection.is_empty());
    assert!(h.query_by_label("Cue name").is_none());
}

#[test]
fn renaming_in_the_inspector_updates_the_show() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "1");
    h.get_by_label("Cue 1 Intro").click();
    settle(&mut h);
    h.get_by_label("Cue name").click();
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("Cue name").type_text("Opening");
    settle(&mut h);
    // Nothing is sent while typing.
    assert_eq!(find_cue(&env.state(), id).unwrap()["name"], "Intro");
    h.key_press(egui::Key::Enter);
    env.wait(&mut h, "renamed", |s| {
        find_cue(s, id).is_some_and(|c| c["name"] == "Opening")
    });
    h.get_by_label("Cue 1 Opening");
}

#[test]
fn invalid_timecode_trigger_is_not_sent() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "4");
    h.get_by_label("Cue 4 Long").click();
    settle(&mut h);
    h.get_by_label("Timing tab").click();
    settle(&mut h);
    h.get_by_label("TC trigger time").click();
    settle(&mut h);
    h.get_by_label("TC trigger time").type_text("99:00:00:00");
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert!(find_cue(&env.state(), id).unwrap()["tc_trigger"].is_null());
    h.get_by_label("TC trigger time").click();
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("TC trigger time").type_text("01:00:10:00");
    h.key_press(egui::Key::Enter);
    env.wait(&mut h, "tc trigger set", |s| {
        find_cue(s, id).is_some_and(|c| c["tc_trigger"] == "01:00:10:00")
    });
}

#[test]
fn add_row_adds_a_memo_after_the_selection_and_selects_it() {
    let env = Env::demo();
    let mut h = harness(&env);
    let before = cues(&env).len();
    h.get_by_label("Cue 2 Fade intro").click();
    settle(&mut h);
    h.get_by_label("Add memo cue").click();
    env.wait(&mut h, "memo added", |s| {
        s["show"]["cue_lists"][0]["cues"].as_array().unwrap().len() == before + 1
    });
    settle(&mut h);
    let list = cues(&env);
    assert_eq!(list[2].3, "memo", "inserted after the selected cue");
    assert_eq!(h.state().view.selection, vec![list[2].0]);
}

#[test]
fn delete_hotkey_removes_the_selected_cue() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "7");
    h.get_by_label("Cue 7 Note").click();
    settle(&mut h);
    h.key_press(egui::Key::Delete);
    env.wait(&mut h, "cue deleted", |s| find_cue(s, id).is_none());
    assert!(h.query_by_label("Cue 7 Note").is_none());
}

#[test]
fn routing_cell_click_toggles_a_crosspoint() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "1");
    h.get_by_label("Cue 1 Intro").click();
    settle(&mut h);
    h.get_by_label("Routing tab").click();
    settle(&mut h);
    // Default routing L->L, R->R; add L->R.
    h.get_by_label("Route L to R").click();
    env.wait(&mut h, "routing set", |s| {
        find_cue(s, id).is_some_and(|c| {
            let r = c["kind"]["routing"].as_array().cloned().unwrap_or_default();
            r.len() == 3 && r.iter().any(|p| p["src"] == 0 && p["out"] == 1 && p["db"] == 0.0)
        })
    });
    // And off again.
    h.get_by_label("Route L to R").click();
    env.wait(&mut h, "crosspoint removed", |s| {
        find_cue(s, id).is_some_and(|c| c["kind"]["routing"].as_array().unwrap().len() == 2)
    });
    h.get_by_label("Default routing").click();
    env.wait(&mut h, "default routing", |s| {
        find_cue(s, id).is_some_and(|c| c["kind"]["routing"].as_array().unwrap().is_empty())
    });
}

#[test]
fn gutter_click_sets_the_playhead() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "4");
    // Children of groups cannot be the playhead: no gutter button for them.
    assert!(h.query_by_label("Set playhead to Bed").is_none());
    h.get_by_label("Set playhead to 4 Long").click();
    env.wait(&mut h, "playhead moved", |s| {
        s["playheads"].as_object().unwrap().values().any(|v| v.as_u64() == Some(id))
    });
}

#[test]
fn armed_checkbox_disarms_the_cue() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "5");
    h.get_by_label("Armed 5 Start TC").click();
    env.wait(&mut h, "disarmed", |s| {
        find_cue(s, id).is_some_and(|c| c["armed"] == false)
    });
}

#[test]
fn groups_collapse_and_ctrl_click_selects_several() {
    let env = Env::demo();
    let mut h = harness(&env);
    h.get_by_label("Cue Bed");
    h.get_by_label("Collapse group 3 Scene").click();
    settle(&mut h);
    assert!(h.query_by_label("Cue Bed").is_none());
    h.get_by_label("Expand group 3 Scene").click();
    settle(&mut h);
    h.get_by_label("Cue Bed");

    h.get_by_label("Cue 1 Intro").click();
    settle(&mut h);
    h.get_by_label("Cue 4 Long")
        .click_modifiers(egui::Modifiers::COMMAND);
    settle(&mut h);
    assert_eq!(h.state().view.selection.len(), 2);
    h.get_by_label("2 cues selected");
    // Bulk edit: disarm both.
    let (a, b) = (id_of(&env, "1"), id_of(&env, "4"));
    h.get_by_label("Disarm all").click();
    env.wait(&mut h, "both disarmed", |s| {
        [a, b]
            .iter()
            .all(|id| find_cue(s, *id).is_some_and(|c| c["armed"] == false))
    });
}

#[test]
fn dragging_a_row_reorders_the_list() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "7");
    let from = h.get_by_label("Cue 7 Note").rect().center();
    let to = h.get_by_label("Cue 1 Intro").rect();
    let target = egui::pos2(from.x, to.top() + 3.0);
    h.hover_at(from);
    h.step();
    h.drag_at(from);
    h.step();
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        h.hover_at(from + (target - from) * t);
        h.step();
    }
    h.drop_at(target);
    env.wait(&mut h, "cue moved to the top", |s| {
        s["show"]["cue_lists"][0]["cues"][0]["id"].as_u64() == Some(id)
    });
}

#[test]
fn move_command_semantics() {
    let env = Env::demo();
    let list: CueList =
        serde_json::from_value(env.state()["show"]["cue_lists"][0].clone()).unwrap();
    let group = id_of(&env, "3");
    let note = id_of(&env, "7");
    let intro = id_of(&env, "1");
    // Into a group: appended to its children.
    let v = move_command(&list, &[note], DropTarget::Row(group, DropPos::Into))
        .unwrap()
        .unwrap();
    assert_eq!(v["parent"], group);
    assert_eq!(v["index"], 2);
    // After the first cue at top level.
    let v = move_command(&list, &[note], DropTarget::Row(intro, DropPos::After))
        .unwrap()
        .unwrap();
    assert_eq!(v["parent"], Value::Null);
    assert_eq!(v["index"], 1);
    // A group cannot go into its own child.
    let bed = list.cues[2].children()[0].id;
    assert!(move_command(&list, &[group], DropTarget::Row(bed, DropPos::After)).is_err());
    // Send it and check the server agrees.
    let mut h = harness(&env);
    let v = move_command(&list, &[note], DropTarget::Row(group, DropPos::Into))
        .unwrap()
        .unwrap();
    h.state_mut().cmd(v);
    env.wait(&mut h, "moved into group", |s| {
        let g = &s["show"]["cue_lists"][0]["cues"][2];
        g["kind"]["children"][2]["id"].as_u64() == Some(note)
    });
}

#[test]
fn fade_tab_shows_target_and_level() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "2");
    h.get_by_label("Cue 2 Fade intro").click();
    settle(&mut h);
    h.get_by_label("Fade tab").click();
    settle(&mut h);
    h.get_by_label("Fade duration");
    // Uncheck "fade level": the level becomes "unchanged" (null).
    h.get_by_label("Fade changes level").click();
    env.wait(&mut h, "level unchanged", |s| {
        find_cue(s, id).is_some_and(|c| c["kind"]["gain_db"].is_null())
    });
}

#[test]
fn every_inspector_tab_draws_with_the_default_fonts() {
    // Panicking glyph policy (kittest default): any glyph the bundled fonts
    // lack fails the test. Nothing here makes the show dirty.
    let env = Env::demo();
    let mut h = env.harness();
    for row in [
        "Cue 1 Intro",
        "Cue 2 Fade intro",
        "Cue 3 Scene",
        "Cue Desk note",
        "Cue 5 Start TC",
        "Cue 6 Stop Long",
        "Cue 7 Note",
    ] {
        h.get_by_label(row).click();
        settle(&mut h);
        for tab in ["Basic tab", "Timing tab", "Audio tab", "Routing tab", "Fade tab", "MIDI tab", "Timecode tab", "Control tab", "Group tab"] {
            if let Some(t) = h.query_by_label(tab) {
                t.click();
                settle(&mut h);
            }
        }
        h.get_by_label(row).click_secondary();
        settle(&mut h);
        h.key_press(egui::Key::Escape);
        settle(&mut h);
    }
    // Waveform finished loading (or failed) without trouble.
    h.get_by_label("Cue 1 Intro").click();
    settle(&mut h);
    h.get_by_label("Audio tab").click();
    for _ in 0..20 {
        settle(&mut h);
    }
    h.get_by_label("Waveform");
}
