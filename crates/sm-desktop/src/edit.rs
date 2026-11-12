//! Edit mode: add-cue row with list tabs and a mini transport, the cue table
//! (`edit_table.rs`) and the inspector (`edit_inspector.rs`, `edit_audio.rs`).
//!
//! Drawing only *borrows* the published state; everything the view wants to
//! change is collected as [`Out`] and applied after the frame was drawn, so
//! the show document is never cloned per frame. All edits are commands on the
//! runtime's command bus (`docs/remote-api.md`).

#[path = "edit_audio.rs"]
mod audio;
#[path = "edit_fields.rs"]
pub(crate) mod fields;
#[path = "edit_inspector.rs"]
mod inspector;
#[path = "edit_table.rs"]
pub mod table;

use crate::app::App;
use crate::{theme, util, widgets};
use egui::{Color32, Id, RichText};
use serde_json::{json, Value};
use sm_core::model::{ControlAction, Cue, CueKind, Show, TcAction};

/// Cue types offered in the add row (command name, button text).
pub const ADD_TYPES: [(&str, &str); 9] = [
    ("audio", "Audio…"),
    ("memo", "Memo"),
    ("fade", "Fade"),
    ("midi", "MIDI"),
    ("osc", "OSC"),
    ("http", "HTTP"),
    ("timecode", "Timecode"),
    ("control", "Control"),
    ("group", "Group"),
];

/// Commands that change the show document (rejected in live mode).
const EDIT_COMMANDS: [&str; 14] = [
    "add_cue",
    "update_cue",
    "update_cues",
    "delete_cues",
    "duplicate_cues",
    "paste_cues",
    "move_cues",
    "renumber",
    "group_cues",
    "add_list",
    "update_list",
    "delete_list",
    "update_show",
    "apply_layout",
];

#[derive(Default)]
pub struct EditState {
    pub(crate) table: table::TableState,
    pub(crate) insp: inspector::InspectorState,
    pub(crate) waves: audio::Waves,
    pub(crate) drafts: fields::Drafts,
    list_dialog: Option<ListDialog>,
}

enum ListDialog {
    New(String),
    Rename(u64, String),
    Delete(u64, String),
}

/// What the view wants done once drawing is finished.
pub(crate) enum Out {
    Cmd(Value),
    /// A command whose result names new cue(s) (`{cue}` / `{cues}`): select them.
    CmdSelect(Value),
    Error(String),
    /// Native dialog: choose the audio file of this cue.
    ChooseAudio(u64),
    /// Native dialog: add audio cues.
    AddAudio,
    ShowList(u64),
    /// Run an app action (delete, duplicate...) after drawing.
    Action(&'static str),
    /// Insert the cues of this template after the selection.
    InsertTemplate(String),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // The edit view is never drawn in live mode; this is just a second fence.
    if app.is_live() {
        return;
    }
    let mut out = Vec::new();
    app.edit.drafts.begin_frame();
    egui::Panel::top("edit_add_row")
        .frame(
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(4, 4))
                .fill(ui.visuals().panel_fill),
        )
        .show(ui, |ui| add_row(app, ui, &mut out));
    let Some(list) = app.current_list() else {
        ui.label("No cue list");
        return;
    };
    {
        let App {
            client, view, edit, ..
        } = &mut *app;
        let state = &client.state;
        let tick = &client.tick;
        let any_selected = view
            .selection
            .iter()
            .any(|id| state.show.cue(*id).is_some());
        // Inspector placement (see `responsive.rs` for the breakpoints):
        // - wide and tall: bottom panel, as always;
        // - wide but short: a side panel, so the table keeps its height;
        // - compact or short: bottom panel that can be folded to one line,
        //   never taller than the room left above a few table rows.
        let screen = crate::responsive::get(ui.ctx());
        let side = !screen.compact() && screen.short();
        edit.insp.collapsible = !side && (screen.compact() || screen.short());
        if !edit.insp.collapsible {
            edit.insp.collapsed = false;
        }
        let frame = egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .inner_margin(egui::Margin::symmetric(12, 8));
        if any_selected && side {
            let max = (ui.available_width() * 0.55).max(380.0);
            egui::Panel::right("edit_inspector_side")
                .resizable(true)
                .default_size(480.0_f32.min(max))
                .size_range(360.0..=max)
                .frame(frame)
                .show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    inspector::show(ui, state, tick, &view.selection, edit, &mut out);
                });
        } else if any_selected && edit.insp.collapsed {
            egui::Panel::bottom("edit_inspector_folded")
                .frame(frame)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let what = match view.selection.as_slice() {
                            [one] => state.show.cue(*one).map(cue_label).unwrap_or_default(),
                            many => format!("{} cues selected", many.len()),
                        };
                        ui.label(
                            RichText::new("INSPECTOR")
                                .small()
                                .strong()
                                .color(theme::MUTED),
                        );
                        ui.add(egui::Label::new(RichText::new(what).strong()).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button("Show inspector")
                                .on_hover_text("Unfold the inspector")
                                .clicked()
                            {
                                edit.insp.collapsed = false;
                            }
                        });
                    });
                });
        } else if any_selected {
            let avail = ui.available_height();
            let (default, range) = if edit.insp.collapsible {
                // Leave the table at least a header and three or four rows.
                let max = (avail - 170.0).max(150.0);
                ((avail * 0.5).clamp(150.0, max), 150.0..=max)
            } else {
                (340.0, 170.0..=760.0)
            };
            egui::Panel::bottom("edit_inspector")
                .resizable(true)
                .default_size(default)
                .size_range(range)
                .frame(frame)
                .show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    inspector::show(ui, state, tick, &view.selection, edit, &mut out);
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            table::show(ui, state, tick, list, &mut view.selection, edit, &mut out);
        });
        edit.drafts.end_frame(&mut out);
    }
    list_dialog(app, ui.ctx(), &mut out);
    apply(app, out);
}

