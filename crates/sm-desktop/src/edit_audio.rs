//! Audio cue inspector: file, waveform with draggable start / end markers,
//! levels, fades, loops and the routing matrix. Waveforms are decoded on a
//! worker thread and cached per file.

use super::fields::{self, Opts};
use super::inspector::{curve_combo, kind_patch, time_field};
use super::{fmt_db_field, fmt_secs, parse_db, parse_secs, EditState, Out};
use crate::model::{Phase, Tick, UiState};
use crate::{theme, util};
use egui::{pos2, vec2, Color32, Id, Mesh, Rect, RichText, Sense, Shape, Stroke, Ui};
use serde_json::{json, Value};
use sm_core::model::{AudioCue, Crosspoint, Cue, CueKind, Show};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

const BUCKETS: usize = 1200;
const DB_FLOOR: f32 = -120.0;

type Peaks = Arc<Vec<(f32, f32)>>;

enum Wave {
    Loading(mpsc::Receiver<Result<Vec<(f32, f32)>, String>>),
    Ready(Peaks),
    Failed(String),
}

/// What the waveform view shows.
pub enum WaveView {
    Loading,
    Ready(Peaks),
    Failed(String),
}

/// Waveform cache (one decode per file, off the UI thread).
#[derive(Default)]
pub struct Waves {
    map: HashMap<PathBuf, Wave>,
}

