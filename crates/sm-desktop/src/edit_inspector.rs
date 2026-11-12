//! Inspector for the selected cue(s): Basic / Timing tabs, one tab for the
//! cue type and (audio) the routing matrix. Several selected cues get a bulk
//! editor for the common fields. Text fields commit on Enter / focus loss,
//! sliders on release; nothing is sent per frame.

use super::fields::{self, Opts};
use super::{audio, cue_label, fmt_secs, parse_db, parse_secs, type_label, EditState, Out};
use crate::model::{Tick, UiState};
use crate::theme;
use egui::{vec2, Color32, Id, RichText, Sense, Stroke, Ui};
use serde_json::{json, Map, Value};
use sm_core::midi::{MidiMessage, MscCommand};
use sm_core::model::{ContinueMode, ControlAction, Cue, CueKind, Curve, GroupMode, Show, TcAction};
use sm_core::timecode::Timecode;
use std::collections::HashSet;
use std::str::FromStr;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Basic,
    Timing,
    Kind,
    Routing,
}

#[derive(Default)]
pub struct InspectorState {
    pub tab: Tab,
    /// Slider being dragged: (slider id, value). Committed on release.
    pub slider: Option<(Id, f32)>,
    /// Waveform marker being dragged.
    pub wave_drag: Option<audio::WaveDrag>,
    renumber_start: String,
    renumber_step: String,
}

pub(crate) fn cue_patch(id: u64, patch: Value) -> Value {
    json!({ "cmd": "update_cue", "cue": id, "patch": patch })
}

pub(crate) fn kind_patch(id: u64, kind: Value) -> Value {
    cue_patch(id, json!({ "kind": kind }))
}

pub(crate) fn show(
    ui: &mut Ui,
    state: &UiState,
    tick: &Tick,
    sel: &[u64],
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let show = &state.show;
    let cues: Vec<&Cue> = if sel.len() <= 4 {
        sel.iter().filter_map(|id| show.cue(*id)).collect()
    } else {
        let want: HashSet<u64> = sel.iter().copied().collect();
        let mut v = Vec::with_capacity(sel.len());
        show.for_each_cue(|_, c| {
            if want.contains(&c.id) {
                v.push(c);
            }
        });
        v
    };
    match cues.as_slice() {
        [] => {}
        [cue] => single(ui, state, tick, cue, edit, out),
        many => bulk(ui, many, edit, out),
    }
}

fn tab_button(ui: &mut Ui, cur: &mut Tab, tab: Tab, label: &str) {
    if super::tab(ui, *cur == tab, label, &format!("{label} tab")).clicked() {
        *cur = tab;
    }
}

fn single(
    ui: &mut Ui,
    state: &UiState,
    tick: &Tick,
    cue: &Cue,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let show = &state.show;
    let kind_tab = match &cue.kind {
        CueKind::Memo => None,
        CueKind::Timecode(_) => Some("Timecode"),
        _ => Some(type_label(cue)),
    };
    let is_audio = matches!(cue.kind, CueKind::Audio(_));
    let tab = &mut edit.insp.tab;
    if (*tab == Tab::Kind && kind_tab.is_none()) || (*tab == Tab::Routing && !is_audio) {
        *tab = Tab::Basic;
    }
    let running = tick.active(cue.id).is_some();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(type_label(cue).to_uppercase())
                .small()
                .strong()
                .color(theme::MUTED),
        );
        ui.add(egui::Label::new(RichText::new(cue_label(cue)).strong().size(16.0)).truncate());
        ui.add_space(12.0);
        tab_button(ui, tab, Tab::Basic, "Basic");
        tab_button(ui, tab, Tab::Timing, "Timing");
        if let Some(k) = kind_tab {
            tab_button(ui, tab, Tab::Kind, k);
        }
        if is_audio {
            tab_button(ui, tab, Tab::Routing, "Routing");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(RichText::new("Delete").color(theme::ERROR))
                .accessible_name("Delete this cue")
                .on_hover_text("Delete the selected cue(s) (Delete / Backspace key)")
                .clicked()
            {
                out.push(Out::Action("delete"));
            }
            if ui
                .add_enabled(running, egui::Button::new("Stop"))
                .accessible_name("Stop cue")
                .on_hover_text("Stop this cue")
                .clicked()
            {
                out.push(Out::Cmd(json!({ "cmd": "stop_cue", "cue": cue.id })));
            }
            if ui
                .button(RichText::new("Preview").color(theme::NEXT))
                .accessible_name("Preview")
                .on_hover_text("Start this cue now without moving the playhead")
                .clicked()
            {
                out.push(Out::Cmd(json!({ "cmd": "start_cue", "cue": cue.id })));
            }
        });
    });
    ui.separator();
    let tab = edit.insp.tab;
    egui::ScrollArea::vertical()
        .id_salt("insp_scroll")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add_space(2.0);
            match tab {
                Tab::Basic => basic_tab(ui, cue, edit, out),
                Tab::Timing => timing_tab(ui, show, cue, edit, out),
                Tab::Kind => match &cue.kind {
                    CueKind::Audio(_) => audio::audio_tab(ui, state, tick, cue, edit, out),
                    CueKind::Fade(_) => fade_tab(ui, show, cue, edit, out),
                    CueKind::Midi(_) => midi_tab(ui, state, cue, edit, out),
                    CueKind::Timecode(_) => timecode_tab(ui, show, cue, edit, out),
                    CueKind::Control(_) => control_tab(ui, show, cue, edit, out),
                    CueKind::Group(_) => group_tab(ui, cue, out),
                    CueKind::Memo => {}
                },
                Tab::Routing => audio::routing_tab(ui, show, cue, out),
            }
        });
}

fn grid(id: impl std::hash::Hash + std::fmt::Debug) -> egui::Grid {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([14.0, 8.0])
        .min_col_width(84.0)
}

fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::MUTED));
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::MUTED));
}

type Cx<'a> = (&'a mut EditState, &'a mut Vec<Out>);

