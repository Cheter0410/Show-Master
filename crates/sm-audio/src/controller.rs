//! The control half of the engine: owned by a normal thread, may allocate.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::renderer::{
    Command, Renderer, RendererParts, RtEvent, RtGenerator, Shared, VoiceData, VoiceShared,
};
use crate::source::SampleSource;
use crate::streamer::{StreamJob, Streamer};
use crate::types::{
    AudioError, AudioEvent, EngineConfig, FadeCurve, Meters, VoiceId, VoiceParams,
    MAX_FADE_CROSSPOINTS,
};

/// Smallest ring buffer, in frames.
const MIN_RING_FRAMES: usize = 8192;

struct VoiceEntry {
    slot: u16,
    shared: Arc<VoiceShared>,
}

/// Control-side handle of the engine. All methods are non-blocking with
/// respect to the audio thread: commands travel through a lock-free queue
/// and a full queue is reported as [`AudioError::QueueFull`].
///
/// Times (`at_frame`) are engine frames as returned by [`Controller::clock`];
/// a time in the past (for example `0`) means "as soon as possible".
pub struct Controller {
    config: EngineConfig,
    shared: Arc<Shared>,
    commands: Producer<Command>,
    events: Consumer<RtEvent>,
    streamer: Option<Streamer>,
    voices: HashMap<u64, VoiceEntry>,
    free_slots: Vec<u16>,
    next_id: u64,
}

/// Create an engine: the [`Controller`] stays with the application, the
/// [`Renderer`] is handed to an [`crate::backend::AudioBackend`] (or driven
/// directly for offline rendering).
pub fn engine(config: EngineConfig) -> (Controller, Renderer) {
    engine_with_generators(config, Vec::new())
}

/// Like [`engine`], with real-time generators (e.g. LTC out) installed in
/// the renderer. They run in the given order, see [`RtGenerator`].
pub fn engine_with_generators(
    mut config: EngineConfig,
    generators: Vec<Box<dyn RtGenerator>>,
) -> (Controller, Renderer) {
    config.sample_rate = config.sample_rate.max(1);
    config.out_channels = config.out_channels.clamp(1, u16::MAX as usize);
    config.max_voices = config.max_voices.clamp(1, 4096);
    config.command_capacity = config.command_capacity.max(64);
    if !config.ring_seconds.is_finite() || config.ring_seconds <= 0.0 {
        config.ring_seconds = 2.0;
    }

    let shared = Arc::new(Shared::new(config.out_channels));
    let (cmd_tx, cmd_rx) = RingBuffer::new(config.command_capacity);
    let (evt_tx, evt_rx) = RingBuffer::new(config.max_voices * 4 + 64);
    let renderer = Renderer::new(RendererParts {
        sample_rate: config.sample_rate,
        out_ch: config.out_channels,
        max_voices: config.max_voices,
        declick: config.declick_frames as u64,
        safety_clamp: config.safety_clamp,
        pending_cap: config.command_capacity,
        shared: Arc::clone(&shared),
        commands: cmd_rx,
        events: evt_tx,
        generators,
    });
    let streamer = match Streamer::spawn() {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::error!("cannot start audio streamer thread: {e}");
            None
        }
    };
    let controller = Controller {
        free_slots: (0..config.max_voices as u16).rev().collect(),
        config,
        shared,
        commands: cmd_tx,
        events: evt_rx,
        streamer,
        voices: HashMap::new(),
        next_id: 1,
    };
    (controller, renderer)
}

impl Controller {
    /// Engine sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// Number of engine output channels.
    pub fn out_channels(&self) -> usize {
        self.config.out_channels
    }

    /// The configuration the engine was built with (after sanitising).
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Master clock: index of the next frame the renderer will produce.
    /// Updated once per audio block.
    pub fn clock(&self) -> u64 {
        self.shared.clock.load(Ordering::Acquire)
    }

