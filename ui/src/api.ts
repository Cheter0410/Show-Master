// Types mirroring docs/remote-api.md (the binding contract) + the small HTTP client.
// Everything stays on the same origin; no external hosts are ever contacted.

export type Role = "admin" | "control" | "view";
export type Mode = "edit" | "live";

// ---------- showfile ----------

export type CueColor = "" | "red" | "orange" | "yellow" | "green" | "blue" | "purple";
export const CUE_COLORS: CueColor[] = ["", "red", "orange", "yellow", "green", "blue", "purple"];
export type ContinueMode = "do_not_continue" | "auto_continue" | "auto_follow";
export type FadeCurve = "linear" | "equal_power" | "s_curve";
export const FADE_CURVES: FadeCurve[] = ["linear", "equal_power", "s_curve"];

export interface RoutePoint {
  src: number;
  out: number;
  db: number;
}

export interface AudioKind {
  type: "audio";
  file: string;
  hash: string | null;
  file_duration: number | null;
  channels: number;
  start: number;
  end: number | null;
  gain_db: number;
  fade_in: number;
  fade_out: number;
  fade_curve: FadeCurve;
  /** 0 = infinite */
  loop_count: number;
  /** empty = default routing */
  routing: RoutePoint[];
}

export interface FadeKind {
  type: "fade";
  target: number | null;
  duration: number;
  curve: FadeCurve;
  gain_db: number | null;
  crosspoints: RoutePoint[];
  stop_target: boolean;
}

export type MscCommand =
  | "go" | "stop" | "resume" | "timed_go" | "load" | "set" | "fire" | "all_off" | "restore" | "reset" | "go_off";
export const MSC_COMMANDS: MscCommand[] = [
  "go", "stop", "resume", "timed_go", "load", "set", "fire", "all_off", "restore", "reset", "go_off",
];

export type MidiMessage =
  | { msg: "note_on"; channel: number; note: number; velocity: number }
  | { msg: "note_off"; channel: number; note: number; velocity: number }
  | { msg: "control_change"; channel: number; controller: number; value: number }
  | { msg: "program_change"; channel: number; program: number }
  | { msg: "pitch_bend"; channel: number; value: number }
  | { msg: "sysex"; hex: string }
  | {
      msg: "msc";
      device_id: number;
      command_format: number;
      command: MscCommand;
      q_number: string;
      q_list: string;
      q_path: string;
    };
export type MidiMsgType = MidiMessage["msg"];
export const MIDI_MSG_TYPES: MidiMsgType[] = [
  "note_on", "note_off", "control_change", "program_change", "pitch_bend", "sysex", "msc",
];

export interface MidiKind {
  type: "midi";
  port: string;
  message: MidiMessage;
}

export type TcAction =
  | { action: "start" | "stop" | "pause" | "resume" }
  | { action: "start_from"; from: string }
  | { action: "relocate"; to: string };

export interface TimecodeKind {
  type: "timecode";
  tc: TcAction;
}

export type ControlAction =
  | "start" | "stop" | "pause" | "resume" | "load_to_time" | "arm" | "disarm" | "go_to" | "reset";
export const CONTROL_ACTIONS: ControlAction[] = [
  "start", "stop", "pause", "resume", "load_to_time", "arm", "disarm", "go_to", "reset",
];

export interface ControlKind {
  type: "control";
  action: ControlAction;
  target: number | null;
  time: number;
}

export type GroupMode = "parallel" | "playlist" | "random";

export interface GroupKind {
  type: "group";
  mode: GroupMode;
  children: Cue[];
}

export interface MemoKind {
  type: "memo";
}

export type CueKind = AudioKind | FadeKind | MidiKind | TimecodeKind | ControlKind | GroupKind | MemoKind;
export type CueType = CueKind["type"];
export const CUE_TYPES: CueType[] = ["audio", "fade", "midi", "timecode", "control", "group", "memo"];