/// Two columns side by side (left fixed width), sharing the edit context.
fn columns(
    ui: &mut Ui,
    left_w: f32,
    cx: Cx<'_>,
    left: impl FnOnce(&mut Ui, &mut EditState, &mut Vec<Out>),
    right: impl FnOnce(&mut Ui, &mut EditState, &mut Vec<Out>),
) {
    let (edit, out) = cx;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(left_w);
            left(ui, edit, out);
        });
        ui.add_space(16.0);
        ui.vertical(|ui| right(ui, edit, out));
    });
}

// ---------------------------------------------------------------------------
// Basic / Timing
// ---------------------------------------------------------------------------

pub(crate) const CONTINUE: [(ContinueMode, &str); 3] = [
    (ContinueMode::DoNotContinue, "Do not continue"),
    (
        ContinueMode::AutoContinue,
        "Auto-continue (after post-wait)",
    ),
    (ContinueMode::AutoFollow, "Auto-follow (when finished)"),
];

fn continue_value(m: ContinueMode) -> Value {
    serde_json::to_value(m).unwrap_or(Value::Null)
}

fn basic_tab(ui: &mut Ui, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let id = cue.id;
    columns(
        ui,
        470.0,
        (edit, out),
        |ui, edit, out| {
            grid(("insp_basic", id)).show(ui, |ui| {
                label(ui, "Number");
                fields::field(
                    ui,
                    &mut edit.drafts,
                    Id::unique(("insp_number", id)),
                    &cue.number,
                    Opts::new("Cue number", 110.0),
                    out,
                    move |t| Ok(Some(cue_patch(id, json!({ "number": t.trim() })))),
                );
                ui.end_row();
                label(ui, "Name");
                fields::field(
                    ui,
                    &mut edit.drafts,
                    Id::unique(("insp_name", id)),
                    &cue.name,
                    Opts::new("Cue name", 340.0),
                    out,
                    move |t| Ok(Some(cue_patch(id, json!({ "name": t })))),
                );
                ui.end_row();
                label(ui, "Colour");
                if let Some(c) = color_swatches(ui, Some(&cue.color)) {
                    out.push(Out::Cmd(cue_patch(id, json!({ "color": c }))));
                }
                ui.end_row();
                label(ui, "Continue");
                if let Some(m) = continue_combo(ui, ("insp_cont", id), Some(cue.continue_mode)) {
                    out.push(Out::Cmd(cue_patch(
                        id,
                        json!({ "continue_mode": continue_value(m) }),
                    )));
                }
                ui.end_row();
                label(ui, "Armed");
                let mut armed = cue.armed;
                if ui
                    .checkbox(&mut armed, "Fires when triggered")
                    .accessible_name("Cue armed")
                    .changed()
                {
                    out.push(Out::Cmd(cue_patch(id, json!({ "armed": armed }))));
                }
                ui.end_row();
            });
        },
        |ui, edit, out| {
            label(ui, "Notes");
            fields::field(
                ui,
                &mut edit.drafts,
                Id::unique(("insp_notes", id)),
                &cue.notes,
                Opts::new("Cue notes", ui.available_width().max(200.0)).multiline(),
                out,
                move |t| Ok(Some(cue_patch(id, json!({ "notes": t })))),
            );
        },
    );
}

/// Colour swatches; `current = None` means mixed. Returns the clicked colour.
fn color_swatches(ui: &mut Ui, current: Option<&str>) -> Option<&'static str> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for name in theme::CUE_COLORS {
            let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
            let p = ui.painter();
            let c = rect.center();
            match theme::cue_color(name) {
                Some(col) => {
                    p.circle_filled(c, 9.0, col);
                }
                None => {
                    p.circle_stroke(c, 8.0, Stroke::new(1.5, theme::MUTED));
                    p.line_segment(
                        [c + vec2(-5.5, 5.5), c + vec2(5.5, -5.5)],
                        Stroke::new(1.5, theme::MUTED),
                    );
                }
            }
            let on = current == Some(name);
            if on {
                p.circle_stroke(c, 11.0, Stroke::new(2.0, Color32::WHITE));
            } else if resp.hovered() {
                p.circle_stroke(c, 11.0, Stroke::new(1.0, Color32::from_gray(160)));
            }
            let label = format!("Colour {}", if name.is_empty() { "none" } else { name });
            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::accesskit::Role::RadioButton, true, on, &label)
            });
            if resp.clicked() {
                picked = Some(name);
            }
        }
    });
    picked
}

fn continue_combo(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    cur: Option<ContinueMode>,
) -> Option<ContinueMode> {
    let text = cur
        .and_then(|c| CONTINUE.iter().find(|(m, _)| *m == c))
        .map_or("(mixed)", |(_, t)| *t);
    let mut picked = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(text)
        .width(260.0)
        .show_ui(ui, |ui| {
            for (m, t) in CONTINUE {
                if ui.selectable_label(cur == Some(m), t).clicked() {
                    picked = Some(m);
                }
            }
        });
    picked.filter(|m| Some(*m) != cur)
}

fn tc_field_parse(t: &str) -> Result<Option<Timecode>, String> {
    let t = t.trim();
    if t.is_empty() {
        return Ok(None);
    }
    Timecode::from_str(t)
        .map(Some)
        .map_err(|_| format!("'{t}' is not a timecode (HH:MM:SS:FF)"))
}

