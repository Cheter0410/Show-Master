//! Optional waveform strip in Live mode (Setup > General > This computer, or
//! the "Waveform" box above the cue list): the audio cue playing - the one
//! started last - with its loops and the playhead; when nothing plays, the
//! audio cue standing by, dimmed, from where GO starts it.

use super::{card, child};
use crate::app::App;
use crate::edit::audio::{show_dir, WaveView};
use crate::model::Phase;
use crate::{theme, util};
use egui::{pos2, vec2, Align2, Color32, FontId, Mesh, Rect, Shape, Stroke};
use sm_core::model::{AudioCue, CueKind};

/// What the strip shows.
struct Shown {
    number: String,
    name: String,
    audio: AudioCue,
    /// Playhead (file seconds); `None` = not known yet.
    pos: Option<f64>,
    playing: bool,
    paused: bool,
    /// Loop playing: name and pass.
    looping: Option<(String, u64)>,
    /// More audio cues playing at the same time.
    others: usize,
}

fn pick(app: &App) -> Option<Shown> {
    let s = &app.client.state;
    let tick = &app.client.tick;
    let audio_of = |cue: u64| {
        s.show.cue(cue).and_then(|c| match &c.kind {
            CueKind::Audio(a) if !a.file.is_empty() => Some((c, a)),
            _ => None,
        })
    };
    let running: Vec<_> = tick
        .active
        .iter()
        .filter(|a| a.phase == Phase::Running)
        .filter_map(|a| audio_of(a.cue).map(|(c, au)| (a, c, au)))
        .collect();
    if let Some((act, c, a)) = running.last() {
        let start = a.start.max(0.0);
        let end = a.end.or(a.file_duration).unwrap_or(0.0);
        let len = end - start;
        let pos = match act.file_pos {
            Some(p) => Some(p),
            None if a.loops.is_empty() && len > 0.0 => Some(start + act.elapsed % len),
            None => None,
        };
        return Some(Shown {
            number: c.number.clone(),
            name: c.name.clone(),
            audio: (*a).clone(),
            pos,
            playing: true,
            paused: act.paused,
            looping: act.looping.as_ref().map(|l| (l.name.clone(), l.pass)),
            others: running.len() - 1,
        });
    }
    let list = app.current_list()?;
    let next = s.playhead(list)?;
    let (c, a) = audio_of(next)?;
    Some(Shown {
        number: c.number.clone(),
        name: c.name.clone(),
        audio: a.clone(),
        pos: Some(a.start.max(0.0) + s.loaded(c.id).unwrap_or(0.0)),
        playing: false,
        paused: false,
        looping: None,
        others: 0,
    })
}

