//! Show Master core: cue model, showfile handling and the deterministic cue
//! engine. Nothing in this crate touches hardware, wall-clock time or
//! threads; the engine runs purely on the audio sample clock handed to it,
//! which makes it fully testable with simulated time.

pub mod chase;
pub mod engine;
pub mod http_cue;
pub mod midi;
pub mod midi_in;
pub mod model;
pub mod osc;
pub mod preflight;
pub mod showfile;

pub use sm_timecode as timecode;

/// Convert decibels to linear gain; anything at or below -120 dB is silence.
pub fn db_to_lin(db: f32) -> f32 {
    if db <= -120.0 || db.is_nan() {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}