export interface Cue {
  id: number;
  number: string;
  name: string;
  notes: string;
  color: CueColor;
  pre_wait: number;
  post_wait: number;
  continue_mode: ContinueMode;
  armed: boolean;
  tc_trigger: string | null;
  kind: CueKind;
}

export type TcJump = "fire" | "skip" | "chase";

export interface CueList {
  id: number;
  name: string;
  tc_jump: TcJump;
  cues: Cue[];
}

export interface AudioSettings {
  backend: string | null;
  device: string | null;
  sample_rate: number;
  buffer_frames: number;
  channels: number;
}

export interface RemoteSettings {
  enabled: boolean;
  port: number;
  control_pin: string;
  view_pin: string;
  osc_port: number;
}

export interface TimerAlarm {
  at: number;
  label: string;
}

export interface Settings {
  go_debounce_ms: number;
  panic_fade_ms: number;
  stop_fade_ms: number;
  panic_double_tap: boolean;
  double_tap_ms: number;
  quit_lock: boolean;
  high_contrast: boolean;
  audio: AudioSettings;
  remote: RemoteSettings;
  timer_alarms: TimerAlarm[];
}

export interface PatchOutput {
  name: string;
  /** device channel, 0-based */
  channel: number;
}

export interface PatchGroup {
  name: string;
  layout: string;
  /** indices into patch.outputs */
  outputs: number[];
}

export interface Patch {
  outputs: PatchOutput[];
  groups: PatchGroup[];
}

export interface MidiPortAlias {
  name: string;
  device: string;
}

export interface RtpMidiSettings {
  enabled: boolean;
  session_name: string;
  port: number;
  connect_to: string[];
  journal: boolean;
}

export type TcRate = "23.976" | "24" | "25" | "29.97df" | "29.97" | "30";
export const TC_RATES: TcRate[] = ["23.976", "24", "25", "29.97df", "29.97", "30"];

export interface TimecodeSettings {
  rate: TcRate;
  start: string;
  mode: "freerun" | "transport";
  ltc_enabled: boolean;
  ltc_channel: number;
  ltc_level_db: number;
  mtc_enabled: boolean;
  mtc_port: string;
}

export interface Show {
  schema_version: number;
  name: string;
  next_id: number;
  notes: string;
  settings: Settings;
  patch: Patch;
  midi_ports: MidiPortAlias[];
  rtp_midi: RtpMidiSettings;
  timecode: TimecodeSettings;
  /** key combo (KeyboardEvent.code with Ctrl+/Shift+/Alt+/Meta+ prefixes) -> action */
  hotkeys: Record<string, string>;
  cue_lists: CueList[];
}

// ---------- state document ----------

export interface AudioDevice {
  backend: string;
  id: string;
  name: string;
  max_output_channels: number;
  sample_rates: number[];
  is_default: boolean;
}

export interface AudioState {
  backend: string;
  device: string;
  sample_rate: number;
  channels: number;
  buffer_frames: number;
  is_null: boolean;
  error: string | null;
  devices: AudioDevice[];
}

export interface RtpPeer {
  name: string;
  addr: string;
  state: string;
  latency_ms: number | null;
}

export interface MidiState {
  ports: string[];
  rtp: { enabled: boolean; name: string; port: number; peers: RtpPeer[] };
}

export interface RemoteClient {
  id: number;
  role: Role;
  addr: string;
  since_ms: number;
  agent: string;
}

export interface RemoteState {
  urls: string[];
  port: number;
  clients: RemoteClient[];
  /** Bonjour name (`http://showmaster.local`), null without mDNS. */
  mdns?: { enabled: boolean; url: string | null } | null;
}

export type Severity = "error" | "warning" | "info";

export interface PreflightIssue {
  severity: Severity;
  cue: number | null;
  message: string;
}

