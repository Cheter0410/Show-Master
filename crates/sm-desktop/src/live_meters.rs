//! Output meters (peak + RMS on a dB scale with client-side decay, peak hold
//! and clip LEDs) and the master fader. Public so other views can reuse them.

use crate::model::Meters;
use crate::theme;
use crate::util;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::time::{Duration, Instant};

/// Bottom of the meter scale.
pub const DB_MIN: f32 = -60.0;
/// Top of the meter scale.
pub const DB_MAX: f32 = 0.0;
/// Scale ticks.
pub const TICKS: [f32; 5] = [0.0, -6.0, -12.0, -24.0, -48.0];
/// Fall-back speed of the bars.
pub const DECAY_DB_PER_S: f32 = 26.0;
/// How long the peak-hold line stays before it falls.
pub const HOLD: Duration = Duration::from_millis(1200);

/// Master fader range (dB); the bottom position means "-inf".
pub const FADER_MIN: f32 = -60.0;
pub const FADER_MAX: f32 = 12.0;
/// Sent to the engine for the fader's bottom position.
pub const SILENT_DB: f32 = -120.0;
/// Minimum time between two `set_master` commands (20 per second).
pub const MASTER_SEND_INTERVAL: Duration = Duration::from_millis(50);

/// Display level of one channel.
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    pub peak: f32,
    pub rms: f32,
    pub hold: f32,
    pub hold_at: Instant,
}

/// Meter ballistics (decay + hold), kept between frames.
#[derive(Debug, Default)]
pub struct Ballistics {
    pub channels: Vec<Channel>,
    last: Option<Instant>,
}

impl Ballistics {
    /// Feed the latest engine levels (linear) and advance the decay.
    pub fn update(&mut self, meters: &Meters, channels: usize) {
        let now = Instant::now();
        let dt = self
            .last
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.25));
        self.last = Some(now);
        if self.channels.len() != channels {
            self.channels.resize(
                channels,
                Channel {
                    peak: DB_MIN,
                    rms: DB_MIN,
                    hold: DB_MIN,
                    hold_at: now,
                },
            );
        }
        let fall = DECAY_DB_PER_S * dt;
        for (i, c) in self.channels.iter_mut().enumerate() {
            let peak_in = util::lin_to_db(meters.peak.get(i).copied().unwrap_or(0.0));
            let rms_in = util::lin_to_db(meters.rms.get(i).copied().unwrap_or(0.0));
            c.peak = peak_in.max(c.peak - fall).max(DB_MIN);
            c.rms = rms_in.max(c.rms - fall).max(DB_MIN);
            if c.peak >= c.hold {
                c.hold = c.peak;
                c.hold_at = now;
            } else if now.duration_since(c.hold_at) > HOLD {
                c.hold = (c.hold - fall).max(c.peak);
            }
        }
    }
}

/// 0..1 position of a level on the meter scale.
pub fn frac(db: f32) -> f32 {
    ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0)
}

fn level_color(db: f32) -> Color32 {
    if db > -3.0 {
        theme::ERROR
    } else if db > -12.0 {
        theme::PAUSED
    } else {
        theme::NEXT
    }
}

/// Paint one bar (segments green / amber / red) from the bottom of `r` up to `level`.
fn bar(painter: &egui::Painter, r: Rect, level: f32, alpha: f32) {
    let y = |db: f32| r.bottom() - frac(db) * r.height();
    for (from, to) in [(DB_MIN, -12.0), (-12.0, -3.0), (-3.0, DB_MAX)] {
        let top = level.min(to);
        if top <= from {
            break;
        }
        let seg = Rect::from_x_y_ranges(r.x_range(), y(top)..=y(from));
        painter.rect_filled(seg, 0.0, level_color(to - 0.1).gamma_multiply(alpha));
    }
}

/// What the user did on the meters.
#[derive(Default)]
pub struct MeterResponse {
    /// A clip LED was clicked.
    pub reset_clip: bool,
}

