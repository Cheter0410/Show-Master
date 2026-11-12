//! Live mode dashboard: everything an operator needs during a show, readable
//! at a glance. No structural editing is reachable from here.
//!
//! ```text
//! +------------------------------------------------+--------------------+
//! | ALARM banner (only when a show timer alarm is due)                  |
//! +------------------------------------------------+--------------------+
//! | Time of day | Show time | Timecode | Next auto |  NEXT              |
//! +------------------------------------------------+  number + name     |
//! | Cue window (list tabs, follow toggle)          |  notes, then...    |
//! |   rows: status | number | name / target | tags +--------------------+
//! |         | type | time + progress               |  GO                |
//! |                                                |  Pause | Stop all  |
//! +------------------------------------------------+  Prev | Next | Rst |
//! | Meters + master | Show notes | Event log       |  ---- PANIC ----   |
//! +------------------------------------------------+--------------------+
//! ```
//!
//! Narrow windows (< 900 px, see `responsive.rs`) stack the zones instead,
//! so GO and PANIC stay large and reachable without scrolling:
//!
//! ```text
//! +-----------------------------------------------------+
//! | Time of day | Show time | Timecode | Next auto      |  (compact row)
//! +--------------------------------------+--------------+
//! | NEXT  number + name                  |      GO      |
//! +--------------------------------------+--------------+
//! | Pause | Stop all | Prev | Next | Reset |   PANIC    |
//! +-----------------------------------------------------+
//! | Cue window                                          |
//! +-----------------------------------------------------+
//! | [Meters] [Notes] [Log]  (tap to open / close)  ▮▮   |
//! +-----------------------------------------------------+
//! ```

#[path = "live_clocks.rs"]
pub mod clocks;
#[path = "live_cues.rs"]
pub mod cues;
#[path = "live_meters.rs"]
pub mod meters;

use crate::app::App;
use crate::theme;
use crate::widgets;
use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use serde_json::json;
use std::time::{Duration, Instant};

/// Notes are sent this long after the last keystroke (or on lost focus).
const NOTES_IDLE: Duration = Duration::from_millis(800);

pub struct LiveState {
    /// Keep the NEXT row in view.
    pub follow: bool,
    pub cues: cues::CueWindowState,
    pub ballistics: meters::Ballistics,
    pub fader: meters::Fader,
    /// Text of the notes editor.
    notes: String,
    /// Last local edit not sent yet.
    notes_dirty_at: Option<Instant>,
    notes_sent_at: Option<Instant>,
    notes_focused: bool,
    confirm_reset: bool,
    /// Narrow layout: the open tools pane (`None` = folded).
    pub tools_tab: Option<ToolsTab>,
}

/// Panes of the tools strip in the narrow layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolsTab {
    Meters,
    Notes,
    Log,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            follow: true,
            cues: Default::default(),
            ballistics: Default::default(),
            fader: Default::default(),
            notes: String::new(),
            notes_dirty_at: None,
            notes_sent_at: None,
            notes_focused: false,
            confirm_reset: false,
            tools_tab: None,
        }
    }
}

const GAP: f32 = 8.0;

fn high_contrast(app: &App) -> bool {
    app.client.state.show.settings.high_contrast
}

/// Background card of a zone.
fn card(ui: &egui::Ui, rect: Rect, hc: bool) {
    let v = ui.visuals();
    ui.painter().rect(
        rect,
        8.0,
        if hc { Color32::BLACK } else { v.faint_bg_color },
        if hc {
            Stroke::new(1.5, Color32::from_gray(200))
        } else {
            Stroke::new(1.0, Color32::from_gray(46))
        },
        egui::StrokeKind::Inside,
    );
}

/// A child ui confined to `rect`.
fn child(ui: &mut egui::Ui, rect: Rect, salt: &str) -> egui::Ui {
    let mut c = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(salt)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    c.set_clip_rect(rect.intersect(ui.clip_rect()));
    c
}

/// Small upper-case caption of a zone.
fn caption(ui: &mut egui::Ui, text: &str, hc: bool) {
    ui.label(RichText::new(text).size(12.0).color(if hc {
        Color32::from_gray(220)
    } else {
        theme::MUTED
    }));
}

