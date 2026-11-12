//! Clock widgets: one "clock cell" (caption + large tabular digits) and the
//! live dashboard's clock strip (wall clock, show time, timecode, next auto
//! cue). Public so other views can reuse them.

use crate::model::{TcInfo, Tick};
use crate::theme;
use crate::util;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Vec2};

/// What a clock cell shows.
pub struct ClockCell<'a> {
    pub caption: &'a str,
    pub value: &'a str,
    /// Colour of the digits.
    pub color: Color32,
    /// Accent (left bar + caption colour) when the clock is "active".
    pub accent: Option<Color32>,
    /// Digit size; `None` = as large as fits.
    pub size: Option<f32>,
}

/// Font size that fits `chars` monospace characters into `width` and `height`.
pub fn fit_digits(chars: usize, width: f32, height: f32, max: f32) -> f32 {
    // Monospace glyphs are about 0.6 em wide.
    let by_width = width / (chars.max(1) as f32 * 0.62);
    by_width.min(height).clamp(10.0, max)
}

/// Paint a clock cell into `rect`; returns the rect of the caption row's
/// right part (free space for small badges / buttons).
pub fn clock_cell(ui: &mut egui::Ui, rect: Rect, cell: &ClockCell<'_>) -> Rect {
    let painter = ui.painter_at(rect);
    let v = ui.visuals();
    let hc = v.override_text_color.is_some();
    painter.rect(
        rect,
        6.0,
        if hc {
            Color32::BLACK
        } else {
            v.extreme_bg_color
        },
        Stroke::new(
            if hc { 1.5 } else { 1.0 },
            if hc {
                Color32::from_gray(200)
            } else {
                Color32::from_gray(48)
            },
        ),
        egui::StrokeKind::Inside,
    );
    if let Some(accent) = cell.accent {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(4.0, rect.height())),
            egui::CornerRadius {
                nw: 6,
                sw: 6,
                ne: 0,
                se: 0,
            },
            accent,
        );
    }
    let pad = 10.0;
    let caption_h = 16.0;
    let caption_color = cell.accent.unwrap_or(if hc {
        Color32::from_gray(220)
    } else {
        theme::MUTED
    });
    let cap = painter.text(
        rect.min + Vec2::new(pad, 6.0),
        Align2::LEFT_TOP,
        cell.caption.to_uppercase(),
        FontId::proportional(12.0),
        caption_color,
    );
    let digits_rect = Rect::from_min_max(
        egui::pos2(rect.left() + pad, rect.top() + 6.0 + caption_h),
        egui::pos2(rect.right() - pad, rect.bottom() - 4.0),
    );
    let size = cell.size.unwrap_or_else(|| {
        fit_digits(
            cell.value.chars().count(),
            digits_rect.width(),
            digits_rect.height() * 0.95,
            64.0,
        )
    });
    painter.text(
        digits_rect.left_center(),
        Align2::LEFT_CENTER,
        cell.value,
        theme::clock_font(size),
        cell.color,
    );
    Rect::from_min_max(
        egui::pos2(cap.right() + 8.0, rect.top() + 4.0),
        egui::pos2(rect.right() - 6.0, rect.top() + 4.0 + caption_h),
    )
}

/// A small badge (e.g. `LTC`) painted right-to-left from `right`; returns the new right edge.
pub fn badge(
    ui: &egui::Ui,
    right: f32,
    center_y: f32,
    text: &str,
    on: bool,
    color: Color32,
) -> f32 {
    let painter = ui.painter();
    let font = FontId::proportional(10.5);
    let fg = if on { Color32::BLACK } else { theme::MUTED };
    let galley = painter.layout_no_wrap(text.to_string(), font, fg);
    let size = galley.size() + Vec2::new(8.0, 2.0);
    let r = Rect::from_min_size(egui::pos2(right - size.x, center_y - size.y / 2.0), size);
    if on {
        painter.rect_filled(r, 3.0, color);
    } else {
        painter.rect_stroke(
            r,
            3.0,
            Stroke::new(1.0, Color32::from_gray(70)),
            egui::StrokeKind::Inside,
        );
    }
    painter.galley(r.min + Vec2::new(4.0, 1.0), galley, fg);
    r.left() - 4.0
}

/// Text of the timecode clock.
pub fn tc_text(tc: &TcInfo) -> &str {
    if tc.position.is_empty() {
        "--:--:--:--"
    } else {
        &tc.position
    }
}

/// Badge for an external timecode source: text, colour, filled.
/// `None` while the internal generator is the source.
pub fn tc_source_badge(tc: &TcInfo) -> Option<(String, Color32, bool)> {
    let src = match tc.source.as_str() {
        "mtc" => "MTC",
        "ltc" => "LTC",
        _ => return None,
    };
    let (state, color, on) = match tc.status.as_str() {
        "locked" => ("LOCKED", theme::NEXT, true),
        "locking" => ("LOCKING", theme::WARN, true),
        "freewheel" => ("FREEWHEEL", theme::PAUSED, true),
        "lost" => ("LOST", theme::ERROR, true),
        "no_signal" => ("NO SIGNAL", theme::ERROR, true),
        "stopped" => ("STOPPED", theme::MUTED, false),
        "held" => ("HELD", theme::WARN, false),
        "reverse" => ("REVERSE", theme::WARN, false),
        other => (other, theme::MUTED, false),
    };
    Some((format!("{src} {state}"), color, on))
}

/// What the user asked for in the clock strip.
#[derive(Default)]
pub struct StripAction {
    pub reset_show_clock: bool,
    /// A timecode generator command (`tc_pause`, `tc_resume`, `tc_start`, `tc_stop`).
    pub timecode: Option<&'static str>,
}

