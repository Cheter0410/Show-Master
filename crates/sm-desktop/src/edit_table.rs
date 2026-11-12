//! The edit-mode cue table: virtualised rows (only the visible ones are laid
//! out and painted, so 500+ cues stay cheap), selection, playhead gutter,
//! inline edit of number / name, drag & drop reordering and the row context
//! menu.

use super::{cue_label, fields, target_text, type_label, EditState, Out};
use crate::model::{Active, Phase, Tick, UiState};
use crate::{theme, util};
use egui::accesskit::Role;
use egui::{
    pos2, vec2, Align, Color32, CursorIcon, Id, Key, Modifiers, Painter, Pos2, Rangef, Rect,
    RichText, Sense, Shape, Stroke, TextStyle, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText,
};
use serde_json::{json, Value};
use sm_core::model::{ContinueMode, Cue, CueKind, CueList};
use std::collections::HashSet;

pub const ROW_H: f32 = 30.0;
/// Row height with touch-friendly controls.
pub const ROW_H_TOUCH: f32 = 44.0;
const HEADER_H: f32 = 24.0;
const INDENT: f32 = 18.0;

#[derive(Default)]
pub struct TableState {
    /// Collapsed groups.
    pub collapsed: HashSet<u64>,
    /// Anchor of shift-click range selection.
    anchor: Option<u64>,
    /// Touch "Select" mode: a tap adds / removes the cue instead of
    /// replacing the selection (Ctrl-click for fingers).
    pub multi: bool,
    inline: Option<Inline>,
    drop: Option<DropTarget>,
    renumber: Option<Renumber>,
    /// Scroll this cue into view (after it was added / selected elsewhere).
    pub scroll_to: Option<u64>,
    scroll_tries: u8,
    last_playhead: Option<Option<u64>>,
    offset: f32,
    view_h: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InlineField {
    Number,
    Name,
}

struct Inline {
    cue: u64,
    field: InlineField,
    opened: bool,
}

/// Where dragged cues land relative to the row under the pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropPos {
    Before,
    After,
    /// Into a group (appended to its children).
    Into,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropTarget {
    Row(u64, DropPos),
    /// Below the last row: end of the list (top level).
    End,
}

struct Renumber {
    cues: Vec<u64>,
    start: String,
    step: String,
}

/// Drag & drop payload: the cues being moved.
struct DragCues(Vec<u64>);

/// One visible row of the flattened group tree.
pub struct Row<'a> {
    pub cue: &'a Cue,
    pub depth: usize,
    /// Parent group, `None` at top level.
    pub parent: Option<u64>,
    /// Index within its parent container.
    pub index: usize,
}

/// Flatten the cue tree into rows, skipping the children of collapsed groups.
pub fn flatten<'a>(
    cues: &'a [Cue],
    collapsed: &HashSet<u64>,
    depth: usize,
    parent: Option<u64>,
    out: &mut Vec<Row<'a>>,
) {
    for (index, cue) in cues.iter().enumerate() {
        out.push(Row {
            cue,
            depth,
            parent,
            index,
        });
        if let CueKind::Group(g) = &cue.kind {
            if !collapsed.contains(&cue.id) {
                flatten(&g.children, collapsed, depth + 1, Some(cue.id), out);
            }
        }
    }
}

fn contains(cue: &Cue, id: u64) -> bool {
    cue.id == id || cue.children().iter().any(|c| contains(c, id))
}

/// The `move_cues` command for dropping `moving` on `target` in `list`.
/// `Ok(None)` = nothing to do; `Err` = not allowed (with the reason).
/// Only the top-most of the moved cues travel (children go with their
/// group), in list order. The index counts positions in the destination
/// container before the moved cues are removed, as the server expects.
pub fn move_command(
    list: &CueList,
    moving: &[u64],
    target: DropTarget,
) -> Result<Option<Value>, String> {
    let mut rows = Vec::new();
    flatten(&list.cues, &HashSet::new(), 0, None, &mut rows);
    let picked: Vec<&Cue> = rows
        .iter()
        .filter(|r| moving.contains(&r.cue.id))
        .map(|r| r.cue)
        .collect();
    let top: Vec<&Cue> = picked
        .iter()
        .copied()
        .filter(|c| !picked.iter().any(|o| o.id != c.id && contains(o, c.id)))
        .collect();
    if top.is_empty() {
        return Ok(None);
    }
    let ids: Vec<u64> = top.iter().map(|c| c.id).collect();
    let (parent, index) = match target {
        DropTarget::End => (None, list.cues.len()),
        DropTarget::Row(t, pos) => {
            let Some(row) = rows.iter().find(|r| r.cue.id == t) else {
                return Ok(None);
            };
            if top.iter().any(|c| contains(c, t)) {
                return if ids.contains(&t) {
                    Ok(None)
                } else {
                    Err("A group cannot be moved into itself.".into())
                };
            }
            let is_group = matches!(row.cue.kind, CueKind::Group(_));
            match pos {
                DropPos::Into if is_group => (Some(t), row.cue.children().len()),
                DropPos::Before => (row.parent, row.index),
                DropPos::After | DropPos::Into => (row.parent, row.index + 1),
            }
        }
    };
    Ok(Some(json!({
        "cmd": "move_cues", "cues": ids, "list": list.id, "parent": parent, "index": index,
    })))
}

/// Drop position from the pointer height inside a row.
pub fn drop_pos(rect: Rect, y: f32, is_group: bool) -> DropPos {
    let f = (y - rect.top()) / rect.height().max(1.0);
    if is_group {
        if f < 0.25 {
            DropPos::Before
        } else if f > 0.75 {
            DropPos::After
        } else {
            DropPos::Into
        }
    } else if f < 0.5 {
        DropPos::Before
    } else {
        DropPos::After
    }
}

