//! I/O layer of Show Master: turns the engine's sample-timestamped
//! [`sm_core::engine::Action`]s into audio (via `sm-audio`), MIDI (system
//! ports via `midir`, network via the built-in RTP-MIDI session), LTC and
//! MTC. Everything platform specific sits behind the traits of `sm-audio`
//! and [`midi::MidiSink`].

pub mod audio;
pub mod clock;
pub mod midi;

mod hub;
pub use hub::{Io, IoStatus};