impl Waves {
    pub fn get(&mut self, ctx: &egui::Context, path: &Path) -> WaveView {
        if !self.map.contains_key(path) {
            if self.map.len() > 64 {
                self.map.retain(|_, w| matches!(w, Wave::Loading(_)));
            }
            let (tx, rx) = mpsc::channel();
            let p = path.to_path_buf();
            let repaint = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("waveform".into())
                .spawn(move || {
                    let r = sm_audio::waveform_peaks(&p, BUCKETS).map_err(|e| e.to_string());
                    let _ = tx.send(r);
                    repaint.request_repaint();
                });
            let w = match spawned {
                Ok(_) => Wave::Loading(rx),
                Err(e) => Wave::Failed(e.to_string()),
            };
            self.map.insert(path.to_path_buf(), w);
        }
        let Some(w) = self.map.get_mut(path) else {
            return WaveView::Loading;
        };
        if let Wave::Loading(rx) = w {
            match rx.try_recv() {
                Ok(Ok(p)) => *w = Wave::Ready(Arc::new(p)),
                Ok(Err(e)) => *w = Wave::Failed(e),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    *w = Wave::Failed("waveform worker stopped".into());
                }
            }
        }
        match w {
            Wave::Loading(_) => WaveView::Loading,
            Wave::Ready(p) => WaveView::Ready(p.clone()),
            Wave::Failed(e) => WaveView::Failed(e.clone()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Marker {
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
pub struct WaveDrag {
    cue: u64,
    marker: Marker,
    time: f64,
}

fn show_dir(state: &UiState) -> Option<PathBuf> {
    state
        .show_path
        .as_deref()
        .and_then(|p| Path::new(p).parent())
        .map(Path::to_path_buf)
}

pub(crate) fn audio_tab(
    ui: &mut Ui,
    state: &UiState,
    tick: &Tick,
    cue: &Cue,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let CueKind::Audio(a) = &cue.kind else { return };
    let id = cue.id;
    // File row
    ui.horizontal(|ui| {
        ui.label(RichText::new("File").color(theme::MUTED));
        if ui
            .button("Choose…")
            .accessible_name("Choose audio file")
            .on_hover_text("Choose the audio file of this cue")
            .clicked()
        {
            out.push(Out::ChooseAudio(id));
        }
        let path = if a.file.is_empty() {
            RichText::new("(no file)").italics().color(theme::MUTED)
        } else {
            RichText::new(&a.file).monospace()
        };
        let resp = ui.add(egui::Label::new(path).truncate());
        if !a.file.is_empty() {
            resp.on_hover_text(&a.file);
        }
    });
    ui.add_space(4.0);
    waveform(ui, state, tick, cue, a, edit, out);
    ui.add_space(8.0);
    let grid = |id: (&str, u64)| {
        egui::Grid::new(id)
            .num_columns(2)
            .spacing([12.0, 8.0])
            .min_col_width(64.0)
    };
    let label = |ui: &mut Ui, t: &str| {
        ui.label(RichText::new(t).color(theme::MUTED));
    };
    ui.horizontal_top(|ui| {
        grid(("audio_a", id)).show(ui, |ui| {
            label(ui, "Start");
            time_field(
                ui,
                edit,
                out,
                ("audio_start", id),
                "Start time",
                a.start,
                move |v| kind_patch(id, json!({ "start": v })),
            );
            ui.end_row();
            label(ui, "End");
            let end = a.end.map(fmt_secs).unwrap_or_default();
            let hint = a
                .file_duration
                .map(fmt_secs)
                .unwrap_or_else(|| "end".into());
            fields::field(
                ui,
                &mut edit.drafts,
                Id::unique(("audio_end", id)),
                &end,
                Opts::new("End time", 90.0).mono().hint(&hint),
                out,
                move |t| Ok(Some(kind_patch(id, json!({ "end": parse_secs(t)? })))),
            );
            ui.end_row();
            label(ui, "Loops");
            ui.horizontal(|ui| {
                let infinite = a.loop_count == 0;
                ui.add_enabled_ui(!infinite, |ui| {
                    fields::field(
                        ui,
                        &mut edit.drafts,
                        Id::unique(("audio_loops", id)),
                        &a.loop_count.max(1).to_string(),
                        Opts::new("Loop count", 50.0).mono(),
                        out,
                        move |t| {
                            let n: u32 = t
                                .trim()
                                .parse()
                                .ok()
                                .filter(|n| (1..=9999).contains(n))
                                .ok_or("Loops must be 1 to 9999")?;
                            Ok(Some(kind_patch(id, json!({ "loop_count": n }))))
                        },
                    );
                });
                let mut inf = infinite;
                if ui
                    .checkbox(&mut inf, "Infinite")
                    .accessible_name("Loop forever")
                    .changed()
                {
                    out.push(Out::Cmd(kind_patch(
                        id,
                        json!({ "loop_count": if inf { 0 } else { 1 } }),
                    )));
                }
            });
            ui.end_row();
        });
        ui.add_space(24.0);
        grid(("audio_b", id)).show(ui, |ui| {
            label(ui, "Gain");
            ui.horizontal(|ui| {
                gain_slider(ui, edit, out, id, a.gain_db);
                fields::field(
                    ui,
                    &mut edit.drafts,
                    Id::unique(("audio_gain", id)),
                    &fmt_db_field(a.gain_db),
                    Opts::new("Gain dB", 56.0).mono(),
                    out,
                    move |t| {
                        Ok(Some(kind_patch(
                            id,
                            json!({ "gain_db": parse_db(t)?.min(12.0) }),
                        )))
                    },
                );
                ui.label("dB");
            });
            ui.end_row();
            label(ui, "Fade in");
            time_field(
                ui,
                edit,
                out,
                ("audio_fin", id),
                "Fade in time",
                a.fade_in,
                move |v| kind_patch(id, json!({ "fade_in": v })),
            );
            ui.end_row();
            label(ui, "Fade out");
            time_field(
                ui,
                edit,
                out,
                ("audio_fout", id),
                "Fade out time",
                a.fade_out,
                move |v| kind_patch(id, json!({ "fade_out": v })),
            );
            ui.end_row();
            label(ui, "Curve");
            if let Some(c) = curve_combo(ui, ("audio_curve", id), a.fade_curve) {
                out.push(Out::Cmd(kind_patch(id, json!({ "fade_curve": c }))));
            }
            ui.end_row();
        });
    });
}

/// Gain slider -60..+12 dB; the value is sent once, on release.
fn gain_slider(ui: &mut Ui, edit: &mut EditState, out: &mut Vec<Out>, id: u64, gain_db: f32) {
    let sid = Id::unique(("audio_gain_slider", id));
    let mut v = match edit.insp.slider {
        Some((s, v)) if s == sid => v,
        _ => gain_db.clamp(-60.0, 12.0),
    };
    let r = ui
        .add(
            egui::Slider::new(&mut v, -60.0..=12.0)
                .show_value(false)
                .step_by(0.5),
        )
        .accessible_name("Gain slider")
        .on_hover_text("Drag to set the level; double-click for 0 dB");
    if r.double_clicked() {
        edit.insp.slider = None;
        out.push(Out::Cmd(kind_patch(id, json!({ "gain_db": 0.0 }))));
        return;
    }
    if r.dragged() {
        edit.insp.slider = Some((sid, v));
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        edit.insp.slider = None;
        let db = if v <= -60.0 { DB_FLOOR } else { v };
        if (db - gain_db).abs() > 0.001 {
            out.push(Out::Cmd(kind_patch(id, json!({ "gain_db": db }))));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn waveform(
    ui: &mut Ui,
    state: &UiState,
    tick: &Tick,
    cue: &Cue,
    a: &AudioCue,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let id = cue.id;
    let width = ui.available_width().max(200.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 96.0), Sense::click_and_drag());
    let resp = resp.accessible_name("Waveform");
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);

    let view = if a.file.is_empty() {
        WaveView::Failed("No file".into())
    } else {
        let path = sm_core::showfile::resolve_media(show_dir(state).as_deref(), &a.file);
        edit.waves.get(ui.ctx(), &path)
    };
    let duration = a.file_duration.unwrap_or(0.0);
    let drag = edit.insp.wave_drag.filter(|d| d.cue == id);
    let start = match drag {
        Some(d) if d.marker == Marker::Start => d.time,
        _ => a.start.clamp(0.0, duration.max(0.0)),
    };
    let end = match drag {
        Some(d) if d.marker == Marker::End => d.time,
        _ => a.end.unwrap_or(duration).min(duration),
    };
    let x_of = |t: f64| {
        if duration > 0.0 {
            rect.left() + (t / duration) as f32 * rect.width()
        } else {
            rect.left()
        }
    };
    let (xs, xe) = (x_of(start), x_of(end));
    let mid = rect.center().y;
    let half = rect.height() * 0.46;
    match &view {
        WaveView::Ready(peaks) if !peaks.is_empty() => {
            let mut mesh = Mesh::default();
            let n = peaks.len();
            let cols = rect.width().floor().max(1.0) as usize;
            for px in 0..cols {
                let b0 = px * n / cols;
                let b1 = ((px + 1) * n / cols).max(b0 + 1).min(n);
                let (mut lo, mut hi) = (0.0f32, 0.0f32);
                for (l, h) in &peaks[b0..b1] {
                    lo = lo.min(*l);
                    hi = hi.max(*h);
                }
                let x = rect.left() + px as f32;
                let inside = x >= xs && x <= xe;
                let color = if inside {
                    theme::RUNNING
                } else {
                    Color32::from_gray(70)
                };
                let r = Rect::from_min_max(
                    pos2(x, mid - hi.clamp(-1.0, 1.0) * half),
                    pos2(
                        x + 1.0,
                        (mid - lo.clamp(-1.0, 1.0) * half).max(mid - hi * half + 1.0),
                    ),
                );
                mesh.add_colored_rect(r, color);
            }
            painter.add(Shape::mesh(mesh));
        }
        WaveView::Ready(_) => {}
        WaveView::Loading => {
            let spinner = Rect::from_center_size(rect.center(), vec2(20.0, 20.0));
            ui.put(spinner, egui::Spinner::new());
        }
        WaveView::Failed(e) => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if a.file.is_empty() {
                    "No file".to_string()
                } else {
                    format!("Waveform not available: {e}")
                },
                egui::FontId::proportional(13.0),
                if a.file.is_empty() {
                    theme::MUTED
                } else {
                    theme::ERROR
                },
            );
        }
    }
    painter.hline(
        rect.x_range(),
        mid,
        Stroke::new(1.0, Color32::from_gray(45)),
    );
    if duration > 0.0 {
        // dim outside the region, markers with flags
        let shade = Color32::from_black_alpha(110);
        painter.rect_filled(
            Rect::from_x_y_ranges(rect.left()..=xs, rect.y_range()),
            0.0,
            shade,
        );
        painter.rect_filled(
            Rect::from_x_y_ranges(xe..=rect.right(), rect.y_range()),
            0.0,
            shade,
        );
        for (x, color, right) in [(xs, theme::NEXT, true), (xe, theme::ERROR, false)] {
            painter.vline(x, rect.y_range(), Stroke::new(2.0, color));
            let s = if right { 10.0 } else { -10.0 };
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x, rect.top()),
                    pos2(x + s, rect.top()),
                    pos2(x, rect.top() + 10.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        // play cursor
        if let Some(act) = tick.active(id).filter(|t| t.phase == Phase::Running) {
            let len = end - start;
            if len > 0.0 {
                let pos = start + act.elapsed % len;
                painter.vline(x_of(pos), rect.y_range(), Stroke::new(1.5, Color32::WHITE));
            }
            ui.ctx().request_repaint();
        }
    }

    // Marker dragging: grab the nearest marker; commit on release.
    let time_at =
        |x: f32| (((x - rect.left()) / rect.width()) as f64 * duration).clamp(0.0, duration);
    let min_len = 0.01;
    if duration > 0.0 {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if let Some(p) = resp.interact_pointer_pos() {
            let t = time_at(p.x);
            let marker = match edit.insp.wave_drag {
                Some(d) if d.cue == id => d.marker,
                _ if (t - start).abs() <= (t - end).abs() => Marker::Start,
                _ => Marker::End,
            };
            if resp.drag_started() || resp.dragged() || resp.clicked() {
                let time = match marker {
                    Marker::Start => t.min(end - min_len).max(0.0),
                    Marker::End => t.max(start + min_len).min(duration),
                };
                edit.insp.wave_drag = Some(WaveDrag {
                    cue: id,
                    marker,
                    time,
                });
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            if let Some(d) = edit.insp.wave_drag.take().filter(|d| d.cue == id) {
                let t = (d.time * 100.0).round() / 100.0;
                let patch = match d.marker {
                    Marker::Start => json!({ "start": t }),
                    Marker::End if t >= (duration * 100.0).round() / 100.0 => {
                        json!({ "end": Value::Null })
                    }
                    Marker::End => json!({ "end": t }),
                };
                out.push(Out::Cmd(kind_patch(id, patch)));
            }
        }
    }
    // Info line under the waveform
    ui.horizontal(|ui| {
        let small = |t: String| RichText::new(t).small().monospace();
        ui.label(small(format!("start {}", util::fmt_time(start))).color(theme::NEXT));
        let info = match (&view, a.file_duration) {
            (WaveView::Failed(_), _) if a.file.is_empty() => String::new(),
            (_, Some(d)) => format!(
                "{} ch · file {} · region {}",
                a.channels,
                util::fmt_time(d),
                util::fmt_time((end - start).max(0.0))
            ),
            (WaveView::Loading, None) => "loading waveform…".into(),
            _ => "duration unknown".into(),
        };
        ui.label(RichText::new(info).small().color(theme::MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(small(format!("end {}", util::fmt_time(end))).color(theme::ERROR));
        });
    });
}

// ---------------------------------------------------------------------------
// Routing matrix
// ---------------------------------------------------------------------------

/// Placeholder that keeps a routing explicit but silent (empty = default routing).
const SILENT: Crosspoint = Crosspoint {
    src: 0,
    out: 0,
    db: DB_FLOOR,
};

fn is_on(p: &Crosspoint) -> bool {
    p.db > DB_FLOOR
}

/// New routing with crosspoint `src -> out` set to `db` (`None` = off),
/// starting from the effective routing (the default one if none is set).
pub fn set_point(
    a: &AudioCue,
    outputs: usize,
    src: u16,
    out: u16,
    db: Option<f32>,
) -> Vec<Crosspoint> {
    let mut next: Vec<Crosspoint> = a
        .effective_routing(outputs)
        .into_iter()
        .filter(|p| !(p.src == src && p.out == out) && is_on(p))
        .collect();
    if let Some(db) = db.filter(|d| *d > DB_FLOOR) {
        next.push(Crosspoint { src, out, db });
    }
    next.sort_by_key(|p| (p.src, p.out));
    if next.is_empty() {
        vec![SILENT]
    } else {
        next
    }
}

fn source_name(sources: u16, src: u16) -> String {
    match (sources, src) {
        (1, _) => "Mono".into(),
        (2, 0) => "L".into(),
        (2, _) => "R".into(),
        _ => format!("Ch {}", src + 1),
    }
}

pub(crate) fn routing_tab(ui: &mut Ui, show: &Show, cue: &Cue, out: &mut Vec<Out>) {
    let CueKind::Audio(a) = &cue.kind else { return };
    let id = cue.id;
    let outputs = &show.patch.outputs;
    let sources = a.channels.max(1);
    let is_default = a.routing.is_empty();
    let set = |r: Vec<Crosspoint>| Out::Cmd(kind_patch(id, json!({ "routing": r })));
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!is_default, egui::Button::new("Default"))
            .accessible_name("Default routing")
            .on_hover_text("Use the default routing (file channels to the first outputs)")
            .clicked()
        {
            out.push(set(Vec::new()));
        }
        if ui
            .button("Clear")
            .accessible_name("Clear routing")
            .on_hover_text("Route nothing (the cue plays silently)")
            .clicked()
        {
            out.push(set(vec![SILENT]));
        }
        if sources == 1 && !show.patch.groups.is_empty() {
            egui::ComboBox::from_id_salt(("route_group", id))
                .selected_text("Send to group…")
                .width(180.0)
                .show_ui(ui, |ui| {
                    for g in &show.patch.groups {
                        let t = format!("{} ({} out)", g.name, g.outputs.len());
                        if ui.selectable_label(false, t).clicked() {
                            let r: Vec<Crosspoint> = g
                                .outputs
                                .iter()
                                .filter(|o| usize::from(**o) < outputs.len())
                                .map(|o| Crosspoint {
                                    src: 0,
                                    out: *o,
                                    db: 0.0,
                                })
                                .collect();
                            out.push(set(if r.is_empty() { vec![SILENT] } else { r }));
                        }
                    }
                });
        }
        ui.label(
            RichText::new(if is_default {
                "Default routing · click a cell for 0 dB / off, right-click to set a level"
            } else {
                "Custom routing · click a cell for 0 dB / off, right-click to set a level"
            })
            .small()
            .color(theme::MUTED),
        );
    });
    ui.add_space(6.0);
    if outputs.is_empty() {
        ui.label(RichText::new("No outputs are patched. Add outputs in Setup.").color(theme::WARN));
        return;
    }
    let eff = a.effective_routing(outputs.len());
    egui::ScrollArea::horizontal()
        .id_salt(("route_scroll", id))
        .show(ui, |ui| {
            egui::Grid::new(("route_grid", id))
                .spacing([4.0, 4.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("source / output").small().color(theme::MUTED));
                    for (i, o) in outputs.iter().enumerate() {
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(format!("{}", i + 1))
                                    .small()
                                    .color(theme::MUTED),
                            );
                            ui.label(RichText::new(&o.name).strong());
                        })
                        .response
                        .on_hover_text(format!(
                            "Output {} = device channel {}",
                            i + 1,
                            o.channel + 1
                        ));
                    }
                    ui.end_row();
                    for src in 0..sources {
                        ui.label(RichText::new(source_name(sources, src)).strong());
                        for (oi, o) in outputs.iter().enumerate() {
                            let oi = oi as u16;
                            let point =
                                eff.iter().find(|p| p.src == src && p.out == oi && is_on(p));
                            let text = point.map(|p| fmt_db_field(p.db)).unwrap_or_default();
                            let (fill, color) = match (point, is_default) {
                                (Some(_), false) => (theme::RUNNING, Color32::WHITE),
                                (Some(_), true) => (theme::RUNNING_BG, Color32::from_gray(210)),
                                (None, _) => (ui.visuals().extreme_bg_color, theme::MUTED),
                            };
                            let resp = ui
                                .add(
                                    egui::Button::new(RichText::new(text).monospace().color(color))
                                        .fill(fill)
                                        .min_size(vec2(56.0, 28.0)),
                                )
                                .accessible_name(format!(
                                    "Route {} to {}",
                                    source_name(sources, src),
                                    o.name
                                ));
                            if resp.clicked() {
                                let db = if point.is_some() { None } else { Some(0.0) };
                                out.push(set(set_point(a, outputs.len(), src, oi, db)));
                            }
                            resp.context_menu(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{} to {}",
                                        source_name(sources, src),
                                        o.name
                                    ))
                                    .strong(),
                                );
                                for (t, db) in [
                                    ("0 dB", Some(0.0)),
                                    ("-3 dB", Some(-3.0)),
                                    ("-6 dB", Some(-6.0)),
                                    ("-10 dB", Some(-10.0)),
                                    ("Off", None),
                                ] {
                                    if ui.button(t).clicked() {
                                        out.push(set(set_point(a, outputs.len(), src, oi, db)));
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                let tid = ui.scope_id().with("route_db");
                                let mut text =
                                    ui.data(|d| d.get_temp::<String>(tid)).unwrap_or_else(|| {
                                        point
                                            .map(|p| fmt_db_field(p.db))
                                            .unwrap_or_else(|| "0".into())
                                    });
                                ui.horizontal(|ui| {
                                    ui.label("Level");
                                    let r = ui
                                        .add(
                                            egui::TextEdit::singleline(&mut text)
                                                .desired_width(60.0),
                                        )
                                        .accessible_name("Crosspoint level");
                                    ui.label("dB");
                                    if r.lost_focus()
                                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    {
                                        match parse_db(&text) {
                                            Ok(db) => {
                                                let db = (db > DB_FLOOR).then_some(db.min(12.0));
                                                out.push(set(set_point(
                                                    a,
                                                    outputs.len(),
                                                    src,
                                                    oi,
                                                    db,
                                                )));
                                                ui.close();
                                            }
                                            Err(e) => out.push(Out::Error(e)),
                                        }
                                    }
                                });
                                ui.data_mut(|d| d.insert_temp(tid, text));
                            });
                        }
                        ui.end_row();
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_points() {
        let a = AudioCue {
            channels: 2,
            ..AudioCue::default()
        };
        // from default (L->1, R->2) turn R->2 off
        let r = set_point(&a, 4, 1, 1, None);
        assert_eq!(
            r,
            vec![Crosspoint {
                src: 0,
                out: 0,
                db: 0.0
            }]
        );
        // everything off keeps an explicit silent routing
        let a2 = AudioCue {
            routing: r,
            ..a.clone()
        };
        assert_eq!(set_point(&a2, 4, 0, 0, None), vec![SILENT]);
        let r = set_point(&a, 4, 0, 3, Some(-6.0));
        assert_eq!(r.len(), 3);
        assert!(r.contains(&Crosspoint {
            src: 0,
            out: 3,
            db: -6.0
        }));
    }
}
