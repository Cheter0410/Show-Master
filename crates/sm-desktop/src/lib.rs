//! Native desktop user interface of Show Master (egui).
//!
//! The window is one more client of the runtime's command bus, exactly like
//! the tablet web UI, the Stream Deck API and OSC: it reads the published
//! state and sends commands. Engine and audio run on their own threads, so a
//! slow or busy window never affects playback.

pub mod app;
pub mod client;
pub mod edit;
pub mod live;
pub mod model;
pub mod recent;
pub mod setup;
pub mod theme;
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

/// Open the window and run until it is closed. Must be called on the main thread.
pub fn run(
    handle: sm_server::runtime::Handle,
    rt: tokio::runtime::Handle,
    open: Option<PathBuf>,
) -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Show Master")
        .with_app_id("showmaster")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Show Master",
        options,
        Box::new(move |cc| {
            let client = client::Client::new(handle, rt);
            let mut app = App::new(&cc.egui_ctx, client, open);
            app.recent = recent::Recent::load();
            Ok(Box::new(Window(app)))
        }),
    )
}

struct Window(App);

impl eframe::App for Window {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui_root(ui);
    }
}
