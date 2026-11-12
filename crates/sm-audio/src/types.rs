//! Plain data types shared by the control and real-time halves.

use serde::{Deserialize, Serialize};

/// Maximum number of crosspoints carried by one real-time fade command.
/// [`crate::Controller::fade`] transparently splits longer lists into several
/// commands with the same timestamp.
pub const MAX_FADE_CROSSPOINTS: usize = 16;

/// Handle of one voice (one playing or prepared sound). Never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VoiceId(pub u64);

/// Shape of a gain ramp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FadeCurve {
    /// Straight line in linear gain.
    #[default]
    Linear,
    /// Sine / cosine law: the midpoint of a 0 ↔ 1 fade is 0.707 (−3 dB).
    EqualPower,
    /// Raised-cosine S shape (slow start, slow end).
    SCurve,
}

impl FadeCurve {
    /// Normalised progress `t` in `[0, 1]` mapped to the interpolation factor
    /// between `from` and `to`. `rising` selects the mirrored equal-power law.
    #[inline]
    pub(crate) fn shape(self, t: f32, rising: bool) -> f32 {
        match self {
            FadeCurve::Linear => t,
            FadeCurve::EqualPower => {
                if rising {
                    (t * std::f32::consts::FRAC_PI_2).sin()
                } else {
                    1.0 - (t * std::f32::consts::FRAC_PI_2).cos()
                }
            }
            FadeCurve::SCurve => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
        }
    }
}

/// Engine construction parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// Engine (= device) sample rate in Hz. All sources deliver at this rate.
    pub sample_rate: u32,
    /// Number of engine output channels (width of every routing matrix).
    pub out_channels: usize,
    /// Size of the fixed voice table.
    pub max_voices: usize,
    /// Length of the anti-click ramp used for pause / resume and for stops
    /// with a fade of 0 frames (≈ 3 ms by default).
    pub declick_frames: u32,
    /// Capacity of each voice's stream ring buffer in seconds.
    pub ring_seconds: f32,
    /// A voice is "primed" once this many frames are buffered (or the whole
    /// stream, if shorter). Clamped to the ring capacity.
    pub preroll_frames: u32,
    /// Capacity of the control → RT command queue and of the RT-side list of
    /// commands scheduled for the future.
    pub command_capacity: usize,
    /// Clamp the final output to `[-1, 1]` (after generators, before the
    /// device). The clip flags are raised either way.
    pub safety_clamp: bool,
}

impl EngineConfig {
    /// Defaults for the given rate and channel count: 128 voices, 3 ms
    /// declick, 2 s rings, 0.5 s preroll.
    pub fn new(sample_rate: u32, out_channels: usize) -> Self {
        Self {
            sample_rate,
            out_channels,
            max_voices: 128,
            declick_frames: (sample_rate as u64 * 3 / 1000) as u32,
            ring_seconds: 4.0,
            preroll_frames: sample_rate / 2,
            command_capacity: 1024,
            safety_clamp: true,
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self::new(48_000, 2)
    }
}

/// Initial gains of a voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceParams {
    /// Voice master gain (linear).
    pub gain: f32,
    /// Routing matrix crosspoints `(source channel, output channel, linear gain)`.
    /// Crosspoints not listed are 0. An **empty** list means the identity
    /// routing: source channel `i` → output channel `i` at unity.
    pub matrix: Vec<(u16, u16, f32)>,
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            gain: 1.0,
            matrix: Vec::new(),
        }
    }
}

/// Snapshot of the output meters (one entry per engine output channel).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Meters {
    /// Decaying peak (linear, ~20 dB/s release).
    pub peak: Vec<f32>,
    /// RMS with a ~300 ms integration time (linear).
    pub rms: Vec<f32>,
    /// Sticky clip flag: a sample reached |x| ≥ 1.0 since the last reset.
    pub clip: Vec<bool>,
}

/// Notifications from the engine, returned by [`crate::Controller::poll_events`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AudioEvent {
    /// The voice finished (end of stream, stop, discard or stop-all). Its id
    /// is no longer valid.
    VoiceEnded { voice: VoiceId },
    /// The voice's ring ran dry while its stream had not ended. Emitted once
    /// per underrun episode.
    Underrun { voice: VoiceId },
    /// The source failed mid-stream; the stream was ended early.
    SourceError { voice: VoiceId, message: String },
}

/// Errors of the audio crate.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The command queue to the real-time thread is full; retry later.
    #[error("audio command queue is full")]
    QueueFull,
    /// All voice slots are in use.
    #[error("no free voice slot (max_voices reached)")]
    NoFreeVoice,
    /// The voice id is unknown or the voice has already ended.
    #[error("unknown voice {0:?}")]
    UnknownVoice(VoiceId),
    /// A parameter is out of range.
    #[error("invalid parameter: {0}")]
    InvalidParam(String),
    /// File system error.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The media could not be decoded.
    #[error("decode error: {0}")]
    Decode(String),
    /// Format / configuration not supported.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The requested device does not exist.
    #[error("audio device not found: {0}")]
    DeviceNotFound(String),
    /// Device / host failure.
    #[error("audio backend error: {0}")]
    Backend(String),
}
