//! Audio cue inspector: file, waveform with draggable start / end markers,
//! levels, fades, loops and the routing matrix. Waveforms are decoded on a
//! worker thread and cached per file.

use super::fields::{self, Opts};
use super::inspector::{curve_combo, kind_patch, label, time_field};
use super::{fmt_db_field, fmt_secs, parse_db, parse_secs, EditState, Out};
use crate::model::{Phase, Tick, UiState};
use crate::{theme, util};
use egui::{pos2, vec2, Color32, Id, Mesh, Rect, RichText, Sense, Shape, Stroke, Ui};
use serde_json::{json, Value};
use sm_core::model::{AudioCue, Crosspoint, Cue, CueKind, Loop, Show};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

/// Waveform resolution: one peak per 5 ms (zooming in stays sharp), at
/// least 1200 and at most 200 000 per file.
const BUCKETS_PER_SECOND: f64 = 200.0;
const BUCKETS_MIN: usize = 1200;
const BUCKETS_MAX: usize = 200_000;
/// Narrowest waveform view in seconds.
const MIN_VIEW: f64 = 0.1;
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
                    let buckets = sm_audio::probe(&p)
                        .ok()
                        .and_then(|i| i.duration_seconds)
                        .map_or(BUCKETS_MIN, |d| {
                            ((d * BUCKETS_PER_SECOND) as usize).clamp(BUCKETS_MIN, BUCKETS_MAX)
                        });
                    let r = sm_audio::waveform_peaks(&p, buckets).map_err(|e| e.to_string());
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

/// What a drag on the waveform moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Marker {
    /// In point (its handle).
    Start,
    /// Out point (its handle).
    End,
    /// One edge of loop `i` (in the loop strip).
    LoopStart(usize),
    LoopEnd(usize),
    /// The whole loop `i`; `grab` = pointer time minus loop start at the press.
    LoopMove {
        i: usize,
        grab: f64,
    },
    /// A new loop drawn in the loop strip from `anchor`.
    LoopNew {
        anchor: f64,
    },
    /// The playhead (anywhere else): set where the pointer is released.
    Seek,
}

#[derive(Clone, Copy, Debug)]
pub struct WaveDrag {
    cue: u64,
    marker: Marker,
    /// Marker time; for loop drags the loop start.
    time: f64,
    /// Loop end for loop drags.
    time2: f64,
}

/// Height of the loop strip along the bottom of the waveform.
const LOOP_STRIP: f32 = 18.0;
/// Pointer distance (px) that grabs a marker or a loop edge.
const GRAB_PX: f32 = 8.0;