export interface State {
  version: string;
  mode: Mode;
  show: Show;
  show_path: string | null;
  dirty: boolean;
  can_undo: boolean;
  can_redo: boolean;
  recovery_available: boolean;
  /** list id -> cue id | null */
  playheads: Record<string, number | null>;
  /** cue id -> message */
  errors: Record<string, string>;
  /** cue id -> loaded time (s) */
  loaded: Record<string, number>;
  paused_all: boolean;
  master_db: number;
  audio: AudioState;
  midi: MidiState;
  remote: RemoteState;
  preflight: PreflightIssue[];
  layouts: string[];
  update: UpdateState;
}

export interface UpdateState {
  state: "disabled" | "checking" | "current" | "offline" | "available" | "downloading" | "installing" | "restarting" | "failed";
  current: string;
  version: string | null;
  notes: string | null;
  /** false = this copy cannot install by itself (use the download page) */
  automatic: boolean | null;
  download_page: string;
  done: number;
  total: number;
  error: string | null;
}

// ---------- tick ----------

export type Phase = "pre_wait" | "running" | "post_wait";

export interface ActiveCue {
  cue: number;
  list: number;
  phase: Phase;
  paused: boolean;
  elapsed: number;
  duration: number | null;
  from_tc: boolean;
}

export interface TcStatus {
  position: string;
  running: boolean;
  rate: string;
  ltc: boolean;
  mtc: boolean;
}

export interface Meters {
  peak: number[];
  rms: number[];
  clip: boolean[];
}

export interface Tick {
  t: "tick";
  frame: number;
  wall_ms: number;
  show_elapsed: number | null;
  tc: TcStatus;
  active: ActiveCue[];
  next_auto: number | null;
  meters: Meters;
  cpu: number;
  underruns: number;
  voices: number;
  alarms: string[];
}

// ---------- websocket ----------

export type PathSeg = string | number;
export interface PatchOp {
  p: PathSeg[];
  v?: unknown;
  d?: boolean;
}

export type LogLevel = "info" | "warn" | "error";
export interface LogEntry {
  time_ms: number;
  level: LogLevel;
  message: string;
}

export type ServerMsg =
  | { t: "hello"; role: Role; client_id: number; version: string }
  | { t: "state"; rev: number; state: State }
  | { t: "patch"; rev: number; ops: PatchOp[] }
  | Tick
  | { t: "log"; entries: LogEntry[] }
  | { t: "ack"; id: number; ok: boolean; result?: unknown; error?: string }
  | { t: "pong"; ts: number }
  | { t: "kicked" };

// ---------- commands ----------

/** JSON merge patch (RFC 7396) of T: arrays replace, null deletes/sets null. */
export type MergePatch<T> = T extends readonly unknown[]
  ? T
  : T extends object
    ? { [K in keyof T]?: MergePatch<T[K]> | null }
    : T;

/** Cue patches are intentionally loose in `kind` because it is a tagged union. */
export type CuePatch = Omit<MergePatch<Cue>, "kind" | "id"> & { kind?: Record<string, unknown> };
export type ShowPatch = MergePatch<Omit<Show, "cue_lists" | "schema_version" | "next_id">>;

