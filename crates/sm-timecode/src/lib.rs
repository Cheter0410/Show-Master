//! Timecode primitives for Show Master: frame rates, timecode values, a
//! sample-clock-anchored transport, and (in submodules) LTC and MTC codecs.
//!
//! Everything in this crate is pure computation: no I/O, no clocks, no
//! threads. All time is expressed in audio sample frames so that timecode
//! can never drift against the audio clock.

pub mod ltc;
pub mod mtc;
pub mod mtc_in;
mod queue;
mod types;

pub use ltc::{encode_frame_bits, DecodedLtcFrame, LtcDecoder, LtcEncoder};
pub use mtc::{full_frame_message, MtcGenerator};
pub use mtc_in::{MtcEvent, MtcReceiver};
pub use queue::PENDING_CAPACITY;
pub use types::{FrameRate, TcTransport, Timecode, TimecodeParseError};
