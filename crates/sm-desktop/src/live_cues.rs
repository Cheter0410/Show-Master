//! The live cue window: big rows, playhead highlight with auto-scroll,
//! running / paused / pre-wait / post-wait / error / loaded / disarmed states.
//! Virtualised (only visible rows are laid out), so 500+ cues stay cheap.

use crate::model::{Active, Phase, Tick, UiState};
use crate::theme;
use crate::util;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use sm_core::midi::MidiMessage;
use sm_core::model::{ContinueMode, ControlAction, Cue, CueKind, GroupMode, Show, TcAction};
use std::sync::Arc;

/// Height of one cue row (without spacing).
pub const ROW_H: f32 = 52.0;
/// Row height with touch-friendly controls.
pub const ROW_H_TOUCH: f32 = 60.0;
/// Space between rows.
pub const ROW_GAP: f32 = 2.0;

/// Scroll state of the cue window.
#[derive(Debug, Default)]
pub struct CueWindowState {
    last_playhead: Option<u64>,
    last_list: Option<u64>,
    last_rows: usize,
    last_follow: bool,
    /// Scroll offset and viewport height of the last frame.
    offset: f32,
    view_h: f32,
}

/// What the user did in the cue window.
pub enum CueAction {
    SetPlayhead(u64),
    Select(u64),
}

/// Visible state of a row, in order of importance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowState {
    Idle,
    PreWait,
    Running,
    PostWait,
    Paused,
    Error,
    Loaded,
}

pub fn row_state(active: Option<&Active>, error: bool, loaded: bool) -> RowState {
    match active {
        Some(a) if a.paused => RowState::Paused,
        Some(a) => match a.phase {
            Phase::PreWait => RowState::PreWait,
            Phase::Running => RowState::Running,
            Phase::PostWait => RowState::PostWait,
        },
        None if error => RowState::Error,
        None if loaded => RowState::Loaded,
        None => RowState::Idle,
    }
}

/// Flatten a cue tree into (cue, depth) rows.
pub fn flatten<'a>(cues: &'a [Cue], depth: u8, out: &mut Vec<(&'a Cue, u8)>) {
    for c in cues {
        out.push((c, depth));
        if let CueKind::Group(g) = &c.kind {
            flatten(&g.children, depth.saturating_add(1), out);
        }
    }
}

pub fn type_label(cue: &Cue) -> &'static str {
    match &cue.kind {
        CueKind::Audio(_) => "Audio",
        CueKind::Fade(_) => "Fade",
        CueKind::Midi(_) => "MIDI",
        CueKind::Osc(_) => "OSC",
        CueKind::Http(_) => "HTTP",
        CueKind::Timecode(_) => "Timecode",
        CueKind::Control(_) => "Control",
        CueKind::Group(_) => "Group",
        CueKind::Memo => "Memo",
    }
}

