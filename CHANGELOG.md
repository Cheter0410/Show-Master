# Changelog

## 0.5.5

- PANIC no longer fills the log with "RTP-MIDI session is not enabled" (about a hundred lines each
  time): the panic messages only go to the RTP-MIDI session when it is on.

## 0.5.4

- Outputs patched on device channels beyond the open ones now play: the number of output channels
  (Settings › Audio) is raised to what the patch uses – when the patch changes and when a show opens.
  Before, choosing a big interface kept the 2 channels set earlier, so outputs 3 and up stayed silent.

## 0.5.3

- Audio files with more channels than the audio device (e.g. a 6-channel WAV on a stereo output) play
  again: routes to outputs the device does not have are skipped instead of failing the cue with
  "crosspoint … is outside the 6x2 matrix". The same for routes from channels the file does not have.

## 0.5.2

- Starting is faster on computers with many audio devices: the device list is asked for once at a time
  and reused for a second, so several engines at once no longer block each other in CoreAudio.
- Tests: all UI and input tests pass again with a plain `cargo test --workspace` (in parallel). The test
  engine waits until the show is loaded, gets more time under load, and test input devices are used from
  the start instead of opening and closing the real ones first.

## 0.5.1

- Setup wizard, Audio step: "Apply audio settings" and "Revert" are now at the top, above the device
  list, so they are visible without scrolling (also in Setup › Audio).
- Setup › General: "Open the last show at start" explains that it skips the start screen.

## 0.5.0

- Start screen like Lantern's, in Show Master's green: New show, Open show, Open show package and the
  recent shows over a banner of the logo's cue list. It shows when Show Master starts without a show
  (never in kiosk mode); *File › Start screen* leads back to it.
- New show asks for a name and a place and makes the show folder right away
  (`<place>/<name>/<name>.smshow`, default: Documents/Show Master).
- A setup wizard runs after New show: audio interface, outputs/patch, timecode, remote (tablet, PIN,
  QR codes) and a summary; *Skip setup* opens the show at once. The steps are the same pages as in
  Setup.
- UI tests for the start screen and the wizard; `SM_SHOTS=<folder>` writes pictures of every step.

## 0.4.3

- Stagehand is now called Showtime Suite: the install scripts put Show Master into the renamed shared
  folder (`/Applications/Showtime Suite`, `~/.local/share/showtime-suite/apps`, Windows
  `…\Programs\Showtime Suite\Show Master`) and move the old `Stagehand` folder over once, with the
  menu entries and commands that point into it.

## 0.4.2

- CLAUDE.md: how two people (and their Claude Code sessions) work on this repository – straight on
  `main`, pull before work and before push, version and changelog with every change, what to do on
  conflicts.

## 0.4.1

- The checkout folder is `show-master` everywhere (Linux instructions too), as Stagehand expects it.
- Development happens on the `main` branch.

## 0.4.0

- New logo: the cue stack with the green NEXT cue (app icon, window, tray and tablet icons).
- Groups: a group's colour tints every cue inside it, and a bracket in its colour shows what belongs
  to the group.
- Group mode "List": a group that only bundles cues - nothing fires by itself, the playhead steps
  through its cues and each takes its own GO.
- Waveform zoom: Ctrl / Cmd + wheel or pinch, Shift + wheel or sideways scrolling to move,
  + / − / Fit buttons and an overview bar; the view follows the play cursor.
- Waveform: a click sets the cue's playhead (a playing cue jumps there); the in / out points move by
  their handles; the loop is drawn, moved and resized by dragging in the strip below the wave.
- A loop can take the timecode with it: the timecode jumps back with every pass.
- macOS: the audio device is switched to the show's sample rate (fixes crackles and dropouts when
  the device ran at another rate).
- Mac builds compile once, for this Mac only; install scripts use their newest version after updating;
  builds can be uploaded to the release right away.

## 0.3.0

- Smoother fades: new default curve "Smooth".
- Loops (vamps) in audio cues, released in the show (Live button, Control cue, OSC).
- Save as: the show gets its own folder, media copied or moved into it.
- Live: gain and routing changes reach playing cues; meters only (master level in Setup > Audio).
- Pause can pause the timecode too; cue colours tint the whole row; tablet UI restyled.
- Steadier audio streaming; real CPU display.

## 0.2.0

- Tablets: showmaster.local, QR pairing, stage manager view, backup computer with automatic takeover.
- OSC and HTTP cues, cue sheet export, show packages, templates, kiosk mode, background mode.

## 0.1.0

- First version: cue engine, audio playback with routing, LTC/MTC/RTP-MIDI, edit and live mode,
  tablet remote, Stream Deck API.
- Installers for macOS and Windows, `.deb` for Linux, in-app updater.
