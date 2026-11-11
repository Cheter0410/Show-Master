//! The show data model. This is exactly what is persisted in a showfile.

use serde::{Deserialize, Serialize};
use sm_timecode::{FrameRate, Timecode};
use std::collections::BTreeMap;

pub type CueId = u64;
pub type ListId = u64;

/// Current showfile schema version. Bump together with a migration step in
/// [`crate::showfile`].
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Show {
    pub schema_version: u32,
    pub name: String,
    /// Next free id for cues and lists.
    pub next_id: u64,
    pub settings: ShowSettings,
    pub patch: OutputPatch,
    pub midi_ports: Vec<MidiPortAlias>,
    pub rtp_midi: RtpMidiSettings,
    pub timecode: TimecodeSettings,
    /// Stage manager memo shown on the dashboard.
    pub notes: String,
    /// Hotkey map: key combination (e.g. `Space`, `Ctrl+KeyS`) -> action name.
    pub hotkeys: BTreeMap<String, String>,
    pub cue_lists: Vec<CueList>,
}

impl Default for Show {
    fn default() -> Self {
        let mut show = Self {
            schema_version: SCHEMA_VERSION,
            name: "Untitled Show".into(),
            next_id: 1,
            settings: ShowSettings::default(),
            patch: OutputPatch::stereo(),
            midi_ports: Vec::new(),
            rtp_midi: RtpMidiSettings::default(),
            timecode: TimecodeSettings::default(),
            notes: String::new(),
            hotkeys: default_hotkeys(),
            cue_lists: Vec::new(),
        };
        let id = show.alloc_id();
        show.cue_lists.push(CueList {
            id,
            name: "Main".into(),
            tc_jump: TcJumpPolicy::Skip,
            cues: Vec::new(),
        });
        show
    }
}

