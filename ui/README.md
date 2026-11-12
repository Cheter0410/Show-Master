# Show Master – web UI

Single-page app (Vite + React + TypeScript, strict) for the three Show Master screens:

| Route     | Screen                                             | Role      |
|-----------|----------------------------------------------------|-----------|
| `/`       | desktop app: **Edit** mode and **Live** dashboard  | `admin`   |
| `/remote` | touch-first control surface (phones and tablets)   | `control` |
| `/view`   | read-only monitor (no control that sends commands) | `view`    |

The binding contract is [`../docs/remote-api.md`](../docs/remote-api.md). The UI renders what the role
granted by the server allows and treats `forbidden` answers as normal.

Offline by design: no CDN, no web fonts, no analytics, no request to anything but its own origin
(system font stack, inline SVG icons, locally generated PNG icons). Runtime dependencies: `react`, `react-dom`.

## Commands

```sh
npm ci
npm run dev        # Vite dev server on :5173, proxies /api and /ws to the backend
npm run build      # tsc --noEmit + vite build  ->  dist/  (hashed files under /assets/)
npm test           # vitest: time/timecode helpers, patch apply, hotkey matching/conflicts
npm run mock       # mock backend on :7400 (also serves dist/ if it was built)
npm run e2e        # Playwright smoke test against the mock + dist (build first)
npm run icons      # regenerate public/*.png icons (pure Node, no deps)
```

### Development

```sh
npm run mock                                   # terminal 1 – mock backend on 127.0.0.1:7400
npm run dev                                    # terminal 2 – http://localhost:5173/
SM_BACKEND=http://127.0.0.1:7400 npm run dev   # other backend (this is the default)
```

Through the dev proxy every request looks local to the backend, so you are `admin` on all routes.

### Build output

`dist/` is embedded into the Rust binary and served at `/`. `index.html` must be returned for `/`,
`/remote` and `/view`; everything else is a static file (`/assets/*` is content-hashed and can be cached
forever; `index.html`, `manifest.webmanifest` and the icons should be `no-cache`).

## Mock backend (`mock/server.mjs`)

Dev tool only (Node 22, `ws` is its single dependency). It implements enough of the contract to develop
and test the UI: session/login, WebSocket `hello/state/patch/tick/log/ack/pong/kicked`, an in-memory demo
show with every cue type, a rough playback simulation (pre-wait, running, auto-continue/-follow, groups,
fades that stop their target), transport and editing commands, naive undo/redo, fake pre-flight, a fake
file tree for `/api/fs`, synthetic waveforms and a fake QR image.

Role simulation: requests whose `Host` is `127.0.0.1` count as loopback (**admin**); any other host name –
including `localhost` – is treated as a remote device:

- <http://127.0.0.1:7400/> – desktop app as admin
- <http://localhost:7400/remote> – asks for the control PIN (`1234`)
- <http://localhost:7400/view> – view PIN is empty, logs in automatically

Environment: `SM_MOCK_PORT` (default 7400), `SM_MOCK_ALL_ADMIN=1` (everybody is admin).

## End-to-end smoke test (`e2e/smoke.mjs`)

Uses the **globally installed** Playwright + Chromium (not a dependency of this package; the script
resolves it through `npm root -g`). It starts the mock in-process on port 7461 (`SM_E2E_PORT`), serves
`dist/` and checks: edit screen and cue list, inspector round trip + validation + undo, setup dialog and
hotkey conflict refusal, deleting cues (toolbar button, Backspace, toast undo, right-click menu), the edit view
and the live dashboard on touch devices (360×640 … 1180×820: no horizontal scroll, GO / PANIC / Add / Delete
inside the viewport, targets ≥ 44 px, long-press context menu, full-screen setup on phones), pre-flight → live
dashboard, GO / debounce / Space hotkey, connection-lost banner and recovery, Live→Edit confirmation, `/remote`
PIN pad (wrong/right PIN), GO / Set NEXT / hold-to-panic, phone portrait and landscape layouts, `/view` at
phone … 1920×1080 without any control, server-side `forbidden`, kick, high contrast, and that **no request left
the mock's origin**.

Screenshots are written to `e2e/screenshots/` (git-ignored, regenerated on every run).

## Source layout

```
src/api.ts          contract types (state, tick, show, cue, commands) + HTTP helpers
src/store.ts        client store (useSyncExternalStore), log, toasts
src/patch.ts        immutable apply of WebSocket patch ops (structural sharing)
src/connection.ts   WebSocket: reconnect/backoff, rev-gap resync, ping/latency, send() -> ack promise
src/session.ts      /api/session, PIN login, token in sessionStorage, re-auth on reconnect
src/commands.ts     GO lock, panic double-tap, transport helpers, mode switching
src/hotkeys.ts      combo matching (+ Backspace / Cmd aliases), conflict detection, defaults
src/editing.ts      delete / duplicate / move-a-step actions shared by toolbar, inspector, context menu, hotkeys
src/time.ts         duration / clock / timecode / dB / SysEx helpers
src/cues.ts         cue tree helpers
src/platform.ts     optional Tauri file-drop adapter, screen wake lock, media-query hook
src/components/     shared: CueList, NextPanel, Clocks, Meters, Transport, Panels, fields, Modal, Menu, icons
src/desktop/        Toolbar, CueTable, Inspector, AudioInspector, Setup, FileBrowser, LiveDashboard
src/routes/         RemoteApp, ViewApp
```

## Behaviour notes

- **GO** fires on `pointerdown` and locks itself for `settings.go_debounce_ms` (hotkey and button share the lock).
- **PANIC**: desktop = immediate (double press if `panic_double_tap`); `/remote` = press and hold 0.5 s.
- **Hotkeys** come from `show.hotkeys`; ignored while typing (except Escape) and while a dialog is open.
  Space never scrolls and never "clicks" a focused button.
- **Connection loss**: full-width red banner; the socket reconnects with 250 ms → 2 s backoff, a `rev` gap
  or 5 s of silence forces a resync. Commands sent while offline are rejected, never queued.
- **Deleting cues** (Edit mode): toolbar *Delete* / *Duplicate* act on the selection, the inspector has the same
  buttons, the Delete key **and Backspace** (the Mac "delete" key) delete when no text field has focus, and a
  toast offers *Undo* (Ctrl/Cmd+Z works too; Cmd stands in for Ctrl when the Cmd combo is unbound). Right-click –
  or long-press on touch screens – a cue row for Set NEXT, Preview, Move up/down, Duplicate, Delete.
- **Screen sizes**: wide screens (≥ 1200 px) show everything as before. Tablets get File / Add-cue drop-down
  menus, fewer table columns (values stay in the inspector) and a collapsible inspector; phones get a compact
  toolbar with a ⋯ menu (bottom sheet), a single-column inspector, full-screen dialogs and a tabbed live
  dashboard with the transport block always at the bottom. Landscape phones put the list left and NEXT /
  transport (or the inspector) right. On touch screens (`pointer: coarse`) targets are ≥ 44 px, inputs use
  16 px text (no iOS zoom), and PANIC needs press-and-hold everywhere. Safe-area insets are respected.
- **High contrast**: `settings.high_contrast` sets `data-contrast="high"` on `<html>`.