/// Fill the central area in Live mode.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    // Throttled master fader changes.
    if let Some(db) = app.live.fader.due() {
        app.cmd(json!({ "cmd": "set_master", "db": db }));
    }
    let channels = app.client.state.audio.channels;
    app.live
        .ballistics
        .update(&app.client.tick.meters, channels);

    let hc = high_contrast(app);
    let full = ui.available_rect_before_wrap();
    let mut r = full;

    if !app.client.tick.alarms.is_empty() {
        let banner = Rect::from_min_size(r.min, Vec2::new(r.width(), 52.0));
        alarm_banner(app, ui, banner);
        r.min.y = banner.bottom() + GAP;
    }

    if app.screen.narrow() {
        stacked(app, ui, r, hc);
        reset_confirm(app, ui);
        ui.advance_cursor_after_rect(full);
        return;
    }

    let side_w = (r.width() * 0.3).clamp(300.0, 440.0);
    let side = Rect::from_min_max(Pos2::new(r.right() - side_w, r.top()), r.max);
    let left = Rect::from_min_max(r.min, Pos2::new(side.left() - GAP, r.bottom()));

    let clock_h = if r.height() < 560.0 {
        64.0
    } else {
        (r.height() * 0.11).clamp(72.0, 100.0)
    };
    let tools_h = (r.height() * 0.3).clamp(160.0, 280.0);
    let clocks = Rect::from_min_size(left.min, Vec2::new(left.width(), clock_h));
    let tools = Rect::from_min_max(Pos2::new(left.left(), left.bottom() - tools_h), left.max);
    let cue_rect = Rect::from_min_max(
        Pos2::new(left.left(), clocks.bottom() + GAP),
        Pos2::new(left.right(), tools.top() - GAP),
    );

    let clock_action = clocks::clock_strip(ui, clocks, &app.client.tick);
    if clock_action.reset_show_clock {
        app.cmd(json!({ "cmd": "reset_show_clock" }));
    }
    if let Some(cmd) = clock_action.timecode {
        app.cmd(json!({ "cmd": cmd }));
    }
    cue_zone(app, ui, cue_rect, hc);
    tools_zone(app, ui, tools, hc);
    side_zone(app, ui, side, hc);
    reset_confirm(app, ui);

    ui.advance_cursor_after_rect(full);
}

// ---------------------------------------------------------------------------
// Alarm banner
// ---------------------------------------------------------------------------

fn alarm_banner(app: &App, ui: &mut egui::Ui, rect: Rect) {
    let t = ui.input(|i| i.time);
    // Slow pulse: noticeable without being frantic.
    let pulse = 0.75 + 0.25 * ((t * std::f64::consts::TAU * 0.8).sin() as f32 * 0.5 + 0.5);
    let fill = theme::PAUSED.gamma_multiply(pulse);
    let p = ui.painter();
    p.rect_filled(rect, 8.0, fill);
    p.rect_stroke(
        rect,
        8.0,
        Stroke::new(3.0, Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    let tag = Rect::from_min_size(
        rect.min + Vec2::new(10.0, 10.0),
        Vec2::new(90.0, rect.height() - 20.0),
    );
    p.rect_filled(tag, 4.0, Color32::BLACK);
    p.text(
        tag.center(),
        Align2::CENTER_CENTER,
        "ALARM",
        FontId::proportional(18.0),
        theme::PAUSED,
    );
    let text = app.client.tick.alarms.join("   ·   ");
    let g = cues::elided(
        ui,
        text.clone(),
        FontId::proportional(26.0),
        Color32::BLACK,
        rect.width() - 130.0,
    );
    ui.painter().galley(
        Pos2::new(tag.right() + 16.0, rect.center().y - g.size().y / 2.0),
        g,
        Color32::BLACK,
    );
    let resp = ui.interact(
        rect,
        ui.scope_id().with("alarm_banner"),
        egui::Sense::hover(),
    );
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Alert, true, format!("Alarm: {text}"))
    });
    ui.ctx().request_repaint();
}

// ---------------------------------------------------------------------------
// Cue window
// ---------------------------------------------------------------------------