/// Carry out what the view asked for.
fn apply(app: &mut App, outs: Vec<Out>) {
    for o in outs {
        match o {
            Out::Cmd(v) | Out::CmdSelect(v) if app.is_live() && is_edit(&v) => {
                app.toast_error(App::error_text("live_mode"));
            }
            Out::Cmd(v) => app.cmd(v),
            Out::CmdSelect(v) => app.cmd_then(v, |app, r| match r {
                Ok(v) => {
                    if let Some(id) = v["cue"].as_u64() {
                        app.view.selection = vec![id];
                        app.edit.table.scroll_to = Some(id);
                    } else if let Some(ids) = v["cues"].as_array() {
                        app.view.selection = ids.iter().filter_map(Value::as_u64).collect();
                        app.edit.table.scroll_to = app.view.selection.first().copied();
                    }
                }
                Err(e) => app.toast_error(App::error_text(&e)),
            }),
            Out::Error(e) => app.toast_error(e),
            Out::ChooseAudio(cue) => {
                if let Some(p) = app.pick_audio_file() {
                    app.cmd(json!({ "cmd": "update_cue", "cue": cue, "patch": { "kind": { "file": p } } }));
                }
            }
            Out::AddAudio => {
                let files = app.pick_audio_files();
                if !files.is_empty() {
                    app.add_audio_cues(files);
                }
            }
            Out::ShowList(id) => {
                app.view.list = Some(id);
                app.view.selection.clear();
            }
            Out::Action(a) => app.run_action(a),
            Out::InsertTemplate(name) => match crate::clipboard::load_template(&name) {
                Ok(text) => app.paste_cues(&text),
                Err(e) => app.toast_error(format!("Template \"{name}\": {e}")),
            },
        }
    }
}

fn is_edit(cmd: &Value) -> bool {
    cmd["cmd"]
        .as_str()
        .is_some_and(|c| EDIT_COMMANDS.contains(&c))
}

// ---------------------------------------------------------------------------
// Add-cue row, list tabs, mini transport
// ---------------------------------------------------------------------------

