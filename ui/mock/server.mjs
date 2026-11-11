// Show Master – MOCK backend for UI development and the e2e smoke test.
// Implements "enough" of docs/remote-api.md. It is a dev tool only and never shipped.
//
//   node mock/server.mjs            # port 7400 (env SM_MOCK_PORT), serves ../dist if present
//
// Role simulation: requests whose Host header is 127.0.0.1 / [::1] count as loopback (admin).
// Any other host name – including "localhost" – is treated like a remote device and needs a PIN
// (control PIN 1234, view PIN empty by default). Set SM_MOCK_ALL_ADMIN=1 to disable that.
import http from "node:http";
import { readFile, stat } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { WebSocketServer } from "ws";

const HERE = dirname(fileURLToPath(import.meta.url));
const VERSION = "0.1.0-mock";

const MIME = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".webmanifest": "application/manifest+json",
  ".ico": "image/x-icon", ".map": "application/json", ".txt": "text/plain; charset=utf-8",
};

const TRANSPORT = new Set([
  "go", "stop_all", "panic", "pause_all", "resume_all", "toggle_pause", "next", "previous", "set_playhead",
  "goto_number", "start_cue", "stop_cue", "pause_cue", "resume_cue", "reset_cue", "load_cue", "arm_cue", "reset",
  "reset_show_clock", "tc_start", "tc_stop", "tc_pause", "tc_resume", "tc_relocate", "set_master", "midi_panic",
  "reset_clip", "set_notes",
]);
const EDITING = new Set([
  "add_cue", "update_cue", "update_cues", "delete_cues", "duplicate_cues", "move_cues", "renumber", "group_cues",
  "add_list", "update_list", "delete_list", "update_show", "apply_layout", "undo", "redo",
]);

const LAYOUTS = {
  mono: ["C"], stereo: ["L", "R"], lcr: ["L", "C", "R"], quad: ["L", "R", "Ls", "Rs"],
  "5.1": ["L", "R", "C", "LFE", "Ls", "Rs"], "7.1": ["L", "R", "C", "LFE", "Lss", "Rss", "Lrs", "Rrs"],
};

const clone = (x) => (x === undefined ? undefined : JSON.parse(JSON.stringify(x)));

// ---------------------------------------------------------------- demo show

function audioKind(file, dur, extra = {}) {
  return {
    type: "audio", file, hash: null, file_duration: dur, channels: 2, start: 0, end: null, gain_db: 0, fade_in: 0,
    fade_out: 0, fade_curve: "equal_power", loop_count: 1, routing: [], ...extra,
  };
}
function cue(id, number, name, kind, extra = {}) {
  return {
    id, number, name, notes: "", color: "", pre_wait: 0, post_wait: 0, continue_mode: "do_not_continue", armed: true,
    tc_trigger: null, kind, ...extra,
  };
}

function demoShow() {
  return {
    schema_version: 2, name: "Demo Show", next_id: 40, notes: "Doors 19:30 · Interval after cue 9 (20 min)\nSM calls on channel A.",
    settings: {
      go_debounce_ms: 250, panic_fade_ms: 0, stop_fade_ms: 500, panic_double_tap: false, double_tap_ms: 600,
      quit_lock: true, high_contrast: false,
      audio: { backend: null, device: null, sample_rate: 48000, buffer_frames: 256, channels: 8 },
      remote: { enabled: true, port: 7400, control_pin: "1234", view_pin: "", osc_port: 0 },
      timer_alarms: [{ at: 3600, label: "Interval in 5 min" }],
    },
    patch: {
      outputs: [
        { name: "Main L", channel: 0 }, { name: "Main R", channel: 1 }, { name: "Centre", channel: 2 },
        { name: "Sub", channel: 3 }, { name: "Surr L", channel: 4 }, { name: "Surr R", channel: 5 },
        { name: "Foldback", channel: 6 }, { name: "Booth", channel: 7 },
      ],
      groups: [
        { name: "Main", layout: "stereo", outputs: [0, 1] },
        { name: "Surround", layout: "stereo", outputs: [4, 5] },
        { name: "All", layout: "5.1", outputs: [0, 1, 2, 3, 4, 5] },
      ],
    },
    midi_ports: [{ name: "Desk", device: "IAC Bus 1" }, { name: "Lights", device: "rtp:Show Master" }],
    rtp_midi: { enabled: true, session_name: "Show Master", port: 5004, connect_to: ["192.168.1.20:5004"], journal: false },
    timecode: {
      rate: "25", start: "01:00:00:00", mode: "freerun", ltc_enabled: true, ltc_channel: 8, ltc_level_db: -12,
      mtc_enabled: false, mtc_port: "",
    },
    hotkeys: {
      Space: "go", Escape: "panic", KeyS: "stop_all", KeyP: "toggle_pause", ArrowDown: "next", ArrowUp: "previous",
      "Ctrl+KeyR": "reset", KeyV: "preview", "Ctrl+KeyL": "toggle_mode", "Ctrl+KeyZ": "undo",
      "Ctrl+Shift+KeyZ": "redo", "Ctrl+KeyS": "save", "Ctrl+KeyD": "duplicate", Delete: "delete", "Ctrl+KeyF": "find",
    },
    cue_lists: [
      {
        id: 1, name: "Main", tc_jump: "fire",
        cues: [
          cue(1, "1", "Preshow music", audioKind("/shows/demo/media/preshow_loop.wav", 184.0, { loop_count: 0, gain_db: -6, fade_in: 2 }), { color: "blue", notes: "Start at house open. Loops until cue 3." }),
          cue(2, "2", "House open – check foyer bell", { type: "memo" }, { notes: "Wait for FOH clearance." }),
          cue(3, "3", "Fade out preshow", { type: "fade", target: 1, duration: 6, curve: "equal_power", gain_db: -120, crosspoints: [], stop_target: true }),
          cue(4, "4", "Start timecode", { type: "timecode", tc: { action: "start_from", from: "01:00:00:00" } }, { continue_mode: "auto_continue" }),
          cue(5, "5", "Overture", audioKind("/shows/demo/media/overture.wav", 42.5, { fade_out: 3 }), { color: "green", notes: "Curtain rises at the timpani hit." }),
          cue(6, "6", "Lights: scene 1 (MSC)", { type: "midi", port: "Lights", message: { msg: "msc", device_id: 1, command_format: 1, command: "go", q_number: "12.5", q_list: "1", q_path: "" } }),
          cue(7, "7", "Thunderstorm", { type: "group", mode: "parallel", children: [
            cue(8, "7.1", "Rain bed", audioKind("/shows/demo/media/rain.wav", 95.0, { gain_db: -9, routing: [{ src: 0, out: 4, db: 0 }, { src: 1, out: 5, db: 0 }] })),
            cue(9, "7.2", "Thunder crack", audioKind("/shows/demo/media/thunder.wav", 8.2, { channels: 1, routing: [{ src: 0, out: 0, db: 0 }, { src: 0, out: 1, db: 0 }, { src: 0, out: 3, db: -3 }] }), { pre_wait: 1.5 }),
          ] }, { color: "purple" }),
          cue(10, "8", "Doorbell", audioKind("/shows/demo/media/doorbell.wav", 2.4, { channels: 1 }), { continue_mode: "auto_follow" }),
          cue(11, "9", "Door slam", audioKind("/shows/demo/media/door_slam.wav", 1.8), { pre_wait: 0.5 }),
          cue(12, "10", "Desk: scene recall", { type: "midi", port: "Desk", message: { msg: "program_change", channel: 1, program: 12 } }),
          cue(13, "11", "Phone ring (missing file)", audioKind("/shows/demo/media/phone_ring.wav", null), { color: "red" }),
          cue(14, "12", "Interval music", audioKind("/shows/demo/media/interval.wav", 600.0, { gain_db: -10 }), { armed: false, notes: "Disarmed for tech rehearsal." }),
          cue(15, "13", "Stop rain", { type: "control", action: "stop", target: 8, time: 0 }),
          cue(16, "14", "Finale", audioKind("/shows/demo/media/finale.wav", 128.3, { fade_in: 1.5 }), { tc_trigger: "01:12:30:00", color: "orange" }),
          cue(17, "15", "Stop timecode", { type: "timecode", tc: { action: "stop" } }),
          cue(18, "16", "Playout", audioKind("/shows/demo/media/playout.wav", 240.0, { gain_db: -3 })),
        ],
      },
    ],
  };
}