/// Click selection: plain click selects one, Ctrl/Cmd toggles, Shift selects
/// the range from the anchor over the visible rows (`order`).
pub fn click_select(
    sel: &mut Vec<u64>,
    anchor: &mut Option<u64>,
    order: &[u64],
    id: u64,
    mods: Modifiers,
) {
    if mods.shift {
        if let Some(a) = *anchor {
            if let (Some(i), Some(j)) = (
                order.iter().position(|x| *x == a),
                order.iter().position(|x| *x == id),
            ) {
                *sel = order[i.min(j)..=i.max(j)].to_vec();
                return;
            }
        }
    }
    *anchor = Some(id);
    if mods.command {
        if let Some(p) = sel.iter().position(|x| *x == id) {
            sel.remove(p);
        } else {
            sel.push(id);
        }
    } else {
        *sel = vec![id];
    }
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

struct Cols {
    gutter: Rangef,
    status: Rangef,
    number: Rangef,
    kind: Rangef,
    name: Rangef,
    target: Rangef,
    pre: Rangef,
    dur: Rangef,
    post: Rangef,
    cont: Rangef,
    tc: Rangef,
    armed: Rangef,
    /// Touch mode: drag handle for reordering.
    grip: Rangef,
    /// Touch mode: "More" button (the row's context menu).
    more: Rangef,
}

/// Table width from which all columns are shown (plus the touch columns).
const FULL_W: f32 = 1080.0;
/// Below this width the type and pre-wait columns are hidden too.
const MIN_W: f32 = 700.0;

impl Cols {
    /// Columns for a table `width` wide. Less important columns are hidden
    /// (zero width) on smaller tables; their values stay in the inspector:
    /// - below `FULL_W`: post-wait, continue glyph, TC trigger, armed;
    /// - below `MIN_W`: also type and pre-wait.
    fn new(left: f32, width: f32, touch: bool) -> Self {
        let extra = if touch { 40.0 + 44.0 } else { 0.0 };
        let level = if width >= FULL_W + extra {
            0
        } else if width >= MIN_W + extra {
            1
        } else {
            2
        };
        let upto = |max_level: u8, w: f32| if level <= max_level { w } else { 0.0 };
        let (kind, pre) = (upto(1, 74.0), upto(1, 76.0));
        let (post, cont, tc, armed) = (upto(0, 76.0), upto(0, 30.0), upto(0, 104.0), upto(0, 54.0));
        let (grip, more) = if touch { (40.0, 44.0) } else { (0.0, 0.0) };
        let fixed = 52.0 + 26.0 + 58.0 + kind + pre + 86.0 + post + cont + tc + armed + grip + more;
        let flex = (width - fixed).max(if level == 2 { 160.0 } else { 220.0 });
        let name_w = (flex * if level == 2 { 0.55 } else { 0.45 }).floor();
        let mut x = left;
        let mut take = |w: f32| {
            let r = Rangef::new(x, x + w);
            x += w;
            r
        };
        Cols {
            gutter: take(52.0),
            status: take(26.0),
            number: take(58.0),
            kind: take(kind),
            name: take(name_w),
            target: take(flex - name_w),
            pre: take(pre),
            dur: take(86.0),
            post: take(post),
            cont: take(cont),
            tc: take(tc),
            armed: take(armed),
            grip: take(grip),
            more: take(more),
        }
    }
}

/// A column that is shown (hidden columns have zero width).
fn visible(x: Rangef) -> bool {
    x.span() > 0.5
}

fn cell(x: Rangef, row: Rect) -> Rect {
    Rect::from_x_y_ranges(x, row.y_range())
}

/// Paint text truncated to the cell.
fn text_in(ui: &Ui, painter: &Painter, cell: Rect, text: impl Into<WidgetText>, align: Align) {
    if cell.width() < 1.0 {
        return; // hidden column
    }
    let pad = 6.0;
    let galley = text.into().into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        (cell.width() - 2.0 * pad).max(4.0),
        TextStyle::Body,
    );
    let w = galley.size().x;
    let x = match align {
        Align::Min => cell.left() + pad,
        Align::Center => cell.center().x - w / 2.0,
        Align::Max => cell.right() - pad - w,
    };
    let pos = pos2(x, cell.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, ui.visuals().text_color());
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

pub(crate) fn show(
    ui: &mut Ui,
    state: &UiState,
    tick: &Tick,
    list_id: u64,
    sel: &mut Vec<u64>,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let Some(list) = state.show.list(list_id) else {
        return;
    };
    let st = &mut edit.table;
    let mut rows = Vec::with_capacity(64);
    flatten(&list.cues, &st.collapsed, 0, None, &mut rows);
    let playhead = state.playhead(list_id);
    if st.last_playhead.is_some_and(|p| p != playhead) && playhead.is_some() {
        st.scroll_to = playhead;
    }
    st.last_playhead = Some(playhead);

    // Ctrl/Cmd+A selects all visible rows (unless a text field has focus).
    if !ui.ctx().egui_wants_keyboard_input()
        && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A))
    {
        *sel = rows.iter().map(|r| r.cue.id).collect();
    }

    let touch = crate::responsive::get(ui.ctx()).touch;
    let row_h = if touch { ROW_H_TOUCH } else { ROW_H };
    let full = ui.available_rect_before_wrap();
    // Solid (touch) scroll bars take room from the rows.
    let bar = ui.spacing().scroll.allocated_width();
    let cols = Cols::new(full.left(), full.width() - bar, touch);
    // Header
    let (head, _) = ui.allocate_exact_size(vec2(full.width(), HEADER_H), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(head, 0.0, ui.visuals().faint_bg_color);
    let h = |t: &str| RichText::new(t).small().strong().color(theme::MUTED);
    for (x, t, a) in [
        (cols.gutter, "", Align::Min),
        (cols.number, "#", Align::Min),
        (cols.kind, "Type", Align::Min),
        (cols.name, "Name", Align::Min),
        (cols.target, "Target / File", Align::Min),
        (cols.pre, "Pre-wait", Align::Max),
        (cols.dur, "Duration", Align::Max),
        (cols.post, "Post-wait", Align::Max),
        (cols.tc, "TC trigger", Align::Min),
        (cols.armed, "Armed", Align::Center),
    ] {
        if visible(x) && !t.is_empty() {
            text_in(ui, painter, cell(x, head), h(t), a);
        }
    }
    painter.hline(
        head.x_range(),
        head.bottom() - 0.5,
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );

    // Scroll a requested row into view.
    // Touch mode: rows only sense clicks, so dragging the list scrolls it
    // (cues are reordered with the grip column instead).
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("edit_table", list_id))
        .auto_shrink(false)
        .scroll_source(crate::responsive::scroll_source(ui.ctx()));
    if let Some(id) = st.scroll_to {
        if let Some(i) = rows.iter().position(|r| r.cue.id == id) {
            let top = i as f32 * row_h;
            if top < st.offset {
                area = area.vertical_scroll_offset(top);
            } else if top + row_h > st.offset + st.view_h && st.view_h > 0.0 {
                area = area.vertical_scroll_offset(top + row_h - st.view_h);
            }
            st.scroll_to = None;
        } else {
            st.scroll_tries += 1;
            if st.scroll_tries > 30 {
                st.scroll_to = None;
            }
        }
        if st.scroll_to.is_none() {
            st.scroll_tries = 0;
        }
    }

    let order: Vec<u64> = rows.iter().map(|r| r.cue.id).collect();
    let dragging = egui::DragAndDrop::has_payload_of_type::<DragCues>(ui.ctx());
    let output = area.show_viewport(ui, |ui, viewport| {
        let content = ui.max_rect();
        let total = rows.len() as f32 * row_h;
        let height = total.max(viewport.height());
        ui.set_height(height);
        // Background: click on empty space clears the selection.
        let bg_rect = Rect::from_min_size(content.min, vec2(content.width(), height));
        let bg = ui.interact(bg_rect, ui.scope_id().with("edit_table_bg"), Sense::click());
        if bg.clicked() {
            sel.clear();
        }
        if rows.is_empty() {
            let c = pos2(content.center().x, content.top() + viewport.height() * 0.4);
            ui.painter().text(
                c,
                egui::Align2::CENTER_CENTER,
                "No cues in this list yet",
                egui::FontId::proportional(18.0),
                ui.visuals().text_color(),
            );
            ui.painter().text(
                c + vec2(0.0, 26.0),
                egui::Align2::CENTER_CENTER,
                "Add a cue above or drop audio files here.",
                egui::FontId::proportional(14.0),
                theme::MUTED,
            );
        }
        let first = (viewport.top() / row_h).floor().max(0.0) as usize;
        let last = ((viewport.bottom() / row_h).ceil() as usize + 1).min(rows.len());
        let ctx = RowCtx {
            state,
            tick,
            list_id,
            playhead,
            cols: &cols,
            order: &order,
            touch,
        };
        for (i, row) in rows.iter().enumerate().take(last).skip(first) {
            let rect = Rect::from_min_size(
                pos2(content.left(), content.top() + i as f32 * row_h),
                vec2(content.width(), row_h),
            );
            draw_row(ui, &ctx, row, i, rect, sel, edit, out);
        }

        // Drag & drop of cues inside the table.
        let st = &mut edit.table;
        if dragging {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            let pointer = ui.input(|i| i.pointer.interact_pos());
            st.drop = pointer
                .filter(|p| viewport.translate(content.min.to_vec2()).contains(*p))
                .map(|p| {
                    let i = ((p.y - content.top()) / row_h).floor();
                    match rows.get(i.max(0.0) as usize) {
                        Some(r) if i >= 0.0 => {
                            let rect = Rect::from_min_size(
                                pos2(content.left(), content.top() + i * row_h),
                                vec2(content.width(), row_h),
                            );
                            let group = matches!(r.cue.kind, CueKind::Group(_));
                            DropTarget::Row(r.cue.id, drop_pos(rect, p.y, group))
                        }
                        _ => DropTarget::End,
                    }
                });
            if let Some(p) = pointer {
                // Auto-scroll near the edges.
                let vis = viewport.translate(content.min.to_vec2());
                if p.y < vis.top() + 24.0 && p.y > vis.top() - 40.0 {
                    ui.scroll_with_delta(vec2(0.0, 6.0));
                } else if p.y > vis.bottom() - 24.0 && p.y < vis.bottom() + 40.0 {
                    ui.scroll_with_delta(vec2(0.0, -6.0));
                }
            }
            paint_drop(ui, st.drop, &rows, content, total, row_h);
            if ui.input(|i| i.pointer.any_released()) {
                let payload = egui::DragAndDrop::take_payload::<DragCues>(ui.ctx());
                if let (Some(p), Some(target)) = (payload, st.drop) {
                    match move_command(list, &p.0, target) {
                        Ok(Some(cmd)) => out.push(Out::Cmd(cmd)),
                        Ok(None) => {}
                        Err(e) => out.push(Out::Error(e)),
                    }
                }
                st.drop = None;
            }
        } else {
            st.drop = None;
        }
    });
    let st = &mut edit.table;
    st.offset = output.state.clamped_offset().y;
    st.view_h = output.inner_rect.height();

    if dragging {
        if let (Some(p), Some(payload)) = (
            ui.input(|i| i.pointer.hover_pos()),
            egui::DragAndDrop::payload::<DragCues>(ui.ctx()),
        ) {
            let n = payload.0.len();
            egui::Area::new(Id::unique("edit_drag_ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(p + vec2(16.0, 10.0))
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(format!("Move {n} cue{}", if n == 1 { "" } else { "s" }));
                    });
                });
        }
    }
    renumber_dialog(ui.ctx(), &mut edit.table, out);
}

