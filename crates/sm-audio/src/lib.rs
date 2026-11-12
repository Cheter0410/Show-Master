//! # sm-audio — the Show Master audio engine
//!
//! A playback engine for live shows: many streamed voices, a routing matrix
//! per voice, sample-accurate timestamped commands and no surprises on the
//! audio thread.
//!
//! ## One clock
//!
//! The master clock is the number of sample frames rendered so far
//! ([`Controller::clock`]). Every command carries the frame at which it
//! takes effect; the renderer splits its blocks at command times, so a
//! command for frame `T` acts exactly on output frame `T` regardless of the
//! device buffer size. A time in the past means "now".
//!
//! ## Threading model
//!
//! ```text
//!  control thread            streamer thread            audio thread
//!  ──────────────            ───────────────            ────────────
//!  Controller  ── commands (rtrb SPSC) ───────────────▶ Renderer::render
//!      ▲                                                    │  ▲
//!      │◀─ events + finished voices (rtrb SPSC) ────────────┘  │
//!      │                                                       │
//!      └─ StreamJob (mpsc) ─▶ Streamer ── f32 ring per voice ──┘
//!                             (decode, resample, loop)
//! ```
//!
//! * **Audio thread** — owns the [`Renderer`]. [`Renderer::render`] never
//!   allocates, frees, locks, blocks or performs I/O. Voices are
//!   pre-allocated on the control thread and moved in through the command
//!   queue; when a voice ends, its ring buffer and gain tables travel back
//!   through the event queue and are dropped by
//!   [`Controller::poll_events`]. This is verified by the
//!   `rt_no_alloc` integration test with a counting global allocator.
//! * **Streamer thread** — pulls every [`SampleSource`] (file decoding,
//!   resampling, looping) and keeps each voice's lock-free ring topped up,
//!   round-robin. Sources may block and allocate.
//! * **Control thread** — owns the [`Controller`]; any normal thread. It
//!   never blocks on the audio thread: a full command queue is an error
//!   ([`AudioError::QueueFull`]).
//!
//! ## Signal flow per block
//!
//! voices (routing matrix × voice gain × fade envelopes) → master gain and
//! per-output gains → [`RtGenerator`]s (e.g. LTC out; not affected by the
//! master fader) → meters / clip flags → safety clamp → device.
//!
//! The engine does **not** limit or soft-clip: levels above full scale set
//! the sticky clip flag of the channel ([`Meters::clip`]) and are then
//! clamped to `[-1, 1]` so that no backend ever receives out-of-range or
//! NaN samples ([`EngineConfig::safety_clamp`], on by default; meters
//! always show the unclamped level).
//!
//! All gains move through ramps ([`FadeCurve`]); curves are evaluated every
//! ≤ 64 frames and interpolated linearly per sample in between, so there is
//! no zipper noise.
//!
//! ## Underruns
//!
//! If a voice's ring is empty before its stream ended, the voice outputs
//! silence, does **not** advance (it continues late, losing nothing), the
//! global underrun counter is incremented and one [`AudioEvent::Underrun`]
//! is emitted per episode.
//!
//! ## Example (offline, no device)
//!
//! ```
//! use sm_audio::{engine, EngineConfig, FadeCurve, MemorySource, VoiceParams};
//!
//! let (mut ctl, mut renderer) = engine(EngineConfig::new(48_000, 2));
//! let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
//! let voice = ctl
//!     .prepare_voice(
//!         Box::new(MemorySource::new(tone, 1)),
//!         VoiceParams { gain: 1.0, matrix: vec![(0, 0, 1.0), (0, 1, 1.0)] },
//!     )
//!     .unwrap();
//! while !ctl.is_primed(voice) {
//!     std::thread::yield_now();
//! }
//! ctl.start(voice, 100, 0, FadeCurve::Linear).unwrap();
//! let mut out = vec![0.0f32; 512 * 2];
//! renderer.render(&mut out, 2);
//! assert_eq!(out[99 * 2], 0.0); // silent before frame 100
//! assert!(out[110 * 2] != 0.0); // playing after
//! ```

pub mod backend;
mod controller;
pub mod input;
pub mod priority;
mod ramp;
mod renderer;
pub mod source;
mod streamer;
mod types;

#[cfg(feature = "cpal-backend")]
pub use backend::CpalBackend;
pub use backend::{
    default_backends, AudioBackend, DeviceInfo, NullBackend, OutputStream, StreamInfo,
    StreamRequest,
};
pub use controller::{engine, engine_with_generators, Controller};
pub use renderer::{Renderer, RtGenerator};
pub use source::{
    probe, waveform_peaks, FileSource, FileSourceOpts, MediaInfo, MemorySource, SampleSource,
};
pub use types::{
    AudioError, AudioEvent, EngineConfig, FadeCurve, Meters, VoiceId, VoiceParams,
    MAX_FADE_CROSSPOINTS,
};

/// `SHOWMASTER_NO_AUDIO_DEVICES=1`: no real audio device is listed or opened
/// (only the silent null output). Release builds run the tests this way, so
/// they neither disturb the computer's audio nor print the audio libraries'
/// device probing.
pub fn no_audio_devices() -> bool {
    std::env::var_os("SHOWMASTER_NO_AUDIO_DEVICES").is_some_and(|v| !v.is_empty() && v != "0")
}