fn timing_tab(ui: &mut Ui, show: &Show, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let id = cue.id;
    let rate = show.timecode.rate.label();
    grid(("insp_timing", id)).show(ui, |ui| {
        label(ui, "Pre-wait");
        ui.horizontal(|ui| {
            time_field(
                ui,
                edit,
                out,
                ("insp_pre", id),
                "Pre-wait time",
                cue.pre_wait,
                move |v| cue_patch(id, json!({ "pre_wait": v })),
            );
            hint(ui, "delay between trigger and action (seconds or m:ss.s)");
        });
        ui.end_row();
        label(ui, "Post-wait");
        ui.horizontal(|ui| {
            time_field(
                ui,
                edit,
                out,
                ("insp_post", id),
                "Post-wait time",
                cue.post_wait,
                move |v| cue_patch(id, json!({ "post_wait": v })),
            );
            hint(ui, "auto-continue fires the next cue after this time");
        });
        ui.end_row();
        label(ui, "TC trigger");
        ui.horizontal(|ui| {
            let cur = cue.tc_trigger.map(|t| t.to_string()).unwrap_or_default();
            fields::field(
                ui,
                &mut edit.drafts,
                Id::unique(("insp_tc", id)),
                &cur,
                Opts::new("TC trigger time", 120.0).mono().hint("none"),
                out,
                move |t| {
                    let tc = tc_field_parse(t)?;
                    Ok(Some(cue_patch(
                        id,
                        json!({ "tc_trigger": tc.map(|t| t.to_string()) }),
                    )))
                },
            );
            if cue.tc_trigger.is_some()
                && ui
                    .small_button("Clear")
                    .accessible_name("Clear TC trigger")
                    .clicked()
            {
                out.push(Out::Cmd(cue_patch(id, json!({ "tc_trigger": null }))));
            }
            hint(
                ui,
                &format!("fires when the timecode reaches HH:MM:SS:FF ({rate} fps); empty = none"),
            );
        });
        ui.end_row();
    });
}

/// A seconds field; `make` gets the parsed value (empty = 0).
pub(crate) fn time_field(
    ui: &mut Ui,
    edit: &mut EditState,
    out: &mut Vec<Out>,
    salt: impl std::hash::Hash + std::fmt::Debug,
    name: &str,
    value: f64,
    make: impl Fn(f64) -> Value + 'static,
) -> egui::Response {
    fields::field(
        ui,
        &mut edit.drafts,
        Id::unique(salt),
        &fmt_secs(value),
        Opts::new(name, 90.0).mono(),
        out,
        move |t| Ok(Some(make(parse_secs(t)?.unwrap_or(0.0)))),
    )
}

/// An integer field within `lo..=hi`.
#[allow(clippy::too_many_arguments)]
fn int_field(
    ui: &mut Ui,
    edit: &mut EditState,
    out: &mut Vec<Out>,
    salt: impl std::hash::Hash + std::fmt::Debug,
    name: &str,
    value: i64,
    (lo, hi): (i64, i64),
    make: impl Fn(i64) -> Value + 'static,
) {
    let name_owned = name.to_string();
    fields::field(
        ui,
        &mut edit.drafts,
        Id::unique(salt),
        &value.to_string(),
        Opts::new(name, 70.0).mono(),
        out,
        move |t| {
            let v: i64 = t
                .trim()
                .parse()
                .map_err(|_| format!("{name_owned}: '{}' is not a whole number", t.trim()))?;
            if !(lo..=hi).contains(&v) {
                return Err(format!("{name_owned} must be {lo} to {hi}"));
            }
            Ok(Some(make(v)))
        },
    );
}

pub(crate) const CURVES: [(Curve, &str); 3] = [
    (Curve::Linear, "Linear"),
    (Curve::EqualPower, "Equal power"),
    (Curve::SCurve, "S-curve"),
];

pub(crate) fn curve_combo(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    cur: Curve,
) -> Option<Value> {
    let text = CURVES
        .iter()
        .find(|(c, _)| *c == cur)
        .map_or("", |(_, t)| *t);
    let mut picked = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(text)
        .width(150.0)
        .show_ui(ui, |ui| {
            for (c, t) in CURVES {
                if ui.selectable_label(cur == c, t).clicked() && c != cur {
                    picked = serde_json::to_value(c).ok();
                }
            }
        });
    picked
}

/// Combo box of cues (`None` = no target). Options are only built while open.
fn cue_combo(
    ui: &mut Ui,
    show: &Show,
    salt: impl std::hash::Hash + std::fmt::Debug,
    cur: Option<u64>,
    exclude: u64,
    filter: impl Fn(&Cue) -> bool,
) -> Option<Option<u64>> {
    let text = match cur.and_then(|c| show.cue(c)) {
        Some(c) => cue_label(c),
        None if cur.is_some() => "(missing cue)".into(),
        None => "— none —".into(),
    };
    let mut picked = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(text)
        .width(300.0)
        .height(360.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(cur.is_none(), "— none —").clicked() {
                picked = Some(None);
            }
            show.for_each_cue(|_, c| {
                if c.id != exclude
                    && filter(c)
                    && ui
                        .selectable_label(cur == Some(c.id), cue_label(c))
                        .clicked()
                {
                    picked = Some(Some(c.id));
                }
            });
        });
    picked.filter(|p| *p != cur)
}

// ---------------------------------------------------------------------------
// Type tabs
// ---------------------------------------------------------------------------

fn fade_tab(ui: &mut Ui, show: &Show, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let CueKind::Fade(f) = &cue.kind else { return };
    let id = cue.id;
    grid(("insp_fade", id)).show(ui, |ui| {
        label(ui, "Target");
        if let Some(t) = cue_combo(ui, show, ("fade_target", id), f.target, id, |c| {
            matches!(c.kind, CueKind::Audio(_) | CueKind::Group(_))
        }) {
            out.push(Out::Cmd(kind_patch(id, json!({ "target": t }))));
        }
        ui.end_row();
        label(ui, "Duration");
        time_field(
            ui,
            edit,
            out,
            ("fade_dur", id),
            "Fade duration",
            f.duration,
            move |v| kind_patch(id, json!({ "duration": v })),
        );
        ui.end_row();
        label(ui, "Curve");
        if let Some(c) = curve_combo(ui, ("fade_curve", id), f.curve) {
            out.push(Out::Cmd(kind_patch(id, json!({ "curve": c }))));
        }
        ui.end_row();
        label(ui, "Level");
        ui.horizontal(|ui| {
            let mut change = f.gain_db.is_some();
            if ui
                .checkbox(&mut change, "Fade level to")
                .accessible_name("Fade changes level")
                .changed()
            {
                let v = if change { json!(-120.0) } else { Value::Null };
                out.push(Out::Cmd(kind_patch(id, json!({ "gain_db": v }))));
            }
            match f.gain_db {
                Some(db) => {
                    fields::field(
                        ui,
                        &mut edit.drafts,
                        Id::unique(("fade_db", id)),
                        &super::fmt_db_field(db),
                        Opts::new("Fade level", 70.0).mono(),
                        out,
                        move |t| Ok(Some(kind_patch(id, json!({ "gain_db": parse_db(t)? })))),
                    );
                    ui.label("dB");
                    hint(ui, "-inf = silence");
                }
                None => hint(ui, "level unchanged"),
            }
        });
        ui.end_row();
        label(ui, "When done");
        let mut stop = f.stop_target;
        if ui
            .checkbox(&mut stop, "Stop the target cue")
            .accessible_name("Stop target when done")
            .changed()
        {
            out.push(Out::Cmd(kind_patch(id, json!({ "stop_target": stop }))));
        }
        ui.end_row();
    });
    fade_crosspoints(ui, show, cue, edit, out);
}

