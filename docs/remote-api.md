# Show Master – local API (HTTP + WebSocket)

One command bus serves everything: desktop UI, iPad remote, hotkeys, Stream Deck / Bitfocus Companion.
All traffic stays on the local machine / local network. No external hosts are ever contacted.

Default port: `7400` (show setting `settings.remote.port`).

## Roles

| Role      | How obtained                                             | May do                                   |
|-----------|----------------------------------------------------------|------------------------------------------|
| `admin`   | automatically for connections from loopback (the desktop UI) | everything incl. editing, files, setup |
| `control` | `POST /api/login` with the control PIN                   | transport commands, `set_notes`          |
| `view`    | `POST /api/login` with the view PIN (or no PIN if empty) | nothing – read only                      |

Roles are enforced on the server for every command, never only in the UI.

## HTTP

| Method & path | Role | Purpose |
|---|---|---|
| `GET /`, `/remote`, `/view` | – | the web UI (single-page app, all assets embedded in the binary) |
| `GET /api/session` | – | `{ "role": "admin"|null, "token": "...", "view_needs_pin": bool, "version": "0.1.0" }`; loopback callers get an admin token |
| `POST /api/login` body `{ "pin": "1234", "role": "control"|"view" }` | – | `{ "token": "...", "role": "control" }` or `401` |
| `GET /ws?token=T` | any | WebSocket, see below |
| `GET /api/v1/state?token=T` | any | full state document (same as WS `state.state`, plus `tick`) – button feedback for Stream Deck/Companion |
| `POST /api/v1/command?token=T` body = command JSON | control | execute one command → `{ "ok": true }` / `{ "ok": false, "error": "…" }` |
| `GET|POST /api/v1/cmd/{name}?token=T&cue=ID&number=N&…` | control | shorthand: `name` = command name, query params become fields (numbers/bools parsed) e.g. `/api/v1/cmd/go`, `/api/v1/cmd/goto_number?number=14` |
| `GET /api/fs?token=T&path=/dir` | admin | `{ "path": "/dir", "parent": "/"|null, "entries": [{ "name", "path", "dir": bool, "size": n }] }` (audio files, showfiles and folders only; empty path = home dir) |
| `GET /api/waveform?token=T&file=PATH&buckets=800` | admin | `{ "duration": s, "channels": n, "sample_rate": n, "peaks": [[min,max], …] }` |
| `GET /api/qr.svg?role=control|view` | admin | QR code (SVG) of the remote URL |
| `GET /api/diagnostics?token=T` | admin | plain-text diagnostics bundle (log, state, system info) |

A pre-shared token may also be passed as `Authorization: Bearer T`.
For Stream Deck / Companion use the control PIN itself as token: `?token=pin:1234`.

## WebSocket

Text frames, one JSON object each, discriminated by `t`.

### Server → client

```jsonc
{ "t": "hello", "role": "admin", "client_id": 3, "version": "0.1.0" }
{ "t": "state", "rev": 12, "state": { /* State */ } }          // full document; sent first and on resync
{ "t": "patch", "rev": 13, "ops": [ { "p": ["show","name"], "v": "New" }, { "p": ["errors","17"], "d": true } ] }
{ "t": "tick", /* Tick, ~20 Hz */ }
{ "t": "log", "entries": [ { "time_ms": 1731312000000, "level": "info"|"warn"|"error", "message": "GO cue 12" } ] }
{ "t": "ack", "id": 7, "ok": true, "result": null }             // or { "ok": false, "error": "…" }
{ "t": "pong", "ts": 123 }
{ "t": "kicked" }                                               // then the socket closes
```

`patch.ops`: `p` is a path of object keys / array indices into the state document. `v` sets the value at
that path (creating it), `d: true` deletes the key. Arrays are always replaced as a whole value. A client
that sees a `rev` gap must reconnect (the server then sends a fresh `state`).

### Client → server

```jsonc
{ "t": "cmd", "id": 7, "cmd": { "cmd": "go" } }
{ "t": "ping", "ts": 123 }
```

## State document