/// `12 Intro` (or just the name / number).
pub fn cue_label(cue: &Cue) -> String {
    match (cue.number.is_empty(), cue.name.is_empty()) {
        (false, false) => format!("{} {}", cue.number, cue.name),
        (false, true) => cue.number.clone(),
        (true, false) => cue.name.clone(),
        (true, true) => type_label(cue).to_string(),
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// One-line description of what a cue acts on.
pub fn cue_target(cue: &Cue, show: &Show) -> String {
    let target = |id: Option<u64>| {
        id.and_then(|id| show.cue(id))
            .map_or_else(|| "(no target)".to_string(), cue_label)
    };
    match &cue.kind {
        CueKind::Audio(a) if a.file.is_empty() => "(no file)".into(),
        CueKind::Audio(a) => base_name(&a.file).to_string(),
        CueKind::Fade(f) => format!(
            "-> {}{}",
            target(f.target),
            if f.stop_target { " · stop" } else { "" }
        ),
        CueKind::Midi(m) => {
            let what = match &m.message {
                MidiMessage::NoteOn { note, .. } => format!("note on {note}"),
                MidiMessage::NoteOff { note, .. } => format!("note off {note}"),
                MidiMessage::ControlChange {
                    controller, value, ..
                } => format!("CC {controller} = {value}"),
                MidiMessage::ProgramChange { program, .. } => format!("program {program}"),
                MidiMessage::PitchBend { value, .. } => format!("pitch bend {value}"),
                MidiMessage::Sysex { .. } => "SysEx".into(),
                MidiMessage::Msc { q_number, .. } => format!("MSC {q_number}"),
                #[allow(unreachable_patterns)]
                _ => "message".into(),
            };
            let port = if m.port.is_empty() {
                "(no port)"
            } else {
                &m.port
            };
            format!("{port} · {what}")
        }
        CueKind::Osc(m) => m.summary(),
        CueKind::Http(r) => r.summary(),
        CueKind::Timecode(t) => match &t.tc {
            TcAction::Start => "TC start".into(),
            TcAction::StartFrom { from } => format!("TC start from {from}"),
            TcAction::Stop => "TC stop".into(),
            TcAction::Pause => "TC pause".into(),
            TcAction::Resume => "TC resume".into(),
            TcAction::Relocate { to } => format!("TC relocate {to}"),
        },
        CueKind::Control(c) => {
            let action = match c.action {
                ControlAction::Start => "start",
                ControlAction::Stop => "stop",
                ControlAction::Pause => "pause",
                ControlAction::Resume => "resume",
                ControlAction::LoadToTime => "load to time",
                ControlAction::Arm => "arm",
                ControlAction::Disarm => "disarm",
                ControlAction::GoTo => "go to",
                ControlAction::Reset => return "reset show".into(),
            };
            format!("{action} -> {}", target(c.target))
        }
        CueKind::Group(g) => {
            let mode = match g.mode {
                GroupMode::Parallel => "parallel",
                GroupMode::Playlist => "playlist",
                GroupMode::Random => "random",
            };
            let n = g.children.len();
            format!("{mode} · {n} cue{}", if n == 1 { "" } else { "s" })
        }
        CueKind::Memo => String::new(),
    }
}

/// Single-line text, elided with "…" to `width`.
pub fn elided(
    ui: &egui::Ui,
    text: String,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text, font, color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// Colours of a state: (accent, row background).
pub fn state_colors(st: RowState) -> Option<(Color32, Color32)> {
    Some(match st {
        RowState::Running | RowState::PreWait | RowState::PostWait => {
            (theme::RUNNING, theme::RUNNING_BG)
        }
        RowState::Paused => (theme::PAUSED, Color32::from_rgb(72, 50, 12)),
        RowState::Error => (theme::ERROR, theme::ERROR_BG),
        RowState::Loaded => (theme::LOADED, Color32::from_rgb(12, 56, 66)),
        RowState::Idle => return None,
    })
}

/// Paint a status glyph (shapes, no font glyphs) centred at `c`.
fn status_glyph(p: &egui::Painter, c: Pos2, st: RowState, armed: bool, s: f32) {
    match st {
        RowState::Running | RowState::PostWait => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    c + Vec2::new(-s * 0.4, -s * 0.5),
                    c + Vec2::new(s * 0.55, 0.0),
                    c + Vec2::new(-s * 0.4, s * 0.5),
                ],
                theme::RUNNING,
                Stroke::NONE,
            ));
        }
        RowState::PreWait => {
            p.circle_stroke(c, s * 0.45, Stroke::new(2.0, theme::RUNNING));
            p.line_segment(
                [c, c + Vec2::new(0.0, -s * 0.3)],
                Stroke::new(2.0, theme::RUNNING),
            );
            p.line_segment(
                [c, c + Vec2::new(s * 0.22, 0.0)],
                Stroke::new(2.0, theme::RUNNING),
            );
        }
        RowState::Paused => {
            for dx in [-s * 0.22, s * 0.22] {
                p.rect_filled(
                    Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(s * 0.22, s * 0.9)),
                    1.0,
                    theme::PAUSED,
                );
            }
        }
        RowState::Error => {
            p.circle_filled(c, s * 0.5, theme::ERROR);
            p.text(
                c,
                Align2::CENTER_CENTER,
                "!",
                FontId::proportional(s * 0.85),
                Color32::WHITE,
            );
        }
        RowState::Loaded => {
            p.circle_stroke(c, s * 0.42, Stroke::new(2.0, theme::LOADED));
            p.circle_filled(c, s * 0.18, theme::LOADED);
        }
        RowState::Idle if !armed => {
            p.circle_stroke(c, s * 0.42, Stroke::new(1.5, theme::MUTED));
            p.line_segment(
                [
                    c + Vec2::new(-s * 0.3, s * 0.3),
                    c + Vec2::new(s * 0.3, -s * 0.3),
                ],
                Stroke::new(1.5, theme::MUTED),
            );
        }
        RowState::Idle => {}
    }
}