/// Routing levels the fade moves to (source channel -> output, dB).
fn fade_crosspoints(ui: &mut Ui, show: &Show, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let CueKind::Fade(f) = &cue.kind else { return };
    let id = cue.id;
    let target = f.target.and_then(|t| show.cue(t));
    let src_channels = match target.map(|t| &t.kind) {
        Some(CueKind::Audio(a)) => a.channels.max(1),
        _ => 2,
    };
    let outputs: Vec<String> = show
        .patch
        .outputs
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if o.name.is_empty() {
                format!("Out {}", i + 1)
            } else {
                o.name.clone()
            }
        })
        .collect();
    let points = f.crosspoints.clone();
    let send = |out: &mut Vec<Out>, pts: &[sm_core::model::Crosspoint]| {
        out.push(Out::Cmd(kind_patch(id, json!({ "crosspoints": pts }))));
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Routing levels").strong());
        hint(
            ui,
            "optional: fade single source-to-output levels of the target",
        );
    });
    if points.is_empty() {
        hint(
            ui,
            "No routing levels - the fade only changes the cue level.",
        );
    }
    let mut next = points.clone();
    let mut changed = false;
    egui::Grid::new(Id::unique(("fade_xp", id)))
        .num_columns(5)
        .spacing(vec2(10.0, 4.0))
        .show(ui, |ui| {
            for (i, p) in points.iter().enumerate() {
                let mut src = p.src;
                egui::ComboBox::from_id_salt(("fade_xp_src", id, i))
                    .width(90.0)
                    .selected_text(format!("Source {}", src + 1))
                    .show_ui(ui, |ui| {
                        for c in 0..src_channels.max(p.src + 1) {
                            ui.selectable_value(&mut src, c, format!("Source {}", c + 1));
                        }
                    });
                ui.label("to");
                let mut o = p.out;
                let name = outputs
                    .get(usize::from(o))
                    .cloned()
                    .unwrap_or_else(|| format!("Out {} (missing)", o + 1));
                egui::ComboBox::from_id_salt(("fade_xp_out", id, i))
                    .width(120.0)
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for (k, n) in outputs.iter().enumerate() {
                            ui.selectable_value(&mut o, k as u16, n);
                        }
                    });
                if src != p.src || o != p.out {
                    if let Some(x) = next.get_mut(i) {
                        x.src = src;
                        x.out = o;
                    }
                    changed = true;
                }
                ui.horizontal(|ui| {
                    let all = next.clone();
                    fields::field(
                        ui,
                        &mut edit.drafts,
                        Id::unique(("fade_xp_db", id, i)),
                        &super::fmt_db_field(p.db),
                        Opts::new("Routing level", 70.0).mono(),
                        out,
                        move |t| {
                            let db = parse_db(t)?;
                            let mut pts = all.clone();
                            if let Some(x) = pts.get_mut(i) {
                                x.db = db;
                            }
                            Ok(Some(kind_patch(id, json!({ "crosspoints": pts }))))
                        },
                    );
                    ui.label("dB");
                });
                if ui.small_button("Remove").clicked() {
                    next.remove(i.min(next.len().saturating_sub(1)));
                    changed = true;
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        if ui.button("Add level").clicked() {
            next.push(sm_core::model::Crosspoint {
                src: 0,
                out: 0,
                db: 0.0,
            });
            changed = true;
        }
        let target_routing = match target.map(|t| &t.kind) {
            Some(CueKind::Audio(a)) => Some(a.effective_routing(show.patch.outputs.len())),
            _ => None,
        };
        if let Some(r) = target_routing {
            if ui
                .button("Start from target's routing")
                .on_hover_text("Copy the target cue's current routing, then change the levels")
                .clicked()
            {
                next = r;
                changed = true;
            }
        }
        if !points.is_empty() && ui.button("Clear").clicked() {
            next.clear();
            changed = true;
        }
    });
    if changed {
        send(out, &next);
    }
}

const MIDI_TYPES: [(&str, &str); 7] = [
    ("note_on", "Note On"),
    ("note_off", "Note Off"),
    ("control_change", "Control Change"),
    ("program_change", "Program Change"),
    ("pitch_bend", "Pitch Bend"),
    ("sysex", "SysEx"),
    ("msc", "MIDI Show Control"),
];

const MSC_COMMANDS: [(MscCommand, &str); 11] = [
    (MscCommand::Go, "GO"),
    (MscCommand::Stop, "STOP"),
    (MscCommand::Resume, "RESUME"),
    (MscCommand::TimedGo, "TIMED GO"),
    (MscCommand::Load, "LOAD"),
    (MscCommand::Set, "SET"),
    (MscCommand::Fire, "FIRE"),
    (MscCommand::AllOff, "ALL OFF"),
    (MscCommand::Restore, "RESTORE"),
    (MscCommand::Reset, "RESET"),
    (MscCommand::GoOff, "GO OFF"),
];

fn msg_type(m: &MidiMessage) -> &'static str {
    match m {
        MidiMessage::NoteOn { .. } => "note_on",
        MidiMessage::NoteOff { .. } => "note_off",
        MidiMessage::ControlChange { .. } => "control_change",
        MidiMessage::ProgramChange { .. } => "program_change",
        MidiMessage::PitchBend { .. } => "pitch_bend",
        MidiMessage::Sysex { .. } => "sysex",
        MidiMessage::Msc { .. } => "msc",
    }
}