fn paint_drop(
    ui: &Ui,
    drop: Option<DropTarget>,
    rows: &[Row<'_>],
    content: Rect,
    total: f32,
    row_h: f32,
) {
    let Some(drop) = drop else { return };
    let stroke = Stroke::new(2.5, theme::RUNNING);
    let painter = ui.painter();
    match drop {
        DropTarget::End => {
            painter.hline(content.x_range(), content.top() + total, stroke);
        }
        DropTarget::Row(id, pos) => {
            let Some(i) = rows.iter().position(|r| r.cue.id == id) else {
                return;
            };
            let top = content.top() + i as f32 * row_h;
            let rect = Rect::from_min_size(pos2(content.left(), top), vec2(content.width(), row_h));
            let indent = rows[i].depth as f32 * INDENT;
            let x = Rangef::new(content.left() + 52.0 + indent, content.right());
            match pos {
                DropPos::Before => {
                    painter.hline(x, rect.top() + 1.0, stroke);
                }
                DropPos::After => {
                    painter.hline(x, rect.bottom() - 1.0, stroke);
                }
                DropPos::Into => {
                    painter.rect_stroke(rect.shrink(1.5), 4.0, stroke, egui::StrokeKind::Inside);
                }
            }
        }
    }
}

struct RowCtx<'a> {
    state: &'a UiState,
    tick: &'a Tick,
    list_id: u64,
    playhead: Option<u64>,
    cols: &'a Cols,
    order: &'a [u64],
    touch: bool,
}