    /// Create a voice for `source`. The streamer thread starts pre-filling
    /// its ring immediately; the voice stays silent until [`Self::start`].
    pub fn prepare_voice(
        &mut self,
        source: Box<dyn SampleSource>,
        params: VoiceParams,
    ) -> Result<VoiceId, AudioError> {
        let src_ch = source.channels();
        let out_ch = self.config.out_channels;
        if src_ch == 0 || src_ch > u16::MAX as usize {
            return Err(AudioError::InvalidParam(format!(
                "source has {src_ch} channels"
            )));
        }
        if !params.gain.is_finite() {
            return Err(AudioError::InvalidParam("voice gain is not finite".into()));
        }
        for &(s, o, g) in &params.matrix {
            if s as usize >= src_ch || o as usize >= out_ch || !g.is_finite() {
                return Err(AudioError::InvalidParam(format!(
                    "crosspoint ({s}, {o}, {g}) is outside the {src_ch}x{out_ch} matrix"
                )));
            }
        }
        let Some(streamer) = self.streamer.as_ref() else {
            return Err(AudioError::Backend("streamer thread is not running".into()));
        };
        let Some(&slot) = self.free_slots.last() else {
            return Err(AudioError::NoFreeVoice);
        };

        let identity: Vec<(u16, u16, f32)>;
        let matrix: &[(u16, u16, f32)] = if params.matrix.is_empty() {
            identity = (0..src_ch.min(out_ch) as u16)
                .map(|i| (i, i, 1.0))
                .collect();
            &identity
        } else {
            &params.matrix
        };

        let ring_frames = ((self.config.ring_seconds as f64 * self.config.sample_rate as f64)
            as usize)
            .max(MIN_RING_FRAMES);
        let (producer, consumer) = RingBuffer::<f32>::new(ring_frames * src_ch);
        let vshared = Arc::new(VoiceShared::new());
        let id = self.next_id;
        let data = Box::new(VoiceData::new(
            id,
            src_ch,
            out_ch,
            consumer,
            Arc::clone(&vshared),
            params.gain,
            matrix,
        ));
        self.commands
            .push(Command::AddVoice { slot, data })
            .map_err(|_| AudioError::QueueFull)?;
        self.next_id += 1;
        self.free_slots.pop();
        streamer.add(StreamJob {
            id,
            source,
            producer,
            shared: Arc::clone(&vshared),
            channels: src_ch,
            preroll_frames: self.config.preroll_frames as u64,
        });
        self.voices.insert(
            id,
            VoiceEntry {
                slot,
                shared: vshared,
            },
        );
        Ok(VoiceId(id))
    }

    fn entry(&self, v: VoiceId) -> Result<&VoiceEntry, AudioError> {
        self.voices.get(&v.0).ok_or(AudioError::UnknownVoice(v))
    }

    fn send(&mut self, cmd: Command) -> Result<(), AudioError> {
        self.commands.push(cmd).map_err(|_| AudioError::QueueFull)
    }

    /// True once the voice's ring holds the preroll (or the whole stream).
    /// Starting an unprimed voice is allowed but may underrun.
    pub fn is_primed(&self, v: VoiceId) -> bool {
        self.voices
            .get(&v.0)
            .is_some_and(|e| e.shared.primed.load(Ordering::Acquire))
    }

    /// Frames currently buffered in the voice's ring.
    pub fn buffered_frames(&self, v: VoiceId) -> Option<u64> {
        let e = self.voices.get(&v.0)?;
        let consumed = e.shared.consumed.load(Ordering::Relaxed);
        let written = e.shared.written.load(Ordering::Acquire);
        Some(written.saturating_sub(consumed))
    }

    /// Frames of the voice's stream played so far (engine rate).
    pub fn voice_position(&self, v: VoiceId) -> Option<u64> {
        let e = self.voices.get(&v.0)?;
        Some(e.shared.consumed.load(Ordering::Relaxed))
    }