fn cue_zone(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    card(ui, rect, hc);
    let mut ui = child(ui, rect.shrink(6.0), "live_cue_zone");
    let list = app.current_list();
    // Header: list tabs, state, follow toggle.
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        let lists = &app.client.state.show.cue_lists;
        let mut chosen = None;
        if lists.len() > 1 {
            for l in lists {
                let on = Some(l.id) == list;
                if ui
                    .selectable_label(on, RichText::new(&l.name).size(15.0).strong())
                    .clicked()
                {
                    chosen = Some(l.id);
                }
            }
        } else if let Some(l) = lists.first() {
            ui.label(RichText::new(&l.name).size(15.0).strong());
        }
        if let Some(id) = chosen {
            app.view.list = Some(id);
        }
        if app.client.state.paused_all {
            ui.label(
                RichText::new(" ALL PAUSED ")
                    .strong()
                    .size(14.0)
                    .color(Color32::BLACK)
                    .background_color(theme::PAUSED),
            );
        }
        if let Some(l) = list {
            if app.client.state.playhead(l).is_none() {
                ui.label(
                    RichText::new("End of list - no cue standing by")
                        .size(13.0)
                        .color(theme::PAUSED),
                );
            }
        }
        let touch = app.screen.touch;
        let roomy = rect.width() >= 700.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut app.live.follow, "Follow playhead")
                .on_hover_text("Keep the NEXT cue in view");
            if roomy {
                ui.label(
                    RichText::new(if touch {
                        "Double-tap a cue to stand it by"
                    } else {
                        "Double-click a cue to stand it by"
                    })
                    .size(12.0)
                    .color(theme::MUTED),
                );
            }
        });
    });
    ui.add_space(4.0);
    let input = cues::CueWindowInput {
        state: &app.client.state,
        tick: &app.client.tick,
        list,
        selection: &app.view.selection,
        follow: app.live.follow,
        high_contrast: hc,
        row_h: if app.screen.touch {
            cues::ROW_H_TOUCH
        } else {
            cues::ROW_H
        },
    };
    let actions = cues::cue_window(&mut ui, &input, &mut app.live.cues);
    for a in actions {
        match a {
            cues::CueAction::SetPlayhead(cue) => {
                app.cmd(json!({ "cmd": "set_playhead", "cue": cue, "list": list }));
            }
            cues::CueAction::Select(cue) => app.view.selection = vec![cue],
        }
    }
}

// ---------------------------------------------------------------------------
// Side column: NEXT panel + transport
// ---------------------------------------------------------------------------

fn side_zone(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    let small = rect.height() < 600.0;
    let go_h = (rect.height() * 0.18).clamp(76.0, 150.0);
    let btn_h = if small { 40.0 } else { 48.0 };
    let double_tap = app.client.state.show.settings.panic_double_tap;
    let panic_h = if small { 46.0 } else { 54.0 };
    let panic_caption = if double_tap { 18.0 } else { 0.0 };
    let transport_h = go_h + btn_h * 2.0 + GAP * 2.0 + 22.0 + panic_h + panic_caption + 12.0;
    let transport = Rect::from_min_max(
        Pos2::new(rect.left(), rect.bottom() - transport_h),
        rect.max,
    );
    let next = Rect::from_min_max(rect.min, Pos2::new(rect.right(), transport.top() - GAP));
    next_panel(app, ui, next, hc);
    transport_zone(app, ui, transport, go_h, btn_h, panic_h);
}