/// What the row shows about the cue's runtime state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunState {
    Idle,
    PreWait,
    Running,
    PostWait,
    Paused,
    Error,
    Loaded,
}

fn run_state(active: Option<&Active>, error: bool, loaded: bool) -> RunState {
    match active {
        _ if error => RunState::Error,
        Some(a) if a.paused => RunState::Paused,
        Some(a) => match a.phase {
            Phase::PreWait => RunState::PreWait,
            Phase::Running => RunState::Running,
            Phase::PostWait => RunState::PostWait,
        },
        None if loaded => RunState::Loaded,
        None => RunState::Idle,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    ui: &mut Ui,
    ctx: &RowCtx<'_>,
    row: &Row<'_>,
    i: usize,
    rect: Rect,
    sel: &mut Vec<u64>,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let cue = row.cue;
    let id = cue.id;
    let cols = ctx.cols;
    let selected = sel.contains(&id);
    let active = ctx.tick.active(id);
    let error = ctx.state.error(id);
    let loaded = ctx.state.loaded(id);
    let rs = run_state(active, error.is_some(), loaded.is_some());
    let is_playhead = row.depth == 0 && ctx.playhead == Some(id);
    let is_group = matches!(cue.kind, CueKind::Group(_));
    let label = format!("Cue {}", cue_label(cue));

    // Touch: a plain drag scrolls the table; the grip column moves cues.
    let sense = if ctx.touch {
        Sense::click()
    } else {
        Sense::click_and_drag()
    };
    let resp = ui.interact(rect, Id::unique(("edit_row", id)), sense);
    let hovered = resp.hovered();

    // --- background
    let painter = ui.painter().clone();
    let v = ui.visuals();
    let bg = if selected {
        Some(v.selection.bg_fill)
    } else {
        match rs {
            RunState::Error => Some(theme::ERROR_BG),
            RunState::Running | RunState::PreWait | RunState::PostWait => Some(theme::RUNNING_BG),
            RunState::Paused => Some(Color32::from_rgb(66, 46, 12)),
            _ if is_playhead => Some(theme::NEXT_BG),
            _ if hovered => Some(v.widgets.hovered.weak_bg_fill),
            _ if i % 2 == 1 => Some(v.faint_bg_color),
            _ => None,
        }
    };
    if let Some(bg) = bg {
        painter.rect_filled(rect, 0.0, bg);
    }
    if selected && hovered {
        painter.rect_filled(rect, 0.0, Color32::from_white_alpha(8));
    }
    if is_playhead {
        painter.rect_filled(
            Rect::from_x_y_ranges(Rangef::new(rect.left(), rect.left() + 3.0), rect.y_range()),
            0.0,
            theme::NEXT,
        );
        painter.hline(
            rect.x_range(),
            rect.top() + 0.5,
            Stroke::new(1.0, theme::NEXT),
        );
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, theme::NEXT),
        );
    }
    // progress bar of the running phase
    if let Some(a) = active {
        if let Some(p) = a.progress() {
            let color = if a.paused {
                theme::PAUSED
            } else {
                theme::RUNNING
            };
            let bar = Rect::from_min_max(
                pos2(rect.left(), rect.bottom() - 3.0),
                pos2(rect.left() + rect.width() * p, rect.bottom()),
            );
            painter.rect_filled(bar, 0.0, color);
        }
    }

    let (fg, muted) = if selected {
        (v.strong_text_color(), Color32::from_gray(200))
    } else {
        (v.text_color(), theme::MUTED)
    };
    let (fg, muted) = if cue.armed {
        (fg, muted)
    } else {
        (fg.gamma_multiply(0.4), muted.gamma_multiply(0.5))
    };

    // --- gutter: NEXT tag
    let gutter = cell(cols.gutter, rect);
    if is_playhead {
        next_tag(&painter, gutter, theme::NEXT, Color32::BLACK);
    }

    // --- status + colour swatch
    let status = cell(cols.status, rect);
    if let Some(c) = theme::cue_color(&cue.color) {
        painter.rect_filled(
            Rect::from_x_y_ranges(
                Rangef::new(status.left() + 1.0, status.left() + 5.0),
                rect.y_range().shrink(3.0),
            ),
            2.0,
            c,
        );
    }
    status_icon(&painter, status.center() + vec2(2.0, 0.0), rs);

    // --- number, type
    let inline = edit
        .table
        .inline
        .as_ref()
        .filter(|e| e.cue == id)
        .map(|e| e.field);
    if inline != Some(InlineField::Number) {
        text_in(
            ui,
            &painter,
            cell(cols.number, rect),
            RichText::new(&cue.number).strong().color(fg),
            Align::Min,
        );
    }
    text_in(
        ui,
        &painter,
        cell(cols.kind, rect),
        RichText::new(type_label(cue)).color(muted),
        Align::Min,
    );

    // --- name (indented, group twisty, loaded tag)
    let name_cell = cell(cols.name, rect);
    let indent = row.depth as f32 * INDENT;
    let mut name_left = name_cell.left() + indent;
    let mut twisty = None;
    if is_group {
        let t = Rect::from_center_size(pos2(name_left + 10.0, rect.center().y), vec2(18.0, 18.0));
        twisty = Some(t);
        name_left += 18.0;
    }
    let mut name_rect =
        Rect::from_x_y_ranges(Rangef::new(name_left, name_cell.right()), rect.y_range());
    if let Some(t) = loaded {
        let tag = format!("LOADED {}", util::fmt_time(t));
        let galley = painter.layout_no_wrap(tag, egui::FontId::proportional(10.5), Color32::BLACK);
        let w = galley.size().x + 10.0;
        if name_rect.width() > w + 40.0 {
            let r = Rect::from_center_size(
                pos2(name_rect.right() - w / 2.0 - 4.0, rect.center().y),
                vec2(w, 16.0),
            );
            painter.rect_filled(r, 3.0, theme::LOADED);
            painter.galley(r.center() - galley.size() / 2.0, galley, Color32::BLACK);
            name_rect.max.x -= w + 6.0;
        }
    }
    if inline != Some(InlineField::Name) {
        let text = if cue.name.is_empty() {
            RichText::new(type_label(cue)).italics().color(muted)
        } else if is_group {
            RichText::new(&cue.name).strong().color(fg)
        } else {
            RichText::new(&cue.name).color(fg)
        };
        text_in(ui, &painter, name_rect, text, Align::Min);
    }

    // --- target / file (or error)
    let target_cell = cell(cols.target, rect);
    match error {
        Some(e) => text_in(
            ui,
            &painter,
            target_cell,
            RichText::new(e).color(theme::ERROR),
            Align::Min,
        ),
        None => text_in(
            ui,
            &painter,
            target_cell,
            RichText::new(target_text(cue, &ctx.state.show)).color(muted),
            Align::Min,
        ),
    }

    // --- times
    let mono = |t: String, c: Color32, strong: bool| {
        let r = RichText::new(t).monospace().color(c);
        if strong {
            r.strong()
        } else {
            r
        }
    };
    let phase_text = |phase: Phase| -> Option<(String, Color32)> {
        let a = active.filter(|a| a.phase == phase)?;
        let c = if a.paused {
            theme::PAUSED
        } else {
            theme::RUNNING
        };
        Some(match a.remaining() {
            Some(r) => (format!("-{}", util::fmt_time(r)), c),
            None => (util::fmt_time(a.elapsed), c),
        })
    };
    let pre = match phase_text(Phase::PreWait) {
        Some((t, c)) => mono(t, c, true),
        None if cue.pre_wait > 0.0 => mono(util::fmt_time(cue.pre_wait), fg, false),
        None => mono(String::new(), fg, false),
    };
    text_in(ui, &painter, cell(cols.pre, rect), pre, Align::Max);
    let dur = match phase_text(Phase::Running) {
        Some((t, c)) => mono(t, c, true),
        None => match (cue.action_duration(), &cue.kind) {
            (None, CueKind::Audio(_)) => mono("∞".into(), fg, false),
            (Some(d), CueKind::Audio(_) | CueKind::Fade(_)) => mono(util::fmt_time(d), fg, false),
            _ => mono(String::new(), fg, false),
        },
    };
    text_in(ui, &painter, cell(cols.dur, rect), dur, Align::Max);
    let post = match phase_text(Phase::PostWait) {
        Some((t, c)) => mono(t, c, true),
        None if cue.post_wait > 0.0 => mono(util::fmt_time(cue.post_wait), fg, false),
        None => mono(String::new(), fg, false),
    };
    text_in(ui, &painter, cell(cols.post, rect), post, Align::Max);
    if visible(cols.cont) {
        let cont_cell = cell(cols.cont, rect);
        continue_glyph(&painter, cont_cell.center(), cue.continue_mode, muted);
    }
    if let Some(tc) = cue.tc_trigger {
        text_in(
            ui,
            &painter,
            cell(cols.tc, rect),
            mono(tc.to_string(), fg, false),
            Align::Min,
        );
    }

    // ------------------------------------------------------------------
    // Interaction (widgets registered after the row take precedence)
    // ------------------------------------------------------------------
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let in_cell = |x: Rangef| pointer.is_some_and(|p| x.contains(p.x));

    if resp.clicked() {
        let mut mods = ui.input(|i| i.modifiers);
        if edit.table.multi && !mods.shift {
            mods.command = true;
        }
        click_select(sel, &mut edit.table.anchor, ctx.order, id, mods);
    }
    if resp.secondary_clicked() && !sel.contains(&id) {
        *sel = vec![id];
        edit.table.anchor = Some(id);
    }
    if resp.double_clicked() {
        if in_cell(cols.number) {
            edit.table.inline = Some(Inline {
                cue: id,
                field: InlineField::Number,
                opened: false,
            });
        } else if in_cell(Rangef::new(name_left, name_cell.right())) {
            edit.table.inline = Some(Inline {
                cue: id,
                field: InlineField::Name,
                opened: false,
            });
        }
    }
    // Touch: grip (drag to reorder) and "More" (the context menu, for
    // fingers that cannot right-click; a long press opens it too).
    let mut drag_started = resp.drag_started();
    if ctx.touch {
        let grip = cell(cols.grip, rect);
        let g = ui.interact(grip, Id::unique(("edit_grip", id)), Sense::drag());
        let c = if g.hovered() || g.dragged() {
            fg
        } else {
            muted
        };
        for dy in [-5.0, 0.0, 5.0] {
            painter.hline(
                Rangef::new(grip.center().x - 8.0, grip.center().x + 8.0),
                grip.center().y + dy,
                Stroke::new(2.0, c),
            );
        }
        drag_started |= g.drag_started();
        let name = format!("Move {}", cue_label(cue));
        g.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));

        let more = cell(cols.more, rect).shrink2(vec2(4.0, 4.0));
        let m = ui.interact(more, Id::unique(("edit_more", id)), Sense::click());
        if m.hovered() {
            painter.rect_filled(more, 4.0, v.widgets.hovered.weak_bg_fill);
        }
        for dx in [-6.0, 0.0, 6.0] {
            painter.circle_filled(more.center() + vec2(dx, 0.0), 2.2, fg);
        }
        if m.clicked() && !sel.contains(&id) {
            *sel = vec![id];
            edit.table.anchor = Some(id);
        }
        let name = format!("More actions for {}", cue_label(cue));
        m.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
        egui::Popup::menu(&m).show(|ui| context_menu(ui, ctx, row, sel, edit, out));
    }
    if drag_started {
        let ids: Vec<u64> = if sel.contains(&id) {
            ctx.order
                .iter()
                .copied()
                .filter(|c| sel.contains(c))
                .collect()
        } else {
            *sel = vec![id];
            vec![id]
        };
        egui::DragAndDrop::set_payload(ui.ctx(), DragCues(ids));
    }
    resp.context_menu(|ui| context_menu(ui, ctx, row, sel, edit, out));
    resp.widget_info(|| WidgetInfo::selected(Role::Row, true, selected, &label));
    let hint = if in_cell(cols.target) {
        match (error, &cue.kind) {
            (Some(e), _) => Some(format!("Error: {e}")),
            (None, CueKind::Audio(a)) if !a.file.is_empty() => Some(a.file.clone()),
            _ => None,
        }
    } else if in_cell(cols.cont) {
        match cue.continue_mode {
            ContinueMode::DoNotContinue => None,
            ContinueMode::AutoContinue => {
                Some("Auto-continue: the next cue starts after the post-wait".into())
            }
            ContinueMode::AutoFollow => {
                Some("Auto-follow: the next cue starts when this one has finished".into())
            }
        }
    } else if in_cell(cols.status) {
        match rs {
            RunState::Error => error.map(|e| format!("Error: {e}")),
            RunState::Running => Some("Running".into()),
            RunState::PreWait => Some("Pre-wait".into()),
            RunState::PostWait => Some("Post-wait".into()),
            RunState::Paused => Some("Paused".into()),
            RunState::Loaded => Some("Loaded".into()),
            RunState::Idle if !cue.armed => Some("Disarmed: skipped by GO".into()),
            RunState::Idle => None,
        }
    } else {
        None
    };
    if let Some(h) = hint {
        if !egui::DragAndDrop::has_any_payload(ui.ctx()) {
            resp.on_hover_text_at_pointer(h);
        }
    }

    // Gutter: set the playhead (top-level cues only).
    if row.depth == 0 {
        let g = ui.interact(gutter, Id::unique(("edit_gutter", id)), Sense::click());
        if g.hovered() && !is_playhead {
            next_tag(
                &painter,
                gutter,
                theme::NEXT.gamma_multiply(0.25),
                theme::NEXT,
            );
        }
        let name = format!("Set playhead to {}", cue_label(cue));
        if g.clicked() {
            out.push(Out::Cmd(
                json!({ "cmd": "set_playhead", "cue": id, "list": ctx.list_id }),
            ));
        }
        g.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
        g.on_hover_text("Set as next cue (playhead)");
    }

    // Group twisty
    if let Some(t) = twisty {
        let open = !edit.table.collapsed.contains(&id);
        let tr = ui.interact(t, Id::unique(("edit_twisty", id)), Sense::click());
        twisty_glyph(
            &painter,
            t.center(),
            open,
            if tr.hovered() { fg } else { muted },
        );
        if tr.clicked() && !edit.table.collapsed.remove(&id) {
            edit.table.collapsed.insert(id);
        }
        let name = format!(
            "{} group {}",
            if open { "Collapse" } else { "Expand" },
            cue_label(cue)
        );
        tr.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
    }

    // Armed checkbox (hidden on compact tables: the inspector has it)
    if !visible(cols.armed) {
        inline_editor(ui, ctx, cue, rect, name_left, name_cell, inline, edit, out);
        return;
    }
    let armed_cell = cell(cols.armed, rect);
    let boxr = Rect::from_center_size(armed_cell.center(), vec2(16.0, 16.0));
    // Touch: the whole cell is the target.
    let hit = if ctx.touch {
        armed_cell
    } else {
        boxr.expand(4.0)
    };
    let ar = ui.interact(hit, Id::unique(("edit_armed", id)), Sense::click());
    checkbox_glyph(&painter, boxr, cue.armed, ar.hovered());
    if ar.clicked() {
        out.push(Out::Cmd(json!({
            "cmd": "update_cue", "cue": id, "patch": { "armed": !cue.armed },
        })));
    }
    let name = format!("Armed {}", cue_label(cue));
    ar.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, cue.armed, &name));
    inline_editor(ui, ctx, cue, rect, name_left, name_cell, inline, edit, out);
}