```jsonc
{
  "version": "0.1.0",
  "mode": "edit" | "live",
  "show": { /* the showfile document, see below */ },
  "show_path": "/path/show.smshow" | null,
  "dirty": false, "can_undo": false, "can_redo": false,
  "recovery_available": false,
  "playheads": { "1": 5 },                 // list id -> cue id | null
  "errors": { "17": "file not found" },    // broken cues
  "loaded": { "9": 30.0 },                 // cues loaded to a time (s)
  "paused_all": false,
  "master_db": 0.0,
  "audio": { "backend": "cpal", "device": "…", "sample_rate": 48000, "channels": 2, "buffer_frames": 256,
             "is_null": false, "error": null,
             "devices": [ { "backend": "cpal", "id": "…", "name": "…", "max_output_channels": 8,
                            "sample_rates": [44100, 48000], "is_default": true } ] },
  "midi": { "ports": ["IAC Bus 1", "rtp:Show Master"],
            "rtp": { "enabled": true, "name": "Show Master", "port": 5004,
                     "peers": [ { "name": "iPad", "addr": "192.168.1.20:5004", "state": "connected", "latency_ms": 1.2 } ] } },
  "remote": { "urls": ["http://192.168.1.10:7400"], "port": 7400,
              "clients": [ { "id": 3, "role": "control", "addr": "192.168.1.20", "since_ms": 1731312000000, "agent": "Safari…" } ] },
  "preflight": [ { "severity": "error"|"warning"|"info", "cue": 17, "message": "…" } ],
  "layouts": ["mono","stereo","lcr","quad","5.1","7.1"]
}
```

## Tick (fast-changing values)

```jsonc
{
  "t": "tick", "frame": 123456, "wall_ms": 1731312000000,
  "show_elapsed": 12.5 | null,
  "tc": { "position": "01:00:12:10", "running": true, "rate": "25", "ltc": true, "mtc": false },
  "active": [ { "cue": 5, "list": 1, "phase": "pre_wait"|"running"|"post_wait", "paused": false,
                "elapsed": 1.2, "duration": 30.0 | null, "from_tc": false } ],
  "next_auto": 3.2 | null,                 // seconds until the next automatically fired cue
  "meters": { "peak": [0.5, 0.4], "rms": [0.2, 0.2], "clip": [false, false] },  // linear, per device channel
  "cpu": 0.03, "underruns": 0, "voices": 2,
  "alarms": ["Interval in 5 min"]          // show-timer alarms currently due (shown for 10 s)
}
```

## Showfile document (`state.show`)

```jsonc
{
  "schema_version": 2, "name": "My Show", "next_id": 20, "notes": "stage manager memo",
  "settings": { "go_debounce_ms": 250, "panic_fade_ms": 0, "stop_fade_ms": 500, "panic_double_tap": false,
                "double_tap_ms": 600, "quit_lock": true, "high_contrast": false,
                "audio": { "backend": null, "device": null, "sample_rate": 48000, "buffer_frames": 256, "channels": 2 },
                "remote": { "enabled": true, "port": 7400, "control_pin": "1234", "view_pin": "", "osc_port": 0 },
                "timer_alarms": [ { "at": 3600.0, "label": "Interval" } ] },
  "patch": { "outputs": [ { "name": "L", "channel": 0 } ], "groups": [ { "name": "stereo", "layout": "stereo", "outputs": [0, 1] } ] },
  "midi_ports": [ { "name": "Desk", "device": "IAC Bus 1" } ],
  "rtp_midi": { "enabled": false, "session_name": "Show Master", "port": 5004, "connect_to": ["192.168.1.20:5004"], "journal": false },
  "timecode": { "rate": "23.976"|"24"|"25"|"29.97df"|"29.97"|"30", "start": "01:00:00:00", "mode": "freerun"|"transport",
                "ltc_enabled": false, "ltc_channel": 1, "ltc_level_db": -12.0, "mtc_enabled": false, "mtc_port": "" },
  "hotkeys": { "Space": "go", "Escape": "panic", "Ctrl+KeyZ": "undo" },   // KeyboardEvent.code with Ctrl+/Shift+/Alt+/Meta+ prefixes -> action
  "cue_lists": [ { "id": 1, "name": "Main", "tc_jump": "fire"|"skip"|"chase", "cues": [ /* Cue */ ] } ]
}
```

Cue:

```jsonc
{
  "id": 5, "number": "12.5", "name": "Intro", "notes": "", "color": "" | "red" | "orange" | "yellow" | "green" | "blue" | "purple",
  "pre_wait": 0.0, "post_wait": 0.0,
  "continue_mode": "do_not_continue" | "auto_continue" | "auto_follow",
  "armed": true, "tc_trigger": "01:00:10:00" | null,
  "kind": { "type": "audio", "file": "media/intro.wav", "hash": null, "file_duration": 93.2, "channels": 2,
            "start": 0.0, "end": null, "gain_db": 0.0, "fade_in": 0.0, "fade_out": 0.0,
            "fade_curve": "linear"|"equal_power"|"s_curve", "loop_count": 1 /* 0 = infinite */,
            "routing": [ { "src": 0, "out": 0, "db": 0.0 } ] /* empty = default routing */ }
  // { "type": "fade", "target": 5, "duration": 3.0, "curve": "equal_power", "gain_db": -120.0|null, "crosspoints": [], "stop_target": true }
  // { "type": "midi", "port": "Desk", "message": { "msg": "note_on", "channel": 1, "note": 60, "velocity": 100 } }
  //      msg: note_on | note_off | control_change{channel,controller,value} | program_change{channel,program}
  //           | pitch_bend{channel,value} | sysex{hex} | msc{device_id,command_format,command,q_number,q_list,q_path}
  //      msc command: go stop resume timed_go load set fire all_off restore reset go_off
  // { "type": "timecode", "tc": { "action": "start" | "stop" | "pause" | "resume" } | { "action": "start_from", "from": "…" } | { "action": "relocate", "to": "…" } }
  // { "type": "control", "action": "start"|"stop"|"pause"|"resume"|"load_to_time"|"arm"|"disarm"|"go_to"|"reset", "target": 5, "time": 0.0 }
  // { "type": "group", "mode": "parallel"|"playlist"|"random", "children": [ /* Cue */ ] }
  // { "type": "memo" }
}
```

