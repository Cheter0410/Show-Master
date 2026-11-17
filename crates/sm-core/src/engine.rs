//! The deterministic cue engine.
//!
//! The engine is a pure state machine over the audio sample clock. It never
//! reads a wall clock and never blocks: the host calls [`Engine::advance`]
//! with the current logical time (audio clock plus a small look-ahead),
//! feeds [`Command`]s, and drains [`Action`]s (sample-timestamped
//! instructions for the audio/MIDI layer) and [`Event`]s (state changes for
//! UI, remote clients and the log).
//!
//! Everything that happens "at the same time" carries the same sample frame,
//! so cues fired by one GO start sample-accurately relative to each other,
//! and pre-wait / post-wait / auto-follow / timecode triggers are exact to
//! the sample because they are computed, not polled.

use crate::chase::{ChaseStep, Chaser, TcObservation, TcStatus};
use crate::db_to_lin;
use crate::model::{
    AudioCue, ContinueMode, ControlAction, Crosspoint, Cue, CueId, CueKind, Curve, GroupMode,
    ListId, Loop, PanicMessage, Show, TcAction, TcJumpPolicy, TcMode, TcSource,
};
use serde::{Deserialize, Serialize};
use sm_timecode::{TcTransport, Timecode};
use std::collections::HashMap;

pub type VoiceId = u64;

/// Everything a user, hotkey, remote or Stream Deck can ask the engine to do.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Go {
        #[serde(default)]
        list: Option<ListId>,
    },
    StopAll,
    Panic,
    PauseAll,
    ResumeAll,
    TogglePause,
    Next {
        #[serde(default)]
        list: Option<ListId>,
    },
    Previous {
        #[serde(default)]
        list: Option<ListId>,
    },
    SetPlayhead {
        #[serde(default)]
        list: Option<ListId>,
        cue: Option<CueId>,
    },
    /// "Go to cue 14": move the playhead to the cue with this number.
    GotoNumber {
        number: String,
    },
    /// Fire a single cue without moving the playhead (preview / hot start).
    StartCue {
        cue: CueId,
    },
    StopCue {
        cue: CueId,
    },
    /// Release the loop an audio cue plays: it leaves it at the end of the pass (or right
    /// away for an "immediate" loop) and plays on to its next loop or the end. With `name`
    /// only that loop: when the cue has not reached it yet, it is skipped when it comes.
    ReleaseLoop {
        cue: CueId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Release the loop of every cue that plays one (with `name`: only loops of that name).
    ReleaseLoops {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Jump to `time` seconds into the cue's action (the waveform's
    /// playhead): a playing cue continues from there, a stopped one is
    /// loaded to that point (GO / preview start there).
    SeekCue {
        cue: CueId,
        time: f64,
    },
    PauseCue {
        cue: CueId,
    },
    ResumeCue {
        cue: CueId,
    },
    LoadCue {
        cue: CueId,
        #[serde(default)]
        time: f64,
    },
    ArmCue {
        cue: CueId,
        armed: bool,
    },
    ResetCue {
        cue: CueId,
    },
    Reset,
    ResetShowClock,
    TcStart {
        #[serde(default)]
        from: Option<Timecode>,
    },
    TcStop,
    TcPause,
    TcResume,
    TcRelocate {
        to: Timecode,
    },
    SetMaster {
        db: f32,
    },
    MidiPanic,
}

/// How to open the media for a voice.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSpec {
    pub file: String,
    pub start: f64,
    pub end: Option<f64>,
    pub loop_count: u32,
    /// Playback offset in frames (engine rate) from the start of the first pass.
    pub offset_frames: u64,
    /// Vamp: passes after the first start here (seconds of file time).
    pub loop_from: Option<f64>,
    /// Crossfade at each jump back of the vamp (seconds; 0 = hard jump).
    pub loop_xfade: f64,
    /// Start in source frames (overrides `start`; sample-exact splices).
    pub start_frame: Option<u64>,
    pub gain: f32,
    /// (source channel, device channel, linear gain)
    pub matrix: Vec<(u16, u16, f32)>,
}