/// Per-channel meters in `rect`. `names[i]` labels channel `i` (may be empty).
pub fn meter_bridge(
    ui: &mut egui::Ui,
    rect: Rect,
    ballistics: &Ballistics,
    clip: &[bool],
    name_of: &dyn Fn(usize) -> Option<String>,
) -> MeterResponse {
    let mut out = MeterResponse::default();
    let n = ballistics.channels.len();
    let painter = ui.painter_at(rect);
    if n == 0 {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No outputs",
            FontId::proportional(13.0),
            theme::MUTED,
        );
        return out;
    }
    let scale_w = 22.0;
    let led_h = 12.0;
    let label_h = if rect.height() > 120.0 { 30.0 } else { 16.0 };
    let bars = Rect::from_min_max(
        Pos2::new(rect.left() + scale_w, rect.top() + led_h + 4.0),
        Pos2::new(rect.right(), rect.bottom() - label_h),
    );
    let y = |db: f32| bars.bottom() - frac(db) * bars.height();
    // Scale.
    for db in TICKS {
        let yy = y(db);
        painter.text(
            Pos2::new(rect.left() + scale_w - 4.0, yy),
            Align2::RIGHT_CENTER,
            format!("{}", db.abs() as i32),
            FontId::monospace(10.0),
            theme::MUTED,
        );
    }
    let gap = 4.0;
    let col_w = ((bars.width() - gap * (n as f32 - 1.0)) / n as f32).clamp(6.0, 34.0);
    let bg = ui.visuals().extreme_bg_color;
    let hc = ui.visuals().override_text_color.is_some();
    for (i, c) in ballistics.channels.iter().enumerate() {
        let x = bars.left() + i as f32 * (col_w + gap);
        if x + col_w > rect.right() + 0.5 {
            break;
        }
        let col = Rect::from_min_size(Pos2::new(x, bars.top()), Vec2::new(col_w, bars.height()));
        painter.rect_filled(col, 2.0, bg);
        // Peak (dim, full width), RMS (bright, narrower).
        bar(&painter, col, c.peak, 0.45);
        let inner = col.shrink2(Vec2::new(col_w * 0.2, 0.0));
        bar(&painter, inner, c.rms, 1.0);
        for db in TICKS {
            let yy = y(db).round();
            painter.hline(
                col.x_range(),
                yy,
                Stroke::new(1.0, Color32::from_black_alpha(if hc { 220 } else { 150 })),
            );
        }
        if c.hold > DB_MIN + 0.5 {
            let yy = y(c.hold);
            painter.rect_filled(
                Rect::from_x_y_ranges(col.x_range(), (yy - 1.0)..=(yy + 1.0)),
                0.0,
                if c.hold > -3.0 {
                    theme::ERROR
                } else {
                    Color32::from_gray(230)
                },
            );
        }
        if hc {
            painter.rect_stroke(
                col,
                2.0,
                Stroke::new(1.0, Color32::from_gray(160)),
                egui::StrokeKind::Outside,
            );
        }
        // Clip LED.
        let clipped = clip.get(i).copied().unwrap_or(false);
        let led = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(col_w, led_h));
        let resp = ui.interact(led, ui.scope_id().with(("clip_led", i)), Sense::click());
        painter.rect(
            led,
            2.0,
            if clipped {
                theme::ERROR
            } else {
                Color32::from_rgb(60, 20, 20)
            },
            Stroke::new(1.0, Color32::from_gray(if resp.hovered() { 160 } else { 60 })),
            egui::StrokeKind::Inside,
        );
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Clip {}", i + 1))
        });
        if resp
            .on_hover_text(if clipped {
                "Clipped - click to reset the clip indicators"
            } else {
                "Clip indicator (click to reset)"
            })
            .clicked()
        {
            out.reset_clip = true;
        }
        // Channel number and name.
        let label_top = bars.bottom() + 2.0;
        painter.text(
            Pos2::new(col.center().x, label_top),
            Align2::CENTER_TOP,
            format!("{}", i + 1),
            FontId::monospace(11.0),
            ui.visuals().text_color(),
        );
        if label_h > 20.0 {
            if let Some(name) = name_of(i) {
                let g = painter.layout(
                    name,
                    FontId::proportional(10.0),
                    theme::MUTED,
                    col_w + gap,
                );
                let g_w = g.size().x;
                painter.galley(
                    Pos2::new(col.center().x - g_w / 2.0, label_top + 13.0),
                    g,
                    theme::MUTED,
                );
            }
        }
    }
    out
}