/// Inline editor of the number or name cell (double-click / "Rename").
#[allow(clippy::too_many_arguments)]
fn inline_editor(
    ui: &mut Ui,
    ctx: &RowCtx<'_>,
    cue: &Cue,
    rect: Rect,
    name_left: f32,
    name_cell: Rect,
    inline: Option<InlineField>,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let id = cue.id;
    let cols = ctx.cols;
    if let Some(field) = inline {
        let (x, current, what) = match field {
            InlineField::Number => (cols.number, cue.number.as_str(), "number"),
            InlineField::Name => (
                Rangef::new(name_left, name_cell.right()),
                cue.name.as_str(),
                "name",
            ),
        };
        let r = Rect::from_x_y_ranges(x, rect.y_range()).shrink2(vec2(2.0, 3.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(r));
        let fid = Id::unique(("edit_inline", id, what));
        let resp = fields::field(
            &mut child,
            &mut edit.drafts,
            fid,
            current,
            fields::Opts::new(
                if field == InlineField::Number {
                    "Inline cue number"
                } else {
                    "Inline cue name"
                },
                r.width(),
            ),
            out,
            move |t| {
                let v = if what == "number" { t.trim() } else { t };
                Ok(Some(
                    json!({ "cmd": "update_cue", "cue": id, "patch": { what: v } }),
                ))
            },
        );
        if let Some(e) = edit.table.inline.as_mut() {
            if !e.opened {
                e.opened = true;
                resp.request_focus();
                fields::select_all(ui.ctx(), fid, current);
            } else if resp.lost_focus() || !resp.has_focus() {
                edit.table.inline = None;
            }
        }
    }
}

fn next_tag(painter: &Painter, gutter: Rect, fill: Color32, text: Color32) {
    let r = Rect::from_center_size(gutter.center() + vec2(2.0, 0.0), vec2(42.0, 18.0));
    painter.rect_filled(r, 4.0, fill);
    painter.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        "NEXT",
        egui::FontId::proportional(11.0),
        text,
    );
}