/// Sample-timestamped instructions for the I/O layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    PrepareVoice {
        voice: VoiceId,
        cue: CueId,
        spec: VoiceSpec,
    },
    StartVoice {
        voice: VoiceId,
        at: u64,
        fade_in_frames: u64,
        curve: Curve,
    },
    StopVoice {
        voice: VoiceId,
        at: u64,
        fade_frames: u64,
        curve: Curve,
        hard: bool,
    },
    PauseVoice {
        voice: VoiceId,
        at: u64,
    },
    ResumeVoice {
        voice: VoiceId,
        at: u64,
    },
    FadeVoice {
        voice: VoiceId,
        at: u64,
        frames: u64,
        curve: Curve,
        master: Option<f32>,
        crosspoints: Vec<(u16, u16, f32)>,
    },
    DiscardVoice {
        voice: VoiceId,
    },
    StopAllAudio {
        at: u64,
        fade_frames: u64,
        hard: bool,
    },
    Midi {
        at: u64,
        port: String,
        bytes: Vec<u8>,
    },
    /// One HTTP request (complete bytes) to `addr` (`host:port`).
    Http {
        at: u64,
        addr: String,
        bytes: Vec<u8>,
        /// For the log: `POST http://...`.
        summary: String,
    },
    /// One OSC message (UDP payload) to `target` (`host:port`).
    Osc {
        at: u64,
        target: String,
        bytes: Vec<u8>,
    },
    MidiPanic,
    Timecode(TcTransport),
    MasterGain {
        gain: f32,
        ramp_frames: u64,
    },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    Go {
        cue: CueId,
    },
    GoIgnored {
        reason: String,
    },
    CueTriggered {
        cue: CueId,
    },
    CueStarted {
        cue: CueId,
    },
    CueEnded {
        cue: CueId,
    },
    CueStopped {
        cue: CueId,
    },
    CuePaused {
        cue: CueId,
    },
    CueResumed {
        cue: CueId,
    },
    CueLoaded {
        cue: CueId,
        time: f64,
    },
    CueError {
        cue: CueId,
        message: String,
    },
    CueArmed {
        cue: CueId,
        armed: bool,
    },
    /// A loop was released; the cue leaves it `in_secs` from now.
    LoopReleased {
        cue: CueId,
        in_secs: f64,
        name: String,
    },
    Playhead {
        list: ListId,
        cue: Option<CueId>,
    },
    StopAll,
    Panic,
    PausedAll,
    ResumedAll,
    Reset,
    Timecode {
        action: String,
        position: String,
    },
    Master {
        db: f32,
    },
    Info {
        message: String,
    },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Event {
    /// Engine time (sample frames) at which this happened.
    pub frame: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    PreWait,
    Running,
    PostWait,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ActiveCue {
    pub cue: CueId,
    pub list: ListId,
    pub phase: Phase,
    pub paused: bool,
    /// Seconds into the current phase.
    pub elapsed: f64,
    /// Length of the current phase in seconds; `None` = open-ended.
    pub duration: Option<f64>,
    pub from_tc: bool,
    /// Audio cue playing a loop and not released yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vamping: bool,
    /// The loop the cue plays (also while it is released and waits for the loop end).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub looping: Option<LoopStatus>,
    /// Position in the file (seconds) for an audio cue with loops.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_pos: Option<f64>,
}

/// The loop an audio cue is playing, for Live and the remotes.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LoopStatus {
    pub name: String,
    /// Index among the cue's loops (0-based) and how many it has.
    pub index: usize,
    pub count: usize,
    /// The pass playing (1-based).
    pub pass: u64,
    /// Passes before it releases itself; 0 = until released.
    pub passes: u32,
    /// Seconds until the end of this pass (released: until it leaves the loop).
    pub remaining: f64,
    /// Released, leaving at the end of the pass.
    pub releasing: bool,
    /// GO releases it.
    pub release_on_go: bool,
    pub immediate: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ListState {
    pub id: ListId,
    pub playhead: Option<CueId>,
}

/// Complete runtime state for display.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub frame: u64,
    pub sample_rate: u32,
    pub lists: Vec<ListState>,
    pub active: Vec<ActiveCue>,
    pub errors: HashMap<CueId, String>,
    /// Cues loaded to a time (seconds).
    pub loaded: HashMap<CueId, f64>,
    /// Seconds since the first GO; `None` before it.
    pub show_elapsed: Option<f64>,
    pub paused_all: bool,
    pub master_db: f32,
    pub tc: TcTransport,
    pub tc_position: String,
    /// Generator, or the state of the external timecode being chased.
    pub tc_status: TcStatus,
    /// Speed of the chased source relative to nominal (1.0 = exact), while locked.
    pub tc_speed: Option<f64>,
    /// Seconds until the next automatically fired cue (auto-continue,
    /// auto-follow with known duration, or timecode trigger).
    pub next_auto: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ev {
    Action,
    VampWrap,
    VampSwitch,
    FadeOut,
    Continue,
    End,
    TcTrigger,
}

#[derive(Clone, Debug)]
struct GroupRt {
    mode: GroupMode,
    order: Vec<CueId>,
    idx: usize,
    live: usize,
}

#[derive(Clone, Debug)]
struct Inst {
    id: u64,
    cue: CueId,
    list: ListId,
    parent: Option<u64>,
    /// Position on the cue's own timeline (frames) at global time `since`.
    local: u64,
    since: u64,
    paused: bool,
    pre: u64,
    dur: Option<u64>,
    started: bool,
    action_done: bool,
    cont_at: Option<u64>,
    fade_at: Option<u64>,
    fade_frames: u64,
    fade_curve: Curve,
    follow: bool,
    next: Option<CueId>,
    voice: Option<VoiceId>,
    from_tc: bool,
    disarmed: bool,
    group: Option<GroupRt>,
    stop_target_on_end: Option<CueId>,
    vamp: VampState,
    /// The vamp voice fading out after the switch, until this global time.
    old_voice: Option<(VoiceId, u64)>,
    /// The loop being played (also while released and waiting for its end).
    lp: Option<LoopRt>,
    /// Index of the loop the cue plays or comes to next (audio cues with loops).
    seg: usize,
    /// Loops released before the cue got to them: skipped when they come.
    skip: Vec<usize>,
    /// "Immediate" loop: the audio after it, opened ahead so a release can switch at once.
    spare: Option<VoiceId>,
    /// Where the current segment's voice started: (offset after the pre-wait, file seconds).
    origin: (u64, f64),
}

/// Where the loop ends of a looping voice fall, exactly: the `k`-th loop end is
/// `intro + k * len` source frames after the voice's first frame, which plays at
/// `base` (engine frames after the pre-wait).
#[derive(Clone, Copy, Debug)]
struct LoopRt {
    index: usize,
    base: u64,
    intro: u64,
    len: u64,
    rate: u64,
    sr: u64,
    /// The voice's first source frame and the loop (absolute source frames).
    src0: u64,
    ls: u64,
    le: u64,
    /// Next loop end the timecode jumps back at (when `tc`).
    next: u64,
    tc: bool,
}

impl LoopRt {
    /// Voice position (engine frames) of source position `n` (relative to `src0`).
    fn at(&self, n: u64) -> u64 {
        ((u128::from(n) * u128::from(self.sr) + u128::from(self.rate) / 2) / u128::from(self.rate))
            as u64
    }
    /// Position of loop end `k` (engine frames after the pre-wait).
    fn end(&self, k: u64) -> u64 {
        self.base + self.at(self.intro + k * self.len)
    }
    /// Length (engine frames) of the pass that ends at loop end `k`.
    fn pass(&self, k: u64) -> u64 {
        self.end(k) - self.base - self.at((self.intro + k * self.len).saturating_sub(self.len))
    }
    /// The first loop end after `pos`.
    fn next_end(&self, pos: u64) -> u64 {
        let rel = pos.saturating_sub(self.base) as f64 * self.rate as f64 / self.sr as f64;
        let mut k = if rel < self.intro as f64 {
            0
        } else {
            ((rel - self.intro as f64) / self.len as f64).floor() as u64 + 1
        };
        while k > 0 && self.end(k - 1) > pos {
            k -= 1;
        }
        while self.end(k) <= pos {
            k += 1;
        }
        k
    }
    /// File position (seconds) at `pos`.
    fn file_pos(&self, pos: u64) -> f64 {
        let rel = (pos.saturating_sub(self.base) as f64 * self.rate as f64 / self.sr as f64) as u64;
        let n = if rel < self.intro {
            self.src0 + rel
        } else {
            self.ls + (rel - self.intro) % self.len.max(1)
        };
        n as f64 / self.rate as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum VampState {
    /// No vamp, or released and done.
    #[default]
    None,
    /// Looping, not released.
    Looping,
    /// Released: at local frame `at` the voice `next` (the audio after the loop) takes
    /// over; `len` = its length in frames when it plays to the end (no further loop).
    Switching {
        at: u64,
        next: VoiceId,
        len: Option<u64>,
        /// Crossfade length (frames), centred on the loop end.
        xfade: u64,
        /// Source frames the new voice starts before the loop end.
        h_src: u64,
        /// Released by its number of passes (a manual release may still come earlier).
        auto: bool,
    },
}

/// Least half crossfade at a vamp release, in frames (both voices play the
/// same audio for at least this long before the loop end).
const VAMP_XFADE_HALF: u64 = 128;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

impl Inst {
    /// Every voice of this instance that may still sound at `t`: the main
    /// one and a vamp voice still fading out after its switch.
    fn voices(&self, t: u64) -> impl Iterator<Item = VoiceId> + '_ {
        self.voice.into_iter().chain(
            self.old_voice
                .filter(|(_, until)| t < *until)
                .map(|(v, _)| v),
        )
    }

    /// Prepared (not started) voices: the audio after a released loop, and the one kept
    /// ready for an immediate release.
    fn pending_voices(&self) -> Vec<VoiceId> {
        let mut v: Vec<VoiceId> = self.spare.into_iter().collect();
        if let VampState::Switching { next, .. } = self.vamp {
            v.push(next);
        }
        v
    }

    fn local_at(&self, t: u64) -> u64 {
        if self.paused {
            self.local
        } else {
            self.local + t.saturating_sub(self.since)
        }
    }

    fn global(&self, local: u64) -> u64 {
        self.since + local.saturating_sub(self.local)
    }

    fn due(&self) -> Option<(u64, Ev)> {
        if self.paused {
            return None;
        }
        if !self.started {
            return Some((self.global(self.pre), Ev::Action));
        }
        let mut best: Option<(u64, Ev)> = None;
        let mut consider = |t: u64, ev: Ev| {
            if best.is_none_or(|b| (t, ev) < b) {
                best = Some((t, ev));
            }
        };
        if let Some(f) = self.fade_at {
            consider(self.global(f), Ev::FadeOut);
        }
        if let VampState::Switching { at, .. } = self.vamp {
            consider(self.global(at), Ev::VampSwitch);
        }
        if let Some(w) = self.lp.filter(|w| w.tc) {
            let local = self.pre + w.end(w.next);
            let before_switch = match self.vamp {
                VampState::Switching { at, .. } => local < at,
                VampState::Looping => true,
                _ => false,
            };
            if before_switch {
                consider(self.global(local), Ev::VampWrap);
            }
        }
        if let Some(c) = self.cont_at {
            consider(self.global(c), Ev::Continue);
        }
        if !self.action_done && self.group.is_none() {
            if let Some(d) = self.dur {
                consider(self.global(self.pre + d), Ev::End);
            }
        }
        best
    }
}

#[derive(Default, Clone, Copy)]
struct Trig {
    parent: Option<u64>,
    /// Start position on the cue timeline (pre-wait + action), frames.
    initial_local: u64,
    from_tc: bool,
    chase: bool,
}

/// Level and crosspoints (source, output, gain) of a playing voice.
type VoiceMix = (f32, Vec<(u16, u16, f32)>);

pub struct Engine {
    show: Show,
    sr: u32,
    now: u64,
    playheads: HashMap<ListId, Option<CueId>>,
    insts: Vec<Inst>,
    next_inst: u64,
    next_voice: VoiceId,
    standby: HashMap<CueId, (VoiceId, u64)>,
    standby_dirty: bool,
    loaded: HashMap<CueId, u64>,
    errors: HashMap<CueId, String>,
    last_go: Option<u64>,
    show_start: Option<u64>,
    paused_all: bool,
    master_db: f32,
    tc: TcTransport,
    tc_next_k: u64,
    tc_pos_fired: bool,
    tc_paused_by_transport: bool,
    tc_triggers: Vec<(u64, CueId, ListId)>,
    /// External timecode follower (`timecode.source == external`).
    chase: Option<Chaser>,
    /// Settings the chaser was built from.
    chase_key: Option<ChaseKey>,
    rng: u64,
    actions: Vec<Action>,
    events: Vec<Event>,
}

const MAX_STANDBY: usize = 8;

/// Settings that require a new chaser when they change.
type ChaseKey = (sm_timecode::FrameRate, crate::model::TcInputKind, u32, u32);

impl Engine {
    pub fn new(show: Show, sample_rate: u32) -> Self {
        let rate = show.timecode.rate;
        let mut e = Self {
            show: Show::default(),
            sr: sample_rate.max(1),
            now: 0,
            playheads: HashMap::new(),
            insts: Vec::new(),
            next_inst: 1,
            next_voice: 1,
            standby: HashMap::new(),
            standby_dirty: true,
            loaded: HashMap::new(),
            errors: HashMap::new(),
            last_go: None,
            show_start: None,
            paused_all: false,
            master_db: 0.0,
            tc: TcTransport::stopped(rate),
            tc_next_k: 0,
            tc_pos_fired: false,
            tc_paused_by_transport: false,
            tc_triggers: Vec::new(),
            chase: None,
            chase_key: None,
            rng: 0x9E37_79B9_7F4A_7C15,
            actions: Vec::new(),
            events: Vec::new(),
        };
        e.set_show(show);
        // A stopped generator rests at the configured start offset.
        e.tc.anchor_frame = e.show.timecode.start.to_frame_number(rate);
        // Choosing the timecode source at start-up is not news.
        e.events
            .retain(|ev| !matches!(ev.kind, EventKind::Timecode { .. }));
        e
    }

    pub fn show(&self) -> &Show {
        &self.show
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn sample_rate(&self) -> u32 {
        self.sr
    }

    /// Seed for random groups (tests use this for reproducibility).
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    /// Replace the show (after an edit). Running cues that still exist keep
    /// running; cues that were removed are stopped.
    pub fn set_show(&mut self, show: Show) {
        let t = self.now;
        // Level and routing of running audio cues follow edits (gain knob,
        // routing matrix) with a short ramp instead of waiting for the next start.
        let before = self.running_mix();
        self.show = show;
        let after = self.running_mix();
        let ramp = self.fr(0.03);
        for (voice, (gain, matrix)) in after {
            let Some((old_gain, old_matrix)) = before.get(&voice) else {
                continue;
            };
            if *old_gain == gain && *old_matrix == matrix {
                continue;
            }
            // Crosspoints that are gone are faded to silence.
            let mut crosspoints = matrix.clone();
            for &(s, d, _) in old_matrix {
                if !matrix.iter().any(|&(s2, d2, _)| s2 == s && d2 == d) {
                    crosspoints.push((s, d, 0.0));
                }
            }
            self.actions.push(Action::FadeVoice {
                voice,
                at: t,
                frames: ramp,
                curve: Curve::Linear,
                master: Some(gain),
                crosspoints,
            });
        }
        let gone: Vec<u64> = self
            .insts
            .iter()
            .filter(|i| self.show.cue(i.cue).is_none())
            .map(|i| i.id)
            .collect();
        for id in gone {
            self.stop_inst(id, t, 0, false, false);
        }
        let mut heads = HashMap::new();
        for l in &self.show.cue_lists {
            let prev = self.playheads.get(&l.id).copied();
            let head = match prev {
                Some(Some(c)) if l.step_index(c).is_some() => Some(c),
                Some(None) if !self.insts.is_empty() || self.show_start.is_some() => None,
                _ => l.steps().first().map(|c| c.id),
            };
            heads.insert(l.id, head);
        }
        self.playheads = heads;
        self.loaded.retain(|c, _| self.show.cue(*c).is_some());
        self.errors.retain(|c, _| self.show.cue(*c).is_some());
        if self.tc.rate != self.show.timecode.rate && !self.tc.running {
            self.tc = TcTransport::stopped(self.show.timecode.rate);
            self.tc.anchor_frame = self.show.timecode.start.to_frame_number(self.tc.rate);
            self.actions.push(Action::Timecode(self.tc));
        }
        self.configure_chase(t);
        self.rebuild_tc_triggers();
        // Media or routing may have changed: re-prepare all standby voices.
        for (_, (voice, _)) in self.standby.drain() {
            self.actions.push(Action::DiscardVoice { voice });
        }
        self.standby_dirty = true;
        self.refresh_standby();
    }

    pub fn drain_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn fr(&self, secs: f64) -> u64 {
        if secs.is_finite() && secs > 0.0 {
            (secs * f64::from(self.sr)).round() as u64
        } else {
            0
        }
    }

    fn secs(&self, frames: u64) -> f64 {
        frames as f64 / f64::from(self.sr)
    }

    fn emit(&mut self, t: u64, kind: EventKind) {
        self.events.push(Event { frame: t, kind });
    }

    fn default_list(&self, list: Option<ListId>) -> Option<ListId> {
        list.filter(|l| self.show.list(*l).is_some())
            .or_else(|| self.show.cue_lists.first().map(|l| l.id))
    }

    // ------------------------------------------------------------------
    // Time
    // ------------------------------------------------------------------

    /// Process everything due up to and including `until` and move the
    /// logical clock there.
    pub fn advance(&mut self, until: u64) {
        if until < self.now {
            return;
        }
        // Losing the external timecode is an event in time: nothing that
        // depends on the running timecode may happen after it.
        if let Some(deadline) = self.chase.as_ref().and_then(Chaser::deadline) {
            if deadline <= until {
                let d = deadline.max(self.now);
                self.process_due(d);
                self.now = d;
                self.chase_tick(d);
            }
        }
        self.process_due(until);
        self.now = until;
        self.chase_tick(until);
        self.refresh_standby();
    }

    fn next_due(&self) -> Option<(u64, Ev, u64)> {
        let mut best: Option<(u64, Ev, u64)> = None;
        for i in &self.insts {
            if let Some((t, ev)) = i.due() {
                if best.is_none_or(|b| (t, ev) < (b.0, b.1)) {
                    best = Some((t, ev, i.id));
                }
            }
        }
        if let Some((t, _)) = self.next_tc_trigger() {
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, Ev::TcTrigger, 0));
            }
        }
        best
    }

    fn process_due(&mut self, until: u64) {
        // Bounded so a pathological show (zero-length follow loop) cannot hang the engine.
        for _ in 0..100_000 {
            let Some((t, ev, id)) = self.next_due() else {
                return;
            };
            if t > until {
                return;
            }
            let t = t.max(self.now);
            self.now = t;
            if ev == Ev::TcTrigger {
                self.fire_tc_triggers(t);
            } else {
                self.fire(id, ev, t);
            }
        }
        self.emit(
            until,
            EventKind::Info {
                message: "engine event limit reached - check for cue loops".into(),
            },
        );
    }

    fn idx(&self, id: u64) -> Option<usize> {
        self.insts.iter().position(|i| i.id == id)
    }

    // ------------------------------------------------------------------
    // Instances
    // ------------------------------------------------------------------

    fn is_active(&self, cue: CueId) -> bool {
        self.insts.iter().any(|i| i.cue == cue)
    }

    fn trigger(&mut self, cue_id: CueId, t: u64, opts: Trig) -> Option<u64> {
        let (list, cue) = match self.show.find(cue_id) {
            Some((l, c)) => (l, c),
            None => {
                self.emit(
                    t,
                    EventKind::Info {
                        message: format!("cue {cue_id} does not exist"),
                    },
                );
                return None;
            }
        };
        if self.is_active(cue_id) {
            let number = cue.number.clone();
            self.emit(
                t,
                EventKind::Info {
                    message: format!("cue {number} is already running - trigger ignored"),
                },
            );
            return None;
        }
        let pre = self.fr(cue.pre_wait);
        let post = self.fr(cue.post_wait);
        let armed = cue.armed;
        let mut next = None;
        let mut mode = ContinueMode::DoNotContinue;
        if opts.parent.is_none() {
            if let Some(l) = self.show.list(list) {
                let steps = l.steps();
                if let Some(i) = steps.iter().position(|c| c.id == cue_id) {
                    mode = cue.continue_mode;
                    next = steps.get(i + 1).map(|c| c.id);
                }
            }
        }
        let mut initial = opts.initial_local;
        if initial == 0 {
            if let Some(off) = self.loaded.remove(&cue_id) {
                initial = pre + off;
                self.standby_dirty = true;
            }
        }
        let cont_local = pre + post;
        let cont_at = match mode {
            ContinueMode::AutoContinue
                if next.is_some() && !(opts.chase && initial > cont_local) =>
            {
                Some(cont_local)
            }
            _ => None,
        };
        self.errors.remove(&cue_id);
        let id = self.next_inst;
        self.next_inst += 1;
        self.insts.push(Inst {
            id,
            cue: cue_id,
            list,
            parent: opts.parent,
            local: initial,
            since: t,
            paused: false,
            pre,
            dur: Some(0),
            started: false,
            action_done: false,
            cont_at,
            fade_at: None,
            fade_frames: 0,
            fade_curve: Curve::Linear,
            follow: mode == ContinueMode::AutoFollow && next.is_some(),
            next,
            voice: None,
            from_tc: opts.from_tc,
            disarmed: !armed,
            group: None,
            stop_target_on_end: None,
            vamp: VampState::None,
            old_voice: None,
            lp: None,
            seg: 0,
            skip: Vec::new(),
            spare: None,
            origin: (0, 0.0),
        });
        self.emit(t, EventKind::CueTriggered { cue: cue_id });
        Some(id)
    }

    fn fire(&mut self, id: u64, ev: Ev, t: u64) {
        let Some(ix) = self.idx(id) else { return };
        match ev {
            Ev::Action => {
                self.insts[ix].started = true;
                let cue_id = self.insts[ix].cue;
                if self.insts[ix].disarmed {
                    self.insts[ix].action_done = true;
                } else if let Some(cue) = self.show.cue(cue_id).cloned() {
                    self.emit(t, EventKind::CueStarted { cue: cue_id });
                    self.start_action(id, &cue, t);
                }
            }
            Ev::FadeOut => {
                let i = &mut self.insts[ix];
                i.fade_at = None;
                if let Some(voice) = i.voice {
                    let (frames, curve) = (i.fade_frames, i.fade_curve);
                    self.actions.push(Action::FadeVoice {
                        voice,
                        at: t,
                        frames,
                        curve,
                        master: Some(0.0),
                        crosspoints: Vec::new(),
                    });
                }
            }
            Ev::VampSwitch => self.vamp_switch(ix, t),
            Ev::VampWrap => self.vamp_wrap(ix, t),
            Ev::Continue => {
                self.insts[ix].cont_at = None;
                if let Some(next) = self.insts[ix].next {
                    self.trigger(next, t, Trig::default());
                }
            }
            Ev::End => {
                self.insts[ix].action_done = true;
            }
            Ev::TcTrigger => {}
        }
        self.reap(id, t);
    }

    /// Remove an instance whose action and pending continue are both done.
    fn reap(&mut self, id: u64, t: u64) {
        let Some(ix) = self.idx(id) else { return };
        let i = &self.insts[ix];
        if !i.started || !i.action_done {
            return;
        }
        // Auto-follow fires the moment the action ends, even if the instance
        // lingers for a pending auto-continue (they are mutually exclusive).
        if i.cont_at.is_some() {
            return;
        }
        let inst = self.insts.remove(ix);
        self.emit(t, EventKind::CueEnded { cue: inst.cue });
        if let Some(target) = inst.stop_target_on_end {
            self.stop_cue(target, t, 0);
        }
        if inst.follow {
            if let Some(next) = inst.next {
                self.trigger(next, t, Trig::default());
            }
        }
        if let Some(parent) = inst.parent {
            self.child_done(parent, t);
        }
    }

    fn child_done(&mut self, parent: u64, t: u64) {
        let Some(ix) = self.idx(parent) else { return };
        let mut next_child = None;
        let done;
        {
            let Some(g) = self.insts[ix].group.as_mut() else {
                return;
            };
            g.live = g.live.saturating_sub(1);
            if g.mode == GroupMode::Playlist && g.idx + 1 < g.order.len() {
                g.idx += 1;
                g.live += 1;
                next_child = g.order.get(g.idx).copied();
            }
            done = g.live == 0;
        }
        if let Some(child) = next_child {
            if self
                .trigger(
                    child,
                    t,
                    Trig {
                        parent: Some(parent),
                        ..Trig::default()
                    },
                )
                .is_none()
            {
                self.child_done(parent, t);
            }
            return;
        }
        if done {
            if let Some(ix) = self.idx(parent) {
                self.insts[ix].action_done = true;
            }
            self.reap(parent, t);
        }
    }

    fn cue_error(&mut self, id: u64, t: u64, message: String) {
        if let Some(ix) = self.idx(id) {
            let inst = self.insts.remove(ix);
            for voice in inst.voices(t) {
                self.actions.push(Action::StopVoice {
                    voice,
                    at: t,
                    fade_frames: 0,
                    curve: Curve::Linear,
                    hard: false,
                });
            }
            for voice in inst.pending_voices() {
                self.actions.push(Action::DiscardVoice { voice });
            }
            self.errors.insert(inst.cue, message.clone());
            self.emit(
                t,
                EventKind::CueError {
                    cue: inst.cue,
                    message,
                },
            );
            if let Some(parent) = inst.parent {
                self.child_done(parent, t);
            }
        }
    }

    fn matrix(&self, routing: &[Crosspoint]) -> Vec<(u16, u16, f32)> {
        routing
            .iter()
            .filter_map(|x| {
                let out = self.show.patch.outputs.get(usize::from(x.out))?;
                Some((x.src, out.channel, db_to_lin(x.db)))
            })
            .collect()
    }

    /// Gain and routing matrix of every running audio voice, from the show.
    fn running_mix(&self) -> HashMap<VoiceId, VoiceMix> {
        let outputs = self.show.patch.outputs.len();
        self.insts
            .iter()
            .filter_map(|i| {
                let voice = i.voice?;
                let CueKind::Audio(a) = &self.show.cue(i.cue)?.kind else {
                    return None;
                };
                Some((
                    voice,
                    (
                        db_to_lin(a.gain_db),
                        self.matrix(&a.effective_routing(outputs)),
                    ),
                ))
            })
            .collect()
    }

    fn voice_spec(&self, cue: &Cue, offset_frames: u64) -> Option<VoiceSpec> {
        let CueKind::Audio(a) = &cue.kind else {
            return None;
        };
        Some(self.seg_spec(a, a.start.max(0.0), 0, offset_frames))
    }

    /// The voice for one stretch of an audio cue: from `from` (file seconds) on, through
    /// loop `k` (looping there) - or to the cue's end when there is no loop `k`.
    fn seg_spec(&self, a: &AudioCue, from: f64, k: usize, offset_frames: u64) -> VoiceSpec {
        let base = VoiceSpec {
            file: a.file.clone(),
            start: from,
            end: a.end,
            loop_count: a.loop_count,
            offset_frames,
            loop_from: None,
            loop_xfade: 0.0,
            start_frame: None,
            gain: db_to_lin(a.gain_db),
            matrix: self.matrix(&a.effective_routing(self.show.patch.outputs.len())),
        };
        let loops = a.active_loops();
        if loops.is_empty() {
            return base;
        }
        match loops.get(k) {
            Some(l) => VoiceSpec {
                end: Some(l.end),
                loop_count: 0,
                loop_from: Some(l.start),
                loop_xfade: l.crossfade,
                ..base
            },
            None => VoiceSpec {
                loop_count: 1,
                ..base
            },
        }
    }

    /// The loop cue instance `ix` plays: the cue's audio and its loops.
    fn inst_loops(&self, ix: usize) -> Option<(Cue, AudioCue, Vec<Loop>)> {
        let c = self.show.cue(self.insts.get(ix)?.cue)?.clone();
        let CueKind::Audio(a) = &c.kind else {
            return None;
        };
        let a = a.clone();
        let loops = a.active_loops();
        Some((c, a, loops))
    }

    /// Frames at the file's own rate, rounded as the file source does.
    fn file_rate(&self, a: &AudioCue) -> u64 {
        u64::from(a.file_rate.unwrap_or(self.sr).max(1))
    }

    /// The cue instance `ix` has just come to its loop `seg` (voice playing, `lp` set):
    /// release it at once when it was released ahead or plays a fixed number of passes,
    /// and keep the audio after it ready for an immediate release.
    fn enter_loop(&mut self, ix: usize, t: u64) {
        let Some((c, a, loops)) = self.inst_loops(ix) else {
            return;
        };
        let seg = self.insts[ix].seg;
        let Some(l) = loops.get(seg).cloned() else {
            return;
        };
        let ahead = self.insts[ix].skip.contains(&seg);
        if l.immediate && !ahead {
            let voice = self.alloc_voice();
            let spec = self.seg_spec(&a, l.end, seg + 1, 0);
            self.actions.push(Action::PrepareVoice {
                voice,
                cue: c.id,
                spec,
            });
            self.insts[ix].spare = Some(voice);
        }
        if ahead || l.passes > 0 {
            let min_k = if ahead { 0 } else { u64::from(l.passes - 1) };
            if let Err(e) = self.release_at(ix, t, min_k, false, !ahead) {
                self.emit(
                    t,
                    EventKind::Info {
                        message: format!("Release loop: {e}"),
                    },
                );
            }
        }
    }

    /// Release the loop cue `cue` plays (`name`: only that loop; a loop the cue has not
    /// reached yet is skipped when it comes).
    fn release_loop(&mut self, cue: CueId, name: Option<&str>, t: u64) -> Result<(), String> {
        let Some(ix) = self.insts.iter().position(|i| i.cue == cue) else {
            return Err("the cue is not running".into());
        };
        let Some((_, _, loops)) = self.inst_loops(ix) else {
            return Err("only audio cues have loops".into());
        };
        if loops.is_empty() {
            return Err("this cue has no loop".into());
        }
        let i = &self.insts[ix];
        let started = i.started && i.voice.is_some();
        let target = match name {
            Some(n) => loops
                .iter()
                .position(|l| l.name.eq_ignore_ascii_case(n.trim()))
                .ok_or_else(|| format!("the cue has no loop \"{}\"", n.trim()))?,
            None => i.seg,
        };
        if target >= loops.len() {
            return Err("the cue has no loop left".into());
        }
        if target < i.seg {
            return Err(format!("\"{}\" is already over", loops[target].name));
        }
        if !started || target > i.seg {
            // Not there yet (pre-wait, or a later loop): skip it when it comes.
            let i = &mut self.insts[ix];
            if !started && target == i.seg {
                i.seg += 1;
                while i.skip.contains(&i.seg) {
                    i.seg += 1;
                }
            } else if !i.skip.contains(&target) {
                i.skip.push(target);
            }
            let name = loops[target].name.clone();
            self.emit(
                t,
                EventKind::LoopReleased {
                    cue,
                    in_secs: 0.0,
                    name,
                },
            );
            return Ok(());
        }
        self.release_at(ix, t, 0, true, false).map(|_| ())
    }

    /// Release every loop playing (`name`: only loops of that name).
    fn release_loops(&mut self, name: Option<&str>, t: u64, list: Option<ListId>) -> usize {
        let cues: Vec<CueId> = self
            .insts
            .iter()
            .filter(|i| list.is_none_or(|l| i.list == l))
            .filter(|i| {
                // By name also a loop the cue comes to later; otherwise the loop playing.
                name.is_some()
                    || matches!(
                        i.vamp,
                        VampState::Looping | VampState::Switching { auto: true, .. }
                    )
            })
            .map(|i| i.cue)
            .collect();
        let mut n = 0;
        for cue in cues {
            if self.release_loop(cue, name, t).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// GO while loops play that GO releases: release them instead of starting the next
    /// cue. Returns how many were released.
    fn go_releases(&mut self, list: ListId, t: u64) -> usize {
        let cues: Vec<CueId> = self
            .insts
            .iter()
            .filter(|i| i.list == list && i.voice.is_some())
            .filter(|i| {
                matches!(
                    i.vamp,
                    VampState::Looping | VampState::Switching { auto: true, .. }
                )
            })
            .filter(|i| {
                let Some(CueKind::Audio(a)) = self.show.cue(i.cue).map(|c| &c.kind) else {
                    return false;
                };
                a.active_loops().get(i.seg).is_some_and(|l| l.release_on_go)
            })
            .map(|i| i.cue)
            .collect();
        cues.into_iter()
            .filter(|c| self.release_loop(*c, None, t).is_ok())
            .count()
    }

    /// Release the loop instance `ix` plays: right away (`now`, an immediate loop with its
    /// audio ready), or at the first loop end from `min_k` on that is far enough ahead to
    /// open the audio after the loop. `auto`: released by its number of passes.
    fn release_at(
        &mut self,
        ix: usize,
        t: u64,
        min_k: u64,
        now: bool,
        auto: bool,
    ) -> Result<f64, String> {
        let Some((c, a, loops)) = self.inst_loops(ix) else {
            return Err("only audio cues have loops".into());
        };
        let cue = c.id;
        match self.insts[ix].vamp {
            VampState::Looping => {}
            VampState::Switching {
                next, auto: true, ..
            } if !auto => {
                // Released by its passes, now by hand: earlier.
                self.actions.push(Action::DiscardVoice { voice: next });
                self.insts[ix].vamp = VampState::Looping;
            }
            _ => return Err("the loop is already released".into()),
        }
        let Some(lp) = self.insts[ix].lp else {
            return Err("the cue is not looping".into());
        };
        let seg = self.insts[ix].seg;
        let Some(l) = loops.get(seg).cloned() else {
            return Err("the cue has no loop left".into());
        };
        if self.insts[ix].voice.is_none() {
            return Err("the cue is not playing".into());
        }
        let pre = self.insts[ix].pre;
        let pos = self.insts[ix].local_at(t).saturating_sub(pre);
        let (rate_u, sr_u) = (lp.rate, lp.sr);
        let (rate, sr) = (rate_u as f64, sr_u as f64);
        let src = |secs: f64| (secs.max(0.0) * rate).round() as u64;
        let end_secs = a.end.or(a.file_duration);

        // Immediate: cross over to the audio after the loop now.
        if now && l.immediate {
            if let Some(next) = self.insts[ix].spare.take() {
                let xfade = self.fr(l.crossfade.max(0.05));
                let i = &mut self.insts[ix];
                let old = i.voice.replace(next);
                i.old_voice = old.map(|v| (v, t + xfade + 1));
                self.actions.push(Action::StartVoice {
                    voice: next,
                    at: t,
                    fade_in_frames: xfade,
                    curve: Curve::EqualPower,
                });
                if let Some(v) = old {
                    self.actions.push(Action::StopVoice {
                        voice: v,
                        at: t,
                        fade_frames: xfade,
                        curve: Curve::EqualPower,
                        hard: false,
                    });
                }
                let le = src(l.end);
                let rest = end_secs.map(|e| self.fr(e - l.end));
                self.next_segment(ix, t, pos, le, rest);
                self.emit(
                    t,
                    EventKind::LoopReleased {
                        cue,
                        in_secs: 0.0,
                        name: l.name.clone(),
                    },
                );
                return Ok(0.0);
            }
        }

        // At a loop end. The new voice starts `h_src` source frames before it, at an engine
        // frame that maps exactly onto a source frame: both rates meet every `q` source =
        // `p` engine frames (147 / 160 for 44.1 / 48 kHz), so the splice has no sub-sample
        // offset.
        let (intro, len) = (lp.intro, lp.len);
        if len == 0 {
            return Err("the loop is empty".into());
        }
        let g = gcd(rate_u, sr_u);
        let (q, p) = (rate_u / g, sr_u / g);
        let min_h = (VAMP_XFADE_HALF as f64 * rate / sr).ceil() as u64;
        let ready = pos + self.fr(0.5);
        let to_engine = |n: u64| -> u64 {
            if q <= 8192 {
                n / q * p
            } else {
                (n as f64 * sr / rate).round() as u64
            }
        };
        let mut k = min_k;
        let (start_src, h_src) = loop {
            let n = intro + k * len;
            let mut h = if q <= 8192 { n % q } else { min_h };
            while h < min_h {
                h += q;
            }
            if h <= n && lp.base + to_engine(n - h) >= ready {
                break (n - h, h);
            }
            k += 1;
        };
        let start = lp.base + to_engine(start_src);
        let h_eng = (h_src as f64 * sr / rate).round() as u64;
        if !auto {
            // Released for the loop end: the audio kept for an immediate release is not needed.
            if let Some(v) = self.insts[ix].spare.take() {
                self.actions.push(Action::DiscardVoice { voice: v });
            }
        }
        let next = self.alloc_voice();
        let mut spec = self.seg_spec(&a, l.end, seg + 1, 0);
        spec.start_frame = Some(lp.le - h_src);
        self.actions.push(Action::PrepareVoice {
            voice: next,
            cue,
            spec,
        });
        let rest = if seg + 1 < loops.len() {
            None
        } else {
            end_secs.map(|e| self.fr(e - l.end) + h_eng)
        };
        let at = pre + start;
        let i = &mut self.insts[ix];
        i.vamp = VampState::Switching {
            at,
            next,
            len: rest,
            xfade: 2 * h_eng,
            h_src,
            auto,
        };
        let in_secs = (i.global(at + h_eng).saturating_sub(t)) as f64 / sr;
        if !auto {
            self.emit(
                t,
                EventKind::LoopReleased {
                    cue,
                    in_secs,
                    name: l.name,
                },
            );
        }
        Ok(in_secs)
    }

    /// The voice of the next stretch plays from source frame `first` (its first frame is at
    /// `base`, engine frames after the pre-wait); `from` = the end of the loop just left.
    /// Sets up its loop (if any) or its length (`rest` frames after `base`).
    fn next_segment(&mut self, ix: usize, t: u64, base: u64, first: u64, rest: Option<u64>) {
        let Some((_, a, loops)) = self.inst_loops(ix) else {
            return;
        };
        let rate = self.file_rate(&a);
        let sr = u64::from(self.sr.max(1));
        let src = |secs: f64| (secs.max(0.0) * rate as f64).round() as u64;
        let i = &mut self.insts[ix];
        i.seg += 1;
        i.origin = (base, first as f64 / rate as f64);
        let seg = i.seg;
        match loops.get(seg) {
            Some(l) => {
                let (ls, le) = (src(l.start), src(l.end));
                i.vamp = VampState::Looping;
                i.lp = Some(LoopRt {
                    index: seg,
                    base,
                    intro: le.saturating_sub(first),
                    len: le.saturating_sub(ls),
                    rate,
                    sr,
                    src0: first,
                    ls,
                    le,
                    next: 0,
                    tc: l.timecode,
                });
                i.dur = None;
                self.enter_loop(ix, t);
            }
            None => {
                i.vamp = VampState::None;
                i.lp = None;
                let pre = i.pre;
                i.dur = rest.map(|r| base + r);
                let fade = (self.fr(a.fade_out), a.fade_curve);
                let i = &mut self.insts[ix];
                if let (Some(d), true) = (i.dur, fade.0 > 0) {
                    let pos = i.local_at(t).saturating_sub(pre);
                    i.fade_frames = fade.0.min(d.saturating_sub(pos));
                    i.fade_curve = fade.1;
                    i.fade_at = Some(pre + d - i.fade_frames);
                }
            }
        }
    }

    /// A loop end of a loop the timecode follows: the generator jumps back by
    /// exactly the length of the pass, so it keeps matching the audio.
    fn vamp_wrap(&mut self, ix: usize, t: u64) {
        let Some(w) = self.insts[ix].lp else {
            return;
        };
        let back = w.pass(w.next);
        if let Some(w) = &mut self.insts[ix].lp {
            w.next += 1;
        }
        let old = self.tc;
        if self.chase.is_some() || !old.running || t < old.anchor_sample + back {
            return;
        }
        // Timecode shown `back` samples ago, anchored where that frame began
        // (shifted by `back`): from here on the generator repeats the pass.
        let sr = self.sr;
        let e = old.frames_elapsed(t - back, sr);
        let Some(frame_start) = old.sample_after_frames(e, sr) else {
            return;
        };
        let day = old.rate.frames_per_day();
        let pos = old.frame_at(t, sr);
        let new = TcTransport {
            rate: old.rate,
            running: true,
            anchor_sample: frame_start + back,
            anchor_frame: (old.anchor_frame + e) % day,
        };
        self.tc = new;
        self.tc_next_k = 0;
        self.tc_pos_fired = false;
        self.actions.push(Action::Timecode(new));
        self.tc_jump(pos, new.anchor_frame, true, t);
    }

    /// The switch point of a released loop: cross over to the audio after it.
    fn vamp_switch(&mut self, ix: usize, t: u64) {
        let VampState::Switching {
            at,
            next,
            len,
            xfade,
            h_src,
            ..
        } = self.insts[ix].vamp
        else {
            return;
        };
        let Some(lp) = self.insts[ix].lp else {
            return;
        };
        let i = &mut self.insts[ix];
        i.vamp = VampState::None;
        if let Some(v) = i.spare.take() {
            self.actions.push(Action::DiscardVoice { voice: v });
        }
        let i = &mut self.insts[ix];
        let old = i.voice.replace(next);
        i.old_voice = old.map(|v| (v, t + xfade + 1));
        let pre = i.pre;
        self.actions.push(Action::StartVoice {
            voice: next,
            at: t,
            fade_in_frames: xfade,
            curve: Curve::Linear,
        });
        if let Some(v) = old {
            self.actions.push(Action::StopVoice {
                voice: v,
                at: t,
                fade_frames: xfade,
                curve: Curve::Linear,
                hard: false,
            });
        }
        self.next_segment(ix, t, at - pre, lp.le - h_src, len);
    }

    fn alloc_voice(&mut self) -> VoiceId {
        let v = self.next_voice;
        self.next_voice += 1;
        v
    }

    fn start_action(&mut self, id: u64, cue: &Cue, t: u64) {
        let Some(ix) = self.idx(id) else { return };
        let offset = self.insts[ix]
            .local_at(t)
            .saturating_sub(self.insts[ix].pre);
        let pre = self.insts[ix].pre;
        match &cue.kind {
            CueKind::Memo => self.insts[ix].action_done = true,
            CueKind::Audio(a) => {
                if a.file.is_empty() {
                    self.cue_error(id, t, "no audio file assigned".into());
                    return;
                }
                let loops = a.active_loops();
                let seg = self.insts[ix].seg.min(loops.len());
                let looping = seg < loops.len();
                let dur = if loops.is_empty() {
                    a.duration()
                } else if looping {
                    None
                } else {
                    a.region()
                }
                .map(|d| self.fr(d));
                if dur.is_some_and(|d| offset >= d) {
                    self.insts[ix].action_done = true;
                    return;
                }
                let standby = self.standby.remove(&cue.id);
                let voice = match standby {
                    Some((v, off)) if off == offset && seg == 0 => v,
                    other => {
                        if let Some((v, _)) = other {
                            self.actions.push(Action::DiscardVoice { voice: v });
                        }
                        let v = self.alloc_voice();
                        let spec = self.seg_spec(a, a.start.max(0.0), seg, offset);
                        self.actions.push(Action::PrepareVoice {
                            voice: v,
                            cue: cue.id,
                            spec,
                        });
                        v
                    }
                };
                self.standby_dirty = true;
                let fade_in = if offset == 0 { self.fr(a.fade_in) } else { 0 };
                self.actions.push(Action::StartVoice {
                    voice,
                    at: t,
                    fade_in_frames: fade_in,
                    curve: a.fade_curve,
                });
                let rate = self.file_rate(a);
                let src = |secs: f64| (secs.max(0.0) * rate as f64).round() as u64;
                let s0 = src(a.start);
                let lp = loops.get(seg).and_then(|l| {
                    let (ls, le) = (src(l.start).max(s0), src(l.end));
                    (le > ls).then(|| {
                        let mut w = LoopRt {
                            index: seg,
                            base: 0,
                            intro: le - s0,
                            len: le - ls,
                            rate,
                            sr: u64::from(self.sr.max(1)),
                            src0: s0,
                            ls,
                            le,
                            next: 0,
                            tc: l.timecode,
                        };
                        // Started inside the loop (load to time): skip passed ends.
                        while w.end(w.next) <= offset {
                            w.next += 1;
                        }
                        w
                    })
                });
                let fade = self.fr(a.fade_out);
                let i = &mut self.insts[ix];
                i.voice = Some(voice);
                i.seg = seg;
                i.origin = (0, a.start.max(0.0));
                i.vamp = if lp.is_some() {
                    VampState::Looping
                } else {
                    VampState::None
                };
                i.lp = lp;
                i.dur = dur;
                if let (Some(d), true) = (dur, fade > 0) {
                    i.fade_frames = fade.min(d - offset);
                    i.fade_curve = a.fade_curve;
                    i.fade_at = Some(pre + d - i.fade_frames);
                }
                if lp.is_some() {
                    self.enter_loop(ix, t);
                }
            }
            CueKind::Fade(f) => {
                let Some(target) = f.target.filter(|c| self.show.cue(*c).is_some()) else {
                    self.cue_error(id, t, "fade has no valid target".into());
                    return;
                };
                let frames = self.fr(f.duration);
                let crosspoints = self.matrix(&f.crosspoints);
                let master = f.gain_db.map(db_to_lin);
                for voice in self.voices_of(target) {
                    self.actions.push(Action::FadeVoice {
                        voice,
                        at: t,
                        frames,
                        curve: f.curve,
                        master,
                        crosspoints: crosspoints.clone(),
                    });
                }
                let i = &mut self.insts[ix];
                i.dur = Some(frames);
                if f.stop_target {
                    i.stop_target_on_end = Some(target);
                }
            }
            CueKind::Midi(m) => {
                match m.message.to_bytes() {
                    Ok(bytes) => {
                        let port = self.show.resolve_midi_port(&m.port).to_string();
                        self.actions.push(Action::Midi { at: t, port, bytes });
                    }
                    Err(e) => {
                        self.cue_error(id, t, e);
                        return;
                    }
                }
                self.insts[ix].action_done = true;
            }
            CueKind::Osc(m) => {
                match m.encode() {
                    Ok(bytes) => self.actions.push(Action::Osc {
                        at: t,
                        target: m.target(),
                        bytes,
                    }),
                    Err(e) => {
                        self.cue_error(id, t, e);
                        return;
                    }
                }
                self.insts[ix].action_done = true;
            }
            CueKind::Http(r) => {
                match r.encode() {
                    Ok((target, bytes)) => self.actions.push(Action::Http {
                        at: t,
                        addr: target.addr(),
                        bytes,
                        summary: r.summary(),
                    }),
                    Err(e) => {
                        self.cue_error(id, t, e);
                        return;
                    }
                }
                self.insts[ix].action_done = true;
            }
            CueKind::Timecode(tc) => {
                self.insts[ix].action_done = true;
                self.tc_action(&tc.tc, t);
            }
            CueKind::Control(c) => {
                self.insts[ix].action_done = true;
                let Some(target) = c.target.filter(|x| self.show.cue(*x).is_some()) else {
                    self.cue_error(id, t, "control cue has no valid target".into());
                    return;
                };
                match c.action {
                    ControlAction::Start => {
                        self.trigger(target, t, Trig::default());
                    }
                    ControlAction::Stop => {
                        let fade = self.fr(c.time);
                        self.stop_cue(target, t, fade);
                    }
                    ControlAction::Pause => self.pause_cue(target, t, true),
                    ControlAction::Resume => self.pause_cue(target, t, false),
                    ControlAction::LoadToTime => self.load_cue(target, c.time, t),
                    ControlAction::Arm => self.arm(target, true, t),
                    ControlAction::Disarm => self.arm(target, false, t),
                    ControlAction::GoTo => {
                        if let Some((list, _)) = self.show.find(target) {
                            self.set_playhead(list, Some(target), t);
                        }
                    }
                    ControlAction::Reset => self.reset_cue(target, t),
                    ControlAction::ReleaseLoop => {
                        let name = Some(c.loop_name.trim()).filter(|n| !n.is_empty());
                        if let Err(e) = self.release_loop(target, name, t) {
                            self.emit(
                                t,
                                EventKind::Info {
                                    message: format!("Release loop: {e}"),
                                },
                            );
                        }
                    }
                }
            }
            CueKind::Group(g) => {
                let children: Vec<CueId> = g.children.iter().map(|c| c.id).collect();
                if children.is_empty() {
                    self.insts[ix].action_done = true;
                    return;
                }
                if g.mode == GroupMode::List {
                    // A list fires nothing by itself (its cues take their own GO).
                    self.insts[ix].action_done = true;
                    return;
                }
                let order = match g.mode {
                    GroupMode::Parallel | GroupMode::Playlist | GroupMode::List => children,
                    GroupMode::Random => {
                        let pick = (self.rand() % children.len() as u64) as usize;
                        vec![children[pick]]
                    }
                };
                let first: Vec<CueId> = match g.mode {
                    GroupMode::Parallel => order.clone(),
                    _ => order.first().copied().into_iter().collect(),
                };
                let i = &mut self.insts[ix];
                i.dur = None;
                i.group = Some(GroupRt {
                    mode: g.mode,
                    order,
                    idx: 0,
                    live: first.len(),
                });
                for child in first {
                    let trig = Trig {
                        parent: Some(id),
                        initial_local: offset,
                        from_tc: false,
                        chase: offset > 0,
                    };
                    if self.trigger(child, t, trig).is_none() {
                        self.child_done(id, t);
                    }
                }
            }
        }
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Ids of the instance for `cue` and all its descendants.
    fn family(&self, cue: CueId) -> Vec<u64> {
        let mut ids: Vec<u64> = self
            .insts
            .iter()
            .filter(|i| i.cue == cue)
            .map(|i| i.id)
            .collect();
        let mut n = 0;
        while n < ids.len() {
            let p = ids[n];
            ids.extend(
                self.insts
                    .iter()
                    .filter(|i| i.parent == Some(p))
                    .map(|i| i.id),
            );
            n += 1;
        }
        ids
    }

    fn voices_of(&self, cue: CueId) -> Vec<VoiceId> {
        let fam = self.family(cue);
        self.insts
            .iter()
            .filter(|i| fam.contains(&i.id))
            .flat_map(|i| i.voices(self.now))
            .collect()
    }

    fn stop_inst(&mut self, id: u64, t: u64, fade_frames: u64, hard: bool, notify_parent: bool) {
        let Some(ix) = self.idx(id) else { return };
        let cue = self.insts[ix].cue;
        let parent = self.insts[ix].parent;
        let fam = self.family(cue);
        let mut removed = Vec::new();
        self.insts.retain(|i| {
            if fam.contains(&i.id) {
                removed.push((i.cue, i.voices(t).collect::<Vec<_>>(), i.pending_voices()));
                false
            } else {
                true
            }
        });
        for (cue, voices, pending) in removed {
            for voice in voices {
                self.actions.push(Action::StopVoice {
                    voice,
                    at: t,
                    fade_frames,
                    curve: Curve::EqualPower,
                    hard,
                });
            }
            for voice in pending {
                self.actions.push(Action::DiscardVoice { voice });
            }
            self.emit(t, EventKind::CueStopped { cue });
        }
        if notify_parent {
            if let Some(p) = parent {
                self.child_done(p, t);
            }
        }
    }

    fn stop_cue(&mut self, cue: CueId, t: u64, fade_frames: u64) {
        if let Some(id) = self.insts.iter().find(|i| i.cue == cue).map(|i| i.id) {
            self.stop_inst(id, t, fade_frames, false, true);
        }
    }

    fn pause_cue(&mut self, cue: CueId, t: u64, pause: bool) {
        for id in self.family(cue) {
            self.pause_inst(id, t, pause);
        }
    }

    fn pause_inst(&mut self, id: u64, t: u64, pause: bool) {
        let Some(ix) = self.idx(id) else { return };
        let i = &mut self.insts[ix];
        if i.paused == pause {
            return;
        }
        let cue = i.cue;
        if pause {
            i.local = i.local_at(t);
            i.paused = true;
            let voices: Vec<VoiceId> = i.voices(t).collect();
            for voice in voices {
                self.actions.push(Action::PauseVoice { voice, at: t });
            }
            self.emit(t, EventKind::CuePaused { cue });
        } else {
            i.since = t;
            i.paused = false;
            // A vamp voice paused during its fade-out is resumed to finish it.
            if let Some((_, until)) = &mut i.old_voice {
                *until = t + 4 * VAMP_XFADE_HALF + 1;
            }
            let voices: Vec<VoiceId> = i.voices(t).collect();
            for voice in voices {
                self.actions.push(Action::ResumeVoice { voice, at: t });
            }
            self.emit(t, EventKind::CueResumed { cue });
        }
    }

    fn load_cue(&mut self, cue: CueId, time: f64, t: u64) {
        if self.show.cue(cue).is_none() {
            return;
        }
        let frames = self.fr(time);
        if frames == 0 {
            self.loaded.remove(&cue);
        } else {
            self.loaded.insert(cue, frames);
        }
        self.standby_dirty = true;
        self.emit(
            t,
            EventKind::CueLoaded {
                cue,
                time: time.max(0.0),
            },
        );
    }

    fn arm(&mut self, cue: CueId, armed: bool, t: u64) {
        if let Some(c) = self.show.cue_mut(cue) {
            if c.armed != armed {
                c.armed = armed;
                self.rebuild_tc_triggers();
                self.emit(t, EventKind::CueArmed { cue, armed });
            }
        }
    }

    fn reset_cue(&mut self, cue: CueId, t: u64) {
        self.stop_cue(cue, t, 0);
        self.loaded.remove(&cue);
        self.errors.remove(&cue);
        self.standby_dirty = true;
    }

    fn set_playhead(&mut self, list: ListId, cue: Option<CueId>, t: u64) {
        let valid = match cue {
            None => true,
            Some(c) => self
                .show
                .list(list)
                .is_some_and(|l| l.step_index(c).is_some()),
        };
        if !valid {
            return;
        }
        if self.playheads.insert(list, cue) != Some(cue) {
            self.standby_dirty = true;
            self.emit(t, EventKind::Playhead { list, cue });
        }
    }

    /// The cue after the continue-chain starting at index `i`.
    fn after_chain(&self, list: ListId, mut i: usize) -> Option<CueId> {
        let steps = self.show.list(list)?.steps();
        while steps
            .get(i)
            .is_some_and(|c| c.continue_mode != ContinueMode::DoNotContinue)
        {
            i += 1;
        }
        steps.get(i + 1).map(|c| c.id)
    }

    // ------------------------------------------------------------------
    // Standby (pre-opened voices for the cues GO will fire next)
    // ------------------------------------------------------------------

    fn refresh_standby(&mut self) {
        if !self.standby_dirty {
            return;
        }
        self.standby_dirty = false;
        let mut want: Vec<CueId> = Vec::new();
        fn collect(cue: &Cue, want: &mut Vec<CueId>) {
            match &cue.kind {
                CueKind::Audio(a) if !a.file.is_empty() && cue.armed => want.push(cue.id),
                CueKind::Group(g) => match g.mode {
                    GroupMode::Playlist => {
                        if let Some(c) = g.children.first() {
                            collect(c, want);
                        }
                    }
                    _ => g.children.iter().for_each(|c| collect(c, want)),
                },
                _ => {}
            }
        }
        for l in &self.show.cue_lists {
            let Some(Some(head)) = self.playheads.get(&l.id) else {
                continue;
            };
            let steps = l.steps();
            let Some(mut i) = steps.iter().position(|c| c.id == *head) else {
                continue;
            };
            while let Some(cue) = steps.get(i).copied() {
                collect(cue, &mut want);
                // Control cues that start another cue make that cue part of the GO.
                if let CueKind::Control(c) = &cue.kind {
                    if c.action == ControlAction::Start {
                        if let Some(target) = c.target.and_then(|t| self.show.cue(t)) {
                            collect(target, &mut want);
                        }
                    }
                }
                if cue.continue_mode == ContinueMode::DoNotContinue {
                    break;
                }
                i += 1;
            }
        }
        want.retain(|c| !self.is_active(*c));
        want.truncate(MAX_STANDBY);
        let stale: Vec<CueId> = self
            .standby
            .iter()
            .filter(|(c, (_, off))| {
                !want.contains(c) || self.loaded.get(c).copied().unwrap_or(0) != *off
            })
            .map(|(c, _)| *c)
            .collect();
        for c in stale {
            if let Some((voice, _)) = self.standby.remove(&c) {
                self.actions.push(Action::DiscardVoice { voice });
            }
        }
        for c in want {
            if self.standby.contains_key(&c) {
                continue;
            }
            let offset = self.loaded.get(&c).copied().unwrap_or(0);
            let Some(spec) = self
                .show
                .cue(c)
                .and_then(|cue| self.voice_spec(cue, offset))
            else {
                continue;
            };
            let voice = self.alloc_voice();
            self.actions.push(Action::PrepareVoice {
                voice,
                cue: c,
                spec,
            });
            self.standby.insert(c, (voice, offset));
        }
    }

    /// The I/O layer reports that a voice could not be opened or failed.
    pub fn voice_error(&mut self, voice: VoiceId, message: String) {
        let t = self.now;
        // The audio kept ready for an immediate release could not be opened: release at
        // the loop end instead.
        if let Some(i) = self.insts.iter_mut().find(|i| i.spare == Some(voice)) {
            i.spare = None;
            return;
        }
        // The audio after a released loop could not be opened: keep looping.
        if let Some(i) = self
            .insts
            .iter_mut()
            .find(|i| matches!(i.vamp, VampState::Switching { next, .. } if next == voice))
        {
            i.vamp = VampState::Looping;
            self.emit(
                t,
                EventKind::Info {
                    message: format!("Release loop failed, the cue keeps looping: {message}"),
                },
            );
            return;
        }
        if let Some(id) = self
            .insts
            .iter()
            .find(|i| i.voice == Some(voice))
            .map(|i| i.id)
        {
            if let Some(ix) = self.idx(id) {
                self.insts[ix].voice = None;
            }
            self.cue_error(id, t, message);
        } else if let Some(cue) = self
            .standby
            .iter()
            .find(|(_, (v, _))| *v == voice)
            .map(|(c, _)| *c)
        {
            // Broken before it was ever fired: flag the cue, keep the entry so
            // we do not retry on every tick.
            self.errors.insert(cue, message.clone());
            self.emit(t, EventKind::CueError { cue, message });
        }
    }

    // ------------------------------------------------------------------
    // Timecode
    // ------------------------------------------------------------------

    fn rebuild_tc_triggers(&mut self) {
        let rate = self.show.timecode.rate;
        let mut v = Vec::new();
        for l in &self.show.cue_lists {
            for c in &l.cues {
                if let (Some(tc), true) = (c.tc_trigger, c.armed) {
                    v.push((tc.to_frame_number(rate), c.id, l.id));
                }
            }
        }
        v.sort();
        self.tc_triggers = v;
    }

    fn next_tc_trigger(&self) -> Option<(u64, u64)> {
        if !self.tc.running {
            return None;
        }
        let from = self.tc.anchor_frame + self.tc_next_k;
        let ix = self.tc_triggers.partition_point(|(f, _, _)| *f < from);
        let (f, _, _) = self.tc_triggers.get(ix)?;
        let k = f - self.tc.anchor_frame;
        self.tc.sample_after_frames(k, self.sr).map(|s| (s, k))
    }

    fn fire_tc_triggers(&mut self, t: u64) {
        let Some((_, k)) = self.next_tc_trigger() else {
            return;
        };
        let frame = self.tc.anchor_frame + k;
        self.tc_next_k = k + 1;
        let cues: Vec<CueId> = self
            .tc_triggers
            .iter()
            .filter(|(f, _, _)| *f == frame)
            .map(|(_, c, _)| *c)
            .collect();
        for cue in cues {
            self.trigger(
                cue,
                t,
                Trig {
                    from_tc: true,
                    ..Trig::default()
                },
            );
        }
    }

    fn tc_frames_to_samples(&self, frames: u64) -> u64 {
        let (num, den) = self.tc.rate.rational();
        (u128::from(frames) * u128::from(self.sr) * u128::from(den) / u128::from(num)) as u64
    }

    fn tc_action(&mut self, action: &TcAction, t: u64) {
        if self.chase.is_some() {
            self.tc_action_external(action, t);
            return;
        }
        let rate = self.show.timecode.rate;
        let pos = self.tc.frame_at(t, self.sr);
        let was_running = self.tc.running;
        let start = self.show.timecode.start.to_frame_number(rate);
        let (new, name, jump) = match action {
            TcAction::Start => (
                TcTransport {
                    rate,
                    running: true,
                    anchor_sample: t,
                    anchor_frame: start,
                },
                "start",
                true,
            ),
            TcAction::StartFrom { from } => (
                TcTransport {
                    rate,
                    running: true,
                    anchor_sample: t,
                    anchor_frame: from.to_frame_number(rate),
                },
                "start",
                true,
            ),
            TcAction::Stop => (
                TcTransport {
                    rate,
                    running: false,
                    anchor_sample: t,
                    anchor_frame: start,
                },
                "stop",
                false,
            ),
            TcAction::Pause => {
                if !was_running {
                    return;
                }
                (
                    TcTransport {
                        rate,
                        running: false,
                        anchor_sample: t,
                        anchor_frame: pos,
                    },
                    "pause",
                    false,
                )
            }
            TcAction::Resume => {
                if was_running {
                    return;
                }
                (
                    TcTransport {
                        rate,
                        running: true,
                        anchor_sample: t,
                        anchor_frame: pos,
                    },
                    "resume",
                    false,
                )
            }
            TcAction::Relocate { to } => (
                TcTransport {
                    rate,
                    running: was_running,
                    anchor_sample: t,
                    anchor_frame: to.to_frame_number(rate),
                },
                "relocate",
                true,
            ),
        };
        let resume = matches!(action, TcAction::Resume);
        self.tc = new;
        self.tc_next_k = u64::from(resume && self.tc_pos_fired);
        self.tc_pos_fired = matches!(action, TcAction::Pause) || (resume && self.tc_pos_fired);
        self.tc_paused_by_transport = false;
        self.actions.push(Action::Timecode(new));
        let position = Timecode::from_frame_number(new.anchor_frame, rate).format(rate);
        self.emit(
            t,
            EventKind::Timecode {
                action: name.into(),
                position,
            },
        );
        if jump && new.running {
            self.tc_jump(pos, new.anchor_frame, was_running, t);
        }
    }

    fn tc_jump(&mut self, old: u64, new: u64, was_running: bool, t: u64) {
        let lists: Vec<(ListId, TcJumpPolicy)> = self
            .show
            .cue_lists
            .iter()
            .map(|l| (l.id, l.tc_jump))
            .collect();
        for (list, policy) in lists {
            let triggers: Vec<(u64, CueId)> = self
                .tc_triggers
                .iter()
                .filter(|(_, _, l)| *l == list)
                .map(|(f, c, _)| (*f, *c))
                .collect();
            match policy {
                TcJumpPolicy::Skip => {}
                TcJumpPolicy::Fire => {
                    if was_running && new > old {
                        for (f, cue) in triggers {
                            if f > old && f < new {
                                self.trigger(
                                    cue,
                                    t,
                                    Trig {
                                        from_tc: true,
                                        ..Trig::default()
                                    },
                                );
                            }
                        }
                    }
                }
                TcJumpPolicy::Chase => {
                    // Stop timecode-started cues that lie in the future of the new position.
                    let late: Vec<u64> = self
                        .insts
                        .iter()
                        .filter(|i| {
                            i.from_tc
                                && i.list == list
                                && triggers.iter().any(|(f, c)| *c == i.cue && *f > new)
                        })
                        .map(|i| i.id)
                        .collect();
                    for id in late {
                        self.stop_inst(id, t, 0, false, true);
                    }
                    for (f, cue) in triggers {
                        if f >= new || self.is_active(cue) {
                            continue;
                        }
                        let offset = self.tc_frames_to_samples(new - f);
                        let Some(c) = self.show.cue(cue) else {
                            continue;
                        };
                        let total = c
                            .action_duration()
                            .map(|d| self.fr(c.pre_wait) + self.fr(d));
                        let open_group = matches!(c.kind, CueKind::Group(_));
                        if total.is_none_or(|d| offset < d)
                            && (total.is_some()
                                || open_group
                                || matches!(c.kind, CueKind::Audio(_)))
                        {
                            self.trigger(
                                cue,
                                t,
                                Trig {
                                    from_tc: true,
                                    initial_local: offset.max(1),
                                    chase: true,
                                    parent: None,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // External timecode (chase)
    // ------------------------------------------------------------------

    /// The generator follows the show transport (start with GO etc.).
    fn transport_coupled(&self) -> bool {
        self.chase.is_none() && self.show.timecode.mode == TcMode::Transport
    }

    /// Generator, or the state of the external timecode input.
    pub fn tc_status(&self) -> TcStatus {
        self.chase
            .as_ref()
            .map_or(TcStatus::Generator, Chaser::status)
    }

    /// Build, keep or drop the chaser to match the show settings.
    fn configure_chase(&mut self, t: u64) {
        let tc = &self.show.timecode;
        let key = (tc.source == TcSource::External).then_some((
            tc.rate,
            tc.input.kind,
            tc.input.lock_frames,
            tc.input.freewheel_ms,
        ));
        if key == self.chase_key {
            return;
        }
        let was_external = self.chase_key.is_some();
        self.chase_key = key;
        let rate = tc.rate;
        let start = tc.start.to_frame_number(rate);
        self.chase =
            key.map(|(rate, _, lock, freewheel)| Chaser::new(rate, self.sr, lock, freewheel));
        // Switching source (or chase settings): the timecode stops; a
        // generator rests at its start offset, a chaser where it was.
        let anchor_frame = if self.chase.is_some() {
            self.tc.frame_at(t, self.sr) % rate.frames_per_day()
        } else {
            start
        };
        if self.tc.running || was_external != self.chase.is_some() || self.tc.rate != rate {
            self.tc = TcTransport {
                rate,
                running: false,
                anchor_sample: t,
                anchor_frame,
            };
            self.tc_next_k = 0;
            self.tc_pos_fired = false;
            self.tc_paused_by_transport = false;
            self.actions.push(Action::Timecode(self.tc));
            let action = if self.chase.is_some() {
                "source external"
            } else {
                "source generator"
            };
            self.emit(
                t,
                EventKind::Timecode {
                    action: action.into(),
                    position: Timecode::from_frame_number(anchor_frame, rate).format(rate),
                },
            );
        }
    }

    /// Feed one frame of external timecode (ignored in generator mode).
    /// `obs.at` is engine time; values in the future count as "now".
    pub fn external_tc(&mut self, mut obs: TcObservation) {
        let t = self.now;
        // Input latency (converters, MIDI interfaces): the frame started earlier.
        let latency = i64::from(self.show.timecode.input.latency_ms) * i64::from(self.sr) / 1000;
        obs.at = if latency >= 0 {
            obs.at.saturating_sub(latency as u64)
        } else {
            obs.at.saturating_add(latency.unsigned_abs())
        };
        obs.at = obs.at.min(t);
        let Some(chase) = self.chase.as_mut() else {
            return;
        };
        let step = chase.observe(obs);
        self.apply_chase(step, t);
        self.process_due(t);
    }

    /// The external source stopped or located to `frame` (MTC Full Frame).
    pub fn external_tc_stopped(&mut self, frame: u64) {
        let t = self.now;
        let Some(chase) = self.chase.as_mut() else {
            return;
        };
        let step = chase.full_frame(frame);
        self.apply_chase(step, t);
    }

    fn chase_tick(&mut self, now: u64) {
        let Some(chase) = self.chase.as_mut() else {
            return;
        };
        let step = chase.tick(now);
        self.apply_chase(step, now);
    }

    fn tc_event(&mut self, t: u64, action: &str, frame: u64) {
        let rate = self.tc.rate;
        self.emit(
            t,
            EventKind::Timecode {
                action: action.into(),
                position: Timecode::from_frame_number(frame, rate).format(rate),
            },
        );
    }

    /// Stop the engine timecode at `frame` (shown, not running).
    fn ext_stop(&mut self, frame: u64, t: u64) {
        self.tc = TcTransport {
            rate: self.tc.rate,
            running: false,
            anchor_sample: t,
            anchor_frame: frame,
        };
        self.tc_next_k = 0;
        self.actions.push(Action::Timecode(self.tc));
    }

    fn apply_chase(&mut self, step: ChaseStep, t: u64) {
        match step {
            ChaseStep::Nothing => {}
            ChaseStep::Lock(new) => self.ext_transport(new, true, "ext locked", t),
            ChaseStep::Jump(new) => self.ext_transport(new, true, "ext jump", t),
            ChaseStep::Reanchor(new, _) => self.ext_transport(new, false, "ext re-anchored", t),
            ChaseStep::Relocked => {
                let pos = self.tc.frame_at(t, self.sr);
                self.tc_event(t, "ext locked again", pos);
            }
            ChaseStep::Freewheel => {
                let pos = self.tc.frame_at(t, self.sr);
                self.tc_event(t, "ext input interrupted - freewheel", pos);
            }
            ChaseStep::Lost => {
                let pos = self.tc.frame_at(t, self.sr);
                self.ext_stop(pos, t);
                self.tc_event(t, "ext LOST - stopped", pos);
            }
            ChaseStep::Stop(frame) => {
                let moved = self.tc.running || self.tc.anchor_frame != frame;
                self.ext_stop(frame, t);
                if moved {
                    self.tc_event(t, "ext stopped", frame);
                }
            }
            ChaseStep::Reverse(frame) => {
                // Follow the position quietly; one event when it starts.
                let started = self.tc.running;
                self.ext_stop(frame, t);
                if started {
                    self.tc_event(t, "ext running backwards - stopped, nothing fires", frame);
                }
            }
        }
    }

    /// Take over a transport from the chaser. `jump` = apply the lists'
    /// jump policies (lock, discontinuity); otherwise it is a drift
    /// correction: triggers already passed are not fired again and triggers
    /// skipped by a forward correction fire now.
    fn ext_transport(&mut self, new: TcTransport, jump: bool, what: &str, t: u64) {
        let day = new.rate.frames_per_day();
        let old = self.tc;
        let old_pos = old.frame_at(t, self.sr);
        let new_pos = new.frame_at(t, self.sr);
        self.tc = new;
        self.tc_pos_fired = false;
        self.tc_paused_by_transport = false;
        self.actions.push(Action::Timecode(new));
        if jump {
            // The trigger on the current frame fires through the normal path.
            self.tc_next_k = (new_pos + day - new.anchor_frame) % day;
            self.tc_event(t, what, new_pos);
            self.tc_jump(old_pos, new_pos, old.running, t);
            return;
        }
        let fired_upto = old_pos;
        if new.anchor_frame > fired_upto + 1 {
            let skipped: Vec<CueId> = self
                .tc_triggers
                .iter()
                .filter(|(f, _, _)| *f > fired_upto && *f < new.anchor_frame)
                .map(|(_, c, _)| *c)
                .collect();
            for cue in skipped {
                self.trigger(
                    cue,
                    t,
                    Trig {
                        from_tc: true,
                        ..Trig::default()
                    },
                );
            }
            self.tc_next_k = 0;
        } else {
            self.tc_next_k = fired_upto + 1 - new.anchor_frame;
        }
        self.tc_event(t, what, new_pos);
    }

    /// Timecode commands and cues while chasing: stop / pause suspend
    /// following, start / resume continue it, relocation is not possible.
    fn tc_action_external(&mut self, action: &TcAction, t: u64) {
        let Some(chase) = self.chase.as_mut() else {
            return;
        };
        match action {
            TcAction::Stop | TcAction::Pause => {
                if chase.held() {
                    return;
                }
                chase.hold();
                let pos = self.tc.frame_at(t, self.sr);
                self.ext_stop(pos, t);
                self.tc_event(t, "ext held (not following)", pos);
            }
            TcAction::Start | TcAction::StartFrom { .. } | TcAction::Resume => {
                if !chase.held() {
                    return;
                }
                chase.release();
                let pos = self.tc.frame_at(t, self.sr);
                self.tc_event(t, "ext following again", pos);
            }
            TcAction::Relocate { .. } => self.emit(
                t,
                EventKind::Info {
                    message: "timecode follows an external source - relocate ignored".into(),
                },
            ),
        }
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    /// Execute a command at the current logical time.
    pub fn command(&mut self, cmd: Command) -> Result<(), String> {
        let t = self.now;
        let result = self.exec(cmd, t);
        self.process_due(t);
        self.refresh_standby();
        result
    }

    fn need_cue(&self, cue: CueId) -> Result<(), String> {
        self.show
            .cue(cue)
            .map(|_| ())
            .ok_or_else(|| format!("cue {cue} does not exist"))
    }

    fn exec(&mut self, cmd: Command, t: u64) -> Result<(), String> {
        match cmd {
            Command::Go { list } => self.go(list, t),
            Command::StopAll => {
                let fade = self.fr(f64::from(self.show.settings.stop_fade_ms) / 1000.0);
                self.stop_everything(t, fade, false);
                self.emit(t, EventKind::StopAll);
                Ok(())
            }
            Command::Panic => {
                let fade = self.fr(f64::from(self.show.settings.panic_fade_ms) / 1000.0);
                self.stop_everything(t, fade, fade == 0);
                // A free-running generator would keep sending LTC/MTC and fire
                // timecode-triggered cues again: hold it where it is (resume
                // continues from there).
                if self.tc.running || self.chase.is_some() {
                    self.tc_action(&TcAction::Pause, t);
                }
                self.actions.push(Action::MidiPanic);
                // Extra messages from the show settings, after All Notes Off.
                for m in self.show.settings.panic_send.clone() {
                    match m {
                        PanicMessage::Midi { port, message } => match message.to_bytes() {
                            Ok(bytes) => {
                                let port = self.show.resolve_midi_port(&port).to_string();
                                self.actions.push(Action::Midi { at: t, port, bytes });
                            }
                            Err(e) => self.emit(
                                t,
                                EventKind::Info {
                                    message: format!("PANIC message: {e}"),
                                },
                            ),
                        },
                        PanicMessage::Osc(o) => match o.encode() {
                            Ok(bytes) => self.actions.push(Action::Osc {
                                at: t,
                                target: o.target(),
                                bytes,
                            }),
                            Err(e) => self.emit(
                                t,
                                EventKind::Info {
                                    message: format!("PANIC message: {e}"),
                                },
                            ),
                        },
                    }
                }
                self.emit(t, EventKind::Panic);
                Ok(())
            }
            Command::PauseAll => {
                self.pause_all(t, true);
                Ok(())
            }
            Command::ResumeAll => {
                self.pause_all(t, false);
                Ok(())
            }
            Command::TogglePause => {
                let pause = !self.paused_all;
                self.pause_all(t, pause);
                Ok(())
            }
            Command::Next { list } => self.step(list, 1, t),
            Command::Previous { list } => self.step(list, -1, t),
            Command::SetPlayhead { list, cue } => {
                let list = match cue {
                    Some(c) => self
                        .show
                        .find(c)
                        .map(|(l, _)| l)
                        .ok_or_else(|| format!("cue {c} does not exist"))?,
                    None => self.default_list(list).ok_or("no cue list")?,
                };
                // A list-mode group stands for its first cue.
                let cue = match cue {
                    Some(c) => Some(
                        self.show
                            .list(list)
                            .and_then(|l| l.first_step_of(c))
                            .ok_or(
                            "cues inside groups cannot take the playhead (except in list groups)",
                        )?,
                    ),
                    None => None,
                };
                self.set_playhead(list, cue, t);
                Ok(())
            }
            Command::GotoNumber { number } => {
                let (list, id) = self
                    .show
                    .find_by_number(&number)
                    .map(|(l, c)| (l, c.id))
                    .ok_or_else(|| format!("no cue with number {number}"))?;
                let Some(id) = self.show.list(list).and_then(|l| l.first_step_of(id)) else {
                    return Err(format!("cue {number} is inside a group"));
                };
                self.set_playhead(list, Some(id), t);
                Ok(())
            }
            Command::StartCue { cue } => {
                self.need_cue(cue)?;
                self.trigger(cue, t, Trig::default())
                    .map(|_| ())
                    .ok_or_else(|| "cue is already running".to_string())
            }
            Command::StopCue { cue } => {
                self.need_cue(cue)?;
                let fade = self.fr(f64::from(self.show.settings.stop_fade_ms) / 1000.0);
                self.stop_cue(cue, t, fade);
                Ok(())
            }
            Command::SeekCue { cue, time } => {
                self.need_cue(cue)?;
                let running = self.insts.iter().any(|i| i.cue == cue && i.started);
                if running {
                    self.stop_cue(cue, t, self.fr(0.01));
                }
                self.load_cue(cue, time.max(0.0), t);
                if running {
                    self.trigger(cue, t, Trig::default());
                }
                Ok(())
            }
            Command::ReleaseLoop { cue, name } => {
                self.need_cue(cue)?;
                self.release_loop(cue, name.as_deref(), t)
            }
            Command::ReleaseLoops { name } => {
                if self.release_loops(name.as_deref(), t, None) == 0 {
                    return Err("no loop is playing".into());
                }
                Ok(())
            }
            Command::PauseCue { cue } => {
                self.need_cue(cue)?;
                self.pause_cue(cue, t, true);
                Ok(())
            }
            Command::ResumeCue { cue } => {
                self.need_cue(cue)?;
                self.pause_cue(cue, t, false);
                Ok(())
            }
            Command::LoadCue { cue, time } => {
                self.need_cue(cue)?;
                self.load_cue(cue, time, t);
                Ok(())
            }
            Command::ArmCue { cue, armed } => {
                self.need_cue(cue)?;
                self.arm(cue, armed, t);
                Ok(())
            }
            Command::ResetCue { cue } => {
                self.need_cue(cue)?;
                self.reset_cue(cue, t);
                Ok(())
            }
            Command::Reset => {
                self.stop_everything(t, 0, false);
                self.loaded.clear();
                self.errors.clear();
                self.last_go = None;
                self.show_start = None;
                let heads: Vec<(ListId, Option<CueId>)> = self
                    .show
                    .cue_lists
                    .iter()
                    .map(|l| (l.id, l.steps().first().map(|c| c.id)))
                    .collect();
                for (list, head) in heads {
                    self.set_playhead(list, head, t);
                }
                if self.chase.is_none()
                    && (self.tc.running
                        || self.tc.anchor_frame
                            != self.show.timecode.start.to_frame_number(self.tc.rate))
                {
                    self.tc_action(&TcAction::Stop, t);
                }
                self.standby_dirty = true;
                self.emit(t, EventKind::Reset);
                Ok(())
            }
            Command::ResetShowClock => {
                self.show_start = None;
                Ok(())
            }
            Command::TcStart { from } => {
                match from {
                    Some(from) => self.tc_action(&TcAction::StartFrom { from }, t),
                    None => self.tc_action(&TcAction::Start, t),
                }
                Ok(())
            }
            Command::TcStop => {
                self.tc_action(&TcAction::Stop, t);
                Ok(())
            }
            Command::TcPause => {
                self.tc_action(&TcAction::Pause, t);
                Ok(())
            }
            Command::TcResume => {
                self.tc_action(&TcAction::Resume, t);
                Ok(())
            }
            Command::TcRelocate { to } => {
                self.tc_action(&TcAction::Relocate { to }, t);
                Ok(())
            }
            Command::SetMaster { db } => {
                let db = if db.is_finite() {
                    db.clamp(-120.0, 12.0)
                } else {
                    -120.0
                };
                self.master_db = db;
                let ramp = self.fr(0.02);
                self.actions.push(Action::MasterGain {
                    gain: db_to_lin(db),
                    ramp_frames: ramp,
                });
                self.emit(t, EventKind::Master { db });
                Ok(())
            }
            Command::MidiPanic => {
                self.actions.push(Action::MidiPanic);
                Ok(())
            }
        }
    }

    fn go(&mut self, list: Option<ListId>, t: u64) -> Result<(), String> {
        let list = self.default_list(list).ok_or("no cue list")?;
        let debounce = self.fr(f64::from(self.show.settings.go_debounce_ms) / 1000.0);
        if let Some(last) = self.last_go {
            if t.saturating_sub(last) < debounce {
                self.emit(
                    t,
                    EventKind::GoIgnored {
                        reason: "debounce".into(),
                    },
                );
                return Ok(());
            }
        }
        // Loops that GO releases go first: GO only lets them go.
        if self.go_releases(list, t) > 0 {
            self.last_go = Some(t);
            return Ok(());
        }
        let Some(Some(head)) = self.playheads.get(&list).copied() else {
            self.emit(
                t,
                EventKind::GoIgnored {
                    reason: "end of list".into(),
                },
            );
            return Ok(());
        };
        let Some(ix) = self.show.list(list).and_then(|l| l.step_index(head)) else {
            return Err("playhead cue no longer exists".into());
        };
        self.last_go = Some(t);
        if self.show_start.is_none() {
            self.show_start = Some(t);
            if self.transport_coupled() && !self.tc.running {
                self.tc_action(&TcAction::Start, t);
            }
        }
        if self.paused_all {
            self.paused_all = false;
        }
        self.emit(t, EventKind::Go { cue: head });
        self.trigger(head, t, Trig::default());
        let next = self.after_chain(list, ix);
        self.set_playhead(list, next, t);
        Ok(())
    }

    fn step(&mut self, list: Option<ListId>, delta: i64, t: u64) -> Result<(), String> {
        let list = self.default_list(list).ok_or("no cue list")?;
        let steps = self.show.list(list).ok_or("no cue list")?.steps();
        let len = steps.len() as i64;
        let cur = match self.playheads.get(&list).copied().flatten() {
            Some(c) => steps
                .iter()
                .position(|s| s.id == c)
                .map(|i| i as i64)
                .unwrap_or(len),
            None => len,
        };
        let new = (cur + delta).clamp(0, len);
        let cue = steps.get(new as usize).map(|c| c.id);
        self.set_playhead(list, cue, t);
        Ok(())
    }

    fn stop_everything(&mut self, t: u64, fade_frames: u64, hard: bool) {
        let cues: Vec<CueId> = self.insts.iter().map(|i| i.cue).collect();
        let pending: Vec<VoiceId> = self.insts.iter().flat_map(Inst::pending_voices).collect();
        for voice in pending {
            self.actions.push(Action::DiscardVoice { voice });
        }
        self.insts.clear();
        for cue in cues {
            self.emit(t, EventKind::CueStopped { cue });
        }
        self.actions.push(Action::StopAllAudio {
            at: t,
            fade_frames,
            hard,
        });
        self.paused_all = false;
        if self.transport_coupled() && (self.tc.running || self.tc_paused_by_transport) {
            self.tc_action(&TcAction::Stop, t);
        }
        self.standby_dirty = true;
    }

    fn pause_all(&mut self, t: u64, pause: bool) {
        let ids: Vec<u64> = self.insts.iter().map(|i| i.id).collect();
        for id in ids {
            self.pause_inst(id, t, pause);
        }
        self.paused_all = pause;
        let follows_pause = self.transport_coupled()
            || (self.chase.is_none() && self.show.timecode.pause_with_transport);
        if follows_pause {
            if pause && self.tc.running {
                self.tc_action(&TcAction::Pause, t);
                self.tc_paused_by_transport = true;
            } else if !pause && self.tc_paused_by_transport {
                self.tc_action(&TcAction::Resume, t);
            }
        }
        self.emit(
            t,
            if pause {
                EventKind::PausedAll
            } else {
                EventKind::ResumedAll
            },
        );
    }

    // ------------------------------------------------------------------
    // Snapshot
    // ------------------------------------------------------------------

    /// The loop instance `i` plays, for display.
    fn loop_status(&self, i: &Inst, t: u64) -> Option<LoopStatus> {
        let lp = i.lp?;
        let Some(CueKind::Audio(a)) = self.show.cue(i.cue).map(|c| &c.kind) else {
            return None;
        };
        let loops = a.active_loops();
        let l = loops.get(lp.index)?;
        let pos = i.local_at(t).saturating_sub(i.pre);
        let k = lp.next_end(pos);
        let (remaining, releasing) = match i.vamp {
            VampState::Switching {
                at, auto: false, ..
            } => (at.saturating_sub(i.pre + pos), true),
            _ => (lp.end(k) - pos, false),
        };
        Some(LoopStatus {
            name: l.name.clone(),
            index: lp.index,
            count: loops.len(),
            pass: k + 1,
            passes: l.passes,
            remaining: self.secs(remaining),
            releasing,
            release_on_go: l.release_on_go,
            immediate: l.immediate,
        })
    }

    /// File position (seconds) of an audio cue with loops.
    fn file_pos(&self, i: &Inst, t: u64) -> Option<f64> {
        if !i.started || i.voice.is_none() {
            return None;
        }
        let Some(CueKind::Audio(a)) = self.show.cue(i.cue).map(|c| &c.kind) else {
            return None;
        };
        if a.active_loops().is_empty() {
            return None;
        }
        let pos = i.local_at(t).saturating_sub(i.pre);
        Some(match i.lp {
            Some(lp) => lp.file_pos(pos),
            None => i.origin.1 + self.secs(pos.saturating_sub(i.origin.0)),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        let t = self.now;
        let active = self
            .insts
            .iter()
            .map(|i| {
                let local = i.local_at(t);
                let (phase, elapsed, duration) = if !i.started || local < i.pre {
                    (Phase::PreWait, local, Some(i.pre))
                } else if i.action_done {
                    let cont = i.cont_at.unwrap_or(i.pre);
                    (
                        Phase::PostWait,
                        local - i.pre,
                        Some(cont.saturating_sub(i.pre)),
                    )
                } else {
                    (Phase::Running, local - i.pre, i.dur)
                };
                ActiveCue {
                    cue: i.cue,
                    list: i.list,
                    phase,
                    paused: i.paused,
                    elapsed: self.secs(elapsed),
                    duration: duration.map(|d| self.secs(d)),
                    from_tc: i.from_tc,
                    vamping: i.vamp == VampState::Looping,
                    looping: self.loop_status(i, t),
                    file_pos: self.file_pos(i, t),
                }
            })
            .collect();
        let mut next_auto: Option<u64> = None;
        let mut consider = |g: u64| {
            if next_auto.is_none_or(|n| g < n) {
                next_auto = Some(g);
            }
        };
        for i in self.insts.iter().filter(|i| !i.paused && i.next.is_some()) {
            if let Some(c) = i.cont_at {
                consider(i.global(c));
            } else if let (true, Some(d)) = (i.follow, i.dur) {
                if i.group.is_none() {
                    consider(i.global(i.pre + d));
                }
            }
        }
        if let Some((s, _)) = self.next_tc_trigger() {
            consider(s);
        }
        Snapshot {
            frame: t,
            sample_rate: self.sr,
            lists: self
                .show
                .cue_lists
                .iter()
                .map(|l| ListState {
                    id: l.id,
                    playhead: self.playheads.get(&l.id).copied().flatten(),
                })
                .collect(),
            active,
            errors: self.errors.clone(),
            loaded: self
                .loaded
                .iter()
                .map(|(c, f)| (*c, self.secs(*f)))
                .collect(),
            show_elapsed: self.show_start.map(|s| self.secs(t.saturating_sub(s))),
            paused_all: self.paused_all,
            master_db: self.master_db,
            tc: self.tc,
            tc_position: self.tc.timecode_at(t, self.sr).format(self.tc.rate),
            tc_status: self.tc_status(),
            tc_speed: self.chase.as_ref().and_then(Chaser::speed),
            next_auto: next_auto.map(|g| self.secs(g.saturating_sub(t))),
        }
    }
}