fn next_panel(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    let s = &app.client.state;
    let list_id = app.current_list();
    let list = list_id.and_then(|l| s.show.list(l));
    let playhead = list_id.and_then(|l| s.playhead(l));
    let cue = playhead.and_then(|p| s.show.cue(p));
    let error = playhead.and_then(|p| s.error(p));
    let text = if hc {
        Color32::WHITE
    } else {
        Color32::from_gray(245)
    };
    let muted = if hc {
        Color32::from_gray(210)
    } else {
        theme::MUTED
    };

    card(ui, rect, hc);
    if cue.is_some() {
        // Green frame: the cue standing by.
        ui.painter().rect_stroke(
            rect,
            8.0,
            Stroke::new(if hc { 3.0 } else { 2.0 }, theme::NEXT),
            egui::StrokeKind::Inside,
        );
    }
    let inner = rect.shrink(14.0);
    let mut ui = child(ui, inner, "live_next_panel");
    let ui = &mut ui;
    ui.spacing_mut().item_spacing.y = 6.0;
    // Only labels here: no need for touch-sized rows.
    ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.min(26.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(" NEXT ")
                .strong()
                .size(15.0)
                .color(Color32::BLACK)
                .background_color(theme::NEXT),
        );
        if let Some(c) = cue {
            if !c.armed {
                ui.label(
                    RichText::new(" DISARMED ")
                        .size(13.0)
                        .color(Color32::BLACK)
                        .background_color(Color32::from_gray(150)),
                );
            }
        }
        if error.is_some() {
            ui.label(
                RichText::new(" BROKEN ")
                    .size(13.0)
                    .color(Color32::WHITE)
                    .background_color(theme::ERROR),
            );
        }
    });
    let Some(cue) = cue else {
        ui.add_space(10.0);
        ui.label(RichText::new("Nothing standing by").size(26.0).color(muted));
        ui.label(
            RichText::new("Double-click a cue, or use Prev / Next.")
                .size(14.0)
                .color(muted),
        );
        return;
    };

    // Number (huge, green) and name (large, up to 3 lines), painted as one block.
    let w = ui.available_width();
    // As large as the width allows, but never taller than the panel (the
    // narrow layout gives the panel little height).
    let by_h = (ui.available_height() - 26.0) * 0.85;
    let num_size = (w * 0.16).min(by_h).clamp(26.0, 64.0);
    let name_size = (num_size * 0.55).clamp(18.0, 32.0);
    let name_rows = if by_h < 64.0 { 2 } else { 3 };
    let painter = ui.painter().clone();
    let num_g = (!cue.number.is_empty()).then(|| {
        painter.layout_no_wrap(
            cue.number.clone(),
            FontId::proportional(num_size),
            theme::NEXT,
        )
    });
    let num_w = num_g.as_ref().map_or(0.0, |g| g.size().x + 14.0);
    let name = if cue.name.is_empty() {
        cues::type_label(cue).to_string()
    } else {
        cue.name.clone()
    };
    let mut job = egui::text::LayoutJob::simple(
        name,
        FontId::proportional(name_size),
        text,
        (w - num_w).max(40.0),
    );
    job.wrap.max_rows = name_rows;
    let name_g = painter.layout_job(job);
    let num_h = num_g.as_ref().map_or(0.0, |g| g.size().y);
    let block_h = num_h.max(name_g.size().y);
    let (block, _) = ui.allocate_exact_size(Vec2::new(w, block_h), egui::Sense::hover());
    if let Some(g) = num_g {
        let y = block.center().y - g.size().y / 2.0;
        painter.galley(Pos2::new(block.left(), y), g, theme::NEXT);
    }
    let y = block.center().y - name_g.size().y / 2.0;
    painter.galley(Pos2::new(block.left() + num_w, y), name_g, text);

    let target = cues::cue_target(cue, &s.show);
    ui.add(
        egui::Label::new(
            RichText::new(if target.is_empty() {
                cues::type_label(cue).to_string()
            } else {
                format!("{}  ·  {target}", cues::type_label(cue))
            })
            .size(15.0)
            .color(muted),
        )
        .truncate(),
    );
    if let Some(e) = error {
        ui.add(egui::Label::new(RichText::new(e).size(15.0).color(theme::ERROR)).wrap());
    }

    // Following cues (top level).
    let after: Vec<&sm_core::model::Cue> = list
        .and_then(|l| {
            let i = l.cues.iter().position(|c| c.id == cue.id)?;
            Some(l.cues.iter().skip(i + 1).take(3).collect())
        })
        .unwrap_or_default();
    let then_row = 26.0;
    let then_h = if after.is_empty() {
        0.0
    } else {
        24.0 + after.len() as f32 * then_row
    };

    if !cue.notes.is_empty() {
        ui.add_space(4.0);
        let avail = (ui.available_height() - then_h - 8.0).max(0.0);
        if avail > 34.0 {
            egui::Frame::new()
                .fill(if hc {
                    Color32::from_gray(20)
                } else {
                    Color32::from_rgb(38, 36, 22)
                })
                .stroke(Stroke::new(1.0, theme::WARN.gamma_multiply(0.6)))
                .corner_radius(4.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt("next_notes")
                        .max_height(avail - 18.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&cue.notes).size(16.0).color(theme::WARN),
                                )
                                .wrap(),
                            );
                        });
                });
        }
    }

    if !after.is_empty() && ui.available_height() >= then_h {
        // Pin the "then" list to the bottom of the panel.
        ui.add_space((ui.available_height() - then_h).max(0.0));
        caption(ui, "THEN", hc);
        for c in after {
            let dim = |col: Color32| {
                if c.armed {
                    col
                } else {
                    col.gamma_multiply(0.45)
                }
            };
            let (r, _) = ui.allocate_exact_size(Vec2::new(w, then_row - 6.0), egui::Sense::hover());
            let cy = r.center().y;
            painter.hline(
                r.x_range(),
                r.top() - 3.0,
                Stroke::new(1.0, Color32::from_gray(if hc { 90 } else { 45 })),
            );
            let num = cues::elided(
                ui,
                c.number.clone(),
                FontId::proportional(17.0),
                dim(text),
                54.0,
            );
            painter.galley(Pos2::new(r.left(), cy - num.size().y / 2.0), num, text);
            let ty = painter.text(
                Pos2::new(r.right(), cy),
                Align2::RIGHT_CENTER,
                cues::type_label(c),
                FontId::proportional(12.0),
                dim(muted),
            );
            let name = if c.name.is_empty() {
                cues::type_label(c).to_string()
            } else {
                c.name.clone()
            };
            let g = cues::elided(
                ui,
                name,
                FontId::proportional(17.0),
                dim(text),
                (ty.left() - r.left() - 66.0).max(10.0),
            );
            painter.galley(Pos2::new(r.left() + 58.0, cy - g.size().y / 2.0), g, text);
        }
    }
}

