//! The real-time half of the engine.
//!
//! Everything in this file that is reachable from [`Renderer::render`] obeys
//! the real-time rules: no allocation, no deallocation, no locks, no I/O, no
//! system calls and bounded work per call. Resources that must be freed
//! (a finished voice's ring buffer and gain tables) are sent back to the
//! control side through the event queue and dropped there.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError};

use crate::ramp::Ramp;
use crate::types::{FadeCurve, MAX_FADE_CROSSPOINTS};

/// Granularity at which gain curves are evaluated. Inside one sub-block the
/// gain is interpolated linearly per sample.
pub(crate) const SUB_BLOCK: usize = 64;

/// Peak meter release, in nepers per second (= 20 dB/s).
const PEAK_DECAY_NEPER_PER_S: f32 = std::f32::consts::LN_10;
/// RMS meter integration time in seconds.
const RMS_TAU_S: f32 = 0.3;

/// A signal generator that runs inside the audio callback (for example LTC
/// output on a dedicated channel).
///
/// Generators are installed at engine construction and are called for every
/// rendered segment **after** voice mixing and master / output gains and
/// **before** metering and the safety clamp. They may add to or overwrite
/// any channel; the master fader does not affect them.
///
/// `out` is interleaved with `channels` engine output channels and holds
/// `out.len() / channels` frames (any length ≥ 1, usually ≤ 64);
/// `start_frame` is the engine clock of its first frame. Implementations
/// must follow the real-time rules (no allocation, locks, I/O or panics).
pub trait RtGenerator: Send {
    /// Render one segment starting at engine frame `start_frame`.
    fn render(&mut self, start_frame: u64, out: &mut [f32], channels: usize);
}

/// State shared between the renderer, the controller and backends.
pub(crate) struct Shared {
    pub(crate) clock: AtomicU64,
    pub(crate) underruns: AtomicU64,
    pub(crate) active: AtomicUsize,
    pub(crate) peak: Box<[AtomicU32]>,
    pub(crate) rms: Box<[AtomicU32]>,
    pub(crate) clip: Box<[AtomicBool]>,
}

impl Shared {
    pub(crate) fn new(out_channels: usize) -> Self {
        Self {
            clock: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            active: AtomicUsize::new(0),
            peak: (0..out_channels).map(|_| AtomicU32::new(0)).collect(),
            rms: (0..out_channels).map(|_| AtomicU32::new(0)).collect(),
            clip: (0..out_channels).map(|_| AtomicBool::new(false)).collect(),
        }
    }
}

/// Per-voice state shared by the streamer, the renderer and the controller.
pub(crate) struct VoiceShared {
    /// Set by the streamer after the last frame was pushed into the ring.
    pub(crate) eos: AtomicBool,
    /// Set when the voice is finished: the streamer stops filling and drops
    /// the source.
    pub(crate) cancel: AtomicBool,
    /// Set by the streamer once the preroll is buffered.
    pub(crate) primed: AtomicBool,
    /// Frames pushed into the ring so far (streamer).
    pub(crate) written: AtomicU64,
    /// Frames consumed from the ring so far (renderer).
    pub(crate) consumed: AtomicU64,
}

