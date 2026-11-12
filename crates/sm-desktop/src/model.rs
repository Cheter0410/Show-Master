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
    /// The log file the running program writes.
    pub log_file: Option<String>,
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
    /// MIDI input and timecode input status.
    pub inputs: InputsInfo,
}

/// The last incoming MIDI message (`inputs.midi_last`).
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct MidiLast {
    pub seq: u64,
    pub port: String,
    pub text: String,
    /// note_on | note_off | control_change | program_change | msc | other
    pub kind: String,
    pub channel: u8,
    pub number: u8,
    pub value: u8,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct AudioInputEntry {
    pub id: String,
    pub name: String,
    pub channels: usize,
    pub is_default: bool,
}

/// `state.inputs`.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct InputsInfo {
    /// MIDI inputs that can be listened to (system ports and `rtp:` session).
    pub midi_ports: Vec<String>,
    pub midi_open: Vec<String>,
    pub midi_count: u64,
    pub midi_last: Option<MidiLast>,
    /// Audio capture devices; `None` until scanned (LTC input selected).
    pub audio_inputs: Option<Vec<AudioInputEntry>>,
    pub ltc_open: bool,
    pub ltc_level_db: Option<f32>,
    pub mtc_rate: Option<String>,
    /// Rate detected on the LTC input.
    pub ltc_rate: Option<String>,
    /// Speed of the chased source (1.0 = nominal), while locked.
    pub tc_speed: Option<f64>,
    /// Same values as tick `tc.status`.
    pub tc_status: String,
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
    /// Other RTP-MIDI sessions found via mDNS/Bonjour.
    pub discovered: Vec<DiscoveredPeer>,
}

/// An RTP-MIDI session found on the network (`midi.rtp.discovered`).
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DiscoveredPeer {
    pub name: String,
    /// `ip:port` of its control port.
    pub addr: String,
    pub host: String,
}

/// mDNS/Bonjour status (`remote.mdns`).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MdnsInfo {
    /// The `remote.mdns` setting.
    pub enabled: bool,
    pub active: bool,
    pub host: String,
    /// `http://showmaster.local:7400` while the web interface is advertised.
    pub url: Option<String>,
    pub rtp_advertised: bool,
    /// Advertised RTP-MIDI instance name ("Name (2)" after a conflict).
    pub rtp_instance: Option<String>,
    pub error: Option<String>,
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
    /// Paired device name ("" = logged in with a PIN / this computer).
    pub device: String,
}

/// Main / backup computer link of this machine.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct LinkInfo {
    /// "off" | "main" | "backup"
    pub role: String,
    pub peer: String,
    pub has_key: bool,
    pub connected: bool,
    pub standby: bool,
    pub taken_over: bool,
    pub backup_took_over: bool,
    pub auto_takeover: bool,
    pub last_seen_ms: Option<u64>,
    pub error: Option<String>,
}

/// A device paired by QR code (no token: that is only shown once).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct PairingInfo {
    pub id: u64,
    pub role: String,
    pub name: String,
    pub created_ms: u64,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RemoteInfo {
    pub urls: Vec<String>,
    pub port: u16,
    pub clients: Vec<Client>,
    pub pairings: Vec<PairingInfo>,
    /// Main / backup computer link (`None` in tests without it).
    pub link: Option<LinkInfo>,
    /// `None` when the binary runs without mDNS (tests).
    pub mdns: Option<MdnsInfo>,
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
    /// `generator` | `mtc` | `ltc`
    #[serde(default)]
    pub source: String,
    /// `generator` | `no_signal` | `locking` | `locked` | `freewheel` | `lost` | `stopped` | `held`
    #[serde(default)]
    pub status: String,
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