/// The four live clocks side by side in `rect`.
pub fn clock_strip(ui: &mut egui::Ui, rect: Rect, tick: &Tick) -> StripAction {
    let gap = 8.0;
    // Timecode needs the most room, the wall clock the least.
    let weights = [1.0, 1.0, 1.4, 0.85];
    let total: f32 = weights.iter().sum();
    let avail = rect.width() - gap * 3.0;
    let mut x = rect.left();
    let mut cells = [Rect::NOTHING; 4];
    for (i, w) in weights.iter().enumerate() {
        let width = avail * w / total;
        cells[i] = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(width, rect.height()));
        x += width + gap;
    }
    // One digit size for all four clocks (calmer than four different sizes).
    let digits_h = (rect.height() - 26.0) * 0.95;
    let fit = |i: usize, chars: usize| fit_digits(chars, cells[i].width() - 20.0, digits_h, 64.0);
    let size = Some(fit(0, 8).min(fit(1, 8)).min(fit(2, 11)).min(fit(3, 6)));
    let text = ui.visuals().strong_text_color();
    let dim = theme::MUTED;

    // Wall clock.
    let wall = util::wall_clock();
    clock_cell(
        ui,
        cells[0],
        &ClockCell {
            caption: "Time of day",
            value: &wall,
            color: text,
            accent: None,
            size,
        },
    );

    // Show running time.
    let show = tick.show_elapsed.map(util::fmt_clock);
    let free = clock_cell(
        ui,
        cells[1],
        &ClockCell {
            caption: "Show time",
            value: show.as_deref().unwrap_or("--:--"),
            color: if show.is_some() { text } else { dim },
            accent: None,
            size,
        },
    );
    let mut reset = false;
    let btn = Rect::from_min_size(
        egui::pos2(free.right() - 44.0, free.top() - 1.0),
        Vec2::new(44.0, 18.0),
    );
    if free.width() >= 46.0 {
        let resp = ui.interact(btn, ui.scope_id().with("reset_show_clock"), Sense::click());
        let hovered = resp.hovered();
        let p = ui.painter();
        p.rect(
            btn,
            3.0,
            if hovered {
                Color32::from_gray(60)
            } else {
                Color32::TRANSPARENT
            },
            Stroke::new(1.0, Color32::from_gray(80)),
            egui::StrokeKind::Inside,
        );
        p.text(
            btn.center(),
            Align2::CENTER_CENTER,
            "Reset",
            FontId::proportional(11.0),
            if hovered { text } else { theme::MUTED },
        );
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, "Reset show clock")
        });
        reset = resp.on_hover_text("Reset the show clock to zero").clicked();
    }

    // Timecode.
    let tc = &tick.tc;
    let external = tc_source_badge(tc);
    let free = clock_cell(
        ui,
        cells[2],
        &ClockCell {
            caption: if external.is_some() {
                "Timecode ext"
            } else {
                "Timecode"
            },
            value: tc_text(tc),
            color: if tc.running { text } else { dim },
            accent: tc.running.then_some(theme::RUNNING),
            size,
        },
    );
    let cy = free.center().y;
    let mut right = free.right();
    if free.width() > 120.0 {
        right = badge(ui, right, cy, "MTC", tc.mtc, theme::LOADED);
        right = badge(ui, right, cy, "LTC", tc.ltc, theme::LOADED);
    }
    if !tc.rate.is_empty() && free.width() > 60.0 {
        right = badge(ui, right, cy, &tc.rate, false, theme::MUTED);
    }
    if let Some((text, color, on)) = &external {
        // Only when it fits next to the caption (about 6.5 px per character).
        if right - (text.len() as f32 * 6.5 + 12.0) > free.left() {
            right = badge(ui, right, cy, text, *on, *color);
        }
    }
    let _ = badge(
        ui,
        right,
        cy,
        if tc.running { "RUN" } else { "STOP" },
        tc.running,
        theme::RUNNING,
    );
    // Generator controls: right-click / long-press only, so a stray tap during
    // the show never stops the timecode.
    let mut timecode = None;
    let resp = ui
        .interact(cells[2], ui.scope_id().with("tc_menu"), Sense::click())
        .on_hover_text("Right-click or long-press for timecode controls");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Timecode controls"));
    resp.context_menu(|ui| {
        if external.is_some() {
            // Chasing: the source runs the clock; only following can be suspended.
            if tc.status == "held" {
                if ui.button("Follow external timecode again").clicked() {
                    timecode = Some("tc_resume");
                }
            } else if ui
                .button("Stop following (hold)")
                .on_hover_text("Timecode triggers stop until you follow again")
                .clicked()
            {
                timecode = Some("tc_pause");
            }
            return;
        }
        if tc.running {
            if ui.button("Pause timecode").clicked() {
                timecode = Some("tc_pause");
            }
        } else if ui
            .button("Resume timecode")
            .on_hover_text("Continue from the shown position")
            .clicked()
        {
            timecode = Some("tc_resume");
        }
        if ui.button("Start from the beginning").clicked() {
            timecode = Some("tc_start");
        }
        if ui
            .button("Stop timecode")
            .on_hover_text("Stop and go back to the start position")
            .clicked()
        {
            timecode = Some("tc_stop");
        }
    });

    // Countdown to the next automatic cue.
    let auto = tick.next_auto.map(util::fmt_time);
    clock_cell(
        ui,
        cells[3],
        &ClockCell {
            caption: "Next auto cue",
            value: auto.as_deref().unwrap_or("--:--"),
            color: if auto.is_some() { theme::NEXT } else { dim },
            accent: auto.is_some().then_some(theme::NEXT),
            size,
        },
    );
    StripAction {
        reset_show_clock: reset,
        timecode,
    }
}