/// Short description for the table.
pub fn midi_summary(m: &MidiMessage) -> String {
    match m {
        MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        } => format!("Note On {note} vel {velocity} ch {channel}"),
        MidiMessage::NoteOff { channel, note, .. } => format!("Note Off {note} ch {channel}"),
        MidiMessage::ControlChange {
            channel,
            controller,
            value,
        } => format!("CC {controller} = {value} ch {channel}"),
        MidiMessage::ProgramChange { channel, program } => {
            format!("Program {program} ch {channel}")
        }
        MidiMessage::PitchBend { channel, value } => format!("Pitch bend {value} ch {channel}"),
        MidiMessage::Sysex { .. } => "SysEx".into(),
        MidiMessage::Msc {
            command, q_number, ..
        } => {
            let c = MSC_COMMANDS
                .iter()
                .find(|(x, _)| x == command)
                .map_or("", |(_, t)| *t);
            if q_number.is_empty() {
                format!("MSC {c}")
            } else {
                format!("MSC {c} {q_number}")
            }
        }
    }
}

/// Patch that switches the message type: the new message plus `null` for
/// every field of the old one (RFC 7396 removal).
pub fn midi_type_patch(old: &MidiMessage, new_type: &str) -> Value {
    let channel = match old {
        MidiMessage::NoteOn { channel, .. }
        | MidiMessage::NoteOff { channel, .. }
        | MidiMessage::ControlChange { channel, .. }
        | MidiMessage::ProgramChange { channel, .. }
        | MidiMessage::PitchBend { channel, .. } => *channel,
        _ => 1,
    };
    let next = match new_type {
        "note_on" => json!({ "msg": "note_on", "channel": channel, "note": 60, "velocity": 100 }),
        "note_off" => json!({ "msg": "note_off", "channel": channel, "note": 60, "velocity": 0 }),
        "control_change" => {
            json!({ "msg": "control_change", "channel": channel, "controller": 0, "value": 0 })
        }
        "program_change" => json!({ "msg": "program_change", "channel": channel, "program": 0 }),
        "pitch_bend" => json!({ "msg": "pitch_bend", "channel": channel, "value": 0 }),
        "sysex" => json!({ "msg": "sysex", "hex": "F0 7F 7F 04 01 00 7F F7" }),
        _ => json!({
            "msg": "msc", "device_id": 127, "command_format": 16, "command": "go",
            "q_number": "", "q_list": "", "q_path": "",
        }),
    };
    let mut patch = Map::new();
    if let Ok(Value::Object(o)) = serde_json::to_value(old) {
        for k in o.keys() {
            patch.insert(k.clone(), Value::Null);
        }
    }
    if let Value::Object(n) = next {
        patch.extend(n);
    }
    Value::Object(patch)
}

