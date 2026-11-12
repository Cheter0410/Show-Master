//! macOS: receive "open documents" Apple Events (kCoreEventClass /
//! kAEOpenDocuments) and queue the files.
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

pub fn install() {
    let Ok(mut slot) = HANDLER.lock() else { return };
    let handler: &AnyObject = match *slot {
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
    };
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
    }
}