/// Width the meter bridge needs for `channels` channels.
pub fn meter_width(channels: usize) -> f32 {
    22.0 + channels.max(1) as f32 * 22.0
}

/// Local state of a master fader (between the user's drag and the engine).
#[derive(Debug, Default)]
pub struct Fader {
    /// Value the user set that the engine has not confirmed yet.
    local: Option<f32>,
    /// Value waiting to be sent (throttled).
    pending: Option<f32>,
    last_sent: Option<Instant>,
    touched_at: Option<Instant>,
}

impl Fader {
    /// The value to send now (throttled to [`MASTER_SEND_INTERVAL`]), if any.
    pub fn due(&mut self) -> Option<f32> {
        let v = self.pending?;
        if self
            .last_sent
            .is_some_and(|t| t.elapsed() < MASTER_SEND_INTERVAL)
        {
            return None;
        }
        self.pending = None;
        self.last_sent = Some(Instant::now());
        Some(v)
    }

    fn set(&mut self, db: f32) {
        self.local = Some(db);
        self.pending = Some(db);
        self.touched_at = Some(Instant::now());
    }
}

/// Fader position (FADER_MIN..FADER_MAX) for a level in dB.
pub fn fader_pos(db: f32) -> f32 {
    db.clamp(FADER_MIN, FADER_MAX)
}

/// Level in dB for a fader position (the bottom means silence).
pub fn fader_db(pos: f32) -> f32 {
    if pos <= FADER_MIN + 0.05 {
        SILENT_DB
    } else {
        pos
    }
}

/// `-inf` / `+3.5 dB` style text.
pub fn fader_text(db: f32) -> String {
    if db <= FADER_MIN + 0.05 {
        "-inf".into()
    } else {
        format!("{db:+.1} dB")
    }
}

/// A vertical master fader in `rect`. `engine_db` is the engine's value.
/// Changes are throttled; call [`Fader::due`] every frame and send what it returns.
pub fn master_fader(ui: &mut egui::Ui, rect: Rect, fader: &mut Fader, engine_db: f32) {
    // Drop the local override once the engine caught up (or after a while).
    if fader.pending.is_none()
        && fader.local.is_some()
        && fader
            .touched_at
            .is_none_or(|t| t.elapsed() > Duration::from_millis(600))
    {
        fader.local = None;
    }
    let shown = fader.local.unwrap_or(engine_db);
    let mut pos = fader_pos(shown);
    let painter = ui.painter_at(rect);
    painter.text(
        Pos2::new(rect.center().x, rect.top()),
        Align2::CENTER_TOP,
        "MASTER",
        FontId::proportional(11.0),
        theme::MUTED,
    );
    let hot = shown > 0.05;
    painter.text(
        Pos2::new(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        fader_text(shown),
        FontId::monospace(12.0),
        if hot {
            theme::PAUSED
        } else {
            ui.visuals().strong_text_color()
        },
    );
    let track = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + 18.0),
        Pos2::new(rect.right(), rect.bottom() - 20.0),
    );
    // 0 dB mark.
    let zero_y = track.bottom() - (0.0 - FADER_MIN) / (FADER_MAX - FADER_MIN) * track.height();
    painter.hline(
        track.x_range(),
        zero_y,
        Stroke::new(1.0, theme::MUTED.gamma_multiply(0.6)),
    );
    let resp = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(track)
                .layout(egui::Layout::top_down(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().slider_width = track.height();
                ui.spacing_mut().slider_rail_height = 6.0;
                ui.spacing_mut().interact_size.y = 24.0;
                ui.add(
                    egui::Slider::new(&mut pos, FADER_MIN..=FADER_MAX)
                        .vertical()
                        .show_value(false)
                        .step_by(0.5)
                        .trailing_fill(true),
                )
            },
        )
        .inner;
    ui.ctx().accesskit_node_builder(resp.id, |b| b.set_label("Master level"));
    let resp = resp.on_hover_text("Master level. Double-click: 0 dB");
    if resp.double_clicked() {
        fader.set(0.0);
    } else if resp.changed() {
        fader.set(fader_db(pos));
    }
}
