//! Typed view of the state document and tick the runtime publishes
//! (`docs/remote-api.md`). Everything is `#[serde(default)]` so a field the
//! UI does not know yet never breaks it.

use serde::Deserialize;
use sm_core::model::Show;
use std::collections::BTreeMap;

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Edit,
    Live,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct UiState {
    pub version: String,
    pub mode: Mode,
    pub show: Show,
    pub show_path: Option<String>,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub recovery_available: bool,
    /// list id -> cue id (`None` = end of list)
    pub playheads: BTreeMap<String, Option<u64>>,
    /// cue id -> error message
    pub errors: BTreeMap<String, String>,
    /// cue id -> loaded time (s)
    pub loaded: BTreeMap<String, f64>,
    pub paused_all: bool,
    pub master_db: f32,
    pub audio: AudioInfo,
    pub midi: MidiInfo,
    pub remote: RemoteInfo,
    pub preflight: Vec<Issue>,
    pub layouts: Vec<String>,
    pub update: UpdateInfo,
}

impl UiState {
    pub fn playhead(&self, list: u64) -> Option<u64> {
        self.playheads.get(&list.to_string()).copied().flatten()
    }
    pub fn error(&self, cue: u64) -> Option<&str> {
        self.errors.get(&cue.to_string()).map(String::as_str)
    }
    pub fn loaded(&self, cue: u64) -> Option<f64> {
        self.loaded.get(&cue.to_string()).copied()
    }
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DeviceEntry {
    pub backend: String,
    pub id: String,
    pub name: String,
    pub max_output_channels: usize,
    pub sample_rates: Vec<u32>,
    pub is_default: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct AudioInfo {
    pub backend: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub buffer_frames: Option<u32>,
    pub is_null: bool,
    pub error: Option<String>,
    pub devices: Vec<DeviceEntry>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Peer {
    pub name: String,
    pub addr: String,
    /// "connecting" | "connected" | "failed"
    pub state: String,
    pub latency_ms: Option<f64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RtpInfo {
    pub enabled: bool,
    pub name: String,
    pub port: u16,
    pub peers: Vec<Peer>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MidiInfo {
    pub ports: Vec<String>,
    pub rtp: RtpInfo,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Client {
    pub id: u64,
    /// "view" | "control" | "admin"
    pub role: String,
    pub addr: String,
    pub since_ms: u64,
    pub agent: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RemoteInfo {
    pub urls: Vec<String>,
    pub port: u16,
    pub clients: Vec<Client>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    #[default]
    Info,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Issue {
    pub severity: Severity,
    pub cue: Option<u64>,
    pub message: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct UpdateInfo {
    /// disabled | checking | current | offline | available | downloading | installing | restarting | failed
    pub state: String,
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub automatic: Option<bool>,
    pub download_page: String,
    pub done: u64,
    pub total: u64,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Tick (fast-changing values, ~20 Hz)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    PreWait,
    #[default]
    Running,
    PostWait,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Active {
    pub cue: u64,
    pub list: u64,
    pub phase: Phase,
    pub paused: bool,
    /// Seconds into the current phase.
    pub elapsed: f64,
    /// Length of the current phase; `None` = open-ended.
    pub duration: Option<f64>,
    pub from_tc: bool,
}

impl Active {
    /// 0..1 progress of the current phase, if it has a length.
    pub fn progress(&self) -> Option<f32> {
        self.duration
            .filter(|d| *d > 0.0)
            .map(|d| (self.elapsed / d).clamp(0.0, 1.0) as f32)
    }
    pub fn remaining(&self) -> Option<f64> {
        self.duration.map(|d| (d - self.elapsed).max(0.0))
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct TcInfo {
    pub position: String,
    pub running: bool,
    pub rate: String,
    pub ltc: bool,
    pub mtc: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Meters {
    /// Linear, per device channel.
    pub peak: Vec<f32>,
    pub rms: Vec<f32>,
    pub clip: Vec<bool>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Tick {
    pub frame: u64,
    pub wall_ms: u64,
    pub show_elapsed: Option<f64>,
    pub tc: TcInfo,
    pub active: Vec<Active>,
    pub next_auto: Option<f64>,
    pub meters: Meters,
    pub cpu: f32,
    pub underruns: u64,
    pub voices: usize,
    pub alarms: Vec<String>,
}

impl Tick {
    pub fn active(&self, cue: u64) -> Option<&Active> {
        self.active.iter().find(|a| a.cue == cue)
    }
}
