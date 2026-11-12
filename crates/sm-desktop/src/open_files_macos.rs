//! macOS: receive "open documents" Apple Events (kCoreEventClass /
//! kAEOpenDocuments) and queue the files; "reopen" (kAEReopenApplication:
//! Dock icon clicked, app started again while running) brings the hidden
//! window back. Also installs the app menu: Cmd+W hides the window (the show
//! keeps running), Cmd+Q quits through the same checks as the Quit button.
//!
//! This is the only Objective-C bridge in the app; it needs `unsafe` for the
//! selector registration (see the SAFETY notes).

#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
use std::path::PathBuf;
use std::sync::Mutex;

/// 'aevt' and 'odoc' (Apple Event class / id for "open documents").
const K_CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const K_AE_OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
/// 'rapp' ("reopen application").
const K_AE_REOPEN_APPLICATION: u32 = u32::from_be_bytes(*b"rapp");
/// '----', the direct object (the list of files).
const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[name = "ShowMasterOpenDocumentsHandler"]
    struct OpenHandler;

    impl OpenHandler {
        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            super::push(files(event));
            crate::background::request_show();
        }

        #[unsafe(method(handleReopen:withReplyEvent:))]
        fn handle_reopen(&self, _event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            crate::background::request_show();
        }

        #[unsafe(method(closeWindow:))]
        fn close_window(&self, _sender: Option<&AnyObject>) {
            crate::background::request_hide();
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: Option<&AnyObject>) {
            crate::background::request_quit();
        }
    }
);

/// The file paths in an "open documents" event.
fn files(event: &NSAppleEventDescriptor) -> Vec<PathBuf> {
    let Some(list) = event.paramDescriptorForKeyword(KEY_DIRECT_OBJECT) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // Apple Event lists are 1-based.
    for i in 1..=list.numberOfItems() {
        let Some(item) = list.descriptorAtIndex(i) else {
            continue;
        };
        if let Some(path) = item.fileURLValue().and_then(|url| url.path()) {
            out.push(PathBuf::from(path.to_string()));
        }
    }
    // A single file can also come as the direct object itself.
    if out.is_empty() {
        if let Some(path) = list.fileURLValue().and_then(|url| url.path()) {
            out.push(PathBuf::from(path.to_string()));
        }
    }
    out
}

/// The handler object must stay alive: the event manager does not retain it.
static HANDLER: Mutex<Option<usize>> = Mutex::new(None);

fn handler() -> Option<&'static AnyObject> {
    let mut slot = HANDLER.lock().ok()?;
    Some(match *slot {
        // SAFETY: the pointer comes from `Retained::into_raw` below and is never freed.
        Some(ptr) => unsafe { &*(ptr as *const AnyObject) },
        None => {
            // SAFETY: plain `init` of an NSObject subclass without ivars.
            let obj: Retained<OpenHandler> = unsafe { msg_send![OpenHandler::alloc(), init] };
            let ptr = Retained::into_raw(obj) as usize;
            *slot = Some(ptr);
            // SAFETY: just leaked above, valid for the rest of the process.
            unsafe { &*(ptr as *const AnyObject) }
        }
    })
}

pub fn install() {
    let Some(handler) = handler() else { return };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    // SAFETY: `handler` is an OpenHandler, which implements exactly this
    // selector with the (event, reply) signature the event manager calls.
    unsafe {
        manager.setEventHandler_andSelector_forEventClass_andEventID(
            handler,
            sel!(handleOpenDocuments:withReplyEvent:),
            K_CORE_EVENT_CLASS,
            K_AE_OPEN_DOCUMENTS,
        );
        manager.setEventHandler_andSelector_forEventClass_andEventID(
            handler,
            sel!(handleReopen:withReplyEvent:),
            K_CORE_EVENT_CLASS,
            K_AE_REOPEN_APPLICATION,
        );
    }
}

/// Replace the default app menu (whose Quit would end the process at once,
/// without asking): "Close Window" (Cmd+W) hides, "Quit Show Master" (Cmd+Q)
/// quits with the usual checks. Call on the main thread after launch.
pub fn install_menu() {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
    use objc2_foundation::NSString;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(handler) = handler() else { return };
    let item = |title: &str, action: objc2::runtime::Sel, key: &str| {
        // SAFETY: plain initialisation of a menu item; the action selector is
        // implemented by `handler`, which lives for the rest of the process.
        unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            );
            item.setTarget(Some(handler));
            item
        }
    };
    let app_menu = NSMenu::new(mtm);
    app_menu.addItem(&item("Close Window", sel!(closeWindow:), "w"));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    app_menu.addItem(&item("Quit Show Master", sel!(quitApp:), "q"));
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&app_menu));
    let bar = NSMenu::new(mtm);
    bar.addItem(&app_item);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
}
