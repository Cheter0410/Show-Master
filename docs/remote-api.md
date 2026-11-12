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
| `GET /api/session` | – | `{ "role": "admin"|null, "token": "...", "view_needs_pin": bool, "version": "0.3.0" }`; loopback callers get an admin token |
| `POST /api/login` body `{ "pin": "1234", "role": "control"|"view" }` | – | `{ "token": "...", "role": "control" }` or `401` |
| `GET /ws?token=T` | any | WebSocket, see below |
| `GET /api/v1/state?token=T` | any | full state document (same as WS `state.state`, plus `tick`) – button feedback for Stream Deck/Companion |
| `POST /api/v1/command?token=T` body = command JSON | control | execute one command → `{ "ok": true }` / `{ "ok": false, "error": "…" }` |
| `GET|POST /api/v1/cmd/{name}?token=T&cue=ID&number=N&…` | control | shorthand: `name` = command name, query params become fields (numbers/bools parsed) e.g. `/api/v1/cmd/go`, `/api/v1/cmd/goto_number?number=14` |
| `GET /api/fs?token=T&path=/dir` | admin | `{ "path": "/dir", "parent": "/"|null, "entries": [{ "name", "path", "dir": bool, "size": n }] }` (audio files, showfiles and folders only; empty path = home dir) |
| `GET /api/waveform?token=T&file=PATH&buckets=800` | admin | `{ "duration": s, "channels": n, "sample_rate": n, "peaks": [[min,max], …] }` |
| `GET /api/qr.svg?role=control|view` | admin | QR code (SVG) of the remote URL |
| `GET /api/cuesheet.pdf`, `GET /api/cuesheet.csv` | view | cue sheet (A4 landscape PDF / semicolon CSV with BOM) |
| `POST /api/link` | link key (`X-ShowMaster-Link` header), backup role only | messages from the main computer: `{"kind":"ping"}`, `{"kind":"cmd","cmd":{…}}` (transport commands), `{"kind":"sync","show":{…},"mode":"live","playheads":{…},"initial":bool}` |
| `GET /api/diagnostics?token=T` | admin | plain-text diagnostics bundle (log, state, system info) |

A pre-shared token may also be passed as `Authorization: Bearer T`.
For Stream Deck / Companion use the control PIN itself as token: `?token=pin:1234`.

## WebSocket

Text frames, one JSON object each, discriminated by `t`.

### Server → client

```jsonc
{ "t": "hello", "role": "admin", "client_id": 3, "version": "0.3.0" }
{ "t": "state", "rev": 12, "state": { /* State */ } }          // full document; sent first and on resync
{ "t": "patch", "rev": 13, "ops": [ { "p": ["show","name"], "v": "New" }, { "p": ["errors","17"], "d": true } ] }
{ "t": "tick", /* Tick, ~20 Hz */ }
{ "t": "log", "entries": [ { "time_ms": 1731312000000, "level": "info"|"warn"|"error", "message": "GO cue 12" } ] }
{ "t": "ack", "id": 7, "ok": true, "result": null }             // or { "ok": false, "error": "…" }
{ "t": "pong", "ts": 123 }
{ "t": "kicked" }                                               // then the socket closes
```

`patch.ops`: `p` is a path of object keys / array indices into the state document. `v` sets the value at
that path (creating it; a numeric last segment replaces that array element), `d: true` deletes the key.
Arrays whose length changes are replaced as a whole value. A client
that sees a `rev` gap must reconnect (the server then sends a fresh `state`).

### Client → server

```jsonc
{ "t": "cmd", "id": 7, "cmd": { "cmd": "go" } }
{ "t": "ping", "ts": 123 }
```

## State document

```jsonc
{
  "version": "0.3.0",
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
                     "peers": [ { "name": "iPad", "addr": "192.168.1.20:5004", "state": "connected", "latency_ms": 1.2,
                                  "lost": 3, "recovered": 2 } ],
                     "discovered": [ { "name": "Studio Mac", "addr": "192.168.1.30:5004", "host": "studio-mac.local" } ] } },
  "remote": { "urls": ["http://192.168.1.10"], "port": 7400,
              "clients": [ { "id": 3, "role": "control", "addr": "192.168.1.20", "since_ms": 1731312000000, "agent": "Safari…" } ],
              "mdns": { "enabled": true, "active": true, "host": "showmaster.local", "url": "http://showmaster.local",
                        "rtp_advertised": true, "rtp_instance": "Show Master", "error": null } },
  "preflight": [ { "severity": "error"|"warning"|"info", "cue": 17, "message": "…" } ],
  "layouts": ["mono","stereo","lcr","quad","5.1","7.1"]
}
```

mDNS/Bonjour fields (optional comfort, see `decisions.md`):

