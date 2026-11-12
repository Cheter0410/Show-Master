//! macOS: put the output device on the show's sample rate.
//!
//! cpal opens an output stream at any rate the device supports without
//! changing the device: when the device runs at 44.1 kHz and the show at
//! 48 kHz, CoreAudio converts inside the real-time callback and asks for
//! odd, changing block sizes - a known source of crackles and dropouts.
//! Like other show software we switch the device to the show's rate instead
//! (Audio MIDI Setup shows the change); audio files are then converted by
//! our own resampler on the streaming thread, never in the callback.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SYSTEM_OBJECT: u32 = 1;
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const ELEMENT_MAIN: u32 = 0;
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const DEVICES: u32 = fourcc(b"dev#");
const DEVICE_NAME: u32 = fourcc(b"name");
const NOMINAL_RATE: u32 = fourcc(b"nsrt");

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(
        id: u32,
        addr: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        id: u32,
        addr: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        id: u32,
        addr: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
}

fn addr(selector: u32) -> Address {
    Address {
        selector,
        scope: SCOPE_GLOBAL,
        element: ELEMENT_MAIN,
    }
}

fn get_u32(id: u32, selector: u32) -> Option<u32> {
    let mut v: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `v` is a valid u32 out-buffer of `size` bytes.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr(selector),
            0,
            std::ptr::null(),
            &mut size,
            (&mut v as *mut u32).cast(),
        )
    };
    (st == 0).then_some(v)
}

fn rate_of(id: u32) -> Option<f64> {
    let mut v: f64 = 0.0;
    let mut size = std::mem::size_of::<f64>() as u32;
    // SAFETY: `v` is a valid f64 out-buffer of `size` bytes.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr(NOMINAL_RATE),
            0,
            std::ptr::null(),
            &mut size,
            (&mut v as *mut f64).cast(),
        )
    };
    (st == 0 && v > 0.0).then_some(v)
}

fn name_of(id: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` is a valid out-buffer of `size` bytes.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr(DEVICE_NAME),
            0,
            std::ptr::null(),
            &mut size,
            buf.as_mut_ptr().cast(),
        )
    };
    if st != 0 {
        return None;
    }
    let bytes = &buf[..(size as usize).min(buf.len())];
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

fn device_ids() -> Vec<u32> {
    let mut size: u32 = 0;
    // SAFETY: size query only.
    let st = unsafe {
        AudioObjectGetPropertyDataSize(
            SYSTEM_OBJECT,
            &addr(DEVICES),
            0,
            std::ptr::null(),
            &mut size,
        )
    };
    if st != 0 || size == 0 {
        return Vec::new();
    }
    let mut ids = vec![0u32; size as usize / std::mem::size_of::<u32>()];
    // SAFETY: `ids` holds `size` bytes.
    let st = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &addr(DEVICES),
            0,
            std::ptr::null(),
            &mut size,
            ids.as_mut_ptr().cast(),
        )
    };
    if st != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / std::mem::size_of::<u32>());
    ids
}

fn find(device_name: Option<&str>) -> Option<u32> {
    match device_name {
        None => get_u32(SYSTEM_OBJECT, DEFAULT_OUTPUT).filter(|id| *id != 0),
        Some(n) => device_ids()
            .into_iter()
            .find(|id| name_of(*id).as_deref() == Some(n)),
    }
}

/// Outcome of [`match_rate`].
pub(crate) enum RateChange {
    /// The device already ran at the rate.
    Already,
    /// Switched from this rate.
    Switched(u32),
    /// Could not switch; the device keeps running at this rate (CoreAudio
    /// converts).
    Failed(u32),
    /// The device was not found by name (nothing done).
    Unknown,
}

/// Put the output device `device_name` (`None` = default output) on `rate`.
pub(crate) fn match_rate(device_name: Option<&str>, rate: u32) -> RateChange {
    let Some(id) = find(device_name) else {
        return RateChange::Unknown;
    };
    let Some(before) = rate_of(id) else {
        return RateChange::Unknown;
    };
    let want = f64::from(rate);
    if (before - want).abs() < 0.5 {
        return RateChange::Already;
    }
    // SAFETY: `want` is a valid f64 for the duration of the call.
    let st = unsafe {
        AudioObjectSetPropertyData(
            id,
            &addr(NOMINAL_RATE),
            0,
            std::ptr::null(),
            std::mem::size_of::<f64>() as u32,
            (&want as *const f64).cast(),
        )
    };
    let before = before.round() as u32;
    if st != 0 {
        return RateChange::Failed(before);
    }
    // The change is asynchronous: wait (briefly) until the device reports it.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if rate_of(id).is_some_and(|r| (r - want).abs() < 0.5) {
            return RateChange::Switched(before);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    RateChange::Failed(rate_of(id).map_or(before, |r| r.round() as u32))
}