/// A small filled tag; returns its width. Painted right-aligned at `right`.
fn tag(p: &egui::Painter, right: f32, cy: f32, text: &str, fg: Color32, bg: Color32) -> f32 {
    let g = p.layout_no_wrap(text.to_string(), FontId::proportional(11.5), fg);
    let size = g.size() + Vec2::new(10.0, 4.0);
    let r = Rect::from_min_size(Pos2::new(right - size.x, cy - size.y / 2.0), size);
    p.rect_filled(r, 3.0, bg);
    p.galley(r.min + Vec2::new(5.0, 2.0), g, fg);
    size.x
}

/// Time column text of an active cue: (big text, small caption, progress).
pub fn active_timing(a: &Active) -> (String, &'static str, Option<f32>) {
    let caption = match (a.phase, a.duration.is_some()) {
        (Phase::PreWait, _) => "pre-wait",
        (Phase::PostWait, _) => "post-wait",
        (Phase::Running, true) => "remaining",
        (Phase::Running, false) => "elapsed",
    };
    match a.remaining() {
        Some(r) => (format!("-{}", util::fmt_time(r)), caption, a.progress()),
        None => (util::fmt_time(a.elapsed), caption, None),
    }
}

pub struct CueWindowInput<'a> {
    pub state: &'a UiState,
    pub tick: &'a Tick,
    pub list: Option<u64>,
    pub selection: &'a [u64],
    pub follow: bool,
    pub high_contrast: bool,
    /// Row height (`ROW_H`, or `ROW_H_TOUCH` in touch mode).
    pub row_h: f32,
}

/// Draw the cue rows into the current ui (fills it). Returns user actions.
pub fn cue_window(
    ui: &mut egui::Ui,
    input: &CueWindowInput<'_>,
    win: &mut CueWindowState,
) -> Vec<CueAction> {
    let mut actions = Vec::new();
    let show = &input.state.show;
    let Some(list) = input.list.and_then(|l| show.list(l)) else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No cue list").color(theme::MUTED))
        });
        return actions;
    };
    let mut flat: Vec<(&Cue, u8)> = Vec::with_capacity(list.cues.len());
    flatten(&list.cues, 0, &mut flat);
    let playhead = input.state.playhead(list.id);
    let ph_index = playhead.and_then(|p| flat.iter().position(|(c, _)| c.id == p));

    if flat.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new("This cue list is empty.")
                    .size(18.0)
                    .color(theme::MUTED),
            )
        });
        return actions;
    }

    // Follow the playhead: scroll only when it moved (or following was
    // switched on) and the NEXT row is not comfortably visible.
    let row_h = input.row_h;
    let step = row_h + ROW_GAP;
    let mut scroll_to = None;
    let changed = win.last_playhead != playhead
        || win.last_list != Some(list.id)
        || win.last_rows != flat.len()
        || (input.follow && !win.last_follow);
    if input.follow && changed {
        if let Some(i) = ph_index {
            let top = i as f32 * step;
            let view = if win.view_h > 0.0 {
                win.view_h
            } else {
                ui.available_height()
            };
            let margin = (view - row_h).clamp(0.0, step * 1.5);
            let visible = top >= win.offset && top + row_h <= win.offset + view - margin;
            if !visible {
                let desired = (top - view * 0.3).max(0.0);
                scroll_to = Some((desired / step).floor() * step);
            }
        }
    }
    win.last_playhead = playhead;
    win.last_list = Some(list.id);
    win.last_rows = flat.len();
    win.last_follow = input.follow;

    let mut area = egui::ScrollArea::vertical()
        .id_salt("live_cue_rows")
        .auto_shrink([false, false])
        .scroll_source(crate::responsive::scroll_source(ui.ctx()));
    if let Some(y) = scroll_to {
        area = area.vertical_scroll_offset(y);
    }
    // `show_rows` positions rows with the ui's item spacing.
    ui.spacing_mut().item_spacing.y = ROW_GAP;
    let out = area.show_rows(ui, row_h, flat.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        for i in range {
            let (cue, depth) = flat[i];
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
            if let Some(a) = draw_row(ui, rect, i, cue, depth, playhead == Some(cue.id), input) {
                actions.push(a);
            }
        }
    });
    win.offset = out.state.clamped_offset().y;
    win.view_h = out.inner_rect.height();
    actions
}

