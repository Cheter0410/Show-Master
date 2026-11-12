//! Files the operating system asks the app to open (double-click on a
//! `.smshow` file, "Open with", dropping on the Dock icon).
//!
//! On Windows and Linux such files arrive as command-line arguments. On
//! macOS they arrive as an Apple Event ("open documents"); a small handler
//! registered with `NSAppleEventManager` queues them here. The window takes
//! them from the queue every frame.

use std::path::PathBuf;
use std::sync::Mutex;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static REPAINT: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Queue files to open (called from the platform hook).
pub fn push(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    if let Ok(mut p) = PENDING.lock() {
        p.extend(paths);
    }
    if let Ok(ctx) = REPAINT.lock() {
        if let Some(ctx) = ctx.as_ref() {
            ctx.request_repaint();
        }
    }
}

/// Files waiting to be opened.
pub fn take() -> Vec<PathBuf> {
    PENDING
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default()
}

/// Let `push` wake the window.
pub fn set_repaint_context(ctx: &egui::Context) {
    if let Ok(mut c) = REPAINT.lock() {
        *c = Some(ctx.clone());
    }
}

/// Install the platform hook. Call before the window opens and again once it
/// exists (macOS may replace an early handler while it finishes launching).
pub fn install() {
    #[cfg(target_os = "macos")]
    macos::install();
}

#[cfg(target_os = "macos")]
#[path = "open_files_macos.rs"]
mod macos;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_round_trip() {
        push(vec![PathBuf::from("/a.smshow"), PathBuf::from("/b.wav")]);
        push(Vec::new());
        assert_eq!(
            take(),
            vec![PathBuf::from("/a.smshow"), PathBuf::from("/b.wav")]
        );
        assert!(take().is_empty());
    }
}