pub fn strip(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    card(ui, rect, hc);
    let inner = rect.shrink(8.0);
    let ui = &mut child(ui, inner, "live_wave");
    let painter = ui.painter_at(inner);
    let muted = if hc { Color32::from_gray(220) } else { theme::MUTED };
    // For screen readers (and tests): what the strip shows.
    let say = |ui: &mut egui::Ui, text: String| {
        let r = ui.interact(inner, ui.scope_id().with("live_wave_info"), egui::Sense::hover());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, format!("Waveform: {text}")));
    };
    let Some(sh) = pick(app) else {
        say(ui, "nothing".into());
        painter.text(
            inner.center(),
            Align2::CENTER_CENTER,
            "No audio playing or standing by",
            FontId::proportional(13.0),
            muted,
        );
        return;
    };
    let a = &sh.audio;

    // Head: what it is, where it is.
    let head_h = 18.0;
    let what = if sh.playing {
        if sh.paused {
            "Paused"
        } else {
            "Playing"
        }
    } else {
        "Standing by"
    };
    let mut title = format!("{what}  {}  {}", sh.number, sh.name);
    if sh.others > 0 {
        title.push_str(&format!("   (+{} more)", sh.others));
    }
    say(ui, title.clone());
    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(13.0),
        if sh.playing { Color32::WHITE } else { muted },
    );
    let start = a.start.max(0.0);
    let end = a.end.or(a.file_duration).unwrap_or(0.0).max(start);
    let mut right = String::new();
    if let Some((name, pass)) = &sh.looping {
        right.push_str(&format!("Loop {name} · pass {pass}    "));
    }
    if let Some(p) = sh.pos {
        right.push_str(&format!(
            "{} / {}",
            util::fmt_time(p - start),
            util::fmt_time(end - start)
        ));
    }
    painter.text(
        inner.right_top(),
        Align2::RIGHT_TOP,
        right,
        FontId::monospace(12.5),
        muted,
    );

    // The waveform of the cue's region.
    let wave = Rect::from_min_max(pos2(inner.left(), inner.top() + head_h + 4.0), inner.max);
    painter.rect_filled(wave, 4.0, ui.visuals().extreme_bg_color);
    let span = end - start;
    if span <= 0.0 {
        return;
    }
    let x_of = |t: f64| wave.left() + ((t - start) / span).clamp(0.0, 1.0) as f32 * wave.width();
    // Loops: a light band with the name.
    for (i, l) in a.loops.iter().enumerate() {
        if l.end <= start || l.start >= end {
            continue;
        }
        let band = Rect::from_min_max(pos2(x_of(l.start), wave.top()), pos2(x_of(l.end), wave.bottom()));
        let live = sh.looping.as_ref().is_some_and(|(n, _)| *n == l.label(i));
        painter.rect_filled(band, 0.0, theme::LOADED.gamma_multiply(if live { 0.16 } else { 0.08 }));
        painter.vline(band.left(), wave.y_range(), Stroke::new(1.0, theme::LOADED.gamma_multiply(0.6)));
        painter.vline(band.right(), wave.y_range(), Stroke::new(1.0, theme::LOADED.gamma_multiply(0.6)));
        painter.text(
            band.left_bottom() + vec2(3.0, -2.0),
            Align2::LEFT_BOTTOM,
            l.label(i),
            FontId::proportional(10.0),
            theme::LOADED.gamma_multiply(0.8),
        );
    }
    let path = sm_core::showfile::resolve_media(show_dir(&app.client.state).as_deref(), &a.file);
    let duration = a.file_duration.unwrap_or(end);
    match app.edit.waves.get(ui.ctx(), &path) {
        WaveView::Ready(peaks) if !peaks.is_empty() && duration > 0.0 => {
            let n = peaks.len();
            let cols = wave.width().floor().max(1.0) as usize;
            let mid = wave.center().y;
            let half = wave.height() * 0.44;
            let bucket = |t: f64| ((t / duration) * n as f64).max(0.0) as usize;
            let played = sh.pos.filter(|_| sh.playing).map(x_of);
            let (on, off) = if sh.playing {
                (theme::RUNNING, Color32::from_gray(if hc { 150 } else { 105 }))
            } else {
                (theme::RUNNING.gamma_multiply(0.55), Color32::from_gray(if hc { 130 } else { 80 }))
            };
            let mut mesh = Mesh::default();
            for px in 0..cols {
                let ta = start + span * px as f64 / cols as f64;
                let tb = start + span * (px + 1) as f64 / cols as f64;
                let b0 = bucket(ta).min(n - 1);
                let b1 = bucket(tb).max(b0 + 1).min(n);
                let (mut lo, mut hi) = (0.0f32, 0.0f32);
                for (l, h) in &peaks[b0..b1] {
                    lo = lo.min(*l);
                    hi = hi.max(*h);
                }
                let x = wave.left() + px as f32;
                let top = mid - hi.clamp(-1.0, 1.0) * half;
                let bottom = (mid - lo.clamp(-1.0, 1.0) * half).max(top + 1.0);
                let color = match played {
                    Some(p) if x <= p => on,
                    Some(_) => off,
                    None => on,
                };
                mesh.add_colored_rect(Rect::from_min_max(pos2(x, top), pos2(x + 1.0, bottom)), color);
            }
            painter.add(Shape::mesh(mesh));
        }
        WaveView::Loading => {
            painter.text(wave.center(), Align2::CENTER_CENTER, "loading waveform…", FontId::proportional(12.0), muted);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        WaveView::Failed(e) => {
            painter.text(
                wave.center(),
                Align2::CENTER_CENTER,
                format!("Waveform not available: {e}"),
                FontId::proportional(12.0),
                muted,
            );
        }
        WaveView::Ready(_) => {}
    }
    // Playhead.
    if let Some(p) = sh.pos {
        let x = x_of(p);
        let color = if sh.playing { Color32::WHITE } else { theme::LOADED };
        painter.vline(x, wave.y_range(), Stroke::new(1.5, color));
        if !sh.playing {
            painter.add(Shape::convex_polygon(
                vec![pos2(x - 5.0, wave.top()), pos2(x + 5.0, wave.top()), pos2(x, wave.top() + 7.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
    if sh.playing && !sh.paused {
        ui.ctx().request_repaint();
    }
}