fn draw_row(
    ui: &mut egui::Ui,
    rect: Rect,
    index: usize,
    cue: &Cue,
    depth: u8,
    is_playhead: bool,
    input: &CueWindowInput<'_>,
) -> Option<CueAction> {
    let state = input.state;
    let id = ui.scope_id().with(("cue_row", cue.id));
    let resp = ui.interact(rect, id, Sense::click());
    let active = input.tick.active(cue.id);
    let error = state.error(cue.id);
    let loaded = state.loaded(cue.id);
    let st = row_state(active, error.is_some(), loaded.is_some());
    let selected = input.selection.contains(&cue.id);
    let hc = input.high_contrast;
    let v = ui.visuals();
    let text_color = if hc {
        Color32::WHITE
    } else {
        Color32::from_gray(235)
    };
    let muted = if hc {
        Color32::from_gray(200)
    } else {
        theme::MUTED
    };
    let dim = |c: Color32| if cue.armed { c } else { c.gamma_multiply(0.45) };

    let p = ui.painter_at(rect);
    // Background.
    let stripe = if index % 2 == 1 {
        v.faint_bg_color
    } else {
        v.extreme_bg_color
    };
    let bg = match state_colors(st) {
        Some((_, bg)) => bg,
        None if is_playhead => theme::NEXT_BG,
        None => stripe,
    };
    p.rect_filled(rect, 4.0, bg);
    if resp.hovered() {
        p.rect_filled(rect, 4.0, Color32::from_white_alpha(8));
    }
    if is_playhead {
        p.rect_stroke(
            rect,
            4.0,
            Stroke::new(if hc { 3.0 } else { 2.0 }, theme::NEXT),
            egui::StrokeKind::Inside,
        );
    } else if selected {
        p.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, Color32::from_gray(150)),
            egui::StrokeKind::Inside,
        );
    }
    // Cue colour swatch on the left edge.
    if let Some(c) = theme::cue_color(&cue.color) {
        p.rect_filled(
            Rect::from_min_size(
                rect.min + Vec2::new(2.0, 4.0),
                Vec2::new(5.0, rect.height() - 8.0),
            ),
            2.0,
            dim(c),
        );
    }

    let cy = rect.center().y;
    let mut x = rect.left() + 10.0;
    // Status / NEXT column.
    let status_w = 54.0;
    if is_playhead && st == RowState::Idle {
        let r = Rect::from_center_size(Pos2::new(x + status_w / 2.0, cy), Vec2::new(46.0, 22.0));
        p.rect_filled(r, 4.0, theme::NEXT);
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            "NEXT",
            FontId::proportional(13.0),
            Color32::BLACK,
        );
    } else {
        status_glyph(&p, Pos2::new(x + status_w / 2.0, cy), st, cue.armed, 18.0);
    }
    x += status_w + 6.0;

    // Number.
    let indent = f32::from(depth) * 22.0;
    let num_w = 70.0;
    if depth > 0 {
        // Tree guide for group children.
        let gx = x + indent - 12.0;
        p.line_segment(
            [Pos2::new(gx, rect.top() + 2.0), Pos2::new(gx, cy)],
            Stroke::new(1.0, muted.gamma_multiply(0.6)),
        );
        p.line_segment(
            [Pos2::new(gx, cy), Pos2::new(gx + 7.0, cy)],
            Stroke::new(1.0, muted.gamma_multiply(0.6)),
        );
    }
    let num = elided(
        ui,
        cue.number.clone(),
        FontId::proportional(22.0),
        dim(text_color),
        num_w,
    );
    p.galley(
        Pos2::new(x + indent, cy - num.size().y / 2.0),
        num,
        text_color,
    );
    x += num_w + indent;

    // Right side: time column, type, tags.
    let time_w = 112.0;
    let time_r = rect.right() - 12.0;
    let accent = state_colors(st).map(|(a, _)| a);
    match active {
        Some(a) => {
            let (big, caption, _) = active_timing(a);
            let color = accent.unwrap_or(text_color);
            p.text(
                Pos2::new(time_r, cy - 3.0),
                Align2::RIGHT_BOTTOM,
                big,
                theme::clock_font(20.0),
                color,
            );
            p.text(
                Pos2::new(time_r, cy + 1.0),
                Align2::RIGHT_TOP,
                caption,
                FontId::proportional(11.5),
                muted,
            );
        }
        None => {
            let text = match cue.action_duration() {
                _ if matches!(cue.kind, CueKind::Memo) => String::new(),
                Some(d) if d > 0.0 => util::fmt_time(d),
                Some(_) => String::new(),
                None if matches!(cue.kind, CueKind::Audio(_)) => "loop".into(),
                None => String::new(),
            };
            p.text(
                Pos2::new(time_r, cy),
                Align2::RIGHT_CENTER,
                text,
                theme::clock_font(17.0),
                dim(muted),
            );
        }
    }
    let type_w = 70.0;
    let type_r = time_r - time_w;
    p.text(
        Pos2::new(type_r, cy),
        Align2::RIGHT_CENTER,
        type_label(cue),
        FontId::proportional(13.0),
        dim(muted),
    );
    let mut tags_r = type_r - type_w - 6.0;
    let mut tag_at = |text: &str, fg: Color32, bg: Color32| {
        let w = tag(&p, tags_r, cy, text, fg, bg);
        tags_r -= w + 6.0;
    };
    match st {
        RowState::Paused => tag_at("PAUSED", Color32::BLACK, theme::PAUSED),
        RowState::PreWait => tag_at("PRE-WAIT", Color32::WHITE, theme::RUNNING),
        RowState::PostWait => tag_at("POST-WAIT", Color32::WHITE, theme::RUNNING),
        RowState::Error => tag_at("BROKEN", Color32::WHITE, theme::ERROR),
        RowState::Loaded => {
            let t = format!("LOADED {}", util::fmt_time(loaded.unwrap_or(0.0)));
            tag_at(&t, Color32::BLACK, theme::LOADED);
        }
        RowState::Running | RowState::Idle => {}
    }
    if !cue.armed {
        tag_at("DISARMED", Color32::BLACK, Color32::from_gray(150));
    }
    match cue.continue_mode {
        ContinueMode::AutoContinue => tag_at("AUTO-CONT", text_color, Color32::from_gray(60)),
        ContinueMode::AutoFollow => tag_at("AUTO-FOLLOW", text_color, Color32::from_gray(60)),
        ContinueMode::DoNotContinue => {}
    }
    if let Some(tc) = &cue.tc_trigger {
        let t = format!("TC {tc}");
        tag_at(&t, text_color, Color32::from_rgb(60, 50, 90));
    }

    // Name + sub line.
    let name_w = (tags_r - x - 8.0).max(20.0);
    let name_text = if cue.name.is_empty() {
        type_label(cue).to_string()
    } else {
        cue.name.clone()
    };
    let name_color = if cue.name.is_empty() {
        muted
    } else {
        text_color
    };
    let name = elided(
        ui,
        name_text,
        FontId::proportional(20.0),
        dim(name_color),
        name_w,
    );
    let sub: Option<(String, Color32)> = if let Some(e) = error {
        Some((e.to_string(), theme::ERROR))
    } else {
        let t = cue_target(cue, &state.show);
        if !t.is_empty() {
            Some((t, muted))
        } else if !cue.notes.is_empty() {
            Some((
                cue.notes.lines().next().unwrap_or_default().to_string(),
                muted,
            ))
        } else {
            None
        }
    };
    match sub {
        Some((text, color)) => {
            let nh = name.size().y;
            p.galley(Pos2::new(x, cy - nh + 3.0), name, text_color);
            let g = elided(ui, text, FontId::proportional(13.0), dim(color), name_w);
            p.galley(Pos2::new(x, cy + 4.0), g, color);
        }
        None => {
            let nh = name.size().y;
            p.galley(Pos2::new(x, cy - nh / 2.0), name, text_color);
        }
    }

    // Progress bar along the bottom edge.
    if let Some(a) = active {
        let bar = Rect::from_min_max(
            Pos2::new(rect.left() + 4.0, rect.bottom() - 5.0),
            Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0),
        );
        p.rect_filled(bar, 1.5, Color32::from_black_alpha(120));
        let color = accent.unwrap_or(theme::RUNNING);
        match a.progress() {
            Some(f) => {
                let mut fill = bar;
                fill.set_width(bar.width() * f);
                p.rect_filled(fill, 1.5, color);
            }
            None => {
                // Open-ended: a sliding segment.
                let t = (a.elapsed as f32 * 0.5).fract();
                let w = bar.width() * 0.2;
                let x0 = bar.left() + (bar.width() + w) * t - w;
                let seg = Rect::from_x_y_ranges(
                    x0.max(bar.left())..=(x0 + w).min(bar.right()),
                    bar.y_range(),
                );
                p.rect_filled(seg, 1.5, color);
            }
        }
    }

    let label = format!("Cue {}", cue_label(cue));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Row, true, &label));
    // No tooltip: a mouse resting on the list during a show must not cover rows.
    if resp.double_clicked() && depth == 0 {
        Some(CueAction::SetPlayhead(cue.id))
    } else if resp.clicked() {
        Some(CueAction::Select(cue.id))
    } else {
        None
    }
}
