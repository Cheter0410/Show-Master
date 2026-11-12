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
    ContinueMode, ControlAction, Crosspoint, Cue, CueId, CueKind, Curve, GroupMode, ListId,
    PanicMessage, Show, TcAction, TcJumpPolicy, TcMode, TcSource,
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
    /// Release a vamping audio cue: it leaves its loop at the end of the
    /// pass and plays on to the end.
    ReleaseLoop {
        cue: CueId,
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
    /// A vamp was released; it leaves the loop `in_secs` from now.
    LoopReleased {
        cue: CueId,
        in_secs: f64,
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
    /// Audio cue playing its vamp (loop) and not released yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vamping: bool,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum VampState {
    /// No vamp, or released and done.
    #[default]
    None,
    /// Looping, not released.
    Looping,
    /// Released before the audio started: play straight through.
    ReleasedEarly,
    /// Released: at local frame `at` the voice `next` (the rest of the file)
    /// takes over; `len` = its length in frames (if known).
    Switching {
        at: u64,
        next: VoiceId,
        len: Option<u64>,
        /// Crossfade length (frames), centred on the loop end.
        xfade: u64,
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

    /// A prepared (not started) voice for a released vamp.
    fn pending_voice(&self) -> Option<VoiceId> {
        match self.vamp {
            VampState::Switching { next, .. } => Some(next),
            _ => None,
        }
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
                Some(Some(c)) if l.index_of(c).is_some() => Some(c),
                Some(None) if !self.insts.is_empty() || self.show_start.is_some() => None,
                _ => l.cues.first().map(|c| c.id),
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
                if let Some(i) = l.index_of(cue_id) {
                    mode = cue.continue_mode;
                    next = l.cues.get(i + 1).map(|c| c.id);
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
            if let Some(voice) = inst.pending_voice() {
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
        let vamp = a.active_vamp();
        Some(VoiceSpec {
            file: a.file.clone(),
            start: a.start.max(0.0),
            end: vamp.map_or(a.end, |v| Some(v.end)),
            loop_count: if vamp.is_some() { 0 } else { a.loop_count },
            offset_frames,
            loop_from: vamp.map(|v| v.start),
            start_frame: None,
            gain: db_to_lin(a.gain_db),
            matrix: self.matrix(&a.effective_routing(self.show.patch.outputs.len())),
        })
    }

    /// The plain (no vamp) spec: an audio cue whose vamp was released
    /// before it started plays straight through.
    fn plain_voice_spec(&self, cue: &Cue, offset_frames: u64) -> Option<VoiceSpec> {
        let CueKind::Audio(a) = &cue.kind else {
            return None;
        };
        Some(VoiceSpec {
            loop_count: 1,
            end: a.end,
            loop_from: None,
            ..self.voice_spec(cue, offset_frames)?
        })
    }

    /// Release the vamp of `cue`: find the first loop end far enough ahead
    /// to open the rest of the file, and schedule the switch there.
    fn release_loop(&mut self, cue: CueId, t: u64) -> Result<(), String> {
        let Some(ix) = self.insts.iter().position(|i| i.cue == cue) else {
            return Err("the cue is not running".into());
        };
        let Some(c) = self.show.cue(cue).cloned() else {
            return Err("no such cue".into());
        };
        let CueKind::Audio(a) = &c.kind else {
            return Err("only audio cues have a loop".into());
        };
        let Some(v) = a.active_vamp() else {
            return Err("this cue has no loop".into());
        };
        let i = &self.insts[ix];
        match i.vamp {
            VampState::Looping => {}
            VampState::None if !i.started || i.voice.is_none() => {
                // Still in its pre-wait: it will play straight through.
                self.insts[ix].vamp = VampState::ReleasedEarly;
                self.emit(t, EventKind::LoopReleased { cue, in_secs: 0.0 });
                return Ok(());
            }
            _ => return Err("the loop is already released".into()),
        }
        if i.voice.is_none() {
            return Err("the cue is not playing".into());
        }
        // Frames at the file's own rate, rounded as the file source does.
        let rate_u = u64::from(a.file_rate.unwrap_or(self.sr).max(1));
        let sr_u = u64::from(self.sr.max(1));
        let (rate, sr) = (rate_u as f64, sr_u as f64);
        let src = |secs: f64| (secs.max(0.0) * rate).round() as u64;
        let s0 = src(a.start);
        let le = src(v.end);
        let ls = src(v.start).max(s0);
        if le <= ls {
            return Err("the loop is empty".into());
        }
        let (intro, len) = (le - s0, le - ls);
        let pos = i.local_at(t).saturating_sub(i.pre);
        let pre = i.pre;
        // The new voice starts `h_src` source frames before the loop end, at
        // an engine frame that maps exactly onto a source frame: both rates
        // meet every `q` source = `p` engine frames (147 / 160 for 44.1 /
        // 48 kHz), so the splice has no sub-sample offset.
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
        let mut k = 0u64;
        let (start_src, h_src) = loop {
            let n = intro + k * len;
            let mut h = if q <= 8192 { n % q } else { min_h };
            while h < min_h {
                h += q;
            }
            if h <= n && to_engine(n - h) >= ready {
                break (n - h, h);
            }
            k += 1;
        };
        let start = to_engine(start_src);
        let h_eng = (h_src as f64 * sr / rate).round() as u64;
        let next = self.alloc_voice();
        let mut spec = self
            .plain_voice_spec(&c, 0)
            .ok_or("only audio cues have a loop")?;
        spec.start = v.end;
        spec.start_frame = Some(le - h_src);
        self.actions.push(Action::PrepareVoice {
            voice: next,
            cue,
            spec,
        });
        let rest = a
            .end
            .or(a.file_duration)
            .map(|end| self.fr(end - v.end) + h_eng);
        let at = pre + start;
        let i = &mut self.insts[ix];
        i.vamp = VampState::Switching {
            at,
            next,
            len: rest,
            xfade: 2 * h_eng,
        };
        let in_secs = (i.global(at + h_eng) - t) as f64 / sr;
        self.emit(t, EventKind::LoopReleased { cue, in_secs });
        Ok(())
    }

    /// The switch point of a released vamp: cross over to the rest of the file.
    fn vamp_switch(&mut self, ix: usize, t: u64) {
        let VampState::Switching {
            at,
            next,
            len,
            xfade,
        } = self.insts[ix].vamp
        else {
            return;
        };
        let i = &mut self.insts[ix];
        i.vamp = VampState::None;
        let old = i.voice.replace(next);
        i.old_voice = old.map(|v| (v, t + xfade + 1));
        let pre = i.pre;
        i.dur = len.map(|l| at - pre + l);
        let cue = i.cue;
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
        let fade = match self.show.cue(cue).map(|c| &c.kind) {
            Some(CueKind::Audio(a)) => (self.fr(a.fade_out), a.fade_curve),
            _ => (0, Curve::Linear),
        };
        let i = &mut self.insts[ix];
        if let (Some(d), true) = (i.dur, fade.0 > 0) {
            i.fade_frames = fade.0.min(d.saturating_sub(at - pre));
            i.fade_curve = fade.1;
            i.fade_at = Some(pre + d - i.fade_frames);
        }
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
                let early = self.insts[ix].vamp == VampState::ReleasedEarly;
                let vamping = !early && a.active_vamp().is_some();
                let dur = if early {
                    a.play_through_duration()
                } else {
                    a.duration()
                }
                .map(|d| self.fr(d));
                if dur.is_some_and(|d| offset >= d) {
                    self.insts[ix].action_done = true;
                    return;
                }
                let standby = self.standby.remove(&cue.id);
                let voice = match standby {
                    Some((v, off)) if off == offset && !early => v,
                    other => {
                        if let Some((v, _)) = other {
                            self.actions.push(Action::DiscardVoice { voice: v });
                        }
                        let v = self.alloc_voice();
                        let spec = if early {
                            self.plain_voice_spec(cue, offset)
                        } else {
                            self.voice_spec(cue, offset)
                        };
                        if let Some(spec) = spec {
                            self.actions.push(Action::PrepareVoice {
                                voice: v,
                                cue: cue.id,
                                spec,
                            });
                        }
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
                let fade = self.fr(a.fade_out);
                let i = &mut self.insts[ix];
                i.voice = Some(voice);
                i.vamp = if vamping {
                    VampState::Looping
                } else {
                    VampState::None
                };
                i.dur = dur;
                if let (Some(d), true) = (dur, fade > 0) {
                    i.fade_frames = fade.min(d - offset);
                    i.fade_curve = a.fade_curve;
                    i.fade_at = Some(pre + d - i.fade_frames);
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
                        if let Err(e) = self.release_loop(target, t) {
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
                let order = match g.mode {
                    GroupMode::Parallel | GroupMode::Playlist => children,
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
                removed.push((i.cue, i.voices(t).collect::<Vec<_>>(), i.pending_voice()));
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
            if let Some(voice) = pending {
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
                .is_some_and(|l| l.index_of(c).is_some()),
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
        let l = self.show.list(list)?;
        while l
            .cues
            .get(i)
            .is_some_and(|c| c.continue_mode != ContinueMode::DoNotContinue)
        {
            i += 1;
        }
        l.cues.get(i + 1).map(|c| c.id)
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
            let Some(mut i) = l.index_of(*head) else {
                continue;
            };
            while let Some(cue) = l.cues.get(i) {
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
        // The rest of a released vamp could not be opened: keep looping.
        if let Some(i) = self
            .insts
            .iter_mut()
            .find(|i| i.pending_voice() == Some(voice))
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
                if let Some(c) = cue {
                    if self.show.list(list).and_then(|l| l.index_of(c)).is_none() {
                        return Err("cues inside groups cannot take the playhead".into());
                    }
                }
                self.set_playhead(list, cue, t);
                Ok(())
            }
            Command::GotoNumber { number } => {
                let (list, id) = self
                    .show
                    .find_by_number(&number)
                    .map(|(l, c)| (l, c.id))
                    .ok_or_else(|| format!("no cue with number {number}"))?;
                if self.show.list(list).and_then(|l| l.index_of(id)).is_none() {
                    return Err(format!("cue {number} is inside a group"));
                }
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
            Command::ReleaseLoop { cue } => {
                self.need_cue(cue)?;
                self.release_loop(cue, t)
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
                    .map(|l| (l.id, l.cues.first().map(|c| c.id)))
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
        let Some(Some(head)) = self.playheads.get(&list).copied() else {
            self.emit(
                t,
                EventKind::GoIgnored {
                    reason: "end of list".into(),
                },
            );
            return Ok(());
        };
        let Some(ix) = self.show.list(list).and_then(|l| l.index_of(head)) else {
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
        let l = self.show.list(list).ok_or("no cue list")?;
        let len = l.cues.len() as i64;
        let cur = match self.playheads.get(&list).copied().flatten() {
            Some(c) => l.index_of(c).map(|i| i as i64).unwrap_or(len),
            None => len,
        };
        let new = (cur + delta).clamp(0, len);
        let cue = l.cues.get(new as usize).map(|c| c.id);
        self.set_playhead(list, cue, t);
        Ok(())
    }

    fn stop_everything(&mut self, t: u64, fade_frames: u64, hard: bool) {
        let cues: Vec<CueId> = self.insts.iter().map(|i| i.cue).collect();
        let pending: Vec<VoiceId> = self.insts.iter().filter_map(Inst::pending_voice).collect();
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