/// "Insert template" submenu (saved groups of cues).
pub(crate) fn template_menu(ui: &mut egui::Ui, out: &mut Vec<Out>) {
    ui.menu_button("Insert template", |ui| {
        let templates = crate::clipboard::templates();
        if templates.is_empty() {
            ui.label(
                RichText::new("No templates yet: right-click cues > Save as template")
                    .color(theme::MUTED),
            );
        }
        for name in &templates {
            if ui
                .button(name)
                .on_hover_text("Insert these cues after the selection")
                .clicked()
            {
                out.push(Out::InsertTemplate(name.clone()));
                ui.close();
            }
        }
    });
}

/// Add-cue buttons (one per type), or the same entries inside a menu.
fn add_buttons(app: &App, ui: &mut egui::Ui, out: &mut Vec<Out>, in_menu: bool) {
    for (kind, label) in ADD_TYPES {
        let hint = if kind == "audio" {
            "Add audio cues: choose one or more files".to_string()
        } else {
            format!("Add a {} cue after the selection", type_label_of(kind))
        };
        let resp = ui
            .add(egui::Button::new(label))
            .accessible_name(format!("Add {kind} cue"))
            .on_hover_text(hint);
        if resp.clicked() {
            if kind == "audio" {
                out.push(Out::AddAudio);
            } else {
                let list = app.current_list();
                let after = app.view.selection.last().copied();
                out.push(Out::CmdSelect(json!({
                    "cmd": "add_cue", "kind": kind, "list": list, "after": after,
                })));
            }
            if in_menu {
                ui.close();
            }
        }
    }
    if in_menu {
        ui.separator();
        template_menu(ui, out);
    }
}

/// Select (touch multi-select) / Duplicate / Delete / Lists.
fn selection_buttons(app: &mut App, ui: &mut egui::Ui, out: &mut Vec<Out>) {
    let has_sel = !app.view.selection.is_empty();
    let touch = crate::responsive::get(ui.ctx()).touch;
    let multi = app.edit.table.multi;
    if touch || multi {
        let n = app.view.selection.len();
        let text = if multi {
            format!("Done ({n})")
        } else {
            "Select".to_string()
        };
        if ui
            .add(egui::Button::new(text).selected(multi))
            .accessible_name("Select several cues")
            .on_hover_text("Tap cues to add them to the selection or remove them (like Ctrl-click)")
            .clicked()
        {
            app.edit.table.multi = !multi;
        }
    }
    if ui
        .add_enabled(has_sel, egui::Button::new("Duplicate"))
        .accessible_name("Duplicate selected cues")
        .on_hover_text("Duplicate the selected cues (Ctrl+D)")
        .clicked()
    {
        out.push(Out::Action("duplicate"));
    }
    if ui
        .add_enabled(
            has_sel,
            egui::Button::new(RichText::new("Delete").color(if has_sel {
                theme::ERROR
            } else {
                theme::MUTED
            })),
        )
        .accessible_name("Delete selected cues")
        .on_hover_text("Delete the selected cues (Delete / Backspace key, undo with Ctrl+Z)")
        .clicked()
    {
        out.push(Out::Action("delete"));
    }
    ui.separator();
    lists_menu(app, ui);
}

/// GO / Stop all / PANIC and the NEXT label, laid out right to left.
fn mini_transport(app: &mut App, ui: &mut egui::Ui, touch: bool) {
    let (h, extra) = if touch { (44.0, 16.0) } else { (30.0, 0.0) };
    widgets::panic_button(app, ui, egui::vec2(92.0 + extra, h));
    if ui
        .add_sized(
            [84.0 + extra, h],
            egui::Button::new(RichText::new("Stop all").strong()),
        )
        .on_hover_text("Stop all cues (with the stop fade)")
        .clicked()
    {
        app.stop_all();
    }
    widgets::go_button(app, ui, egui::vec2(72.0 + extra, h));
    next_label(app, ui);
}