impl VoiceShared {
    pub(crate) fn new() -> Self {
        Self {
            eos: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            primed: AtomicBool::new(false),
            written: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceState {
    Prepared,
    Playing,
    Finished,
}

/// Everything the renderer needs for one voice. Allocated by the controller,
/// moved to the renderer inside a command and moved back inside an event.
pub(crate) struct VoiceData {
    id: u64,
    src_ch: usize,
    consumer: Consumer<f32>,
    shared: Arc<VoiceShared>,
    /// Routing matrix, index `src * out_channels + out`.
    xp: Box<[Ramp]>,
    /// Crosspoints that are (or have ever been) non-zero, as `(src, out)`.
    /// Only these are visited while mixing; capacity is the whole matrix,
    /// so adding one in the audio thread never allocates.
    xp_active: Box<[(u16, u16)]>,
    xp_active_len: usize,
    xp_listed: Box<[bool]>,
    master: Ramp,
    /// Start fade-in / stop fade-out envelope.
    env: Ramp,
    /// Pause / resume declick envelope.
    pause: Ramp,
    state: VoiceState,
    stopping: bool,
    pausing: bool,
    /// Paused and fully ramped down: no frames are consumed.
    held: bool,
    in_underrun: bool,
    underrun_reported: bool,
    /// Upper bound of the frames any crosspoint ramp still has to run.
    xp_ramp_left: u64,
    consumed: u64,
}

impl VoiceData {
    pub(crate) fn new(
        id: u64,
        src_ch: usize,
        out_ch: usize,
        consumer: Consumer<f32>,
        shared: Arc<VoiceShared>,
        gain: f32,
        matrix: &[(u16, u16, f32)],
    ) -> Self {
        let mut this = Self {
            id,
            src_ch,
            consumer,
            shared,
            xp: vec![Ramp::constant(0.0); src_ch * out_ch].into_boxed_slice(),
            xp_active: vec![(0u16, 0u16); src_ch * out_ch].into_boxed_slice(),
            xp_active_len: 0,
            xp_listed: vec![false; src_ch * out_ch].into_boxed_slice(),
            master: Ramp::constant(gain),
            env: Ramp::constant(1.0),
            pause: Ramp::constant(1.0),
            state: VoiceState::Prepared,
            stopping: false,
            pausing: false,
            held: false,
            in_underrun: false,
            underrun_reported: false,
            xp_ramp_left: 0,
            consumed: 0,
        };
        for &(s, o, g) in matrix {
            this.set_crosspoint(out_ch, s, o, g, 0, FadeCurve::Linear);
        }
        this
    }

    /// Ramp one crosspoint; out-of-range indices are ignored.
    fn set_crosspoint(&mut self, oc: usize, s: u16, o: u16, gain: f32, len: u64, curve: FadeCurve) {
        if (s as usize) >= self.src_ch || (o as usize) >= oc {
            return;
        }
        let idx = s as usize * oc + o as usize;
        self.xp[idx].set(gain, len, curve);
        self.xp_ramp_left = self.xp_ramp_left.max(len);
        if !self.xp_listed[idx] && self.xp_active_len < self.xp_active.len() {
            self.xp_listed[idx] = true;
            self.xp_active[self.xp_active_len] = (s, o);
            self.xp_active_len += 1;
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    fn finish(&mut self, shared: &Shared) {
        if self.state == VoiceState::Playing {
            shared.active.fetch_sub(1, Ordering::Relaxed);
        }
        self.state = VoiceState::Finished;
        self.shared.cancel.store(true, Ordering::Release);
    }

    fn stop(&mut self, shared: &Shared, fade: u64, curve: FadeCurve, hard: bool, declick: u64) {
        match self.state {
            VoiceState::Finished => {}
            VoiceState::Prepared => self.finish(shared),
            VoiceState::Playing => {
                let (fade, curve) = if fade == 0 {
                    (declick, FadeCurve::Linear)
                } else {
                    (fade, curve)
                };
                if hard || self.held || fade == 0 {
                    self.finish(shared);
                } else {
                    self.env.set(0.0, fade, curve);
                    self.stopping = true;
                }
            }
        }
    }
}

/// Control → RT commands. `at` is the engine frame at which the command
/// takes effect; times in the past mean "now".
pub(crate) enum Command {
    AddVoice {
        slot: u16,
        data: Box<VoiceData>,
    },
    Start {
        slot: u16,
        id: u64,
        at: u64,
        fade_in: u64,
        curve: FadeCurve,
    },
    Stop {
        slot: u16,
        id: u64,
        at: u64,
        fade: u64,
        curve: FadeCurve,
        hard: bool,
    },
    Pause {
        slot: u16,
        id: u64,
        at: u64,
    },
    Resume {
        slot: u16,
        id: u64,
        at: u64,
    },
    Fade {
        slot: u16,
        id: u64,
        at: u64,
        duration: u64,
        curve: FadeCurve,
        master: Option<f32>,
        n_xp: u8,
        xp: [(u16, u16, f32); MAX_FADE_CROSSPOINTS],
    },
    StopAll {
        at: u64,
        fade: u64,
        curve: FadeCurve,
        hard: bool,
    },
    MasterGain {
        at: u64,
        gain: f32,
        ramp: u64,
        curve: FadeCurve,
    },
    OutputGain {
        at: u64,
        channel: u16,
        gain: f32,
        ramp: u64,
    },
}

impl Command {
    fn at(&self) -> u64 {
        match self {
            Command::AddVoice { .. } => 0,
            Command::Start { at, .. }
            | Command::Stop { at, .. }
            | Command::Pause { at, .. }
            | Command::Resume { at, .. }
            | Command::Fade { at, .. }
            | Command::StopAll { at, .. }
            | Command::MasterGain { at, .. }
            | Command::OutputGain { at, .. } => *at,
        }
    }
}

/// RT → control events.
pub(crate) enum RtEvent {
    /// The voice is finished; its resources come back for deallocation.
    Ended { slot: u16, data: Box<VoiceData> },
    /// Underrun episode started on this voice.
    Underrun { id: u64 },
    /// A voice that could not be installed (never happens unless the
    /// controller's slot bookkeeping is broken); just drop it.
    Orphan { data: Box<VoiceData> },
}

/// The real-time renderer. Owned by the audio callback (or by a test /
/// offline bounce loop, which simply calls [`Renderer::render`]).
pub struct Renderer {
    sample_rate: u32,
    out_ch: usize,
    declick: u64,
    safety_clamp: bool,
    clock: u64,
    shared: Arc<Shared>,
    commands: Consumer<Command>,
    events: Producer<RtEvent>,
    /// Commands waiting for their time, sorted by `at` (stable).
    pending: VecDeque<Command>,
    pending_cap: usize,
    slots: Box<[Option<Box<VoiceData>>]>,
    master: Ramp,
    out_gain: Box<[Ramp]>,
    out_gain_ramp_left: u64,
    generators: Vec<Box<dyn RtGenerator>>,
    /// Interleaved scratch bus, `SUB_BLOCK * out_ch` samples.
    mix: Box<[f32]>,
    blk_peak: Box<[f32]>,
    blk_sq: Box<[f32]>,
    blk_clip: Box<[bool]>,
    peak_state: Box<[f32]>,
    ms_state: Box<[f32]>,
}

pub(crate) struct RendererParts {
    pub(crate) sample_rate: u32,
    pub(crate) out_ch: usize,
    pub(crate) max_voices: usize,
    pub(crate) declick: u64,
    pub(crate) safety_clamp: bool,
    pub(crate) pending_cap: usize,
    pub(crate) shared: Arc<Shared>,
    pub(crate) commands: Consumer<Command>,
    pub(crate) events: Producer<RtEvent>,
    pub(crate) generators: Vec<Box<dyn RtGenerator>>,
}

impl Renderer {
    pub(crate) fn new(p: RendererParts) -> Self {
        let c = p.out_ch;
        Self {
            sample_rate: p.sample_rate,
            out_ch: c,
            declick: p.declick,
            safety_clamp: p.safety_clamp,
            clock: 0,
            shared: p.shared,
            commands: p.commands,
            events: p.events,
            pending: VecDeque::with_capacity(p.pending_cap),
            pending_cap: p.pending_cap,
            slots: (0..p.max_voices).map(|_| None).collect(),
            master: Ramp::constant(1.0),
            out_gain: vec![Ramp::constant(1.0); c].into_boxed_slice(),
            out_gain_ramp_left: 0,
            generators: p.generators,
            mix: vec![0.0; SUB_BLOCK * c].into_boxed_slice(),
            blk_peak: vec![0.0; c].into_boxed_slice(),
            blk_sq: vec![0.0; c].into_boxed_slice(),
            blk_clip: vec![false; c].into_boxed_slice(),
            peak_state: vec![0.0; c].into_boxed_slice(),
            ms_state: vec![0.0; c].into_boxed_slice(),
        }
    }

    /// Engine sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Number of engine output channels.
    pub fn out_channels(&self) -> usize {
        self.out_ch
    }

    /// Frame index of the next sample that will be rendered.
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// Render `out.len() / channels` frames of interleaved f32 audio.
    ///
    /// `channels` is the channel count of `out` and may differ from the
    /// engine's: the first `min(channels, out_channels)` channels are
    /// copied, extra device channels are zeroed. Any block size works,
    /// including odd ones; commands are applied sample-accurately inside
    /// the block.
    ///
    /// Real-time safe: no allocation, deallocation, lock, I/O or system call.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        if channels == 0 {
            out.fill(0.0);
            return;
        }
        let frames = out.len() / channels;
        let oc = self.out_ch;
        self.drain_commands();
        self.blk_peak.fill(0.0);
        self.blk_sq.fill(0.0);
        self.blk_clip.fill(false);

        let mut done = 0usize;
        let mut clock = self.clock;
        while done < frames {
            self.apply_due(clock);
            let mut n = (frames - done).min(SUB_BLOCK);
            if let Some(next) = self.pending.front() {
                let d = next.at().saturating_sub(clock);
                if d < n as u64 {
                    n = (d as usize).max(1);
                }
            }
            n = self.master.limit(n);
            if self.out_gain_ramp_left > 0 {
                for r in self.out_gain.iter() {
                    n = r.limit(n);
                }
            }

            let mix = &mut self.mix[..n * oc];
            mix.fill(0.0);

            // 1. voices
            for v in self.slots.iter_mut().flatten() {
                if v.state == VoiceState::Playing {
                    mix_voice(v, mix, oc, n, &self.shared, &mut self.events);
                }
            }

            // 2. master gain and per-output gains
            let (ms, me) = self.master.step(n);
            let out_ramping = self.out_gain_ramp_left > 0;
            let inv = 1.0 / n as f32;
            for (c, r) in self.out_gain.iter_mut().enumerate() {
                let (os, oe) = if out_ramping {
                    r.step(n)
                } else {
                    (r.value(), r.value())
                };
                let gs = ms * os;
                let ge = me * oe;
                if gs == 1.0 && ge == 1.0 {
                    continue;
                }
                let dg = (ge - gs) * inv;
                for (i, frame) in mix.chunks_exact_mut(oc).enumerate() {
                    frame[c] *= gs + dg * i as f32;
                }
            }
            self.out_gain_ramp_left = self.out_gain_ramp_left.saturating_sub(n as u64);

            // 3. generators (not affected by the master fader)
            for g in self.generators.iter_mut() {
                g.render(clock, mix, oc);
            }

            // 4. meters, clip detection, safety clamp
            let clamp = self.safety_clamp;
            for frame in mix.chunks_exact_mut(oc) {
                for (c, s) in frame.iter_mut().enumerate() {
                    let a = s.abs();
                    if a < 1.0 {
                        if a > self.blk_peak[c] {
                            self.blk_peak[c] = a;
                        }
                        self.blk_sq[c] += a * a;
                    } else {
                        // |x| >= 1.0 or NaN
                        self.blk_clip[c] = true;
                        if a.is_nan() {
                            *s = 0.0;
                        } else {
                            let m = if a.is_finite() { a } else { f32::MAX };
                            if m > self.blk_peak[c] {
                                self.blk_peak[c] = m;
                            }
                            self.blk_sq[c] += m.min(1.0e6) * m.min(1.0e6);
                            if clamp {
                                *s = s.clamp(-1.0, 1.0);
                            }
                        }
                    }
                }
            }

            // 5. copy to the device buffer
            let dst = &mut out[done * channels..(done + n) * channels];
            if channels == oc {
                dst.copy_from_slice(mix);
            } else {
                let common = channels.min(oc);
                for (d, s) in dst.chunks_exact_mut(channels).zip(mix.chunks_exact(oc)) {
                    d[..common].copy_from_slice(&s[..common]);
                    d[common..].fill(0.0);
                }
            }

            done += n;
            clock += n as u64;
        }
        out[frames * channels..].fill(0.0);

        self.clock = clock;
        self.shared.clock.store(clock, Ordering::Release);
        self.publish_meters(frames);
        self.return_finished();
    }

    /// Move newly arrived commands into the time-sorted pending list.
    fn drain_commands(&mut self) {
        while self.pending.len() < self.pending_cap {
            let Ok(cmd) = self.commands.pop() else { break };
            let at = cmd.at();
            // Stable: after every command with the same or an earlier time.
            let idx = self.pending.partition_point(|c| c.at() <= at);
            self.pending.insert(idx, cmd);
        }
    }

    /// Apply every pending command whose time has come.
    fn apply_due(&mut self, clock: u64) {
        while self.pending.front().is_some_and(|c| c.at() <= clock) {
            if let Some(cmd) = self.pending.pop_front() {
                self.apply(cmd);
            }
        }
    }

    fn voice_mut(&mut self, slot: u16, id: u64) -> Option<&mut VoiceData> {
        self.slots
            .get_mut(slot as usize)?
            .as_deref_mut()
            .filter(|v| v.id == id && v.state != VoiceState::Finished)
    }

    fn apply(&mut self, cmd: Command) {
        let declick = self.declick;
        match cmd {
            Command::AddVoice { slot, data } => {
                match self.slots.get_mut(slot as usize) {
                    Some(s) if s.is_none() => *s = Some(data),
                    _ => {
                        if let Err(PushError::Full(ev)) = self.events.push(RtEvent::Orphan { data })
                        {
                            // Cannot free memory here; leaking is the only
                            // real-time-safe option (unreachable in practice).
                            std::mem::forget(ev);
                        }
                    }
                }
            }
            Command::Start {
                slot,
                id,
                fade_in,
                curve,
                ..
            } => {
                let shared = &self.shared;
                if let Some(v) = self
                    .slots
                    .get_mut(slot as usize)
                    .and_then(|s| s.as_deref_mut())
                    .filter(|v| v.id == id && v.state == VoiceState::Prepared)
                {
                    v.state = VoiceState::Playing;
                    shared.active.fetch_add(1, Ordering::Relaxed);
                    if fade_in > 0 {
                        v.env.jump(0.0);
                        v.env.set(1.0, fade_in, curve);
                    } else {
                        v.env.jump(1.0);
                    }
                }
            }
            Command::Stop {
                slot,
                id,
                fade,
                curve,
                hard,
                ..
            } => {
                let shared = &self.shared;
                if let Some(v) = self
                    .slots
                    .get_mut(slot as usize)
                    .and_then(|s| s.as_deref_mut())
                    .filter(|v| v.id == id)
                {
                    v.stop(shared, fade, curve, hard, declick);
                }
            }
            Command::Pause { slot, id, .. } => {
                if let Some(v) = self.voice_mut(slot, id) {
                    if v.state == VoiceState::Playing && !v.pausing {
                        v.pausing = true;
                        v.pause.set(0.0, declick, FadeCurve::Linear);
                        if declick == 0 {
                            v.held = true;
                        }
                    }
                }
            }
            Command::Resume { slot, id, .. } => {
                if let Some(v) = self.voice_mut(slot, id) {
                    if v.pausing {
                        v.pausing = false;
                        v.held = false;
                        v.pause.set(1.0, declick, FadeCurve::Linear);
                    }
                }
            }
            Command::Fade {
                slot,
                id,
                duration,
                curve,
                master,
                n_xp,
                xp,
                ..
            } => {
                let oc = self.out_ch;
                if let Some(v) = self.voice_mut(slot, id) {
                    // A voice that has not started yet has no audible state
                    // to ramp from: apply instantly.
                    let duration = if v.state == VoiceState::Prepared {
                        0
                    } else {
                        duration
                    };
                    if let Some(g) = master {
                        v.master.set(g, duration, curve);
                    }
                    for &(s, o, g) in xp.iter().take(n_xp as usize) {
                        v.set_crosspoint(oc, s, o, g, duration, curve);
                    }
                }
            }
            Command::StopAll {
                fade, curve, hard, ..
            } => {
                let shared = &self.shared;
                for v in self.slots.iter_mut().flatten() {
                    if v.state == VoiceState::Playing {
                        v.stop(shared, fade, curve, hard, declick);
                    }
                }
                // Voices with a start still scheduled count as started.
                for c in self.pending.iter() {
                    if let Command::Start { slot, id, .. } = c {
                        if let Some(v) = self
                            .slots
                            .get_mut(*slot as usize)
                            .and_then(|s| s.as_deref_mut())
                            .filter(|v| v.id == *id && v.state == VoiceState::Prepared)
                        {
                            v.finish(shared);
                        }
                    }
                }
            }
            Command::MasterGain {
                gain, ramp, curve, ..
            } => {
                self.master.set(gain, ramp, curve);
            }
            Command::OutputGain {
                channel,
                gain,
                ramp,
                ..
            } => {
                if let Some(r) = self.out_gain.get_mut(channel as usize) {
                    r.set(gain, ramp, FadeCurve::Linear);
                    self.out_gain_ramp_left = self.out_gain_ramp_left.max(ramp);
                }
            }
        }
    }

    fn publish_meters(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        let dt = frames as f32 / self.sample_rate.max(1) as f32;
        let peak_decay = (-PEAK_DECAY_NEPER_PER_S * dt).exp();
        let alpha = 1.0 - (-dt / RMS_TAU_S).exp();
        let inv_frames = 1.0 / frames as f32;
        for c in 0..self.out_ch {
            let p = (self.peak_state[c] * peak_decay).max(self.blk_peak[c]);
            let p = if p < 1.0e-9 { 0.0 } else { p };
            self.peak_state[c] = p;
            let ms = self.ms_state[c] + (self.blk_sq[c] * inv_frames - self.ms_state[c]) * alpha;
            let ms = if ms < 1.0e-18 { 0.0 } else { ms };
            self.ms_state[c] = ms;
            self.shared.peak[c].store(p.to_bits(), Ordering::Relaxed);
            self.shared.rms[c].store(ms.sqrt().to_bits(), Ordering::Relaxed);
            if self.blk_clip[c] {
                self.shared.clip[c].store(true, Ordering::Relaxed);
            }
        }
    }

    /// Hand finished voices back to the control side for deallocation.
    fn return_finished(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if !matches!(slot, Some(v) if v.state == VoiceState::Finished) {
                continue;
            }
            if let Some(data) = slot.take() {
                match self.events.push(RtEvent::Ended {
                    slot: i as u16,
                    data,
                }) {
                    Ok(()) => {}
                    // Queue full: keep the voice and retry on the next block.
                    Err(PushError::Full(RtEvent::Ended { data, .. })) => *slot = Some(data),
                    Err(PushError::Full(other)) => std::mem::forget(other),
                }
            }
        }
    }
}

/// Mix `n` frames of one playing voice into `mix` (interleaved, `oc` channels).
fn mix_voice(
    v: &mut VoiceData,
    mix: &mut [f32],
    oc: usize,
    n: usize,
    shared: &Shared,
    events: &mut Producer<RtEvent>,
) {
    let sc = v.src_ch;
    let mut done = 0usize;
    while done < n {
        if v.held {
            // Paused: nothing is consumed; gain ramps keep following the
            // master clock so a fade always ends when it was asked to.
            let k = n - done;
            v.master.step(k);
            v.env.step(k);
            if v.xp_ramp_left > 0 {
                for &(s, o) in v.xp_active[..v.xp_active_len].iter() {
                    v.xp[s as usize * oc + o as usize].step(k);
                }
                v.xp_ramp_left = v.xp_ramp_left.saturating_sub(k as u64);
            }
            return;
        }

        // Segment: up to the end of the sub-block or of the nearest ramp.
        let mut seg = n - done;
        seg = v.env.limit(seg);
        seg = v.master.limit(seg);
        seg = v.pause.limit(seg);
        let xp_ramping = v.xp_ramp_left > 0;
        if xp_ramping {
            for &(s, o) in v.xp_active[..v.xp_active_len].iter() {
                seg = v.xp[s as usize * oc + o as usize].limit(seg);
            }
        }

        let (ms, me) = v.master.step(seg);
        let (es, ee) = v.env.step(seg);
        let (ps, pe) = v.pause.step(seg);
        let vs = ms * es * ps;
        let ve = me * ee * pe;

        // `eos` must be read before the fill level: the streamer sets it
        // only after the last frames are in the ring.
        let eos = v.shared.eos.load(Ordering::Acquire);
        let avail = v.consumer.slots() / sc;
        let want = seg.min(avail);
        let chunk = v.consumer.read_chunk(want * sc).ok();
        let (a, b): (&[f32], &[f32]) = match &chunk {
            Some(c) => c.as_slices(),
            None => (&[], &[]),
        };
        let take = (a.len() + b.len()) / sc;
        let first_b = a.len() / sc;
        let inv = 1.0 / seg as f32;
        let dst = &mut mix[done * oc..(done + seg) * oc];

        for &(s, o) in v.xp_active[..v.xp_active_len].iter() {
            let (s, o) = (s as usize, o as usize);
            let r = &mut v.xp[s * oc + o];
            let (xs, xe) = if xp_ramping {
                r.step(seg)
            } else {
                (r.value(), r.value())
            };
            let gs = xs * vs;
            let ge = xe * ve;
            if gs == 0.0 && ge == 0.0 {
                continue;
            }
            let dg = (ge - gs) * inv;
            mix_crosspoint(a, sc, s, dst, oc, o, gs, dg, 0);
            mix_crosspoint(b, sc, s, dst, oc, o, gs, dg, first_b);
        }
        if let Some(c) = chunk {
            c.commit_all();
        }
        if xp_ramping {
            v.xp_ramp_left = v.xp_ramp_left.saturating_sub(seg as u64);
        }
        v.consumed += take as u64;
        v.shared.consumed.store(v.consumed, Ordering::Relaxed);

        if take < seg {
            if eos {
                v.finish(shared);
                return;
            }
            // Underrun: silence for the missing frames, position not
            // advanced, so the voice simply continues late.
            if !v.in_underrun {
                v.in_underrun = true;
                v.underrun_reported = false;
                shared.underruns.fetch_add(1, Ordering::Relaxed);
            }
            if !v.underrun_reported && events.push(RtEvent::Underrun { id: v.id }).is_ok() {
                v.underrun_reported = true;
            }
        } else {
            v.in_underrun = false;
            if eos && v.consumer.is_empty() {
                v.finish(shared);
                return;
            }
        }
        if v.stopping && !v.env.is_ramping() {
            v.finish(shared);
            return;
        }
        if v.pausing && !v.pause.is_ramping() {
            if v.stopping {
                // Inaudible from here on: no point in finishing the fade.
                v.finish(shared);
                return;
            }
            v.held = true;
        }
        done += seg;
    }
}

/// `dst[frame][o] += src[frame][s] * (g0 + dg * (first + frame))`.
#[allow(clippy::too_many_arguments)]
#[inline]
fn mix_crosspoint(
    src: &[f32],
    sc: usize,
    s: usize,
    dst: &mut [f32],
    oc: usize,
    o: usize,
    g0: f32,
    dg: f32,
    first: usize,
) {
    if src.is_empty() {
        return;
    }
    let dst = &mut dst[first * oc..];
    if dg == 0.0 {
        for (sf, df) in src.chunks_exact(sc).zip(dst.chunks_exact_mut(oc)) {
            df[o] += sf[s] * g0;
        }
    } else {
        for (i, (sf, df)) in src
            .chunks_exact(sc)
            .zip(dst.chunks_exact_mut(oc))
            .enumerate()
        {
            df[o] += sf[s] * (g0 + dg * (first + i) as f32);
        }
    }
}