fn midi_tab(ui: &mut Ui, state: &UiState, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let CueKind::Midi(k) = &cue.kind else { return };
    let id = cue.id;
    let show = &state.show;
    let m = &k.message;
    let msg = |v: Value| kind_patch(id, json!({ "message": v }));
    columns(
        ui,
        470.0,
        (edit, out),
        |ui, edit, out| {
            grid(("insp_midi", id)).show(ui, |ui| {
                label(ui, "Port");
                let text = if k.port.is_empty() {
                    "— none —".to_string()
                } else {
                    k.port.clone()
                };
                let mut picked: Option<String> = None;
                egui::ComboBox::from_id_salt(("midi_port", id))
                    .selected_text(text)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(k.port.is_empty(), "— none —").clicked() {
                            picked = Some(String::new());
                        }
                        for p in &show.midi_ports {
                            let t = format!(
                                "{}  ({})",
                                p.name,
                                if p.device.is_empty() {
                                    "unassigned"
                                } else {
                                    &p.device
                                }
                            );
                            if ui.selectable_label(k.port == p.name, t).clicked() {
                                picked = Some(p.name.clone());
                            }
                        }
                        for d in &state.midi.ports {
                            if show.midi_ports.iter().any(|p| &p.name == d) {
                                continue;
                            }
                            if ui
                                .selectable_label(k.port == *d, format!("{d}  (device)"))
                                .clicked()
                            {
                                picked = Some(d.clone());
                            }
                        }
                    });
                if let Some(p) = picked.filter(|p| *p != k.port) {
                    out.push(Out::Cmd(kind_patch(id, json!({ "port": p }))));
                }
                ui.end_row();
                if !k.port.is_empty()
                    && !show.midi_ports.iter().any(|p| p.name == k.port)
                    && !state.midi.ports.contains(&k.port)
                {
                    ui.label("");
                    ui.label(
                        RichText::new("This port is not available on this computer")
                            .small()
                            .color(theme::WARN),
                    );
                    ui.end_row();
                }
                label(ui, "Message");
                let cur = msg_type(m);
                let text = MIDI_TYPES
                    .iter()
                    .find(|(t, _)| *t == cur)
                    .map_or("", |(_, l)| *l);
                let mut new_type = None;
                egui::ComboBox::from_id_salt(("midi_type", id))
                    .selected_text(text)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (t, l) in MIDI_TYPES {
                            if ui.selectable_label(cur == t, l).clicked() && t != cur {
                                new_type = Some(t);
                            }
                        }
                    });
                if let Some(t) = new_type {
                    out.push(Out::Cmd(msg(midi_type_patch(m, t))));
                }
                ui.end_row();
                let ch_field = |ui: &mut Ui, edit: &mut EditState, out: &mut Vec<Out>, ch: u8| {
                    label(ui, "Channel");
                    int_field(
                        ui,
                        edit,
                        out,
                        ("midi_ch", id),
                        "MIDI channel",
                        ch.into(),
                        (1, 16),
                        move |v| kind_patch(id, json!({ "message": { "channel": v } })),
                    );
                    ui.end_row();
                };
                let byte = |ui: &mut Ui,
                            edit: &mut EditState,
                            out: &mut Vec<Out>,
                            key: &'static str,
                            (text, name): (&str, &str),
                            v: u8| {
                    label(ui, text);
                    int_field(
                        ui,
                        edit,
                        out,
                        ("midi_b", id, key),
                        name,
                        v.into(),
                        (0, 127),
                        move |v| kind_patch(id, json!({ "message": { key: v } })),
                    );
                    ui.end_row();
                };
                match m {
                    MidiMessage::NoteOn {
                        channel,
                        note,
                        velocity,
                    }
                    | MidiMessage::NoteOff {
                        channel,
                        note,
                        velocity,
                    } => {
                        ch_field(ui, edit, out, *channel);
                        byte(ui, edit, out, "note", ("Note", "Note number"), *note);
                        byte(
                            ui,
                            edit,
                            out,
                            "velocity",
                            ("Velocity", "Note velocity"),
                            *velocity,
                        );
                    }
                    MidiMessage::ControlChange {
                        channel,
                        controller,
                        value,
                    } => {
                        ch_field(ui, edit, out, *channel);
                        byte(
                            ui,
                            edit,
                            out,
                            "controller",
                            ("Controller", "Controller number"),
                            *controller,
                        );
                        byte(
                            ui,
                            edit,
                            out,
                            "value",
                            ("Value", "Controller value"),
                            *value,
                        );
                    }
                    MidiMessage::ProgramChange { channel, program } => {
                        ch_field(ui, edit, out, *channel);
                        byte(
                            ui,
                            edit,
                            out,
                            "program",
                            ("Program", "Program number"),
                            *program,
                        );
                    }
                    MidiMessage::PitchBend { channel, value } => {
                        ch_field(ui, edit, out, *channel);
                        label(ui, "Value");
                        ui.horizontal(|ui| {
                            int_field(
                                ui,
                                edit,
                                out,
                                ("midi_pb", id),
                                "Pitch bend",
                                (*value).into(),
                                (-8192, 8191),
                                move |v| kind_patch(id, json!({ "message": { "value": v } })),
                            );
                            hint(ui, "-8192 … 8191, 0 = centre");
                        });
                        ui.end_row();
                    }
                    MidiMessage::Sysex { hex } => {
                        label(ui, "Hex data");
                        fields::field(
                            ui,
                            &mut edit.drafts,
                            Id::unique(("midi_sysex", id)),
                            hex,
                            Opts::new("SysEx hex", 320.0).mono().hint("F0 … F7"),
                            out,
                            move |t| {
                                let hex = t.trim().to_string();
                                MidiMessage::Sysex { hex: hex.clone() }.to_bytes()?;
                                Ok(Some(kind_patch(id, json!({ "message": { "hex": hex } }))))
                            },
                        );
                        ui.end_row();
                        ui.label("");
                        hint(
                            ui,
                            "hex bytes; F0 / F7 framing is added if missing, data 00–7F",
                        );
                        ui.end_row();
                    }
                    MidiMessage::Msc { .. } => {}
                }
            });
        },
        |ui, edit, out| {
            let MidiMessage::Msc {
                device_id,
                command_format,
                command,
                q_number,
                q_list,
                q_path,
            } = m
            else {
                return;
            };
            grid(("insp_msc", id)).show(ui, |ui| {
                label(ui, "Device ID");
                ui.horizontal(|ui| {
                    int_field(
                        ui,
                        edit,
                        out,
                        ("msc_dev", id),
                        "MSC device ID",
                        (*device_id).into(),
                        (0, 127),
                        move |v| kind_patch(id, json!({ "message": { "device_id": v } })),
                    );
                    hint(ui, "0–111, 127 = all");
                });
                ui.end_row();
                label(ui, "Format");
                ui.horizontal(|ui| {
                    int_field(
                        ui,
                        edit,
                        out,
                        ("msc_fmt", id),
                        "Command format",
                        (*command_format).into(),
                        (0, 127),
                        move |v| kind_patch(id, json!({ "message": { "command_format": v } })),
                    );
                    hint(ui, "1 = lighting, 16 = sound, 127 = all");
                });
                ui.end_row();
                label(ui, "Command");
                let text = MSC_COMMANDS
                    .iter()
                    .find(|(c, _)| c == command)
                    .map_or("", |(_, t)| *t);
                let mut picked = None;
                egui::ComboBox::from_id_salt(("msc_cmd", id))
                    .selected_text(text)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for (c, t) in MSC_COMMANDS {
                            if ui.selectable_label(*command == c, t).clicked() && c != *command {
                                picked = serde_json::to_value(c).ok();
                            }
                        }
                    });
                if let Some(c) = picked {
                    out.push(Out::Cmd(msg(json!({ "command": c }))));
                }
                ui.end_row();
                for (key, name, v) in [
                    ("q_number", "Q number", q_number),
                    ("q_list", "Q list", q_list),
                    ("q_path", "Q path", q_path),
                ] {
                    label(ui, name);
                    fields::field(
                        ui,
                        &mut edit.drafts,
                        Id::unique(("msc_q", id, key)),
                        v,
                        Opts::new(&format!("MSC {name}"), 110.0).mono(),
                        out,
                        move |t| {
                            let t = t.trim();
                            if !t.chars().all(|c| c.is_ascii_digit() || c == '.') {
                                return Err(format!("{key}: digits and dots only"));
                            }
                            Ok(Some(kind_patch(id, json!({ "message": { key: t } }))))
                        },
                    );
                    ui.end_row();
                }
            });
        },
    );
}

const TC_ACTIONS: [(&str, &str); 6] = [
    ("start", "Start"),
    ("start_from", "Start from…"),
    ("stop", "Stop"),
    ("pause", "Pause"),
    ("resume", "Resume"),
    ("relocate", "Relocate to…"),
];

