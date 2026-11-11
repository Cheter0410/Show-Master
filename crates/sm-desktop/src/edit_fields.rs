//! Text fields that commit on Enter or when they lose focus, never per
//! keystroke. While a field is being edited its text lives here as a draft;
//! a draft whose field disappears (other cue selected, tab switched) is
//! committed at the end of the frame, like a blur in a browser.

use super::Out;
use crate::theme;
use egui::{Id, Key, TextEdit, TextStyle, Ui};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Turns the field text into the command to send: `Ok(None)` = nothing to
/// do, `Err` = invalid input (shown red while typing, toast on commit).
pub type MakeCmd = Box<dyn Fn(&str) -> Result<Option<Value>, String>>;

struct Draft {
    text: String,
    seen: bool,
    make: MakeCmd,
}

#[derive(Default)]
pub struct Drafts {
    map: HashMap<Id, Draft>,
    /// Fields left with Escape: their focus loss must not commit.
    cancelled: HashSet<Id>,
}

impl Drafts {
    pub fn begin_frame(&mut self) {
        for d in self.map.values_mut() {
            d.seen = false;
        }
    }

    /// Commit drafts whose field was not drawn this frame.
    pub fn end_frame(&mut self, out: &mut Vec<Out>) {
        if self.map.values().all(|d| d.seen) {
            return;
        }
        let stale: Vec<Id> = self
            .map
            .iter()
            .filter(|(_, d)| !d.seen)
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(d) = self.map.remove(&id) {
                commit(&d.make, &d.text, out);
            }
        }
    }
}

fn commit(make: &MakeCmd, text: &str, out: &mut Vec<Out>) {
    match make(text) {
        Ok(Some(cmd)) => out.push(Out::Cmd(cmd)),
        Ok(None) => {}
        Err(e) => out.push(Out::Error(e)),
    }
}

#[derive(Clone, Copy)]
pub struct Opts<'a> {
    /// Accessibility name (screen readers, tests).
    pub name: &'a str,
    pub width: f32,
    pub hint: &'a str,
    pub mono: bool,
    pub multiline: bool,
}

impl<'a> Opts<'a> {
    pub fn new(name: &'a str, width: f32) -> Self {
        Self {
            name,
            width,
            hint: "",
            mono: false,
            multiline: false,
        }
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
}

/// A text field showing `current`; on commit `make` builds the command.
pub fn field(
    ui: &mut Ui,
    drafts: &mut Drafts,
    id: Id,
    current: &str,
    o: Opts<'_>,
    out: &mut Vec<Out>,
    make: impl Fn(&str) -> Result<Option<Value>, String> + 'static,
) -> egui::Response {
    // Escape cancels. egui drops the focus on Escape before any widget runs
    // (and the shell may consume the key as a hotkey), so look at the raw
    // events; a field that still has focus with Escape pressed is cancelled too.
    let escape = escape_pressed(ui);
    let had_focus = ui.memory(|m| m.has_focus(id));
    if had_focus && escape {
        drafts.map.remove(&id);
        drafts.cancelled.insert(id);
    }
    let editing = drafts.map.get_mut(&id);
    let has_draft = editing.is_some();
    let mut text = match editing {
        Some(d) => {
            d.seen = true;
            d.text.clone()
        }
        None => current.to_owned(),
    };
    let error = if has_draft { make(&text).err() } else { None };
    let invalid = error.is_some();
    let mut te = if o.multiline {
        TextEdit::multiline(&mut text).desired_rows(4)
    } else {
        TextEdit::singleline(&mut text)
    }
    .id(id)
    .desired_width(o.width)
    .hint_text(o.hint);
    if o.mono {
        te = te.font(TextStyle::Monospace);
    }
    if invalid {
        te = te.text_color(theme::ERROR);
    }
    let mut resp = ui.add(te).accessible_name(o.name);
    if resp.lost_focus() {
        drafts.map.remove(&id);
        let cancelled = drafts.cancelled.remove(&id) || escape;
        if !cancelled && text != current {
            commit(&(Box::new(make) as MakeCmd), &text, out);
        }
    } else if resp.has_focus() {
        drafts.cancelled.remove(&id);
        if has_draft || text != current {
            drafts.map.insert(
                id,
                Draft {
                    text,
                    seen: true,
                    make: Box::new(make),
                },
            );
        }
    }
    if let Some(e) = error.filter(|e| !e.is_empty()) {
        resp = resp.on_hover_text(e);
    }
    resp
}

fn escape_pressed(ui: &Ui) -> bool {
    ui.input(|i| {
        i.raw.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                }
            )
        })
    })
}

/// Put the cursor over the whole text of a field (when it opens for editing).
pub fn select_all(ctx: &egui::Context, id: Id, text: &str) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::select_all_str(text)));
    state.store(ctx, id);
}