fn add_row(app: &mut App, ui: &mut egui::Ui, out: &mut Vec<Out>) {
    let touch = crate::responsive::get(ui.ctx()).touch;
    // Fit check: the full row (every cue type as a button) needs the room
    // of a wide screen; otherwise the types fold into an "Add cue" menu, and
    // if even that does not fit the transport moves to a second row.
    let bw = |ui: &egui::Ui, t: &str| App::button_width(ui, t);
    let sel_w = bw(ui, "Duplicate")
        + bw(ui, "Delete")
        + bw(ui, "Lists")
        + if touch || app.edit.table.multi {
            bw(ui, "Done (99)")
        } else {
            0.0
        }
        + 16.0;
    let types_w: f32 = ADD_TYPES.iter().map(|(_, l)| bw(ui, l)).sum::<f32>();
    let transport_w = 248.0 + if touch { 48.0 } else { 0.0 } + 3.0 * 8.0;
    let avail = ui.available_width();
    // The full row keeps room for the "NEXT: cue" label; the folded row lets
    // it shrink (it truncates) before the transport moves down.
    let full = 70.0 + types_w + sel_w + transport_w + 140.0 <= avail;
    let one_row = full || bw(ui, "Add cue") + 20.0 + sel_w + transport_w + 40.0 <= avail;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if full {
            ui.label(RichText::new("Add cue").color(theme::MUTED));
            add_buttons(app, ui, out, false);
        } else {
            ui.menu_button("Add cue", |ui| add_buttons(app, ui, out, true))
                .response
                .on_hover_text("Add a cue after the selection");
        }
        ui.separator();
        selection_buttons(app, ui, out);
        if one_row {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                mini_transport(app, ui, touch);
            });
        }
    });
    if !one_row {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                mini_transport(app, ui, touch);
            });
        });
    }
    let lists = &app.client.state.show.cue_lists;
    if lists.len() > 1 {
        let current = app.current_list();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for l in lists {
                let on = current == Some(l.id);
                let resp = tab(ui, on, &l.name, &format!("Cue list {}", l.name))
                    .on_hover_text("Click to show this list · right-click to rename or delete");
                if resp.clicked() && !on {
                    out.push(Out::ShowList(l.id));
                }
                resp.context_menu(|ui| {
                    if ui.button("Rename…").clicked() {
                        app.edit.list_dialog = Some(ListDialog::Rename(l.id, l.name.clone()));
                        ui.close();
                    }
                    if ui.button("Delete…").clicked() {
                        app.edit.list_dialog = Some(ListDialog::Delete(l.id, l.name.clone()));
                        ui.close();
                    }
                });
            }
        });
    }
}

/// "Next: 3 Scene" left of the transport.
fn next_label(app: &App, ui: &mut egui::Ui) {
    let s = &app.client.state;
    let Some(list) = app.current_list() else {
        return;
    };
    let text = match s.playhead(list).and_then(|id| s.show.cue(id)) {
        Some(c) => cue_label(c),
        None => "end of list".into(),
    };
    // Only the room left of the transport (a narrow row drops the label).
    let room = ui.available_width() - 56.0;
    if room < 50.0 {
        return;
    }
    ui.scope(|ui| {
        ui.set_max_width(room);
        ui.add(egui::Label::new(RichText::new(text).strong()).truncate())
    })
    .inner
    .on_hover_text("The cue GO fires next (click the left gutter of a row to change it)");
    ui.label(
        RichText::new(" NEXT ")
            .small()
            .strong()
            .color(Color32::BLACK)
            .background_color(theme::NEXT),
    );
}

fn lists_menu(app: &mut App, ui: &mut egui::Ui) {
    let current = app.current_list();
    let lists = &app.client.state.show.cue_lists;
    let name = current
        .and_then(|c| lists.iter().find(|l| l.id == c))
        .map(|l| l.name.clone())
        .unwrap_or_default();
    let several = lists.len() > 1;
    let mut dialog = None;
    ui.menu_button("Lists", |ui| {
        if ui.button("New cue list…").clicked() {
            dialog = Some(ListDialog::New(format!("List {}", lists.len() + 1)));
        }
        if let Some(c) = current {
            if ui.button(format!("Rename \"{name}\"…")).clicked() {
                dialog = Some(ListDialog::Rename(c, name.clone()));
            }
            if ui
                .add_enabled(several, egui::Button::new(format!("Delete \"{name}\"…")))
                .on_disabled_hover_text("The last cue list cannot be deleted")
                .clicked()
            {
                dialog = Some(ListDialog::Delete(c, name.clone()));
            }
        }
    })
    .response
    .on_hover_text("Add, rename or delete cue lists");
    if dialog.is_some() {
        app.edit.list_dialog = dialog;
    }
}