/// A transport button with a consistent look; `fill` highlights a state.
fn tbutton(ui: &mut egui::Ui, size: Vec2, label: &str, hint: &str, fill: Option<Color32>) -> bool {
    let mut text = RichText::new(label)
        .strong()
        .size(if size.y < 44.0 { 16.0 } else { 18.0 });
    let mut b = egui::Button::new(text.clone());
    if let Some(f) = fill {
        text = text.color(Color32::BLACK);
        b = egui::Button::new(text).fill(f);
    }
    ui.add_sized(size, b).on_hover_text(hint).clicked()
}

fn transport_zone(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    go_h: f32,
    btn_h: f32,
    panic_h: f32,
) {
    let mut ui = child(ui, rect, "live_transport");
    let ui = &mut ui;
    ui.spacing_mut().item_spacing = Vec2::splat(GAP);
    let w = ui.available_width();

    // GO.
    let has_next = app
        .current_list()
        .and_then(|l| app.client.state.playhead(l))
        .is_some();
    widgets::go_button(app, ui, Vec2::new(w, go_h));
    if !has_next {
        let r = ui.min_rect();
        ui.painter().text(
            Pos2::new(r.center().x, r.top() + go_h - 10.0),
            Align2::CENTER_BOTTOM,
            "no cue standing by",
            FontId::proportional(14.0),
            Color32::from_white_alpha(210),
        );
    }

    // Pause / Stop all.
    let paused = app.client.state.paused_all;
    ui.horizontal(|ui| {
        let half = Vec2::new((w - GAP) / 2.0, btn_h);
        let (label, hint) = if paused {
            ("Resume", "Resume all paused cues")
        } else {
            ("Pause", "Pause all running cues")
        };
        if tbutton(ui, half, label, hint, paused.then_some(theme::PAUSED)) {
            app.toggle_pause();
        }
        if tbutton(
            ui,
            half,
            "Stop all",
            "Stop all cues (with the stop fade)",
            None,
        ) {
            app.stop_all();
        }
    });

    // Playhead + reset.
    ui.horizontal(|ui| {
        let third = Vec2::new((w - GAP * 2.0) / 3.0, btn_h);
        if tbutton(
            ui,
            third,
            "Prev",
            "Move the playhead up (does not fire)",
            None,
        ) {
            app.step_playhead(false);
        }
        if tbutton(
            ui,
            third,
            "Next",
            "Move the playhead down (does not fire)",
            None,
        ) {
            app.step_playhead(true);
        }
        if tbutton(
            ui,
            third,
            "Reset",
            "Stop all, playhead to the top, clear loads and reset the show clock (asks first)",
            None,
        ) {
            app.live.confirm_reset = true;
        }
    });

    // PANIC, visually separated.
    ui.add_space(4.0);
    let y = ui.cursor().top() - 2.0;
    ui.painter()
        .hline(rect.x_range(), y, Stroke::new(1.0, Color32::from_gray(70)));
    ui.add_space(8.0);
    widgets::panic_button(app, ui, Vec2::new(w, panic_h));
    if app.client.state.show.settings.panic_double_tap {
        let armed = app.panic_armed();
        ui.add_space(-4.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(if armed {
                    "ARMED - press PANIC again to stop everything"
                } else {
                    "Double-tap safety: press PANIC twice"
                })
                .size(12.0)
                .color(if armed { theme::ERROR } else { theme::MUTED }),
            );
        });
        if armed {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn reset_confirm(app: &mut App, ui: &mut egui::Ui) {
    if !app.live.confirm_reset {
        return;
    }
    let mut close = false;
    let resp = egui::Modal::new(egui::Id::unique("live_reset_confirm")).show(ui.ctx(), |ui| {
        ui.set_max_width(420.0);
        ui.heading("Reset the show?");
        ui.label("Stops all cues, moves the playhead to the top, clears loads and errors and resets the show clock.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("Reset show").strong().color(Color32::WHITE))
                        .fill(theme::PANIC),
                )
                .clicked()
            {
                app.reset();
                close = true;
            }
        });
    });
    if close || resp.should_close() {
        app.live.confirm_reset = false;
    }
}