fn status_icon(painter: &Painter, c: Pos2, rs: RunState) {
    match rs {
        RunState::Idle => {}
        RunState::Running => {
            painter.add(Shape::convex_polygon(
                vec![
                    c + vec2(-4.0, -6.0),
                    c + vec2(6.0, 0.0),
                    c + vec2(-4.0, 6.0),
                ],
                theme::RUNNING,
                Stroke::NONE,
            ));
        }
        RunState::PreWait | RunState::PostWait => {
            painter.circle_stroke(c, 5.5, Stroke::new(2.0, theme::RUNNING));
            painter.line_segment([c, c + vec2(0.0, -3.5)], Stroke::new(1.5, theme::RUNNING));
        }
        RunState::Paused => {
            for dx in [-3.0, 3.0] {
                painter.rect_filled(
                    Rect::from_center_size(c + vec2(dx, 0.0), vec2(3.0, 11.0)),
                    1.0,
                    theme::PAUSED,
                );
            }
        }
        RunState::Error => {
            painter.circle_filled(c, 7.0, theme::ERROR);
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                "!",
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
        RunState::Loaded => {
            painter.circle_filled(c, 5.0, theme::LOADED);
        }
    }
}

fn continue_glyph(painter: &Painter, c: Pos2, mode: ContinueMode, color: Color32) {
    if mode == ContinueMode::DoNotContinue {
        return;
    }
    let s = Stroke::new(1.6, color);
    painter.line_segment([c + vec2(0.0, -7.0), c + vec2(0.0, 5.0)], s);
    painter.add(Shape::convex_polygon(
        vec![c + vec2(-4.0, 2.0), c + vec2(4.0, 2.0), c + vec2(0.0, 7.0)],
        color,
        Stroke::NONE,
    ));
    if mode == ContinueMode::AutoFollow {
        painter.line_segment([c + vec2(-5.0, 9.0), c + vec2(5.0, 9.0)], s);
    }
}

fn twisty_glyph(painter: &Painter, c: Pos2, open: bool, color: Color32) {
    let pts = if open {
        vec![
            c + vec2(-5.0, -3.0),
            c + vec2(5.0, -3.0),
            c + vec2(0.0, 4.0),
        ]
    } else {
        vec![
            c + vec2(-3.0, -5.0),
            c + vec2(4.0, 0.0),
            c + vec2(-3.0, 5.0),
        ]
    };
    painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

fn checkbox_glyph(painter: &Painter, r: Rect, on: bool, hovered: bool) {
    if on {
        painter.rect_filled(r, 3.0, theme::EDIT);
        let s = Stroke::new(2.0, Color32::WHITE);
        painter.line_segment(
            [
                r.left_center() + vec2(3.5, 0.0),
                r.center_bottom() + vec2(-1.0, -4.0),
            ],
            s,
        );
        painter.line_segment(
            [
                r.center_bottom() + vec2(-1.0, -4.0),
                r.right_top() + vec2(-3.5, 4.0),
            ],
            s,
        );
    } else {
        let c = if hovered {
            Color32::from_gray(200)
        } else {
            Color32::from_gray(110)
        };
        painter.rect_stroke(r, 3.0, Stroke::new(1.5, c), egui::StrokeKind::Inside);
    }
}

fn context_menu(
    ui: &mut Ui,
    ctx: &RowCtx<'_>,
    row: &Row<'_>,
    sel: &mut Vec<u64>,
    edit: &mut EditState,
    out: &mut Vec<Out>,
) {
    let id = row.cue.id;
    let ids: Vec<u64> = ctx
        .order
        .iter()
        .copied()
        .filter(|c| sel.contains(c))
        .collect();
    let ids = if ids.is_empty() { vec![id] } else { ids };
    let n = ids.len();
    let plural = if n == 1 {
        String::new()
    } else {
        format!(" {n} cues")
    };
    ui.set_min_width(190.0);
    if ui
        .button("Rename")
        .on_hover_text("Edit the name in the row (or double-click the name)")
        .clicked()
    {
        *sel = vec![id];
        edit.table.inline = Some(Inline {
            cue: id,
            field: InlineField::Name,
            opened: false,
        });
        ui.close();
    }
    if ui
        .button("Preview")
        .on_hover_text("Start this cue without moving the playhead")
        .clicked()
    {
        out.push(Out::Cmd(json!({ "cmd": "start_cue", "cue": id })));
        ui.close();
    }
    if ui.button("Stop").clicked() {
        out.push(Out::Cmd(json!({ "cmd": "stop_cue", "cue": id })));
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(row.depth == 0, egui::Button::new("Set as next"))
        .on_disabled_hover_text("Only top-level cues can be the playhead")
        .clicked()
    {
        out.push(Out::Cmd(
            json!({ "cmd": "set_playhead", "cue": id, "list": ctx.list_id }),
        ));
        ui.close();
    }
    ui.separator();
    let copy_key = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    if ui
        .button(format!("Copy{plural}"))
        .on_hover_text(format!(
            "{copy_key}+C - paste into this or another show with {copy_key}+V"
        ))
        .clicked()
    {
        *sel = ids.clone();
        out.push(Out::Action("copy"));
        ui.close();
    }
    if ui.button("Paste after this").clicked() {
        *sel = vec![id];
        out.push(Out::Action("paste"));
        ui.close();
    }
    super::template_menu(ui, out);
    if ui
        .button(format!("Save{plural} as template…"))
        .on_hover_text("Reuse these cues in other shows: Add cue > Templates")
        .clicked()
    {
        *sel = ids.clone();
        out.push(Out::Action("save_template"));
        ui.close();
    }
    ui.separator();
    if ui.button(format!("Duplicate{plural}")).clicked() {
        out.push(Out::CmdSelect(
            json!({ "cmd": "duplicate_cues", "cues": ids }),
        ));
        ui.close();
    }
    if ui.button(format!("Group{plural}")).clicked() {
        out.push(Out::CmdSelect(json!({ "cmd": "group_cues", "cues": ids })));
        ui.close();
    }
    if ui.button("Renumber…").clicked() {
        let start = row
            .cue
            .number
            .parse::<f64>()
            .map(|_| row.cue.number.clone());
        edit.table.renumber = Some(Renumber {
            cues: ids.clone(),
            start: start.unwrap_or_else(|_| "1".into()),
            step: "1".into(),
        });
        ui.close();
    }
    let all_armed = ids
        .iter()
        .all(|c| ctx.state.show.cue(*c).is_none_or(|c| c.armed));
    if ui
        .button(if all_armed { "Disarm" } else { "Arm" })
        .clicked()
    {
        out.push(Out::Cmd(json!({
            "cmd": "update_cues", "cues": ids, "patch": { "armed": !all_armed },
        })));
        ui.close();
    }
    ui.separator();
    if ui
        .button(RichText::new(format!("Delete{plural}")).color(theme::ERROR))
        .clicked()
    {
        out.push(Out::Cmd(json!({ "cmd": "delete_cues", "cues": ids })));
        sel.clear();
        ui.close();
    }
}

fn renumber_dialog(ctx: &egui::Context, st: &mut TableState, out: &mut Vec<Out>) {
    let Some(r) = st.renumber.as_mut() else {
        return;
    };
    let mut keep = true;
    let resp = egui::Modal::new(Id::unique("edit_renumber")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.heading("Renumber cues");
        ui.label(
            RichText::new(format!("{} cue(s), in list order", r.cues.len())).color(theme::MUTED),
        );
        ui.add_space(6.0);
        egui::Grid::new("renumber_grid")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Start");
                ui.add(egui::TextEdit::singleline(&mut r.start).desired_width(90.0))
                    .accessible_name("Renumber start");
                ui.end_row();
                ui.label("Step");
                ui.add(egui::TextEdit::singleline(&mut r.step).desired_width(90.0))
                    .accessible_name("Renumber step");
                ui.end_row();
            });
        let start = r.start.trim().replace(',', ".").parse::<f64>().ok();
        let step = r
            .step
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|s| *s > 0.0);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                keep = false;
            }
            let ok = start.is_some() && step.is_some();
            if ui.add_enabled(ok, egui::Button::new("Renumber")).clicked() {
                if let (Some(start), Some(step)) = (start, step) {
                    out.push(Out::Cmd(json!({
                        "cmd": "renumber", "cues": r.cues, "start": start, "step": step,
                    })));
                }
                keep = false;
            }
        });
    });
    if resp.should_close() || !keep {
        st.renumber = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sm_core::model::GroupCue;

    fn cue(id: u64, kind: CueKind) -> Cue {
        Cue {
            id,
            kind,
            ..Cue::default()
        }
    }

    fn list() -> CueList {
        let g = cue(
            3,
            CueKind::Group(GroupCue {
                children: vec![cue(4, CueKind::Memo), cue(5, CueKind::Memo)],
                ..GroupCue::default()
            }),
        );
        CueList {
            id: 100,
            cues: vec![
                cue(1, CueKind::Memo),
                cue(2, CueKind::Memo),
                g,
                cue(6, CueKind::Memo),
            ],
            ..CueList::default()
        }
    }

    #[test]
    fn flatten_respects_collapsed_groups() {
        let l = list();
        let mut rows = Vec::new();
        flatten(&l.cues, &HashSet::new(), 0, None, &mut rows);
        let ids: Vec<u64> = rows.iter().map(|r| r.cue.id).collect();
        assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
        assert_eq!(rows[3].depth, 1);
        assert_eq!(rows[3].parent, Some(3));
        let mut rows = Vec::new();
        flatten(&l.cues, &HashSet::from([3]), 0, None, &mut rows);
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn move_commands() {
        let l = list();
        let m = |c: &[u64], t| move_command(&l, c, t);
        let v = m(&[1], DropTarget::Row(6, DropPos::After))
            .unwrap()
            .unwrap();
        assert_eq!(v["index"], 4);
        assert_eq!(v["parent"], Value::Null);
        let v = m(&[6, 1], DropTarget::Row(3, DropPos::Into))
            .unwrap()
            .unwrap();
        assert_eq!(v["cues"], json!([1, 6]));
        assert_eq!(v["parent"], 3);
        assert_eq!(v["index"], 2);
        let v = m(&[1], DropTarget::Row(5, DropPos::Before))
            .unwrap()
            .unwrap();
        assert_eq!(
            (v["parent"].clone(), v["index"].clone()),
            (json!(3), json!(1))
        );
        // children travel with their group
        let v = m(&[3, 4], DropTarget::End).unwrap().unwrap();
        assert_eq!(v["cues"], json!([3]));
        assert!(m(&[3], DropTarget::Row(4, DropPos::Before)).is_err());
        assert_eq!(m(&[3], DropTarget::Row(3, DropPos::Into)), Ok(None));
    }

    #[test]
    fn selection_clicks() {
        let order = [1, 2, 3, 4, 5];
        let (mut sel, mut anchor) = (Vec::new(), None);
        click_select(&mut sel, &mut anchor, &order, 2, Modifiers::NONE);
        assert_eq!(sel, [2]);
        click_select(&mut sel, &mut anchor, &order, 4, Modifiers::SHIFT);
        assert_eq!(sel, [2, 3, 4]);
        click_select(&mut sel, &mut anchor, &order, 3, Modifiers::COMMAND);
        assert_eq!(sel, [2, 4]);
        click_select(&mut sel, &mut anchor, &order, 5, Modifiers::NONE);
        assert_eq!(sel, [5]);
    }
}
