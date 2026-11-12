//! Screen classes for the responsive layout.
//!
//! Breakpoints (logical points of the window's content area):
//!
//! | class    | condition        | typical screen                          |
//! |----------|------------------|-----------------------------------------|
//! | wide     | width >= 1150    | desktop, 13"+ laptop: everything shown  |
//! | compact  | width <  1150    | 1024x768 / 1024x600 show-control screen |
//! | narrow   | width <   900    | 800x600, small tablet in portrait-ish   |
//! | short    | height <  720    | 1024x600, 1280x700 netbook-style panels |
//!
//! Wide screens keep the full layout (all columns, all zones, all panels).
//! Compact hides the less important table columns and collapses toolbars
//! into menus; narrow stacks the live dashboard vertically. Touch mode
//! (see `prefs.rs`) is independent of the size: it only makes targets larger.

/// Below this width the layout is "compact".
pub const COMPACT_W: f32 = 1150.0;
/// Below this width the layout is "narrow".
pub const NARROW_W: f32 = 900.0;
/// Below this height the layout is "short".
pub const SHORT_H: f32 = 720.0;
/// Smallest window the layouts are designed for.
pub const MIN_SIZE: [f32; 2] = [760.0, 540.0];

/// Minimum height of touch targets (buttons, rows, checkboxes, tabs).
pub const TOUCH_TARGET: f32 = 44.0;

/// Size class of the window this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: f32,
    pub height: f32,
    /// Touch-friendly controls are on.
    pub touch: bool,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            width: 1440.0,
            height: 900.0,
            touch: false,
        }
    }
}

impl Screen {
    pub fn new(size: egui::Vec2, touch: bool) -> Self {
        Self {
            width: size.x,
            height: size.y,
            touch,
        }
    }

    pub fn compact(&self) -> bool {
        self.width < COMPACT_W
    }

    pub fn narrow(&self) -> bool {
        self.width < NARROW_W
    }

    pub fn short(&self) -> bool {
        self.height < SHORT_H
    }

    /// The top bar folds File / Save / Undo / Redo into one menu.
    pub fn fold_toolbar(&self) -> bool {
        self.narrow() || (self.compact() && self.touch)
    }

    /// Largest width a dialog may take (screen minus a margin).
    pub fn dialog_width(&self, wanted: f32) -> f32 {
        wanted.min(self.width - 48.0).max(200.0)
    }
}

fn key() -> egui::Id {
    egui::Id::unique("sm_responsive_screen")
}

/// Remember this frame's screen class for code that only has a `Ui`.
pub fn store(ctx: &egui::Context, screen: Screen) {
    ctx.data_mut(|d| d.insert_temp(key(), screen));
}

/// This frame's screen class (wide, no touch when nothing was stored).
pub fn get(ctx: &egui::Context) -> Screen {
    ctx.data(|d| d.get_temp(key())).unwrap_or_default()
}

/// Scroll sources for a scroll area: in touch mode content can always be
/// dragged with a finger (even when the setting forces touch mode on a
/// machine that reported no touch screen).
pub fn scroll_source(ctx: &egui::Context) -> egui::scroll_area::ScrollSource {
    if get(ctx).touch {
        egui::scroll_area::ScrollSource::ALL
    } else {
        egui::scroll_area::ScrollSource::default()
    }
}