fn list_dialog(app: &mut App, ctx: &egui::Context, out: &mut Vec<Out>) {
    let Some(mut dialog) = app.edit.list_dialog.take() else {
        return;
    };
    let mut keep = true;
    let resp = egui::Modal::new(Id::unique("edit_list_dialog")).show(ctx, |ui| {
        ui.set_width(360.0);
        let new = matches!(dialog, ListDialog::New(_));
        let rename = match dialog {
            ListDialog::Rename(id, _) => Some(id),
            _ => None,
        };
        match &mut dialog {
            ListDialog::New(name) | ListDialog::Rename(_, name) => {
                ui.heading(if new { "New cue list" } else { "Rename cue list" });
                let te = ui
                    .add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY))
                    .accessible_name("List name");
                if !te.has_focus() && !te.lost_focus() {
                    te.request_focus();
                }
                let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    let ok = !name.trim().is_empty();
                    if (ui
                        .add_enabled(ok, egui::Button::new(if new { "Create" } else { "Rename" }))
                        .clicked()
                        || enter)
                        && ok
                    {
                        let name = name.trim().to_string();
                        match rename {
                            None => {
                                let cmd = json!({ "cmd": "add_list", "name": name });
                                app.cmd_then(cmd, |app, r| match r {
                                    Ok(v) => {
                                        if let Some(id) = v["list"].as_u64() {
                                            app.view.list = Some(id);
                                            app.view.selection.clear();
                                        }
                                    }
                                    Err(e) => app.toast_error(App::error_text(&e)),
                                });
                            }
                            Some(id) => out.push(Out::Cmd(
                                json!({ "cmd": "update_list", "list": id, "patch": { "name": name } }),
                            )),
                        }
                        keep = false;
                    }
                });
            }
            ListDialog::Delete(id, name) => {
                ui.heading("Delete cue list?");
                ui.label(format!(
                    "\"{name}\" and all its cues will be deleted. Undo brings it back."
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Delete list").strong().color(Color32::WHITE),
                            )
                            .fill(theme::PANIC),
                        )
                        .clicked()
                    {
                        out.push(Out::Cmd(json!({ "cmd": "delete_list", "list": *id })));
                        keep = false;
                    }
                });
            }
        }
    });
    if resp.should_close() {
        keep = false;
    }
    if keep && app.edit.list_dialog.is_none() {
        app.edit.list_dialog = Some(dialog);
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// A flat tab: strong text and a blue underline when active.
pub(crate) fn tab(ui: &mut egui::Ui, on: bool, label: &str, name: &str) -> egui::Response {
    let text = if on {
        RichText::new(label)
            .strong()
            .color(ui.visuals().strong_text_color())
    } else {
        RichText::new(label).color(theme::MUTED)
    };
    let resp = ui
        .add(egui::Button::new(text).frame_when_inactive(false))
        .accessible_name(name);
    if on {
        ui.painter().hline(
            resp.rect.x_range().shrink(4.0),
            resp.rect.bottom() + 3.0,
            egui::Stroke::new(2.5, theme::EDIT),
        );
    }
    resp
}

pub fn type_label_of(kind: &str) -> &'static str {
    match kind {
        "audio" => "Audio",
        "fade" => "Fade",
        "midi" => "MIDI",
        "osc" => "OSC",
        "http" => "HTTP",
        "timecode" => "Timecode",
        "control" => "Control",
        "group" => "Group",
        _ => "Memo",
    }
}

pub fn type_label(cue: &Cue) -> &'static str {
    type_label_of(cue.type_name())
}