pub fn default_hotkeys() -> BTreeMap<String, String> {
    [
        ("Space", "go"),
        ("Escape", "panic"),
        ("ArrowDown", "next"),
        ("ArrowUp", "previous"),
        ("KeyP", "toggle_pause"),
        ("KeyS", "stop_all"),
        ("KeyR", "reset"),
        ("KeyV", "preview"),
        ("KeyL", "toggle_mode"),
        ("Ctrl+KeyZ", "undo"),
        ("Ctrl+Shift+KeyZ", "redo"),
        ("Ctrl+KeyS", "save"),
        ("Ctrl+KeyD", "duplicate"),
        ("Delete", "delete"),
        ("Ctrl+KeyF", "find"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ShowSettings {
    /// A second GO within this window after the previous one is ignored.
    pub go_debounce_ms: u32,
    /// PANIC fade time; 0 = hard cut.
    pub panic_fade_ms: u32,
    /// Fade time for a normal "Stop All".
    pub stop_fade_ms: u32,
    /// Require the panic hotkey to be tapped twice.
    pub panic_double_tap: bool,
    pub double_tap_ms: u32,
    /// Ask for confirmation before the app quits or reloads in live mode.
    pub quit_lock: bool,
    pub high_contrast: bool,
    pub audio: AudioSettings,
    pub remote: RemoteSettings,
    pub timer_alarms: Vec<TimerAlarm>,
}

impl Default for ShowSettings {
    fn default() -> Self {
        Self {
            go_debounce_ms: 250,
            panic_fade_ms: 0,
            stop_fade_ms: 500,
            panic_double_tap: false,
            double_tap_ms: 600,
            quit_lock: true,
            high_contrast: false,
            audio: AudioSettings::default(),
            remote: RemoteSettings::default(),
            timer_alarms: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AudioSettings {
    /// Backend name (e.g. `cpal`, `null`); `None` = first backend with a device.
    pub backend: Option<String>,
    /// Device id as reported by the backend; `None` = system default.
    pub device: Option<String>,
    pub sample_rate: u32,
    pub buffer_frames: Option<u32>,
    /// Number of device output channels to open.
    pub channels: u16,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            backend: None,
            device: None,
            sample_rate: 48_000,
            buffer_frames: Some(256),
            channels: 2,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RemoteSettings {
    /// Serve the web UI to other devices on the local network.
    pub enabled: bool,
    pub port: u16,
    /// PIN granting full control from a remote device.
    pub control_pin: String,
    /// PIN granting read-only access; empty = view-only needs no PIN.
    pub view_pin: String,
    /// Optional OSC command listener (UDP), 0 = disabled.
    pub osc_port: u16,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 7400,
            control_pin: "1234".into(),
            view_pin: String::new(),
            osc_port: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct TimerAlarm {
    /// Show running time in seconds at which the alarm is raised.
    pub at: f64,
    pub label: String,
}

/// Named logical outputs mapped to device channels, plus reusable groups.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct OutputPatch {
    pub outputs: Vec<OutputDef>,
    pub groups: Vec<OutputGroup>,
}

impl OutputPatch {
    pub fn stereo() -> Self {
        Self::from_layout("stereo").unwrap_or_default()
    }

    /// Build a patch from a named layout template, device channels 0..n.
    pub fn from_layout(layout: &str) -> Option<Self> {
        let names = layout_channels(layout)?;
        Some(Self {
            outputs: names
                .iter()
                .enumerate()
                .map(|(i, n)| OutputDef {
                    name: (*n).to_string(),
                    channel: i as u16,
                })
                .collect(),
            groups: vec![OutputGroup {
                name: layout.to_string(),
                layout: Some(layout.to_string()),
                outputs: (0..names.len() as u16).collect(),
            }],
        })
    }
}

/// Speaker names of the built-in layout templates.
pub fn layout_channels(layout: &str) -> Option<&'static [&'static str]> {
    Some(match layout {
        "mono" => &["M"],
        "stereo" => &["L", "R"],
        "lcr" => &["L", "R", "C"],
        "quad" => &["L", "R", "Ls", "Rs"],
        "5.1" => &["L", "R", "C", "LFE", "Ls", "Rs"],
        "7.1" => &["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"],
        _ => return None,
    })
}

pub const LAYOUTS: [&str; 6] = ["mono", "stereo", "lcr", "quad", "5.1", "7.1"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct OutputDef {
    pub name: String,
    /// Zero-based device channel.
    pub channel: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct OutputGroup {
    pub name: String,
    /// Layout template this group follows (`5.1`, ...) or `None` for a free set.
    pub layout: Option<String>,
    /// Indices into [`OutputPatch::outputs`].
    pub outputs: Vec<u16>,
}

/// A stable show-level name for a MIDI destination, so cues survive
/// re-patching on a different machine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct MidiPortAlias {
    pub name: String,
    /// System port name, or `rtp:<session name>` for the built-in RTP-MIDI session.
    pub device: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RtpMidiSettings {
    pub enabled: bool,
    pub session_name: String,
    pub port: u16,
    /// Peers (`ip:port`) we actively invite.
    pub connect_to: Vec<String>,
    pub journal: bool,
}

impl Default for RtpMidiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            session_name: "Show Master".into(),
            port: 5004,
            connect_to: Vec::new(),
            journal: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TcMode {
    /// Runs only when started explicitly (timecode cue or command).
    #[default]
    Freerun,
    /// Starts with the first GO, pauses/stops with the show transport.
    Transport,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TimecodeSettings {
    pub rate: FrameRate,
    /// Start offset used when the generator is started without a position.
    pub start: Timecode,
    pub mode: TcMode,
    pub ltc_enabled: bool,
    /// Zero-based device channel carrying LTC.
    pub ltc_channel: u16,
    pub ltc_level_db: f32,
    pub mtc_enabled: bool,
    /// MIDI port alias (or device name) for MTC.
    pub mtc_port: String,
}

impl Default for TimecodeSettings {
    fn default() -> Self {
        Self {
            rate: FrameRate::F25,
            start: Timecode::new(1, 0, 0, 0),
            mode: TcMode::Freerun,
            ltc_enabled: false,
            ltc_channel: 1,
            ltc_level_db: -12.0,
            mtc_enabled: false,
            mtc_port: String::new(),
        }
    }
}

/// What happens to timecode-triggered cues when the timecode jumps.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TcJumpPolicy {
    /// Forward jump: fire every cue whose trigger was jumped over. Backward: nothing.
    Fire,
    /// Jumped-over cues are not fired; cues only fire when their time is reached.
    #[default]
    Skip,
    /// Bring the list into the state it would have at the new position: cues
    /// that would be playing are started at the right offset, cues triggered
    /// later than the new position are stopped.
    Chase,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct CueList {
    pub id: ListId,
    pub name: String,
    pub tc_jump: TcJumpPolicy,
    pub cues: Vec<Cue>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContinueMode {
    #[default]
    DoNotContinue,
    /// Fire the next cue when this cue's post-wait has elapsed.
    AutoContinue,
    /// Fire the next cue when this cue has finished.
    AutoFollow,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Cue {
    pub id: CueId,
    pub number: String,
    pub name: String,
    pub notes: String,
    pub color: String,
    /// Seconds between trigger and action.
    pub pre_wait: f64,
    /// Seconds between action and auto-continue.
    pub post_wait: f64,
    pub continue_mode: ContinueMode,
    pub armed: bool,
    pub tc_trigger: Option<Timecode>,
    pub kind: CueKind,
}

impl Default for Cue {
    fn default() -> Self {
        Self {
            id: 0,
            number: String::new(),
            name: String::new(),
            notes: String::new(),
            color: String::new(),
            pre_wait: 0.0,
            post_wait: 0.0,
            continue_mode: ContinueMode::DoNotContinue,
            armed: true,
            tc_trigger: None,
            kind: CueKind::Memo,
        }
    }
}

impl Cue {
    /// Duration of the cue's action in seconds; `None` = open-ended.
    pub fn action_duration(&self) -> Option<f64> {
        match &self.kind {
            CueKind::Audio(a) => a.duration(),
            CueKind::Fade(f) => Some(f.duration.max(0.0)),
            CueKind::Group(_) => None,
            _ => Some(0.0),
        }
    }

    pub fn children(&self) -> &[Cue] {
        match &self.kind {
            CueKind::Group(g) => &g.children,
            _ => &[],
        }
    }

    pub fn type_name(&self) -> &'static str {
        match &self.kind {
            CueKind::Audio(_) => "audio",
            CueKind::Fade(_) => "fade",
            CueKind::Midi(_) => "midi",
            CueKind::Timecode(_) => "timecode",
            CueKind::Control(_) => "control",
            CueKind::Group(_) => "group",
            CueKind::Memo => "memo",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CueKind {
    Audio(AudioCue),
    Fade(FadeCue),
    Midi(MidiCue),
    Timecode(TimecodeCue),
    Control(ControlCue),
    Group(GroupCue),
    Memo,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    Linear,
    #[default]
    EqualPower,
    SCurve,
}

/// One routing matrix cell: source channel -> logical output, level in dB.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct Crosspoint {
    pub src: u16,
    /// Index into [`OutputPatch::outputs`].
    pub out: u16,
    pub db: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AudioCue {
    /// Media path, relative to the showfile directory when possible.
    pub file: String,
    /// BLAKE3 hash of the media file (hex), filled by "collect show folder".
    pub hash: Option<String>,
    /// Cached media info (filled when the file is assigned).
    pub file_duration: Option<f64>,
    pub channels: u16,
    /// Start point in seconds.
    pub start: f64,
    /// End point in seconds; `None` = end of file.
    pub end: Option<f64>,
    pub gain_db: f32,
    pub fade_in: f64,
    pub fade_out: f64,
    pub fade_curve: Curve,
    /// Number of passes; 0 = loop forever.
    pub loop_count: u32,
    /// Routing matrix; empty = default routing (channel i -> output i, mono -> outputs 1+2).
    pub routing: Vec<Crosspoint>,
}

impl Default for AudioCue {
    fn default() -> Self {
        Self {
            file: String::new(),
            hash: None,
            file_duration: None,
            channels: 2,
            start: 0.0,
            end: None,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_curve: Curve::EqualPower,
            loop_count: 1,
            routing: Vec::new(),
        }
    }
}

impl AudioCue {
    /// Length of one pass in seconds, if known.
    pub fn region(&self) -> Option<f64> {
        let end = self.end.or(self.file_duration)?;
        Some((end - self.start.max(0.0)).max(0.0))
    }

    pub fn duration(&self) -> Option<f64> {
        if self.loop_count == 0 {
            return None;
        }
        Some(self.region()? * f64::from(self.loop_count))
    }

    /// Effective routing: explicit crosspoints, or the default mapping.
    pub fn effective_routing(&self, outputs: usize) -> Vec<Crosspoint> {
        if !self.routing.is_empty() {
            return self.routing.clone();
        }
        if self.channels <= 1 {
            (0..outputs.min(2) as u16)
                .map(|out| Crosspoint {
                    src: 0,
                    out,
                    db: 0.0,
                })
                .collect()
        } else {
            (0..self.channels.min(outputs as u16))
                .map(|i| Crosspoint {
                    src: i,
                    out: i,
                    db: 0.0,
                })
                .collect()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FadeCue {
    pub target: Option<CueId>,
    pub duration: f64,
    pub curve: Curve,
    /// New cue level in dB; `None` = leave the master level alone.
    pub gain_db: Option<f32>,
    /// Crosspoint levels to fade to.
    pub crosspoints: Vec<Crosspoint>,
    /// Stop the target when the fade completes.
    pub stop_target: bool,
}

impl Default for FadeCue {
    fn default() -> Self {
        Self {
            target: None,
            duration: 3.0,
            curve: Curve::EqualPower,
            gain_db: Some(-120.0),
            crosspoints: Vec::new(),
            stop_target: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct MidiCue {
    /// Port alias or device name.
    pub port: String,
    pub message: crate::midi::MidiMessage,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TcAction {
    /// Start the generator at `from` (or the configured start offset).
    #[default]
    Start,
    StartFrom {
        from: Timecode,
    },
    Stop,
    Pause,
    Resume,
    Relocate {
        to: Timecode,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct TimecodeCue {
    pub tc: TcAction,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Start,
    #[default]
    Stop,
    Pause,
    Resume,
    LoadToTime,
    Arm,
    Disarm,
    GoTo,
    Reset,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ControlCue {
    pub action: ControlAction,
    pub target: Option<CueId>,
    /// Position for `LoadToTime` (seconds), fade time for `Stop`.
    pub time: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GroupMode {
    /// Start all children together.
    #[default]
    Parallel,
    /// Play children one after another.
    Playlist,
    /// Play one randomly chosen child.
    Random,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct GroupCue {
    pub mode: GroupMode,
    pub children: Vec<Cue>,
}

// ---------------------------------------------------------------------------
// Tree helpers
// ---------------------------------------------------------------------------

fn find_in(cues: &[Cue], id: CueId) -> Option<&Cue> {
    for c in cues {
        if c.id == id {
            return Some(c);
        }
        if let Some(found) = find_in(c.children(), id) {
            return Some(found);
        }
    }
    None
}

fn find_in_mut(cues: &mut [Cue], id: CueId) -> Option<&mut Cue> {
    for c in cues {
        if c.id == id {
            return Some(c);
        }
        if let CueKind::Group(g) = &mut c.kind {
            if let Some(found) = find_in_mut(&mut g.children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn walk<'a>(cues: &'a [Cue], f: &mut dyn FnMut(&'a Cue)) {
    for c in cues {
        f(c);
        walk(c.children(), f);
    }
}

fn walk_mut(cues: &mut [Cue], f: &mut dyn FnMut(&mut Cue)) {
    for c in cues {
        f(c);
        if let CueKind::Group(g) = &mut c.kind {
            walk_mut(&mut g.children, f);
        }
    }
}

impl CueList {
    /// Index of a top-level cue.
    pub fn index_of(&self, id: CueId) -> Option<usize> {
        self.cues.iter().position(|c| c.id == id)
    }
}

impl Show {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub fn list(&self, id: ListId) -> Option<&CueList> {
        self.cue_lists.iter().find(|l| l.id == id)
    }

    pub fn list_mut(&mut self, id: ListId) -> Option<&mut CueList> {
        self.cue_lists.iter_mut().find(|l| l.id == id)
    }

    /// Find a cue anywhere (including inside groups) with its list id.
    pub fn find(&self, id: CueId) -> Option<(ListId, &Cue)> {
        self.cue_lists
            .iter()
            .find_map(|l| find_in(&l.cues, id).map(|c| (l.id, c)))
    }

    pub fn cue(&self, id: CueId) -> Option<&Cue> {
        self.find(id).map(|(_, c)| c)
    }

    pub fn cue_mut(&mut self, id: CueId) -> Option<&mut Cue> {
        self.cue_lists
            .iter_mut()
            .find_map(|l| find_in_mut(&mut l.cues, id))
    }

    /// Visit every cue, depth first.
    pub fn for_each_cue<'a>(&'a self, mut f: impl FnMut(ListId, &'a Cue)) {
        for l in &self.cue_lists {
            let id = l.id;
            walk(&l.cues, &mut |c| f(id, c));
        }
    }

    pub fn for_each_cue_mut(&mut self, mut f: impl FnMut(&mut Cue)) {
        for l in &mut self.cue_lists {
            walk_mut(&mut l.cues, &mut f);
        }
    }

    /// Find a cue by its (user-assigned) number.
    pub fn find_by_number(&self, number: &str) -> Option<(ListId, &Cue)> {
        let number = number.trim();
        let mut found = None;
        self.for_each_cue(|l, c| {
            if found.is_none() && !c.number.is_empty() && c.number == number {
                found = Some((l, c));
            }
        });
        found
    }

    /// Resolve a MIDI port alias to its device name (or pass the name through).
    pub fn resolve_midi_port<'a>(&'a self, name: &'a str) -> &'a str {
        self.midi_ports
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.device.as_str())
            .unwrap_or(name)
    }

    /// Give every cue with id 0 (and the lists) a fresh id; fix `next_id`.
    pub fn assign_missing_ids(&mut self) {
        let mut max = 0;
        self.for_each_cue(|_, c| max = max.max(c.id));
        for l in &self.cue_lists {
            max = max.max(l.id);
        }
        let mut next = self.next_id.max(max + 1);
        for l in &mut self.cue_lists {
            if l.id == 0 {
                l.id = next;
                next += 1;
            }
        }
        self.for_each_cue_mut(|c| {
            if c.id == 0 {
                c.id = next;
                next += 1;
            }
        });
        self.next_id = next;
    }
}