const FS_TREE = {
  "/": ["home", "shows", "Volumes"],
  "/home": ["operator"],
  "/home/operator": ["Music", "Desktop", "notes.smshow"],
  "/home/operator/Music": ["walk_in_playlist.flac", "bows.mp3"],
  "/home/operator/Desktop": [],
  "/Volumes": ["USB_STICK"],
  "/Volumes/USB_STICK": ["director_track_v7_FINAL.wav"],
  "/shows": ["demo"],
  "/shows/demo": ["media", "demo.smshow", "demo_backup.smshow"],
  "/shows/demo/media": [
    "preshow_loop.wav", "overture.wav", "rain.wav", "thunder.wav", "doorbell.wav", "door_slam.wav", "interval.wav",
    "finale.wav", "playout.wav", "applause_long.wav", "sting_brass.aiff",
  ],
};

// ---------------------------------------------------------------- server

export async function startMock(opts = {}) {
  const port = opts.port ?? Number(process.env.SM_MOCK_PORT ?? 7400);
  const distDir = resolve(opts.dist ?? join(HERE, "..", "dist"));
  const allAdmin = opts.allAdmin ?? process.env.SM_MOCK_ALL_ADMIN === "1";
  const quiet = opts.quiet ?? false;

  // ---- state ----
  const tokens = new Map(); // token -> role
  const adminToken = "adm-" + randomBytes(8).toString("hex");
  tokens.set(adminToken, "admin");
  let nextClientId = 1;
  const clients = new Set(); // { ws, id, role, addr, since_ms, agent }
  let rev = 1;
  let wsBlockedUntil = 0;

  let state = {
    version: VERSION, mode: "edit", show: demoShow(), show_path: "/shows/demo/demo.smshow", dirty: false,
    can_undo: false, can_redo: false, recovery_available: false, playheads: { 1: 1 }, errors: { 13: "file not found: phone_ring.wav" },
    loaded: {}, paused_all: false, master_db: 0,
    audio: {
      backend: "cpal", device: "Mock Interface 8", sample_rate: 48000, channels: 8, buffer_frames: 256, is_null: false, error: null,
      devices: [
        { backend: "cpal", id: "mock8", name: "Mock Interface 8", max_output_channels: 8, sample_rates: [44100, 48000, 96000], is_default: true },
        { backend: "cpal", id: "builtin", name: "Built-in Output", max_output_channels: 2, sample_rates: [44100, 48000], is_default: false },
        { backend: "null", id: "null", name: "No audio (silent)", max_output_channels: 64, sample_rates: [48000], is_default: false },
      ],
    },
    midi: {
      ports: ["IAC Bus 1", "USB MIDI Interface", "rtp:Show Master"],
      rtp: { enabled: true, name: "Show Master", port: 5004, peers: [{ name: "iPad Stage Left", addr: "192.168.1.20:5004", state: "connected", latency_ms: 1.2 }] },
    },
    remote: { urls: [`http://192.168.1.10:${port}`], port, clients: [] },
    preflight: [],
    layouts: Object.keys(LAYOUTS),
    update: { state: "current", current: "0.1.0", version: null, notes: null, automatic: null, download_page: "", done: 0, total: 0, error: null },
  };
  let sent = clone(state); // what the clients have
  const undoStack = [];
  const redoStack = [];

  // engine (not part of the state document)
  const active = new Map(); // cue id -> { cue, list, phase, paused, elapsed, duration, from_tc, timers }
  let showStart = null; // ms
  let frame = 0;
  const tc = { running: false, frames: 0, last: Date.now() };
  const clip = [];
  const logBuf = [];

  // ---- helpers ----
  const log = (level, message) => {
    const entry = { time_ms: Date.now(), level, message };
    logBuf.push(entry);
    if (logBuf.length > 200) logBuf.shift();
    broadcast({ t: "log", entries: [entry] });
  };

  function broadcast(msg) {
    const text = JSON.stringify(msg);
    for (const c of clients) if (c.ws.readyState === 1) c.ws.send(text);
  }

  function diff(a, b, path, ops) {
    if (a === b) return;
    const plain = (x) => typeof x === "object" && x !== null && !Array.isArray(x);
    if (plain(a) && plain(b)) {
      for (const k of Object.keys(a)) if (!(k in b)) ops.push({ p: [...path, k], d: true });
      for (const k of Object.keys(b)) {
        if (!(k in a)) ops.push({ p: [...path, k], v: b[k] });
        else diff(a[k], b[k], [...path, k], ops);
      }
      return;
    }
    if (JSON.stringify(a) !== JSON.stringify(b)) ops.push({ p: path, v: b });
  }

  function flush() {
    state.remote.clients = [...clients].map((c) => ({ id: c.id, role: c.role, addr: c.addr, since_ms: c.since_ms, agent: c.agent }));
    state.can_undo = undoStack.length > 0;
    state.can_redo = redoStack.length > 0;
    const ops = [];
    diff(sent, state, [], ops);
    if (ops.length === 0) return;
    rev += 1;
    sent = clone(state);
    broadcast({ t: "patch", rev, ops: clone(ops) });
  }

  const rows = (cues, out = [], parent = null) => {
    for (const c of cues) {
      out.push({ cue: c, parent });
      if (c.kind.type === "group") rows(c.kind.children, out, c);
    }
    return out;
  };
  const allRows = () => state.show.cue_lists.flatMap((l) => rows(l.cues).map((r) => ({ ...r, list: l })));
  const find = (id) => allRows().find((r) => r.cue.id === Number(id)) ?? null;
  const listById = (id) => state.show.cue_lists.find((l) => l.id === Number(id)) ?? state.show.cue_lists[0];
  const containerOf = (r) => (r.parent ? r.parent.kind.children : r.list.cues);

  function mergePatch(target, patch) {
    if (typeof patch !== "object" || patch === null || Array.isArray(patch)) return patch;
    const out = typeof target === "object" && target !== null && !Array.isArray(target) ? target : {};
    for (const [k, v] of Object.entries(patch)) {
      if (v === null && !(k in out && out[k] === null) && !NULLABLE.has(k)) delete out[k];
      else if (v === null) out[k] = null;
      else out[k] = mergePatch(out[k], v);
    }
    return out;
  }
  // keys whose value may legitimately be null in the documents (merge-patch null = set null, not remove)
  const NULLABLE = new Set(["tc_trigger", "end", "gain_db", "target", "hash", "file_duration", "backend", "device"]);

  function checkpoint() {
    undoStack.push(JSON.stringify(state.show));
    if (undoStack.length > 200) undoStack.shift();
    redoStack.length = 0;
    state.dirty = true;
  }

  function validateErrors() {
    const errors = {};
    for (const { cue: c } of allRows()) {
      const k = c.kind;
      if (k.type === "audio") {
        if (!k.file) errors[c.id] = "no file assigned";
        else if (!fileExists(k.file)) errors[c.id] = `file not found: ${k.file.split("/").pop()}`;
      } else if (k.type === "fade" && (k.target == null || !find(k.target))) errors[c.id] = "fade has no target";
      else if (k.type === "control" && k.action !== "reset" && (k.target == null || !find(k.target))) errors[c.id] = "control cue has no target";
      else if (k.type === "midi" && !k.port) errors[c.id] = "no MIDI port selected";
    }
    state.errors = errors;
  }

  function fileExists(path) {
    const dir = path.slice(0, path.lastIndexOf("/")) || "/";
    return (FS_TREE[dir] ?? []).includes(path.slice(path.lastIndexOf("/") + 1));
  }
  const fakeDuration = (path) => 5 + ((path.length * 37) % 900) / 10;

  function runPreflight() {
    validateErrors();
    const out = [];
    for (const { cue: c } of allRows()) {
      if (state.errors[c.id]) out.push({ severity: "error", cue: c.id, message: `Cue ${c.number}: ${state.errors[c.id]}` });
      if (!c.armed) out.push({ severity: "warning", cue: c.id, message: `Cue ${c.number} "${c.name}" is disarmed` });
      if (c.kind.type === "audio" && c.kind.gain_db > 6) out.push({ severity: "warning", cue: c.id, message: `Cue ${c.number}: gain above +6 dB` });
    }
    if (state.dirty) out.push({ severity: "warning", cue: null, message: "Show has unsaved changes" });
    out.push({ severity: "info", cue: null, message: `Audio device "${state.audio.device}" – ${state.audio.channels} outputs at ${state.audio.sample_rate} Hz` });
    out.push({ severity: "info", cue: null, message: `${allRows().length} cues checked` });
    state.preflight = out;
  }

  // ---- playback engine (very rough simulation) ----
  function actionDuration(c) {
    const k = c.kind;
    if (k.type === "audio") {
      if (k.loop_count === 0) return null;
      const end = k.end ?? k.file_duration ?? 0;
      return Math.max(0, end - k.start) * Math.max(1, k.loop_count);
    }
    if (k.type === "fade") return k.duration;
    if (k.type === "group") return null;
    return 0;
  }

  function nextSibling(c) {
    const r = find(c.id);
    if (!r) return null;
    const cont = containerOf(r);
    return cont[cont.indexOf(r.cue) + 1] ?? null;
  }

  function fire(c, listId, fromTc = false) {
    if (!c.armed) {
      log("warn", `Cue ${c.number} is disarmed – skipped`);
      return;
    }
    if (state.errors[c.id]) {
      log("error", `Cue ${c.number} failed: ${state.errors[c.id]}`);
      return;
    }
    stopCue(c.id);
    const a = { cue: c.id, list: listId, phase: c.pre_wait > 0 ? "pre_wait" : "running", paused: false, elapsed: 0, duration: c.pre_wait > 0 ? c.pre_wait : actionDuration(c), from_tc: fromTc, started: false, sincePost: 0 };
    active.set(c.id, a);
    if (showStart == null) showStart = Date.now();
    log("info", `GO cue ${c.number} "${c.name}"`);
    if (a.phase === "running") startAction(c, a);
    if (c.continue_mode === "auto_continue") a.continueIn = c.pre_wait + c.post_wait;
  }

  function startAction(c, a) {
    a.started = true;
    a.phase = "running";
    a.elapsed = 0;
    a.duration = actionDuration(c);
    const k = c.kind;
    if (k.type === "group") {
      if (k.mode === "parallel") for (const ch of k.children) fire(ch, a.list);
      else if (k.children[0]) fire(k.mode === "random" ? k.children[Math.floor(Math.random() * k.children.length)] : k.children[0], a.list);
      a.duration = 0;
    } else if (k.type === "midi") log("info", `MIDI → ${k.port}: ${k.message.msg}`);
    else if (k.type === "timecode") {
      if (k.tc.action === "start" || k.tc.action === "resume") tcStart();
      else if (k.tc.action === "start_from") tcStart(k.tc.from);
      else if (k.tc.action === "relocate") tc.frames = tcToFrames(k.tc.to);
      else tc.running = false;
    } else if (k.type === "control") {
      if (k.action === "stop") stopCue(k.target);
      else if (k.action === "start") { const t = find(k.target); if (t) fire(t.cue, a.list); }
      else if (k.action === "pause") { const t = active.get(k.target); if (t) t.paused = true; }
      else if (k.action === "resume") { const t = active.get(k.target); if (t) t.paused = false; }
      else if (k.action === "arm" || k.action === "disarm") { const t = find(k.target); if (t) t.cue.armed = k.action === "arm"; }
      else if (k.action === "go_to") state.playheads[a.list] = k.target;
      else if (k.action === "load_to_time") state.loaded[k.target] = k.time;
    }
  }

  function stopCue(id) {
    const r = find(id);
    active.delete(Number(id));
    if (r && r.cue.kind.type === "group") for (const ch of r.cue.kind.children) stopCue(ch.id);
  }

  function step(dt) {
    for (const a of [...active.values()]) {
      if (a.paused || state.paused_all) continue;
      const r = find(a.cue);
      if (!r) { active.delete(a.cue); continue; }
      const c = r.cue;
      a.elapsed += dt;
      if (a.continueIn != null) {
        a.continueIn -= dt;
        if (a.continueIn <= 0) {
          a.continueIn = null;
          const n = nextSibling(c);
          if (n) fire(n, a.list);
        }
      }
      if (a.phase === "pre_wait" && a.elapsed >= c.pre_wait) startAction(c, a);
      else if (a.phase === "running" && a.duration != null && a.elapsed >= a.duration) {
        if (c.kind.type === "fade" && c.kind.stop_target && c.kind.target != null) stopCue(c.kind.target);
        if (c.kind.type === "group" && c.kind.children.some((ch) => active.has(ch.id))) { a.duration = null; continue; }
        if (c.continue_mode === "auto_follow") { const n = nextSibling(c); if (n) fire(n, a.list); }
        if (c.post_wait > 0 && a.continueIn != null) { a.phase = "post_wait"; a.duration = c.post_wait; a.elapsed = 0; }
        else if (a.continueIn == null) active.delete(a.cue);
      } else if (a.phase === "running" && a.duration == null && c.kind.type === "group") {
        if (!c.kind.children.some((ch) => active.has(ch.id))) active.delete(a.cue);
      } else if (a.phase === "post_wait" && a.elapsed >= (a.duration ?? 0)) active.delete(a.cue);
    }
  }

  function movePlayhead(listId, delta) {
    const list = listById(listId);
    const top = list.cues;
    const cur = state.playheads[list.id];
    const flat = rows(top).map((r) => r.cue.id);
    let idx = cur == null ? (delta > 0 ? -1 : flat.length) : flat.indexOf(cur);
    idx += delta;
    state.playheads[list.id] = idx >= 0 && idx < flat.length ? flat[idx] : delta > 0 ? null : flat[0] ?? null;
  }

  function go(listId) {
    const list = listById(listId);
    const id = state.playheads[list.id];
    const r = id == null ? null : find(id);
    if (!r) { log("warn", "GO: no cue at the playhead"); return; }
    fire(r.cue, list.id);
    // advance: skip the cue's children and everything it auto-continues into
    const cont = containerOf(r);
    let i = cont.indexOf(r.cue);
    while (cont[i] && cont[i].continue_mode !== "do_not_continue" && cont[i + 1]) i++;
    let next = cont[i + 1] ?? null;
    if (!next && r.parent) { const pr = find(r.parent.id); const pc = containerOf(pr); next = pc[pc.indexOf(pr.cue) + 1] ?? null; }
    state.playheads[list.id] = next ? next.id : null;
  }

  const fps = () => ({ "23.976": 24, 24: 24, 25: 25 })[state.show.timecode.rate] ?? 30;
  function tcToFrames(s) {
    const m = /^(\d\d):(\d\d):(\d\d)[:;](\d\d)$/.exec(s ?? "");
    if (!m) return 0;
    return ((Number(m[1]) * 60 + Number(m[2])) * 60 + Number(m[3])) * fps() + Number(m[4]);
  }
  function framesToTc(f) {
    const n = fps();
    f = Math.floor(f);
    const p = (x) => String(x).padStart(2, "0");
    return `${p(Math.floor(f / n / 3600) % 24)}:${p(Math.floor(f / n / 60) % 60)}:${p(Math.floor(f / n) % 60)}:${p(f % n)}`;
  }
  function tcStart(from) {
    tc.frames = tcToFrames(from ?? state.show.timecode.start);
    tc.running = true;
  }
  tc.frames = tcToFrames(state.show.timecode.start);

  function resetAll() {
    active.clear();
    state.paused_all = false;
    state.loaded = {};
    validateErrors();
    for (const l of state.show.cue_lists) state.playheads[l.id] = l.cues[0]?.id ?? null;
    showStart = null;
    tc.running = false;
    tc.frames = tcToFrames(state.show.timecode.start);
  }

  function newCue(kind, c) {
    const id = state.show.next_id++;
    const kinds = {
      audio: () => audioKind(c.file ?? "", c.file ? (fileExists(c.file) ? fakeDuration(c.file) : null) : null),
      fade: () => ({ type: "fade", target: c.target ?? null, duration: 3, curve: "equal_power", gain_db: -120, crosspoints: [], stop_target: true }),
      midi: () => ({ type: "midi", port: state.show.midi_ports[0]?.name ?? "", message: { msg: "note_on", channel: 1, note: 60, velocity: 100 } }),
      timecode: () => ({ type: "timecode", tc: { action: "start" } }),
      control: () => ({ type: "control", action: "stop", target: c.target ?? null, time: 0 }),
      group: () => ({ type: "group", mode: "parallel", children: [] }),
      memo: () => ({ type: "memo" }),
    };
    if (!kinds[kind]) throw new Error("unknown cue kind");
    const name = kind === "audio" && c.file ? c.file.split("/").pop().replace(/\.[^.]+$/, "") : "";
    return cue(id, "", name, kinds[kind]());
  }

  function nextNumber(list) {
    let max = 0;
    for (const c of list.cues) { const n = parseFloat(c.number); if (Number.isFinite(n)) max = Math.max(max, n); }
    return String(Math.floor(max) + 1);
  }

  function reId(c) {
    c.id = state.show.next_id++;
    if (c.kind.type === "group") c.kind.children.forEach(reId);
    return c;
  }

  // ---- command execution ----
  function exec(c, role) {
    const name = c?.cmd;
    if (typeof name !== "string") throw new Error("bad_command");
    if (role === "view") throw new Error("forbidden");
    if (!TRANSPORT.has(name) && role !== "admin") throw new Error("forbidden");
    if (EDITING.has(name) && state.mode === "live") throw new Error("live_mode");
    let result = null;
    switch (name) {
      // transport
      case "go": go(c.list); break;
      case "stop_all": active.clear(); state.paused_all = false; log("info", "STOP ALL"); break;
      case "panic": active.clear(); state.paused_all = false; log("warn", "PANIC"); break;
      case "pause_all": state.paused_all = true; break;
      case "resume_all": state.paused_all = false; break;
      case "toggle_pause": state.paused_all = !state.paused_all; log("info", state.paused_all ? "Paused" : "Resumed"); break;
      case "next": movePlayhead(c.list, 1); break;
      case "previous": movePlayhead(c.list, -1); break;
      case "set_playhead": {
        if (c.cue != null && !find(c.cue)) throw new Error("unknown_cue");
        const l = c.cue != null ? find(c.cue).list : listById(c.list);
        state.playheads[l.id] = c.cue ?? null;
        break;
      }
      case "goto_number": {
        const r = allRows().find((x) => x.cue.number === String(c.number));
        if (!r) throw new Error(`no cue with number ${c.number}`);
        state.playheads[r.list.id] = r.cue.id;
        break;
      }
      case "start_cue": { const r = find(c.cue); if (!r) throw new Error("unknown_cue"); fire(r.cue, r.list.id); break; }
      case "stop_cue": stopCue(c.cue); break;
      case "pause_cue": { const a = active.get(Number(c.cue)); if (a) a.paused = true; break; }
      case "resume_cue": { const a = active.get(Number(c.cue)); if (a) a.paused = false; break; }
      case "reset_cue": stopCue(c.cue); delete state.loaded[c.cue]; break;
      case "load_cue": state.loaded[c.cue] = Number(c.time) || 0; break;
      case "arm_cue": { const r = find(c.cue); if (!r) throw new Error("unknown_cue"); r.cue.armed = !!c.armed; break; }
      case "reset": resetAll(); log("info", "Reset"); break;
      case "reset_show_clock": showStart = null; break;
      case "tc_start": tcStart(c.from); break;
      case "tc_stop": tc.running = false; tc.frames = tcToFrames(state.show.timecode.start); break;
      case "tc_pause": tc.running = false; break;
      case "tc_resume": tc.running = true; break;
      case "tc_relocate": tc.frames = tcToFrames(c.to); break;
      case "set_master": state.master_db = Math.max(-120, Math.min(12, Number(c.db) || 0)); break;
      case "midi_panic": log("warn", "MIDI panic (all notes off)"); break;
      case "reset_clip": clip.length = 0; break;
      case "set_notes": state.show.notes = String(c.text ?? ""); state.dirty = true; break;

      // editing
      case "add_cue": {
        checkpoint();
        const nc = newCue(c.kind, c);
        const after = c.after != null ? find(c.after) : null;
        const parent = c.parent != null ? find(c.parent) : null;
        const list = after?.list ?? parent?.list ?? listById(c.list);
        nc.number = nextNumber(list);
        if (parent && parent.cue.kind.type === "group") parent.cue.kind.children.push(nc);
        else if (after) { const cont = containerOf(after); cont.splice(cont.indexOf(after.cue) + 1, 0, nc); }
        else list.cues.push(nc);
        result = { cue: nc.id };
        break;
      }
      case "update_cue": case "update_cues": {
        const ids = name === "update_cue" ? [c.cue] : c.cues;
        if (!Array.isArray(ids) || ids.some((id) => !find(id))) throw new Error("unknown_cue");
        checkpoint();
        for (const id of ids) {
          const r = find(id);
          const p = clone(c.patch);
          delete p.id;
          // changing kind.type replaces the kind wholesale
          if (p.kind?.type && p.kind.type !== r.cue.kind.type) r.cue.kind = p.kind;
          else mergePatch(r.cue, p);
          if (r.cue.kind.type === "audio" && p.kind?.file !== undefined) {
            r.cue.kind.file_duration = fileExists(r.cue.kind.file) ? fakeDuration(r.cue.kind.file) : null;
          }
        }
        break;
      }
      case "delete_cues": {
        checkpoint();
        for (const id of c.cues ?? []) {
          const r = find(id);
          if (!r) continue;
          stopCue(id);
          const cont = containerOf(r);
          cont.splice(cont.indexOf(r.cue), 1);
          for (const k of Object.keys(state.playheads)) if (state.playheads[k] === Number(id)) state.playheads[k] = null;
        }
        break;
      }
      case "duplicate_cues": {
        checkpoint();
        const ids = [];
        for (const id of c.cues ?? []) {
          const r = find(id);
          if (!r) continue;
          const copy = reId(clone(r.cue));
          copy.name = copy.name ? `${copy.name} copy` : copy.name;
          const cont = containerOf(r);
          cont.splice(cont.indexOf(r.cue) + 1, 0, copy);
          ids.push(copy.id);
        }
        result = { cues: ids };
        break;
      }
      case "move_cues": {
        const moving = (c.cues ?? []).map(find).filter(Boolean);
        if (moving.length === 0) break;
        const parent = c.parent != null ? find(c.parent) : null;
        if (c.parent != null && (!parent || parent.cue.kind.type !== "group")) throw new Error("parent is not a group");
        const within = (g, id) => g.id === id || (g.kind.type === "group" && g.kind.children.some((x) => within(x, id)));
        if (parent && moving.some((m) => within(m.cue, parent.cue.id))) throw new Error("cannot move a group into itself");
        checkpoint();
        const dest = parent ? parent.cue.kind.children : (c.list != null ? listById(c.list) : moving[0].list).cues;
        let index = Math.max(0, Math.min(dest.length, Number(c.index) || 0));
        // `index` refers to the destination BEFORE the moved cues are taken out
        for (const m of moving) {
          const cont = containerOf(m);
          const i = cont.indexOf(m.cue);
          if (cont === dest && i < index) index--;
          cont.splice(i, 1);
        }
        dest.splice(index, 0, ...moving.map((m) => m.cue));
        break;
      }
      case "renumber": {
        checkpoint();
        let n = Number(c.start) || 1;
        const stepN = Number(c.step) || 1;
        for (const id of c.cues ?? []) { const r = find(id); if (r) { r.cue.number = String(Math.round(n * 100) / 100); n += stepN; } }
        break;
      }
      case "group_cues": {
        const moving = (c.cues ?? []).map(find).filter(Boolean);
        if (moving.length === 0) break;
        checkpoint();
        const g = newCue("group", {});
        g.name = "Group";
        const cont = containerOf(moving[0]);
        const at = cont.indexOf(moving[0].cue);
        for (const m of moving) { const mc = containerOf(m); mc.splice(mc.indexOf(m.cue), 1); g.kind.children.push(m.cue); }
        cont.splice(Math.min(at, cont.length), 0, g);
        result = { cue: g.id };
        break;
      }
      case "add_list": {
        checkpoint();
        const id = state.show.next_id++;
        state.show.cue_lists.push({ id, name: c.name || "List", tc_jump: "fire", cues: [] });
        state.playheads[id] = null;
        result = { list: id };
        break;
      }
      case "update_list": { checkpoint(); const p = clone(c.patch ?? {}); delete p.cues; delete p.id; mergePatch(listById(c.list), p); break; }
      case "delete_list": {
        if (state.show.cue_lists.length <= 1) throw new Error("cannot delete the last list");
        checkpoint();
        state.show.cue_lists = state.show.cue_lists.filter((l) => l.id !== Number(c.list));
        delete state.playheads[c.list];
        break;
      }
      case "update_show": {
        const p = clone(c.patch ?? {});
        delete p.cue_lists;
        checkpoint();
        mergePatch(state.show, p);
        const a = state.show.settings.audio;
        const dev = state.audio.devices.find((d) => d.id === a.device) ?? state.audio.devices.find((d) => d.is_default);
        state.audio.device = dev.name;
        state.audio.backend = dev.backend;
        state.audio.is_null = dev.backend === "null";
        state.audio.sample_rate = a.sample_rate;
        state.audio.buffer_frames = a.buffer_frames;
        state.audio.channels = Math.min(a.channels, dev.max_output_channels);
        state.midi.rtp.enabled = state.show.rtp_midi.enabled;
        state.midi.rtp.name = state.show.rtp_midi.session_name;
        state.midi.rtp.port = state.show.rtp_midi.port;
        break;
      }
      case "apply_layout": {
        const names = LAYOUTS[c.layout];
        if (!names) throw new Error("unknown layout");
        checkpoint();
        state.show.patch = { outputs: names.map((n, i) => ({ name: n, channel: i })), groups: [{ name: c.layout, layout: c.layout, outputs: names.map((_, i) => i) }] };
        break;
      }
      case "undo": {
        if (!undoStack.length) throw new Error("nothing to undo");
        redoStack.push(JSON.stringify(state.show));
        state.show = JSON.parse(undoStack.pop());
        state.dirty = true;
        break;
      }
      case "redo": {
        if (!redoStack.length) throw new Error("nothing to redo");
        undoStack.push(JSON.stringify(state.show));
        state.show = JSON.parse(redoStack.pop());
        state.dirty = true;
        break;
      }

      // application
      case "set_mode": {
        if (c.mode !== "edit" && c.mode !== "live") throw new Error("bad mode");
        if (c.mode === state.mode) break;
        if (c.mode === "live") runPreflight();
        else if (active.size > 0 && !c.force) throw new Error("cues_running");
        state.mode = c.mode;
        log("info", `Mode: ${c.mode.toUpperCase()}`);
        break;
      }
      case "run_preflight": runPreflight(); result = clone(state.preflight); break;
      case "new_show": {
        if (state.dirty && !c.force) throw new Error("unsaved_changes");
        const s = demoShow();
        s.name = "Untitled"; s.notes = ""; s.cue_lists = [{ id: 1, name: "Main", tc_jump: "fire", cues: [] }];
        state.show = s; state.show_path = null; state.dirty = false; undoStack.length = 0; redoStack.length = 0;
        resetAll();
        break;
      }
      case "open_show": {
        if (state.dirty && !c.force) throw new Error("unsaved_changes");
        if (!String(c.path).endsWith(".smshow")) throw new Error("not a showfile");
        state.show = demoShow(); state.show_path = String(c.path); state.dirty = false; undoStack.length = 0; redoStack.length = 0;
        state.show.name = String(c.path).split("/").pop().replace(/\.smshow$/, "");
        resetAll();
        log("info", `Opened ${c.path}`);
        break;
      }
      case "save_show": {
        const path = c.path ?? state.show_path;
        if (!path) throw new Error("path required");
        state.show_path = path.endsWith(".smshow") ? path : `${path}.smshow`;
        state.dirty = false;
        log("info", `Saved ${state.show_path}`);
        break;
      }
      case "collect_media": log("info", "Collected media (mock)"); break;
      case "recover": state.recovery_available = false; break;
      case "kick_client": {
        const k = [...clients].find((x) => x.id === Number(c.id));
        if (!k) throw new Error("unknown client");
        k.ws.send(JSON.stringify({ t: "kicked" }));
        k.ws.close();
        break;
      }
      case "rtp_connect": log("info", `RTP-MIDI: inviting ${c.addr}`); break;
      case "clear_log": logBuf.length = 0; break;
      default: throw new Error(`unknown command ${name}`);
    }
    if (EDITING.has(name)) validateErrors();
    flush();
    return result;
  }

  // ---- tick ----
  function makeTick() {
    const now = Date.now();
    const audioRunning = [...active.values()].filter((a) => a.phase === "running" && !a.paused && !state.paused_all && find(a.cue)?.cue.kind.type === "audio");
    const n = state.audio.channels;
    const master = Math.pow(10, state.master_db / 20);
    const peak = [], rms = [];
    for (let i = 0; i < n; i++) {
      let lvl = 0;
      for (const a of audioRunning) {
        const k = find(a.cue).cue.kind;
        const outs = k.routing.length ? k.routing.map((r) => state.show.patch.outputs[r.out]?.channel) : [0, 1];
        if (outs.includes(i)) lvl += Math.pow(10, k.gain_db / 20) * (0.35 + 0.25 * Math.abs(Math.sin(now / 180 + i * 1.7 + a.cue)) + 0.1 * Math.random());
      }
      lvl *= master;
      peak.push(Math.min(1.2, lvl));
      rms.push(Math.min(1, lvl * 0.6));
      if (lvl >= 1) clip[i] = true;
    }
    let nextAuto = null;
    for (const a of active.values()) {
      const c = find(a.cue)?.cue;
      if (!c) continue;
      let t = null;
      if (a.continueIn != null) t = a.continueIn;
      else if (c.continue_mode === "auto_follow" && a.duration != null) t = (a.phase === "pre_wait" ? c.pre_wait - a.elapsed + (actionDuration(c) ?? 0) : a.duration - a.elapsed);
      if (t != null && nextSibling(c) && (nextAuto == null || t < nextAuto)) nextAuto = Math.max(0, t);
    }
    const elapsed = showStart == null ? null : (now - showStart) / 1000;
    const alarms = elapsed == null ? [] : state.show.settings.timer_alarms.filter((al) => elapsed >= al.at && elapsed < al.at + 10).map((al) => al.label);
    return {
      t: "tick", frame: frame++, wall_ms: now, show_elapsed: elapsed,
      tc: { position: framesToTc(tc.frames), running: tc.running, rate: state.show.timecode.rate, ltc: state.show.timecode.ltc_enabled, mtc: state.show.timecode.mtc_enabled },
      active: [...active.values()].map((a) => ({ cue: a.cue, list: a.list, phase: a.phase, paused: a.paused || state.paused_all, elapsed: Math.round(a.elapsed * 1000) / 1000, duration: a.duration, from_tc: a.from_tc })),
      next_auto: nextAuto,
      meters: { peak, rms, clip: Array.from({ length: n }, (_, i) => !!clip[i]) },
      cpu: 0.02 + audioRunning.length * 0.015, underruns: 0, voices: audioRunning.length, alarms,
    };
  }

  let lastStep = Date.now();
  const tickTimer = setInterval(() => {
    const now = Date.now();
    const dt = (now - lastStep) / 1000;
    lastStep = now;
    step(dt);
    if (tc.running) {
      tc.frames += dt * fps();
      const pos = framesToTc(tc.frames);
      for (const r of allRows()) {
        if (r.cue.tc_trigger && r.cue.tc_trigger === pos && !active.has(r.cue.id)) fire(r.cue, r.list.id, true);
      }
    }
    flush();
    broadcast(makeTick());
  }, 50);

  // ---- HTTP ----
  const hostIsLoopback = (req) => {
    if (allAdmin) return true;
    const host = String(req.headers.host ?? "").replace(/:\d+$/, "");
    return host === "127.0.0.1" || host === "[::1]";
  };
  const tokenOf = (req, url) => {
    const q = url.searchParams.get("token");
    const auth = req.headers.authorization;
    const t = q ?? (auth && auth.startsWith("Bearer ") ? auth.slice(7) : null);
    if (t && t.startsWith("pin:")) return t.slice(4) === state.show.settings.remote.control_pin ? "control" : null;
    return t ? tokens.get(t) ?? null : null;
  };
  const json = (res, code, body) => {
    res.writeHead(code, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    res.end(JSON.stringify(body));
  };
  const readBody = (req) => new Promise((ok) => { let b = ""; req.on("data", (d) => (b += d)); req.on("end", () => ok(b)); });

  async function serveStatic(res, pathname) {
    let rel = pathname === "/" || pathname === "/remote" || pathname === "/view" ? "/index.html" : pathname;
    const file = normalize(join(distDir, rel));
    if (!file.startsWith(distDir)) { res.writeHead(403); res.end(); return; }
    try {
      const st = await stat(file);
      if (!st.isFile()) throw new Error("not a file");
      const data = await readFile(file);
      res.writeHead(200, { "Content-Type": MIME[extname(file)] ?? "application/octet-stream", "Cache-Control": rel.startsWith("/assets/") ? "public, max-age=31536000, immutable" : "no-cache" });
      res.end(data);
    } catch {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end(pathname === "/index.html" || rel === "/index.html" ? "ui/dist not built – run `npm run build` (or use `npm run dev`)" : "not found");
    }
  }

  function fakeQr(text) {
    let h = 2166136261;
    const cells = [];
    const N = 25;
    const finder = (x, y) => { const f = (ox, oy) => x >= ox && x < ox + 7 && y >= oy && y < oy + 7 && (x === ox || x === ox + 6 || y === oy || y === oy + 6 || (x >= ox + 2 && x <= ox + 4 && y >= oy + 2 && y <= oy + 4)); const inF = (ox, oy) => x >= ox - 1 && x < ox + 8 && y >= oy - 1 && y < oy + 8; if (inF(0, 0)) return f(0, 0); if (inF(N - 7, 0)) return f(N - 7, 0); if (inF(0, N - 7)) return f(0, N - 7); return null; };
    for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) {
      h ^= text.charCodeAt((x + y * N) % text.length) + x * 31 + y * 17; h = Math.imul(h, 16777619);
      const f = finder(x, y);
      if (f ?? ((h >>> 13) & 1) === 1) cells.push(`<rect x="${x + 2}" y="${y + 2}" width="1" height="1"/>`);
    }
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${N + 4} ${N + 4}" shape-rendering="crispEdges"><rect width="100%" height="100%" fill="#fff"/><g fill="#000">${cells.join("")}</g></svg>`;
  }

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://x");
    const p = url.pathname;
    try {
      if (p === "/api/session") {
        if (hostIsLoopback(req)) return json(res, 200, { role: "admin", token: adminToken, view_needs_pin: state.show.settings.remote.view_pin !== "", version: VERSION });
        const role = tokenOf(req, url);
        const auth = req.headers.authorization;
        return json(res, 200, { role, token: role ? auth.slice(7) : "", view_needs_pin: state.show.settings.remote.view_pin !== "", version: VERSION });
      }
      if (p === "/api/login" && req.method === "POST") {
        let body = {};
        try { body = JSON.parse(await readBody(req)); } catch { /* bad json */ }
        const r = state.show.settings.remote;
        const want = body.role === "view" ? "view" : "control";
        const ok = want === "view" ? r.view_pin === "" || body.pin === r.view_pin : r.control_pin !== "" && body.pin === r.control_pin;
        if (!ok) return json(res, 401, { error: "wrong pin" });
        const token = `${want.slice(0, 3)}-${randomBytes(8).toString("hex")}`;
        tokens.set(token, want);
        return json(res, 200, { token, role: want });
      }
      if (p.startsWith("/api/")) {
        const role = tokenOf(req, url);
        if (!role) return json(res, 401, { ok: false, error: "unauthorized" });
        if (p === "/api/v1/state") return json(res, 200, { ...state, tick: makeTick() });
        if (p === "/api/v1/command" && req.method === "POST") {
          try { exec(JSON.parse(await readBody(req)), role); return json(res, 200, { ok: true }); }
          catch (e) { return json(res, 200, { ok: false, error: e.message }); }
        }
        if (p.startsWith("/api/v1/cmd/")) {
          const c = { cmd: p.slice("/api/v1/cmd/".length) };
          for (const [k, v] of url.searchParams) {
            if (k === "token") continue;
            c[k] = v === "true" ? true : v === "false" ? false : v !== "" && !Number.isNaN(Number(v)) && k !== "number" ? Number(v) : v;
          }
          try { exec(c, role); return json(res, 200, { ok: true }); }
          catch (e) { return json(res, 200, { ok: false, error: e.message }); }
        }
        if (role !== "admin") return json(res, 403, { ok: false, error: "forbidden" });
        if (p === "/api/fs") {
          let dir = url.searchParams.get("path") || "/home/operator";
          if (dir.length > 1) dir = dir.replace(/\/+$/, "");
          const names = FS_TREE[dir];
          if (!names) return json(res, 404, { error: "no such directory" });
          const entries = names.map((n) => {
            const path = dir === "/" ? `/${n}` : `${dir}/${n}`;
            const isDir = path in FS_TREE;
            return { name: n, path, dir: isDir, size: isDir ? 0 : 100000 + ((n.length * 7919) % 900) * 51200 };
          }).sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name));
          return json(res, 200, { path: dir, parent: dir === "/" ? null : dir.slice(0, dir.lastIndexOf("/")) || "/", entries });
        }
        if (p === "/api/waveform") {
          const file = url.searchParams.get("file") ?? "";
          if (!fileExists(file)) return json(res, 404, { error: "file not found" });
          const buckets = Math.max(10, Math.min(4000, Number(url.searchParams.get("buckets")) || 800));
          const r = find(allRows().find((x) => x.cue.kind.type === "audio" && x.cue.kind.file === file)?.cue.id);
          const duration = r?.cue.kind.file_duration ?? fakeDuration(file);
          let seed = file.length * 2654435761 >>> 0;
          const rnd = () => ((seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0) / 4294967296);
          const peaks = [];
          for (let i = 0; i < buckets; i++) {
            const x = i / buckets;
            const env = Math.min(1, x * 12, (1 - x) * 8) * (0.45 + 0.35 * Math.abs(Math.sin(x * 23 + file.length)) + 0.2 * Math.abs(Math.sin(x * 131)));
            const v = env * (0.6 + 0.4 * rnd());
            peaks.push([-Math.round(v * 0.95 * 1000) / 1000, Math.round(v * 1000) / 1000]);
          }
          return json(res, 200, { duration, channels: r?.cue.kind.channels ?? 2, sample_rate: 48000, peaks });
        }
        if (p === "/api/qr.svg") {
          const role2 = url.searchParams.get("role") === "view" ? "view" : "remote";
          res.writeHead(200, { "Content-Type": "image/svg+xml", "Cache-Control": "no-store" });
          return res.end(fakeQr(`${state.remote.urls[0]}/${role2}`));
        }
        if (p === "/api/diagnostics") {
          res.writeHead(200, { "Content-Type": "text/plain; charset=utf-8" });
          return res.end(`Show Master mock diagnostics\n\n${logBuf.map((l) => `${new Date(l.time_ms).toISOString()} ${l.level} ${l.message}`).join("\n")}\n\n${JSON.stringify(state, null, 2)}`);
        }
        return json(res, 404, { error: "not found" });
      }
      return serveStatic(res, p);
    } catch (e) {
      json(res, 500, { error: String(e?.message ?? e) });
    }
  });

  // ---- WebSocket ----
  const wss = new WebSocketServer({ noServer: true });
  server.on("upgrade", (req, socket, head) => {
    const url = new URL(req.url ?? "/", "http://x");
    const role = url.pathname === "/ws" ? tokenOf(req, url) : null;
    if (!role || Date.now() < wsBlockedUntil) {
      socket.write(`HTTP/1.1 ${role ? "503 Service Unavailable" : "401 Unauthorized"}\r\nConnection: close\r\n\r\n`);
      socket.destroy();
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => {
      const c = { ws, id: nextClientId++, role, addr: req.socket.remoteAddress ?? "", since_ms: Date.now(), agent: String(req.headers["user-agent"] ?? "") };
      clients.add(c);
      ws.send(JSON.stringify({ t: "hello", role, client_id: c.id, version: VERSION }));
      ws.send(JSON.stringify({ t: "state", rev, state: sent }));
      if (logBuf.length) ws.send(JSON.stringify({ t: "log", entries: logBuf.slice(-50) }));
      flush();
      ws.on("message", (data) => {
        let msg;
        try { msg = JSON.parse(String(data)); } catch { return; }
        if (msg.t === "ping") ws.send(JSON.stringify({ t: "pong", ts: msg.ts }));
        else if (msg.t === "cmd") {
          try {
            const result = exec(msg.cmd, role);
            ws.send(JSON.stringify({ t: "ack", id: msg.id, ok: true, result }));
          } catch (e) {
            ws.send(JSON.stringify({ t: "ack", id: msg.id, ok: false, error: String(e?.message ?? e) }));
          }
        }
      });
      ws.on("close", () => { clients.delete(c); flush(); });
      ws.on("error", () => {});
    });
  });

  validateErrors();
  sent = clone(state);
  await new Promise((ok, fail) => { server.once("error", fail); server.listen(port, "0.0.0.0", ok); });
  if (!quiet) {
    console.log(`Show Master MOCK backend on http://127.0.0.1:${port}  (admin)`);
    console.log(`  remote device simulation: http://localhost:${port}/remote  (control PIN 1234)  ·  http://localhost:${port}/view`);
  }

  return {
    port,
    adminToken,
    /** test hook: close every WebSocket and refuse new ones for `blockMs`. */
    dropSockets(blockMs = 0) {
      wsBlockedUntil = Date.now() + blockMs;
      for (const c of clients) c.ws.terminate();
    },
    getState: () => state,
    exec: (c) => exec(c, "admin"),
    async close() {
      clearInterval(tickTimer);
      for (const c of clients) c.ws.terminate();
      wss.close();
      await new Promise((ok) => server.close(ok));
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  startMock().catch((e) => { console.error(e); process.exit(1); });
}