/// `12 Intro`, or the type name for an unnamed cue.
pub fn cue_label(cue: &Cue) -> String {
    let name = if cue.name.is_empty() {
        type_label(cue)
    } else {
        cue.name.as_str()
    };
    if cue.number.is_empty() {
        name.to_string()
    } else {
        format!("{} {}", cue.number, name)
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn target_label(show: &Show, id: Option<u64>) -> String {
    id.and_then(|t| show.cue(t))
        .map(cue_label)
        .unwrap_or_else(|| "(no target)".into())
}

/// Text of the "target / file" column.
pub fn target_text(cue: &Cue, show: &Show) -> String {
    match &cue.kind {
        CueKind::Audio(a) if a.file.is_empty() => "(no file)".into(),
        CueKind::Audio(a) => file_name(&a.file).to_string(),
        CueKind::Fade(f) => {
            let stop = if f.stop_target { " · then stop" } else { "" };
            match f.target {
                Some(_) => format!("{}{stop}", target_label(show, f.target)),
                None => "(no target)".into(),
            }
        }
        CueKind::Midi(m) => {
            let port = if m.port.is_empty() {
                "(no port)"
            } else {
                &m.port
            };
            format!("{port} · {}", inspector::midi_summary(&m.message))
        }
        CueKind::Osc(m) => m.summary(),
        CueKind::Http(r) => r.summary(),
        CueKind::Timecode(t) => match &t.tc {
            TcAction::Start => "TC start".into(),
            TcAction::StartFrom { from } => format!("TC start from {from}"),
            TcAction::Stop => "TC stop".into(),
            TcAction::Pause => "TC pause".into(),
            TcAction::Resume => "TC resume".into(),
            TcAction::Relocate { to } => format!("TC relocate to {to}"),
        },
        CueKind::Control(c) if c.action == ControlAction::Reset => "reset show".into(),
        CueKind::Control(c) => format!(
            "{}: {}",
            inspector::control_label(c.action).to_lowercase(),
            target_label(show, c.target)
        ),
        CueKind::Group(g) => format!(
            "{} · {} cue{}",
            inspector::group_mode_label(g.mode),
            g.children.len(),
            if g.children.len() == 1 { "" } else { "s" }
        ),
        CueKind::Memo => String::new(),
    }
}

/// Seconds for an editable field: `2.5`, `1:30.25`.
pub fn fmt_secs(v: f64) -> String {
    let v = (v.max(0.0) * 1000.0).round() / 1000.0;
    let trim = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if v < 60.0 {
        trim(format!("{v:.3}"))
    } else {
        let m = (v / 60.0).floor();
        let s = v - m * 60.0;
        let sec = trim(format!("{s:06.3}"));
        let sec = if sec.len() < 2 {
            format!("0{sec}")
        } else {
            sec
        };
        format!("{m}:{sec}")
    }
}

/// Parse a time field (`1.5`, `1:30`); empty = `None`.
pub fn parse_secs(text: &str) -> Result<Option<f64>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    util::parse_time(t)
        .map(Some)
        .ok_or_else(|| format!("'{t}' is not a time (use seconds or m:ss.s)"))
}

/// dB text: `-6`, `+3.5`, `-inf`/`off` = silence (-120).
pub fn parse_db(text: &str) -> Result<f32, String> {
    let t = text
        .trim()
        .trim_end_matches("dB")
        .trim_end_matches("db")
        .trim();
    match t.to_ascii_lowercase().as_str() {
        "-inf" | "inf" | "off" | "-∞" | "∞" => Ok(-120.0),
        s => s
            .replace(',', ".")
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(-120.0, 24.0))
            .ok_or_else(|| format!("'{t}' is not a level in dB")),
    }
}

pub fn fmt_db_field(db: f32) -> String {
    if db <= -119.9 {
        "-inf".into()
    } else {
        let s = format!("{db:.1}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_fields() {
        assert_eq!(fmt_secs(0.0), "0");
        assert_eq!(fmt_secs(2.5), "2.5");
        assert_eq!(fmt_secs(90.25), "1:30.25");
        assert_eq!(fmt_secs(61.0), "1:01");
        assert_eq!(parse_secs(""), Ok(None));
        assert_eq!(parse_secs("1:30"), Ok(Some(90.0)));
        assert!(parse_secs("abc").is_err());
        assert_eq!(parse_db("-6 dB"), Ok(-6.0));
        assert_eq!(parse_db("off"), Ok(-120.0));
        assert!(parse_db("loud").is_err());
        assert_eq!(fmt_db_field(-120.0), "-inf");
        assert_eq!(fmt_db_field(-6.0), "-6");
        assert_eq!(fmt_db_field(2.5), "2.5");
    }
}