fn timecode_tab(ui: &mut Ui, show: &Show, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let CueKind::Timecode(k) = &cue.kind else {
        return;
    };
    let id = cue.id;
    let cur = match &k.tc {
        TcAction::Start => "start",
        TcAction::StartFrom { .. } => "start_from",
        TcAction::Stop => "stop",
        TcAction::Pause => "pause",
        TcAction::Resume => "resume",
        TcAction::Relocate { .. } => "relocate",
    };
    let start = show.timecode.start.to_string();
    grid(("insp_tc", id)).show(ui, |ui| {
        label(ui, "Action");
        let text = TC_ACTIONS
            .iter()
            .find(|(a, _)| *a == cur)
            .map_or("", |(_, t)| *t);
        let mut picked = None;
        egui::ComboBox::from_id_salt(("tc_action", id))
            .selected_text(text)
            .width(180.0)
            .show_ui(ui, |ui| {
                for (a, t) in TC_ACTIONS {
                    if ui.selectable_label(cur == a, t).clicked() && a != cur {
                        picked = Some(a);
                    }
                }
            });
        if let Some(a) = picked {
            let time = match &k.tc {
                TcAction::StartFrom { from } => from.to_string(),
                TcAction::Relocate { to } => to.to_string(),
                _ => start.clone(),
            };
            let tc = match a {
                "start_from" => json!({ "action": a, "from": time, "to": null }),
                "relocate" => json!({ "action": a, "to": time, "from": null }),
                _ => json!({ "action": a, "from": null, "to": null }),
            };
            out.push(Out::Cmd(kind_patch(id, json!({ "tc": tc }))));
        }
        ui.end_row();
        let time = match &k.tc {
            TcAction::StartFrom { from } => Some(("from", "From", from)),
            TcAction::Relocate { to } => Some(("to", "To", to)),
            _ => None,
        };
        if let Some((key, name, tc)) = time {
            label(ui, name);
            fields::field(
                ui,
                &mut edit.drafts,
                Id::unique(("tc_time", id, key)),
                &tc.to_string(),
                Opts::new("Timecode position", 120.0).mono(),
                out,
                move |t| {
                    let tc = tc_field_parse(t)?.ok_or("Enter a timecode (HH:MM:SS:FF)")?;
                    Ok(Some(kind_patch(
                        id,
                        json!({ "tc": { key: tc.to_string() } }),
                    )))
                },
            );
            ui.end_row();
        }
    });
}

const CONTROL_ACTIONS: [(ControlAction, &str); 9] = [
    (ControlAction::Start, "Start"),
    (ControlAction::Stop, "Stop"),
    (ControlAction::Pause, "Pause"),
    (ControlAction::Resume, "Resume"),
    (ControlAction::LoadToTime, "Load to time"),
    (ControlAction::Arm, "Arm"),
    (ControlAction::Disarm, "Disarm"),
    (ControlAction::GoTo, "Go to (set playhead)"),
    (ControlAction::Reset, "Reset show"),
];

pub fn control_label(a: ControlAction) -> &'static str {
    CONTROL_ACTIONS
        .iter()
        .find(|(x, _)| *x == a)
        .map_or("", |(_, t)| *t)
}

fn control_tab(ui: &mut Ui, show: &Show, cue: &Cue, edit: &mut EditState, out: &mut Vec<Out>) {
    let CueKind::Control(k) = &cue.kind else {
        return;
    };
    let id = cue.id;
    grid(("insp_control", id)).show(ui, |ui| {
        label(ui, "Action");
        let mut picked = None;
        egui::ComboBox::from_id_salt(("control_action", id))
            .selected_text(control_label(k.action))
            .width(200.0)
            .show_ui(ui, |ui| {
                for (a, t) in CONTROL_ACTIONS {
                    if ui.selectable_label(k.action == a, t).clicked() && a != k.action {
                        picked = serde_json::to_value(a).ok();
                    }
                }
            });
        if let Some(a) = picked {
            out.push(Out::Cmd(kind_patch(id, json!({ "action": a }))));
        }
        ui.end_row();
        if k.action != ControlAction::Reset {
            label(ui, "Target");
            if let Some(t) = cue_combo(ui, show, ("control_target", id), k.target, id, |_| true) {
                out.push(Out::Cmd(kind_patch(id, json!({ "target": t }))));
            }
            ui.end_row();
        }
        let time = match k.action {
            ControlAction::LoadToTime => Some(("Time", "position to load the target to")),
            ControlAction::Stop => Some(("Fade time", "0 = stop immediately")),
            _ => None,
        };
        if let Some((name, h)) = time {
            label(ui, name);
            ui.horizontal(|ui| {
                time_field(
                    ui,
                    edit,
                    out,
                    ("control_time", id),
                    "Control time",
                    k.time,
                    move |v| kind_patch(id, json!({ "time": v })),
                );
                hint(ui, h);
            });
            ui.end_row();
        }
    });
}

const GROUP_MODES: [(GroupMode, &str); 3] = [
    (GroupMode::Parallel, "Parallel – fire all children together"),
    (GroupMode::Playlist, "Playlist – children one after another"),
    (GroupMode::Random, "Random – one random child"),
];

pub fn group_mode_label(m: GroupMode) -> &'static str {
    match m {
        GroupMode::Parallel => "parallel",
        GroupMode::Playlist => "playlist",
        GroupMode::Random => "random",
    }
}

fn group_tab(ui: &mut Ui, cue: &Cue, out: &mut Vec<Out>) {
    let CueKind::Group(g) = &cue.kind else { return };
    let id = cue.id;
    grid(("insp_group", id)).show(ui, |ui| {
        label(ui, "Mode");
        let text = GROUP_MODES
            .iter()
            .find(|(m, _)| *m == g.mode)
            .map_or("", |(_, t)| *t);
        let mut picked = None;
        egui::ComboBox::from_id_salt(("group_mode", id))
            .selected_text(text)
            .width(320.0)
            .show_ui(ui, |ui| {
                for (m, t) in GROUP_MODES {
                    if ui.selectable_label(g.mode == m, t).clicked() && m != g.mode {
                        picked = serde_json::to_value(m).ok();
                    }
                }
            });
        if let Some(m) = picked {
            out.push(Out::Cmd(kind_patch(id, json!({ "mode": m }))));
        }
        ui.end_row();
        label(ui, "Children");
        let n = g.children.len();
        ui.label(format!(
            "{n} cue{} – drag cues onto the group row to add them",
            if n == 1 { "" } else { "s" }
        ));
        ui.end_row();
    });
}

// ---------------------------------------------------------------------------
// Bulk edit
// ---------------------------------------------------------------------------

fn common<T: PartialEq + Copy>(cues: &[&Cue], get: impl Fn(&Cue) -> T) -> Option<T> {
    let first = get(cues.first()?);
    cues.iter().all(|c| get(c) == first).then_some(first)
}