// ---------------------------------------------------------------------------
// Bottom tools: meters + master, notes, log
// ---------------------------------------------------------------------------

fn tools_zone(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    let channels = app.live.ballistics.channels.len();
    let fader_w = 62.0;
    let mix_w = (meters::meter_width(channels) + fader_w + 30.0).min(rect.width() * 0.4);
    let mix = Rect::from_min_size(rect.min, Vec2::new(mix_w, rect.height()));
    let rest = rect.width() - mix_w - GAP * 2.0;
    let notes_w = rest * 0.45;
    let notes = Rect::from_min_size(
        Pos2::new(mix.right() + GAP, rect.top()),
        Vec2::new(notes_w, rect.height()),
    );
    let log = Rect::from_min_max(Pos2::new(notes.right() + GAP, rect.top()), rect.max);

    mix_panel(app, ui, mix, hc);
    notes_panel(app, ui, notes, hc);
    log_panel(app, ui, log, hc);
}

/// Meters + master fader in `mix`.
fn mix_panel(app: &mut App, ui: &mut egui::Ui, mix: Rect, hc: bool) {
    let fader_w = 62.0;
    card(ui, mix, hc);
    let inner = mix.shrink(10.0);
    let meter_rect = Rect::from_min_max(
        inner.min,
        Pos2::new(inner.right() - fader_w - 8.0, inner.bottom()),
    );
    let fader_rect = Rect::from_min_max(Pos2::new(inner.right() - fader_w, inner.top()), inner.max);
    let outputs = &app.client.state.show.patch.outputs;
    let name_of = |ch: usize| {
        outputs
            .iter()
            .find(|o| usize::from(o.channel) == ch)
            .map(|o| o.name.clone())
            .filter(|n| !n.is_empty())
    };
    let m = meters::meter_bridge(
        ui,
        meter_rect,
        &app.live.ballistics,
        &app.client.tick.meters.clip,
        &name_of,
    );
    if m.reset_clip {
        app.cmd(json!({ "cmd": "reset_clip" }));
    }
    let engine_db = app.client.state.master_db;
    meters::master_fader(ui, fader_rect, &mut app.live.fader, engine_db);
    if let Some(db) = app.live.fader.due() {
        app.cmd(json!({ "cmd": "set_master", "db": db }));
    }
}

fn notes_panel(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    card(ui, rect, hc);
    let live = &mut app.live;
    let server = &app.client.state.show.notes;
    let recently_sent = live
        .notes_sent_at
        .is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
    if !live.notes_focused
        && live.notes_dirty_at.is_none()
        && !recently_sent
        && live.notes != *server
    {
        live.notes.clone_from(server);
    }
    let mut ui = child(ui, rect.shrink(10.0), "live_notes");
    let ui = &mut ui;
    ui.horizontal(|ui| {
        caption(ui, "SHOW NOTES", hc);
        if live.notes_dirty_at.is_some() {
            ui.label(RichText::new("editing…").size(11.0).color(theme::MUTED));
        }
    });
    let h = ui.available_height();
    let w = ui.available_width();
    let resp = egui::ScrollArea::vertical()
        .id_salt("live_notes_scroll")
        .max_height(h)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_sized(
                [w - 2.0, h - 2.0],
                egui::TextEdit::multiline(&mut live.notes)
                    .hint_text("Stage manager notes…")
                    .font(FontId::proportional(15.0))
                    .id_salt("live_notes_edit"),
            )
        })
        .inner;
    ui.ctx()
        .accesskit_node_builder(resp.id, |b| b.set_label("Show notes"));
    if resp.changed() {
        live.notes_dirty_at = Some(Instant::now());
    }
    live.notes_focused = resp.has_focus();
    let due = live
        .notes_dirty_at
        .is_some_and(|t| resp.lost_focus() || !live.notes_focused || t.elapsed() >= NOTES_IDLE);
    if due {
        live.notes_dirty_at = None;
        live.notes_sent_at = Some(Instant::now());
        let text = live.notes.clone();
        app.cmd(json!({ "cmd": "set_notes", "text": text }));
    } else if live.notes_dirty_at.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

fn log_time(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "--:--:--".into())
}