## Commands

Every command is `{ "cmd": "<name>", …fields }`.

### Transport (role `control` and up; work in edit and live mode)

| cmd | fields | |
|---|---|---|
| `go` | `list?` | fire the playhead cue (debounced) |
| `stop_all` | | stop everything with `stop_fade_ms` |
| `panic` | | stop everything immediately / `panic_fade_ms`, MIDI all-notes-off |
| `pause_all`, `resume_all`, `toggle_pause` | | |
| `next`, `previous` | `list?` | move playhead without firing |
| `set_playhead` | `cue` (id or null), `list?` | |
| `goto_number` | `number` | "Go to cue 14" – moves the playhead |
| `start_cue`, `stop_cue`, `pause_cue`, `resume_cue`, `reset_cue` | `cue` | single cue; `start_cue` = preview, does not move the playhead |
| `load_cue` | `cue`, `time` | load to time (s) |
| `arm_cue` | `cue`, `armed` | |
| `reset` | | stop all, playhead to top, clear loads/errors, show clock reset |
| `reset_show_clock` | | |
| `tc_start` (`from?`), `tc_stop`, `tc_pause`, `tc_resume`, `tc_relocate` (`to`) | | timecode generator |
| `set_master` | `db` | master fader |
| `midi_panic` | | |
| `reset_clip` | | clear clip indicators |
| `set_notes` | `text` | stage manager memo |

### Editing (role `admin`, rejected in live mode)

| cmd | fields | |
|---|---|---|
| `add_cue` | `kind` (type name), `list?`, `after?` (cue id), `parent?` (group id), `file?` (path, for audio), `target?` | result: `{ "cue": id }` |
| `update_cue` | `cue`, `patch` | JSON merge patch on the cue (RFC 7396; arrays replace) |
| `update_cues` | `cues` [ids], `patch` | bulk edit |
| `delete_cues` | `cues` | |
| `duplicate_cues` | `cues` | result `{ "cues": [new ids] }` |
| `move_cues` | `cues`, `list?`, `parent?` (group id or null), `index` | reorder / move in or out of groups |
| `renumber` | `cues`, `start` (number), `step` (number) | |
| `group_cues` | `cues` | wrap in a new group |
| `add_list` (`name`), `update_list` (`list`, `patch`), `delete_list` (`list`) | | |
| `update_show` | `patch` | merge patch on the show document (not `cue_lists`): settings, patch, midi_ports, rtp_midi, timecode, hotkeys, name, notes |
| `apply_layout` | `layout` | replace the output patch by a layout template |
| `undo`, `redo` | | unlimited within the session |

### Application (role `admin`)

| cmd | fields | |
|---|---|---|
| `set_mode` | `mode`: `edit`/`live`, `force?` | Edit→Live runs the pre-flight check (result in `state.preflight`). Live→Edit while cues run fails with error `cues_running` unless `force: true`. |
| `run_preflight` | | |
| `new_show` | `force?` | fails with `unsaved_changes` unless forced |
| `open_show` | `path`, `force?` | |
| `save_show` | `path?` | `path` required for a never-saved show |
| `collect_media` | | copy media into `<show dir>/media`, store hashes |
| `recover` | `accept`: bool | load or discard the autosave |
| `kick_client` | `id` | disconnect a remote client |
| `rtp_connect` | `addr` | invite an RTP-MIDI peer now |
| `clear_log` | | |

Errors come back as `{ "ok": false, "error": "…" }`; permission problems use `forbidden`, live-mode
rejections `live_mode`.

## OSC (optional)

If `settings.remote.osc_port` is non-zero, UDP OSC messages `/showmaster/<cmd>` are accepted from the local
network with the same names as the transport commands, e.g. `/showmaster/go`, `/showmaster/goto_number "14"`,
`/showmaster/start_cue 5`. OSC has no authentication – enable only on trusted show networks.
