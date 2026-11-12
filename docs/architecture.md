# Architektur

## Ziele

Zuverlässigkeit, Timing-Genauigkeit und Bedienbarkeit unter Stress vor Feature-Menge. Daraus folgen
drei Grundsätze: **eine Zeitbasis**, **ein Command-Bus**, **nichts Blockierendes im Audio-Thread**.

## Komponenten

```
core (sm-core, sm-timecode)   Cue-Modell, Engine, Scheduler, Timecode-Mathematik   – rein, ohne I/O-Geräte
io   (sm-audio, sm-rtpmidi,   Audio-Mixer + Backends, MIDI, LTC, rtp-midi           – plattformabhängiges hinter Traits
      sm-io)
server (sm-server)            Command-Bus, Dokument/Undo, HTTP/WS, OSC
desktop (sm-desktop)          natives Fenster (egui): Edit-Modus, Live-Dashboard, Setup – Client am Command-Bus
ui   (ui/)                    Web-Oberfläche für Tablets (/remote, /view), im Binary eingebettet
```

Abhängigkeiten zeigen nur nach unten: `server → io → core`. `sm-core` kennt weder Audio-Geräte noch
Threads noch Uhren.

## Zeitbasis

Die einzige Uhr ist der Frame-Zähler des Audio-Renderers (`sm_audio::Controller::clock`). Die Engine
arbeitet in *logischer Zeit* = Audio-Clock + Look-ahead (ein Gerätepuffer + 12 ms). Jede Aktion, die
sie ausgibt, trägt den Frame, zu dem sie wirksam wird:

- `StartVoice { at }` – der Mixer teilt den Block an genau diesem Frame; zwei Voices mit demselben
  `at` starten auf demselben Sample.
- Pre-Wait, Post-Wait, Auto-Follow und Timecode-Trigger werden **berechnet**, nicht gepollt:
  `advance(until)` verarbeitet alle fälligen Ereignisse in Zeitreihenfolge mit ihrem exakten Frame.
  Ob in Schritten von 1 oder 100 000 Frames vorgerückt wird, ändert das Ergebnis nicht (Test
  `stepping_in_small_or_large_blocks_gives_identical_results`).
- Timecode ist ein `TcTransport { rate, running, anchor_sample, anchor_frame }`. LTC-Encoder (im
  Audio-Thread) und MTC-Generator rechnen ihre Position mit exakter Ganzzahlarithmetik aus der
  Sample-Position – Drift gegenüber der Audio-Clock ist konstruktionsbedingt ausgeschlossen.
- MIDI hat keinen samplegenauen Transport. Der MIDI-Sender bekommt für jede Nachricht einen
  Zeitpunkt, der aus dem Frame über `ClockMap` (Audio-Clock ↔ `Instant`) berechnet wird. Jitter:
  Betriebssystem-Scheduling, typisch < 1 ms.

## Threads

| Thread | Aufgabe | Regeln |
|---|---|---|
| Audio-Callback (cpal bzw. Null-Backend) | `Renderer::render`: Voices mischen, Master, LTC, Meter | keine Allokation, keine Locks, kein I/O (per Test mit zählendem Allokator belegt); Panics werden abgefangen → Stille + Fehlerflag |
| `sm-engine` | Dokument, Cue-Engine, Command-Bus, I/O-Hub; Takt 2 ms | einziger Schreiber des Zustands |
| Streamer | füllt die Ringpuffer der Voices von Platte (Decoding, Resampling) | |
| `sm-midi` | gibt terminierte MIDI-Nachrichten zum Zeitpunkt aus | |
| `sm-midi-in` | öffnet/schließt MIDI-Eingänge (Rescan 2 s); Callbacks stempeln nur die Zeit und schicken in einen Kanal | Engine liest den Kanal, wartet nie auf Treiber |
| `sm-audio-in` (+ Geräte-Thread) | LTC-Eingang: öffnet den Eingangsstrom, listet Eingabegeräte; der Callback schreibt einen Kanal in einen lock-freien Ring | Callback wie Audio: keine Allokation, kein Lock, kein I/O |
| rtp-midi (3) | UDP-Empfang, Session-Zustand, Clock-Sync | |
| `sm-net` (tokio, 2) | HTTP/WebSocket | redet mit der Engine nur über Kanäle |
| `sm-mdns` + `mDNS_daemon` | optionales Bonjour: Ankündigung (Web, RTP-MIDI), Suche nach RTP-MIDI-Sessions | liest nur den veröffentlichten Zustand; nur Link-lokales Multicast |
| Haupt-Thread | natives Fenster (egui), ~25 Bilder/s | liest den veröffentlichten Zustand, schickt Commands; blockiert nie Engine oder Audio |