fn log_panel(app: &mut App, ui: &mut egui::Ui, rect: Rect, hc: bool) {
    card(ui, rect, hc);
    let mut ui = child(ui, rect.shrink(10.0), "live_log");
    let ui = &mut ui;
    caption(ui, "EVENT LOG", hc);
    let log = &app.client.log;
    if log.is_empty() {
        ui.label(RichText::new("No events yet").color(theme::MUTED));
        return;
    }
    let row_h = 18.0;
    let text = if hc {
        Color32::WHITE
    } else {
        Color32::from_gray(220)
    };
    egui::ScrollArea::vertical()
        .id_salt("live_log_scroll")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show_rows(ui, row_h, log.len(), |ui, range| {
            for e in &log[range] {
                let color = match e.level {
                    "error" => theme::ERROR,
                    "warn" => theme::WARN,
                    _ => text,
                };
                ui.horizontal(|ui| {
                    ui.set_height(row_h);
                    ui.label(
                        RichText::new(log_time(e.time_ms))
                            .font(FontId::monospace(12.0))
                            .color(theme::MUTED),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(&e.message).size(13.0).color(color))
                            .truncate(),
                    );
                });
            }
        });
}

// ---------------------------------------------------------------------------
// Narrow layout: everything stacked
// ---------------------------------------------------------------------------

fn stacked(app: &mut App, ui: &mut egui::Ui, r: Rect, hc: bool) {
    let touch = app.screen.touch;
    let clock_h = 50.0;
    let top_h = (r.height() * 0.24).clamp(104.0, 150.0);
    let row_h = if touch { 52.0 } else { 46.0 };
    let strip_h = if touch { 48.0 } else { 34.0 };
    let fixed = clock_h + top_h + row_h + strip_h + GAP * 4.0;
    // Room for the cue window and an open tools pane together.
    let shared = (r.height() - fixed).max(0.0);
    let mut pane_h = if app.live.tools_tab.is_some() {
        (r.height() * 0.3).clamp(130.0, 220.0)
    } else {
        0.0
    };
    // Too little room for both: the open pane takes the cue window's place
    // until its tab is tapped again (GO, PANIC and transport stay).
    let min_cues = 150.0;
    if pane_h > 0.0 && shared - pane_h - GAP < min_cues {
        pane_h = shared - GAP;
    }

    let mut y = r.top();
    let mut take = |h: f32| {
        let rect = Rect::from_min_size(Pos2::new(r.left(), y), Vec2::new(r.width(), h));
        y += h + GAP;
        rect
    };
    let clocks = take(clock_h);
    let top = take(top_h);
    let transport = take(row_h);
    let bottom_h = strip_h + if pane_h > 0.0 { pane_h + GAP } else { 0.0 };
    let cue_rect = Rect::from_min_max(
        Pos2::new(r.left(), y),
        Pos2::new(r.right(), (r.bottom() - bottom_h - GAP).max(y)),
    );
    let tools = Rect::from_min_max(Pos2::new(r.left(), r.bottom() - bottom_h), r.max);

    let clock_action = clocks::clock_strip(ui, clocks, &app.client.tick);
    if clock_action.reset_show_clock {
        app.cmd(json!({ "cmd": "reset_show_clock" }));
    }
    if let Some(cmd) = clock_action.timecode {
        app.cmd(json!({ "cmd": cmd }));
    }

    // NEXT panel and a big GO (right: thumb side on a tablet held in landscape).
    let go_w = (r.width() * 0.32).clamp(180.0, 300.0);
    let go = Rect::from_min_max(Pos2::new(top.right() - go_w, top.top()), top.max);
    let next = Rect::from_min_max(top.min, Pos2::new(go.left() - GAP, top.bottom()));
    next_panel(app, ui, next, hc);
    {
        let mut c = child(ui, go, "live_go_stacked");
        widgets::go_button(app, &mut c, go.size());
        let has_next = app
            .current_list()
            .and_then(|l| app.client.state.playhead(l))
            .is_some();
        if !has_next {
            c.painter().text(
                Pos2::new(go.center().x, go.bottom() - 8.0),
                Align2::CENTER_BOTTOM,
                "no cue standing by",
                FontId::proportional(13.0),
                Color32::from_white_alpha(210),
            );
        }
    }

    // Transport row; PANIC is wider and at the far right.
    {
        let mut c = child(ui, transport, "live_transport_stacked");
        let ui = &mut c;
        ui.spacing_mut().item_spacing = Vec2::splat(GAP);
        let unit = (transport.width() - GAP * 5.0) / 6.6;
        let b = Vec2::new(unit, row_h);
        let paused = app.client.state.paused_all;
        ui.horizontal(|ui| {
            let (label, hint) = if paused {
                ("Resume", "Resume all paused cues")
            } else {
                ("Pause", "Pause all running cues")
            };
            if tbutton(ui, b, label, hint, paused.then_some(theme::PAUSED)) {
                app.toggle_pause();
            }
            if tbutton(
                ui,
                b,
                "Stop all",
                "Stop all cues (with the stop fade)",
                None,
            ) {
                app.stop_all();
            }
            if tbutton(ui, b, "Prev", "Move the playhead up (does not fire)", None) {
                app.step_playhead(false);
            }
            if tbutton(
                ui,
                b,
                "Next",
                "Move the playhead down (does not fire)",
                None,
            ) {
                app.step_playhead(true);
            }
            if tbutton(
                ui,
                b,
                "Reset",
                "Stop all, playhead to the top, clear loads and reset the show clock (asks first)",
                None,
            ) {
                app.live.confirm_reset = true;
            }
            let rest = ui.available_width();
            widgets::panic_button(app, ui, Vec2::new(rest, row_h));
        });
        if app.panic_armed() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
    }

    if cue_rect.height() >= 60.0 {
        cue_zone(app, ui, cue_rect, hc);
    }
    tools_strip(app, ui, tools, strip_h, hc);
}