    /// True when every voice has at least `frames` frames buffered or has
    /// reached the end of its stream. Offline renderers call this before
    /// each block so they never outrun the streamer.
    pub fn all_buffered(&self, frames: u64) -> bool {
        self.voices.values().all(|e| {
            // Order matters: `eos` first, see the renderer.
            let eos = e.shared.eos.load(Ordering::Acquire);
            let consumed = e.shared.consumed.load(Ordering::Relaxed);
            let written = e.shared.written.load(Ordering::Acquire);
            eos || e.shared.cancel.load(Ordering::Relaxed)
                || written.saturating_sub(consumed) >= frames
        })
    }

    /// Start a prepared voice: its first sample lands exactly on output
    /// frame `at_frame`, optionally with a fade-in.
    pub fn start(
        &mut self,
        v: VoiceId,
        at_frame: u64,
        fade_in_frames: u64,
        curve: FadeCurve,
    ) -> Result<(), AudioError> {
        let slot = self.entry(v)?.slot;
        self.send(Command::Start {
            slot,
            id: v.0,
            at: at_frame,
            fade_in: fade_in_frames,
            curve,
        })
    }

    /// Stop a voice at `at_frame`. With `fade_frames == 0` a short declick
    /// ramp (`EngineConfig::declick_frames`) is used; `hard` cuts without
    /// any ramp. The voice ends after the fade ([`AudioEvent::VoiceEnded`]).
    pub fn stop(
        &mut self,
        v: VoiceId,
        at_frame: u64,
        fade_frames: u64,
        curve: FadeCurve,
        hard: bool,
    ) -> Result<(), AudioError> {
        let slot = self.entry(v)?.slot;
        self.send(Command::Stop {
            slot,
            id: v.0,
            at: at_frame,
            fade: fade_frames,
            curve,
            hard,
        })
    }

    /// Pause at `at_frame` (declick ramp, then the position is held).
    pub fn pause(&mut self, v: VoiceId, at_frame: u64) -> Result<(), AudioError> {
        let slot = self.entry(v)?.slot;
        self.send(Command::Pause {
            slot,
            id: v.0,
            at: at_frame,
        })
    }

    /// Resume a paused voice at `at_frame` (declick ramp).
    pub fn resume(&mut self, v: VoiceId, at_frame: u64) -> Result<(), AudioError> {
        let slot = self.entry(v)?.slot;
        self.send(Command::Resume {
            slot,
            id: v.0,
            at: at_frame,
        })
    }

    /// Ramp the voice master gain and / or matrix crosspoints
    /// `(source channel, output channel, target gain)` to new values over
    /// `duration_frames`, starting at `at_frame`.
    pub fn fade(
        &mut self,
        v: VoiceId,
        at_frame: u64,
        duration_frames: u64,
        curve: FadeCurve,
        master: Option<f32>,
        crosspoints: &[(u16, u16, f32)],
    ) -> Result<(), AudioError> {
        let slot = self.entry(v)?.slot;
        if master.is_some_and(|g| !g.is_finite()) || crosspoints.iter().any(|x| !x.2.is_finite()) {
            return Err(AudioError::InvalidParam("fade target is not finite".into()));
        }
        let n_cmds = crosspoints.len().div_ceil(MAX_FADE_CROSSPOINTS).max(1);
        if self.commands.slots() < n_cmds {
            return Err(AudioError::QueueFull);
        }
        let mut master = master;
        let mut chunks = crosspoints.chunks(MAX_FADE_CROSSPOINTS);
        for _ in 0..n_cmds {
            let chunk = chunks.next().unwrap_or(&[]);
            let mut xp = [(0u16, 0u16, 0f32); MAX_FADE_CROSSPOINTS];
            xp[..chunk.len()].copy_from_slice(chunk);
            self.send(Command::Fade {
                slot,
                id: v.0,
                at: at_frame,
                duration: duration_frames,
                curve,
                master: master.take(),
                n_xp: chunk.len() as u8,
                xp,
            })?;
        }
        Ok(())
    }

    /// Stop every started voice (playing, paused or with a start still
    /// scheduled) at `at_frame`. Prepared voices without a scheduled start
    /// are left alone. `hard` cuts instantly (PANIC).
    pub fn stop_all(
        &mut self,
        at_frame: u64,
        fade_frames: u64,
        hard: bool,
    ) -> Result<(), AudioError> {
        self.send(Command::StopAll {
            at: at_frame,
            fade: fade_frames,
            curve: FadeCurve::Linear,
            hard,
        })
    }

