//! Native desktop user interface of Show Master (egui).
//!
//! The window is one more client of the runtime's command bus, exactly like
//! the tablet web UI, the Stream Deck API and OSC: it reads the published
//! state and sends commands. Engine and audio run on their own threads, so a
//! slow or busy window never affects playback.

pub mod app;
pub mod autostart;
pub mod background;
pub mod client;
pub mod clipboard;
pub mod edit;
pub mod global_hotkeys;
pub mod live;
pub mod model;
pub mod open_files;
pub mod prefs;
pub mod recent;
pub mod responsive;
pub mod setup;
pub mod theme;
pub mod tray;
pub mod util;
pub mod widgets;

use std::path::PathBuf;

pub use app::App;

/// Load the application icon.
fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon.png"))
        .ok()?
        .to_rgba8();
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Open the window and run until Show Master quits (closing the window only
/// hides it, see [`background`]). Must be called on the main thread.
pub fn run(
    handle: sm_server::runtime::Handle,
    rt: tokio::runtime::Handle,
    open: Option<PathBuf>,
) -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Show Master")
        .with_app_id("showmaster")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size(responsive::MIN_SIZE)
        .with_drag_and_drop(true);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }
    // Where the window was last time (per machine, desktop.json).
    if let Some(g) = prefs::Prefs::load().window {
        viewport = viewport
            .with_inner_size([g.w, g.h])
            .with_position([g.x, g.y])
            .with_maximized(g.maximized);
    }
    // Early, so a document that launched the app is caught (macOS).
    open_files::install();
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Show Master",
        options,
        Box::new(move |cc| {
            // A show given on the command line is already open in the runtime.
            let has_show = !handle.state()["show_path"].is_null();
            let client = client::Client::new(handle, rt.clone());
            open_files::install();
            open_files::set_repaint_context(&cc.egui_ctx);
            background::set_repaint_context(&cc.egui_ctx);
            open_files::install_menu();
            let prefs = prefs::Prefs::load();
            let recent = recent::Recent::load();
            // "Open the last show at start" (when started without a show).
            let open = open.or_else(|| {
                (prefs.open_last && !has_show)
                    .then(|| recent.existing().next().cloned())
                    .flatten()
            });
            let mut app = App::new(&cc.egui_ctx, client, open);
            app.recent = recent;
            app.prefs = prefs;
            app.global_hotkeys = global_hotkeys::GlobalHotkeys::native();
            let tray = tray::start(app.tablet_address(), &rt);
            Ok(Box::new(Window(app, tray)))
        }),
    )
}

/// Store the window's size and position (only while it is shown normally:
/// a hidden or minimized window reports nothing useful).
fn remember_geometry(app: &mut App, ctx: &egui::Context) {
    // Twice a second is plenty (and avoids writing the file every frame
    // while the window is dragged).
    if ctx.cumulative_pass_nr() % 15 != 0 {
        return;
    }
    let info = ctx.input(|i| i.viewport().clone());
    if info.minimized == Some(true) || info.visible() == Some(false) {
        return;
    }
    let maximized = info.maximized == Some(true);
    let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) else {
        return; // e.g. Wayland: no positions
    };
    let g = if maximized {
        // Keep the size from before maximizing; only remember the flag.
        match app.prefs.window {
            Some(w) => prefs::WindowGeometry {
                maximized: true,
                ..w
            },
            None => return,
        }
    } else {
        prefs::WindowGeometry {
            x: outer.min.x,
            y: outer.min.y,
            w: inner.width(),
            h: inner.height(),
            maximized: false,
        }
    };
    app.prefs.set_window(g);
}

/// The app and its tray icon (kept alive as long as the window loop runs).
struct Window(App, Option<tray::Tray>);

impl eframe::App for Window {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
        if let Some(tray) = self.1.as_mut() {
            tray.set_address(self.0.tablet_address());
        }
        remember_geometry(&mut self.0, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui_root(ui);
    }
}