export type Command =
  // transport
  | { cmd: "go"; list?: number }
  | { cmd: "stop_all" | "panic" | "pause_all" | "resume_all" | "toggle_pause" }
  | { cmd: "next" | "previous"; list?: number }
  | { cmd: "set_playhead"; cue: number | null; list?: number }
  | { cmd: "goto_number"; number: string }
  | { cmd: "start_cue" | "stop_cue" | "pause_cue" | "resume_cue" | "reset_cue"; cue: number }
  | { cmd: "load_cue"; cue: number; time: number }
  | { cmd: "arm_cue"; cue: number; armed: boolean }
  | { cmd: "reset" | "reset_show_clock" | "midi_panic" | "reset_clip" }
  | { cmd: "tc_start"; from?: string }
  | { cmd: "tc_stop" | "tc_pause" | "tc_resume" }
  | { cmd: "tc_relocate"; to: string }
  | { cmd: "set_master"; db: number }
  | { cmd: "set_notes"; text: string }
  // editing
  | { cmd: "add_cue"; kind: CueType; list?: number; after?: number; parent?: number; file?: string; target?: number }
  | { cmd: "update_cue"; cue: number; patch: CuePatch }
  | { cmd: "update_cues"; cues: number[]; patch: CuePatch }
  | { cmd: "delete_cues" | "duplicate_cues" | "group_cues"; cues: number[] }
  | { cmd: "move_cues"; cues: number[]; list?: number; parent?: number | null; index: number }
  | { cmd: "renumber"; cues: number[]; start: number; step: number }
  | { cmd: "add_list"; name: string }
  | { cmd: "update_list"; list: number; patch: MergePatch<Omit<CueList, "cues" | "id">> }
  | { cmd: "delete_list"; list: number }
  | { cmd: "update_show"; patch: ShowPatch }
  | { cmd: "apply_layout"; layout: string }
  | { cmd: "undo" | "redo" }
  // application
  | { cmd: "set_mode"; mode: Mode; force?: boolean }
  | { cmd: "run_preflight" | "collect_media" | "clear_log" | "check_update" | "install_update" }
  | { cmd: "quit"; force?: boolean }
  | { cmd: "new_show"; force?: boolean }
  | { cmd: "open_show"; path: string; force?: boolean }
  | { cmd: "save_show"; path?: string }
  | { cmd: "recover"; accept: boolean }
  | { cmd: "kick_client"; id: number }
  | { cmd: "rtp_connect"; addr: string };

/** Error thrown by `send()`; `code` is the server's error string (e.g. `forbidden`, `live_mode`). */
export class CommandError extends Error {
  readonly code: string;
  constructor(code: string) {
    super(code);
    this.name = "CommandError";
    this.code = code;
  }
}

// ---------- HTTP ----------

export interface SessionInfo {
  role: Role | null;
  token: string;
  view_needs_pin: boolean;
  version: string;
}

export interface LoginResult {
  token: string;
  role: Role;
}

export interface FsEntry {
  name: string;
  path: string;
  dir: boolean;
  size: number;
}

export interface FsListing {
  path: string;
  parent: string | null;
  entries: FsEntry[];
}

export interface Waveform {
  duration: number;
  channels: number;
  sample_rate: number;
  peaks: [number, number][];
}

export class HttpError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "HttpError";
    this.status = status;
  }
}

async function getJson<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, { cache: "no-store", credentials: "same-origin", ...init });
  if (!res.ok) throw new HttpError(res.status, `${res.status} ${res.statusText}`);
  // JSON boundary: the payload is trusted to follow the contract.
  return (await res.json()) as T;
}

export function fetchSession(token?: string | null): Promise<SessionInfo> {
  return getJson<SessionInfo>("/api/session", token ? { headers: { Authorization: `Bearer ${token}` } } : undefined);
}

export function login(pin: string, role: "control" | "view"): Promise<LoginResult> {
  return getJson<LoginResult>("/api/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ pin, role }),
  });
}

export function fetchFs(token: string, path: string): Promise<FsListing> {
  return getJson<FsListing>(`/api/fs?token=${encodeURIComponent(token)}&path=${encodeURIComponent(path)}`);
}

export function fetchWaveform(token: string, file: string, buckets: number, signal?: AbortSignal): Promise<Waveform> {
  return getJson<Waveform>(
    `/api/waveform?token=${encodeURIComponent(token)}&file=${encodeURIComponent(file)}&buckets=${buckets}`,
    { signal },
  );
}

export function qrUrl(role: "control" | "view", token: string): string {
  return `/api/qr.svg?role=${role}&token=${encodeURIComponent(token)}`;
}

export function diagnosticsUrl(token: string): string {
  return `/api/diagnostics?token=${encodeURIComponent(token)}`;
}
