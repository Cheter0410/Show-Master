# Changelog

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
