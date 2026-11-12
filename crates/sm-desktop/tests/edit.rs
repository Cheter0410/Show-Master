//! Edit mode: cue table, add row and inspector against a real runtime.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{settle, Env};
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use serde_json::Value;
use sm_core::model::CueList;
use sm_desktop::edit::table::{move_command, DropPos, DropTarget};
use sm_desktop::App;

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

fn harness(env: &Env) -> Harness<'static, App> {
    env.harness()
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
            r.len() == 3
                && r.iter()
                    .any(|p| p["src"] == 0 && p["out"] == 1 && p["db"] == 0.0)
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
        s["playheads"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| v.as_u64() == Some(id))
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
    // kittest panics on any glyph the bundled fonts lack.
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
        for tab in [
            "Basic tab",
            "Timing tab",
            "Audio tab",
            "Routing tab",
            "Fade tab",
            "MIDI tab",
            "Timecode tab",
            "Control tab",
            "Group tab",
        ] {
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

#[test]
fn double_click_on_the_name_edits_inline_and_escape_cancels() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "7");
    let row = h.get_by_label("Cue 7 Note").rect();
    // Name column starts after gutter, status, number and type columns.
    let p = egui::pos2(
        row.left() + 52.0 + 26.0 + 58.0 + 74.0 + 30.0,
        row.center().y,
    );
    let double_click = |h: &mut Harness<'static, App>| {
        // Explicit input times: the harness otherwise reports none, and
        // egui needs them to recognise a double click.
        let t0 = h.ctx.input(|i| i.time) + 1.0;
        h.hover_at(p);
        h.step();
        for (n, pressed) in [true, false, true, false].into_iter().enumerate() {
            let input = h.input_mut();
            input.time = Some(t0 + n as f64 * 0.05);
            input.events.push(egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        settle(h);
    };
    double_click(&mut h);
    h.get_by_label("Inline cue name").type_text("Changed");
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    assert!(h.query_by_label("Inline cue name").is_none());
    assert_eq!(find_cue(&env.state(), id).unwrap()["name"], "Note");
    double_click(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("Inline cue name").type_text("Standby");
    h.key_press(egui::Key::Enter);
    env.wait(&mut h, "renamed inline", |s| {
        find_cue(s, id).is_some_and(|c| c["name"] == "Standby")
    });
}

#[test]
fn lists_can_be_added_renamed_and_switched() {
    let env = Env::demo();
    let mut h = harness(&env);
    h.get_by_label("Lists").click();
    settle(&mut h);
    h.get_by_label("New cue list…").click();
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("List name").type_text("Interval");
    settle(&mut h);
    h.get_by_label("Create").click();
    env.wait(&mut h, "list added", |s| {
        s["show"]["cue_lists"].as_array().unwrap().len() == 2
    });
    settle(&mut h);
    // The new (empty) list is shown, with tabs for both lists.
    let new_list = env.state()["show"]["cue_lists"][1]["id"].as_u64();
    assert_eq!(h.state().view.list, new_list);
    h.get_by_label("Cue list Interval");
    assert!(h.query_by_label("Cue 1 Intro").is_none());
    h.get_by_label("Cue list Main").click();
    settle(&mut h);
    h.get_by_label("Cue 1 Intro");
    // Rename through the tab's context menu.
    h.get_by_label("Cue list Interval").click_secondary();
    settle(&mut h);
    h.get_by_label("Rename…").click();
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("List name").type_text("Act 2");
    h.key_press(egui::Key::Enter);
    env.wait(&mut h, "list renamed", |s| {
        s["show"]["cue_lists"][1]["name"] == "Act 2"
    });
}

#[test]
fn fade_routing_levels_can_be_added_and_copied_from_the_target() {
    let env = Env::demo();
    let mut h = harness(&env);
    let id = id_of(&env, "2");
    h.get_by_label("Cue 2 Fade intro").click();
    settle(&mut h);
    h.get_by_label("Fade tab").click();
    settle(&mut h);
    h.get_by_label("Add level").click();
    env.wait(&mut h, "one crosspoint", |s| {
        find_cue(s, id).is_some_and(|c| {
            c["kind"]["crosspoints"]
                .as_array()
                .is_some_and(|a| a.len() == 1)
        })
    });
    h.get_by_label("Start from target's routing").click();
    env.wait(&mut h, "target routing copied", |s| {
        // Stereo audio target -> two outputs in the default stereo patch.
        find_cue(s, id).is_some_and(|c| {
            c["kind"]["crosspoints"]
                .as_array()
                .is_some_and(|a| a.len() == 2)
        })
    });
    h.get_by_label("Clear").click();
    env.wait(&mut h, "cleared", |s| {
        find_cue(s, id).is_some_and(|c| {
            c["kind"]["crosspoints"]
                .as_array()
                .is_some_and(|a| a.is_empty())
        })
    });
}

#[test]
fn stepping_the_playhead_in_edit_mode_selects_that_cue() {
    let env = Env::demo();
    let mut h = harness(&env);
    let second = id_of(&env, "2");
    h.key_press(egui::Key::ArrowDown);
    env.wait(&mut h, "playhead on cue 2", |s| {
        s["playheads"]
            .as_object()
            .is_some_and(|p| p.values().any(|v| v.as_u64() == Some(second)))
    });
    settle(&mut h);
    assert_eq!(h.state().view.selection, vec![second]);
}

#[test]
fn cues_can_be_deleted_with_the_toolbar_button_and_backspace() {
    let env = Env::demo();
    let mut h = harness(&env);
    let seven = id_of(&env, "7");
    let five = id_of(&env, "5");
    // Toolbar button.
    h.get_by_label("Cue 7 Note").click();
    settle(&mut h);
    h.get_by_label("Delete selected cues").click();
    env.wait(&mut h, "cue 7 gone", |s| find_cue(s, seven).is_none());
    // Backspace (the Mac "delete" key), not in the show's hotkey list.
    h.get_by_label("Cue 5 Start TC").click();
    settle(&mut h);
    h.key_press(egui::Key::Backspace);
    env.wait(&mut h, "cue 5 gone", |s| find_cue(s, five).is_none());
    // Inspector button.
    let four = id_of(&env, "4");
    h.get_by_label("Cue 4 Long").click();
    settle(&mut h);
    h.get_by_label("Delete this cue").click();
    env.wait(&mut h, "cue 4 gone", |s| find_cue(s, four).is_none());
}

#[test]
fn osc_cue_is_added_and_edited_in_the_inspector() {
    let env = Env::demo();
    let mut h = harness(&env);
    let before = cues(&env).len();
    h.get_by_label("Add osc cue").click();
    env.wait(&mut h, "osc added", |s| {
        s["show"]["cue_lists"][0]["cues"].as_array().unwrap().len() == before + 1
    });
    settle(&mut h);
    let id = *h.state().view.selection.first().unwrap();
    h.get_by_label("OSC tab").click();
    settle(&mut h);
    for (label, text) in [
        ("OSC address, e.g. /cue/1/start", "/cue/5/start"),
        ("Separated by spaces: 12 = int, 0.5 = float, true/false, text = string, \"in quotes\" for spaces", "5 0.5 \"two words\""),
        ("UDP port the receiver listens on", "53000"),
    ] {
        h.get_by_label(label).click();
        settle(&mut h);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label(label).type_text(text);
        h.key_press(egui::Key::Enter);
        settle(&mut h);
    }
    env.wait(&mut h, "osc edited", |s| {
        find_cue(s, id).is_some_and(|c| {
            let k = &c["kind"];
            k["address"] == "/cue/5/start"
                && k["port"] == 53000
                && k["args"] == serde_json::json!([5, 0.5, "two words"])
        })
    });
    // An address without "/" is refused.
    let label = "OSC address, e.g. /cue/1/start";
    h.get_by_label(label).click();
    settle(&mut h);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label(label).type_text("oops");
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    assert_eq!(
        find_cue(&env.state(), id).unwrap()["kind"]["address"],
        "/cue/5/start"
    );
}