    /// Drop a voice immediately without any ramp. Meant for prepared voices
    /// that were never started; on a playing voice it is a hard cut.
    pub fn discard(&mut self, v: VoiceId) -> Result<(), AudioError> {
        self.stop(v, 0, 0, FadeCurve::Linear, true)
    }

    /// Ramp the master output gain to `gain` over `ramp_frames`, now.
    pub fn set_master_gain(&mut self, gain: f32, ramp_frames: u32) -> Result<(), AudioError> {
        self.set_master_gain_at(0, gain, ramp_frames as u64, FadeCurve::Linear)
    }

    /// Timestamped master gain ramp with a curve.
    pub fn set_master_gain_at(
        &mut self,
        at_frame: u64,
        gain: f32,
        ramp_frames: u64,
        curve: FadeCurve,
    ) -> Result<(), AudioError> {
        if !gain.is_finite() {
            return Err(AudioError::InvalidParam("master gain is not finite".into()));
        }
        self.send(Command::MasterGain {
            at: at_frame,
            gain,
            ramp: ramp_frames,
            curve,
        })
    }

    /// Ramp the gain of one output channel (0.0 = mute) over `ramp_frames`.
    pub fn set_output_gain(
        &mut self,
        channel: usize,
        gain: f32,
        ramp_frames: u32,
    ) -> Result<(), AudioError> {
        if channel >= self.config.out_channels || !gain.is_finite() {
            return Err(AudioError::InvalidParam(format!(
                "output gain: channel {channel}, gain {gain}"
            )));
        }
        self.send(Command::OutputGain {
            at: 0,
            channel: channel as u16,
            gain,
            ramp: ramp_frames as u64,
        })
    }

    /// Current output meters.
    pub fn meters(&self) -> Meters {
        let load = |a: &std::sync::atomic::AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        Meters {
            peak: self.shared.peak.iter().map(load).collect(),
            rms: self.shared.rms.iter().map(load).collect(),
            clip: self
                .shared
                .clip
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .collect(),
        }
    }

    /// Clear the sticky clip flags.
    pub fn reset_clip(&self) {
        for c in self.shared.clip.iter() {
            c.store(false, Ordering::Relaxed);
        }
    }

    /// Number of underrun episodes since the engine was created.
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// Number of voices that are started and not yet ended (paused voices
    /// count).
    pub fn active_voices(&self) -> usize {
        self.shared.active.load(Ordering::Relaxed)
    }

    /// Number of voices known to the controller (prepared + started).
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Collect engine events. Must be called regularly (e.g. every UI /
    /// scheduler tick): it also frees the resources of ended voices and
    /// recycles their slots.
    pub fn poll_events(&mut self) -> Vec<AudioEvent> {
        let mut out = Vec::new();
        if let Some(s) = &self.streamer {
            while let Some((id, message)) = s.poll_error() {
                out.push(AudioEvent::SourceError {
                    voice: VoiceId(id),
                    message,
                });
            }
        }
        let mut ended = false;
        while let Ok(ev) = self.events.pop() {
            match ev {
                RtEvent::Ended { slot, data } => {
                    let id = data.id();
                    drop(data); // deferred deallocation of ring + gain tables
                    if self.voices.remove(&id).is_some() {
                        self.free_slots.push(slot);
                    }
                    ended = true;
                    out.push(AudioEvent::VoiceEnded { voice: VoiceId(id) });
                }
                RtEvent::Underrun { id } => out.push(AudioEvent::Underrun { voice: VoiceId(id) }),
                RtEvent::Orphan { data } => {
                    tracing::error!(voice = data.id(), "voice could not be installed");
                    self.voices.remove(&data.id());
                }
            }
        }
        if ended {
            if let Some(s) = &self.streamer {
                s.wake(); // let it drop the finished sources promptly
            }
        }
        out
    }
}