Fällt das Netzwerk, ein Client oder der ganze Webserver aus, läuft die Show weiter: Engine und Audio
hängen nicht von `sm-net` ab.

## Command-Bus und Zustand

Alles ist ein Command (`{ "cmd": "go" }`): Hotkeys, Desktop-UI, iPad, HTTP-API, OSC. Drei Familien:

- **Transport** (`sm_core::engine::Command`) – ab Rolle *control*, in beiden Modi.
- **Edit** (`sm_server::doc::EditCommand`) – nur *admin*, nur im Edit-Modus; jede Änderung landet im
  Undo-Stack (Snapshot der Show).
- **System** (`SysCommand`) – Modus, Dateien, Pre-Flight, Clients.

Die Rollenprüfung sitzt in `Handle::command` und nochmals im Runtime-Thread, nicht in der UI.

Zustandsänderungen verlassen die Runtime auf drei Wegen: **State-Diff** (Pfad-Operationen auf einem
JSON-Dokument, mit Revisionsnummer), **Tick** (20 Hz: Uhren, laufende Cues, Pegel) und **Log**.
Nicht-Admins bekommen die PINs maskiert. Neue Clients erhalten einen Snapshot und danach Diffs; wer
zu langsam ist, wird mit einem neuen Snapshot resynchronisiert.

## Audio

`Controller` (Steuerseite) ⇄ `Renderer` (Echtzeitseite) über lock-freie Queues. Eine Voice liest aus
ihrem Ringpuffer (2 s) und mischt über eine Matrix Quelle×Ausgang; Pegel ändern sich nur über Rampen.
Die Engine öffnet die Voices der Cues, die das nächste GO feuert, schon vorab (**Standby**), damit
beim GO kein Dateizugriff nötig ist. Underruns erzeugen Stille, werden gezählt und im Log gemeldet.

Reihenfolge je Block: Voices → Master → Generatoren (LTC, unabhängig vom Master) → Meter → Clamp.

## Persistenz und Crash-Sicherheit

Showfile = lesbares JSON mit `schema_version`; ältere Versionen werden schrittweise migriert, neuere
abgelehnt. Speichern: Temp-Datei, `fsync`, `rename` (atomar); die Vorversion wandert nach `backups/`
(20 Stück). Autosave alle 5 s bei Änderungen in `<show>.autosave`; beim Öffnen wird ein neueres
Autosave zur Wiederherstellung angeboten. Medien sind relativ zum Showfile referenziert; „Sammeln“
kopiert sie nach `media/` und speichert BLAKE3-Hashes.

## Erweiterbarkeit (Nicht-Ziele, aber nicht verbaut)

- Neue Cue-Typen (Video, Script, HTTP; OSC als Vorbild): Variante in `CueKind` + Zweig in `Engine::start_action`
  + `Action`, die der I/O-Hub ausführt.
- Timecode-Eingang (umgesetzt, `sm_core::chase`): MTC/LTC werden zu Beobachtungen „Frame n begann bei
  Sample s“; der Chaser liefert daraus den `TcTransport`, Trigger und Sprungregeln funktionieren
  unverändert (siehe `decisions.md`, „Externe Eingänge“).
- Lichtsteuerung (Art-Net/sACN): weiterer Sink im I/O-Hub, terminiert wie MIDI.
- Systemweite Hotkeys (wenn das Fenster keinen Fokus hat): eigener Listener, der Commands auf den Bus legt.