/// Tab strip (Meters / Notes / Log) with one pane below it; tapping the open
/// tab folds the pane. Small level bars stay visible in the strip.
fn tools_strip(app: &mut App, ui: &mut egui::Ui, rect: Rect, strip_h: f32, hc: bool) {
    // The pane laid out for this frame (a tap below takes effect next frame).
    let open = app.live.tools_tab;
    let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), strip_h));
    {
        let mut c = child(ui, strip, "live_tools_tabs");
        c.horizontal(|ui| {
            ui.set_height(strip_h);
            for (tab, label) in [
                (ToolsTab::Meters, "Meters"),
                (ToolsTab::Notes, "Notes"),
                (ToolsTab::Log, "Log"),
            ] {
                let on = app.live.tools_tab == Some(tab);
                let resp = ui
                    .add(
                        egui::Button::new(RichText::new(label).strong())
                            .selected(on)
                            .min_size(Vec2::new(84.0, strip_h - 4.0)),
                    )
                    .on_hover_text(if on {
                        "Tap again to fold"
                    } else {
                        "Show this pane"
                    });
                if resp.clicked() {
                    app.live.tools_tab = if on { None } else { Some(tab) };
                    ui.ctx().request_repaint();
                }
            }
        });
        // Mini meters at the right end of the strip.
        let n = app.live.ballistics.channels.len();
        if n > 0 {
            let w = (strip.width() * 0.3).min(240.0);
            let area = Rect::from_min_max(
                Pos2::new(strip.right() - w, strip.top() + 6.0),
                Pos2::new(strip.right(), strip.bottom() - 6.0),
            );
            let p = ui.painter_at(area);
            let h = ((area.height() - (n as f32 - 1.0) * 2.0) / n as f32).clamp(2.0, 8.0);
            for (i, ch) in app.live.ballistics.channels.iter().enumerate() {
                let top = area.top() + i as f32 * (h + 2.0);
                if top + h > area.bottom() + 0.5 {
                    break;
                }
                let bar =
                    Rect::from_min_size(Pos2::new(area.left(), top), Vec2::new(area.width(), h));
                p.rect_filled(bar, 1.0, Color32::from_gray(if hc { 40 } else { 32 }));
                let f = meters::frac(ch.peak);
                let color = if ch.peak > -3.0 {
                    theme::ERROR
                } else if ch.peak > -12.0 {
                    theme::PAUSED
                } else {
                    theme::NEXT
                };
                let mut fill = bar;
                fill.set_width(bar.width() * f);
                p.rect_filled(fill, 1.0, color);
            }
        }
    }
    let pane = Rect::from_min_max(Pos2::new(rect.left(), strip.bottom() + GAP), rect.max);
    if open != Some(ToolsTab::Notes) {
        flush_notes(app);
    }
    if pane.height() < 40.0 {
        return;
    }
    match open {
        Some(ToolsTab::Meters) => mix_panel(app, ui, pane, hc),
        Some(ToolsTab::Notes) => notes_panel(app, ui, pane, hc),
        Some(ToolsTab::Log) => log_panel(app, ui, pane, hc),
        None => {}
    }
}

/// Send typed notes whose editor is no longer shown.
fn flush_notes(app: &mut App) {
    let live = &mut app.live;
    if live.notes_dirty_at.take().is_some() {
        live.notes_focused = false;
        live.notes_sent_at = Some(Instant::now());
        let text = live.notes.clone();
        app.cmd(json!({ "cmd": "set_notes", "text": text }));
    }
}