- `midi.rtp.peers[].lost` – RTP packets from this peer detected as lost; `recovered` – MIDI messages
  synthesised from the peer's recovery journal after a loss (note-offs, controller values, program
  changes, recent note-ons).
- `midi.rtp.discovered` – other RTP-MIDI sessions found via mDNS (`_apple-midi._udp`), sorted by name.
  `addr` (`ip:port` of the control port) can be passed to `rtp_connect`. Empty while RTP-MIDI or
  `rtp_midi.advertise` is off.
- `remote.mdns` – `null` when the process runs without mDNS (tests, embedding). `enabled` mirrors
  `settings.remote.mdns`; `active` = the mDNS responder runs; `host` = advertised host name
  (`showmaster.local`, `showmaster-2.local` after a name conflict); `url` = base URL by name while the
  web interface is advertised (`null` otherwise – e.g. with `--local-only`); `rtp_advertised` = the
  RTP-MIDI session is advertised, `rtp_instance` = under which name (`Name (2)` if the name was taken); `error` = last problem (e.g. no multicast on this network). The IP
  URLs in `remote.urls` always work and remain the primary way in.
- Port: besides the configured port the server also listens on the standard web port 80 when it can;
  then `remote.urls` and `remote.mdns.url` carry no port (`http://192.168.1.10`). Otherwise they use the
  configured port. `GET /` from a network device redirects (302) to `/view` (its "Control" button links to `/remote`; a view login never counts there, the control PIN is asked); on this computer `/`
  is the full interface.

### Inputs (`state.inputs`)

```jsonc
"inputs": {
  "midi_ports": ["USB Pad", "IAC Bus 1", "rtp:Show Master"],  // MIDI inputs that can be listened to
  "midi_open": ["USB Pad", "rtp:Show Master"],                // open right now
  "midi_count": 12,                                           // messages received (not MTC / clock)
  "midi_last": { "seq": 12, "port": "USB Pad", "text": "Note on ch 1 note 60 vel 100",
                 "kind": "note_on"|"note_off"|"control_change"|"program_change"|"msc"|"other",
                 "channel": 1, "number": 60, "value": 100 } | null,   // for "learn" and troubleshooting
  "audio_inputs": [ { "id": "…", "name": "…", "channels": 2, "is_default": true } ] | null,  // scanned only while LTC input is selected
  "ltc_open": false, "ltc_level_db": -18.5 | null,            // LTC capture stream, peak of the last second
  "mtc_rate": "25 fps" | null,                                // rate announced by the incoming MTC
  "ltc_rate": "25" | null,                                    // rate detected on the LTC input
  "tc_speed": 1.0 | null,                                     // chased source speed (1.02 = 2 % fast), while locked
  "tc_status": "generator"                                    // same values as tick tc.status
}
```

`inputs` is republished at most every 200 ms while MIDI arrives.

## Tick (fast-changing values)

