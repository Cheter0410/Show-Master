//! Running in the background: closing the window only hides it, the show
//! (engine, audio, timecode, MIDI, tablet server) keeps running.
//!
//! The window comes back when Show Master is started again (the second start
//! asks the running one via `raise_window`), from the tray icon, or on macOS
//! from the Dock icon / Finder ("reopen" Apple Event). Show Master really
//! quits only through Quit in the window (Cmd+Q on macOS) or the tray menu.
//!
//! Platform hooks (tray menu, macOS app menu and Apple Events) run outside
//! the window's frame loop; they set a flag here and wake the window, which
//! picks the request up in its next `logic` pass (that also runs while the
//! window is hidden).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static SHOW: AtomicBool = AtomicBool::new(false);
static HIDE: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);
/// A tray icon is shown, so a hidden window can always be brought back.
static TRAY: AtomicBool = AtomicBool::new(false);
static REPAINT: Mutex<Option<egui::Context>> = Mutex::new(None);

fn wake() {
    if let Ok(ctx) = REPAINT.lock() {
        if let Some(ctx) = ctx.as_ref() {
            ctx.request_repaint();
        }
    }
}

/// Let the platform hooks wake the window.
pub fn set_repaint_context(ctx: &egui::Context) {
    if let Ok(mut c) = REPAINT.lock() {
        *c = Some(ctx.clone());
    }
}

/// Bring the window back (tray, Dock, second start).
pub fn request_show() {
    SHOW.store(true, Ordering::SeqCst);
    wake();
}

/// Hide the window, keep running (macOS Cmd+W).
pub fn request_hide() {
    HIDE.store(true, Ordering::SeqCst);
    wake();
}

/// Quit Show Master (tray menu, macOS Cmd+Q). Asks first when cues are
/// running, in Live mode or with unsaved changes.
pub fn request_quit() {
    QUIT.store(true, Ordering::SeqCst);
    wake();
}

pub fn take_show() -> bool {
    SHOW.swap(false, Ordering::SeqCst)
}

pub fn take_hide() -> bool {
    HIDE.swap(false, Ordering::SeqCst)
}

pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::SeqCst)
}

pub fn set_tray_available(on: bool) {
    TRAY.store(on, Ordering::SeqCst);
}

pub fn tray_available() -> bool {
    TRAY.load(Ordering::SeqCst)
}

/// Can the window really be hidden here and still be found again?
///
/// macOS: always (Dock icon, starting the app again). Windows: with the tray
/// icon. Linux: with a tray icon and X11 - Wayland has no "hidden" windows.
/// Otherwise closing the window minimizes it instead.
pub fn can_hide() -> bool {
    if cfg!(target_os = "macos") {
        true
    } else if cfg!(target_os = "linux") {
        tray_available() && std::env::var_os("WAYLAND_DISPLAY").is_none()
    } else {
        tray_available()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_taken_once() {
        request_show();
        request_hide();
        assert!(take_show());
        assert!(!take_show());
        assert!(take_hide());
        assert!(!take_hide());
        // QUIT is shared with other tests in this binary; only check the round trip.
        request_quit();
        assert!(take_quit());
    }
}
