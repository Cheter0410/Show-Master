//! I/O layer of Show Master: turns the engine's sample-timestamped
//! [`sm_core::engine::Action`]s into audio (via `sm-audio`), MIDI (system
//! ports via `midir`, network via the built-in RTP-MIDI session), LTC and
//! MTC. Everything platform specific sits behind the traits of `sm-audio`
//! and [`midi::MidiSink`].

pub mod audio;
pub mod clock;
pub mod http_out;
pub mod inputs;
pub mod midi;
pub mod midi_in;
pub mod tc_in;

mod hub;
pub use hub::{Io, IoStatus};