```jsonc
{
  "t": "tick", "frame": 123456, "wall_ms": 1731312000000,
  "show_elapsed": 12.5 | null,
  "tc": { "position": "01:00:12:10", "running": true, "rate": "25", "ltc": true, "mtc": false,
          "source": "generator"|"mtc"|"ltc",
          "status": "generator"|"no_signal"|"locking"|"locked"|"freewheel"|"lost"|"stopped"|"held" },
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
  "schema_version": 3, "name": "My Show", "next_id": 20, "notes": "stage manager memo",
  "settings": { "go_debounce_ms": 250, "panic_fade_ms": 0, "stop_fade_ms": 500, "panic_double_tap": false,
                "double_tap_ms": 600, "quit_lock": true, "high_contrast": false,
                "audio": { "backend": null, "device": null, "sample_rate": 48000, "buffer_frames": 256, "channels": 2 },
                "remote": { "enabled": true, "port": 7400, "control_pin": "1234", "view_pin": "", "osc_port": 0, "mdns": true },
                "timer_alarms": [ { "at": 3600.0, "label": "Interval" } ],
                "panic_send": [ { "kind": "osc", "host": "10.0.0.9", "port": 7000, "address": "/blackout", "args": [] },
                                { "kind": "midi", "port": "Desk", "message": { "msg": "program_change", "channel": 1, "program": 99 } } ] },
  "patch": { "outputs": [ { "name": "L", "channel": 0 } ], "groups": [ { "name": "stereo", "layout": "stereo", "outputs": [0, 1] } ] },
  "midi_ports": [ { "name": "Desk", "device": "IAC Bus 1" } ],
  "rtp_midi": { "enabled": false, "session_name": "Show Master", "port": 5004, "connect_to": ["192.168.1.20:5004"], "journal": false,
                "advertise": true },
  "timecode": { "rate": "23.976"|"24"|"25"|"29.97df"|"29.97"|"30", "start": "01:00:00:00", "mode": "freerun"|"transport",
                "ltc_enabled": false, "ltc_channel": 1, "ltc_level_db": -12.0, "mtc_enabled": false, "mtc_port": "",
                "source": "generator"|"external",
                "input": { "kind": "mtc"|"ltc", "mtc_port": "",            // alias / device / "rtp:…"; "" = any listened port
                           "ltc_device": null, "ltc_channel": 0,           // audio input id (null = default), zero-based channel
                           "lock_frames": 4, "freewheel_ms": 2000, "latency_ms": 0 } },
  "midi_input": { "mode": "none"|"all"|"selected", "ports": ["Desk", "rtp:Show Master"],
                  "mappings": [ { "port": "", "kind": "note_on"|"control_change"|"program_change", "channel": 0 /* 0 = any */,
                                  "number": 60, "min_value": 1,
                                  "action": "go"|"stop_all"|"panic"|"toggle_pause"|"pause_all"|"resume_all"|"next"|"previous"|"reset"|"goto_number"|"start_cue",
                                  "cue_number": "" } ],
                  "msc": { "enabled": false, "device_id": 1 /* 127 = any */, "command_format": 16 /* 0x10 sound, 127 = all */ } },
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
            "fade_curve": "smooth"|"linear"|"equal_power"|"s_curve", "loop_count": 1 /* 0 = infinite */,
            "routing": [ { "src": 0, "out": 0, "db": 0.0 } ] /* empty = default routing */,
            "file_rate": 44100 /* cached */, "vamp": { "start": 4.0, "end": 6.0 } /* optional: loops until released */ }
  // { "type": "fade", "target": 5, "duration": 3.0, "curve": "equal_power", "gain_db": -120.0|null, "crosspoints": [], "stop_target": true }
  // { "type": "midi", "port": "Desk", "message": { "msg": "note_on", "channel": 1, "note": 60, "velocity": 100 } }
  //      msg: note_on | note_off | control_change{channel,controller,value} | program_change{channel,program}
  //           | pitch_bend{channel,value} | sysex{hex} | msc{device_id,command_format,command,q_number,q_list,q_path}
  //      msc command: go stop resume timed_go load set fire all_off restore reset go_off
  // { "type": "osc", "host": "10.0.0.5", "port": 53000, "address": "/cue/1/start", "args": [1, 0.5, true, "text"] }
  //      UDP, OSC 1.0 message; args: integer -> i, number with fraction -> f, true/false -> T/F, string -> s
  // { "type": "http", "method": "GET"|"POST"|"PUT"|"PATCH"|"DELETE", "url": "http://10.0.0.5/api/x", "body": "", "content_type": "application/json" }
  //      plain HTTP/1.1 (no https), sent from a worker thread; failures are logged
  // { "type": "timecode", "tc": { "action": "start" | "stop" | "pause" | "resume" } | { "action": "start_from", "from": "…" } | { "action": "relocate", "to": "…" } }
  // { "type": "control", "action": "start"|"stop"|"pause"|"resume"|"load_to_time"|"arm"|"disarm"|"go_to"|"release_loop"|"reset", "target": 5, "time": 0.0 }
  // { "type": "group", "mode": "parallel"|"playlist"|"random", "children": [ /* Cue */ ] }
  // { "type": "memo" }
}
```