pub(crate) fn show_dir(state: &UiState) -> Option<PathBuf> {
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
            loops_rows(ui, edit, out, id, a);
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

/// Gain slider -60..+12 dB; sent while dragging and on release.
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
        // Sent while dragging too, so a playing cue follows the fader (the
        // server makes the whole drag one undo step).
        let prev = edit.insp.slider.filter(|(s, _)| *s == sid).map(|(_, p)| p);
        edit.insp.slider = Some((sid, v));
        if prev.is_some_and(|p| (p - v).abs() > 0.001) {
            let db = if v <= -60.0 { DB_FLOOR } else { v };
            out.push(Out::Cmd(kind_patch(id, json!({ "gain_db": db }))));
        }
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
    // The loops as drawn (index into `a.loops`, start, end, name): one may be dragged,
    // a new one drawn.
    let mut shown_loops: Vec<(Option<usize>, f64, f64, String)> = a
        .loops
        .iter()
        .enumerate()
        .map(|(i, l)| (Some(i), l.start, l.end, l.label(i)))
        .collect();
    if let Some(d) = drag {
        match d.marker {
            Marker::LoopStart(i) | Marker::LoopEnd(i) | Marker::LoopMove { i, .. } => {
                if let Some(l) = shown_loops.get_mut(i) {
                    (l.1, l.2) = (d.time, d.time2);
                }
            }
            Marker::LoopNew { .. } => {
                shown_loops.push((None, d.time, d.time2, new_loop_name(&a.loops)));
            }
            _ => {}
        }
    }
    // Neighbours of a loop (or of a time): the free room around it.
    let room = |skip: Option<usize>, t: f64| -> (f64, f64) {
        let mut lo = a.start.max(0.0);
        let mut hi = a.end.or(a.file_duration).unwrap_or(f64::INFINITY);
        for (i, l) in a.loops.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            if l.end <= t {
                lo = lo.max(l.end);
            } else if l.start >= t {
                hi = hi.min(l.start);
            }
        }
        (lo, hi)
    };
    // The part of the file shown (zoom): whole file unless zoomed in.
    let (v0, v1) = match edit.insp.wave_view {
        Some((c, a, b)) if c == id && duration > 0.0 => {
            let span = (b - a).clamp(MIN_VIEW.min(duration), duration);
            let a = a.clamp(0.0, duration - span);
            (a, a + span)
        }
        _ => (0.0, duration),
    };
    let span = (v1 - v0).max(1e-9);
    let x_of = |t: f64| {
        if duration > 0.0 {
            rect.left() + ((t - v0) / span) as f32 * rect.width()
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
            let bucket = |t: f64| ((t / duration.max(1e-9)) * n as f64).max(0.0) as usize;
            for px in 0..cols {
                let ta = v0 + span * px as f64 / cols as f64;
                let tb = v0 + span * (px + 1) as f64 / cols as f64;
                let b0 = bucket(ta).min(n - 1);
                let b1 = bucket(tb).max(b0 + 1).min(n);
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
        // Loop strip along the bottom: the vamp as an orange band (its
        // part of the wave tinted too), or a hint how to draw one.
        let strip =
            Rect::from_x_y_ranges(rect.x_range(), (rect.bottom() - LOOP_STRIP)..=rect.bottom());
        painter.rect_filled(strip, 0.0, Color32::from_black_alpha(70));
        let playing_loop = tick
            .active(id)
            .and_then(|t| t.looping.as_ref())
            .map(|l| l.name.clone());
        for (i, ls, le, name) in &shown_loops {
            let (x0, x1) = (x_of(*ls), x_of(*le));
            let live = playing_loop.as_deref() == Some(name.as_str());
            let tint = if live { 0.22 } else { 0.12 };
            painter.rect_filled(
                Rect::from_x_y_ranges(x0..=x1, rect.top()..=strip.top()),
                0.0,
                theme::PAUSED.gamma_multiply(tint),
            );
            let band = Rect::from_x_y_ranges(x0..=x1, strip.y_range());
            painter.rect_filled(
                band,
                3.0,
                theme::PAUSED.gamma_multiply(if live { 1.0 } else { 0.8 }),
            );
            for x in [x0, x1] {
                painter.vline(x, rect.y_range(), Stroke::new(1.0, theme::PAUSED));
            }
            // The crossfade: the audio after the loop end that fades out at each jump back.
            let xf = i.and_then(|i| a.loops.get(i)).map_or(0.0, |l| l.crossfade);
            if xf > 0.0 {
                let xe = x_of(le + xf);
                painter.rect_filled(
                    Rect::from_x_y_ranges(
                        x1..=xe.max(x1),
                        (strip.top() + 5.0)..=(strip.bottom() - 5.0),
                    ),
                    2.0,
                    theme::PAUSED.gamma_multiply(0.35),
                );
            }
            let galley = painter.layout_no_wrap(
                name.to_uppercase(),
                egui::FontId::proportional(10.0),
                Color32::BLACK,
            );
            if band.width() > galley.size().x + 8.0 {
                painter.galley(band.center() - galley.size() / 2.0, galley, Color32::BLACK);
            }
        }
        if shown_loops.is_empty() {
            painter.text(
                strip.center(),
                egui::Align2::CENTER_CENTER,
                "drag here to loop a part",
                egui::FontId::proportional(10.0),
                Color32::from_gray(110),
            );
        }
        // play cursor
        if let Some(act) = tick.active(id).filter(|t| t.phase == Phase::Running) {
            let len = end - start;
            let pos = match act.file_pos {
                Some(p) => Some(p),
                None if a.loops.is_empty() => (len > 0.0).then(|| start + act.elapsed % len),
                None => None,
            };
            if let Some(pos) = pos {
                painter.vline(x_of(pos), rect.y_range(), Stroke::new(1.5, Color32::WHITE));
            }
            ui.ctx().request_repaint();
        } else if let Some(l) = state.loaded(id) {
            // Stopped but loaded: GO / preview start here.
            let x = x_of(start + l);
            painter.vline(x, rect.y_range(), Stroke::new(1.5, theme::LOADED));
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 5.0, rect.top()),
                    pos2(x + 5.0, rect.top()),
                    pos2(x, rect.top() + 7.0),
                ],
                theme::LOADED,
                Stroke::NONE,
            ));
        }
        if let Some(d) = drag.filter(|d| d.marker == Marker::Seek) {
            painter.vline(
                x_of(d.time),
                rect.y_range(),
                Stroke::new(1.0, Color32::from_white_alpha(140)),
            );
        }
    }

    // Pointer: a click (anywhere but a marker handle or the loop strip)
    // sets the playhead; marker handles and the loop strip are dragged.
    let time_at =
        |x: f32| (v0 + ((x - rect.left()) / rect.width()) as f64 * span).clamp(0.0, duration);
    let min_len = 0.01;
    let min_loop = 0.05;
    let r2 = |t: f64| (t * 1000.0).round() / 1000.0;
    if duration > 0.0 {
        let in_strip = |y: f32| y >= rect.bottom() - LOOP_STRIP;
        let near = |px: f32, t: f64| (px - x_of(t)).abs() <= GRAB_PX;
        // What a press at `p` grabs.
        let grab = |p: egui::Pos2| -> (Marker, f64, f64) {
            let t = time_at(p.x);
            if in_strip(p.y) {
                for (i, l) in a.loops.iter().enumerate() {
                    let (ls, le) = (l.start, l.end);
                    if near(p.x, ls) && (!near(p.x, le) || t <= (ls + le) / 2.0) {
                        return (Marker::LoopStart(i), ls, le);
                    }
                    if near(p.x, le) {
                        return (Marker::LoopEnd(i), ls, le);
                    }
                    if t > ls && t < le {
                        return (Marker::LoopMove { i, grab: t - ls }, ls, le);
                    }
                }
                return (Marker::LoopNew { anchor: t }, t, t);
            }
            if near(p.x, start) && (!near(p.x, end) || t <= (start + end) / 2.0) {
                return (Marker::Start, start, 0.0);
            }
            if near(p.x, end) {
                return (Marker::End, end, 0.0);
            }
            (Marker::Seek, t, 0.0)
        };
        // Cursor: what a press would grab.
        if let Some(p) = resp.hover_pos() {
            let icon = match edit
                .insp
                .wave_drag
                .filter(|d| d.cue == id)
                .map(|d| d.marker)
            {
                Some(Marker::LoopMove { .. }) => egui::CursorIcon::Grabbing,
                Some(Marker::Seek) => egui::CursorIcon::Default,
                Some(_) => egui::CursorIcon::ResizeHorizontal,
                None => match grab(p).0 {
                    Marker::LoopMove { .. } => egui::CursorIcon::Grab,
                    Marker::Seek => egui::CursorIcon::Default,
                    Marker::LoopNew { .. } => egui::CursorIcon::Crosshair,
                    _ => egui::CursorIcon::ResizeHorizontal,
                },
            };
            ui.ctx().set_cursor_icon(icon);
        }
        // Decide what is grabbed when the button goes down (the press origin
        // is gone by the time a click is reported).
        let held = edit.insp.wave_drag.is_some_and(|d| d.cue == id);
        let press = ui.input(|i| i.pointer.press_origin());
        let primary = ui.input(|i| i.pointer.primary_down());
        if !held && primary && resp.is_pointer_button_down_on() {
            if let Some(p) = press.filter(|p| rect.contains(*p)) {
                let (marker, time, time2) = grab(p);
                edit.insp.wave_drag = Some(WaveDrag {
                    cue: id,
                    marker,
                    time,
                    time2,
                });
            }
        }
        if let (Some(p), Some(d)) = (
            resp.interact_pointer_pos(),
            edit.insp.wave_drag.as_mut().filter(|d| d.cue == id),
        ) {
            let t = time_at(p.x);
            match d.marker {
                Marker::Start => d.time = t.min(end - min_len).max(0.0),
                Marker::End => d.time = t.max(start + min_len).min(duration),
                Marker::Seek => d.time = t,
                Marker::LoopStart(i) => {
                    let (lo, _) = room(Some(i), d.time2);
                    d.time = t.clamp(lo.max(start), d.time2 - min_loop);
                }
                Marker::LoopEnd(i) => {
                    let (_, hi) = room(Some(i), d.time);
                    d.time2 = t.clamp(d.time + min_loop, hi.min(end));
                }
                Marker::LoopMove { i, grab } => {
                    let len = d.time2 - d.time;
                    let (lo, hi) = room(Some(i), (d.time + d.time2) / 2.0);
                    let (lo, hi) = (lo.max(start), hi.min(end));
                    let s = (t - grab).clamp(lo, (hi - len).max(lo));
                    d.time = s;
                    d.time2 = s + len;
                }
                Marker::LoopNew { anchor } => {
                    let (lo, hi) = room(None, anchor);
                    let (lo, hi) = (lo.max(start), hi.min(end));
                    let a = anchor.clamp(lo, hi);
                    let b = t.clamp(lo, hi);
                    d.time = a.min(b);
                    d.time2 = a.max(b);
                }
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            if let Some(d) = edit.insp.wave_drag.take().filter(|d| d.cue == id) {
                let patch = match d.marker {
                    Marker::Start => Some(json!({ "start": r2(d.time) })),
                    Marker::End if r2(d.time) >= r2(duration) => {
                        Some(json!({ "end": Value::Null }))
                    }
                    Marker::End => Some(json!({ "end": r2(d.time) })),
                    Marker::LoopStart(i) | Marker::LoopEnd(i) | Marker::LoopMove { i, .. } => {
                        let mut loops = a.loops.clone();
                        if let Some(l) = loops.get_mut(i) {
                            l.start = r2(d.time);
                            l.end = r2(d.time2);
                        }
                        Some(loops_patch(loops))
                    }
                    Marker::LoopNew { .. } => {
                        // A click in the strip (or a tiny drag) makes no loop.
                        (d.time2 - d.time >= min_loop).then(|| {
                            let mut loops = a.loops.clone();
                            loops.push(Loop {
                                name: new_loop_name(&a.loops),
                                start: r2(d.time),
                                end: r2(d.time2),
                                ..Loop::default()
                            });
                            loops_patch(loops)
                        })
                    }
                    Marker::Seek => {
                        // The playhead: a playing cue jumps there, a stopped
                        // one is loaded there (GO / preview start there).
                        // Anywhere in the region: past a loop, the cue plays on
                        // from there (the loops before it are behind it).
                        let at = d.time.clamp(start, (end - min_len).max(start));
                        out.push(Out::Cmd(json!({
                            "cmd": "seek_cue", "cue": id, "time": r2(at - start),
                        })));
                        None
                    }
                };
                if let Some(patch) = patch {
                    out.push(Out::Cmd(kind_patch(id, patch)));
                }
            }
        }
        // Released without a click or drag being reported (e.g. outside):
        // drop the grab.
        if !primary
            && !resp.drag_stopped()
            && !resp.clicked()
            && edit.insp.wave_drag.is_some_and(|d| d.cue == id)
        {
            edit.insp.wave_drag = None;
        }
        // Right-click: remove the loop under the pointer (or all), play from the start.
        let hovered_loop = resp
            .hover_pos()
            .or(ui.input(|i| i.pointer.latest_pos()))
            .map(|p| time_at(p.x))
            .and_then(|t| a.loops.iter().position(|l| t >= l.start && t <= l.end));
        if let Some(i) = hovered_loop {
            ui.data_mut(|d| d.insert_temp(Id::unique(("loop_menu", id)), i));
        }
        resp.context_menu(|ui| {
            let i: Option<usize> = ui.data(|d| d.get_temp(Id::unique(("loop_menu", id))));
            if let Some(i) = i.filter(|i| *i < a.loops.len()) {
                if ui
                    .button(format!("Remove \"{}\"", a.loops[i].label(i)))
                    .clicked()
                {
                    let mut loops = a.loops.clone();
                    loops.remove(i);
                    out.push(Out::Cmd(kind_patch(id, loops_patch(loops))));
                    ui.close();
                }
            }
            if !a.loops.is_empty() && ui.button("Remove all loops").clicked() {
                out.push(Out::Cmd(kind_patch(id, json!({ "loops": [] }))));
                ui.close();
            }
            if ui.button("Play from the start").clicked() {
                out.push(Out::Cmd(
                    json!({ "cmd": "seek_cue", "cue": id, "time": 0.0 }),
                ));
                ui.close();
            }
        });
    }
    // Zoom: Ctrl / Cmd + wheel or pinch zooms around the pointer;
    // horizontal scrolling (or Shift + wheel) moves the view.
    let mut wanted = (v0, v1);
    if duration > 0.0 {
        if let Some(p) = resp.hover_pos() {
            let (zoom, dx) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.x));
            if (zoom - 1.0).abs() > 1e-4 {
                let at = time_at(p.x);
                let new_span = (span / f64::from(zoom)).clamp(MIN_VIEW.min(duration), duration);
                let f = (at - v0) / span;
                wanted = (at - f * new_span, at - f * new_span + new_span);
            } else if dx.abs() > 0.0 && span < duration {
                let shift = -f64::from(dx) / f64::from(rect.width()) * span;
                wanted = (v0 + shift, v1 + shift);
                ui.input_mut(|i| i.smooth_scroll_delta.x = 0.0);
            }
        }
        // While playing zoomed in, the view pages along with the play cursor.
        if edit.insp.wave_drag.is_none() && span < duration {
            if let Some(act) = tick
                .active(id)
                .filter(|t| t.phase == Phase::Running && !t.paused)
            {
                let pos = match act.file_pos {
                    Some(p) => Some(p),
                    None if a.loops.is_empty() => {
                        Some(start + act.elapsed % (end - start).max(1e-9))
                    }
                    None => None,
                };
                if let Some(pos) = pos.filter(|p| *p < v0 || *p > v1) {
                    wanted = (pos - span * 0.1, pos + span * 0.9);
                }
            }
        }
    }
    let set_view = |edit: &mut EditState, (a, b): (f64, f64)| {
        let span = (b - a).clamp(MIN_VIEW.min(duration), duration);
        edit.insp.wave_view = if span >= duration - 1e-9 {
            None
        } else {
            let a = a.clamp(0.0, duration - span);
            Some((id, a, a + span))
        };
    };
    if wanted != (v0, v1) {
        set_view(edit, wanted);
        ui.ctx().request_repaint();
    }

    // Overview bar while zoomed in: where the view is; drag or click to move it.
    if duration > 0.0 && span < duration - 1e-9 {
        let (bar, bresp) = ui.allocate_exact_size(vec2(width, 10.0), Sense::click_and_drag());
        let bresp = bresp.accessible_name("Waveform overview");
        let p = ui.painter_at(bar);
        p.rect_filled(bar, 3.0, ui.visuals().extreme_bg_color);
        let bx = |t: f64| bar.left() + (t / duration) as f32 * bar.width();
        p.rect_filled(
            Rect::from_x_y_ranges(bx(start)..=bx(end), bar.y_range()),
            3.0,
            theme::RUNNING.gamma_multiply(0.25),
        );
        p.rect_filled(
            Rect::from_x_y_ranges(bx(v0)..=bx(v1).max(bx(v0) + 3.0), bar.y_range()),
            3.0,
            Color32::from_gray(170),
        );
        if let Some(q) = bresp.interact_pointer_pos() {
            let c = f64::from((q.x - bar.left()) / bar.width()) * duration;
            set_view(edit, (c - span / 2.0, c + span / 2.0));
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
            if duration > 0.0 {
                ui.add_space(8.0);
                let zoomed = span < duration - 1e-9;
                if ui
                    .add_enabled(zoomed, egui::Button::new("Fit").small())
                    .on_hover_text("Show the whole file")
                    .clicked()
                {
                    edit.insp.wave_view = None;
                }
                let centre = (v0 + v1) / 2.0;
                if ui
                    .add_enabled(span > MIN_VIEW, egui::Button::new("+").small())
                    .accessible_name("Zoom in")
                    .on_hover_text("Zoom in (or Ctrl / Cmd + wheel, pinch)")
                    .clicked()
                {
                    set_view(edit, (centre - span / 4.0, centre + span / 4.0));
                }
                if ui
                    .add_enabled(zoomed, egui::Button::new("−").small())
                    .accessible_name("Zoom out")
                    .on_hover_text("Zoom out")
                    .clicked()
                {
                    set_view(edit, (centre - span, centre + span));
                }
                if zoomed {
                    ui.label(
                        RichText::new(format!("{}–{}", util::fmt_time(v0), util::fmt_time(v1)))
                            .small()
                            .monospace()
                            .color(theme::MUTED),
                    );
                }
            }
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

/// "Loop a part" (vamp): the part loops until it is released in the show.
/// The `loops` of an audio cue as a patch (in file order).
fn loops_patch(mut loops: Vec<Loop>) -> Value {
    loops.sort_by(|a, b| a.start.total_cmp(&b.start));
    json!({ "loops": loops })
}

/// "Loop n" with the first n not taken.
fn new_loop_name(loops: &[Loop]) -> String {
    (1..)
        .map(|n| format!("Loop {n}"))
        .find(|n| {
            !loops
                .iter()
                .enumerate()
                .any(|(i, l)| l.label(i).eq_ignore_ascii_case(n))
        })
        .unwrap_or_default()
}

fn loops_rows(ui: &mut Ui, edit: &mut EditState, out: &mut Vec<Out>, id: u64, a: &AudioCue) {
    label(ui, "Vamps");
    ui.horizontal(|ui| {
        if ui
            .button("Add loop")
            .accessible_name("Add loop")
            .on_hover_text(
                "A part that loops until it is released: GO (if set), Release loop in Live, a \
                 hotkey, a Control cue, MIDI or OSC - or after a number of passes. Then the cue \
                 plays on to its next loop or the end. Shown in orange on the waveform: drag in \
                 the strip along its bottom to draw, move or resize a loop.",
            )
            .clicked()
        {
            let region_start = a.start.max(0.0);
            let region_end = a.end.or(a.file_duration).unwrap_or(region_start + 6.0);
            let from = a.loops.iter().map(|l| l.end).fold(region_start, f64::max);
            let len = ((region_end - from) / 3.0).max(0.01);
            let r = |x: f64| (x * 100.0).round() / 100.0;
            let mut loops = a.loops.clone();
            loops.push(Loop {
                name: new_loop_name(&a.loops),
                start: r(from + len),
                end: r(from + 2.0 * len),
                ..Loop::default()
            });
            out.push(Out::Cmd(kind_patch(id, loops_patch(loops))));
        }
        if a.loops.is_empty() {
            ui.label(RichText::new("none").color(theme::MUTED));
        }
    });
    ui.end_row();
    let active = a.active_loops();
    for (i, l) in a.loops.iter().enumerate() {
        let with = {
            let loops = a.loops.clone();
            move |f: &dyn Fn(&mut Loop)| {
                let mut loops = loops.clone();
                if let Some(l) = loops.get_mut(i) {
                    f(l);
                }
                kind_patch(id, loops_patch(loops))
            }
        };
        let name = l.label(i);
        label(ui, &format!("{}.", i + 1));
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let w = with.clone();
                fields::field(
                    ui,
                    &mut edit.drafts,
                    Id::unique(("loop_name", id, i)),
                    &l.name,
                    Opts::new(&format!("Loop {} name", i + 1), 120.0).hint(&name),
                    out,
                    move |t| {
                        let t = t.trim().to_string();
                        Ok(Some(w(&|l: &mut Loop| l.name = t.clone())))
                    },
                );
                ui.label(RichText::new("from").color(theme::MUTED));
                let w = with.clone();
                time_field(
                    ui,
                    edit,
                    out,
                    ("loop_start", id, i),
                    &format!("Loop {} start", i + 1),
                    l.start,
                    move |x| w(&|l: &mut Loop| l.start = x),
                );
                ui.label(RichText::new("to").color(theme::MUTED));
                let w = with.clone();
                time_field(
                    ui,
                    edit,
                    out,
                    ("loop_end", id, i),
                    &format!("Loop {} end", i + 1),
                    l.end,
                    move |x| w(&|l: &mut Loop| l.end = x),
                );
                if ui
                    .small_button("Remove")
                    .accessible_name(format!("Remove loop {}", i + 1))
                    .clicked()
                {
                    let mut loops = a.loops.clone();
                    loops.remove(i);
                    out.push(Out::Cmd(kind_patch(id, loops_patch(loops))));
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Crossfade").color(theme::MUTED));
                let w = with.clone();
                time_field(
                    ui,
                    edit,
                    out,
                    ("loop_xfade", id, i),
                    &format!("Loop {} crossfade", i + 1),
                    l.crossfade,
                    move |x| w(&|l: &mut Loop| l.crossfade = x.clamp(0.0, 10.0)),
                )
                .on_hover_text(
                    "At each jump back the audio after the loop end fades out while the loop \
                     start fades in, so the jump is not heard. 0 = hard jump.",
                );
                ui.label(RichText::new("Passes").color(theme::MUTED));
                let w = with.clone();
                fields::field(
                    ui,
                    &mut edit.drafts,
                    Id::unique(("loop_passes", id, i)),
                    &if l.passes == 0 {
                        String::new()
                    } else {
                        l.passes.to_string()
                    },
                    Opts::new(&format!("Loop {} passes", i + 1), 44.0)
                        .mono()
                        .hint("∞"),
                    out,
                    move |t| {
                        let n: u32 = match t.trim() {
                            "" | "0" | "∞" => 0,
                            t => t
                                .parse()
                                .ok()
                                .filter(|n| (1..=9999).contains(n))
                                .ok_or("Passes must be 1 to 9999 (empty: until released)")?,
                        };
                        Ok(Some(w(&|l: &mut Loop| l.passes = n)))
                    },
                )
                .on_hover_text("Passes before the loop releases itself; empty = until released");
            });
            ui.horizontal_wrapped(|ui| {
                let mut go = l.release_on_go;
                if ui
                    .checkbox(&mut go, "GO releases")
                    .accessible_name(format!("Loop {} GO releases", i + 1))
                    .on_hover_text(
                        "While this loop plays, GO releases it (and starts nothing else); the \
                         next GO starts the next cue.",
                    )
                    .changed()
                {
                    out.push(Out::Cmd(with(&|l: &mut Loop| l.release_on_go = go)));
                }
                let mut now = l.immediate;
                if ui
                    .checkbox(&mut now, "Release at once")
                    .accessible_name(format!("Loop {} release at once", i + 1))
                    .on_hover_text(
                        "A release leaves the loop right away with a crossfade to the audio after \
                         it, instead of at the end of the pass.",
                    )
                    .changed()
                {
                    out.push(Out::Cmd(with(&|l: &mut Loop| l.immediate = now)));
                }
                let mut tc = l.timecode;
                if ui
                    .checkbox(&mut tc, "Timecode loops too")
                    .accessible_name(format!("Loop {} timecode loops too", i + 1))
                    .on_hover_text(
                        "While the part loops, a running timecode jumps back with it, so it keeps \
                         matching the audio (timecode triggers in the loop fire again each pass).",
                    )
                    .changed()
                {
                    out.push(Out::Cmd(with(&|l: &mut Loop| l.timecode = tc)));
                }
            });
            if !active.iter().any(|x| x.name == name) {
                ui.label(
                    RichText::new(
                        "Not played: outside the cue's start / end or inside another loop.",
                    )
                    .color(theme::WARN),
                );
            }
        });
        ui.end_row();
    }
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