fn bulk(ui: &mut Ui, cues: &[&Cue], edit: &mut EditState, out: &mut Vec<Out>) {
    let ids: Vec<u64> = cues.iter().map(|c| c.id).collect();
    let patch = |p: Value| Out::Cmd(json!({ "cmd": "update_cues", "cues": ids, "patch": p }));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} cues selected", cues.len()))
                .strong()
                .size(16.0),
        );
        hint(ui, "changes apply to all selected cues");
    });
    ui.separator();
    let armed = common(cues, |c| c.armed);
    let cont = common(cues, |c| c.continue_mode);
    let color = cues
        .first()
        .map(|c| c.color.as_str())
        .filter(|f| cues.iter().all(|c| c.color == *f));
    let pre = common(cues, |c| c.pre_wait.to_bits()).map(f64::from_bits);
    let post = common(cues, |c| c.post_wait.to_bits()).map(f64::from_bits);
    let salt = ids.clone();
    columns(
        ui,
        470.0,
        (edit, out),
        |ui, edit, out| {
            grid("insp_bulk").show(ui, |ui| {
                label(ui, "Armed");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(armed != Some(true), egui::Button::new("Arm all"))
                        .clicked()
                    {
                        out.push(patch(json!({ "armed": true })));
                    }
                    if ui
                        .add_enabled(armed != Some(false), egui::Button::new("Disarm all"))
                        .clicked()
                    {
                        out.push(patch(json!({ "armed": false })));
                    }
                    if armed.is_none() {
                        hint(ui, "mixed");
                    }
                });
                ui.end_row();
                label(ui, "Colour");
                if let Some(c) = color_swatches(ui, color) {
                    out.push(patch(json!({ "color": c })));
                }
                ui.end_row();
                label(ui, "Continue");
                if let Some(m) = continue_combo(ui, "bulk_cont", cont) {
                    out.push(patch(json!({ "continue_mode": continue_value(m) })));
                }
                ui.end_row();
                for (key, name, v) in [
                    ("pre_wait", "Pre-wait", pre),
                    ("post_wait", "Post-wait", post),
                ] {
                    label(ui, name);
                    let ids = ids.clone();
                    fields::field(
                        ui,
                        &mut edit.drafts,
                        Id::unique(("bulk_time", &salt, key)),
                        &v.map(fmt_secs).unwrap_or_default(),
                        Opts::new(&format!("{name} time"), 90.0)
                            .mono()
                            .hint("mixed"),
                        out,
                        move |t| {
                            Ok(parse_secs(t)?.map(|v| {
                                json!({ "cmd": "update_cues", "cues": ids, "patch": { key: v } })
                            }))
                        },
                    );
                    ui.end_row();
                }
            });
        },
        |ui, edit, out| {
            grid("insp_bulk2").show(ui, |ui| {
                label(ui, "Renumber");
                ui.horizontal(|ui| {
                    let st = &mut edit.insp;
                    if st.renumber_start.is_empty() {
                        st.renumber_start = "1".into();
                    }
                    if st.renumber_step.is_empty() {
                        st.renumber_step = "1".into();
                    }
                    ui.label("start");
                    ui.add(egui::TextEdit::singleline(&mut st.renumber_start).desired_width(56.0))
                        .accessible_name("Renumber start");
                    ui.label("step");
                    ui.add(egui::TextEdit::singleline(&mut st.renumber_step).desired_width(56.0))
                        .accessible_name("Renumber step");
                    let start = st
                        .renumber_start
                        .trim()
                        .replace(',', ".")
                        .parse::<f64>()
                        .ok();
                    let step = st
                        .renumber_step
                        .trim()
                        .replace(',', ".")
                        .parse::<f64>()
                        .ok()
                        .filter(|s| *s > 0.0);
                    if ui
                        .add_enabled(
                            start.is_some() && step.is_some(),
                            egui::Button::new("Apply"),
                        )
                        .accessible_name("Apply renumber")
                        .clicked()
                    {
                        if let (Some(start), Some(step)) = (start, step) {
                            out.push(Out::Cmd(json!({
                                "cmd": "renumber", "cues": ids, "start": start, "step": step,
                            })));
                        }
                    }
                });
                ui.end_row();
                label(ui, "Actions");
                ui.horizontal(|ui| {
                    if ui
                        .button("Group")
                        .accessible_name("Group selected")
                        .clicked()
                    {
                        out.push(Out::CmdSelect(json!({ "cmd": "group_cues", "cues": ids })));
                    }
                    if ui
                        .button("Duplicate")
                        .accessible_name("Duplicate selected")
                        .clicked()
                    {
                        out.push(Out::CmdSelect(
                            json!({ "cmd": "duplicate_cues", "cues": ids }),
                        ));
                    }
                    if ui
                        .button(RichText::new("Delete").color(theme::ERROR))
                        .accessible_name("Delete selected")
                        .clicked()
                    {
                        out.push(Out::Cmd(json!({ "cmd": "delete_cues", "cues": ids })));
                    }
                });
                ui.end_row();
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_type_change_removes_old_fields() {
        let old = MidiMessage::NoteOn {
            channel: 3,
            note: 60,
            velocity: 100,
        };
        let p = midi_type_patch(&old, "program_change");
        assert_eq!(p["msg"], "program_change");
        assert_eq!(p["channel"], 3);
        assert_eq!(p["note"], Value::Null);
        assert_eq!(p["velocity"], Value::Null);
        assert_eq!(p["program"], 0);
        let mut doc = serde_json::to_value(&old).unwrap();
        sm_server::doc::merge_patch(&mut doc, &p);
        let m: MidiMessage = serde_json::from_value(doc).unwrap();
        assert_eq!(
            m,
            MidiMessage::ProgramChange {
                channel: 3,
                program: 0
            }
        );
    }

    #[test]
    fn timecode_fields() {
        assert_eq!(tc_field_parse(""), Ok(None));
        assert!(tc_field_parse("01:00:10:00").unwrap().is_some());
        assert!(tc_field_parse("25:00:00:00").is_err());
        assert!(tc_field_parse("1:2").is_err());
    }
}