All `midi_input` and `timecode.source` / `timecode.input` fields are optional in a showfile (defaults
above); shows saved before they existed load unchanged. Schema 3 added OSC cues and
`settings.panic_send` (messages sent right after PANIC's All Notes Off); v2 files load unchanged,
older program versions refuse v3 files with "made by a newer version".

## External inputs

**MIDI input.** `midi_input.mode` chooses the system input ports that are opened: `none` (default),
`all` (every port except "Through" loopbacks, plus RTP-MIDI peers) or `selected` (`ports`, aliases or
device names, `rtp:<session>` = RTP-MIDI peers). Ports are opened on a background thread and reopened
when a device comes back. Incoming messages are handled on the engine thread:

- **Mapping** (`mappings`, empty by default): note-on (velocity ≥ `min_value`, velocity 0 never),
  control change (fires when the value rises to `min_value` or above; a repeated high value counts as a
  new press after 250 ms) or program change, on `channel` (0 = any) from `port` (empty = any listened
  port) → the transport command `action` (`goto_number` / `start_cue` use `cue_number`). Executed as
  commands on the bus with role `control` and origin `midi`; every mapped message is logged.
- **MSC** (`msc.enabled`, off by default): messages to `device_id` (or all-call 127; `device_id` 127
  answers every id) with `command_format` (or 0x7F all-types). `GO` with a cue number = go to that cue
  and fire it (a cue inside a group is started directly), `GO` without = `go`; `STOP` = stop that cue /
  `stop_all`; `RESUME` = resume that cue / `resume_all`; `ALL_OFF` = `stop_all`; `RESET` = `reset`;
  `LOAD` = playhead to that cue. Other commands and unknown cue numbers are logged and ignored.

**Timecode input / chase.** With `timecode.source: "external"` the engine's timecode follows MTC (from
`input.mtc_port`) or LTC (audio input `input.ltc_device`, channel `input.ltc_channel`; the frame rate
is detected and converted to `timecode.rate`). It locks after `lock_frames` consistent frames, keeps running on its own for
`freewheel_ms` when the input stops and then stops (`lost`); an MTC Full Frame or a repeated LTC frame
stops it at that position. Jumps of the source apply each list's `tc_jump` policy. While chasing,
`tc_start`/`tc_resume` and `tc_stop`/`tc_pause` (and timecode cues) only resume or suspend following
(`held`; PANIC also holds), `tc_relocate` is ignored, and LTC/MTC outputs send the chased timecode.
A source at another rate is converted by timecode value; speed deviations are followed within a quarter
frame (`inputs.tc_speed`); a source running backwards puts the status to `reverse` (clock stopped at the
source position, nothing fires) until it runs forward and locks again. `input.latency_ms` (-500..500)
shifts incoming frames earlier to compensate the input path.

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
| `release_loop` | `cue` | release a vamping audio cue: it leaves its loop at the next loop end (at least 0.5 s ahead, sample-exact crossover) and plays on; `tick.active[].vamping` is true until then |
| `load_cue` | `cue`, `time` | load to time (s) |
| `arm_cue` | `cue`, `armed` | |
| `reset` | | stop all, playhead to top, clear loads/errors, show clock reset |
| `reset_show_clock` | | |
| `tc_start` (`from?`), `tc_stop`, `tc_pause`, `tc_resume`, `tc_relocate` (`to`) | | timecode generator |
| `set_master` | `db` | master level (Setup > Audio; the Live view only meters) |
| `midi_panic` | | |
| `reset_clip` | | clear clip indicators |
| `set_notes` | `text` | stage manager memo |

### Editing (role `admin`, rejected in live mode - except `update_cue` / `update_cues` patches that only change `kind.gain_db`, `kind.routing` or `notes`; level and routing changes ramp a playing cue in 30 ms, and a stream of them on the same cues within 2 s is one undo step)

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
| `save_show` | `path?`, `folder?`, `media?` | `path` required for a never-saved show. `folder: true` saves into `<dir>/<name>/<name>.smshow` (not nested again when `<dir>` is already that folder); `media: "copy"` or `"move"` brings every audio file into its `media/` folder. A show still called "Untitled Show" takes the file name. |
| `collect_media` | | copy media into `<show dir>/media`, store hashes |
| `recover` | `accept`: bool | load or discard the autosave |
| `kick_client` | `id` | disconnect a remote client |
| `create_pairing` | `role` (`control`/`view`), `name?` | pair a device; result `{ id, token, role, name }`. Open `/<page>?pair=<token>` on the device (QR code): it stores the token and is logged in without a PIN. Never `admin`. Kept per machine in `pairings.json`. |
| `revoke_pairing` | `id` | revoke one pairing (its clients are disconnected) |
| `revoke_all_pairings` | – | revoke every pairing |
| `set_link` | `role` (`off`/`main`/`backup`), `peer` (main: backup address `host[:port]`), `key` (shared, ≥ 4 chars; empty = keep) | main / backup computer link of this machine (`link.json`) |
| `rtp_connect` | `addr` | invite an RTP-MIDI peer now (`ip:port`, e.g. from `midi.rtp.discovered`) |
| `clear_log` | | |

Errors come back as `{ "ok": false, "error": "…" }`; permission problems use `forbidden`, live-mode
rejections `live_mode`.

## OSC (optional)

If `settings.remote.osc_port` is non-zero, UDP OSC messages `/showmaster/<cmd>` are accepted from the local
network with the same names as the transport commands, e.g. `/showmaster/go`, `/showmaster/goto_number "14"`,
`/showmaster/start_cue 5`. OSC has no authentication – enable only on trusted show networks.

Short addresses (QLab-style, cue *numbers* instead of ids): `/go`, `/stop` (stop all), `/panic`, `/pause`,
`/resume`, `/toggle_pause`, `/reset`, `/next`, `/previous`, `/master -6.0`, `/playhead/12`,
`/cue/12/start|stop|pause|resume|arm|disarm|release`, `/cue/12/load 4.5` (seconds), `/tc/start|stop|pause|resume`,
`/tc/locate "01:00:10:00"`. Unknown addresses and numbers are ignored.
